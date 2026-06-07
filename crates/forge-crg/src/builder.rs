//! # Code-Review Graph: Builder and Incremental Updates
//!
//! Walks a repository directory, extracts structural symbol and dependency metadata,
//! resolves calls/imports/implements/tests relationships into edges, and updates
//! the graph incrementally on file saves.
//!
//! ## Input
//! - Project repository root path
//! - Save events carrying updated file paths
//!
//! ## Output
//! - `SQLite` database rows in symbols, edges, `call_sites`, and imports tables
//! - Refreshed connectivity scores for affected nodes
//!
//! ## Related
//! - `forge-ast::parser` — parses source files for symbol extraction
//! - `forge-crg::schema` — establishes `SQLite` schema
//! - `forge-crg::connectivity` — calculates connectivity scores

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use uuid::Uuid;
use yantra_core::SymbolId;

use crate::error::CrgResult;
use crate::schema::{create_crg_schema, EdgeType};

/// Walks a repository directory, extracts symbols, resolves relationships into
/// edges, and incrementally re-indexes individual files on save.
pub struct GraphBuilder {
    connection: Connection,
}

impl GraphBuilder {
    /// Creates a new `GraphBuilder` wrapping the given `SQLite` connection.
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    /// Returns a reference to the underlying `SQLite` connection.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Consumes the builder and returns the underlying `SQLite` connection.
    pub fn into_connection(self) -> Connection {
        self.connection
    }

    /// Walks `repository_root`, indexes all supported source files, resolves
    /// call/import/implements relationships into edges, and computes connectivity scores.
    ///
    /// # Errors
    ///
    /// Returns an error if schema creation, file walking, symbol extraction, or
    /// any `SQLite` operation fails.
    pub fn build_from_repo(&self, repository_root: &Path) -> CrgResult<()> {
        create_crg_schema(&self.connection)?;

        let mut language_files_list = Vec::new();
        collect_supported_files(repository_root, &mut language_files_list)?;

        self.connection.execute("BEGIN TRANSACTION", [])?;

        let result = (|| -> CrgResult<()> {
            let mut insert_call_stmt = self.connection.prepare_cached(
                "INSERT OR REPLACE INTO call_sites (id, caller_symbol_id, callee_name, callee_symbol_id, line) VALUES (?1, ?2, ?3, ?4, ?5)"
            )?;

            let mut insert_import_stmt = self.connection.prepare_cached(
                "INSERT OR REPLACE INTO imports (id, file_id, module_path, imported_names) VALUES (?1, ?2, ?3, ?4)"
            )?;

            for file_path in &language_files_list {
                if let Ok(parsed_file) = yantra_ast::parse_file(file_path) {
                    let lines_count = parsed_file.source.lines().count();
                    yantra_ast::insert_file(
                        &self.connection,
                        file_path,
                        parsed_file.language,
                        lines_count,
                        &parsed_file.source,
                    )?;

                    if let Ok((symbols, calls, imports)) = yantra_ast::extract_all(&parsed_file) {
                        for symbol in &symbols {
                            yantra_ast::insert_symbol(&self.connection, symbol)?;
                        }

                        for call_site in calls {
                            let call_site_uuid = Uuid::new_v4().to_string();

                            // Optimize: In-memory line-to-symbol range matching instead of SELECT queries!
                            let mut caller_symbol_id: Option<String> = None;
                            let mut smallest_range = usize::MAX;
                            for symbol in &symbols {
                                if call_site.caller_line >= symbol.start_line
                                    && call_site.caller_line <= symbol.end_line
                                {
                                    let range = symbol.end_line - symbol.start_line;
                                    if range < smallest_range {
                                        smallest_range = range;
                                        caller_symbol_id = Some(symbol.id.to_string());
                                    }
                                }
                            }

                            insert_call_stmt.execute(params![
                                call_site_uuid,
                                caller_symbol_id,
                                call_site.callee_name,
                                Option::<String>::None,
                                call_site.caller_line as i64,
                            ])?;
                        }

                        for import_declaration in imports {
                            let import_uuid = Uuid::new_v4().to_string();
                            let file_identifier = file_id_for_path(file_path);
                            let imported_names_json = serde_json::to_string(
                                &import_declaration.imported_names,
                            )
                            .map_err(|serialization_error| {
                                crate::error::CrgError::Serialization(
                                    serialization_error.to_string(),
                                )
                            })?;

                            insert_import_stmt.execute(params![
                                import_uuid,
                                file_identifier,
                                import_declaration.module_path,
                                imported_names_json,
                            ])?;
                        }
                    }
                }
            }
            Ok(())
        })();

        if result.is_err() {
            self.connection.execute("ROLLBACK", []).ok();
            result?;
        } else {
            self.connection.execute("COMMIT", [])?;
        }

        self.resolve_call_sites()?;
        self.rebuild_edges()?;
        self.recompute_all_connectivity_scores()?;

        Ok(())
    }

    /// Re-indexes a single file at `file_path`, replacing its symbols and edges
    /// in the graph while preserving all other files' data.
    ///
    /// # Errors
    ///
    /// Returns an error if reading the file, parsing, or any `SQLite` operation fails.
    pub fn update_file(&self, file_path: &Path) -> CrgResult<()> {
        let file_identifier = file_id_for_path(file_path);

        let in_tx = !self.connection.is_autocommit();
        if !in_tx {
            self.connection.execute("BEGIN TRANSACTION", [])?;
        }

        let result = (|| -> CrgResult<()> {
            let mut old_symbols_map = HashMap::new();
            {
                let mut statement = self.connection.prepare_cached(
                    "SELECT id, name, kind, start_line, end_line, signature, docstring FROM symbols WHERE file_id = ?1"
                )?;
                let rows = statement.query_map([&file_identifier], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        (
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                        ),
                    ))
                })?;

                for row_result in rows {
                    let (symbol_id, metadata) = row_result?;
                    old_symbols_map.insert(symbol_id, metadata);
                }
            }

            let parsed_file = match yantra_ast::parse_file(file_path) {
                Ok(parsed) => parsed,
                Err(_) => return Ok(()),
            };

            let lines_count = parsed_file.source.lines().count();
            yantra_ast::insert_file(
                &self.connection,
                file_path,
                parsed_file.language,
                lines_count,
                &parsed_file.source,
            )?;

            let (new_symbols, calls, imports) =
                yantra_ast::extract_all(&parsed_file).unwrap_or_default();

            let new_symbols_ids: HashSet<String> = new_symbols
                .iter()
                .map(|symbol| symbol.id.to_string())
                .collect();

            let mut delete_calls_stmt = self
                .connection
                .prepare_cached("DELETE FROM call_sites WHERE caller_symbol_id = ?1")?;
            let mut null_callee_stmt = self.connection.prepare_cached(
                "UPDATE call_sites SET callee_symbol_id = NULL WHERE callee_symbol_id = ?1",
            )?;
            let mut delete_edges_stmt = self
                .connection
                .prepare_cached("DELETE FROM edges WHERE from_id = ?1 OR to_id = ?1")?;
            let mut delete_symbol_stmt = self
                .connection
                .prepare_cached("DELETE FROM symbols WHERE id = ?1")?;
            let mut delete_embedding_stmt = self
                .connection
                .prepare_cached("DELETE FROM symbol_embeddings WHERE symbol_id = ?1")?;

            for old_symbol_id in old_symbols_map.keys() {
                if !new_symbols_ids.contains(old_symbol_id) {
                    delete_calls_stmt.execute([old_symbol_id])?;
                    null_callee_stmt.execute([old_symbol_id])?;
                    delete_edges_stmt.execute([old_symbol_id])?;
                    delete_symbol_stmt.execute([old_symbol_id])?;
                    delete_embedding_stmt.execute([old_symbol_id])?;
                }
            }

            for symbol in &new_symbols {
                let symbol_id_str = symbol.id.to_string();
                let is_new = !old_symbols_map.contains_key(&symbol_id_str);
                let has_changed = if let Some(old_meta) = old_symbols_map.get(&symbol_id_str) {
                    let kind_json = serde_json::to_string(&symbol.kind).unwrap_or_default();
                    old_meta.0 != symbol.name
                        || old_meta.1 != kind_json
                        || old_meta.2 != symbol.start_line as i64
                        || old_meta.3 != symbol.end_line as i64
                        || old_meta.4 != symbol.signature
                        || old_meta.5 != symbol.docstring
                } else {
                    false
                };

                if is_new || has_changed {
                    yantra_ast::insert_symbol(&self.connection, symbol)?;
                    delete_embedding_stmt.execute([&symbol_id_str])?;
                }
            }

            self.connection.execute(
                "DELETE FROM call_sites WHERE caller_symbol_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
                [&file_identifier]
            )?;

            if !calls.is_empty() {
                let mut insert_call_stmt = self.connection.prepare_cached(
                    "INSERT OR REPLACE INTO call_sites (id, caller_symbol_id, callee_name, callee_symbol_id, line) VALUES (?1, ?2, ?3, ?4, ?5)"
                )?;

                for call_site in calls {
                    let call_site_uuid = Uuid::new_v4().to_string();

                    // Optimize: In-memory range matching!
                    let mut caller_symbol_id: Option<String> = None;
                    let mut smallest_range = usize::MAX;
                    for symbol in &new_symbols {
                        if call_site.caller_line >= symbol.start_line
                            && call_site.caller_line <= symbol.end_line
                        {
                            let range = symbol.end_line - symbol.start_line;
                            if range < smallest_range {
                                smallest_range = range;
                                caller_symbol_id = Some(symbol.id.to_string());
                            }
                        }
                    }

                    insert_call_stmt.execute(params![
                        call_site_uuid,
                        caller_symbol_id,
                        call_site.callee_name,
                        Option::<String>::None,
                        call_site.caller_line as i64,
                    ])?;
                }
            }

            self.connection
                .execute("DELETE FROM imports WHERE file_id = ?1", [&file_identifier])?;
            if !imports.is_empty() {
                let mut insert_import_stmt = self.connection.prepare_cached(
                    "INSERT OR REPLACE INTO imports (id, file_id, module_path, imported_names) VALUES (?1, ?2, ?3, ?4)"
                )?;

                for import_declaration in imports {
                    let import_uuid = Uuid::new_v4().to_string();
                    let imported_names_json = serde_json::to_string(
                        &import_declaration.imported_names,
                    )
                    .map_err(|serialization_error| {
                        crate::error::CrgError::Serialization(serialization_error.to_string())
                    })?;

                    insert_import_stmt.execute(params![
                        import_uuid,
                        file_identifier,
                        import_declaration.module_path,
                        imported_names_json,
                    ])?;
                }
            }

            self.resolve_call_sites()?;

            let mut affected_symbol_ids = HashSet::new();
            {
                let mut statement = self.connection.prepare_cached(
                    "SELECT to_id FROM edges WHERE from_id IN (SELECT id FROM symbols WHERE file_id = ?1)"
                )?;
                let rows =
                    statement.query_map([&file_identifier], |row| row.get::<_, String>(0))?;
                for row_result in rows {
                    affected_symbol_ids.insert(row_result?);
                }
            }
            {
                let mut statement = self.connection.prepare_cached(
                    "SELECT from_id FROM edges WHERE to_id IN (SELECT id FROM symbols WHERE file_id = ?1)"
                )?;
                let rows =
                    statement.query_map([&file_identifier], |row| row.get::<_, String>(0))?;
                for row_result in rows {
                    affected_symbol_ids.insert(row_result?);
                }
            }

            self.connection.execute(
                "DELETE FROM edges WHERE from_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
                [&file_identifier],
            )?;
            self.connection.execute(
                "DELETE FROM edges WHERE to_id IN (SELECT id FROM symbols WHERE file_id = ?1) AND edge_type = ?2",
                params![&file_identifier, EdgeType::Calls.as_sql_str()]
            )?;

            self.rebuild_edges()?;

            for symbol_id_str in &new_symbols_ids {
                affected_symbol_ids.insert(symbol_id_str.clone());
            }
            {
                let mut statement = self.connection.prepare_cached(
                    "SELECT to_id FROM edges WHERE from_id IN (SELECT id FROM symbols WHERE file_id = ?1)"
                )?;
                let rows =
                    statement.query_map([&file_identifier], |row| row.get::<_, String>(0))?;
                for row_result in rows {
                    affected_symbol_ids.insert(row_result?);
                }
            }
            {
                let mut statement = self.connection.prepare_cached(
                    "SELECT from_id FROM edges WHERE to_id IN (SELECT id FROM symbols WHERE file_id = ?1)"
                )?;
                let rows =
                    statement.query_map([&file_identifier], |row| row.get::<_, String>(0))?;
                for row_result in rows {
                    affected_symbol_ids.insert(row_result?);
                }
            }

            let mut update_statement = self.connection.prepare_cached(
                "UPDATE symbols
                 SET connectivity_score = (
                     (SELECT COUNT(*) FROM edges WHERE to_id = symbols.id AND edge_type = 'CALLS') * 2 +
                     (SELECT COUNT(*) FROM edges WHERE from_id = symbols.id AND edge_type = 'CALLS') * 2 +
                     (SELECT COUNT(*) FROM edges WHERE to_id = symbols.id AND edge_type = 'IMPORTS') * 2 +
                     (SELECT COUNT(*) FROM edges WHERE to_id = symbols.file_id AND edge_type = 'IMPORTS')
                 )
                 WHERE id = ?1"
            )?;

            for symbol_identifier in affected_symbol_ids {
                update_statement.execute(params![symbol_identifier])?;
            }

            Ok(())
        })();

        if result.is_err() {
            if !in_tx {
                self.connection.execute("ROLLBACK", []).ok();
            }
            result?;
        } else if !in_tx {
            self.connection.execute("COMMIT", [])?;
        }

        Ok(())
    }

    fn resolve_call_sites(&self) -> CrgResult<()> {
        let mut call_sites_to_resolve = Vec::new();
        {
            let mut statement = self.connection.prepare_cached(
                "SELECT cs.id, cs.callee_name, f.path
                 FROM call_sites cs
                 JOIN symbols s ON cs.caller_symbol_id = s.id
                 JOIN files f ON s.file_id = f.id
                 WHERE cs.callee_symbol_id IS NULL",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;

            for row_result in rows {
                call_sites_to_resolve.push(row_result?);
            }
        }

        let in_tx = !self.connection.is_autocommit();
        if !in_tx {
            self.connection.execute("BEGIN TRANSACTION", [])?;
        }

        let result = (|| -> CrgResult<()> {
            let mut stmt_file = self.connection.prepare_cached(
                "SELECT id FROM symbols WHERE file_id = ?1 AND name = ?2 LIMIT 1",
            )?;
            let mut stmt_dir = self.connection.prepare_cached(
                "SELECT s.id FROM symbols s JOIN files f ON s.file_id = f.id WHERE f.path LIKE ?1 AND s.name = ?2 LIMIT 1"
            )?;
            let mut stmt_any = self
                .connection
                .prepare_cached("SELECT id FROM symbols WHERE name = ?1 LIMIT 1")?;
            let mut stmt_update = self
                .connection
                .prepare_cached("UPDATE call_sites SET callee_symbol_id = ?1 WHERE id = ?2")?;

            for (call_site_id, callee_name, file_path_str) in call_sites_to_resolve {
                let file_path = Path::new(&file_path_str);
                let file_identifier = file_id_for_path(file_path);

                let mut resolved_id: Option<String> = stmt_file
                    .query_row(params![file_identifier, callee_name], |row| row.get(0))
                    .ok();

                if resolved_id.is_none() {
                    if let Some(directory_path) = file_path.parent() {
                        let directory_pattern = format!("{}%", directory_path.to_string_lossy());
                        resolved_id = stmt_dir
                            .query_row(params![directory_pattern, callee_name], |row| row.get(0))
                            .ok();
                    }
                }

                if resolved_id.is_none() {
                    resolved_id = stmt_any
                        .query_row(params![callee_name], |row| row.get(0))
                        .ok();
                }

                if let Some(callee_symbol_id) = resolved_id {
                    stmt_update.execute(params![callee_symbol_id, call_site_id])?;
                }
            }
            Ok(())
        })();

        if result.is_err() {
            if !in_tx {
                self.connection.execute("ROLLBACK", []).ok();
            }
            result?;
        } else if !in_tx {
            self.connection.execute("COMMIT", [])?;
        }

        Ok(())
    }

    fn rebuild_edges(&self) -> CrgResult<()> {
        let mut resolved_calls = Vec::new();
        {
            let mut statement = self.connection.prepare_cached(
                "SELECT caller_symbol_id, callee_symbol_id FROM call_sites WHERE caller_symbol_id IS NOT NULL AND callee_symbol_id IS NOT NULL"
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row_result in rows {
                resolved_calls.push(row_result?);
            }
        }

        let in_tx = !self.connection.is_autocommit();
        if !in_tx {
            self.connection.execute("BEGIN TRANSACTION", [])?;
        }

        let result = (|| -> CrgResult<()> {
            let mut insert_stmt = self.connection.prepare_cached(
                "INSERT OR IGNORE INTO edges (from_id, to_id, edge_type, weight) VALUES (?1, ?2, ?3, 1)"
            )?;

            for (from_id, to_id) in resolved_calls {
                insert_stmt.execute(params![from_id, to_id, EdgeType::Calls.as_sql_str()])?;
            }

            let mut imports_list = Vec::new();
            {
                let mut statement = self
                    .connection
                    .prepare_cached("SELECT file_id, module_path, imported_names FROM imports")?;
                let rows = statement.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?;
                for row_result in rows {
                    imports_list.push(row_result?);
                }
            }

            let mut query_file_stmt = self.connection.prepare_cached(
                "SELECT id FROM files WHERE path LIKE ?1 OR path LIKE ?2 LIMIT 1",
            )?;
            let mut query_symbol_stmt = self.connection.prepare_cached(
                "SELECT id FROM symbols WHERE file_id = ?1 AND name = ?2 LIMIT 1",
            )?;

            for (importing_file_id, module_path, imported_names_json) in imports_list {
                let imported_names: Vec<String> =
                    serde_json::from_str(&imported_names_json).unwrap_or_default();
                let target_file_pattern = format!("%{}", module_path.replace('.', "/"));
                let target_file_id: Option<String> = query_file_stmt
                    .query_row(
                        params![
                            format!("{target_file_pattern}.rs"),
                            format!("{target_file_pattern}.py"),
                        ],
                        |row| row.get(0),
                    )
                    .ok();

                if let Some(target_file_id) = target_file_id {
                    insert_stmt.execute(params![
                        importing_file_id,
                        target_file_id,
                        EdgeType::Imports.as_sql_str()
                    ])?;

                    for name in imported_names {
                        let target_symbol_id: Option<String> = query_symbol_stmt
                            .query_row(params![target_file_id, name], |row| row.get(0))
                            .ok();

                        if let Some(target_symbol_id) = target_symbol_id {
                            insert_stmt.execute(params![
                                importing_file_id,
                                target_symbol_id,
                                EdgeType::Imports.as_sql_str()
                            ])?;
                        }
                    }
                }
            }

            let mut rust_impls = Vec::new();
            {
                let mut statement = self.connection.prepare_cached(
                    "SELECT id, signature FROM symbols WHERE kind = '\"Impl\"' AND signature IS NOT NULL"
                )?;
                let rows = statement.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                for row_result in rows {
                    rust_impls.push(row_result?);
                }
            }

            let mut query_trait_stmt = self.connection.prepare_cached(
                "SELECT id FROM symbols WHERE name = ?1 AND kind = '\"Trait\"' LIMIT 1",
            )?;
            let mut query_struct_stmt = self.connection.prepare_cached(
                "SELECT id FROM symbols WHERE name = ?1 AND kind = '\"Struct\"' LIMIT 1",
            )?;

            for (impl_symbol_id, signature) in rust_impls {
                if signature.starts_with("impl") {
                    if let Some(for_index) = signature.find(" for ") {
                        let trait_part = &signature["impl".len()..for_index];
                        let struct_part = &signature[for_index + " for ".len()..];

                        let trait_name = clean_rust_type_name(trait_part);
                        let struct_name = clean_rust_type_name(struct_part);

                        let trait_symbol_id: Option<String> = query_trait_stmt
                            .query_row(params![trait_name], |row| row.get(0))
                            .ok();

                        let struct_symbol_id: Option<String> = query_struct_stmt
                            .query_row(params![struct_name], |row| row.get(0))
                            .ok();

                        if let (Some(struct_id), Some(trait_id)) =
                            (struct_symbol_id, trait_symbol_id)
                        {
                            insert_stmt.execute(params![
                                struct_id,
                                trait_id,
                                EdgeType::Implements.as_sql_str()
                            ])?;
                            insert_stmt.execute(params![
                                impl_symbol_id,
                                struct_id,
                                EdgeType::Implements.as_sql_str()
                            ])?;
                        }
                    }
                }
            }

            let mut test_symbols = Vec::new();
            {
                let mut statement = self.connection.prepare_cached(
                    "SELECT s.id, s.name, f.path FROM symbols s JOIN files f ON s.file_id = f.id WHERE f.path LIKE '%tests%' OR s.name LIKE 'test_%'"
                )?;
                let rows = statement.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?;
                for row_result in rows {
                    test_symbols.push(row_result?);
                }
            }

            let mut query_test_stmt = self
                .connection
                .prepare_cached("SELECT id FROM symbols WHERE name = ?1 LIMIT 1")?;

            for (test_symbol_id, name, _) in test_symbols {
                if name.starts_with("test_") {
                    let target_name = name.strip_prefix("test_").unwrap_or(&name);
                    let target_symbol_id: Option<String> = query_test_stmt
                        .query_row(params![target_name], |row| row.get(0))
                        .ok();

                    if let Some(target_symbol_id) = target_symbol_id {
                        insert_stmt.execute(params![
                            test_symbol_id,
                            target_symbol_id,
                            EdgeType::Tests.as_sql_str()
                        ])?;
                    }
                }
            }
            Ok(())
        })();

        if result.is_err() {
            if !in_tx {
                self.connection.execute("ROLLBACK", []).ok();
            }
            result?;
        } else if !in_tx {
            self.connection.execute("COMMIT", [])?;
        }

        Ok(())
    }

    fn recompute_all_connectivity_scores(&self) -> CrgResult<()> {
        self.connection.execute(
            "UPDATE symbols
             SET connectivity_score = (
                 (SELECT COUNT(*) FROM edges WHERE to_id = symbols.id AND edge_type = 'CALLS') * 2 +
                 (SELECT COUNT(*) FROM edges WHERE from_id = symbols.id AND edge_type = 'CALLS') * 2 +
                 (SELECT COUNT(*) FROM edges WHERE to_id = symbols.id AND edge_type = 'IMPORTS') * 2 +
                 (SELECT COUNT(*) FROM edges WHERE to_id = symbols.file_id AND edge_type = 'IMPORTS')
             )",
            [],
        )?;
        Ok(())
    }
}

fn collect_supported_files(path: &Path, files: &mut Vec<PathBuf>) -> CrgResult<()> {
    if path.is_file() {
        if yantra_ast::LanguageRegistry::language_for_path(path).is_some() {
            files.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let child_path = entry.path();
            if let Some(name) = child_path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.')
                    || name == "target"
                    || name == "node_modules"
                    || name == "tests"
                    || name == "benches"
                    || name == "examples"
                    || name == "fixtures"
                {
                    continue;
                }
            }
            collect_supported_files(&child_path, files)?;
        }
    }
    Ok(())
}

fn file_id_for_path(path: &Path) -> String {
    SymbolId::from_parts(&path.to_string_lossy(), "file", "file").to_string()
}

fn clean_rust_type_name(part: &str) -> String {
    let cleaned = part.split('{').next().unwrap_or(part);
    let cleaned = cleaned.split('<').next().unwrap_or(cleaned);
    cleaned
        .trim()
        .trim_start_matches('&')
        .trim_start_matches("mut ")
        .split("::")
        .last()
        .unwrap_or(cleaned)
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::fs;

    #[test]
    fn test_graph_builder_full_lifecycle() {
        let temp_dir_path =
            std::env::temp_dir().join(format!("yantra-crg-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir_path).unwrap();

        let source_file_path = temp_dir_path.join("lib.rs");
        let source_code = r"
            pub trait MyTrait {
                fn do_something(&self);
            }

            pub struct MyStruct;

            impl MyTrait for MyStruct {
                fn do_something(&self) {
                    helper_function();
                }
            }

            pub fn helper_function() {}

            #[test]
            fn test_helper_function() {
                helper_function();
            }
        ";
        fs::write(&source_file_path, source_code).unwrap();

        let connection = Connection::open_in_memory().unwrap();
        let builder = GraphBuilder::new(connection);

        builder.build_from_repo(&temp_dir_path).unwrap();

        let symbols_count: i64 = builder
            .connection
            .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
            .unwrap();
        assert!(symbols_count > 0, "No symbols inserted");

        let implements_edges: i64 = builder
            .connection
            .query_row(
                "SELECT COUNT(*) FROM edges WHERE edge_type = 'IMPLEMENTS'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(implements_edges > 0, "No IMPLEMENTS edges created");

        let calls_edges: i64 = builder
            .connection
            .query_row(
                "SELECT COUNT(*) FROM edges WHERE edge_type = 'CALLS'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(calls_edges > 0, "No CALLS edges created");

        let tests_edges: i64 = builder
            .connection
            .query_row(
                "SELECT COUNT(*) FROM edges WHERE edge_type = 'TESTS'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(tests_edges > 0, "No TESTS edges created");

        let max_connectivity: i32 = builder
            .connection
            .query_row("SELECT MAX(connectivity_score) FROM symbols", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(max_connectivity > 0, "Connectivity scores not computed");

        let updated_source_code = r"
            pub trait MyTrait {
                fn do_something(&self);
            }

            pub struct MyStruct;

            impl MyTrait for MyStruct {
                fn do_something(&self) {
                }
            }

            pub fn helper_function() {}

            #[test]
            fn test_helper_function() {
            }
        ";
        fs::write(&source_file_path, updated_source_code).unwrap();

        builder.update_file(&source_file_path).unwrap();

        let updated_calls_edges: i64 = builder
            .connection
            .query_row(
                "SELECT COUNT(*) FROM edges WHERE edge_type = 'CALLS'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            updated_calls_edges, 0,
            "CALLS edges not cleared after update"
        );

        fs::remove_dir_all(&temp_dir_path).unwrap();
    }

    #[test]
    fn test_supported_file_detection_accepts_rust_files() {
        // Verifies that .rs files are recognised as indexable source files
        // using the same LanguageRegistry mechanism that collect_supported_files uses.
        let rust_file_path = std::path::Path::new("lib.rs");
        let is_supported =
            yantra_ast::LanguageRegistry::language_for_path(rust_file_path).is_some();
        assert!(
            is_supported,
            ".rs files must be accepted by the language registry"
        );
    }
}

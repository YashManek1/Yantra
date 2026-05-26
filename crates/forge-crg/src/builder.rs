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
//! - SQLite database rows in symbols, edges, call_sites, and imports tables
//! - Refreshed connectivity scores for affected nodes
//!
//! ## Related
//! - `forge-ast::parser` — parses source files for symbol extraction
//! - `forge-crg::schema` — establishes SQLite schema
//! - `forge-crg::connectivity` — calculates connectivity scores

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use rusqlite::{params, Connection};
use uuid::Uuid;

use yantra_core::SymbolId;

use crate::schema::create_crg_schema;

pub struct GraphBuilder {
    connection: Connection,
}

impl GraphBuilder {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn build_from_repo(&self, repository_root: &Path) -> anyhow::Result<()> {
        create_crg_schema(&self.connection)?;

        let mut language_files_list = Vec::new();
        collect_supported_files(repository_root, &mut language_files_list)?;

        for file_path in &language_files_list {
            if let Ok(parsed_file) = yantra_ast::parse_file(file_path) {
                if let Ok(symbols) = yantra_ast::extract_symbols(&parsed_file) {
                    for symbol in symbols {
                        yantra_ast::insert_symbol(&self.connection, &symbol)?;
                    }
                }

                if let Ok(calls) = yantra_ast::extract_calls(&parsed_file) {
                    for call_site in calls {
                        let call_site_uuid = Uuid::new_v4().to_string();
                        let file_identifier = file_id_for_path(file_path);

                        let caller_symbol_id: Option<String> = self.connection.query_row(
                            "SELECT id FROM symbols WHERE file_id = ?1 AND ?2 >= start_line AND ?2 <= end_line ORDER BY (end_line - start_line) ASC LIMIT 1",
                            params![file_identifier, call_site.caller_line as i64],
                            |row| row.get(0)
                        ).ok();

                        self.connection.execute(
                            "INSERT OR REPLACE INTO call_sites (id, caller_symbol_id, callee_name, callee_symbol_id, line) VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![
                                call_site_uuid,
                                caller_symbol_id,
                                call_site.callee_name,
                                Option::<String>::None,
                                call_site.caller_line as i64,
                            ]
                        )?;
                    }
                }

                if let Ok(imports) = yantra_ast::extract_imports(&parsed_file) {
                    for import_declaration in imports {
                        let import_uuid = Uuid::new_v4().to_string();
                        let file_identifier = file_id_for_path(file_path);
                        let imported_names_json = serde_json::to_string(&import_declaration.imported_names)?;

                        self.connection.execute(
                            "INSERT OR REPLACE INTO imports (id, file_id, module_path, imported_names) VALUES (?1, ?2, ?3, ?4)",
                            params![
                                import_uuid,
                                file_identifier,
                                import_declaration.module_path,
                                imported_names_json,
                            ]
                        )?;
                    }
                }
            }
        }

        self.resolve_call_sites()?;
        self.rebuild_edges()?;
        self.recompute_all_connectivity_scores()?;

        Ok(())
    }

    pub fn update_file(&self, file_path: &Path) -> anyhow::Result<()> {
        let file_identifier = file_id_for_path(file_path);

        let mut old_symbols_map = HashMap::new();
        {
            let mut statement = self.connection.prepare(
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
                    )
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

        let new_symbols = yantra_ast::extract_symbols(&parsed_file).unwrap_or_default();
        let new_symbols_ids: HashSet<String> = new_symbols.iter().map(|symbol| symbol.id.to_string()).collect();

        for (old_symbol_id, _) in &old_symbols_map {
            if !new_symbols_ids.contains(old_symbol_id) {
                self.connection.execute("DELETE FROM call_sites WHERE caller_symbol_id = ?1", [old_symbol_id])?;
                self.connection.execute("UPDATE call_sites SET callee_symbol_id = NULL WHERE callee_symbol_id = ?1", [old_symbol_id])?;
                self.connection.execute("DELETE FROM edges WHERE from_id = ?1 OR to_id = ?1", [old_symbol_id])?;
                self.connection.execute("DELETE FROM symbols WHERE id = ?1", [old_symbol_id])?;
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
            }
        }

        self.connection.execute(
            "DELETE FROM call_sites WHERE caller_symbol_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
            [&file_identifier]
        )?;
        if let Ok(calls) = yantra_ast::extract_calls(&parsed_file) {
            for call_site in calls {
                let call_site_uuid = Uuid::new_v4().to_string();
                let caller_symbol_id: Option<String> = self.connection.query_row(
                    "SELECT id FROM symbols WHERE file_id = ?1 AND ?2 >= start_line AND ?2 <= end_line ORDER BY (end_line - start_line) ASC LIMIT 1",
                    params![file_identifier, call_site.caller_line as i64],
                    |row| row.get(0)
                ).ok();

                self.connection.execute(
                    "INSERT OR REPLACE INTO call_sites (id, caller_symbol_id, callee_name, callee_symbol_id, line) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        call_site_uuid,
                        caller_symbol_id,
                        call_site.callee_name,
                        Option::<String>::None,
                        call_site.caller_line as i64,
                    ]
                )?;
            }
        }

        self.connection.execute("DELETE FROM imports WHERE file_id = ?1", [&file_identifier])?;
        if let Ok(imports) = yantra_ast::extract_imports(&parsed_file) {
            for import_declaration in imports {
                let import_uuid = Uuid::new_v4().to_string();
                let imported_names_json = serde_json::to_string(&import_declaration.imported_names)?;

                self.connection.execute(
                    "INSERT OR REPLACE INTO imports (id, file_id, module_path, imported_names) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        import_uuid,
                        file_identifier,
                        import_declaration.module_path,
                        imported_names_json,
                    ]
                )?;
            }
        }

        self.resolve_call_sites()?;

        let mut affected_symbol_ids = HashSet::new();
        {
            let mut statement = self.connection.prepare(
                "SELECT to_id FROM edges WHERE from_id IN (SELECT id FROM symbols WHERE file_id = ?1)"
            )?;
            let rows = statement.query_map([&file_identifier], |row| row.get::<_, String>(0))?;
            for row_result in rows {
                affected_symbol_ids.insert(row_result?);
            }
        }
        {
            let mut statement = self.connection.prepare(
                "SELECT from_id FROM edges WHERE to_id IN (SELECT id FROM symbols WHERE file_id = ?1)"
            )?;
            let rows = statement.query_map([&file_identifier], |row| row.get::<_, String>(0))?;
            for row_result in rows {
                affected_symbol_ids.insert(row_result?);
            }
        }

        self.connection.execute(
            "DELETE FROM edges WHERE from_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
            [&file_identifier]
        )?;
        self.connection.execute(
            "DELETE FROM edges WHERE to_id IN (SELECT id FROM symbols WHERE file_id = ?1) AND edge_type = 'CALLS'",
            [&file_identifier]
        )?;

        self.rebuild_edges()?;

        for symbol_id_str in &new_symbols_ids {
            affected_symbol_ids.insert(symbol_id_str.clone());
        }
        {
            let mut statement = self.connection.prepare(
                "SELECT to_id FROM edges WHERE from_id IN (SELECT id FROM symbols WHERE file_id = ?1)"
            )?;
            let rows = statement.query_map([&file_identifier], |row| row.get::<_, String>(0))?;
            for row_result in rows {
                affected_symbol_ids.insert(row_result?);
            }
        }
        {
            let mut statement = self.connection.prepare(
                "SELECT from_id FROM edges WHERE to_id IN (SELECT id FROM symbols WHERE file_id = ?1)"
            )?;
            let rows = statement.query_map([&file_identifier], |row| row.get::<_, String>(0))?;
            for row_result in rows {
                affected_symbol_ids.insert(row_result?);
            }
        }

        let mut update_statement = self.connection.prepare(
            "UPDATE symbols
             SET connectivity_score = (
                 (SELECT COUNT(*) FROM edges WHERE to_id = symbols.id AND edge_type = 'CALLS') +
                 (SELECT COUNT(*) FROM edges WHERE from_id = symbols.id AND edge_type = 'CALLS') +
                 (SELECT COUNT(*) FROM edges WHERE to_id = symbols.id AND edge_type = 'IMPORTS') +
                 (SELECT COUNT(*) FROM edges WHERE to_id = symbols.file_id AND edge_type = 'IMPORTS')
             )
             WHERE id = ?1"
        )?;

        for symbol_identifier in affected_symbol_ids {
            update_statement.execute(params![symbol_identifier])?;
        }

        Ok(())
    }

    fn resolve_call_sites(&self) -> anyhow::Result<()> {
        let mut call_sites_to_resolve = Vec::new();
        {
            let mut statement = self.connection.prepare(
                "SELECT cs.id, cs.callee_name, f.path
                 FROM call_sites cs
                 JOIN symbols s ON cs.caller_symbol_id = s.id
                 JOIN files f ON s.file_id = f.id
                 WHERE cs.callee_symbol_id IS NULL"
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

        for (call_site_id, callee_name, file_path_str) in call_sites_to_resolve {
            let file_path = Path::new(&file_path_str);
            let file_identifier = file_id_for_path(file_path);

            let mut resolved_id: Option<String> = self.connection.query_row(
                "SELECT id FROM symbols WHERE file_id = ?1 AND name = ?2 LIMIT 1",
                params![file_identifier, callee_name],
                |row| row.get(0)
            ).ok();

            if resolved_id.is_none() {
                if let Some(directory_path) = file_path.parent() {
                    let directory_pattern = format!("{}%", directory_path.to_string_lossy());
                    resolved_id = self.connection.query_row(
                        "SELECT s.id FROM symbols s JOIN files f ON s.file_id = f.id WHERE f.path LIKE ?1 AND s.name = ?2 LIMIT 1",
                        params![directory_pattern, callee_name],
                        |row| row.get(0)
                    ).ok();
                }
            }

            if resolved_id.is_none() {
                resolved_id = self.connection.query_row(
                    "SELECT id FROM symbols WHERE name = ?1 LIMIT 1",
                    params![callee_name],
                    |row| row.get(0)
                ).ok();
            }

            if let Some(callee_symbol_id) = resolved_id {
                self.connection.execute(
                    "UPDATE call_sites SET callee_symbol_id = ?1 WHERE id = ?2",
                    params![callee_symbol_id, call_site_id]
                )?;
            }
        }

        Ok(())
    }

    fn rebuild_edges(&self) -> anyhow::Result<()> {
        let mut resolved_calls = Vec::new();
        {
            let mut statement = self.connection.prepare(
                "SELECT caller_symbol_id, callee_symbol_id FROM call_sites WHERE caller_symbol_id IS NOT NULL AND callee_symbol_id IS NOT NULL"
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row_result in rows {
                resolved_calls.push(row_result?);
            }
        }
        for (from_id, to_id) in resolved_calls {
            self.connection.execute(
                "INSERT OR IGNORE INTO edges (from_id, to_id, edge_type, weight) VALUES (?1, ?2, 'CALLS', 1)",
                params![from_id, to_id]
            )?;
        }

        let mut imports_list = Vec::new();
        {
            let mut statement = self.connection.prepare(
                "SELECT file_id, module_path, imported_names FROM imports"
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })?;
            for row_result in rows {
                imports_list.push(row_result?);
            }
        }
        for (importing_file_id, module_path, imported_names_json) in imports_list {
            let imported_names: Vec<String> = serde_json::from_str(&imported_names_json).unwrap_or_default();
            let target_file_pattern = format!("%{}", module_path.replace('.', "/"));
            let target_file_id: Option<String> = self.connection.query_row(
                "SELECT id FROM files WHERE path LIKE ?1 OR path LIKE ?2 LIMIT 1",
                params![
                    format!("{}.rs", target_file_pattern),
                    format!("{}.py", target_file_pattern),
                ],
                |row| row.get(0)
            ).ok();

            if let Some(target_file_id) = target_file_id {
                self.connection.execute(
                    "INSERT OR IGNORE INTO edges (from_id, to_id, edge_type, weight) VALUES (?1, ?2, 'IMPORTS', 1)",
                    params![importing_file_id, target_file_id]
                )?;

                for name in imported_names {
                    let target_symbol_id: Option<String> = self.connection.query_row(
                        "SELECT id FROM symbols WHERE file_id = ?1 AND name = ?2 LIMIT 1",
                        params![target_file_id, name],
                        |row| row.get(0)
                    ).ok();

                    if let Some(target_symbol_id) = target_symbol_id {
                        self.connection.execute(
                            "INSERT OR IGNORE INTO edges (from_id, to_id, edge_type, weight) VALUES (?1, ?2, 'IMPORTS', 1)",
                            params![importing_file_id, target_symbol_id]
                        )?;
                    }
                }
            }
        }

        let mut rust_impls = Vec::new();
        {
            let mut statement = self.connection.prepare(
                "SELECT id, signature FROM symbols WHERE kind = '\"Impl\"' AND signature IS NOT NULL"
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row_result in rows {
                rust_impls.push(row_result?);
            }
        }
        for (impl_symbol_id, signature) in rust_impls {
            if signature.starts_with("impl") {
                if let Some(for_index) = signature.find(" for ") {
                    let trait_part = &signature["impl".len() .. for_index];
                    let struct_part = &signature[for_index + " for ".len() ..];

                    let trait_name = clean_rust_type_name(trait_part);
                    let struct_name = clean_rust_type_name(struct_part);

                    let trait_symbol_id: Option<String> = self.connection.query_row(
                        "SELECT id FROM symbols WHERE name = ?1 AND kind = '\"Trait\"' LIMIT 1",
                        params![trait_name],
                        |row| row.get(0)
                    ).ok();

                    let struct_symbol_id: Option<String> = self.connection.query_row(
                        "SELECT id FROM symbols WHERE name = ?1 AND kind = '\"Struct\"' LIMIT 1",
                        params![struct_name],
                        |row| row.get(0)
                    ).ok();

                    if let (Some(struct_id), Some(trait_id)) = (struct_symbol_id, trait_symbol_id) {
                        self.connection.execute(
                            "INSERT OR IGNORE INTO edges (from_id, to_id, edge_type, weight) VALUES (?1, ?2, 'IMPLEMENTS', 1)",
                            params![struct_id, trait_id]
                        )?;
                        self.connection.execute(
                            "INSERT OR IGNORE INTO edges (from_id, to_id, edge_type, weight) VALUES (?1, ?2, 'IMPLEMENTS', 1)",
                            params![impl_symbol_id, struct_id]
                        )?;
                    }
                }
            }
        }

        let mut test_symbols = Vec::new();
        {
            let mut statement = self.connection.prepare(
                "SELECT s.id, s.name, f.path FROM symbols s JOIN files f ON s.file_id = f.id WHERE f.path LIKE '%tests%' OR s.name LIKE 'test_%'"
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })?;
            for row_result in rows {
                test_symbols.push(row_result?);
            }
        }
        for (test_symbol_id, name, _) in test_symbols {
            if name.starts_with("test_") {
                let target_name = name.strip_prefix("test_").unwrap_or(&name);
                let target_symbol_id: Option<String> = self.connection.query_row(
                    "SELECT id FROM symbols WHERE name = ?1 LIMIT 1",
                    params![target_name],
                    |row| row.get(0)
                ).ok();

                if let Some(target_symbol_id) = target_symbol_id {
                    self.connection.execute(
                        "INSERT OR IGNORE INTO edges (from_id, to_id, edge_type, weight) VALUES (?1, ?2, 'TESTS', 1)",
                        params![test_symbol_id, target_symbol_id]
                    )?;
                }
            }
        }

        Ok(())
    }

    fn recompute_all_connectivity_scores(&self) -> anyhow::Result<()> {
        self.connection.execute(
            "UPDATE symbols
             SET connectivity_score = (
                 (SELECT COUNT(*) FROM edges WHERE to_id = symbols.id AND edge_type = 'CALLS') +
                 (SELECT COUNT(*) FROM edges WHERE from_id = symbols.id AND edge_type = 'CALLS') +
                 (SELECT COUNT(*) FROM edges WHERE to_id = symbols.id AND edge_type = 'IMPORTS') +
                 (SELECT COUNT(*) FROM edges WHERE to_id = symbols.file_id AND edge_type = 'IMPORTS')
             )",
            [],
        )?;
        Ok(())
    }
}

fn collect_supported_files(path: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if path.is_file() {
        if yantra_ast::LanguageRegistry::language_for_path(path).is_some() {
            files.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let child_path = entry.path();
            if let Some(name) = child_path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') || name == "target" || name == "node_modules" {
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
    cleaned.trim()
        .trim_start_matches('&')
        .trim_start_matches("mut ")
        .split("::").last()
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
        let temp_dir_path = std::env::temp_dir().join(format!("yantra-crg-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir_path).unwrap();

        let source_file_path = temp_dir_path.join("lib.rs");
        let source_code = r#"
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
        "#;
        fs::write(&source_file_path, source_code).unwrap();

        let connection = Connection::open_in_memory().unwrap();
        let builder = GraphBuilder::new(connection);

        builder.build_from_repo(&temp_dir_path).unwrap();

        let symbols_count: i64 = builder.connection.query_row(
            "SELECT COUNT(*) FROM symbols",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(symbols_count > 0, "No symbols inserted");

        let implements_edges: i64 = builder.connection.query_row(
            "SELECT COUNT(*) FROM edges WHERE edge_type = 'IMPLEMENTS'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(implements_edges > 0, "No IMPLEMENTS edges created");

        let calls_edges: i64 = builder.connection.query_row(
            "SELECT COUNT(*) FROM edges WHERE edge_type = 'CALLS'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(calls_edges > 0, "No CALLS edges created");

        let tests_edges: i64 = builder.connection.query_row(
            "SELECT COUNT(*) FROM edges WHERE edge_type = 'TESTS'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(tests_edges > 0, "No TESTS edges created");

        let max_connectivity: i32 = builder.connection.query_row(
            "SELECT MAX(connectivity_score) FROM symbols",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(max_connectivity > 0, "Connectivity scores not computed");

        let updated_source_code = r#"
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
        "#;
        fs::write(&source_file_path, updated_source_code).unwrap();

        builder.update_file(&source_file_path).unwrap();

        let updated_calls_edges: i64 = builder.connection.query_row(
            "SELECT COUNT(*) FROM edges WHERE edge_type = 'CALLS'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(updated_calls_edges, 0, "CALLS edges not cleared after update");

        fs::remove_dir_all(&temp_dir_path).unwrap();
    }
}

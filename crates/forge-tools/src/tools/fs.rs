//! # forge-tools: Filesystem MCP Server
//!
//! Implements project-root-contained filesystem tools for reading, listing,
//! writing, deleting, and checking files. Inputs are JSON tool parameters and
//! a `ProjectRoot`; outputs are JSON payloads with file metadata and hashes.
//!
//! ## Input
//! - `path` strings relative to the project root
//! - Write content and write mode
//! - Sacred authorization metadata for guarded writes
//!
//! ## Output
//! - File contents, directory entries, existence information, and SHA-256 hashes
//!
//! ## Related
//! - `forge-core::path` — containment and sacred-file checks
//! - `forge-tools::mcp` — server trait and capability metadata

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use ring::digest::{digest, SHA256};
use serde::Serialize;
use serde_json::{json, Value};
use yantra_core::{canonicalize_within, is_sacred, ProjectRoot};

use crate::error::{McpError, ToolsError, ToolsResult};
use crate::mcp::{McpServer, ToolCapability};

/// Filesystem MCP server scoped to a single project root.
#[derive(Debug, Clone)]
pub struct FsMcpServer {
    project_root: ProjectRoot,
    server_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteMode {
    Overwrite,
    CreateNew,
}

#[derive(Debug, Serialize)]
struct DirectoryEntry {
    name: String,
    kind: String,
    size: u64,
}

impl FsMcpServer {
    /// Creates a filesystem server for a project root.
    pub fn new(project_root: ProjectRoot) -> Self {
        Self {
            project_root,
            server_name: "fs".to_owned(),
        }
    }

    /// Reads a UTF-8 file and returns content, size, and hash.
    ///
    /// # Errors
    ///
    /// Returns an MCP error for invalid parameters, path escapes, IO failures,
    /// or non-UTF-8 file contents.
    pub fn read_file(&self, params: &Value) -> Result<Value, McpError> {
        let requested_path = required_path(params)?;
        let canonical_path = canonicalize_within(&self.project_root, requested_path.as_path())?;
        let file_bytes = std::fs::read(&canonical_path).map_err(|source| ToolsError::Io {
            path: canonical_path.clone(),
            source,
        })?;
        let content = String::from_utf8(file_bytes.clone())
            .map_err(|utf8_error| ToolsError::InvalidParams(utf8_error.to_string()))?;

        Ok(json!({
            "content": content,
            "size": file_bytes.len() as u64,
            "hash": sha256_hex(&file_bytes)
        }))
    }

    /// Lists the immediate entries of a directory.
    ///
    /// # Errors
    ///
    /// Returns an MCP error for invalid parameters, path escapes, or IO failures.
    pub fn list_dir(&self, params: &Value) -> Result<Value, McpError> {
        let requested_path = required_path(params)?;
        let canonical_path = canonicalize_within(&self.project_root, requested_path.as_path())?;
        let directory_reader =
            std::fs::read_dir(&canonical_path).map_err(|source| ToolsError::Io {
                path: canonical_path.clone(),
                source,
            })?;
        let mut entries = Vec::new();

        for directory_entry_result in directory_reader {
            let directory_entry = directory_entry_result.map_err(|source| ToolsError::Io {
                path: canonical_path.clone(),
                source,
            })?;
            let metadata = directory_entry
                .metadata()
                .map_err(|source| ToolsError::Io {
                    path: directory_entry.path(),
                    source,
                })?;
            let kind = if metadata.is_dir() { "dir" } else { "file" };
            entries.push(DirectoryEntry {
                name: directory_entry.file_name().to_string_lossy().into_owned(),
                kind: kind.to_owned(),
                size: metadata.len(),
            });
        }

        entries.sort_by(|left_entry, right_entry| left_entry.name.cmp(&right_entry.name));
        Ok(json!({ "entries": entries }))
    }

    /// Writes a file atomically with sacred-file enforcement.
    ///
    /// # Errors
    ///
    /// Returns an MCP error for invalid parameters, missing sacred authorization,
    /// path escapes, or IO failures.
    pub fn write_file(&self, params: &Value) -> Result<Value, McpError> {
        let requested_path = required_path(params)?;
        let file_content = required_string(params, "content")?;
        let mode = required_string(params, "mode")?;
        let write_mode = parse_write_mode(mode.as_str())?;
        let canonical_path = canonicalize_within(&self.project_root, requested_path.as_path())?;

        if is_sacred(&self.project_root, requested_path.as_path())?
            && !has_sacred_authorization(params)
        {
            return Err(McpError::forbidden(
                "sacred file write requires authorization",
            ));
        }

        if write_mode == WriteMode::CreateNew && canonical_path.exists() {
            return Err(McpError::forbidden("target already exists"));
        }

        if let Some(parent_directory) = canonical_path.parent() {
            std::fs::create_dir_all(parent_directory).map_err(|source| ToolsError::Io {
                path: parent_directory.to_path_buf(),
                source,
            })?;
        }

        let temporary_path = temporary_write_path(&canonical_path);
        let write_result =
            write_file_atomically(&temporary_path, &canonical_path, file_content.as_str());
        if write_result.is_err() {
            remove_temporary_file(&temporary_path)?;
        }
        write_result?;

        Ok(json!({ "hash": sha256_hex(file_content.as_bytes()) }))
    }

    /// Deletes a project-contained file.
    ///
    /// # Errors
    ///
    /// Returns an MCP error for invalid parameters, sacred files, path escapes,
    /// or IO failures.
    pub fn delete_file(&self, params: &Value) -> Result<Value, McpError> {
        let requested_path = required_path(params)?;
        let canonical_path = canonicalize_within(&self.project_root, requested_path.as_path())?;

        if is_sacred(&self.project_root, requested_path.as_path())?
            && !has_sacred_authorization(params)
        {
            return Err(McpError::forbidden(
                "sacred file delete requires authorization",
            ));
        }

        std::fs::remove_file(&canonical_path).map_err(|source| ToolsError::Io {
            path: canonical_path,
            source,
        })?;
        Ok(json!({ "ok": true }))
    }

    /// Checks whether a project-contained path exists.
    ///
    /// # Errors
    ///
    /// Returns an MCP error for invalid parameters or path escapes.
    pub fn exists(&self, params: &Value) -> Result<Value, McpError> {
        let requested_path = required_path(params)?;
        let canonical_path = canonicalize_within(&self.project_root, requested_path.as_path())?;
        let kind = if canonical_path.is_file() {
            Some("file")
        } else if canonical_path.is_dir() {
            Some("dir")
        } else {
            None
        };

        Ok(json!({
            "exists": kind.is_some(),
            "kind": kind
        }))
    }
}

#[async_trait]
impl McpServer for FsMcpServer {
    fn name(&self) -> &str {
        &self.server_name
    }

    async fn handle(&self, method: &str, params: Value) -> Result<Value, McpError> {
        match method {
            "fs.read_file" => self.read_file(&params),
            "fs.list_dir" => self.list_dir(&params),
            "fs.write_file" => self.write_file(&params),
            "fs.delete_file" => self.delete_file(&params),
            "fs.exists" => self.exists(&params),
            _ => Err(McpError::method_not_found(method.to_owned())),
        }
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::ReadFiles,
            ToolCapability::WriteFiles,
            ToolCapability::DeleteFiles,
            ToolCapability::SacredWrite,
        ]
    }
}

fn required_path(params: &Value) -> Result<PathBuf, ToolsError> {
    Ok(PathBuf::from(required_string(params, "path")?))
}

fn required_string(params: &Value, field_name: &str) -> Result<String, ToolsError> {
    params
        .get(field_name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ToolsError::InvalidParams(format!("missing string field `{field_name}`")))
}

fn parse_write_mode(mode: &str) -> Result<WriteMode, ToolsError> {
    match mode {
        "overwrite" => Ok(WriteMode::Overwrite),
        "create_new" => Ok(WriteMode::CreateNew),
        _ => Err(ToolsError::InvalidParams(format!(
            "invalid write mode `{mode}`"
        ))),
    }
}

fn has_sacred_authorization(params: &Value) -> bool {
    params
        .get("sacred_authorization")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || params
            .get("_meta")
            .and_then(|metadata| metadata.get("sacred_authorization"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn write_file_atomically(
    temporary_path: &Path,
    canonical_path: &Path,
    file_content: &str,
) -> ToolsResult<()> {
    std::fs::write(temporary_path, file_content).map_err(|source| ToolsError::Io {
        path: temporary_path.to_path_buf(),
        source,
    })?;
    std::fs::rename(temporary_path, canonical_path).map_err(|source| ToolsError::Io {
        path: canonical_path.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn remove_temporary_file(temporary_path: &Path) -> ToolsResult<()> {
    if temporary_path.exists() {
        std::fs::remove_file(temporary_path).map_err(|source| ToolsError::Io {
            path: temporary_path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

fn temporary_write_path(canonical_path: &Path) -> PathBuf {
    let file_name = canonical_path
        .file_name()
        .unwrap_or_else(|| OsStr::new("yantra-write"));
    canonical_path.with_file_name(format!(".{}.yantra-tmp", file_name.to_string_lossy()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest_bytes = digest(&SHA256, bytes);
    encode_hex(digest_bytes.as_ref())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX_CHARS[(byte >> 4) as usize] as char);
        encoded.push(HEX_CHARS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

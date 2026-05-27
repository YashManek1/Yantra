//! # forge-lsp: LSP Bridge
//!
//! Owns one `LspClient` per language and routes high-level requests
//! (go-to-definition, references, diagnostics, hover, rename) to the
//! correct client. Language is determined from the file extension of the
//! target file. Clients are spawned on first use and kept alive for the
//! process lifetime.
//!
//! ## Input
//! - File path, line, column, and optional new symbol name
//! - Per-language server binary paths (defaults to PATH lookup)
//!
//! ## Output
//! - `Location`, `Vec<Diagnostic>`, `HoverInfo`, `Vec<TextEdit>`
//!
//! ## Related
//! - `forge-lsp::client` — `LspClient` subprocess manager
//! - `forge-tools::tools::lsp` — MCP adapter that calls into this bridge

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use tokio::sync::Mutex;

use crate::client::{extract_location, extract_range, LspClient};
use crate::error::{LspError, LspResult};
use crate::types::{Diagnostic, DiagnosticSeverity, HoverInfo, Language, Location, TextEdit};

pub(crate) const LSP_METHOD_DEFINITION: &str = "textDocument/definition";
pub(crate) const LSP_METHOD_REFERENCES: &str = "textDocument/references";
pub(crate) const LSP_METHOD_HOVER: &str = "textDocument/hover";
pub(crate) const LSP_METHOD_RENAME: &str = "textDocument/rename";
pub(crate) const LSP_METHOD_DIAGNOSTIC: &str = "textDocument/diagnostic";
pub(crate) const LSP_METHOD_DID_OPEN: &str = "textDocument/didOpen";
pub(crate) const LSP_METHOD_INITIALIZE: &str = "initialize";
pub(crate) const LSP_METHOD_INITIALIZED: &str = "initialized";

const LSP_SEVERITY_ERROR: u64 = 1;
const LSP_SEVERITY_WARNING: u64 = 2;
const LSP_SEVERITY_INFORMATION: u64 = 3;

/// Per-language configuration for spawning an LSP server.
#[derive(Debug, Clone)]
pub struct LspServerConfig {
    /// Path to the language server binary.
    pub binary: String,
    /// Additional command-line arguments.
    pub args: Vec<String>,
}

impl LspServerConfig {
    /// Creates a config using the default binary name and arguments for the given language.
    pub fn default_for(language: &Language) -> Self {
        Self {
            binary: language.default_server_binary().to_owned(),
            args: default_args_for(language),
        }
    }
}

/// Routes LSP queries to per-language server processes.
///
/// Spawns one `LspClient` per language on first use and reuses it for all
/// subsequent queries. Thread-safe via `Arc<Mutex<...>>` on the client map.
pub struct LspBridge {
    workspace_root: PathBuf,
    server_configs: HashMap<Language, LspServerConfig>,
    active_clients: Arc<Mutex<HashMap<Language, Arc<LspClient>>>>,
}

impl LspBridge {
    /// Creates a bridge for the given workspace root with default server configs.
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        let mut server_configs = HashMap::new();
        server_configs.insert(
            Language::Rust,
            LspServerConfig::default_for(&Language::Rust),
        );
        server_configs.insert(
            Language::Python,
            LspServerConfig::default_for(&Language::Python),
        );
        server_configs.insert(
            Language::TypeScript,
            LspServerConfig::default_for(&Language::TypeScript),
        );

        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
            server_configs,
            active_clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Creates a bridge with custom server configs (useful for testing).
    pub fn with_configs(
        workspace_root: impl AsRef<Path>,
        server_configs: HashMap<Language, LspServerConfig>,
    ) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
            server_configs,
            active_clients: Arc::new(Mutex::new(HashMap::<Language, Arc<LspClient>>::new())),
        }
    }

    /// Returns a go-to-definition location for a symbol at (file, line, column).
    ///
    /// # Errors
    ///
    /// Returns `LspError::UnsupportedLanguage` if no server handles the file type.
    /// Returns `LspError::SpawnFailed` if the server binary is not on PATH.
    pub async fn get_definition(&self, file: &str, line: u32, column: u32) -> LspResult<Location> {
        let language = language_for_file(file)?;
        self.open_document(file, language.lsp_language_id()).await?;
        let client = self.get_or_spawn(&language).await?;
        let params = text_document_position(file, line, column);
        let response = client.request(LSP_METHOD_DEFINITION, params).await?;

        let location_value = if response.is_array() {
            response
                .as_array()
                .and_then(|array| array.first())
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        } else {
            response
        };

        Ok(extract_location(&location_value))
    }

    /// Returns all reference locations for a symbol at (file, line, column).
    ///
    /// # Errors
    ///
    /// Returns `LspError::UnsupportedLanguage` or `LspError::SpawnFailed`.
    pub async fn find_references(
        &self,
        file: &str,
        line: u32,
        column: u32,
    ) -> LspResult<Vec<Location>> {
        let language = language_for_file(file)?;
        self.open_document(file, language.lsp_language_id()).await?;
        let client = self.get_or_spawn(&language).await?;
        let mut params = text_document_position(file, line, column);
        params["context"] = json!({ "includeDeclaration": true });
        let response = client.request(LSP_METHOD_REFERENCES, params).await?;

        let locations: Vec<Location> = response
            .as_array()
            .map(|array| array.iter().map(extract_location).collect())
            .unwrap_or_default();
        Ok(locations)
    }

    /// Returns all diagnostics the server currently holds for the given file.
    ///
    /// # Errors
    ///
    /// Returns `LspError::UnsupportedLanguage` or `LspError::SpawnFailed`.
    pub async fn get_diagnostics(&self, file: &str) -> LspResult<Vec<Diagnostic>> {
        let language = language_for_file(file)?;
        self.open_document(file, language.lsp_language_id()).await?;
        let client = self.get_or_spawn(&language).await?;
        let file_uri = path_to_lsp_uri(file);

        let response = client
            .request(
                LSP_METHOD_DIAGNOSTIC,
                json!({ "textDocument": { "uri": file_uri } }),
            )
            .await;

        match response {
            Ok(diagnostics_value) => {
                let items = diagnostics_value["items"]
                    .as_array()
                    .or_else(|| diagnostics_value.as_array())
                    .cloned()
                    .unwrap_or_default();
                let diagnostics: Vec<Diagnostic> = items
                    .iter()
                    .map(|item| parse_diagnostic(item, file))
                    .collect();
                Ok(diagnostics)
            }
            Err(LspError::ServerError { .. }) => Ok(vec![]),
            Err(other_error) => Err(other_error),
        }
    }

    /// Returns hover documentation for a symbol at (file, line, column).
    ///
    /// # Errors
    ///
    /// Returns `LspError::UnsupportedLanguage` or `LspError::SpawnFailed`.
    pub async fn get_hover(&self, file: &str, line: u32, column: u32) -> LspResult<HoverInfo> {
        let language = language_for_file(file)?;
        self.open_document(file, language.lsp_language_id()).await?;
        let client = self.get_or_spawn(&language).await?;
        let params = text_document_position(file, line, column);
        let response = client.request(LSP_METHOD_HOVER, params).await?;

        let contents_text = match &response["contents"] {
            serde_json::Value::String(text_value) => text_value.clone(),
            object_value if object_value.is_object() => {
                object_value["value"].as_str().unwrap_or("").to_owned()
            }
            array_value if array_value.is_array() => {
                if let Some(elements) = array_value.as_array() {
                    elements
                        .iter()
                        .filter_map(|element| {
                            if element.is_string() {
                                element.as_str().map(str::to_owned)
                            } else {
                                element["value"].as_str().map(str::to_owned)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    String::new()
                }
            }
            _ => String::new(),
        };

        let hover_range = if response["range"].is_object() {
            Some(extract_location(&json!({
                "uri": format!("file://{file}"),
                "range": response["range"]
            })))
        } else {
            None
        };

        Ok(HoverInfo {
            contents: contents_text,
            range: hover_range,
        })
    }

    /// Returns text edits for renaming the symbol at (file, line, column).
    ///
    /// # Errors
    ///
    /// Returns `LspError::CapabilityUnsupported` if the server does not support rename.
    pub async fn rename_symbol(
        &self,
        file: &str,
        line: u32,
        column: u32,
        new_name: &str,
    ) -> LspResult<Vec<TextEdit>> {
        let language = language_for_file(file)?;
        self.open_document(file, language.lsp_language_id()).await?;
        let client = self.get_or_spawn(&language).await?;
        let file_uri = path_to_lsp_uri(file);

        let params = json!({
            "textDocument": { "uri": file_uri },
            "position": { "line": line, "character": column },
            "newName": new_name
        });

        let response = client.request(LSP_METHOD_RENAME, params).await?;

        let mut text_edits: Vec<TextEdit> = Vec::new();
        if let Some(changes_map) = response["changes"].as_object() {
            for (uri_key, edits_array) in changes_map {
                let edit_file = uri_key
                    .strip_prefix("file:///")
                    .or_else(|| uri_key.strip_prefix("file://"))
                    .unwrap_or(uri_key);
                if let Some(edit_items) = edits_array.as_array() {
                    for edit in edit_items {
                        let (start_line, start_column, end_line, end_column) =
                            extract_range(&edit["range"]);
                        text_edits.push(TextEdit {
                            file: edit_file.to_owned(),
                            start_line,
                            start_column,
                            end_line,
                            end_column,
                            new_text: edit["newText"].as_str().unwrap_or(new_name).to_owned(),
                        });
                    }
                }
            }
        }

        Ok(text_edits)
    }

    /// Sends a `textDocument/didOpen` notification before any query.
    ///
    /// Required by the LSP protocol: servers will not respond to definition,
    /// references, hover, rename, or diagnostic requests until the document
    /// has been opened. Reads the file content from disk synchronously.
    async fn open_document(&self, file_path: &str, language_id: &str) -> LspResult<()> {
        let file_uri = path_to_lsp_uri(file_path);
        let file_content = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|io_error| LspError::Framing(io_error.to_string()))?;

        let language = language_for_file(file_path)?;
        let client = self.get_or_spawn(&language).await?;
        client
            .notify(
                LSP_METHOD_DID_OPEN,
                json!({
                    "textDocument": {
                        "uri": file_uri,
                        "languageId": language_id,
                        "version": 1,
                        "text": file_content
                    }
                }),
            )
            .await
    }

    async fn get_or_spawn(&self, language: &Language) -> LspResult<Arc<LspClient>> {
        let mut client_map = self.active_clients.lock().await;
        if !client_map.contains_key(language) {
            let server_config =
                self.server_configs
                    .get(language)
                    .ok_or_else(|| LspError::UnsupportedLanguage {
                        language: format!("{language:?}"),
                    })?;
            let arg_refs: Vec<&str> = server_config.args.iter().map(String::as_str).collect();
            let new_client = LspClient::spawn(
                &server_config.binary,
                &arg_refs,
                &self.workspace_root,
                &format!("{language:?}"),
            )
            .await?;
            client_map.insert(language.clone(), Arc::new(new_client));
        }
        Ok(Arc::clone(
            client_map
                .get(language)
                .expect("key was just inserted into the client map"),
        ))
    }
}

/// Determines the LSP language from a file path's extension.
///
/// Returns `LspError::UnsupportedLanguage` for unknown extensions.
fn language_for_file(file: &str) -> LspResult<Language> {
    let extension = Path::new(file)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");
    Language::from_extension(extension).ok_or_else(|| LspError::UnsupportedLanguage {
        language: extension.to_owned(),
    })
}

/// Builds an LSP `textDocument/position` parameter JSON value.
///
/// The URI is derived from `file` using `path_to_lsp_uri`.
fn text_document_position(file: &str, line: u32, column: u32) -> serde_json::Value {
    json!({
        "textDocument": { "uri": path_to_lsp_uri(file) },
        "position": { "line": line, "character": column }
    })
}

/// Converts a file path or existing `file://` URI to an LSP-compatible URI.
///
/// Already-valid `file://` URIs are returned unchanged. Unix absolute paths
/// are prefixed with `file://`. Windows paths have backslashes normalized
/// and receive a `file:///` prefix.
pub(crate) fn path_to_lsp_uri(file: &str) -> String {
    if file.starts_with("file://") {
        file.to_owned()
    } else if file.starts_with('/') {
        format!("file://{file}")
    } else {
        let forward_slash = file.replace('\\', "/");
        format!("file:///{forward_slash}")
    }
}

/// Parses one LSP diagnostic JSON object into a typed `Diagnostic`.
///
/// Falls back to `DiagnosticSeverity::Hint` for unknown severity codes.
/// The `file` argument is used as the `Location::file` field.
fn parse_diagnostic(item: &serde_json::Value, file: &str) -> Diagnostic {
    let severity = match item["severity"].as_u64().unwrap_or(LSP_SEVERITY_ERROR) {
        severity_code if severity_code == LSP_SEVERITY_ERROR => DiagnosticSeverity::Error,
        severity_code if severity_code == LSP_SEVERITY_WARNING => DiagnosticSeverity::Warning,
        severity_code if severity_code == LSP_SEVERITY_INFORMATION => {
            DiagnosticSeverity::Information
        }
        _ => DiagnosticSeverity::Hint,
    };
    let (start_line, start_column, end_line, end_column) = extract_range(&item["range"]);
    Diagnostic {
        severity,
        message: item["message"].as_str().unwrap_or("").to_owned(),
        location: Location {
            file: file.to_owned(),
            start_line,
            start_column,
            end_line,
            end_column,
        },
        code: item["code"].as_str().map(str::to_owned),
    }
}

/// Returns the default command-line arguments for a given language server.
fn default_args_for(language: &Language) -> Vec<String> {
    match language {
        Language::Rust => vec![],
        Language::Python => vec!["--stdio".to_owned()],
        Language::TypeScript => vec!["--stdio".to_owned()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_for_rust_file() {
        let language = language_for_file("src/lib.rs").unwrap();
        assert_eq!(language, Language::Rust);
    }

    #[test]
    fn test_language_for_python_file() {
        let language = language_for_file("script.py").unwrap();
        assert_eq!(language, Language::Python);
    }

    #[test]
    fn test_language_for_typescript_file() {
        let language = language_for_file("index.ts").unwrap();
        assert_eq!(language, Language::TypeScript);
    }

    #[test]
    fn test_language_for_unknown_extension_returns_error() {
        let result = language_for_file("document.md");
        assert!(result.is_err());
        match result.unwrap_err() {
            LspError::UnsupportedLanguage { language } => assert_eq!(language, "md"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn test_path_to_lsp_uri_already_uri() {
        let uri = path_to_lsp_uri("file:///src/lib.rs");
        assert_eq!(uri, "file:///src/lib.rs");
    }

    #[test]
    fn test_path_to_lsp_uri_unix_path() {
        let uri = path_to_lsp_uri("/home/user/src/lib.rs");
        assert_eq!(uri, "file:///home/user/src/lib.rs");
    }

    #[test]
    fn test_parse_diagnostic_extracts_fields() {
        let item = json!({
            "severity": 1,
            "message": "mismatched types",
            "range": {
                "start": { "line": 5, "character": 4 },
                "end":   { "line": 5, "character": 8 }
            }
        });
        let diagnostic = parse_diagnostic(&item, "src/lib.rs");
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostic.message, "mismatched types");
        assert_eq!(diagnostic.location.start_line, 5);
    }

    #[test]
    fn test_parse_diagnostic_warning_severity() {
        let item = json!({
            "severity": 2,
            "message": "unused variable",
            "range": {
                "start": { "line": 3, "character": 0 },
                "end":   { "line": 3, "character": 5 }
            }
        });
        let diagnostic = parse_diagnostic(&item, "src/main.rs");
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Warning);
    }
}

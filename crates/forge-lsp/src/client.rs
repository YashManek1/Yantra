//! # forge-lsp: LSP Client
//!
//! Spawns a language server as a subprocess, performs the `initialize` /
//! `initialized` handshake, and routes JSON-RPC requests/responses over
//! the process's stdin/stdout using the Content-Length framing required by
//! the LSP specification. Responses are matched to pending requests via an
//! in-process `id → oneshot::Sender` map.
//!
//! ## Input
//! - Server binary path and optional arguments
//! - JSON-RPC method names and parameter values
//! - Project root URI for LSP workspace configuration
//!
//! ## Output
//! - JSON `Value` responses from the language server
//! - `LspError` for transport, framing, and protocol failures
//!
//! ## Related
//! - `forge-lsp::types` — `Language` enum used to select the client
//! - `forge-lsp::bridge` — owns one `LspClient` per language

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::time::Duration;

use crate::error::{LspError, LspResult};

const LSP_REQUEST_TIMEOUT_MS: u64 = 10_000;

/// Manages a single language server subprocess and its JSON-RPC communication.
///
/// The client spawns the server, performs the LSP `initialize`/`initialized` handshake,
/// and multiplexes concurrent requests over the process's stdin/stdout.
pub struct LspClient {
    server_binary: String,
    stdin_writer: Arc<Mutex<ChildStdin>>,
    pending_requests: Arc<Mutex<HashMap<i64, oneshot::Sender<LspResult<Value>>>>>,
    next_request_id: Arc<AtomicI64>,
    language_label: String,
    process: Child,
}

impl LspClient {
    /// Spawns the language server binary and performs the LSP initialize handshake.
    ///
    /// # Errors
    ///
    /// Returns `LspError::SpawnFailed` if the binary cannot be launched.
    /// Returns `LspError::ServerError` if the server rejects the initialize request.
    pub async fn spawn(
        server_binary: &str,
        server_args: &[&str],
        workspace_root: &Path,
        language_label: &str,
    ) -> LspResult<Self> {
        let mut child_process = Command::new(server_binary)
            .args(server_args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|io_error| LspError::SpawnFailed {
                language: language_label.to_owned(),
                source: io_error,
            })?;

        // INVARIANT: stdin and stdout are piped because we called .stdin(Stdio::piped())
        // and .stdout(Stdio::piped()) on the Command above. These takes cannot fail.
        let child_stdin = child_process
            .stdin
            .take()
            .expect("stdin is piped per the Command configuration above");
        let child_stdout = child_process
            .stdout
            .take()
            .expect("stdout is piped per the Command configuration above");

        let pending_requests: Arc<Mutex<HashMap<i64, oneshot::Sender<LspResult<Value>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stdin_writer = Arc::new(Mutex::new(child_stdin));
        let next_request_id = Arc::new(AtomicI64::new(1));

        let pending_requests_clone = Arc::clone(&pending_requests);
        let language_for_reader = language_label.to_owned();
        tokio::spawn(async move {
            read_stdout_loop(child_stdout, pending_requests_clone, language_for_reader).await;
        });

        let workspace_uri = crate::bridge::path_to_lsp_uri(workspace_root.to_str().unwrap_or(""));
        let client = Self {
            server_binary: server_binary.to_owned(),
            stdin_writer,
            pending_requests,
            next_request_id,
            language_label: language_label.to_owned(),
            process: child_process,
        };

        client.initialize(&workspace_uri).await?;

        Ok(client)
    }

    /// Sends a JSON-RPC request and waits for the response.
    ///
    /// # Errors
    ///
    /// Returns `LspError::Timeout` if no response arrives within 10 seconds.
    /// Returns `LspError::ServerError` for error responses from the server.
    pub async fn request(&self, method: &str, params: Value) -> LspResult<Value> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let (response_sender, response_receiver) = oneshot::channel();

        {
            let mut pending_map = self.pending_requests.lock().await;
            pending_map.insert(request_id, response_sender);
        }

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        });

        self.write_message(&request_body).await?;

        tokio::time::timeout(
            Duration::from_millis(LSP_REQUEST_TIMEOUT_MS),
            response_receiver,
        )
        .await
        .map_err(|_| LspError::Timeout {
            timeout_ms: LSP_REQUEST_TIMEOUT_MS,
        })?
        .map_err(|_| LspError::ProcessExited {
            language: self.language_label.clone(),
        })?
    }

    /// Sends a JSON-RPC notification (no response expected).
    ///
    /// # Errors
    ///
    /// Returns `LspError::Framing` if the message cannot be written to the server's stdin.
    pub async fn notify(&self, method: &str, params: Value) -> LspResult<()> {
        let notification_body = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_message(&notification_body).await
    }

    async fn initialize(&self, workspace_uri: &str) -> LspResult<()> {
        let initialize_params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "yantra", "version": env!("CARGO_PKG_VERSION") },
            "rootUri": workspace_uri,
            "capabilities": {
                "textDocument": {
                    "definition": { "linkSupport": false },
                    "references": {},
                    "hover": { "contentFormat": ["plaintext"] },
                    "rename": {},
                    "publishDiagnostics": {}
                },
                "workspace": {}
            }
        });

        self.request(crate::bridge::LSP_METHOD_INITIALIZE, initialize_params)
            .await?;
        self.notify(crate::bridge::LSP_METHOD_INITIALIZED, json!({}))
            .await?;
        Ok(())
    }

    async fn write_message(&self, message: &Value) -> LspResult<()> {
        let json_body = serde_json::to_string(message)?;
        let content_length_header = format!("Content-Length: {}\r\n\r\n", json_body.len());

        let mut stdin_guard = self.stdin_writer.lock().await;
        stdin_guard
            .write_all(content_length_header.as_bytes())
            .await
            .map_err(|io_error| LspError::Framing(io_error.to_string()))?;
        stdin_guard
            .write_all(json_body.as_bytes())
            .await
            .map_err(|io_error| LspError::Framing(io_error.to_string()))?;
        stdin_guard
            .flush()
            .await
            .map_err(|io_error| LspError::Framing(io_error.to_string()))?;
        Ok(())
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if let Err(kill_error) = self.process.start_kill() {
            tracing::warn!(
                server_binary = %self.server_binary,
                error = %kill_error,
                "LSP server process was already gone or could not be killed on drop"
            );
        }
    }
}

/// Continuously reads LSP messages from stdout and resolves pending requests.
///
/// Runs in a dedicated tokio task for each language server process. The loop
/// exits when the server closes its stdout (process exit or pipe error).
async fn read_stdout_loop(
    child_stdout: ChildStdout,
    pending_requests: Arc<Mutex<HashMap<i64, oneshot::Sender<LspResult<Value>>>>>,
    language_label: String,
) {
    let mut buffered_reader = BufReader::new(child_stdout);

    loop {
        let parsed_message = read_lsp_message(&mut buffered_reader).await;
        match parsed_message {
            Err(_) => break,
            Ok(message_value) => {
                let message_id = message_value.get("id").and_then(Value::as_i64);
                if let Some(request_id) = message_id {
                    let mut pending_map = pending_requests.lock().await;
                    if let Some(response_sender) = pending_map.remove(&request_id) {
                        let response_result = if let Some(error_object) = message_value.get("error")
                        {
                            Err(LspError::ServerError {
                                code: error_object
                                    .get("code")
                                    .and_then(Value::as_i64)
                                    .unwrap_or(-1),
                                message: error_object
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("unknown error")
                                    .to_owned(),
                            })
                        } else {
                            Ok(message_value.get("result").cloned().unwrap_or(Value::Null))
                        };
                        let _ = response_sender.send(response_result);
                    }
                }
            }
        }
    }

    tracing::warn!(language = %language_label, "LSP stdout reader loop exited");
}

/// Reads one Content-Length framed LSP message from the buffered reader.
///
/// Parses LSP headers, extracts the `Content-Length` value, reads exactly
/// that many bytes, and deserializes the result as JSON.
async fn read_lsp_message(buffered_reader: &mut BufReader<ChildStdout>) -> LspResult<Value> {
    let mut content_length: Option<usize> = None;

    loop {
        let mut header_line = String::new();
        let bytes_read = buffered_reader
            .read_line(&mut header_line)
            .await
            .map_err(|io_error| LspError::Framing(io_error.to_string()))?;
        if bytes_read == 0 {
            return Err(LspError::Framing("EOF reading LSP headers".to_owned()));
        }
        let trimmed_header = header_line.trim();
        if trimmed_header.is_empty() {
            break;
        }
        if let Some(length_value) = trimmed_header.strip_prefix("Content-Length: ") {
            content_length = length_value.parse().ok();
        }
    }

    let message_content_length = content_length
        .ok_or_else(|| LspError::Framing("missing Content-Length header".to_owned()))?;

    let mut body_buffer = vec![0u8; message_content_length];
    buffered_reader
        .read_exact(&mut body_buffer)
        .await
        .map_err(|io_error| LspError::Framing(io_error.to_string()))?;

    serde_json::from_slice(&body_buffer).map_err(LspError::Json)
}

/// Converts an LSP `Range` JSON object to `(start_line, start_col, end_line, end_col)`.
///
/// Returns `(0, 0, 0, 0)` for any missing or non-numeric fields.
pub(crate) fn extract_range(range: &Value) -> (u32, u32, u32, u32) {
    let start_line = range["start"]["line"].as_u64().unwrap_or(0) as u32;
    let start_column = range["start"]["character"].as_u64().unwrap_or(0) as u32;
    let end_line = range["end"]["line"].as_u64().unwrap_or(0) as u32;
    let end_column = range["end"]["character"].as_u64().unwrap_or(0) as u32;
    (start_line, start_column, end_line, end_column)
}

/// Converts an LSP `Location` JSON object to a `crate::types::Location`.
///
/// Strips the `file:///` or `file://` URI prefix from the file path.
pub(crate) fn extract_location(location_value: &Value) -> crate::types::Location {
    let raw_uri = location_value["uri"].as_str().unwrap_or("");
    let file_path = raw_uri
        .strip_prefix("file:///")
        .or_else(|| raw_uri.strip_prefix("file://"))
        .unwrap_or(raw_uri)
        .to_owned();
    let (start_line, start_column, end_line, end_column) = extract_range(&location_value["range"]);
    crate::types::Location {
        file: file_path,
        start_line,
        start_column,
        end_line,
        end_column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_to_uri_windows_style() {
        let path = std::path::Path::new("C:\\Users\\user\\project");
        let uri = crate::bridge::path_to_lsp_uri(&path.to_string_lossy());
        assert!(uri.starts_with("file:///"));
    }

    #[test]
    fn test_extract_range_from_lsp_json() {
        let range = json!({
            "start": { "line": 10, "character": 4 },
            "end":   { "line": 10, "character": 15 }
        });
        let (start_line, start_col, end_line, end_col) = extract_range(&range);
        assert_eq!((start_line, start_col, end_line, end_col), (10, 4, 10, 15));
    }

    #[test]
    fn test_extract_location_strips_file_prefix() {
        let location = json!({
            "uri": "file:///src/lib.rs",
            "range": {
                "start": { "line": 5, "character": 0 },
                "end":   { "line": 5, "character": 10 }
            }
        });
        let extracted = extract_location(&location);
        assert_eq!(extracted.file, "src/lib.rs");
        assert_eq!(extracted.start_line, 5);
    }
}

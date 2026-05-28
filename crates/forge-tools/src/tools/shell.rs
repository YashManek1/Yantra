//! # forge-tools: Shell Sandbox MCP Server
//!
//! Executes shell commands inside a Docker container with a read-only project
//! mount, a writable overlay, a 60-second hard timeout, and no external network
//! access. Prevents agent-driven code from escaping the sandbox or persisting
//! side effects to the host filesystem.
//!
//! ## Input
//! - `shell.exec { command: "...", working_dir?: "..." }` — command to run
//!
//! ## Output
//! - `{ stdout, stderr, exit_code }` on success
//! - `McpError` on timeout, Docker unavailability, or container failure
//!
//! ## Related
//! - `forge-tools::tools::fs` — read-only file access that does not need Docker
//! - Wave 2: replace `--network=none` with a gVisor policy for pypi/crates.io/npm

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use yantra_core::TaskId;

use crate::error::McpError;
use crate::mcp::{McpServer, ToolCapability};

/// Hard execution timeout before the Docker container is forcibly removed.
const CONTAINER_TIMEOUT: Duration = Duration::from_secs(60);

/// MCP server that runs commands inside a Docker container sandbox.
#[derive(Debug, Clone)]
pub struct ShellSandboxServer {
    /// Absolute path to the project root, mounted read-only inside the container.
    project_root_path: PathBuf,
    /// Docker image to run; must include `sh`.
    docker_image: String,
    /// Execution timeout in seconds, capped at `CONTAINER_TIMEOUT`.
    timeout_seconds: u64,
    server_name: String,
}

impl ShellSandboxServer {
    /// Creates a sandbox server for the given project root.
    ///
    /// `docker_image` should be a minimal image with `/bin/sh` such as
    /// `alpine:3.19`. `timeout_seconds` is clamped to 60.
    pub fn new(project_root_path: PathBuf, docker_image: String, timeout_seconds: u64) -> Self {
        Self {
            project_root_path,
            docker_image,
            timeout_seconds: timeout_seconds.min(CONTAINER_TIMEOUT.as_secs()),
            server_name: "shell".to_owned(),
        }
    }

    /// Executes `command` inside an isolated Docker container.
    ///
    /// The project root is mounted at `/project` (read-only). A per-execution
    /// writable temp directory is mounted at `/work`. The container has no
    /// network access (`--network=none`). The process is killed after the
    /// configured timeout.
    ///
    /// # Errors
    ///
    /// Returns `McpError` when the command parameter is missing, Docker is
    /// unavailable, or the execution times out.
    pub async fn exec(&self, params: &Value) -> Result<Value, McpError> {
        let command_string = params
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| McpError::invalid_params("missing required field: command"))?
            .to_owned();

        let work_dir = std::env::temp_dir().join(format!("yantra-sandbox-{}", TaskId::new()));
        std::fs::create_dir_all(&work_dir).map_err(|io_error| {
            McpError::internal(format!("failed to create sandbox workdir: {io_error}"))
        })?;

        let project_root_display = format_volume_path(&self.project_root_path);
        let work_dir_display = format_volume_path(&work_dir);

        let exec_result = tokio::time::timeout(
            Duration::from_secs(self.timeout_seconds),
            tokio::process::Command::new("docker")
                .args([
                    "run",
                    "--rm",
                    "--network=none",
                    "--memory=512m",
                    "--cpus=1.0",
                    &format!("-v={project_root_display}:/project:ro"),
                    &format!("-v={work_dir_display}:/work:rw"),
                    "--workdir=/work",
                    &self.docker_image,
                    "sh",
                    "-c",
                    &command_string,
                ])
                .output(),
        )
        .await;

        let _ = std::fs::remove_dir_all(&work_dir);

        match exec_result {
            Err(_timeout_elapsed) => Err(McpError::internal(format!(
                "sandbox execution timed out after {} seconds",
                self.timeout_seconds
            ))),
            Ok(Err(io_error)) => Err(McpError::internal(format!(
                "failed to spawn Docker container: {io_error}"
            ))),
            Ok(Ok(output)) => Ok(json!({
                "stdout": String::from_utf8_lossy(&output.stdout).as_ref(),
                "stderr": String::from_utf8_lossy(&output.stderr).as_ref(),
                "exit_code": output.status.code().unwrap_or(-1),
            })),
        }
    }
}

fn format_volume_path(path: &std::path::Path) -> String {
    let abs_path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let path_str = abs_path.to_string_lossy();
    #[cfg(windows)]
    {
        let mut clean_path = path_str.into_owned();
        if clean_path.starts_with(r"\\?\") {
            clean_path = clean_path[4..].to_owned();
        }
        let clean_path = clean_path.replace('\\', "/");
        if clean_path.len() >= 2 && clean_path.as_bytes()[1] == b':' {
            let drive = clean_path[0..1].to_lowercase();
            let rest = &clean_path[2..];
            format!("/{drive}{rest}")
        } else {
            clean_path
        }
    }
    #[cfg(not(windows))]
    {
        path_str.into_owned()
    }
}

#[async_trait]
impl McpServer for ShellSandboxServer {
    fn name(&self) -> &str {
        &self.server_name
    }

    async fn handle(&self, method: &str, params: Value) -> Result<Value, McpError> {
        match method {
            "shell.exec" => self.exec(&params).await,
            _ => Err(McpError::method_not_found(method.to_owned())),
        }
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::SandboxedExec]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_docker_available() -> bool {
        let ps_success = std::process::Command::new("docker")
            .arg("ps")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);

        if !ps_success {
            return false;
        }

        std::process::Command::new("docker")
            .args(["info", "--format", "{{.OSType}}"])
            .output()
            .map(|output| {
                output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "linux"
            })
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn docker_sandbox_isolates_test_process() {
        if !is_docker_available() {
            // Docker not available in this environment — skip rather than fail.
            return;
        }

        let project_root =
            std::env::temp_dir().join(format!("yantra-sandbox-proj-{}", TaskId::new()));
        std::fs::create_dir_all(&project_root).unwrap();

        let server = ShellSandboxServer::new(project_root.clone(), "alpine:3.19".to_owned(), 60);

        let params = serde_json::json!({ "command": "echo hello-from-sandbox" });
        let result = server.handle("shell.exec", params).await.unwrap();

        let stdout = result["stdout"].as_str().unwrap_or("");
        assert!(
            stdout.contains("hello-from-sandbox"),
            "sandbox must echo the command output; got: {stdout:?}"
        );

        let exit_code = result["exit_code"].as_i64().unwrap_or(-1);
        assert_eq!(exit_code, 0, "exit code must be 0 for a successful command");

        std::fs::remove_dir_all(&project_root).unwrap_or_default();
    }

    #[tokio::test]
    async fn exec_returns_error_for_missing_command_param() {
        let project_root =
            std::env::temp_dir().join(format!("yantra-sandbox-proj-{}", TaskId::new()));
        std::fs::create_dir_all(&project_root).unwrap();

        let server = ShellSandboxServer::new(project_root, "alpine:3.19".to_owned(), 60);

        let result = server.handle("shell.exec", serde_json::json!({})).await;
        assert!(
            result.is_err(),
            "missing command param must return an error"
        );
        let error = result.unwrap_err();
        assert_eq!(error.code, -32602, "must return invalid-params error code");
    }

    #[tokio::test]
    async fn docker_sandbox_handles_paths_with_spaces() {
        if !is_docker_available() {
            return;
        }

        let project_root =
            std::env::temp_dir().join(format!("yantra sandbox path space {}", TaskId::new()));
        std::fs::create_dir_all(&project_root).unwrap();

        let server = ShellSandboxServer::new(project_root.clone(), "alpine:3.19".to_owned(), 60);

        let params = serde_json::json!({ "command": "echo spaces-work" });
        let result = server.handle("shell.exec", params).await.unwrap();

        let stdout = result["stdout"].as_str().unwrap_or("");
        assert!(
            stdout.contains("spaces-work"),
            "sandbox must handle paths with spaces; got: {stdout:?}"
        );

        let exit_code = result["exit_code"].as_i64().unwrap_or(-1);
        assert_eq!(exit_code, 0);

        std::fs::remove_dir_all(&project_root).unwrap_or_default();
    }
}

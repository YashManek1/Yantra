//! # forge-tools: Public MCP Integration Tests
//!
//! Exercises filesystem, Git, and JSON-RPC behavior through the public
//! `yantra-tools` API. Inputs are temporary project roots; outputs are MCP
//! JSON values and JSON-RPC frames.
//!
//! ## Input
//! - Temporary project directories
//! - Public server and router APIs
//!
//! ## Output
//! - Assertions over path safety, sacred-file enforcement, Git operations, and framing
//!
//! ## Related
//! - `yantra_tools::FsMcpServer`
//! - `yantra_tools::GitMcpServer`
//! - `yantra_tools::McpRouter`

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use yantra_core::{ProjectRoot, TaskId};
use yantra_tools::{FsMcpServer, GitMcpServer, McpRouter};

#[test]
fn fs_path_traversal_is_blocked() {
    let project_directory = temporary_project("fs-traversal");
    let project_root = ProjectRoot::new(&project_directory).unwrap();
    let server = FsMcpServer::new(project_root);

    let response = server.read_file(&json!({ "path": "../../../etc/passwd" }));

    assert!(response.is_err());
}

#[test]
fn sacred_file_write_without_authorization_is_blocked() {
    let project_directory = temporary_project("fs-sacred");
    let yantra_directory = project_directory.join(".yantra");
    std::fs::create_dir_all(&yantra_directory).unwrap();
    std::fs::write(yantra_directory.join("sacred.txt"), "secret.txt\n").unwrap();
    let project_root = ProjectRoot::new(&project_directory).unwrap();
    let server = FsMcpServer::new(project_root);

    let response = server.write_file(&json!({
        "path": "secret.txt",
        "content": "classified",
        "mode": "overwrite"
    }));

    assert!(response.is_err());
    assert!(!project_directory.join("secret.txt").exists());
}

#[test]
fn atomic_write_removes_temporary_file_on_failure() {
    let project_directory = temporary_project("fs-atomic");
    std::fs::create_dir_all(project_directory.join("target.txt")).unwrap();
    let project_root = ProjectRoot::new(&project_directory).unwrap();
    let server = FsMcpServer::new(project_root);

    let response = server.write_file(&json!({
        "path": "target.txt",
        "content": "new content",
        "mode": "overwrite"
    }));

    assert!(response.is_err());
    assert!(!project_directory.join(".target.txt.yantra-tmp").exists());
}

#[test]
fn git_operations_work_on_temporary_fixture_repository() {
    let project_directory = temporary_project("git-fixture");
    create_fixture_repository(&project_directory);
    std::fs::write(project_directory.join("new.txt"), "new").unwrap();
    let project_root = ProjectRoot::new(&project_directory).unwrap();
    let server = GitMcpServer::new(project_root);

    let status = server.status().unwrap();
    assert_eq!(status["branch"], "main");
    assert!(status["untracked"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path_value| path_value == "new.txt"));

    let create_response = server
        .branch_create(&json!({ "name": "feature/tools" }))
        .unwrap();
    assert_eq!(create_response["ok"], true);

    let checkout_response = server
        .branch_checkout(&json!({ "name": "feature/tools" }))
        .unwrap();
    assert_eq!(checkout_response["ok"], true);
    assert_eq!(server.status().unwrap()["branch"], "feature/tools");
}

#[tokio::test]
async fn mcp_json_rpc_round_trip_returns_result_frame() {
    let project_directory = temporary_project("mcp-roundtrip");
    std::fs::write(project_directory.join("README.md"), "hello").unwrap();
    let project_root = ProjectRoot::new(&project_directory).unwrap();
    let mut router = McpRouter::new(["fs.exists".to_owned()]);
    router.register_server(Arc::new(FsMcpServer::new(project_root)));

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "fs.exists",
        "params": { "path": "README.md" }
    });
    let response_line = router
        .handle_json_rpc_line(&request.to_string())
        .await
        .unwrap();
    let response: Value = serde_json::from_str(&response_line).unwrap();

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["exists"], true);
    assert!(response.get("error").is_none());
}

fn temporary_project(label: &str) -> PathBuf {
    let project_directory =
        std::env::temp_dir().join(format!("yantra-tools-{label}-{}", TaskId::new()));
    if project_directory.exists() {
        std::fs::remove_dir_all(&project_directory).unwrap();
    }
    std::fs::create_dir_all(&project_directory).unwrap();
    project_directory
}

fn create_fixture_repository(project_directory: &Path) {
    let git_directory = project_directory.join(".git");
    std::fs::create_dir_all(git_directory.join("refs").join("heads")).unwrap();
    std::fs::create_dir_all(git_directory.join("logs")).unwrap();
    std::fs::write(git_directory.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(
        git_directory.join("refs").join("heads").join("main"),
        "1111111111111111111111111111111111111111\n",
    )
    .unwrap();
    std::fs::write(
        git_directory.join("config"),
        "[core]\n\trepositoryformatversion = 0\n\tfilemode = false\n\tbare = false\n",
    )
    .unwrap();
    std::fs::write(
        git_directory.join("logs").join("HEAD"),
        "0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 Test <test@example.com> 1700000000 +0000\tcommit: initial\n",
    )
    .unwrap();
}

//! # Path Traversal Vulnerability Tests
//!
//! Verifies that absolute paths, parent directory traversals, and path-traversing
//! branch names are correctly rejected by containment utilities.
//!
//! ## Input
//! - Project roots and test directory fixtures
//! - Crafted traversing tool parameters
//!
//! ## Output
//! - Execution success or expected error outcomes
//!
//! ## Related
//! - `forge-core::path` — containment utilities
//! - `forge-tools::tools::fs` — filesystem MCP server
//! - `forge-tools::tools::git` — git MCP server

use std::path::{Path, PathBuf};
use yantra_core::path::{canonicalize_within, ProjectRoot};
use yantra_tools::{FsMcpServer, GitMcpServer, McpServer};

#[test]
fn test_canonicalize_within_absolute_and_traversal_guards() {
    let temporary_dir = tempfile::tempdir().unwrap();
    let root_path = temporary_dir.path().to_path_buf();
    let project_root = ProjectRoot::new(&root_path).unwrap();

    let invalid_traversal_path = Path::new("../escaped_file.txt");
    let result_traversal = canonicalize_within(&project_root, invalid_traversal_path);
    assert!(result_traversal.is_err());

    let outside_absolute_path = Path::new("/etc/passwd");
    let result_absolute = canonicalize_within(&project_root, outside_absolute_path);
    assert!(result_absolute.is_err());
}

#[test]
fn test_fs_mcp_write_path_traversal_blocked() {
    let temporary_dir = tempfile::tempdir().unwrap();
    let root_path = temporary_dir.path().to_path_buf();
    let project_root = ProjectRoot::new(&root_path).unwrap();
    let filesystem_server = FsMcpServer::new(project_root);

    let params_traversal = serde_json::json!({
        "path": "../malicious.txt",
        "content": "payload",
        "mode": "overwrite"
    });
    let result_traversal = filesystem_server.write_file(&params_traversal);
    assert!(result_traversal.is_err());

    let params_absolute = serde_json::json!({
        "path": "/etc/passwd",
        "content": "payload",
        "mode": "overwrite"
    });
    let result_absolute = filesystem_server.write_file(&params_absolute);
    assert!(result_absolute.is_err());
}

#[test]
fn test_fs_mcp_apply_diff_path_traversal_blocked() {
    let temporary_dir = tempfile::tempdir().unwrap();
    let root_path = temporary_dir.path().to_path_buf();
    let project_root = ProjectRoot::new(&root_path).unwrap();
    let filesystem_server = FsMcpServer::new(project_root);

    let params_traversal = serde_json::json!({
        "path": "../malicious.txt",
        "diff": "some diff text"
    });
    let result_traversal = filesystem_server.apply_diff(&params_traversal);
    assert!(result_traversal.is_err());
}

#[tokio::test]
async fn test_git_mcp_branch_name_traversal_blocked() {
    let temporary_dir = tempfile::tempdir().unwrap();
    let root_path = temporary_dir.path().to_path_buf();
    let project_root = ProjectRoot::new(&root_path).unwrap();
    let git_server = GitMcpServer::new(project_root);

    let params_traversal = serde_json::json!({
        "name": "../../outside/ref"
    });
    let result_traversal = git_server.handle("git.branch_create", params_traversal).await;
    assert!(result_traversal.is_err());
}

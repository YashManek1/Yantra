//! # Sacred Write Bypass Vulnerability Tests
//!
//! Verifies that sacred-file checks cannot be bypassed using simple boolean flags,
//! and that only validly signed Ed25519 `TruthToken`s are accepted.
//!
//! ## Input
//! - Setup of sacred pattern configs
//! - Signed and unsigned truth tokens
//!
//! ## Output
//! - Verification outcomes of allowed or blocked writes
//!
//! ## Related
//! - `forge-tools::tools::fs` — filesystem MCP server
//! - `forge-stvp::token` — token verification

use std::path::PathBuf;
use yantra_core::path::ProjectRoot;
use yantra_core::{Strictness, TaskClass, TaskId, TruthToken};
use yantra_tools::{FsMcpServer, McpServer, SacredGuardServer};

fn setup_sacred_project() -> (tempfile::TempDir, ProjectRoot) {
    let temporary_dir = tempfile::tempdir().unwrap();
    let root_path = temporary_dir.path().to_path_buf();
    let yantra_dir = root_path.join(".yantra");
    std::fs::create_dir_all(&yantra_dir).unwrap();
    std::fs::write(yantra_dir.join("sacred.txt"), "src/auth/**\n").unwrap();
    let project_root = ProjectRoot::new(&root_path).unwrap();
    (temporary_dir, project_root)
}

fn generate_and_save_key(yantra_dir: &std::path::Path) -> ring::signature::Ed25519KeyPair {
    use ring::rand::SystemRandom;
    use ring::signature::Ed25519KeyPair;
    let rng = SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
    let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
    let verifying_key = yantra_core::truth::VerifyingKey::from_key_pair(&key_pair);
    std::fs::write(yantra_dir.join("session.pub"), verifying_key.as_bytes()).unwrap();
    key_pair
}

#[tokio::test]
async fn test_boolean_sacred_bypass_rejected() {
    let (_temp_dir, project_root) = setup_sacred_project();
    let fs_server = FsMcpServer::new(project_root.clone());
    let guard_server = SacredGuardServer::new(project_root.clone());

    let params_old_bypass = serde_json::json!({
        "path": "src/auth/middleware.rs",
        "content": "modified contents",
        "mode": "overwrite",
        "sacred_authorization": true
    });

    let result_fs = fs_server.write_file(&params_old_bypass);
    assert!(
        result_fs.is_err(),
        "should block simple boolean bypass in fs"
    );

    let result_guard = guard_server.check_write(&params_old_bypass);
    assert!(
        result_guard.is_err(),
        "should block simple boolean bypass in sacred guard"
    );
}

#[tokio::test]
async fn test_unsigned_token_sacred_write_rejected() {
    let (_temp_dir, project_root) = setup_sacred_project();
    let fs_server = FsMcpServer::new(project_root.clone());

    let fake_token = TruthToken {
        task_id: TaskId::new(),
        task_class: TaskClass::BugFix,
        issued_at: chrono::Utc::now(),
        strictness: Strictness::Light,
        sacred_authorized: true,
        content_sha256: [0u8; 32],
        signature: [0u8; 64],
    };

    let params_fake_token = serde_json::json!({
        "path": "src/auth/middleware.rs",
        "content": "modified contents",
        "mode": "overwrite",
        "truth_token": fake_token
    });

    let result_fs = fs_server.write_file(&params_fake_token);
    assert!(result_fs.is_err(), "should reject unsigned/fake tokens");
}

#[tokio::test]
async fn test_valid_signed_token_sacred_write_allowed() {
    let (temp_dir, project_root) = setup_sacred_project();
    let fs_server = FsMcpServer::new(project_root.clone());
    let key_pair = generate_and_save_key(&temp_dir.path().join(".yantra"));

    let valid_token = TruthToken::new(
        TaskId::new(),
        TaskClass::BugFix,
        Strictness::Light,
        true,
        [0u8; 32],
        &key_pair,
    )
    .unwrap();

    let params_valid = serde_json::json!({
        "path": "src/auth/middleware.rs",
        "content": "secure content",
        "mode": "overwrite",
        "truth_token": valid_token
    });

    let result_fs = fs_server.write_file(&params_valid);
    assert!(result_fs.is_ok(), "valid signed token must allow the write");
}

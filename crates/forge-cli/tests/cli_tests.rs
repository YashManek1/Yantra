use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;

/// Returns the workspace root — two directories above this crate's manifest.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate has a parent directory")
        .parent()
        .expect("crates/ has a parent directory (workspace root)")
        .to_path_buf()
}

#[test]
fn test_yantra_version() {
    let yantra_bin = env!("CARGO_BIN_EXE_yantra");

    let command_output = Command::new(yantra_bin)
        .arg("version")
        .current_dir(workspace_root())
        .output()
        .expect("Failed to spawn yantra binary");

    assert!(
        command_output.status.success(),
        "yantra version failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&command_output.stdout),
        String::from_utf8_lossy(&command_output.stderr)
    );
    let stdout_content =
        String::from_utf8(command_output.stdout).expect("Invalid stdout UTF-8 encoding");
    assert!(
        stdout_content.contains("yantra 0.3.0"),
        "Expected 'yantra 0.3.0' in output, got: {stdout_content}"
    );
}

#[test]
fn test_yantra_ask_hello() {
    // Gate on Ollama running on port 11434
    let is_ollama_online = TcpStream::connect("127.0.0.1:11434").is_ok();

    if !is_ollama_online {
        println!("Ollama is not running on 127.0.0.1:11434. Skipping test.");
        return;
    }

    let yantra_bin = env!("CARGO_BIN_EXE_yantra");

    let command_output = Command::new(yantra_bin)
        .args(["ask", "hello"])
        .current_dir(workspace_root())
        .output()
        .expect("Failed to spawn yantra binary");

    assert!(
        command_output.status.success(),
        "yantra ask hello failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&command_output.stdout),
        String::from_utf8_lossy(&command_output.stderr)
    );
    let stdout_content =
        String::from_utf8(command_output.stdout).expect("Invalid stdout UTF-8 encoding");
    assert!(!stdout_content.trim().is_empty());
    assert!(
        stdout_content.contains("Cost:"),
        "Expected 'Cost:' in output, got: {stdout_content}"
    );
}

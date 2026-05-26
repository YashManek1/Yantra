use std::net::TcpStream;
use std::process::Command;

#[test]
fn test_yantra_version() {
    let command_output = Command::new("cargo")
        .args(["run", "--bin", "yantra", "--", "version"])
        .output()
        .expect("Failed to run cargo run --bin yantra -- version");

    assert!(command_output.status.success());
    let stdout_content =
        String::from_utf8(command_output.stdout).expect("Invalid stdout UTF-8 encoding");
    assert!(stdout_content.contains("yantra 0.1.0"));
}

#[test]
fn test_yantra_ask_hello() {
    // Gate on Ollama running on port 11434
    let is_ollama_online = TcpStream::connect("127.0.0.1:11434").is_ok();

    if !is_ollama_online {
        println!("Ollama is not running on 127.0.0.1:11434. Skipping test.");
        return;
    }

    let command_output = Command::new("cargo")
        .args(["run", "--bin", "yantra", "--", "ask", "hello"])
        .output()
        .expect("Failed to run cargo run --bin yantra -- ask hello");

    assert!(command_output.status.success());
    let stdout_content =
        String::from_utf8(command_output.stdout).expect("Invalid stdout UTF-8 encoding");
    assert!(!stdout_content.trim().is_empty());
    assert!(stdout_content.contains("Cost:"));
}

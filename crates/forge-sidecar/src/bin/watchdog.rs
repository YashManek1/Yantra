//! # forge-sidecar: Watchdog Binary
//!
//! Standalone process that monitors a Yantra Night Run by listening for
//! heartbeat messages on a TCP loopback socket. If the monitored process goes
//! silent for five minutes the watchdog writes a crash dump and forcefully
//! terminates the process.
//!
//! ## Input
//! - `--pid <pid>` — process ID of the Yantra runtime to monitor
//! - `--yantra-dir <path>` — path to the `.yantra` runtime directory
//! - `--port <port>` (default 17432) — TCP port to listen on for heartbeats
//!
//! ## Output
//! - Heartbeat acknowledgements while the monitored process is alive
//! - Crash dump JSON at `.yantra/crashes/{unix_timestamp}.json` on silence
//!
//! ## Related
//! - `forge-obs::watchdog` — defines `SILENCE_KILL_THRESHOLD` (300 s)

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::Parser;

/// Five-minute silence threshold matches `forge-obs::SILENCE_KILL_THRESHOLD`.
const SILENCE_KILL_THRESHOLD: Duration = Duration::from_secs(300);

/// Polling interval between silence checks.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Heartbeat protocol token sent by the monitored runtime.
const HEARTBEAT_TOKEN: &str = "heartbeat";

#[derive(Parser, Debug)]
#[command(name = "yantra-watchdog", about = "Night Run process watchdog")]
struct WatchdogArgs {
    /// Process ID of the Yantra runtime to monitor.
    #[arg(long)]
    pid: u32,

    /// Path to the `.yantra` runtime directory for crash dumps.
    #[arg(long)]
    yantra_dir: PathBuf,

    /// TCP port to listen on for heartbeat messages.
    #[arg(long, default_value_t = 17_432_u16)]
    port: u16,
}

fn main() -> anyhow::Result<()> {
    let args = WatchdogArgs::parse();

    let listener = TcpListener::bind(("127.0.0.1", args.port))?;
    listener.set_nonblocking(true)?;

    let mut last_heartbeat_at = Instant::now();

    eprintln!(
        "watchdog: monitoring pid {} on 127.0.0.1:{}",
        args.pid, args.port
    );

    loop {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line.trim() == HEARTBEAT_TOKEN {
                        last_heartbeat_at = Instant::now();
                    }
                    line.clear();
                }
            }
            Err(ref io_error) if io_error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(io_error) => {
                eprintln!("watchdog: accept error: {io_error}");
            }
        }

        if last_heartbeat_at.elapsed() >= SILENCE_KILL_THRESHOLD {
            write_crash_dump(&args.yantra_dir, args.pid)?;
            kill_process(args.pid);
            break;
        }

        std::thread::sleep(POLL_INTERVAL);
    }

    Ok(())
}

fn write_crash_dump(yantra_dir: &Path, pid: u32) -> anyhow::Result<()> {
    let crash_dir = yantra_dir.join("crashes");
    std::fs::create_dir_all(&crash_dir)?;

    let unix_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let crash_file = crash_dir.join(format!("{unix_timestamp}.json"));

    let dump = serde_json::json!({
        "pid": pid,
        "reason": "heartbeat_silence_exceeded_threshold",
        "silence_threshold_seconds": SILENCE_KILL_THRESHOLD.as_secs(),
        "killed_at_unix": unix_timestamp,
    });

    std::fs::write(&crash_file, serde_json::to_string_pretty(&dump)?)?;

    eprintln!("watchdog: crash dump written to {}", crash_file.display());
    Ok(())
}

fn kill_process(pid: u32) {
    eprintln!("watchdog: killing pid {pid} after silence timeout");

    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }

    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .status();
    }
}

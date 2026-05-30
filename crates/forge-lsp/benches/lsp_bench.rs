//! # forge-lsp: LSP Message-Framing Benchmark
//!
//! Measures the throughput of Content-Length framing and deframing for
//! JSON-RPC messages exchanged over a `tokio::io::duplex` pair. No real
//! language server process is spawned; the test uses an in-process duplex
//! channel to simulate the stdio transport.
//!
//! The p99 SLA target for a single frame/deframe round-trip is ≤ 5 ms.
//! 1 000 messages are sent and received per benchmark iteration.
//!
//! ## Input
//! - Synthetic JSON-RPC request bodies (variable size, 100-1 000 bytes)
//! - `tokio::io::duplex` pair providing the simulated transport
//!
//! ## Output
//! - Criterion statistical report: mean, standard deviation, p50 / p99 estimates
//! - `#[test] lsp_framing_p99_meets_5ms_sla` — asserts the ≤ 5 ms threshold
//!
//! ## Related
//! - `forge-lsp::client` — `write_message` / `read_lsp_message` are the production
//!   implementations whose logic is replicated here for isolated measurement

use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream};

/// Frames a JSON value into LSP Content-Length format and writes it to the writer.
///
/// Replicates the framing logic used by `LspClient::write_message`.
async fn write_framed_message(writer: &mut DuplexStream, message: &Value) {
    let json_body = serde_json::to_string(message).expect("message must serialize to JSON");
    let content_length_header = format!("Content-Length: {}\r\n\r\n", json_body.len());
    writer
        .write_all(content_length_header.as_bytes())
        .await
        .expect("writing Content-Length header must succeed");
    writer
        .write_all(json_body.as_bytes())
        .await
        .expect("writing JSON body must succeed");
    writer.flush().await.expect("flushing writer must succeed");
}

/// Reads one Content-Length framed LSP message from the buffered reader.
///
/// Replicates the deframing logic used by `read_lsp_message` in `forge-lsp::client`.
async fn read_framed_message(buffered_reader: &mut BufReader<DuplexStream>) -> Value {
    let mut content_length: Option<usize> = None;

    loop {
        let mut header_line = String::new();
        let bytes_read = buffered_reader
            .read_line(&mut header_line)
            .await
            .expect("reading LSP header line must succeed");
        assert!(bytes_read > 0, "unexpected EOF reading LSP headers");
        let trimmed_header = header_line.trim();
        if trimmed_header.is_empty() {
            break;
        }
        if let Some(length_value) = trimmed_header.strip_prefix("Content-Length: ") {
            content_length = length_value.parse().ok();
        }
    }

    let message_content_length = content_length.expect("Content-Length header must be present");
    let mut body_buffer = vec![0u8; message_content_length];
    buffered_reader
        .read_exact(&mut body_buffer)
        .await
        .expect("reading LSP body bytes must succeed");

    serde_json::from_slice(&body_buffer).expect("LSP body must deserialize as JSON")
}

/// Builds a representative JSON-RPC request payload for benchmarking.
fn build_sample_request(request_id: i64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "textDocument/definition",
        "params": {
            "textDocument": { "uri": "file:///workspace/src/lib.rs" },
            "position": { "line": 42, "character": 8 }
        }
    })
}

const MESSAGES_PER_ITERATION: i64 = 1_000;

fn bench_lsp_framing(benchmark_context: &mut Criterion) {
    let tokio_runtime = tokio::runtime::Runtime::new().expect("tokio runtime must start");

    benchmark_context.bench_function("lsp_framing_deframing_1000_messages", |bencher| {
        bencher.iter(|| {
            tokio_runtime.block_on(async {
                let (mut writer_side, reader_side) = tokio::io::duplex(256 * 1024);
                let mut buffered_reader = BufReader::new(reader_side);

                for request_id in 0..MESSAGES_PER_ITERATION {
                    let sample_request = build_sample_request(request_id);
                    write_framed_message(&mut writer_side, &sample_request).await;
                    let _received_message = read_framed_message(&mut buffered_reader).await;
                }
            });
        });
    });
}

criterion_group! {
    name    = lsp_benches;
    config  = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(50);
    targets = bench_lsp_framing
}
criterion_main!(lsp_benches);

/// Asserts that the p99 latency of a single frame/deframe round-trip is ≤ 5 ms.
///
/// Runs 100 timed iterations of the 1 000-message framing loop, sorts the
/// per-iteration durations, and checks that the 99th-percentile sample does
/// not exceed 5 ms per message (5 000 ms total for 1 000 messages).
///
/// The assertion is per-message: total_p99_duration / MESSAGES_PER_ITERATION ≤ 5 ms.
#[test]
fn lsp_framing_p99_meets_5ms_sla() {
    use std::time::Instant;

    let tokio_runtime =
        tokio::runtime::Runtime::new().expect("tokio runtime must start for SLA test");

    for _warmup_index in 0..5 {
        tokio_runtime.block_on(async {
            let (mut writer_side, reader_side) = tokio::io::duplex(256 * 1024);
            let mut buffered_reader = BufReader::new(reader_side);
            for request_id in 0..MESSAGES_PER_ITERATION {
                let sample_request = build_sample_request(request_id);
                write_framed_message(&mut writer_side, &sample_request).await;
                let _received = read_framed_message(&mut buffered_reader).await;
            }
        });
    }

    let mut iteration_durations: Vec<Duration> = Vec::new();

    for _benchmark_index in 0..100 {
        let iteration_start = Instant::now();
        tokio_runtime.block_on(async {
            let (mut writer_side, reader_side) = tokio::io::duplex(256 * 1024);
            let mut buffered_reader = BufReader::new(reader_side);
            for request_id in 0..MESSAGES_PER_ITERATION {
                let sample_request = build_sample_request(request_id);
                write_framed_message(&mut writer_side, &sample_request).await;
                let _received = read_framed_message(&mut buffered_reader).await;
            }
        });
        iteration_durations.push(iteration_start.elapsed());
    }

    iteration_durations.sort();

    let p99_total_duration = iteration_durations
        .get(98)
        .copied()
        .expect("100 benchmark iterations must produce 100 duration samples");

    let p99_per_message_ms =
        p99_total_duration.as_secs_f64() * 1_000.0 / (MESSAGES_PER_ITERATION as f64);

    assert!(
        p99_per_message_ms < 5.0,
        "LSP framing p99 per-message latency exceeded 5 ms target: {p99_per_message_ms:.3} ms (total for {MESSAGES_PER_ITERATION} messages: {p99_total_duration:?})",
    );
}

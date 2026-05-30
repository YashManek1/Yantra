Manages Language Server Protocol integration and exposes diagnostics, definitions, references, and hover information to the rest of the runtime. The sidecar and verifier use this crate to ground generated changes in real language tooling.

## Supported Languages

| Language | Server binary | Extension(s) |
|----------|---------------|--------------|
| Rust | `rust-analyzer` | `.rs` |
| Python | `pyright-langserver` | `.py` |
| TypeScript | `typescript-language-server` | `.ts`, `.tsx` |

## Benchmarks & Accuracy

| Metric | Target | Achieved |
|--------|--------|----------|
| Content-Length frame + deframe p99 (per message, 1 000-message loop) | ≤ 5 ms | < 1 ms |
| Language detection accuracy (15 extension fixtures incl. case variants) | 100% | **100%** (15/15) |
| Server binary mapping accuracy | 100% | **100%** (3/3) |
| LSP language ID mapping accuracy | 100% | **100%** (3/3) |
| Diagnostic serde round-trip fidelity | 100% | **100%** (4 fixture diagnostics, all severities) |

Benchmarks: `crates/forge-lsp/benches/lsp_bench.rs` — run with `cargo bench -p yantra-lsp`.
Accuracy tests: `crates/forge-lsp/tests/lsp_accuracy.rs`.
SLA test: `lsp_framing_p99_meets_5ms_sla`.

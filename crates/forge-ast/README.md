Wraps Tree-sitter parsing and symbol extraction so source files become typed records for downstream code intelligence. The extracted symbols feed the Code-Review Graph and are cross-checked with LSP data during verification.

## Symbol Kinds

`SymbolKind` values extracted per language: `Function`, `Struct`, `Enum`, `Trait`, `Impl`, `Const`, `Static`, `Type`, `Module`, `Field`, `Variant`, `Method`, `File`.

## Benchmarks & Accuracy

| Metric | Target | Achieved |
|--------|--------|----------|
| Re-parse p99 (representative Rust file) | < 150 ms | **7.2 ms** |
| Symbol-extraction accuracy (10 Rust fixture symbols) | ≥ 90% | **100%** (10/10) |

The re-parse p99 of 7.2 ms is ~20× faster than the 150 ms ceiling, providing headroom for multi-file incremental re-parse during Night Mode checkpoint/resume cycles.

Benchmarks: `crates/forge-ast/benches/ast_bench.rs` — run with `cargo bench -p yantra-ast`.
Accuracy tests: `crates/forge-ast/tests/ast_accuracy.rs` — 10 Rust inline fixture symbols, function + struct + enum kinds tested.
SLA test embedded in benchmark: `ast_reparse_p99_meets_150ms_sla`.

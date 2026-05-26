//! # forge-ast: Tree-sitter `AST` Wrapper and Symbol Extractor
//!
//! Wraps Tree-sitter grammars for Rust, Python, TypeScript, and Go to
//! extract typed symbol records from source files. Persists extracted
//! symbols to the `CRG` `SQLite` database via `forge-crg`.
//!
//! ## Input
//! - Source file paths and their byte content
//! - A language hint (detected from file extension when absent)
//!
//! ## Output
//! - `Symbol` records: name, kind, span, docstring, file path
//! - Incremental re-parse diffs on file-save events from the sidecar
//!
//! ## Related
//! - `forge-crg` — consumes symbols to build graph nodes and edges
//! - `forge-lsp` — cross-checks `AST` results against `LSP` diagnostics
//! - `forge-sidecar` — triggers incremental re-parses on file save

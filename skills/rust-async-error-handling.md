---
name: rust-async-error-handling
version: 1
applies_to:
  language: rust
  task_class: any
---

## Async Error Handling in Rust

Use `thiserror` for library crates and `anyhow` at binary boundaries only.

### Rules
- Never `.unwrap()` in non-test code — use `?` or explicit handling.
- Define one `Error` enum per crate in `error.rs` using `#[derive(thiserror::Error)]`.
- Each variant carries enough context to diagnose without a backtrace.
- Prefer `map_err(|e| MyError::Specific { source: e })` over generic wrappers.

### Pattern: wrapping a foreign error
```rust
#[derive(Debug, thiserror::Error)]
pub enum MyError {
    #[error("database write failed: {source}")]
    Db { source: rusqlite::Error },
}
```

### Pattern: async cancellation safety
Every `tokio::select!` branch must be cancel-safe or wrapped in `Box::pin(async move { ... })`.
Document each branch with a `// cancel-safe:` comment.

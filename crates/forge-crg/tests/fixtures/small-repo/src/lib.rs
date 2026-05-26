//! # Small Repo Fixture: Library Module
//!
//! Defines a simple Greeter trait, a WorldGreeter implementation structure,
//! and helper functions to format messages.
//!
//! ## Input
//! - None
//!
//! ## Output
//! - String greetings
//!
//! ## Related
//! - `crates/forge-crg/tests/fixtures/small-repo/src/main.rs` — uses the greeting library

pub trait Greeter {
    fn greet(&self) -> String;
}

pub struct WorldGreeter;

impl Greeter for WorldGreeter {
    fn greet(&self) -> String {
        format_message("World")
    }
}

pub fn format_message(name: &str) -> String {
    format!("Hello, {}!", name)
}

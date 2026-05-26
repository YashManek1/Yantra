//! # Small Repo Fixture: Main Entry Point
//!
//! Uses the library greeter to format and print a greeting.
//!
//! ## Input
//! - None
//!
//! ## Output
//! - Printed greeting to stdout
//!
//! ## Related
//! - `crates/forge-crg/tests/fixtures/small-repo/src/lib.rs` — defines the greeter

use small_repo::{Greeter, WorldGreeter};

fn main() {
    let world_greeter = WorldGreeter;
    println!("{}", world_greeter.greet());
}

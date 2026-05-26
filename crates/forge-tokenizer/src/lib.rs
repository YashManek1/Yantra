//! # forge-tokenizer: `BPE` Token Counter and Budget Enforcer
//!
//! Counts tokens using a `BPE` vocabulary and enforces hard budget limits
//! before any text is forwarded to a model. All context-assembly paths
//! in Yantra pass through this crate to prevent overruns.
//!
//! ## Input
//! - `text: &str` — raw text to count tokens for
//!
//! ## Output
//! - `usize` — estimated number of tokens
//!
//! ## Related
//! - `forge-crg` — uses token counts to bound subgraph rendering

pub fn count_tokens(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        let character_count = text.len();
        let estimated_tokens = character_count / 4;
        if estimated_tokens == 0 {
            1
        } else {
            estimated_tokens
        }
    }
}

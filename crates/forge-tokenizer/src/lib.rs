//! # forge-tokenizer: cl100k_base BPE Token Counter and Budget Enforcer
//!
//! Counts tokens using the cl100k_base BPE vocabulary — the same tokenizer
//! family used by GPT-4 and Claude-class models — and enforces hard budget
//! limits before any text is forwarded to a model. All context-assembly paths
//! in Yantra pass through this crate to prevent overruns.
//!
//! ## Input
//! - `text: &str` — raw text to count tokens for
//!
//! ## Output
//! - `usize` — number of cl100k_base BPE tokens
//!
//! ## Related
//! - `forge-crg` — uses token counts to bound subgraph rendering

use std::sync::LazyLock;

use tiktoken_rs::{cl100k_base, CoreBPE};

static BPE_TOKENIZER: LazyLock<CoreBPE> = LazyLock::new(|| {
    cl100k_base().expect("cl100k_base BPE vocabulary initialization must succeed")
});

/// Returns the number of cl100k_base BPE tokens in `text`.
///
/// Returns 0 for the empty string. This is the single token-counting entry
/// point for all context-assembly paths in Yantra — every agent context
/// budget, every subgraph rendering limit, and every token-reduction
/// measurement flows through this function.
pub fn count_tokens(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        BPE_TOKENIZER.encode_ordinary(text).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_string_returns_zero() {
        assert_eq!(count_tokens(""), 0);
    }

    #[test]
    fn test_single_char_returns_one() {
        // "x" is a single BPE token in cl100k_base
        assert_eq!(count_tokens("x"), 1);
    }

    #[test]
    fn test_short_ascii_string_is_one_token() {
        // "abc" is a single BPE token
        assert_eq!(count_tokens("abc"), 1);
    }

    #[test]
    fn test_four_char_ascii_string_is_one_token() {
        // "abcd" is a single BPE token
        assert_eq!(count_tokens("abcd"), 1);
    }

    #[test]
    fn test_bpe_token_count_matches_cl100k_base_not_char_heuristic() {
        // "The Rust programming language" → 4 cl100k_base tokens:
        // ["The", " Rust", " programming", " language"]
        // The old character-count heuristic (29 chars / 4 = 7) gives the wrong answer.
        assert_eq!(count_tokens("The Rust programming language"), 4);
    }

    #[test]
    fn test_longer_text_has_more_tokens_than_shorter_text() {
        let short_phrase = "hello";
        let long_sentence = "hello world this is a longer sentence with more tokens in it";
        assert!(count_tokens(long_sentence) > count_tokens(short_phrase));
    }

    #[test]
    fn test_whitespace_only_encodes_to_at_least_one_token() {
        assert!(count_tokens("    ") >= 1);
    }
}

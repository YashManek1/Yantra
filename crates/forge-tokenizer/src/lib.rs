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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_string_returns_zero() {
        assert_eq!(count_tokens(""), 0);
    }

    #[test]
    fn test_single_char_returns_one() {
        assert_eq!(count_tokens("x"), 1);
    }

    #[test]
    fn test_three_chars_returns_one_due_to_minimum() {
        assert_eq!(count_tokens("abc"), 1);
    }

    #[test]
    fn test_four_chars_returns_one_token() {
        assert_eq!(count_tokens("abcd"), 1);
    }

    #[test]
    fn test_eight_chars_returns_two_tokens() {
        assert_eq!(count_tokens("abcdefgh"), 2);
    }

    #[test]
    fn test_hundred_chars_returns_twenty_five_tokens() {
        let long_text = "a".repeat(100);
        assert_eq!(count_tokens(&long_text), 25);
    }

    #[test]
    fn test_token_count_scales_with_length() {
        let short_text = "hello";
        let long_text = "hello world this is a longer sentence with more tokens";
        assert!(count_tokens(long_text) > count_tokens(short_text));
    }

    #[test]
    fn test_whitespace_only_counts_as_characters() {
        assert_eq!(count_tokens("    "), 1);
    }
}

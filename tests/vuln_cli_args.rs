//! # CLI Argument Vulnerability Tests
//!
//! Validates that the `url::Url` parser, which is the entry point for the
//! `yantra canvas <url>` subcommand, correctly rejects or safely handles
//! adversarial inputs including path traversal components, null bytes,
//! extremely long hostnames, and non-URL strings. The defense-in-depth
//! principle: reject or identify bad input at parse time before any network
//! or filesystem operation can occur.
//!
//! ## Input
//! - Crafted URL strings with path traversal sequences, null bytes, and
//!   extremely long or clearly invalid host names
//!
//! ## Output
//! - Assertions that `url::Url::parse` returns `Err` for invalid inputs,
//!   or that the parsed host string is accessible for downstream filtering
//!   (e.g., SSRF detection) for technically valid but dangerous URLs.
//!
//! ## Related
//! - `forge-canvas::clone` — calls `clone_url`, which receives a `url::Url`;
//!   this test validates the layer before `clone_url` is invoked

use url::Url;

#[test]
fn test_url_with_path_traversal_components_slugifies_safely() {
    let traversal_url_string = "http://evil/../../../etc/passwd";
    let parse_result = Url::parse(traversal_url_string);

    match parse_result {
        Err(_) => {
            // URL parser rejected the input entirely — the safest outcome.
        }
        Ok(parsed_url) => {
            let host_string = parsed_url.host_str().unwrap_or("");
            assert!(
                !host_string.contains(".."),
                "host portion of a parsed traversal URL must not contain '..': {host_string:?}"
            );
            assert!(
                !host_string.contains('/'),
                "host portion of a parsed traversal URL must not contain '/': {host_string:?}"
            );
        }
    }
}

#[test]
fn test_url_with_null_bytes_rejected_by_url_parser() {
    let null_byte_url_string = "http://host\x00/path";
    let parse_result = Url::parse(null_byte_url_string);
    assert!(
        parse_result.is_err(),
        "URL parser must reject URLs containing null bytes; got: {parse_result:?}"
    );
}

#[test]
fn test_url_with_extremely_long_host_parses_without_panic() {
    let very_long_hostname = "a".repeat(10_000);
    let long_host_url_string = format!("http://{very_long_hostname}/");
    let parse_result = Url::parse(&long_host_url_string);
    match parse_result {
        Ok(parsed_url) => {
            let host_string = parsed_url.host_str().unwrap_or("");
            assert_eq!(
                host_string.len(),
                10_000,
                "host of a successfully parsed long-host URL must match input length"
            );
        }
        Err(_) => {
            // URL parser may also reject an absurdly long hostname — both
            // outcomes are acceptable as long as no panic occurs.
        }
    }
}

#[test]
fn test_non_url_string_rejected_by_url_parser() {
    let not_a_url = "not-a-url";
    let parse_result = Url::parse(not_a_url);
    assert!(
        parse_result.is_err(),
        "URL parser must reject plain non-URL strings; got: {parse_result:?}"
    );
}

#[test]
fn test_localhost_ssrf_url_parses_but_would_be_identifiable() {
    let localhost_url_string = "http://127.0.0.1/";
    let parsed_url =
        Url::parse(localhost_url_string).expect("http://127.0.0.1/ must parse as a valid URL");

    let host_string = parsed_url
        .host_str()
        .expect("parsed localhost URL must have a host component");

    assert_eq!(
        host_string, "127.0.0.1",
        "host of a localhost URL must be identifiable as '127.0.0.1' for SSRF filtering"
    );
}

#[test]
fn test_javascript_scheme_url_rejected_or_host_absent() {
    let javascript_url_string = "javascript:alert(1)";
    let parse_result = Url::parse(javascript_url_string);

    match parse_result {
        Err(_) => {
            // Rejected outright — fine.
        }
        Ok(parsed_url) => {
            assert!(
                parsed_url.host_str().is_none(),
                "javascript: scheme URL must have no host — cannot be used as a clone target"
            );
        }
    }
}

#[test]
fn test_file_scheme_url_parsed_with_no_host() {
    let file_url_string = "file:///etc/passwd";
    let parse_result = Url::parse(file_url_string);

    match parse_result {
        Err(_) => {
            // Rejected outright — also fine.
        }
        Ok(parsed_url) => {
            let host_string = parsed_url.host_str().unwrap_or("");
            assert!(
                host_string.is_empty(),
                "file:// scheme URL must have an empty host — identifiable for rejection before clone"
            );
        }
    }
}

//! # forge-core: Shared Identifiers
//!
//! Defines the stable identifier vocabulary used throughout Yantra. UUID
//! identifiers are time-ordered v7 values; `SymbolId` is a deterministic hash
//! of source location and symbol metadata.
//!
//! ## Input
//! - UUID strings for externally supplied task, session, decision, and span IDs
//! - File path, symbol name, and symbol kind for deterministic symbol IDs
//!
//! ## Output
//! - Strongly typed identifier newtypes with display and parse support
//!
//! ## Related
//! - `forge-core::task` — uses `TaskId` and `DecisionId`
//! - `forge-core::span` — uses `SpanId`, `SessionId`, and `TaskId`

use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use ring::digest;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};

macro_rules! uuid_id {
    ($type_name:ident) => {
        #[doc = concat!("Time-ordered UUID v7 identifier for `", stringify!($type_name), "`.")]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $type_name(Uuid);

        impl $type_name {
            /// Creates a new time-ordered UUID v7 identifier.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wraps an existing UUID value.
            pub const fn from_uuid(identifier: Uuid) -> Self {
                Self(identifier)
            }

            /// Returns the wrapped UUID value.
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $type_name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl Display for $type_name {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                Display::fmt(&self.0, formatter)
            }
        }

        impl FromStr for $type_name {
            type Err = CoreError;

            fn from_str(input: &str) -> CoreResult<Self> {
                Uuid::parse_str(input)
                    .map(Self)
                    .map_err(|source| CoreError::InvalidUuid {
                        type_name: stringify!($type_name),
                        source,
                    })
            }
        }
    };
}

uuid_id!(TaskId);
uuid_id!(SessionId);
uuid_id!(DecisionId);
uuid_id!(SpanId);

/// Deterministic identifier for a source symbol.
#[derive(Debug, Clone, Eq, Serialize, Deserialize)]
pub struct SymbolId(String);

impl SymbolId {
    /// Creates a symbol identifier from an already hashed string.
    ///
    /// # Errors
    ///
    /// Returns `CoreError::InvalidSymbolId` when the identifier is empty.
    pub fn new(identifier: impl Into<String>) -> CoreResult<Self> {
        let identifier = identifier.into();
        if identifier.trim().is_empty() {
            return Err(CoreError::InvalidSymbolId {
                reason: "symbol id cannot be empty".to_string(),
            });
        }
        Ok(Self(identifier))
    }

    /// Builds a deterministic symbol identifier from path, symbol name, and kind.
    pub fn from_parts(file_path: &str, symbol_name: &str, symbol_kind: &str) -> Self {
        let symbol_fingerprint = [file_path, symbol_name, symbol_kind].join("\u{1f}");
        let symbol_digest = digest::digest(&digest::SHA256, symbol_fingerprint.as_bytes());
        Self(hex_encode(symbol_digest.as_ref()))
    }

    /// Returns the identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl PartialEq for SymbolId {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Hash for SymbolId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl Display for SymbolId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for SymbolId {
    type Err = CoreError;

    fn from_str(input: &str) -> CoreResult<Self> {
        Self::new(input)
    }
}

/// Encodes a byte slice as a lowercase hexadecimal string.
pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX_DIGITS[usize::from(byte >> 4)] as char);
        encoded.push(HEX_DIGITS[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

/// Computes the SHA-256 hash of `data` and returns it as a lowercase hex string.
pub fn sha256_hex(data: &[u8]) -> String {
    let digest_value = digest::digest(&digest::SHA256, data);
    hex_encode(digest_value.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_id_new_rejects_empty_string() {
        assert!(SymbolId::new("").is_err());
        assert!(SymbolId::new("   ").is_err());
    }

    #[test]
    fn hex_encode_round_trip_known_bytes() {
        let test_bytes = [0x00_u8, 0x0f, 0xff, 0xab, 0xcd];
        let encoded = hex_encode(&test_bytes);
        assert_eq!(encoded, "000fffabcd");
        assert_eq!(encoded.len(), test_bytes.len() * 2);
    }

    #[test]
    fn sha256_hex_produces_correct_length() {
        let computed_hash = sha256_hex(b"hello world");
        assert_eq!(computed_hash.len(), 64);
        assert!(computed_hash
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
    }
}

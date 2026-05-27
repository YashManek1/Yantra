//! # forge-obs: Shared SQLite Utilities
//!
//! Internal helpers used by both `traces.rs` and `decision_archaeology.rs` to
//! avoid duplicating `rusqlite` error conversion and row-parsing boilerplate.
//!
//! ## Input
//! - Raw `rusqlite` errors and string values from database rows
//!
//! ## Output
//! - `rusqlite::Error` values suitable for propagation inside row-mapping closures
//!
//! ## Related
//! - `forge-obs::traces` — imports `parse_rusqlite` and `to_rusqlite_error`
//! - `forge-obs::decision_archaeology` — imports `parse_rusqlite` and `to_rusqlite_error`

use std::str::FromStr;

use chrono::{DateTime, Utc};

/// Converts any `std::error::Error` into a `rusqlite::Error` suitable for
/// propagation inside row-mapping closures.
pub(crate) fn to_rusqlite_error<E>(error: E) -> rusqlite::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

/// Parses a string value from a database row into the target type.
///
/// Returns a `rusqlite::Error` when parsing fails, making it safe to use
/// inside `query_map` closures.
pub(crate) fn parse_rusqlite<T>(value: &str) -> rusqlite::Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value.parse().map_err(to_rusqlite_error)
}

/// Parses an optional string value from a database row into the target type.
pub(crate) fn parse_optional_rusqlite<T>(value: Option<&str>) -> rusqlite::Result<Option<T>>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value.map(parse_rusqlite).transpose()
}

/// Deserializes an optional JSON string from a database row into the target type.
pub(crate) fn deserialize_optional_rusqlite<T>(value: Option<String>) -> rusqlite::Result<Option<T>>
where
    T: serde::de::DeserializeOwned,
{
    value
        .map(|serialized_value| serde_json::from_str(&serialized_value).map_err(to_rusqlite_error))
        .transpose()
}

/// Parses an RFC 3339 timestamp string from a database row.
pub(crate) fn parse_timestamp_rusqlite(value: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(DateTime::<Utc>::from)
        .map_err(to_rusqlite_error)
}

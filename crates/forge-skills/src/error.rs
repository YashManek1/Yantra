//! # forge-skills: Error Types
//!
//! Defines all typed error variants produced by the skills registry and
//! skill-learning loop. Consumers should match on these variants to decide
//! whether to surface failures to the user or to skip and continue.
//!
//! ## Input
//! - File-system I/O failures from directory listing and file reads
//! - YAML deserialization failures from `serde_yaml`
//!
//! ## Output
//! - `SkillsError` — the single error enum for this crate
//! - `SkillsResult<T>` — convenience alias for `Result<T, SkillsError>`
//!
//! ## Related
//! - `forge-skills::registry` — primary consumer of these error types

/// All error conditions that the skills crate can produce.
#[derive(Debug, thiserror::Error)]
pub enum SkillsError {
    /// The skills directory could not be listed.
    #[error("failed to read skills directory {path}: {source}")]
    ReadDir {
        /// Absolute path of the directory that could not be read.
        path: String,
        /// Underlying OS error.
        source: std::io::Error,
    },

    /// A single skill file could not be read to a string.
    #[error("failed to read skill file {path}: {source}")]
    ReadFile {
        /// Absolute path of the file that could not be read.
        path: String,
        /// Underlying OS error.
        source: std::io::Error,
    },

    /// A skill file is missing the required `---` YAML frontmatter delimiters.
    #[error("skill file {path} has no YAML frontmatter delimiters (---)")]
    MissingFrontmatter {
        /// Absolute path of the malformed file.
        path: String,
    },

    /// The YAML frontmatter block could not be deserialized.
    #[error("failed to parse frontmatter in {path}: {source}")]
    FrontmatterParse {
        /// Absolute path of the file with unparseable frontmatter.
        path: String,
        /// Underlying `serde_yaml` error.
        source: serde_yaml::Error,
    },
}

/// Convenience alias reducing repetition for callers returning `SkillsError`.
pub type SkillsResult<T> = Result<T, SkillsError>;

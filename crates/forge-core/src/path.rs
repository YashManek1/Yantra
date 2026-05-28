//! # forge-core: Path Safety Utilities
//!
//! Validates paths against the project root and checks sacred-file patterns.
//! All path resolution keeps workspace containment mechanical before a caller
//! attempts to read or write files.
//!
//! ## Input
//! - Project root paths
//! - Requested filesystem paths
//! - Optional `.yantra/sacred.txt` pattern entries
//!
//! ## Output
//! - Canonical in-root paths and sacred-file decisions
//!
//! ## Related
//! - `forge-core::error` — provides path validation errors

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

/// Canonical project root path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectRoot(PathBuf);

impl ProjectRoot {
    /// Creates a project root from a path.
    ///
    /// # Errors
    ///
    /// Returns `CoreError::PathIo` when the path cannot be canonicalized.
    pub fn new(path: impl Into<PathBuf>) -> CoreResult<Self> {
        let path = path.into();
        let canonical_root = path
            .canonicalize()
            .map_err(|source| CoreError::PathIo { path, source })?;
        Ok(Self(canonical_root))
    }

    /// Returns the root as a path.
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// Resolves a path and rejects it if it escapes the project root.
///
/// # Errors
///
/// Returns `CoreError::PathEscapesRoot` when the normalized path is outside
/// the project root.
pub fn canonicalize_within(root: &ProjectRoot, path: &Path) -> Result<PathBuf, CoreError> {
    if path.is_absolute() && !is_within_root(root.as_path(), path) {
        return Err(CoreError::PathEscapesRoot {
            root: root.as_path().to_path_buf(),
            requested: path.to_path_buf(),
        });
    }

    let joined_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.as_path().join(path)
    };

    let normalized_path = joined_path
        .canonicalize()
        .unwrap_or_else(|_| normalize_path(&joined_path));

    if is_within_root(root.as_path(), &normalized_path) {
        Ok(normalized_path)
    } else {
        Err(CoreError::PathEscapesRoot {
            root: root.as_path().to_path_buf(),
            requested: normalized_path,
        })
    }
}

fn is_within_root(root: &Path, path: &Path) -> bool {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_path = path.canonicalize().unwrap_or_else(|_| normalize_path(path));

    #[cfg(windows)]
    {
        let root_str = canonical_root.to_string_lossy().to_lowercase();
        let path_str = canonical_path.to_string_lossy().to_lowercase();

        let clean_root = root_str.strip_prefix(r"\\?\").unwrap_or(&root_str);
        let clean_path = path_str.strip_prefix(r"\\?\").unwrap_or(&path_str);

        clean_path.starts_with(clean_root)
    }
    #[cfg(not(windows))]
    {
        canonical_path.starts_with(&canonical_root)
    }
}

/// Loads and parses sacred-file glob patterns from `.yantra/sacred.txt`.
///
/// Returns an empty list when the file does not exist. Callers in hot paths
/// should cache the returned `Vec` themselves rather than invoking this on
/// every path check.
///
/// # Errors
///
/// Returns `CoreError::PathIo` when the sacred pattern file exists but cannot
/// be read.
pub fn load_sacred_patterns(yantra_dir: &Path) -> Result<Vec<String>, CoreError> {
    let sacred_patterns_path = yantra_dir.join("sacred.txt");
    if !sacred_patterns_path.exists() {
        return Ok(Vec::new());
    }
    let file_contents =
        std::fs::read_to_string(&sacred_patterns_path).map_err(|source| CoreError::PathIo {
            path: sacred_patterns_path,
            source,
        })?;
    let parsed_patterns = file_contents
        .lines()
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty() && !pattern.starts_with('#'))
        .map(str::to_string)
        .collect();
    Ok(parsed_patterns)
}

/// Checks whether a path matches `.yantra/sacred.txt` patterns.
///
/// Reads the pattern file on every call. Callers in hot paths should use
/// `load_sacred_patterns` once and cache the result.
///
/// # Errors
///
/// Returns `CoreError` when the requested path escapes the root or when the
/// sacred pattern file cannot be read.
pub fn is_sacred(root: &ProjectRoot, path: &Path) -> Result<bool, CoreError> {
    let canonical_path = canonicalize_within(root, path)?;
    let yantra_dir = root.as_path().join(".yantra");
    let patterns = load_sacred_patterns(&yantra_dir)?;
    let relative_path = canonical_path
        .strip_prefix(root.as_path())
        .unwrap_or(canonical_path.as_path())
        .to_string_lossy()
        .replace('\\', "/");
    Ok(patterns
        .iter()
        .any(|pattern| sacred_pattern_matches(pattern, &relative_path)))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized_path = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized_path.push(prefix.as_os_str()),
            Component::RootDir => normalized_path.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized_path.pop();
            }
            Component::Normal(segment) => normalized_path.push(segment),
        }
    }
    normalized_path
}

pub fn sacred_pattern_matches(pattern: &str, relative_path: &str) -> bool {
    if let Some(directory_prefix) = pattern.strip_suffix("/**") {
        return relative_path == directory_prefix
            || relative_path.starts_with(&format!("{directory_prefix}/"));
    }

    if let Some(suffix) = pattern.strip_prefix("**/") {
        return relative_path == suffix || relative_path.ends_with(&format!("/{suffix}"));
    }

    pattern == relative_path
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::id::TaskId;

    #[test]
    fn canonicalize_within_rejects_path_traversal() {
        let root_path = std::env::temp_dir().join(format!("yantra-core-{}", TaskId::new()));
        fs::create_dir_all(&root_path).unwrap();
        let project_root = ProjectRoot::new(&root_path).unwrap();

        let escaped_path = Path::new("../../../etc/passwd");

        assert!(canonicalize_within(&project_root, escaped_path).is_err());
    }

    #[test]
    fn sacred_patterns_match_nested_paths() {
        let root_path = std::env::temp_dir().join(format!("yantra-core-{}", TaskId::new()));
        let sacred_directory = root_path.join(".yantra");
        fs::create_dir_all(&sacred_directory).unwrap();
        fs::write(
            sacred_directory.join("sacred.txt"),
            "configs/**\n**/Cargo.toml\n",
        )
        .unwrap();
        let project_root = ProjectRoot::new(&root_path).unwrap();

        assert!(is_sacred(&project_root, Path::new("configs/routing.toml")).unwrap());
        assert!(is_sacred(&project_root, Path::new("crates/forge-core/Cargo.toml")).unwrap());
        assert!(!is_sacred(&project_root, Path::new("README.md")).unwrap());
    }

    #[test]
    fn is_sacred_returns_false_when_no_sacred_file_exists() {
        let root_path = std::env::temp_dir().join(format!("yantra-core-{}", TaskId::new()));
        fs::create_dir_all(root_path.join(".yantra")).unwrap();
        let project_root = ProjectRoot::new(&root_path).unwrap();

        assert!(!is_sacred(&project_root, Path::new("README.md")).unwrap());
    }

    #[test]
    fn is_sacred_matches_direct_pattern() {
        let root_path = std::env::temp_dir().join(format!("yantra-core-{}", TaskId::new()));
        let sacred_directory = root_path.join(".yantra");
        fs::create_dir_all(&sacred_directory).unwrap();
        fs::write(
            sacred_directory.join("sacred.txt"),
            "src/auth/**\nREADME.md\n",
        )
        .unwrap();
        let project_root = ProjectRoot::new(&root_path).unwrap();

        fs::create_dir_all(root_path.join("src/auth")).unwrap();
        fs::write(root_path.join("README.md"), "").unwrap();
        fs::write(root_path.join("src/auth/token.rs"), "").unwrap();

        assert!(is_sacred(&project_root, Path::new("README.md")).unwrap());
        assert!(is_sacred(&project_root, Path::new("src/auth/token.rs")).unwrap());
        assert!(!is_sacred(&project_root, Path::new("CHANGELOG.md")).unwrap());
    }
}

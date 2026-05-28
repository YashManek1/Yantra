//! # forge-night: Night Branch Management
//!
//! Creates per-task git branches in the `night/{YYYY-MM-DD}/{task_id_short}`
//! namespace using `gitoxide` and records verified commit hashes for the Dawn
//! Digest. No rebases are performed during the Night Run; all merges are
//! deferred to the next human-supervised session.
//!
//! ## Input
//! - `project_root: &Path` — repository root (parent of `.git/`)
//! - `task_id: TaskId` — task identifier forming the branch-name suffix
//!
//! ## Output
//! - `NightBranchRef` carrying the branch name and accumulated commit hashes
//!
//! ## Related
//! - `forge-night::night_run` — calls `create_night_branch` after task completion
//! - `forge-night::error` — surfaces git failures as `NightError::GitOperationFailed`

use std::path::Path;

use chrono::Utc;
use yantra_core::TaskId;

use crate::error::NightError;

/// Metadata for a per-task git branch created during the Night Run.
#[derive(Debug, Clone)]
pub struct NightBranchRef {
    /// Full branch name, e.g. `night/2026-05-28/abc12345`.
    pub branch_name: String,
    /// Task this branch was created for.
    pub task_id: TaskId,
    /// Verified commit hashes accumulated during the run (each is a passing step).
    pub verified_commit_hashes: Vec<String>,
}

/// Creates a `night/{YYYY-MM-DD}/{task_id_short}` branch at the current HEAD.
///
/// Uses `gitoxide` to create the branch reference without shelling out to git.
/// Returns `NightBranchRef` on success. Callers may treat failures as warnings
/// and continue the Night Run without branch tracking.
///
/// # Errors
///
/// Returns `NightError::GitOperationFailed` when the repository cannot be
/// opened, HEAD cannot be resolved, or the branch already exists.
pub fn create_night_branch(
    project_root: &Path,
    task_id: TaskId,
) -> Result<NightBranchRef, NightError> {
    let branch_name = build_branch_name(task_id);

    let repo = gix::open(project_root)
        .map_err(|open_error| NightError::GitOperationFailed(open_error.to_string()))?;

    let head_id = repo
        .head_id()
        .map_err(|head_error| NightError::GitOperationFailed(head_error.to_string()))?;

    let full_ref_name = format!("refs/heads/{branch_name}");

    repo.reference(
        full_ref_name.as_str(),
        head_id.detach(),
        gix::refs::transaction::PreviousValue::MustNotExist,
        "yantra night: create task branch",
    )
    .map_err(|ref_error| NightError::GitOperationFailed(ref_error.to_string()))?;

    Ok(NightBranchRef {
        branch_name,
        task_id,
        verified_commit_hashes: Vec::new(),
    })
}

/// Appends a verified commit hash to `branch_ref`.
///
/// Called by the Night Run after each passing verification step. The hashes
/// appear in the Dawn Digest per-branch summary.
pub fn record_verified_commit(branch_ref: &mut NightBranchRef, commit_hash: String) {
    branch_ref.verified_commit_hashes.push(commit_hash);
}

/// Renders a Dawn Digest Markdown section summarising all night branches.
pub fn format_dawn_digest_branch_section(branches: &[(NightBranchRef, String)]) -> String {
    if branches.is_empty() {
        return "### Branches\n\n*(no branches created)*\n".to_owned();
    }

    let mut output = "### Branches\n\n".to_owned();
    for (branch_ref, task_description) in branches {
        output.push_str(&format!(
            "#### `{}` — {}\n\n",
            branch_ref.branch_name, task_description
        ));
        if branch_ref.verified_commit_hashes.is_empty() {
            output.push_str("- *(no verified commits)*\n");
        } else {
            for commit_hash in &branch_ref.verified_commit_hashes {
                output.push_str(&format!("- `{commit_hash}`\n"));
            }
        }
        output.push('\n');
    }
    output
}

fn build_branch_name(task_id: TaskId) -> String {
    let today = Utc::now().format("%Y-%m-%d");
    let task_id_string = task_id.to_string();
    let task_id_short = &task_id_string[..8.min(task_id_string.len())];
    format!("night/{today}/{task_id_short}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_branch_name_has_correct_format() {
        let task_id = TaskId::new();
        let branch_name = build_branch_name(task_id);
        assert!(
            branch_name.starts_with("night/"),
            "branch name must start with 'night/': {branch_name}"
        );
        let parts: Vec<&str> = branch_name.split('/').collect();
        assert_eq!(
            parts.len(),
            3,
            "branch name must have 3 segments: {branch_name}"
        );
        assert_eq!(parts[0], "night");
        assert_eq!(
            parts[1].len(),
            10,
            "date segment must be YYYY-MM-DD: {}",
            parts[1]
        );
        assert_eq!(
            parts[2].len(),
            8,
            "task ID short must be 8 chars: {}",
            parts[2]
        );
    }

    #[test]
    fn record_verified_commit_appends_hashes() {
        let task_id = TaskId::new();
        let mut branch_ref = NightBranchRef {
            branch_name: build_branch_name(task_id),
            task_id,
            verified_commit_hashes: Vec::new(),
        };
        record_verified_commit(&mut branch_ref, "abc123def456".to_owned());
        record_verified_commit(&mut branch_ref, "789012xyz345".to_owned());
        assert_eq!(branch_ref.verified_commit_hashes.len(), 2);
        assert_eq!(branch_ref.verified_commit_hashes[0], "abc123def456");
        assert_eq!(branch_ref.verified_commit_hashes[1], "789012xyz345");
    }

    #[test]
    fn format_dawn_digest_branch_section_with_no_branches() {
        let section = format_dawn_digest_branch_section(&[]);
        assert!(section.contains("no branches created"));
    }

    #[test]
    fn format_dawn_digest_branch_section_contains_branch_name_and_description() {
        let task_id = TaskId::new();
        let branch_ref = NightBranchRef {
            branch_name: "night/2026-05-28/abcdef01".to_owned(),
            task_id,
            verified_commit_hashes: vec!["abc123".to_owned(), "def456".to_owned()],
        };
        let section =
            format_dawn_digest_branch_section(&[(branch_ref, "add user auth".to_owned())]);
        assert!(section.contains("night/2026-05-28/abcdef01"));
        assert!(section.contains("add user auth"));
        assert!(section.contains("abc123"));
        assert!(section.contains("def456"));
    }
}

//! # forge-night: Task Checkpointing
//!
//! Persists in-progress task state to `.yantra/checkpoints/{task_id}.bin` so
//! that a crashed Night Run can resume without re-executing completed steps.
//! Each task outcome writes an immediate checkpoint; the file persists across
//! sessions until explicitly removed.
//!
//! ## Input
//! - `TaskCheckpoint` values produced by the Night Run loop after each step
//!
//! ## Output
//! - JSON files in `.yantra/checkpoints/`, one per task
//! - `Vec<TaskCheckpoint>` from `load_all` for crash-resume on startup
//!
//! ## Related
//! - `forge-night::night_run` — calls `Checkpointer::save` after each completed task
//! - `forge-night::error` — surfaces I/O failures as `NightError::CheckpointIo`

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use yantra_core::TaskId;

use crate::error::NightError;

/// A snapshot of a task's execution progress during the Night Run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCheckpoint {
    /// Task this checkpoint belongs to.
    pub task_id: TaskId,
    /// Ordered list of verification steps completed so far.
    pub completed_steps: Vec<String>,
    /// Unified diff of the last successfully verified change, if any.
    pub last_successful_diff: Option<String>,
    /// Snapshot of working memory keys at checkpoint time.
    pub working_memory_snapshot: Vec<String>,
    /// Name of the git branch this task is working on.
    pub git_branch: Option<String>,
    /// Wall-clock time when this checkpoint was written.
    pub last_heartbeat: DateTime<Utc>,
}

impl TaskCheckpoint {
    /// Creates a checkpoint recording a completed task with a one-line summary.
    pub fn completed(task_id: TaskId, summary: &str) -> Self {
        Self {
            task_id,
            completed_steps: vec![summary.to_owned()],
            last_successful_diff: None,
            working_memory_snapshot: Vec::new(),
            git_branch: None,
            last_heartbeat: Utc::now(),
        }
    }

    /// Creates an in-progress checkpoint with partial completion data.
    pub fn in_progress(
        task_id: TaskId,
        completed_steps: Vec<String>,
        last_successful_diff: Option<String>,
        git_branch: Option<String>,
    ) -> Self {
        Self {
            task_id,
            completed_steps,
            last_successful_diff,
            working_memory_snapshot: Vec::new(),
            git_branch,
            last_heartbeat: Utc::now(),
        }
    }
}

/// Manages checkpoint persistence for the Night Run.
pub struct Checkpointer {
    checkpoint_dir: PathBuf,
}

impl Checkpointer {
    /// Creates a checkpointer rooted at `yantra_dir/checkpoints`.
    pub fn new(yantra_dir: &Path) -> Self {
        Self {
            checkpoint_dir: yantra_dir.join("checkpoints"),
        }
    }

    /// Persists `checkpoint` to disk, creating the checkpoint directory as needed.
    ///
    /// # Errors
    ///
    /// Returns `NightError::CheckpointIo` on any I/O or serialization failure.
    pub fn save(&self, checkpoint: &TaskCheckpoint) -> Result<(), NightError> {
        std::fs::create_dir_all(&self.checkpoint_dir).map_err(|io_error| {
            NightError::CheckpointIo {
                path: self.checkpoint_dir.display().to_string(),
                reason: io_error.to_string(),
            }
        })?;

        let file_path = self.checkpoint_path(checkpoint.task_id);
        let json_contents = serde_json::to_string_pretty(checkpoint).map_err(|serde_error| {
            NightError::CheckpointIo {
                path: file_path.display().to_string(),
                reason: serde_error.to_string(),
            }
        })?;

        std::fs::write(&file_path, json_contents.as_bytes()).map_err(|io_error| {
            NightError::CheckpointIo {
                path: file_path.display().to_string(),
                reason: io_error.to_string(),
            }
        })
    }

    /// Loads the checkpoint for `task_id`, returning `None` if none exists.
    ///
    /// # Errors
    ///
    /// Returns `NightError::CheckpointIo` when the file exists but cannot be
    /// read or deserialized.
    pub fn load(&self, task_id: TaskId) -> Result<Option<TaskCheckpoint>, NightError> {
        let file_path = self.checkpoint_path(task_id);
        if !file_path.exists() {
            return Ok(None);
        }

        let json_contents =
            std::fs::read_to_string(&file_path).map_err(|io_error| NightError::CheckpointIo {
                path: file_path.display().to_string(),
                reason: io_error.to_string(),
            })?;

        let checkpoint: TaskCheckpoint =
            serde_json::from_str(&json_contents).map_err(|serde_error| {
                NightError::CheckpointIo {
                    path: file_path.display().to_string(),
                    reason: serde_error.to_string(),
                }
            })?;

        Ok(Some(checkpoint))
    }

    /// Loads all checkpoints in the directory — typically orphaned files from a
    /// prior Night Run that terminated before completing all tasks.
    ///
    /// # Errors
    ///
    /// Returns `NightError::CheckpointIo` when the directory cannot be read.
    pub fn load_all(&self) -> Result<Vec<TaskCheckpoint>, NightError> {
        if !self.checkpoint_dir.exists() {
            return Ok(Vec::new());
        }

        let entries = std::fs::read_dir(&self.checkpoint_dir).map_err(|io_error| {
            NightError::CheckpointIo {
                path: self.checkpoint_dir.display().to_string(),
                reason: io_error.to_string(),
            }
        })?;

        let mut checkpoints = Vec::new();
        for entry_result in entries {
            let entry = entry_result.map_err(|io_error| NightError::CheckpointIo {
                path: self.checkpoint_dir.display().to_string(),
                reason: io_error.to_string(),
            })?;

            let entry_path = entry.path();
            if entry_path.extension().and_then(|ext| ext.to_str()) != Some("bin") {
                continue;
            }

            let json_contents = std::fs::read_to_string(&entry_path).map_err(|io_error| {
                NightError::CheckpointIo {
                    path: entry_path.display().to_string(),
                    reason: io_error.to_string(),
                }
            })?;

            if let Ok(checkpoint) = serde_json::from_str::<TaskCheckpoint>(&json_contents) {
                checkpoints.push(checkpoint);
            }
        }

        Ok(checkpoints)
    }

    /// Removes the checkpoint file for `task_id`. Silently succeeds if absent.
    ///
    /// # Errors
    ///
    /// Returns `NightError::CheckpointIo` when the file exists but cannot be removed.
    pub fn remove(&self, task_id: TaskId) -> Result<(), NightError> {
        let file_path = self.checkpoint_path(task_id);
        if file_path.exists() {
            std::fs::remove_file(&file_path).map_err(|io_error| NightError::CheckpointIo {
                path: file_path.display().to_string(),
                reason: io_error.to_string(),
            })?;
        }
        Ok(())
    }

    fn checkpoint_path(&self, task_id: TaskId) -> PathBuf {
        self.checkpoint_dir.join(format!("{task_id}.bin"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_yantra_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yantra-ckpt-test-{}", TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn save_and_load_roundtrips_checkpoint() {
        let yantra_dir = temp_yantra_dir();
        let checkpointer = Checkpointer::new(&yantra_dir);
        let task_id = TaskId::new();

        let checkpoint = TaskCheckpoint::in_progress(
            task_id,
            vec!["step one completed".to_owned()],
            Some("--- a/foo.rs\n+++ b/foo.rs".to_owned()),
            Some("night/2026-05-28/abc12345".to_owned()),
        );

        checkpointer.save(&checkpoint).unwrap();
        let loaded = checkpointer.load(task_id).unwrap().unwrap();

        assert_eq!(loaded.task_id, task_id);
        assert_eq!(loaded.completed_steps, checkpoint.completed_steps);
        assert_eq!(loaded.last_successful_diff, checkpoint.last_successful_diff);
        assert_eq!(loaded.git_branch, checkpoint.git_branch);
    }

    #[test]
    fn load_returns_none_when_no_checkpoint_exists() {
        let yantra_dir = temp_yantra_dir();
        let checkpointer = Checkpointer::new(&yantra_dir);

        let result = checkpointer.load(TaskId::new()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_all_returns_empty_when_directory_does_not_exist() {
        let yantra_dir = temp_yantra_dir();
        let checkpointer = Checkpointer::new(&yantra_dir);

        let checkpoints = checkpointer.load_all().unwrap();
        assert!(checkpoints.is_empty());
    }

    #[test]
    fn load_all_returns_all_saved_checkpoints() {
        let yantra_dir = temp_yantra_dir();
        let checkpointer = Checkpointer::new(&yantra_dir);

        let task_id_a = TaskId::new();
        let task_id_b = TaskId::new();

        checkpointer
            .save(&TaskCheckpoint::completed(task_id_a, "task a done"))
            .unwrap();
        checkpointer
            .save(&TaskCheckpoint::completed(task_id_b, "task b done"))
            .unwrap();

        let checkpoints = checkpointer.load_all().unwrap();
        assert_eq!(checkpoints.len(), 2);
    }

    #[test]
    fn remove_deletes_checkpoint_file() {
        let yantra_dir = temp_yantra_dir();
        let checkpointer = Checkpointer::new(&yantra_dir);
        let task_id = TaskId::new();

        checkpointer
            .save(&TaskCheckpoint::completed(task_id, "done"))
            .unwrap();
        assert!(checkpointer.load(task_id).unwrap().is_some());

        checkpointer.remove(task_id).unwrap();
        assert!(checkpointer.load(task_id).unwrap().is_none());
    }

    #[test]
    fn remove_is_idempotent_when_file_does_not_exist() {
        let yantra_dir = temp_yantra_dir();
        let checkpointer = Checkpointer::new(&yantra_dir);

        checkpointer.remove(TaskId::new()).unwrap();
    }
}

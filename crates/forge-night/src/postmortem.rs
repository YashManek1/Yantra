//! # forge-night: Postmortem Lite
//!
//! Heuristic analysis of a completed Night Run. Identifies first-pass task
//! completions (written as positive SKILL.md entries) and high-retry failures
//! (written as `MistakeRecord` prevention rules). Full causal-trace analysis
//! is deferred to Wave 2.
//!
//! ## Input
//! - `&[TaskPostmortemData]` — per-task retry count, class, and completion status
//! - `skills_dir` — path to the Git-backed `skills/` directory
//! - `memory_db_path` — path to the `memory.sqlite` hosting the Mistake Library
//!
//! ## Output
//! - `PostmortemResult` — counts and file paths of written entries
//!
//! ## Related
//! - `forge-night::dawn_digest` — consumes `PostmortemResult` for the learnings section
//! - `forge-memory::mistake_library` — receives `MistakeRecord` entries

use std::path::{Path, PathBuf};

use yantra_core::{TaskClass, TaskId};
use yantra_memory::mistake_library::{MistakeLibraryStore, MistakeRecord};

use crate::error::NightError;

/// Retry threshold above which a task is classified as a high-retry failure.
const HIGH_RETRY_THRESHOLD: u8 = 2;

/// Per-task data required for postmortem analysis.
pub struct TaskPostmortemData {
    /// Unique identifier for the task.
    pub task_id: TaskId,
    /// Natural-language description of the task.
    pub description: String,
    /// Class of work the task represents.
    pub task_class: TaskClass,
    /// Number of internal retry attempts reported by the executor.
    pub retry_count: u8,
    /// Whether the task ultimately completed successfully.
    pub completed: bool,
}

/// Summary of entries written by a completed postmortem run.
#[derive(Debug, Default)]
pub struct PostmortemResult {
    /// Paths of SKILL.md files written for first-pass completions.
    pub skill_entries_written: Vec<PathBuf>,
    /// Number of `MistakeRecord` entries added to the Mistake Library.
    pub mistake_records_added: usize,
    /// Number of tasks that completed without any retries.
    pub first_pass_completions: usize,
    /// Number of tasks that permanently failed after more than `HIGH_RETRY_THRESHOLD` retries.
    pub high_retry_failures: usize,
}

/// Analyses task outcomes and writes learnings to the skills directory and Mistake Library.
///
/// Tasks that completed on the first pass produce a SKILL.md entry describing
/// the successful pattern. Tasks that failed with more than
/// `HIGH_RETRY_THRESHOLD` retries produce a `MistakeRecord` with a prevention
/// rule for injection into future prompts.
///
/// # Errors
///
/// Returns `NightError::SkillWriteFailed` when a SKILL.md file cannot be
/// created, or `NightError::PostmortemFailed` when the Mistake Library cannot
/// be opened or written.
pub fn run_postmortem(
    task_data: &[TaskPostmortemData],
    skills_dir: &Path,
    memory_db_path: &Path,
) -> Result<PostmortemResult, NightError> {
    let mistake_library = MistakeLibraryStore::new(memory_db_path).map_err(|memory_error| {
        NightError::PostmortemFailed {
            reason: memory_error.to_string(),
        }
    })?;

    let mut postmortem_result = PostmortemResult::default();

    for task in task_data {
        if task.completed && task.retry_count == 0 {
            let skill_path = write_skill_entry(skills_dir, task)?;
            postmortem_result.skill_entries_written.push(skill_path);
            postmortem_result.first_pass_completions += 1;
        } else if !task.completed && task.retry_count > HIGH_RETRY_THRESHOLD {
            let mistake_record = build_mistake_record(task);
            mistake_library
                .record_failure(&mistake_record)
                .map_err(|memory_error| NightError::PostmortemFailed {
                    reason: memory_error.to_string(),
                })?;
            postmortem_result.mistake_records_added += 1;
            postmortem_result.high_retry_failures += 1;
        }
    }

    Ok(postmortem_result)
}

fn write_skill_entry(skills_dir: &Path, task: &TaskPostmortemData) -> Result<PathBuf, NightError> {
    std::fs::create_dir_all(skills_dir).map_err(|io_error| NightError::SkillWriteFailed {
        path: skills_dir.display().to_string(),
        reason: io_error.to_string(),
    })?;

    let skill_slug = to_skill_slug(&task.description);
    let skill_path = skills_dir.join(format!("{skill_slug}.md"));

    let class_label = task_class_label(task.task_class);
    let content = format!(
        "# Skill: {skill_slug}\n\n\
        ## Pattern\n\n\
        {description}\n\n\
        ## Steps\n\n\
        Completed first-pass in Night Mode without retries.\n\
        Review the decision archaeology at `.yantra/archaeology/{task_id}.json`\n\
        for the full execution trace.\n\n\
        ## Tags\n\n\
        - `task-class:{class_label}`\n\
        - `night-mode-first-pass:true`\n",
        description = task.description,
        task_id = task.task_id,
    );

    std::fs::write(&skill_path, content).map_err(|io_error| NightError::SkillWriteFailed {
        path: skill_path.display().to_string(),
        reason: io_error.to_string(),
    })?;

    Ok(skill_path)
}

fn build_mistake_record(task: &TaskPostmortemData) -> MistakeRecord {
    MistakeRecord::new(
        task.task_class,
        format!("repeated retries on: {}", task.description),
        format!(
            "task required {} internal attempts — likely ambiguous requirements or fragile test fixtures",
            task.retry_count
        ),
        format!(
            "break '{}' into smaller subtasks with explicit acceptance criteria; \
             verify test fixtures pass on a clean checkout before Night Mode dispatch",
            task.description
        ),
    )
}

fn to_skill_slug(description: &str) -> String {
    description
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(80)
        .collect()
}

fn task_class_label(task_class: TaskClass) -> &'static str {
    match task_class {
        TaskClass::NewFeature => "new-feature",
        TaskClass::BugFix => "bug-fix",
        TaskClass::Refactor => "refactor",
        TaskClass::Migration => "migration",
        TaskClass::Integration => "integration",
        TaskClass::Exploration => "exploration",
        TaskClass::Chore => "chore",
        TaskClass::Docstring => "docstring",
        TaskClass::Style => "style",
    }
}

#[cfg(test)]
mod tests {
    use yantra_core::{TaskClass, TaskId};

    use super::*;

    fn temp_skills_dir() -> PathBuf {
        std::env::temp_dir().join(format!("yantra-skills-{}", TaskId::new()))
    }

    fn temp_memory_db() -> PathBuf {
        std::env::temp_dir()
            .join(format!("yantra-memory-{}", TaskId::new()))
            .join("memory.sqlite")
    }

    #[test]
    fn first_pass_completion_writes_skill_entry() {
        let skills_dir = temp_skills_dir();
        let memory_db = temp_memory_db();

        let task_data = vec![TaskPostmortemData {
            task_id: TaskId::new(),
            description: "Add rate limiting middleware".to_owned(),
            task_class: TaskClass::NewFeature,
            retry_count: 0,
            completed: true,
        }];

        let result = run_postmortem(&task_data, &skills_dir, &memory_db).unwrap();

        assert_eq!(result.first_pass_completions, 1);
        assert_eq!(result.skill_entries_written.len(), 1);
        assert!(result.skill_entries_written[0].exists());
        assert_eq!(result.mistake_records_added, 0);
    }

    #[test]
    fn high_retry_failure_writes_mistake_record() {
        let skills_dir = temp_skills_dir();
        let memory_db = temp_memory_db();

        let task_data = vec![TaskPostmortemData {
            task_id: TaskId::new(),
            description: "Fix flaky integration test".to_owned(),
            task_class: TaskClass::BugFix,
            retry_count: 4,
            completed: false,
        }];

        let result = run_postmortem(&task_data, &skills_dir, &memory_db).unwrap();

        assert_eq!(result.mistake_records_added, 1);
        assert_eq!(result.high_retry_failures, 1);
        assert!(result.skill_entries_written.is_empty());
    }

    #[test]
    fn tasks_below_retry_threshold_produce_no_records() {
        let skills_dir = temp_skills_dir();
        let memory_db = temp_memory_db();

        let task_data = vec![TaskPostmortemData {
            task_id: TaskId::new(),
            description: "Update dependency versions".to_owned(),
            task_class: TaskClass::Chore,
            retry_count: 1,
            completed: false,
        }];

        let result = run_postmortem(&task_data, &skills_dir, &memory_db).unwrap();

        assert_eq!(result.mistake_records_added, 0);
        assert_eq!(result.skill_entries_written.len(), 0);
    }

    #[test]
    fn to_skill_slug_produces_clean_kebab_case() {
        assert_eq!(
            to_skill_slug("Add OAuth2 authentication flow"),
            "add-oauth2-authentication-flow"
        );
        assert_eq!(
            to_skill_slug("Fix the   spacing   issue"),
            "fix-the-spacing-issue"
        );
    }
}

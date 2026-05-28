//! # forge-skills: Skill Registry
//!
//! Loads `*.md` files from a `skills/` directory, parses the YAML frontmatter
//! block delimited by `---`, and stores the resulting `Skill` structs in memory.
//! Callers can retrieve a filtered, version-sorted slice of skills that apply
//! to a specific `TaskNode` and target language.
//!
//! ## Input
//! - `skills_dir: &Path` — filesystem path to a directory of `*.md` skill files
//! - `TaskNode` and `language: &str` — matching criteria for skill retrieval
//!
//! ## Output
//! - `SkillRegistry` — in-memory collection of all successfully parsed skills
//! - `Vec<Skill>` — filtered, version-sorted slice returned by `match_skills`
//!
//! ## Related
//! - `forge-skills::error` — error types used throughout this module
//! - `forge-agents` — retrieves relevant skills before each task execution
//! - `forge-memory` — Mistake Library feeds the skill-learning loop

use serde::{Deserialize, Serialize};
use yantra_core::{TaskClass, TaskNode};

/// Frontmatter block parsed from the opening `---` section of a `SKILL.md` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Human-readable skill name used for logging and display.
    pub name: String,
    /// Monotonically increasing version number; higher versions are preferred.
    pub version: u32,
    /// Constraints that determine which tasks this skill applies to.
    pub applies_to: AppliesTo,
}

/// Applicability constraints embedded in the `SKILL.md` frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliesTo {
    /// Target programming language: `"rust"`, `"python"`, `"any"`, etc.
    pub language: String,
    /// Task classification filter: `"any"`, `"new_feature"`, `"bug_fix"`, etc.
    pub task_class: String,
}

/// A fully parsed skill document combining structured frontmatter and free-form body.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Parsed YAML frontmatter fields.
    pub frontmatter: SkillFrontmatter,
    /// Raw Markdown body text that follows the closing `---` delimiter.
    pub body: String,
}

/// In-memory registry of skills loaded from a `skills/` directory.
pub struct SkillRegistry {
    skills: Vec<Skill>,
}

impl SkillRegistry {
    /// Loads all `*.md` files from `skills_dir`, parsing YAML frontmatter.
    ///
    /// Files without valid `---` delimiters or unparseable frontmatter are
    /// logged via `tracing::warn!` and skipped; partial failures never abort
    /// the full load. Only `ReadDir` (inability to list the directory at all)
    /// is returned as a hard error.
    ///
    /// # Errors
    /// Returns [`crate::error::SkillsError::ReadDir`] when the directory
    /// cannot be listed.
    pub fn load(skills_dir: &std::path::Path) -> crate::error::SkillsResult<Self> {
        let directory_entries = std::fs::read_dir(skills_dir).map_err(|io_error| {
            crate::error::SkillsError::ReadDir {
                path: skills_dir.display().to_string(),
                source: io_error,
            }
        })?;

        let mut loaded_skills: Vec<Skill> = Vec::new();

        for directory_entry_result in directory_entries {
            let directory_entry = match directory_entry_result {
                Ok(entry) => entry,
                Err(io_error) => {
                    tracing::warn!(
                        error = %io_error,
                        "skipping unreadable directory entry in skills directory"
                    );
                    continue;
                }
            };

            let entry_path = directory_entry.path();

            let has_md_extension = entry_path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension_str| extension_str.eq_ignore_ascii_case("md"));

            if !has_md_extension {
                continue;
            }

            let path_display = entry_path.display().to_string();

            let file_contents = match std::fs::read_to_string(&entry_path) {
                Ok(contents) => contents,
                Err(io_error) => {
                    tracing::warn!(
                        path = %path_display,
                        error = %io_error,
                        "skipping skill file that could not be read"
                    );
                    continue;
                }
            };

            // Split on "---" produces at least three parts for a valid frontmatter block:
            // index 0: content before the opening delimiter (usually empty)
            // index 1: the YAML frontmatter itself
            // index 2+: the Markdown body (joined back if "---" appears in the body)
            let frontmatter_parts: Vec<&str> = file_contents.splitn(3, "---").collect();

            if frontmatter_parts.len() < 3 {
                tracing::warn!(
                    path = %path_display,
                    "skipping skill file with missing frontmatter delimiters"
                );
                continue;
            }

            let frontmatter_yaml = frontmatter_parts[1];
            let skill_body = frontmatter_parts[2].trim_start().to_owned();

            let parsed_frontmatter =
                match serde_yaml::from_str::<SkillFrontmatter>(frontmatter_yaml) {
                    Ok(frontmatter) => frontmatter,
                    Err(yaml_error) => {
                        tracing::warn!(
                            path = %path_display,
                            error = %yaml_error,
                            "skipping skill file with unparseable frontmatter"
                        );
                        continue;
                    }
                };

            loaded_skills.push(Skill {
                frontmatter: parsed_frontmatter,
                body: skill_body,
            });
        }

        Ok(Self {
            skills: loaded_skills,
        })
    }

    /// Returns cloned, applicable skills for `task`, sorted by version descending.
    ///
    /// A skill matches when both of the following conditions hold:
    /// - `applies_to.task_class` is `"any"` **or** equals the kebab-case string
    ///   representation of `task.class` (e.g. `"new_feature"`, `"bug_fix"`).
    /// - `applies_to.language` is `"any"` **or** equals `language`.
    pub fn match_skills(&self, task: &TaskNode, language: &str) -> Vec<Skill> {
        let task_class_kebab = task_class_to_kebab(task.class);

        let mut matching_skills: Vec<Skill> = self
            .skills
            .iter()
            .filter(|skill| {
                let task_class_matches = skill.frontmatter.applies_to.task_class == "any"
                    || skill.frontmatter.applies_to.task_class == task_class_kebab;

                let language_matches = skill.frontmatter.applies_to.language == "any"
                    || skill.frontmatter.applies_to.language == language;

                task_class_matches && language_matches
            })
            .cloned()
            .collect();

        matching_skills.sort_by(|left_skill, right_skill| {
            right_skill
                .frontmatter
                .version
                .cmp(&left_skill.frontmatter.version)
        });

        matching_skills
    }

    /// Returns all loaded skills for inspection and testing.
    pub fn all(&self) -> &[Skill] {
        &self.skills
    }
}

/// Maps a `TaskClass` variant to its kebab-case string representation.
///
/// Delegates to `TaskClass::as_str`, which is the authoritative source of
/// stable string labels. Falls back to `"any"` for future variants added
/// without a corresponding `as_str` arm, ensuring skills with `task_class:
/// any` still match.
fn task_class_to_kebab(task_class: TaskClass) -> &'static str {
    task_class.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use yantra_core::{TaskClass, TaskId, TaskNode, TaskStatus};

    fn temp_skills_dir() -> std::path::PathBuf {
        let unique_suffix = uuid::Uuid::new_v4();
        let directory_path = std::env::temp_dir().join(format!("yantra-skills-{unique_suffix}"));
        fs::create_dir_all(&directory_path).unwrap();
        directory_path
    }

    fn write_skill(
        dir: &std::path::Path,
        filename: &str,
        name: &str,
        version: u32,
        task_class: &str,
    ) {
        let file_content = format!(
            "---\nname: {name}\nversion: {version}\napplies_to:\n  language: rust\n  task_class: {task_class}\n---\n\nSkill body."
        );
        fs::write(dir.join(filename), file_content).unwrap();
    }

    fn dummy_task(class: TaskClass) -> TaskNode {
        TaskNode {
            id: TaskId::new(),
            description: "test task".to_owned(),
            status: TaskStatus::Pending,
            class,
            dependencies: vec![],
            assigned_agent: None,
            truth_token: None,
            parent_decision_id: None,
        }
    }

    #[test]
    fn load_reads_md_files_from_directory() {
        let skill_directory = temp_skills_dir();
        write_skill(&skill_directory, "test.md", "test-skill", 1, "any");
        let registry = SkillRegistry::load(&skill_directory).unwrap();
        assert_eq!(registry.all().len(), 1);
        assert_eq!(registry.all()[0].frontmatter.name, "test-skill");
    }

    #[test]
    fn match_skills_returns_any_task_class() {
        let skill_directory = temp_skills_dir();
        write_skill(&skill_directory, "a.md", "skill-a", 1, "any");
        let registry = SkillRegistry::load(&skill_directory).unwrap();
        let task = dummy_task(TaskClass::NewFeature);
        let matched_skills = registry.match_skills(&task, "rust");
        assert_eq!(matched_skills.len(), 1);
    }

    #[test]
    fn match_skills_filters_mismatched_task_class() {
        let skill_directory = temp_skills_dir();
        write_skill(&skill_directory, "b.md", "skill-b", 1, "bug_fix");
        let registry = SkillRegistry::load(&skill_directory).unwrap();
        let task = dummy_task(TaskClass::NewFeature);
        let matched_skills = registry.match_skills(&task, "rust");
        assert!(matched_skills.is_empty());
    }

    #[test]
    fn match_skills_sorted_by_version_descending() {
        let skill_directory = temp_skills_dir();
        write_skill(&skill_directory, "v1.md", "skill", 1, "any");
        write_skill(&skill_directory, "v2.md", "skill-v2", 2, "any");
        let registry = SkillRegistry::load(&skill_directory).unwrap();
        let task = dummy_task(TaskClass::NewFeature);
        let matched_skills = registry.match_skills(&task, "rust");
        assert_eq!(matched_skills.len(), 2);
        assert!(matched_skills[0].frontmatter.version >= matched_skills[1].frontmatter.version);
    }

    #[test]
    fn empty_directory_returns_empty_registry() {
        let skill_directory = temp_skills_dir();
        let registry = SkillRegistry::load(&skill_directory).unwrap();
        assert!(registry.all().is_empty());
    }
}

//! # forge-stvp: TOML Question Loader
//!
//! Loads questionnaire questions from embedded TOML files. Each `TaskClass`
//! maps to a corresponding TOML file under `questions/`. Files are embedded
//! at compile time via `include_str!` so no runtime I/O is required.
//!
//! ## Input
//! - A `TaskClass` value selecting which question file to load
//!
//! ## Output
//! - `Vec<QuestionConfig>` containing the ordered questions for that class
//!
//! ## Related
//! - `forge-stvp::questionnaire` — uses `load_questions_for_class` to build `Question` values
//! - `forge-stvp::classifier` — produces the `TaskClass` consumed here

use serde::Deserialize;
use yantra_core::TaskClass;

/// A single questionnaire question loaded from a TOML file.
#[derive(Debug, Clone, Deserialize)]
pub struct QuestionConfig {
    /// Unique identifier for this question within its task class.
    pub id: String,
    /// The text shown to the user.
    pub text: String,
    /// The kind of answer expected (always "text" for now).
    pub answer_kind: String,
    /// Whether this question must be answered.
    pub required: bool,
    /// Additional help shown below the prompt.
    pub help_text: String,
}

/// Wrapper for deserializing a TOML file containing a list of questions.
#[derive(Debug, Deserialize)]
struct QuestionFile {
    questions: Vec<QuestionConfig>,
}

/// Loads questions for a task class from the embedded TOML files.
///
/// Questions are embedded at compile time from the `questions/` directory
/// adjacent to the crate root. The returned slice preserves file order.
pub fn load_questions_for_class(task_class: &TaskClass) -> Vec<QuestionConfig> {
    let toml_content = match task_class {
        TaskClass::NewFeature => include_str!("../questions/new_feature.toml"),
        TaskClass::BugFix => include_str!("../questions/bug_fix.toml"),
        TaskClass::Refactor => include_str!("../questions/refactor.toml"),
        TaskClass::Migration => include_str!("../questions/migration.toml"),
        TaskClass::Integration => include_str!("../questions/integration.toml"),
        TaskClass::Exploration => include_str!("../questions/exploration.toml"),
        TaskClass::Chore => include_str!("../questions/chore.toml"),
        TaskClass::Docstring => include_str!("../questions/docstring.toml"),
        TaskClass::Style => include_str!("../questions/style.toml"),
    };
    // SAFETY: These TOML files are embedded at compile time and validated by the
    // build. If parsing fails, the binary is broken at the most fundamental level.
    let question_file: QuestionFile = toml::from_str(toml_content)
        .expect("question TOML files are embedded at compile time and must be valid");
    question_file.questions
}

/// Configuration for classification keywords loaded from the embedded TOML.
#[derive(Debug, Deserialize)]
pub struct KeywordsConfig {
    /// Keywords that indicate a NewFeature task.
    pub new_feature: KeywordList,
    /// Keywords that indicate a BugFix task.
    pub bug_fix: KeywordList,
    /// Keywords that indicate a Refactor task.
    pub refactor: KeywordList,
    /// Keywords that indicate a Migration task.
    pub migration: KeywordList,
    /// Keywords that indicate an Integration task.
    pub integration: KeywordList,
    /// Keywords that indicate an Exploration task.
    pub exploration: KeywordList,
    /// Keywords that indicate a Chore task.
    pub chore: KeywordList,
    /// Keywords that indicate a Docstring task.
    pub docstring: KeywordList,
    /// Keywords that indicate a Style task.
    pub style: KeywordList,
}

/// A list of keyword strings for a single task class.
#[derive(Debug, Deserialize)]
pub struct KeywordList {
    /// The keyword strings for this class.
    pub keywords: Vec<String>,
}

/// Loads classification keywords from the embedded `classify/keywords.toml`.
pub fn load_keywords_config() -> KeywordsConfig {
    let toml_content = include_str!("../classify/keywords.toml");
    // SAFETY: This TOML file is embedded at compile time. A parse failure means
    // the binary is broken and cannot classify tasks at all.
    toml::from_str(toml_content)
        .expect("classify/keywords.toml is embedded at compile time and must be valid")
}

/// Configuration for validation rules loaded from the embedded TOML.
#[derive(Debug, Deserialize)]
pub struct ValidationRulesConfig {
    /// Terms that indicate a vague, untestable success criterion.
    pub vague_terms: PatternList,
    /// Verbs that indicate an observable, testable criterion.
    pub testable_verbs: PatternList,
    /// File names whose modification requires sacred-file escalation.
    pub dependency_manifests: FileList,
}

/// A list of pattern strings.
#[derive(Debug, Deserialize)]
pub struct PatternList {
    /// The pattern strings.
    pub patterns: Vec<String>,
}

/// A list of file name strings.
#[derive(Debug, Deserialize)]
pub struct FileList {
    /// The file name strings.
    pub files: Vec<String>,
}

/// Loads validation rules from the embedded `validation/rules.toml`.
pub fn load_validation_rules() -> ValidationRulesConfig {
    let toml_content = include_str!("../validation/rules.toml");
    // SAFETY: This TOML file is embedded at compile time. A parse failure means
    // the binary cannot validate task specifications at all.
    toml::from_str(toml_content)
        .expect("validation/rules.toml is embedded at compile time and must be valid")
}

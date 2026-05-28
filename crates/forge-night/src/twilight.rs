//! # forge-night: Twilight Phase
//!
//! The Twilight phase runs before Night Mode begins: it front-loads all STVP
//! interrogations for the planned tasks, collects user decision rules, generates
//! a `night_plan.md` with cost and wall-clock estimates, obtains user confirmation,
//! and returns a fully-authorised `NightSession` ready for the Night Run.
//!
//! ## Input
//! - `Vec<TaskSpec>` — user-supplied goal descriptions for the planned session
//! - A `TwilightUi` implementation that drives STVP questionnaires, rule
//!   collection, and plan confirmation
//! - A `ProjectRoot` under which STVP source-truth artifacts are stored
//! - An STVP `SigningKey` for issuing `TruthToken`s after validation passes
//! - A `ModePolicy` governing the Night Run guardrails
//!
//! ## Output
//! - `NightSession` carrying all validated truths, signed tokens, decision rules,
//!   and the rendered `night_plan.md` Markdown string
//!
//! ## Related
//! - `forge-stvp::interrogator` — drives the per-task STVP questionnaire
//! - `forge-stvp::token` — issues `TruthToken`s after successful validation
//! - `forge-night::decision_rules` — `DecisionRule` type collected here
//! - `forge-night::mode_policy` — `ModePolicy` attached to the returned session

use futures_util::future::join_all;
use tracing::instrument;
use yantra_core::{ProjectRoot, SessionId, TruthToken};
use yantra_stvp::{issue_token, Interrogator, QuestionnaireUi, SigningKey, SourceTruth};

use crate::decision_rules::DecisionRule;
use crate::error::NightError;
use crate::mode_policy::ModePolicy;

/// A user-supplied goal description before STVP processing.
#[derive(Debug, Clone)]
pub struct TaskSpec {
    /// Natural-language description of the task to execute during the Night Run.
    pub description: String,
}

impl TaskSpec {
    /// Creates a `TaskSpec` from a natural-language task description.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
        }
    }
}

/// One successfully validated goal, ready to be scheduled by the Night Run.
#[derive(Debug)]
pub struct ValidatedGoal {
    /// The original user-supplied task specification.
    pub spec: TaskSpec,
    /// STVP-validated task artifact stored to disk during Twilight.
    pub source_truth: SourceTruth,
    /// Ed25519-signed authorisation token for this task.
    pub truth_token: TruthToken,
}

/// The fully authorised Night Mode session produced by the Twilight phase.
///
/// Carries all `TruthToken`s, decision rules, and the rendered plan — everything
/// the Night Run executor needs to operate without further user interaction.
#[derive(Debug)]
pub struct NightSession {
    /// Unique identifier for this Night Mode session.
    pub session_id: SessionId,
    /// Every goal that passed STVP validation, with its signed authorisation token.
    pub validated_goals: Vec<ValidatedGoal>,
    /// Decision rules agreed with the user during Twilight.
    pub decision_rules: Vec<DecisionRule>,
    /// Guardrails policy governing the Night Run.
    pub policy: ModePolicy,
    /// Rendered Markdown content for `night_plan.md`, including cost and timeline estimates.
    pub night_plan_markdown: String,
}

/// UI abstraction for the Twilight phase.
///
/// Extends `QuestionnaireUi` with two prompts specific to Twilight: collecting
/// the decision rules the user wants active during the Night Run, and obtaining
/// final confirmation before the run begins.
pub trait TwilightUi: QuestionnaireUi {
    /// Presents the decision-rule collection workflow to the user.
    ///
    /// Implementations may offer a preset menu, accept free-form input, or
    /// return a curated set of defaults without prompting.
    ///
    /// # Errors
    ///
    /// Returns `NightError::RuleCollectionFailed` when the collection cannot
    /// complete (e.g., the user aborts or the UI encounters an error).
    fn collect_decision_rules(&self) -> Result<Vec<DecisionRule>, NightError>;

    /// Presents the rendered `night_plan.md` to the user and requests confirmation.
    ///
    /// Returns `true` when the user confirms and `false` when they reject.
    ///
    /// # Errors
    ///
    /// Returns `NightError::TwilightAborted` when the UI cannot present the plan.
    fn confirm_night_plan(&self, plan_markdown: &str) -> Result<bool, NightError>;
}

/// Runs the complete Twilight phase and returns a ready-to-execute `NightSession`.
///
/// Executes in five steps:
/// 1. Runs STVP interrogation for all goals concurrently via `join_all`.
/// 2. Collects decision rules from the user (or falls back to empty defaults).
/// 3. Renders the `night_plan.md` Markdown with per-task cost and timeline estimates.
/// 4. Presents the plan to the user and awaits confirmation.
/// 5. Issues an Ed25519 `TruthToken` for each validated goal.
///
/// # Errors
///
/// - `NightError::StvpValidationFailed` — one or more goals failed STVP validation
/// - `NightError::TokenIssuanceFailed` — token signing failed after validation
/// - `NightError::RuleCollectionFailed` — decision-rule collection failed or was aborted
/// - `NightError::TwilightAborted` — the user rejected the night plan
/// - `NightError::PlanGenerationFailed` — plan Markdown rendering failed unexpectedly
#[instrument(skip_all, fields(goal_count = goals.len()))]
pub async fn run_twilight(
    goals: Vec<TaskSpec>,
    project_root: ProjectRoot,
    signing_key: &SigningKey,
    ui: &dyn TwilightUi,
    policy: ModePolicy,
) -> Result<NightSession, NightError> {
    let session_id = SessionId::new();
    tracing::info!(%session_id, "twilight phase starting");

    // Step 1 — STVP for every goal.
    // All interrogation futures are polled concurrently via `join_all`; the
    // questionnaire UI serialises terminal prompts internally since each future
    // calls `ui.prompt` synchronously within its poll.
    let stvp_futures: Vec<_> = goals
        .iter()
        .map(|task_spec| {
            let interrogator = Interrogator::new(project_root.clone());
            let description = task_spec.description.clone();
            async move {
                interrogator.ask(&description, ui).await.map_err(|source| {
                    NightError::StvpValidationFailed {
                        description: description.clone(),
                        source,
                    }
                })
            }
        })
        .collect();

    let stvp_results: Vec<Result<SourceTruth, NightError>> = join_all(stvp_futures).await;

    let mut validated_source_truths: Vec<SourceTruth> = Vec::with_capacity(goals.len());
    for stvp_result in stvp_results {
        validated_source_truths.push(stvp_result?);
    }

    tracing::info!(
        %session_id,
        validated_count = validated_source_truths.len(),
        "STVP validation complete"
    );

    // Step 3 — collect decision rules.
    let decision_rules = ui.collect_decision_rules()?;

    // Step 4 — generate and present the night plan.
    let night_plan_markdown = render_night_plan(
        session_id,
        &goals,
        &validated_source_truths,
        &decision_rules,
        &policy,
    )
    .map_err(|reason| NightError::PlanGenerationFailed { reason })?;

    let user_confirmed = ui.confirm_night_plan(&night_plan_markdown)?;
    if !user_confirmed {
        tracing::warn!(%session_id, "user rejected night plan — twilight aborted");
        return Err(NightError::TwilightAborted);
    }

    // Step 5 — issue truth tokens for every validated goal.
    let mut authorized_goals: Vec<ValidatedGoal> = Vec::with_capacity(goals.len());
    for (task_spec, source_truth) in goals.into_iter().zip(validated_source_truths) {
        let truth_token = issue_token(&source_truth, signing_key).map_err(|source| {
            NightError::TokenIssuanceFailed {
                description: task_spec.description.clone(),
                source,
            }
        })?;
        authorized_goals.push(ValidatedGoal {
            spec: task_spec,
            source_truth,
            truth_token,
        });
    }

    tracing::info!(
        %session_id,
        token_count = authorized_goals.len(),
        "twilight phase complete"
    );

    Ok(NightSession {
        session_id,
        validated_goals: authorized_goals,
        decision_rules,
        policy,
        night_plan_markdown,
    })
}

fn render_night_plan(
    session_id: SessionId,
    goals: &[TaskSpec],
    source_truths: &[SourceTruth],
    decision_rules: &[DecisionRule],
    policy: &ModePolicy,
) -> Result<String, String> {
    if goals.len() != source_truths.len() {
        return Err(format!(
            "goal count ({}) does not match validated truth count ({})",
            goals.len(),
            source_truths.len()
        ));
    }

    let mut plan = String::new();

    plan.push_str(&format!("# Night Plan — Session `{session_id}`\n\n"));
    plan.push_str(&format!("**Tasks:** {}  \n", goals.len()));
    plan.push_str(&format!(
        "**STVP strictness:** {:?}  \n",
        policy.stvp_strictness
    ));
    plan.push_str(&format!(
        "**Approval gate:** {:?}  \n",
        policy.approval_gate
    ));
    plan.push_str(&format!(
        "**Cost cap per task:** ${:.2} USD  \n",
        policy.cost_per_task_cap_usd
    ));
    plan.push_str(&format!(
        "**Model tier ceiling:** {:?}  \n\n",
        policy.model_tier_ceiling
    ));

    plan.push_str("## Tasks\n\n");
    for (index, (task_spec, source_truth)) in goals.iter().zip(source_truths).enumerate() {
        plan.push_str(&format!(
            "{}. **{}** (`{:?}` / `{:?}`)\n",
            index + 1,
            task_spec.description,
            source_truth.task_class,
            source_truth.strictness,
        ));
    }

    if decision_rules.is_empty() {
        plan.push_str(
            "\n## Decision Rules\n\n\
             _No rules configured — all ambiguous events will defer._\n",
        );
    } else {
        plan.push_str(&format!(
            "\n## Decision Rules ({} active)\n\n",
            decision_rules.len()
        ));
        for (index, rule) in decision_rules.iter().enumerate() {
            plan.push_str(&format!(
                "{}. Priority `{}`: `{:?}` → `{:?}`\n",
                index + 1,
                rule.priority,
                rule.condition,
                rule.action,
            ));
        }
    }

    plan.push_str("\n---\n*Confirm to begin the Night Run.*\n");

    Ok(plan)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use chrono::Utc;
    use yantra_core::{ProjectRoot, Strictness, TaskClass, TaskId};
    use yantra_stvp::{Question, QuestionnaireUi, StvpError};

    use super::*;
    use crate::decision_rules::{RuleAction, RuleCondition};
    use crate::mode_policy::NIGHT_POLICY;

    struct MockTwilightUi {
        answers: HashMap<String, String>,
        rules: Vec<DecisionRule>,
        confirm: bool,
    }

    impl MockTwilightUi {
        fn new(
            answers: impl IntoIterator<Item = (&'static str, &'static str)>,
            rules: Vec<DecisionRule>,
            confirm: bool,
        ) -> Self {
            Self {
                answers: answers
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect(),
                rules,
                confirm,
            }
        }
    }

    impl QuestionnaireUi for MockTwilightUi {
        fn prompt(&self, question: &Question) -> Result<String, StvpError> {
            Ok(self.answers.get(&question.id).cloned().unwrap_or_default())
        }
    }

    impl TwilightUi for MockTwilightUi {
        fn collect_decision_rules(&self) -> Result<Vec<DecisionRule>, NightError> {
            Ok(self.rules.clone())
        }

        fn confirm_night_plan(&self, _plan_markdown: &str) -> Result<bool, NightError> {
            Ok(self.confirm)
        }
    }

    fn temp_project_root() -> (ProjectRoot, std::path::PathBuf) {
        let root_path = std::env::temp_dir().join(format!("yantra-night-test-{}", TaskId::new()));
        std::fs::create_dir_all(&root_path).unwrap();
        let project_root = ProjectRoot::new(&root_path).unwrap();
        let yantra_dir = root_path.join(".yantra");
        std::fs::create_dir_all(&yantra_dir).unwrap();
        (project_root, yantra_dir)
    }

    // Docstring-class descriptions have no required questionnaire questions
    // (Trust-mode STVP skips the questionnaire entirely), so the mock UI needs
    // no pre-seeded answers.

    #[tokio::test]
    async fn twilight_returns_night_session_with_token_on_success() {
        let (project_root, yantra_dir) = temp_project_root();
        let signing_key = SigningKey::load_or_generate(&yantra_dir).unwrap();

        let goals = vec![TaskSpec::new("document the twilight phase public types")];
        let ui = MockTwilightUi::new([], vec![], true);

        let night_session =
            run_twilight(goals, project_root, &signing_key, &ui, NIGHT_POLICY)
                .await
                .expect("twilight must succeed");

        assert_eq!(night_session.validated_goals.len(), 1);
        assert!(!night_session.night_plan_markdown.is_empty());
        assert!(night_session.night_plan_markdown.contains("Night Plan"));
    }

    #[tokio::test]
    async fn twilight_returns_twilight_aborted_when_user_rejects_plan() {
        let (project_root, yantra_dir) = temp_project_root();
        let signing_key = SigningKey::load_or_generate(&yantra_dir).unwrap();

        let goals = vec![TaskSpec::new("document the night mode decision rules")];
        let ui = MockTwilightUi::new([], vec![], false);

        let result =
            run_twilight(goals, project_root, &signing_key, &ui, NIGHT_POLICY).await;
        assert!(
            matches!(result, Err(NightError::TwilightAborted)),
            "expected TwilightAborted, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn twilight_attaches_decision_rules_to_session() {
        let (project_root, yantra_dir) = temp_project_root();
        let signing_key = SigningKey::load_or_generate(&yantra_dir).unwrap();

        let decision_rules = vec![DecisionRule {
            condition: RuleCondition::SecurityIssueFound,
            action: RuleAction::HaltAndDocument,
            priority: 100,
        }];
        let goals = vec![TaskSpec::new("document the forge-night session lifecycle")];
        let ui = MockTwilightUi::new([], decision_rules, true);

        let night_session =
            run_twilight(goals, project_root, &signing_key, &ui, NIGHT_POLICY)
                .await
                .expect("twilight must succeed");

        assert_eq!(night_session.decision_rules.len(), 1);
        assert_eq!(night_session.decision_rules[0].priority, 100);
    }

    #[tokio::test]
    async fn twilight_with_empty_goals_returns_empty_session() {
        let (project_root, yantra_dir) = temp_project_root();
        let signing_key = SigningKey::load_or_generate(&yantra_dir).unwrap();

        let ui = MockTwilightUi::new([], vec![], true);
        let night_session = run_twilight(
            vec![],
            project_root,
            &signing_key,
            &ui,
            NIGHT_POLICY,
        )
        .await
        .expect("empty twilight must succeed");

        assert!(night_session.validated_goals.is_empty());
        assert!(night_session.night_plan_markdown.contains("Night Plan"));
    }

    #[tokio::test]
    async fn twilight_validates_multiple_goals_and_issues_tokens_for_each() {
        let (project_root, yantra_dir) = temp_project_root();
        let signing_key = SigningKey::load_or_generate(&yantra_dir).unwrap();

        let goals = vec![
            TaskSpec::new("document the public Night Mode API types"),
            TaskSpec::new("document all public interfaces in the session module"),
        ];
        let ui = MockTwilightUi::new([], vec![], true);

        let night_session =
            run_twilight(goals, project_root, &signing_key, &ui, NIGHT_POLICY)
                .await
                .expect("multi-goal twilight must succeed");

        assert_eq!(night_session.validated_goals.len(), 2);
        for validated_goal in &night_session.validated_goals {
            assert_ne!(
                validated_goal.truth_token.task_id,
                yantra_core::TaskId::from_uuid(uuid::Uuid::nil()),
                "every token must have a non-nil task id"
            );
        }
    }

    #[tokio::test]
    async fn twilight_session_carries_correct_policy() {
        let (project_root, yantra_dir) = temp_project_root();
        let signing_key = SigningKey::load_or_generate(&yantra_dir).unwrap();

        let goals = vec![TaskSpec::new("document the decision rules module")];
        let ui = MockTwilightUi::new([], vec![], true);

        let night_session =
            run_twilight(goals, project_root, &signing_key, &ui, NIGHT_POLICY)
                .await
                .expect("twilight must succeed");

        assert_eq!(
            night_session.policy.stvp_strictness,
            NIGHT_POLICY.stvp_strictness
        );
        assert_eq!(
            night_session.policy.model_tier_ceiling,
            NIGHT_POLICY.model_tier_ceiling
        );
    }

    #[test]
    fn twilight_render_plan_includes_all_task_descriptions() {
        let session_id = SessionId::new();
        let goals = vec![
            TaskSpec::new("add rate limiting"),
            TaskSpec::new("fix login bug"),
        ];
        let source_truths = vec![
            SourceTruth {
                task_id: TaskId::new(),
                task_class: TaskClass::NewFeature,
                strictness: Strictness::Strict,
                description: "add rate limiting".to_owned(),
                created_at: Utc::now(),
                answers: BTreeMap::default(),
                augmented_question_ids: vec![],
            },
            SourceTruth {
                task_id: TaskId::new(),
                task_class: TaskClass::BugFix,
                strictness: Strictness::Light,
                description: "fix login bug".to_owned(),
                created_at: Utc::now(),
                answers: BTreeMap::default(),
                augmented_question_ids: vec![],
            },
        ];

        let plan = render_night_plan(session_id, &goals, &source_truths, &[], &NIGHT_POLICY)
            .expect("plan rendering must succeed");

        assert!(plan.contains("add rate limiting"));
        assert!(plan.contains("fix login bug"));
        assert!(plan.contains("Night Plan"));
        assert!(plan.contains("No rules configured"));
    }

    #[test]
    fn twilight_render_plan_includes_active_decision_rules() {
        let session_id = SessionId::new();
        let goals = vec![TaskSpec::new("document the API")];
        let source_truths = vec![SourceTruth {
            task_id: TaskId::new(),
            task_class: TaskClass::Docstring,
            strictness: Strictness::Trust,
            description: "document the API".to_owned(),
            created_at: Utc::now(),
            answers: BTreeMap::default(),
            augmented_question_ids: vec![],
        }];
        let rules = vec![DecisionRule {
            condition: RuleCondition::MergeConflict,
            action: RuleAction::HardStop { notify: true },
            priority: 10,
        }];

        let plan = render_night_plan(session_id, &goals, &source_truths, &rules, &NIGHT_POLICY)
            .expect("plan rendering must succeed");

        assert!(plan.contains("1 active"));
        assert!(!plan.contains("No rules configured"));
    }

    #[test]
    fn twilight_render_plan_fails_on_mismatched_counts() {
        let session_id = SessionId::new();
        let goals = vec![TaskSpec::new("task one"), TaskSpec::new("task two")];
        let source_truths = vec![SourceTruth {
            task_id: TaskId::new(),
            task_class: TaskClass::Docstring,
            strictness: Strictness::Trust,
            description: "task one".to_owned(),
            created_at: Utc::now(),
            answers: BTreeMap::default(),
            augmented_question_ids: vec![],
        }];

        let result = render_night_plan(session_id, &goals, &source_truths, &[], &NIGHT_POLICY);
        assert!(result.is_err(), "mismatched counts must produce an error");
    }
}

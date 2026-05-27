//! # forge-stvp: Dynamic Question Augmenter
//!
//! Generates context-specific, dynamic task questions by delegating to a routed LLM
//! via the `AugmenterPort` abstraction. Applies a strict 5-second timeout to ensure
//! LLM delays never block the critical validation path.
//!
//! ## Input
//! - `task_description: &str` — natural-language description of the task
//! - `task_class: TaskClass` — classified task class
//! - `existing_question_ids: &[String]` — questions already queued to avoid duplicates
//! - `max_count: usize` — maximum number of questions to generate
//!
//! ## Output
//! - `Vec<Question>` — generated dynamic questions marked as augmented
//!
//! ## Related
//! - `forge-core::task::AugmenterPort` — the boundary trait injected into the augmenter
//! - `forge-stvp::interrogator` — utilizes the augmenter during questionnaire generation

use std::sync::Arc;
use yantra_core::{AugmenterPort, TaskClass};

use crate::questionnaire::{AnswerKind, Question};

/// Dynamically generates questionnaire questions utilizing an optional LLM augmenter port.
pub struct QuestionAugmenter {
    augmenter_port: Option<Arc<dyn AugmenterPort>>,
}

impl QuestionAugmenter {
    /// Creates a new `QuestionAugmenter` with the supplied implementation port.
    pub fn new(augmenter_port: Option<Arc<dyn AugmenterPort>>) -> Self {
        Self { augmenter_port }
    }

    /// Asynchronously generates up to `max_count` additional context questions.
    /// Deduplicates against `existing_question_ids` and returns them.
    /// If the augmenter port is not set or times out, returns an empty vector.
    pub async fn augment(
        &self,
        task_description: &str,
        task_class: TaskClass,
        existing_question_ids: &[String],
        max_count: usize,
    ) -> Vec<Question> {
        let augmenter_port = match &self.augmenter_port {
            Some(active_port) => active_port,
            None => return Vec::new(),
        };

        let existing_ids = existing_question_ids.to_vec();
        let augment_future =
            augmenter_port.augment(task_description, task_class, existing_ids, max_count);
        let timeout_duration = std::time::Duration::from_secs(5);

        let timeout_result = tokio::time::timeout(timeout_duration, augment_future).await;

        let raw_augmented_questions = if let Ok(questions) = timeout_result {
            questions
        } else {
            tracing::warn!("LLM question augmentation timed out after 5 seconds");
            return Vec::new();
        };

        let mut augmented_questions = Vec::new();
        for (question_id, question_text, question_required, suggested_answer, help_text) in
            raw_augmented_questions
        {
            if question_id.trim().is_empty() || question_text.trim().is_empty() {
                continue;
            }
            let clean_identifier = question_id.trim().replace(' ', "_").to_lowercase();
            let mut question = Question::new_augmented(
                clean_identifier,
                question_text,
                AnswerKind::FreeText,
                question_required,
            );
            if let Some(suggested) = suggested_answer {
                question = question.with_suggested_answer(suggested);
            }
            if let Some(help) = help_text {
                question = question.with_help_text(help);
            }
            augmented_questions.push(question);
        }

        augmented_questions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yantra_core::AugmentFuture;

    struct MockAugmenterPort {
        mock_questions: Vec<yantra_core::AugmentedQuestionTuple>,
    }

    impl AugmenterPort for MockAugmenterPort {
        fn augment(
            &self,
            _task_description: &str,
            _task_class: TaskClass,
            _existing_question_ids: Vec<String>,
            _max_count: usize,
        ) -> AugmentFuture {
            let questions = self.mock_questions.clone();
            Box::pin(async move { questions })
        }
    }

    #[tokio::test]
    async fn augmenter_returns_empty_when_no_port() {
        let augmenter = QuestionAugmenter::new(None);
        let questions = augmenter
            .augment("add JWT rotation", TaskClass::NewFeature, &[], 2)
            .await;
        assert!(questions.is_empty());
    }

    #[tokio::test]
    async fn augmenter_correctly_parses_and_tags_questions() {
        let mock_port = Arc::new(MockAugmenterPort {
            mock_questions: vec![(
                "auth_mechanism".to_string(),
                "Which auth mechanism?".to_string(),
                true,
                Some("JWT".to_string()),
                Some("The authentication token standard to use".to_string()),
            )],
        });

        let augmenter = QuestionAugmenter::new(Some(mock_port));
        let questions = augmenter
            .augment("add JWT rotation", TaskClass::NewFeature, &[], 2)
            .await;

        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].id, "auth_mechanism");
        assert_eq!(questions[0].text, "Which auth mechanism?");
        assert!(questions[0].required);
        assert!(questions[0].augmented);
        assert_eq!(questions[0].suggested_answer, Some("JWT".to_string()));
    }
}

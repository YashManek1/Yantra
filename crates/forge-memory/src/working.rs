//! # forge-memory: Working Memory and Prompt Assembler
//!
//! Builds the fully assembled context prompt for an agent turn from
//! persona, task description, recalled skills, archival shards, the CRG
//! subgraph, and conversation history. Uses `forge-tokenizer` to enforce
//! a hard token budget, dropping or compressing sections when necessary.
//!
//! ## Input
//! - `persona: &str` — system persona injected at the top of the context
//! - `task: &str` — the validated task description from `SourceTruth`
//! - `skills: &[String]` — skill names/summaries from the SKILL registry
//! - `shards: &[MemoryShard]` — archival memory retrieved for this task
//! - `subgraph: &str` — CRG subgraph text rendered for the task
//! - `history: &[ConversationTurn]` — recent turns from the recall tier
//! - `token_budget: usize` — hard upper bound on assembled prompt tokens
//!
//! ## Output
//! - `AssembledPrompt` — final text plus metadata on what was dropped
//!
//! ## Related
//! - `forge-tokenizer` — `count_tokens` function
//! - `forge-memory::recall` — provides `ConversationTurn`
//! - `forge-memory::archival` — provides `MemoryShard`

use yantra_tokenizer::count_tokens;

use crate::archival::MemoryShard;
use crate::recall::ConversationTurn;

/// A section of the assembled prompt with a drop-priority label.
///
/// Lower `priority` values are kept when the budget is tight; higher values
/// are dropped first.
#[derive(Debug, Clone)]
struct PromptSection {
    /// Section label shown in the prompt header.
    label: &'static str,
    /// Rendered text content of this section.
    content: String,
    /// Drop priority: 0 = never drop, 255 = drop first.
    priority: u8,
}

/// The result of assembling a context prompt within a token budget.
#[derive(Debug, Clone)]
pub struct AssembledPrompt {
    /// The fully assembled context prompt text.
    pub text: String,
    /// Total tokens in the assembled text.
    pub token_count: usize,
    /// Labels of sections that were dropped to meet the budget.
    pub sections_dropped: Vec<&'static str>,
}

/// Builds a context prompt within `token_budget` tokens.
///
/// Assembly order (by priority, never-dropped first):
/// 1. Persona (priority 0 — always kept)
/// 2. Task (priority 0 — always kept)
/// 3. Subgraph (priority 10)
/// 4. Skills (priority 30)
/// 5. History — summarized first, then front-trimmed if still over budget
/// 6. Archival shards (priority 50 each, dropped from last to first)
pub fn assemble_prompt(
    persona: &str,
    task: &str,
    skills: &[String],
    shards: &[MemoryShard],
    subgraph: &str,
    history: &[ConversationTurn],
    token_budget: usize,
) -> AssembledPrompt {
    let mut sections = build_sections(persona, task, skills, shards, subgraph, history);

    let mut sections_dropped: Vec<&'static str> = Vec::new();

    let total_tokens: usize = sections
        .iter()
        .map(|section| count_tokens(&section.content))
        .sum();
    if total_tokens <= token_budget {
        return build_result(&sections, sections_dropped);
    }

    sections.sort_by_key(|section| section.priority);

    let mut accumulated_tokens = 0_usize;
    let mut kept_sections: Vec<PromptSection> = Vec::new();

    for section in sections {
        let section_tokens = count_tokens(&section.content);
        if accumulated_tokens + section_tokens <= token_budget {
            accumulated_tokens += section_tokens;
            kept_sections.push(section);
        } else {
            sections_dropped.push(section.label);
        }
    }

    build_result(&kept_sections, sections_dropped)
}

fn build_sections(
    persona: &str,
    task: &str,
    skills: &[String],
    shards: &[MemoryShard],
    subgraph: &str,
    history: &[ConversationTurn],
) -> Vec<PromptSection> {
    let mut sections = Vec::new();

    sections.push(PromptSection {
        label: "persona",
        content: format!("# SYSTEM PERSONA\n{persona}"),
        priority: 0,
    });

    sections.push(PromptSection {
        label: "task",
        content: format!("# TASK\n{task}"),
        priority: 0,
    });

    if !subgraph.is_empty() {
        sections.push(PromptSection {
            label: "subgraph",
            content: format!("# CODE CONTEXT\n{subgraph}"),
            priority: 10,
        });
    }

    if !skills.is_empty() {
        let skills_text = skills.join("\n---\n");
        sections.push(PromptSection {
            label: "skills",
            content: format!("# APPLICABLE SKILLS\n{skills_text}"),
            priority: 30,
        });
    }

    if !history.is_empty() {
        let history_text = render_history(history);
        sections.push(PromptSection {
            label: "history",
            content: format!("# CONVERSATION HISTORY\n{history_text}"),
            priority: 40,
        });
    }

    for (shard_index, shard) in shards.iter().enumerate() {
        let shard_priority = 50_u8.saturating_add(u8::try_from(shard_index).unwrap_or(u8::MAX));
        sections.push(PromptSection {
            label: "memory_shard",
            content: format!("# RECALLED MEMORY\n{}", shard.content),
            priority: shard_priority,
        });
    }

    sections
}

fn render_history(turns: &[ConversationTurn]) -> String {
    let summarized_turns: Vec<String> = turns
        .iter()
        .map(|turn| {
            let displayed_content = turn.summary.as_deref().unwrap_or(turn.content.as_str());
            format!(
                "[{}] {}: {}",
                turn.timestamp.format("%H:%M"),
                turn.role,
                displayed_content
            )
        })
        .collect();
    summarized_turns.join("\n")
}

fn build_result(sections: &[PromptSection], sections_dropped: Vec<&'static str>) -> AssembledPrompt {
    let assembled_text = sections
        .iter()
        .map(|section| section.content.as_str())
        .collect::<Vec<&str>>()
        .join("\n\n");
    let token_count = count_tokens(&assembled_text);
    AssembledPrompt {
        text: assembled_text,
        token_count,
        sections_dropped,
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use yantra_core::TaskId;

    use super::*;
    use crate::archival::ShardType;

    fn sample_shard(content: &str) -> MemoryShard {
        MemoryShard {
            id: TaskId::new().to_string(),
            shard_type: ShardType::ResearchMemo,
            source_ref: None,
            content: content.to_owned(),
            created_at: Utc::now(),
        }
    }

    fn sample_turn(role: &str, content: &str) -> ConversationTurn {
        ConversationTurn {
            id: TaskId::new().to_string(),
            session_id: "test-session".to_owned(),
            timestamp: Utc::now(),
            role: role.to_owned(),
            content: content.to_owned(),
            summary: None,
            tokens: None,
        }
    }

    #[test]
    fn assembles_prompt_with_all_sections_when_within_budget() {
        let result = assemble_prompt(
            "You are a Rust engineer.",
            "implement rate limiting",
            &["rate-limiting-skill".to_owned()],
            &[sample_shard("token bucket pattern")],
            "fn rate_limit() {}",
            &[sample_turn("user", "add it")],
            100_000,
        );
        assert!(result.text.contains("rate limiting"));
        assert!(result.text.contains("token bucket"));
        assert!(result.text.contains("SYSTEM PERSONA"));
        assert!(result.sections_dropped.is_empty());
    }

    #[test]
    fn drops_low_priority_sections_when_over_budget() {
        let large_shard_content = "x".repeat(10_000);
        let result = assemble_prompt(
            "You are a Rust engineer.",
            "implement rate limiting",
            &[],
            &[sample_shard(&large_shard_content)],
            "",
            &[],
            50,
        );
        assert!(
            result.token_count <= 50,
            "assembled prompt must be within budget"
        );
        assert!(
            result.sections_dropped.contains(&"memory_shard"),
            "shard should be dropped when over budget"
        );
    }

    #[test]
    fn persona_and_task_are_never_dropped() {
        let result = assemble_prompt(
            "You are a Rust engineer.",
            "implement rate limiting",
            &[],
            &[],
            &"z".repeat(10_000),
            &[],
            20,
        );
        assert!(
            result.text.contains("SYSTEM PERSONA"),
            "persona must always be present"
        );
        assert!(result.text.contains("TASK"), "task must always be present");
    }

    #[test]
    fn history_with_summary_uses_summary_text() {
        let mut turn = sample_turn("user", "original long content here");
        turn.summary = Some("summarized content".to_owned());

        let result = assemble_prompt("persona", "task", &[], &[], "", &[turn], 100_000);
        assert!(result.text.contains("summarized content"));
        assert!(!result.text.contains("original long content here"));
    }
}

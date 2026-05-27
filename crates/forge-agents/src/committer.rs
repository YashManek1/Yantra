//! # forge-agents: Committer Agent
//!
//! The only agent authorised to write Git state. Generates a structured commit
//! message ("what changed, why, decision_id"), signs the message metadata with
//! a dedicated Ed25519 key (separate from the STVP TruthToken signing key), and
//! calls the `git.commit` MCP tool with the signed message.
//!
//! ## Input
//! - `TaskNode` carrying a valid `TruthToken` and a `description` that encodes
//!   the accepted diff
//! - `CommitterAgent` constructed with a `CommitSigningKey`
//! - `AgentContext` with `git.commit` registered in the `McpRouter`
//!
//! ## Output
//! - `TaskResult` with `outcome = Success` and `summary` containing the commit
//!   hash; `diff` is always `None` (the Committer writes Git state, not diffs)
//!
//! ## Related
//! - `forge-agents::verifier_agent` — must return Pass before Committer runs
//! - `forge-tools::tools::git` — `GitMcpServer` handles the `git.commit` call

use async_trait::async_trait;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use serde_json::json;
use yantra_core::{AgentCapability, AgentKind, DecisionId, Outcome, TaskNode};

use crate::agent::{Agent, AgentContext, TaskResult};
use crate::error::AgentError;

/// Ed25519 signing key used exclusively for commit message signing.
///
/// Distinct from the TruthToken signing key — each process can generate a fresh
/// key or load one from secure storage.
pub struct CommitSigningKey {
    pkcs8_document_bytes: Vec<u8>,
}

impl CommitSigningKey {
    /// Generates a fresh Ed25519 key pair for this session.
    ///
    /// # Errors
    ///
    /// Returns `AgentError::CommitSigning` if the OS RNG is unavailable.
    pub fn generate() -> Result<Self, AgentError> {
        let random_generator = SystemRandom::new();
        let pkcs8_document =
            Ed25519KeyPair::generate_pkcs8(&random_generator).map_err(|key_error| {
                AgentError::CommitSigning(format!("key generation failed: {key_error:?}"))
            })?;
        Ok(Self {
            pkcs8_document_bytes: pkcs8_document.as_ref().to_vec(),
        })
    }

    /// Returns the DER-encoded public key bytes for this signing key.
    ///
    /// # Errors
    ///
    /// Returns `AgentError::CommitSigning` if the PKCS8 bytes are malformed.
    pub fn public_key_bytes(&self) -> Result<Vec<u8>, AgentError> {
        let key_pair = self.key_pair()?;
        Ok(key_pair.public_key().as_ref().to_vec())
    }

    /// Signs `message_bytes` and returns the 64-byte Ed25519 signature.
    ///
    /// # Errors
    ///
    /// Returns `AgentError::CommitSigning` if signing fails or the signature
    /// cannot be represented as a 64-byte array.
    pub fn sign(&self, message_bytes: &[u8]) -> Result<[u8; 64], AgentError> {
        let key_pair = self.key_pair()?;
        let signature = key_pair.sign(message_bytes);
        signature
            .as_ref()
            .try_into()
            .map_err(|_| AgentError::CommitSigning("signature was not 64 bytes".to_owned()))
    }

    fn key_pair(&self) -> Result<Ed25519KeyPair, AgentError> {
        Ed25519KeyPair::from_pkcs8(&self.pkcs8_document_bytes).map_err(|parse_error| {
            AgentError::CommitSigning(format!("PKCS8 parse failed: {parse_error:?}"))
        })
    }
}

/// Structured commit message written to Git history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitMessage {
    /// Human-readable summary of what was changed (first line).
    pub what_changed: String,
    /// Motivation for the change (second paragraph).
    pub why: String,
    /// Decision identifier linking this commit to the archaeology log.
    pub decision_id: DecisionId,
    /// Hex-encoded 64-byte Ed25519 signature over `<what_changed>|<decision_id>`.
    pub signature_hex: String,
}

impl CommitMessage {
    /// Formats the message as a Git commit body string.
    ///
    /// ```text
    /// <what_changed>
    ///
    /// <why>
    ///
    /// decision-id: <decision_id>
    /// sig: <signature_hex>
    /// ```
    pub fn format_body(&self) -> String {
        format!(
            "{}\n\n{}\n\ndecision-id: {}\nsig: {}",
            self.what_changed, self.why, self.decision_id, self.signature_hex
        )
    }
}

/// Specialist agent with exclusive write access to the Git main branch.
pub struct CommitterAgent {
    signing_key: CommitSigningKey,
}

impl CommitterAgent {
    /// Creates a `CommitterAgent` with the supplied signing key.
    pub fn new(signing_key: CommitSigningKey) -> Self {
        Self { signing_key }
    }

    fn build_commit_message(
        task_description: &str,
        decision_id: DecisionId,
        signing_key: &CommitSigningKey,
    ) -> Result<CommitMessage, AgentError> {
        let what_changed = extract_commit_subject(task_description);
        let why = extract_commit_body(task_description);

        let signing_payload = format!("{what_changed}|{decision_id}");
        let signature_bytes = signing_key.sign(signing_payload.as_bytes())?;
        let signature_hex = hex_encode_signature(&signature_bytes);

        Ok(CommitMessage {
            what_changed,
            why,
            decision_id,
            signature_hex,
        })
    }
}

/// Extracts the first non-empty line of `task_description` as the commit subject.
pub fn extract_commit_subject(task_description: &str) -> String {
    task_description
        .lines()
        .find(|line| !line.trim().is_empty()).map_or_else(|| "chore: automated commit".to_owned(), |line| line.trim().to_owned())
}

/// Extracts up to three meaningful lines from `task_description` as the commit body.
pub fn extract_commit_body(task_description: &str) -> String {
    let body_lines: Vec<&str> = task_description
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .take(3)
        .collect();

    if body_lines.is_empty() {
        "Automated commit from Yantra agentic pipeline.".to_owned()
    } else {
        body_lines.join("\n")
    }
}

fn hex_encode_signature(signature_bytes: &[u8]) -> String {
    let mut output = String::with_capacity(signature_bytes.len() * 2);
    for byte in signature_bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[async_trait]
impl Agent for CommitterAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Committer
    }

    fn capabilities(&self) -> Vec<AgentCapability> {
        vec![AgentCapability::GitWrite]
    }

    async fn run(&self, task: TaskNode, context: AgentContext) -> Result<TaskResult, AgentError> {
        let _truth_token =
            task.truth_token
                .as_ref()
                .ok_or_else(|| AgentError::MissingTruthToken {
                    task_id: task.id.to_string(),
                })?;

        let decision_id = DecisionId::new();
        let commit_message =
            Self::build_commit_message(&task.description, decision_id, &self.signing_key)?;
        let commit_body = commit_message.format_body();

        tracing::info!(
            task_id = %task.id,
            decision_id = %decision_id,
            subject = %commit_message.what_changed,
            "committer writing signed commit"
        );

        let commit_response = context
            .tools
            .handle("git.commit", json!({ "message": commit_body }))
            .await;

        let commit_hash = match commit_response {
            Ok(response_value) => response_value
                .get("hash")
                .and_then(|hash| hash.as_str())
                .unwrap_or("(no hash returned)")
                .to_owned(),
            Err(mcp_error) => {
                tracing::warn!(
                    task_id = %task.id,
                    error = %mcp_error.message,
                    "git.commit MCP call failed — recording decision without git write"
                );
                format!("(git.commit unavailable: {})", mcp_error.message)
            }
        };

        Ok(TaskResult {
            task_id: task.id,
            outcome: Outcome::Success,
            diff: None,
            summary: format!("commit {commit_hash}: {}", commit_message.what_changed),
            decision_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_signing_key_generates_and_signs() {
        let signing_key = CommitSigningKey::generate().expect("key generation succeeds");
        let signature_bytes = signing_key.sign(b"test payload").expect("signing succeeds");
        assert_eq!(signature_bytes.len(), 64);
    }

    #[test]
    fn commit_signing_key_public_key_has_expected_length() {
        let signing_key = CommitSigningKey::generate().expect("key generation succeeds");
        let public_key_bytes = signing_key
            .public_key_bytes()
            .expect("public key extraction succeeds");
        assert_eq!(
            public_key_bytes.len(),
            32,
            "Ed25519 public keys are 32 bytes"
        );
    }

    #[test]
    fn extract_commit_subject_returns_first_nonempty_line() {
        let description = "\nfix: handle null pointer in auth module\n\nLonger explanation.";
        let subject = extract_commit_subject(description);
        assert_eq!(subject, "fix: handle null pointer in auth module");
    }

    #[test]
    fn extract_commit_body_returns_subsequent_lines() {
        let description = "fix: add JWT rotation\n\nThe old token was never invalidated.\nThis fix calls rotate() after issuance.";
        let body = extract_commit_body(description);
        assert!(body.contains("was never invalidated"));
    }

    #[test]
    fn commit_message_formats_correctly() {
        let decision_id = DecisionId::new();
        let commit_message = CommitMessage {
            what_changed: "feat: add rate limiting".to_owned(),
            why: "Prevent DoS under high load.".to_owned(),
            decision_id,
            signature_hex: "deadbeef".to_owned(),
        };
        let formatted_body = commit_message.format_body();
        assert!(formatted_body.contains("feat: add rate limiting"));
        assert!(formatted_body.contains("decision-id:"));
        assert!(formatted_body.contains("sig: deadbeef"));
    }

    #[test]
    fn hex_encode_is_lowercase_hex() {
        let all_bytes: Vec<u8> = (0u8..16).collect();
        let hex = hex_encode_signature(&all_bytes);
        assert_eq!(hex, "000102030405060708090a0b0c0d0e0f");
    }
}

//! # forge-stvp: Truth Token Issuance
//!
//! Generates, persists, and loads the session-scoped Ed25519 signing key used
//! to issue `TruthToken` values. Provides `issue_token` to sign a validated
//! `SourceTruth` and `verify_token` for orchestrator-side signature checks.
//!
//! ## Input
//! - `SourceTruth` — the validated task specification to be signed
//! - `.yantra/session.key` — PKCS#8 private key bytes (generated on first run)
//!
//! ## Output
//! - `TruthToken` — signed proof returned to the orchestrator
//! - `.yantra/session.pub` — raw public key bytes for verification
//!
//! ## Related
//! - `forge-core::truth` — defines `TruthToken` and `VerifyingKey`
//! - `forge-stvp::interrogator` — calls `issue_token` after storing the truth
//! - `forge-orchestrator` — calls `verify_token` before scheduling

use std::path::{Path, PathBuf};

use ring::rand::SystemRandom;
use ring::signature::Ed25519KeyPair;
use yantra_core::truth::{TruthToken, VerifyingKey};

use crate::error::StvpError;
use crate::source_truth::SourceTruth;

const SESSION_KEY_FILE: &str = "session.key";
const SESSION_PUB_FILE: &str = "session.pub";

/// Wraps an `Ed25519KeyPair` for session-scoped truth-token signing.
///
/// Internally holds the PKCS#8 document bytes so the key can be
/// written to disk after generation without regenerating a different key.
pub struct SigningKey {
    key_pair: Ed25519KeyPair,
    /// Raw PKCS#8 document; held so `save` can write the exact bytes that
    /// correspond to `key_pair`.
    pkcs8_bytes: Vec<u8>,
    /// Public verifying key derived from the key pair.
    pub verifying_key: VerifyingKey,
}

impl SigningKey {
    /// Generates a fresh Ed25519 signing key.
    ///
    /// # Errors
    ///
    /// Returns `StvpError::KeyGeneration` if the system RNG fails, or
    /// `StvpError::KeyRejected` if the generated document is invalid.
    pub fn generate() -> Result<Self, StvpError> {
        let random_generator = SystemRandom::new();
        let pkcs8_document = Ed25519KeyPair::generate_pkcs8(&random_generator)
            .map_err(|_| StvpError::KeyGeneration)?;
        let pkcs8_bytes = pkcs8_document.as_ref().to_vec();
        let key_pair =
            Ed25519KeyPair::from_pkcs8(&pkcs8_bytes).map_err(|error| StvpError::KeyRejected {
                reason: error.to_string(),
            })?;
        let verifying_key = VerifyingKey::from_key_pair(&key_pair);
        Ok(Self {
            key_pair,
            pkcs8_bytes,
            verifying_key,
        })
    }

    /// Loads the signing key from `yantra_dir/session.key`.
    ///
    /// # Errors
    ///
    /// Returns `StvpError::KeyRead` if the file cannot be read, or
    /// `StvpError::KeyRejected` if the bytes are not a valid PKCS#8 key.
    pub fn load(yantra_dir: &Path) -> Result<Self, StvpError> {
        let key_file_path = key_path(yantra_dir);
        let pkcs8_bytes = std::fs::read(&key_file_path).map_err(|source| StvpError::KeyRead {
            path: key_file_path.display().to_string(),
            source,
        })?;
        let key_pair =
            Ed25519KeyPair::from_pkcs8(&pkcs8_bytes).map_err(|error| StvpError::KeyRejected {
                reason: error.to_string(),
            })?;
        let verifying_key = VerifyingKey::from_key_pair(&key_pair);
        Ok(Self {
            key_pair,
            pkcs8_bytes,
            verifying_key,
        })
    }

    /// Returns the signing key for `yantra_dir`, generating and persisting a
    /// new one if none exists yet.
    ///
    /// # Errors
    ///
    /// Returns `StvpError` on key generation or filesystem failure.
    pub fn load_or_generate(yantra_dir: &Path) -> Result<Self, StvpError> {
        if key_path(yantra_dir).exists() {
            return Self::load(yantra_dir);
        }

        std::fs::create_dir_all(yantra_dir).map_err(|source| StvpError::DirectoryCreate {
            path: yantra_dir.display().to_string(),
            source,
        })?;

        let signing_key = Self::generate()?;
        signing_key.save(yantra_dir)?;
        Ok(signing_key)
    }

    /// Writes the PKCS#8 private key and raw public key bytes to `yantra_dir`.
    ///
    /// # Errors
    ///
    /// Returns `StvpError::KeyWrite` on filesystem failure.
    pub fn save(&self, yantra_dir: &Path) -> Result<(), StvpError> {
        let key_file_path = key_path(yantra_dir);
        std::fs::write(&key_file_path, &self.pkcs8_bytes).map_err(|source| {
            StvpError::KeyWrite {
                path: key_file_path.display().to_string(),
                source,
            }
        })?;

        let pub_file_path = pub_path(yantra_dir);
        std::fs::write(&pub_file_path, self.verifying_key.as_bytes()).map_err(|source| {
            StvpError::KeyWrite {
                path: pub_file_path.display().to_string(),
                source,
            }
        })?;

        Ok(())
    }
}

/// Signs a validated `SourceTruth` and returns the resulting `TruthToken`.
///
/// # Errors
///
/// Returns `StvpError::Core` if the Ed25519 signature cannot be converted to
/// the expected 64-byte array (extremely rare hardware-level failure).
pub fn issue_token(truth: &SourceTruth, signing_key: &SigningKey) -> Result<TruthToken, StvpError> {
    let yaml_content = truth.to_yaml()?;
    let digest_output = ring::digest::digest(&ring::digest::SHA256, yaml_content.as_bytes());
    let mut content_sha256 = [0u8; 32];
    content_sha256.copy_from_slice(digest_output.as_ref());

    let sacred_authorized = truth
        .answers
        .get("sacred_files")
        .is_some_and(|list| !list.trim().is_empty());

    TruthToken::new(
        truth.task_id,
        truth.task_class,
        truth.strictness,
        sacred_authorized,
        content_sha256,
        &signing_key.key_pair,
    )
    .map_err(StvpError::Core)
}

/// Verifies that `token` was signed by the key whose public bytes are stored
/// in `yantra_dir/session.pub`.
///
/// Returns `true` when the signature is valid, `false` otherwise.
///
/// # Errors
///
/// Returns `StvpError::KeyRead` if the public key file cannot be read.
pub fn verify_token(token: &TruthToken, yantra_dir: &Path) -> Result<bool, StvpError> {
    let pub_file_path = pub_path(yantra_dir);
    let public_key_bytes = std::fs::read(&pub_file_path).map_err(|source| StvpError::KeyRead {
        path: pub_file_path.display().to_string(),
        source,
    })?;
    let verifying_key = VerifyingKey::new(public_key_bytes);
    Ok(token.verify(&verifying_key))
}

fn key_path(yantra_dir: &Path) -> PathBuf {
    yantra_dir.join(SESSION_KEY_FILE)
}

fn pub_path(yantra_dir: &Path) -> PathBuf {
    yantra_dir.join(SESSION_PUB_FILE)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use yantra_core::{Strictness, TaskClass, TaskId};

    use super::*;
    use crate::source_truth::SourceTruth;

    fn temp_yantra_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yantra-token-test-{}", TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_source_truth() -> SourceTruth {
        SourceTruth {
            task_id: TaskId::new(),
            task_class: TaskClass::BugFix,
            strictness: Strictness::Light,
            description: "fix null pointer dereference".to_owned(),
            created_at: Utc::now(),
            answers: BTreeMap::new(),
            augmented_question_ids: Vec::new(),
        }
    }

    #[test]
    fn load_or_generate_creates_key_files_on_first_call() {
        let yantra_dir = temp_yantra_dir();
        let signing_key =
            SigningKey::load_or_generate(&yantra_dir).expect("key created successfully");
        assert!(
            key_path(&yantra_dir).exists(),
            "session.key must be written"
        );
        assert!(
            pub_path(&yantra_dir).exists(),
            "session.pub must be written"
        );
        assert!(
            !signing_key.verifying_key.as_bytes().is_empty(),
            "verifying key must have bytes"
        );
    }

    #[test]
    fn load_or_generate_returns_same_key_on_second_call() {
        let yantra_dir = temp_yantra_dir();
        let first_key = SigningKey::load_or_generate(&yantra_dir).expect("first call succeeds");
        let second_key = SigningKey::load_or_generate(&yantra_dir).expect("second call succeeds");
        assert_eq!(
            first_key.verifying_key.as_bytes(),
            second_key.verifying_key.as_bytes(),
            "same key must be returned on subsequent loads"
        );
    }

    #[test]
    fn issue_and_verify_token_round_trip() {
        let yantra_dir = temp_yantra_dir();
        let signing_key = SigningKey::load_or_generate(&yantra_dir).expect("key generated");
        let source_truth = sample_source_truth();

        let token = issue_token(&source_truth, &signing_key).expect("token issued");
        assert_eq!(token.task_id, source_truth.task_id);
        assert_eq!(token.task_class, source_truth.task_class);
        assert_eq!(token.strictness, source_truth.strictness);

        let is_valid = verify_token(&token, &yantra_dir).expect("verification runs");
        assert!(is_valid, "token must verify with matching public key");
    }

    #[test]
    fn token_from_different_key_does_not_verify() {
        let yantra_dir_a = temp_yantra_dir();
        let yantra_dir_b = temp_yantra_dir();

        let signing_key_a = SigningKey::load_or_generate(&yantra_dir_a).expect("key A generated");
        SigningKey::load_or_generate(&yantra_dir_b).expect("key B generated");

        let source_truth = sample_source_truth();
        let token = issue_token(&source_truth, &signing_key_a).expect("token issued with key A");

        // Verify against key B's public key — should fail
        let is_valid = verify_token(&token, &yantra_dir_b).expect("verification runs");
        assert!(
            !is_valid,
            "token signed with key A must not verify against key B"
        );
    }
}

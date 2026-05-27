//! # forge-core: Truth Tokens
//!
//! Defines the signed proof of validated user intent that gates task
//! scheduling. Tokens are signed with Ed25519 using `ring` and verified from
//! canonical token payload bytes.
//!
//! ## Input
//! - Task identifier, task class, strictness, timestamp, and signing key
//!
//! ## Output
//! - `TruthToken` values and signature verification results
//!
//! ## Related
//! - `forge-core::task` — embeds truth tokens in task nodes

use chrono::{DateTime, Utc};
use ring::signature::{self, Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};
use crate::id::TaskId;
use crate::task::TaskClass;

/// `STVP` strictness level used to validate a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Strictness {
    /// Full validation for high-risk tasks.
    Strict,
    /// Reduced validation for narrow fixes and refactors.
    Light,
    /// Trusted path for low-risk style or documentation tasks.
    Trust,
}

impl Strictness {
    /// Returns the stable lowercase string representation of the strictness level.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Light => "light",
            Self::Trust => "trust",
        }
    }
}

/// Public Ed25519 verifying key used for truth-token verification.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VerifyingKey {
    bytes: Vec<u8>,
}

impl VerifyingKey {
    /// Creates a verifying key from raw public key bytes.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    /// Extracts the verifying key from an Ed25519 key pair.
    pub fn from_key_pair(signing_key_pair: &Ed25519KeyPair) -> Self {
        Self::new(signing_key_pair.public_key().as_ref())
    }

    /// Returns raw public key bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Signed proof that `STVP` validated a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruthToken {
    /// Task authorized by the token.
    pub task_id: TaskId,
    /// Class of task authorized by the token.
    pub task_class: TaskClass,
    /// Timestamp at which the token was issued.
    pub issued_at: DateTime<Utc>,
    /// Validation strictness used to issue the token.
    pub strictness: Strictness,
    /// Ed25519 signature over the canonical token payload.
    #[serde(with = "signature_bytes")]
    pub signature: [u8; 64],
}

impl TruthToken {
    /// Creates and signs a truth token.
    ///
    /// # Errors
    ///
    /// Returns `CoreError::SigningFailed` if the generated Ed25519 signature
    /// cannot be represented as the expected 64-byte signature array.
    pub fn new(
        task_id: TaskId,
        task_class: TaskClass,
        strictness: Strictness,
        signing_key_pair: &Ed25519KeyPair,
    ) -> CoreResult<Self> {
        let issued_at = Utc::now();
        let payload = canonical_payload(task_id, task_class, issued_at, strictness);
        let signature = signing_key_pair.sign(payload.as_bytes());
        let signature_bytes: [u8; 64] = signature
            .as_ref()
            .try_into()
            .map_err(|_| CoreError::SigningFailed)?;

        Ok(Self {
            task_id,
            task_class,
            issued_at,
            strictness,
            signature: signature_bytes,
        })
    }

    /// Verifies the token signature with the supplied public key.
    pub fn verify(&self, public_key: &VerifyingKey) -> bool {
        let payload = canonical_payload(
            self.task_id,
            self.task_class,
            self.issued_at,
            self.strictness,
        );
        let public_key =
            signature::UnparsedPublicKey::new(&signature::ED25519, public_key.as_bytes());
        public_key
            .verify(payload.as_bytes(), &self.signature)
            .is_ok()
    }
}

fn canonical_payload(
    task_id: TaskId,
    task_class: TaskClass,
    issued_at: DateTime<Utc>,
    strictness: Strictness,
) -> String {
    format!(
        "v1|{}|{}|{}|{}",
        task_id,
        task_class.as_str(),
        issued_at.timestamp_nanos_opt().unwrap_or_default(),
        strictness.as_str()
    )
}

mod signature_bytes {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(signature: &[u8; 64], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(signature)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 64], D::Error>
    where
        D: Deserializer<'de>,
    {
        let signature = Vec::<u8>::deserialize(deserializer)?;
        signature
            .try_into()
            .map_err(|signature: Vec<u8>| D::Error::invalid_length(signature.len(), &"64 bytes"))
    }
}

#[cfg(test)]
mod tests {
    use ring::rand::SystemRandom;
    use ring::signature::Ed25519KeyPair;

    use super::*;

    #[test]
    fn truth_token_verify_succeeds_and_tamper_fails() {
        let random_generator = SystemRandom::new();
        let key_pair_document = Ed25519KeyPair::generate_pkcs8(&random_generator).unwrap();
        let signing_key_pair = Ed25519KeyPair::from_pkcs8(key_pair_document.as_ref()).unwrap();
        let verifying_key = VerifyingKey::from_key_pair(&signing_key_pair);
        let task_id = TaskId::new();

        let truth_token = TruthToken::new(
            task_id,
            TaskClass::BugFix,
            Strictness::Light,
            &signing_key_pair,
        )
        .unwrap();
        let mut tampered_truth_token = truth_token.clone();
        tampered_truth_token.task_class = TaskClass::NewFeature;

        assert!(truth_token.verify(&verifying_key));
        assert!(!tampered_truth_token.verify(&verifying_key));
    }
}

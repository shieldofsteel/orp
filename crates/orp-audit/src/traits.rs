//! Shared `AuditLogger` trait + error type for the in-memory and persistent
//! backends.
//!
//! The trait is intentionally narrow: callers only need `record`, `replay`,
//! `verify_chain`, and `len`. Backend-specific details (DuckDB connection,
//! ring buffer state) are private to each implementation.

use crate::entry::AuditEntry;
use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("Database error: {0}")]
    Database(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Chain corrupted at sequence {seq}: {reason}")]
    ChainCorrupt { seq: u64, reason: String },
    #[error("Signature invalid at sequence {seq}")]
    BadSignature { seq: u64 },
    #[error("IO error: {0}")]
    Io(String),
    #[error("Hex decode error: {0}")]
    Hex(String),
}

pub type AuditResult<T> = Result<T, AuditError>;

/// Append-only, hash-chained, Ed25519-signed audit log.
#[async_trait]
pub trait AuditLogger: Send + Sync {
    /// Append a new entry. The sequence number, previous_hash, content_hash,
    /// and signature are computed by the implementation. Returns the fully-
    /// realised entry that was persisted.
    async fn record(
        &self,
        operation: &str,
        entity_type: Option<&str>,
        entity_id: Option<&str>,
        user_id: Option<&str>,
        details: serde_json::Value,
    ) -> AuditResult<AuditEntry>;

    /// Return all entries in sequence order (small logs / debugging) or up to
    /// `limit` if specified.
    async fn replay(&self, limit: Option<usize>) -> AuditResult<Vec<AuditEntry>>;

    /// Re-derive every row's `content_hash`, walk the prev_hash linkage, and
    /// verify the Ed25519 signature using `verifier`. Returns the seq of the
    /// first bad row or `Ok(())` if the chain is whole.
    async fn verify_chain(&self, verifier: &VerifyKey) -> AuditResult<()>;

    /// Number of rows currently in the log.
    async fn len(&self) -> AuditResult<u64>;

    /// True iff the log has no rows. Default-impl simply checks `len() == 0`;
    /// backends with cheaper "any rows?" SQL can override.
    async fn is_empty(&self) -> AuditResult<bool> {
        Ok(self.len().await? == 0)
    }

    /// Return the public key that was used to sign rows, hex-encoded. Useful
    /// for stamping exports so external auditors know which key to verify
    /// against. Returns `None` for backends that don't carry their own key
    /// (legacy in-memory mode constructed without a signer).
    async fn public_key_hex(&self) -> Option<String>;
}

/// Verification handle — a verifying key, by hex or raw bytes.
///
/// We pass this to `verify_chain` rather than embedding it on the audit log
/// because external auditors verify *exports*, where the writing process and
/// its private key are long gone. They supply the recorded public key out-of-
/// band.
pub struct VerifyKey(pub ed25519_dalek::VerifyingKey);

impl VerifyKey {
    pub fn from_hex(hex_str: &str) -> Result<Self, AuditError> {
        let bytes = hex::decode(hex_str.trim()).map_err(|e| AuditError::Hex(e.to_string()))?;
        if bytes.len() != 32 {
            return Err(AuditError::Hex(format!(
                "expected 32 bytes, got {}",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&arr)
            .map_err(|e| AuditError::Hex(e.to_string()))?;
        Ok(Self(vk))
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AuditError> {
        if bytes.len() != 32 {
            return Err(AuditError::Hex(format!(
                "expected 32 bytes, got {}",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&arr)
            .map_err(|e| AuditError::Hex(e.to_string()))?;
        Ok(Self(vk))
    }

    pub fn verify_signature(&self, hash_hex: &str, signature_hex: &str) -> bool {
        let sig_bytes = match hex::decode(signature_hex) {
            Ok(b) if b.len() == 64 => b,
            _ => return false,
        };
        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes);
        let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);
        use ed25519_dalek::Verifier;
        self.0.verify(hash_hex.as_bytes(), &sig).is_ok()
    }
}

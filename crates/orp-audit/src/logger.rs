//! In-memory audit log backend.
//!
//! Used in tests and when the operator launches with `--in-memory`. Identical
//! semantics to [`crate::PersistentAuditLog`] — same hash function, same
//! pre-image format, same Ed25519 signature pipeline — so behavioural test
//! suites written against one apply to the other.

use crate::crypto::EventSigner;
use crate::entry::{canonical_preimage, compute_content_hash, AuditEntry, GENESIS_PREV_HASH};
use crate::traits::{AuditError, AuditLogger, AuditResult, VerifyKey};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// RAM-backed audit log. Vec-based; cheap for small ledgers and tests.
///
/// This struct used to be named `AuditLog` (pre-v0.3.0). The rename to
/// `InMemoryAuditLog` reflects that we now have a peer persistent backend.
pub struct InMemoryAuditLog {
    inner: Mutex<Vec<AuditEntry>>,
    signer: Arc<EventSigner>,
}

impl InMemoryAuditLog {
    /// Create a new log with a fresh Ed25519 signer.
    pub fn new() -> Self {
        Self::with_signer(Arc::new(EventSigner::new()))
    }

    /// Create a new log using a caller-provided signer (so multiple components
    /// can share the same key material — e.g. AppState's `audit_signer`).
    pub fn with_signer(signer: Arc<EventSigner>) -> Self {
        Self {
            inner: Mutex::new(Vec::new()),
            signer,
        }
    }

    pub fn signer(&self) -> &Arc<EventSigner> {
        &self.signer
    }
}

impl Default for InMemoryAuditLog {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AuditLogger for InMemoryAuditLog {
    async fn record(
        &self,
        operation: &str,
        entity_type: Option<&str>,
        entity_id: Option<&str>,
        user_id: Option<&str>,
        details: serde_json::Value,
    ) -> AuditResult<AuditEntry> {
        let mut entries = self
            .inner
            .lock()
            .map_err(|e| AuditError::Database(format!("mutex poisoned: {}", e)))?;

        let seq = entries.len() as u64 + 1;
        let timestamp = chrono::Utc::now();
        let previous_hash = entries
            .last()
            .map(|e| e.content_hash.clone())
            .unwrap_or_else(|| GENESIS_PREV_HASH.to_string());

        let preimage = canonical_preimage(
            seq,
            &timestamp,
            operation,
            entity_type,
            entity_id,
            user_id,
            &details,
        );
        let content_hash = compute_content_hash(&previous_hash, &preimage);
        let signature_bytes = self.signer.sign(content_hash.as_bytes());
        let signature = hex::encode(signature_bytes);

        let entry = AuditEntry {
            sequence_number: seq,
            timestamp,
            operation: operation.to_string(),
            entity_type: entity_type.map(String::from),
            entity_id: entity_id.map(String::from),
            user_id: user_id.map(String::from),
            details,
            previous_hash,
            content_hash,
            signature,
        };
        entries.push(entry.clone());
        Ok(entry)
    }

    async fn replay(&self, limit: Option<usize>) -> AuditResult<Vec<AuditEntry>> {
        let entries = self
            .inner
            .lock()
            .map_err(|e| AuditError::Database(format!("mutex poisoned: {}", e)))?;
        let take = limit.unwrap_or(entries.len());
        Ok(entries.iter().take(take).cloned().collect())
    }

    async fn verify_chain(&self, verifier: &VerifyKey) -> AuditResult<()> {
        let entries = self
            .inner
            .lock()
            .map_err(|e| AuditError::Database(format!("mutex poisoned: {}", e)))?;

        let mut prev = GENESIS_PREV_HASH.to_string();
        for (i, entry) in entries.iter().enumerate() {
            let expected_seq = (i as u64) + 1;
            if entry.sequence_number != expected_seq {
                return Err(AuditError::ChainCorrupt {
                    seq: entry.sequence_number,
                    reason: format!(
                        "out-of-order sequence: expected {}, got {}",
                        expected_seq, entry.sequence_number
                    ),
                });
            }
            if entry.previous_hash != prev {
                return Err(AuditError::ChainCorrupt {
                    seq: entry.sequence_number,
                    reason: format!(
                        "previous_hash mismatch: expected {}, got {}",
                        prev, entry.previous_hash
                    ),
                });
            }
            let preimage = canonical_preimage(
                entry.sequence_number,
                &entry.timestamp,
                &entry.operation,
                entry.entity_type.as_deref(),
                entry.entity_id.as_deref(),
                entry.user_id.as_deref(),
                &entry.details,
            );
            let expected_hash = compute_content_hash(&entry.previous_hash, &preimage);
            if expected_hash != entry.content_hash {
                return Err(AuditError::ChainCorrupt {
                    seq: entry.sequence_number,
                    reason: format!(
                        "content_hash mismatch: expected {}, got {}",
                        expected_hash, entry.content_hash
                    ),
                });
            }
            if !verifier.verify_signature(&entry.content_hash, &entry.signature) {
                return Err(AuditError::BadSignature {
                    seq: entry.sequence_number,
                });
            }
            prev = entry.content_hash.clone();
        }
        Ok(())
    }

    async fn len(&self) -> AuditResult<u64> {
        let entries = self
            .inner
            .lock()
            .map_err(|e| AuditError::Database(format!("mutex poisoned: {}", e)))?;
        Ok(entries.len() as u64)
    }

    async fn public_key_hex(&self) -> Option<String> {
        Some(hex::encode(self.signer.public_key_bytes()))
    }
}

// ── Backwards-compatible alias ────────────────────────────────────────────────
//
// The pre-v0.3.0 type was `AuditLog`. Some downstream code (and the older
// `compute_sha256` test harness) still references it under that name. We
// re-export `InMemoryAuditLog` as `AuditLog` so existing call sites continue
// to compile without changes.
pub type AuditLog = InMemoryAuditLog;

#[cfg(test)]
mod tests {
    use super::*;

    fn vk_for(log: &InMemoryAuditLog) -> VerifyKey {
        VerifyKey::from_bytes(&log.signer().public_key_bytes()).unwrap()
    }

    #[tokio::test]
    async fn append_and_verify() {
        let log = InMemoryAuditLog::new();
        log.record(
            "entity_created",
            Some("ship"),
            Some("mmsi:123"),
            Some("system"),
            serde_json::json!({"name": "Foo"}),
        )
        .await
        .unwrap();
        log.record(
            "property_updated",
            Some("ship"),
            Some("mmsi:123"),
            Some("ais"),
            serde_json::json!({"speed": 12.5}),
        )
        .await
        .unwrap();
        assert_eq!(log.len().await.unwrap(), 2);
        let vk = vk_for(&log);
        log.verify_chain(&vk).await.unwrap();
    }

    #[tokio::test]
    async fn detect_tamper() {
        let log = InMemoryAuditLog::new();
        log.record(
            "entity_created",
            Some("ship"),
            Some("mmsi:1"),
            None,
            serde_json::json!({}),
        )
        .await
        .unwrap();
        log.record(
            "entity_updated",
            Some("ship"),
            Some("mmsi:1"),
            None,
            serde_json::json!({"speed": 10}),
        )
        .await
        .unwrap();
        let vk = vk_for(&log);
        log.verify_chain(&vk).await.unwrap();

        // Tamper with the operation field of the first row, which is part of
        // the hash pre-image — verify_chain must catch it.
        {
            let mut entries = log.inner.lock().unwrap();
            entries[0].operation = "tampered".to_string();
        }
        match log.verify_chain(&vk).await {
            Err(AuditError::ChainCorrupt { seq, .. }) => assert_eq!(seq, 1),
            other => panic!("expected ChainCorrupt, got {:?}", other),
        }
    }
}

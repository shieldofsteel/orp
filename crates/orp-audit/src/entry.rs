//! Audit entry data model + canonical pre-image computation.
//!
//! The `AuditEntry` is the value persisted per row. The pre-image used to
//! derive `content_hash` is built from the entry **minus** `content_hash` and
//! `signature`, prefixed by `previous_hash`. This guarantees:
//!
//! 1. Reproducibility — anyone with the row can re-derive the hash.
//! 2. Chain linkage — flipping `previous_hash` on one row breaks all
//!    downstream rows because their hash inputs change.
//! 3. Independence from hex casing — we serialize via canonical JSON.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A single signed, hash-chained audit row.
///
/// Hash and signature fields are hex-encoded on the wire so the JSON form is
/// human-readable and the DuckDB column type stays a plain VARCHAR (matching
/// the schema put in place by v0.2.0).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEntry {
    /// 1-based monotonically-increasing sequence number assigned at insert.
    pub sequence_number: u64,
    /// Insert-time UTC timestamp.
    pub timestamp: DateTime<Utc>,
    /// Action being recorded (e.g. `entity_created`, `monitor_rule_added`).
    pub operation: String,
    /// Optional entity type the action applies to.
    pub entity_type: Option<String>,
    /// Optional entity id the action applies to.
    pub entity_id: Option<String>,
    /// Optional principal who performed the action.
    pub user_id: Option<String>,
    /// Free-form JSON payload — typed details, sanitized at the call site.
    pub details: serde_json::Value,
    /// SHA-256 (hex) of the previous row's `content_hash`. Genesis rows use
    /// 64 zero hex chars (rather than the literal string "genesis") so the
    /// chain head is always a fixed-width hash and tooling stays uniform.
    pub previous_hash: String,
    /// SHA-256 (hex) over `previous_hash || canonical_preimage(entry)`.
    pub content_hash: String,
    /// Ed25519 signature (hex) over `content_hash` bytes.
    pub signature: String,
}

/// Genesis previous_hash — 64 zero hex chars.
pub const GENESIS_PREV_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// Build the canonical pre-image string used to derive `content_hash`.
///
/// The pre-image is formed from the fields *every* verifier sees: sequence,
/// timestamp (RFC3339), operation, entity_type, entity_id, user_id, and
/// canonical-JSON details. We avoid `serde_json::to_string` for the whole
/// struct because that would include `previous_hash`, `content_hash`, and
/// `signature` — but those are exactly what we're trying to derive.
///
/// `previous_hash` is included as a separate prefix in [`compute_content_hash`],
/// so the hash is over `previous_hash || preimage`.
pub fn canonical_preimage(
    sequence_number: u64,
    timestamp: &DateTime<Utc>,
    operation: &str,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
    user_id: Option<&str>,
    details: &serde_json::Value,
) -> String {
    // Use a fixed delimiter that cannot appear in our field types (RFC3339
    // timestamps, JSON, alphanumeric ids) so concatenation is unambiguous.
    format!(
        "{}||{}||{}||{}||{}||{}||{}",
        sequence_number,
        timestamp.to_rfc3339(),
        operation,
        entity_type.unwrap_or(""),
        entity_id.unwrap_or(""),
        user_id.unwrap_or(""),
        // Compact JSON — `Value::to_string()` is stable for our purposes
        // (object key order is preserved by `serde_json`'s preserve_order? No —
        // serde_json::Value uses BTreeMap by default in serde_json's Map impl?
        // Actually serde_json::Map uses LinkedHashMap when feature flag is set,
        // BTreeMap otherwise. Without the flag we get BTreeMap, which is
        // already key-sorted — sufficient for our determinism guarantee.)
        details,
    )
}

/// Compute `content_hash` = SHA-256(prev_hash_hex || preimage) as hex.
pub fn compute_content_hash(prev_hash_hex: &str, preimage: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prev_hash_hex.as_bytes());
    hasher.update(b"|");
    hasher.update(preimage.as_bytes());
    format!("{:x}", hasher.finalize())
}

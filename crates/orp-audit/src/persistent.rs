//! DuckDB-backed audit log.
//!
//! Schema (compatible with the v0.2.0 `audit_log` table — we keep the existing
//! VARCHAR columns rather than reshaping to BLOB so old rows from the legacy
//! `DuckDbStorage::log_audit` path stay readable):
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS audit_log (
//!     sequence_number  BIGINT PRIMARY KEY,
//!     timestamp        TIMESTAMP NOT NULL,
//!     operation        VARCHAR  NOT NULL,
//!     entity_type      VARCHAR,
//!     entity_id        VARCHAR,
//!     user_id          VARCHAR,
//!     previous_hash    VARCHAR,
//!     content_hash     VARCHAR NOT NULL,
//!     signature        VARCHAR,
//!     details          JSON
//! );
//! ```
//!
//! Concurrency:
//! * One `Arc<Mutex<Connection>>` is shared with the rest of `DuckDbStorage`
//!   so audit appends serialise with entity inserts (DuckDB itself is
//!   single-writer-per-connection).
//! * A separate `chain_lock: tokio::sync::Mutex<()>` guards the SELECT-then-
//!   INSERT critical section so two concurrent `record()` calls cannot pick
//!   the same `prev_hash` and produce a forked chain.
//!
//! Crash safety:
//! * Each append runs inside a DuckDB transaction (`BEGIN`/`COMMIT`). A power
//!   loss leaves the chain intact at the last fully-committed seq.

use crate::crypto::EventSigner;
use crate::entry::{canonical_preimage, compute_content_hash, AuditEntry, GENESIS_PREV_HASH};
use crate::traits::{AuditError, AuditLogger, AuditResult, VerifyKey};
use async_trait::async_trait;
use duckdb::{params, Connection};
use std::sync::{Arc, Mutex};

/// Round a `DateTime<Utc>` down to microsecond precision so it survives a
/// DuckDB `TIMESTAMP` round-trip without losing bits the audit-chain hash
/// is computed over. Without this the chain replay computes a different
/// pre-image than the one that was hashed at insert time.
fn truncate_to_micros(dt: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    use chrono::Timelike;
    let micros = dt.nanosecond() / 1_000;
    dt.with_nanosecond(micros * 1_000).unwrap_or(dt)
}

/// Try to interpret a `details` cell as the at-rest envelope
/// `{"orpaead1": "<base64>"}`. Returns:
/// * `Ok(Some(plaintext_json))` — the cell was sealed and decrypted cleanly.
/// * `Ok(None)` — the cell is not in envelope shape (legacy plaintext row).
/// * `Err(AtRestError)` — envelope shape but decryption failed (wrong key
///   or tampered ciphertext); caller should surface the error.
fn try_unseal_envelope(
    cell: &str,
    key: &crate::AtRestKey,
) -> Result<Option<String>, crate::AtRestError> {
    let parsed: serde_json::Value = match serde_json::from_str(cell) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let Some(obj) = parsed.as_object() else {
        return Ok(None);
    };
    if obj.len() != 1 {
        return Ok(None);
    }
    let Some(blob) = obj.get("orpaead1").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let plain = key.unseal(blob)?;
    String::from_utf8(plain)
        .map(Some)
        .map_err(|_| crate::AtRestError::Malformed)
}

/// SQL fragments — kept here so this crate is self-contained and the table is
/// idempotently created even if `orp-storage`'s base schema hasn't run yet
/// (e.g. when callers use `PersistentAuditLog::open` against a bare DB file).
pub const AUDIT_LOG_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS audit_log (
    sequence_number  BIGINT PRIMARY KEY,
    timestamp        TIMESTAMP NOT NULL,
    operation        VARCHAR NOT NULL,
    entity_type      VARCHAR,
    entity_id        VARCHAR,
    user_id          VARCHAR,
    previous_hash    VARCHAR,
    content_hash     VARCHAR NOT NULL,
    signature        VARCHAR,
    details          JSON
);
CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_operation ON audit_log(operation);
CREATE INDEX IF NOT EXISTS idx_audit_user      ON audit_log(user_id);
"#;

/// DuckDB-persisted audit log. Owns no resources of its own beyond a shared
/// `Arc<Mutex<Connection>>` and an `Arc<EventSigner>`.
pub struct PersistentAuditLog {
    conn: Arc<Mutex<Connection>>,
    signer: Arc<EventSigner>,
    /// Optional at-rest envelope key. When set, the `details` column is
    /// AES-256-GCM-sealed before INSERT and unsealed on read. Closes
    /// P-audit Wave 2 F7 for the most sensitive column without requiring
    /// DuckDB encryption-extension support. The chain hash is computed
    /// over the plaintext JSON, so verification still works across the
    /// encrypt/decrypt boundary.
    at_rest: Option<Arc<crate::AtRestKey>>,
    /// Async-safe lock that wraps the SELECT-prev → INSERT-new critical
    /// section. We could fold this into the connection mutex (since DuckDB
    /// already serialises) but a dedicated lock keeps the contract explicit
    /// and makes future migration to a serialised writer task trivial.
    chain_lock: tokio::sync::Mutex<()>,
}

impl PersistentAuditLog {
    /// Construct against an existing DuckDB connection. The `audit_log` table
    /// is created if missing (idempotent).
    pub fn from_connection(
        conn: Arc<Mutex<Connection>>,
        signer: Arc<EventSigner>,
    ) -> AuditResult<Self> {
        {
            let c = conn
                .lock()
                .map_err(|e| AuditError::Database(format!("mutex poisoned: {}", e)))?;
            c.execute_batch(AUDIT_LOG_SCHEMA)
                .map_err(|e| AuditError::Database(e.to_string()))?;
        }
        Ok(Self {
            conn,
            signer,
            at_rest: None,
            chain_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// Builder-style: enable at-rest envelope encryption for the `details`
    /// column. Mixed-mode is supported — pre-existing plaintext rows
    /// continue to read correctly, and new rows are sealed.
    pub fn with_at_rest_key(mut self, key: Arc<crate::AtRestKey>) -> Self {
        self.at_rest = Some(key);
        self
    }

    /// Open a fresh DuckDB file dedicated to the audit log. Used by the
    /// `orp audit verify` / `orp audit export` CLI subcommands which read a
    /// caller-supplied DB without spinning up the full storage engine.
    pub fn open(path: &std::path::Path, signer: Arc<EventSigner>) -> AuditResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AuditError::Io(e.to_string()))?;
        }
        let conn = Connection::open(path).map_err(|e| AuditError::Database(e.to_string()))?;
        Self::from_connection(Arc::new(Mutex::new(conn)), signer)
    }

    /// Number of currently-persisted rows. Cheap COUNT(*).
    fn count_rows(conn: &Connection) -> AuditResult<u64> {
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0))
            .map_err(|e| AuditError::Database(e.to_string()))?;
        Ok(n as u64)
    }

    /// Read the (seq, content_hash) of the most recently persisted row, or
    /// `(0, GENESIS_PREV_HASH)` if the log is empty.
    fn last_row(conn: &Connection) -> AuditResult<(u64, String)> {
        let row: Option<(i64, Option<String>)> = conn
            .query_row(
                "SELECT sequence_number, content_hash
                 FROM audit_log
                 ORDER BY sequence_number DESC
                 LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        Ok(match row {
            Some((seq, Some(h))) => (seq as u64, h),
            Some((seq, None)) => (seq as u64, GENESIS_PREV_HASH.to_string()),
            None => (0, GENESIS_PREV_HASH.to_string()),
        })
    }

    fn parse_ts(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                    .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f"))
                    .map(|n| n.and_utc())
                    .unwrap_or_else(|_| chrono::Utc::now())
            })
    }

    /// Iterate rows in seq-ascending order, returning materialised entries.
    /// At-rest envelope (when configured on the log) is reversed here so
    /// callers always see plaintext `details`. Pre-encrypted (legacy)
    /// rows that don't carry the ORPAEAD1 magic pass through unchanged.
    pub fn read_all(conn: &Connection) -> AuditResult<Vec<AuditEntry>> {
        Self::read_all_with(conn, None)
    }

    /// Variant that accepts an at-rest key for unsealing the `details`
    /// column. Used by instance methods that already hold an `Arc<AtRestKey>`.
    pub fn read_all_with(
        conn: &Connection,
        at_rest: Option<&crate::AtRestKey>,
    ) -> AuditResult<Vec<AuditEntry>> {
        let mut stmt = conn
            .prepare(
                "SELECT sequence_number, CAST(timestamp AS VARCHAR) AS ts,
                        operation, entity_type, entity_id, user_id,
                        previous_hash, content_hash, signature, details
                 FROM audit_log
                 ORDER BY sequence_number ASC",
            )
            .map_err(|e| AuditError::Database(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| AuditError::Database(e.to_string()))?;

        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| AuditError::Database(e.to_string()))?
        {
            let seq: i64 = row
                .get(0)
                .map_err(|e| AuditError::Database(e.to_string()))?;
            let ts_str: String = row
                .get(1)
                .map_err(|e| AuditError::Database(e.to_string()))?;
            let operation: String = row
                .get(2)
                .map_err(|e| AuditError::Database(e.to_string()))?;
            let entity_type: Option<String> = row
                .get(3)
                .map_err(|e| AuditError::Database(e.to_string()))?;
            let entity_id: Option<String> = row
                .get(4)
                .map_err(|e| AuditError::Database(e.to_string()))?;
            let user_id: Option<String> = row
                .get(5)
                .map_err(|e| AuditError::Database(e.to_string()))?;
            let previous_hash: Option<String> = row
                .get(6)
                .map_err(|e| AuditError::Database(e.to_string()))?;
            let content_hash: String = row
                .get(7)
                .map_err(|e| AuditError::Database(e.to_string()))?;
            let signature: Option<String> = row
                .get(8)
                .map_err(|e| AuditError::Database(e.to_string()))?;
            let raw_details: String = row
                .get::<_, Option<String>>(9)
                .map_err(|e| AuditError::Database(e.to_string()))?
                .unwrap_or_else(|| "null".to_string());
            // Detect the sealed envelope: a JSON object with a sole key
            // `orpaead1` whose value is the base64-AEAD blob. Anything else
            // is treated as legacy plaintext JSON (mixed-mode migration).
            let details_str: String = match at_rest {
                Some(key) => match try_unseal_envelope(&raw_details, key) {
                    Ok(Some(plain)) => plain,
                    Ok(None) => raw_details,
                    Err(e) => return Err(AuditError::Database(format!("at-rest unseal: {e}"))),
                },
                None => raw_details,
            };
            let details: serde_json::Value = serde_json::from_str(&details_str)
                .unwrap_or(serde_json::Value::String(details_str));

            out.push(AuditEntry {
                sequence_number: seq as u64,
                timestamp: Self::parse_ts(&ts_str),
                operation,
                entity_type,
                entity_id,
                user_id,
                details,
                previous_hash: previous_hash.unwrap_or_else(|| GENESIS_PREV_HASH.to_string()),
                content_hash,
                signature: signature.unwrap_or_default(),
            });
        }
        Ok(out)
    }

    pub fn signer(&self) -> &Arc<EventSigner> {
        &self.signer
    }
}

#[async_trait]
impl AuditLogger for PersistentAuditLog {
    async fn record(
        &self,
        operation: &str,
        entity_type: Option<&str>,
        entity_id: Option<&str>,
        user_id: Option<&str>,
        details: serde_json::Value,
    ) -> AuditResult<AuditEntry> {
        // Hold the chain lock across the SELECT-prev-then-INSERT-new sequence.
        // Without this, two concurrent appenders could read the same prev_hash
        // and race to INSERT — DuckDB's PK on sequence_number would reject the
        // loser, but the loser's chain logic would still believe it had
        // succeeded. The lock keeps the chain head linearised.
        let _guard = self.chain_lock.lock().await;

        // Truncate to microsecond precision BEFORE hashing. DuckDB's
        // TIMESTAMP column rounds nanoseconds, so a hash computed from the
        // nanosecond-precision `Utc::now()` would diverge from the hash
        // recomputed during replay (read-back hits the truncated value).
        // This was a platform-portable failure hiding behind macOS's
        // sometimes-zero-ns clock — Linux CI surfaced it consistently.
        let timestamp = truncate_to_micros(chrono::Utc::now());

        let conn = self
            .conn
            .lock()
            .map_err(|e| AuditError::Database(format!("mutex poisoned: {}", e)))?;

        let (last_seq, previous_hash) = Self::last_row(&conn)?;
        let seq = last_seq + 1;

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

        // Write inside an explicit transaction. DuckDB's autocommit would also
        // be atomic here (single-row INSERT), but BEGIN/COMMIT documents the
        // intent and lets us extend the critical section later (e.g. dual
        // writes to a side index) without restructuring.
        conn.execute_batch("BEGIN TRANSACTION")
            .map_err(|e| AuditError::Database(e.to_string()))?;

        let timestamp_str = timestamp.to_rfc3339();
        // The chain hash is computed over the plaintext JSON above. The
        // stored bytes can be the same JSON OR a JSON-wrapped AES-GCM
        // envelope (`{"orpaead1": "<base64>"}`) so the column-level JSON
        // type contract holds — either way `read_all` reverses the
        // encoding and `verify_chain` recomputes from the plaintext.
        let plaintext_details = details.to_string();
        let details_str = match &self.at_rest {
            Some(key) => {
                let sealed = key
                    .seal(plaintext_details.as_bytes())
                    .map_err(|e| AuditError::Database(format!("at-rest seal failed: {e}")))?;
                serde_json::json!({ "orpaead1": sealed }).to_string()
            }
            None => plaintext_details,
        };

        let insert_res = conn.execute(
            "INSERT INTO audit_log
               (sequence_number, timestamp, operation, entity_type, entity_id,
                user_id, previous_hash, content_hash, signature, details)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                seq as i64,
                timestamp_str,
                operation,
                entity_type,
                entity_id,
                user_id,
                previous_hash,
                content_hash,
                signature,
                details_str,
            ],
        );

        match insert_res {
            Ok(_) => {
                conn.execute_batch("COMMIT")
                    .map_err(|e| AuditError::Database(e.to_string()))?;
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(AuditError::Database(e.to_string()));
            }
        }

        Ok(AuditEntry {
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
        })
    }

    async fn replay(&self, limit: Option<usize>) -> AuditResult<Vec<AuditEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AuditError::Database(format!("mutex poisoned: {}", e)))?;
        let mut all = Self::read_all_with(&conn, self.at_rest.as_deref())?;
        if let Some(n) = limit {
            all.truncate(n);
        }
        Ok(all)
    }

    async fn verify_chain(&self, verifier: &VerifyKey) -> AuditResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AuditError::Database(format!("mutex poisoned: {}", e)))?;
        let entries = Self::read_all_with(&conn, self.at_rest.as_deref())?;
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
        let conn = self
            .conn
            .lock()
            .map_err(|e| AuditError::Database(format!("mutex poisoned: {}", e)))?;
        Self::count_rows(&conn)
    }

    async fn public_key_hex(&self) -> Option<String> {
        Some(hex::encode(self.signer.public_key_bytes()))
    }
}

// ── Export support ────────────────────────────────────────────────────────────

/// One JSONL line of an audit export.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct ExportLine {
    pub seq: u64,
    pub ts: String,
    pub actor: Option<String>,
    pub action: String,
    pub target: Option<String>,
    pub payload: serde_json::Value,
    pub prev_hash_hex: String,
    pub hash_hex: String,
    pub signature_hex: String,
    /// True iff (a) the row's content_hash matches its pre-image AND (b) the
    /// signature verifies against the supplied public key. False if the
    /// caller had no public key (in which case `signature_hex` is reported
    /// untouched).
    pub verified: bool,
}

/// Stream every row in `audit_log` to `out` as JSONL, computing per-row
/// verification using `verifier` if supplied.
pub fn export_jsonl<W: std::io::Write>(
    conn: &Connection,
    verifier: Option<&VerifyKey>,
    mut out: W,
) -> AuditResult<usize> {
    let entries = PersistentAuditLog::read_all(conn)?;
    let mut prev = GENESIS_PREV_HASH.to_string();
    let mut count = 0;
    for entry in entries {
        // Re-derive hash so callers see verified == false on tamper even if
        // the row's stored hash is consistent-with-itself but inconsistent
        // with the chain.
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
        let chain_ok = entry.previous_hash == prev
            && expected_hash == entry.content_hash
            && entry.sequence_number == (count as u64) + 1;
        let sig_ok = verifier
            .map(|v| v.verify_signature(&entry.content_hash, &entry.signature))
            .unwrap_or(false);

        let line = ExportLine {
            seq: entry.sequence_number,
            ts: entry.timestamp.to_rfc3339(),
            actor: entry.user_id.clone(),
            action: entry.operation.clone(),
            target: entry.entity_id.clone(),
            payload: entry.details.clone(),
            prev_hash_hex: entry.previous_hash.clone(),
            hash_hex: entry.content_hash.clone(),
            signature_hex: entry.signature.clone(),
            verified: chain_ok && sig_ok,
        };
        let json =
            serde_json::to_string(&line).map_err(|e| AuditError::Serialization(e.to_string()))?;
        out.write_all(json.as_bytes())
            .map_err(|e| AuditError::Io(e.to_string()))?;
        out.write_all(b"\n")
            .map_err(|e| AuditError::Io(e.to_string()))?;
        prev = entry.content_hash;
        count += 1;
    }
    out.flush().map_err(|e| AuditError::Io(e.to_string()))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn tmp_db() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("audit.duckdb");
        (dir, path)
    }

    fn vk_from_signer(s: &EventSigner) -> VerifyKey {
        VerifyKey::from_bytes(&s.public_key_bytes()).unwrap()
    }

    #[test]
    fn truncate_to_micros_drops_only_sub_microsecond_bits() {
        use chrono::{Datelike, TimeZone, Timelike};
        // Pick a timestamp with non-zero nanoseconds — the bug masking case
        // is exactly when the live clock returns sub-microsecond precision.
        let dt = chrono::Utc
            .with_ymd_and_hms(2026, 5, 1, 15, 24, 0)
            .unwrap()
            .with_nanosecond(123_456_789)
            .unwrap();
        let truncated = truncate_to_micros(dt);
        // Only the bottom 3 digits (789 ns) are dropped.
        assert_eq!(truncated.nanosecond(), 123_456_000);
        // Higher-order fields are untouched.
        assert_eq!(truncated.second(), 0);
        assert_eq!(truncated.year(), 2026);
        // The truncated value is a fixed point of the function.
        assert_eq!(truncate_to_micros(truncated), truncated);
    }

    #[tokio::test]
    async fn at_rest_encrypted_details_round_trip_and_chain_verifies() {
        // F7 end-to-end: with an at-rest key configured, the `details`
        // column is sealed on INSERT and unsealed on read. Chain hash
        // is over the plaintext JSON, so verify_chain still passes.
        use crate::AtRestKey;

        let signer = Arc::new(EventSigner::new());
        let pubkey = signer.public_key_bytes();
        let key = Arc::new(AtRestKey::from_bytes(&[42u8; 32]).unwrap());
        let (_dir, path) = tmp_db();

        {
            let log = PersistentAuditLog::open(&path, signer.clone())
                .unwrap()
                .with_at_rest_key(key.clone());
            log.record(
                "entity_created",
                Some("ship"),
                Some("mmsi:42"),
                Some("system"),
                serde_json::json!({"name": "Sealed Boat", "secret_field": "PII"}),
            )
            .await
            .unwrap();
        }

        // Re-open with the same key. Chain replays cleanly.
        {
            let log = PersistentAuditLog::open(&path, signer.clone())
                .unwrap()
                .with_at_rest_key(key.clone());
            let rows = log.replay(None).await.unwrap();
            assert_eq!(rows.len(), 1);
            // Plaintext recovered through the unseal pass.
            assert_eq!(rows[0].details["name"], "Sealed Boat");
            assert_eq!(rows[0].details["secret_field"], "PII");
            // Chain verifies.
            let vk = VerifyKey::from_bytes(&pubkey).unwrap();
            log.verify_chain(&vk).await.unwrap();
        }

        // Confirm the at-rest discipline: opening WITHOUT the key (i.e.
        // a stolen DB without the sidecar key file) cannot read details.
        {
            let log = PersistentAuditLog::open(&path, signer.clone()).unwrap();
            let rows = log.replay(None).await.unwrap();
            assert_eq!(rows.len(), 1);
            // The sealed blob comes back as a JSON string (since
            // serde_json::from_str fails on it and falls into the String
            // fallback). Either way, the plaintext is NOT visible.
            let recovered = rows[0].details.to_string();
            assert!(!recovered.contains("Sealed Boat"));
            assert!(!recovered.contains("secret_field"));
        }
    }

    #[tokio::test]
    async fn at_rest_mixed_mode_reads_legacy_plaintext_rows() {
        // Migration scenario: a database has rows from before encryption
        // was turned on. After enabling at-rest, the new rows are sealed
        // but the old plaintext rows must still read correctly.
        use crate::AtRestKey;

        let signer = Arc::new(EventSigner::new());
        let key = Arc::new(AtRestKey::from_bytes(&[1u8; 32]).unwrap());
        let (_dir, path) = tmp_db();

        // Phase 1: write a plaintext row.
        {
            let log = PersistentAuditLog::open(&path, signer.clone()).unwrap();
            log.record(
                "legacy",
                None,
                None,
                None,
                serde_json::json!({"era": "before-at-rest"}),
            )
            .await
            .unwrap();
        }

        // Phase 2: append a sealed row.
        {
            let log = PersistentAuditLog::open(&path, signer.clone())
                .unwrap()
                .with_at_rest_key(key.clone());
            log.record(
                "post_at_rest",
                None,
                None,
                None,
                serde_json::json!({"era": "after-at-rest"}),
            )
            .await
            .unwrap();

            // Both rows read correctly through the keyed log.
            let rows = log.replay(None).await.unwrap();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].details["era"], "before-at-rest");
            assert_eq!(rows[1].details["era"], "after-at-rest");
        }
    }

    #[tokio::test]
    async fn append_close_reopen_chain_continues() {
        // Persist a stable signer across reopen — the chain replay only
        // verifies *hashes*, but we need the same public key to verify
        // *signatures* after reopen.
        let signer = Arc::new(EventSigner::new());
        let pubkey = signer.public_key_bytes();
        let (_dir, path) = tmp_db();
        {
            let log = PersistentAuditLog::open(&path, signer.clone()).unwrap();
            log.record(
                "entity_created",
                Some("ship"),
                Some("mmsi:1"),
                Some("system"),
                serde_json::json!({"name": "Bow"}),
            )
            .await
            .unwrap();
            log.record(
                "entity_updated",
                Some("ship"),
                Some("mmsi:1"),
                Some("ais"),
                serde_json::json!({"speed": 12}),
            )
            .await
            .unwrap();
            assert_eq!(log.len().await.unwrap(), 2);
        }
        // Reopen the DB and append a third entry. Chain head must continue
        // from row #2's content_hash, not from genesis.
        {
            let log = PersistentAuditLog::open(&path, signer.clone()).unwrap();
            assert_eq!(log.len().await.unwrap(), 2);
            let third = log
                .record(
                    "entity_deleted",
                    Some("ship"),
                    Some("mmsi:1"),
                    Some("admin"),
                    serde_json::json!({}),
                )
                .await
                .unwrap();
            assert_eq!(third.sequence_number, 3);
            // Verify chain end-to-end with the persisted public key.
            let vk = VerifyKey::from_bytes(&pubkey).unwrap();
            log.verify_chain(&vk).await.unwrap();
        }
    }

    #[tokio::test]
    async fn detect_payload_corruption_via_raw_sql() {
        let signer = Arc::new(EventSigner::new());
        let (_dir, path) = tmp_db();
        let log = PersistentAuditLog::open(&path, signer.clone()).unwrap();
        log.record(
            "entity_created",
            Some("ship"),
            Some("mmsi:1"),
            Some("system"),
            serde_json::json!({"speed": 5}),
        )
        .await
        .unwrap();
        log.record(
            "entity_updated",
            Some("ship"),
            Some("mmsi:1"),
            Some("system"),
            serde_json::json!({"speed": 10}),
        )
        .await
        .unwrap();

        // Corrupt the payload of seq=1 directly via SQL — the stored content
        // hash is now stale.
        {
            let conn = log.conn.lock().unwrap();
            conn.execute(
                "UPDATE audit_log SET details = ? WHERE sequence_number = 1",
                params![r#"{"speed":99999}"#],
            )
            .unwrap();
        }

        let vk = vk_from_signer(&signer);
        match log.verify_chain(&vk).await {
            Err(AuditError::ChainCorrupt { seq, .. }) => assert_eq!(seq, 1),
            other => panic!("expected ChainCorrupt, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn concurrent_appends_serialise() {
        let signer = Arc::new(EventSigner::new());
        let (_dir, path) = tmp_db();
        let log = Arc::new(PersistentAuditLog::open(&path, signer.clone()).unwrap());

        let mut handles = Vec::new();
        for thread_idx in 0..10 {
            let log = log.clone();
            handles.push(tokio::spawn(async move {
                for i in 0..100 {
                    log.record(
                        "concurrent_op",
                        Some("test"),
                        Some(&format!("entity:{}-{}", thread_idx, i)),
                        Some("tester"),
                        serde_json::json!({"i": i, "thread": thread_idx}),
                    )
                    .await
                    .unwrap();
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(log.len().await.unwrap(), 1000);
        let vk = vk_from_signer(&signer);
        log.verify_chain(&vk).await.unwrap();

        // Sequence numbers must be a contiguous 1..=1000 run.
        let entries = log.replay(None).await.unwrap();
        for (i, e) in entries.iter().enumerate() {
            assert_eq!(e.sequence_number, (i as u64) + 1);
        }
    }

    #[tokio::test]
    async fn export_round_trip() {
        let signer = Arc::new(EventSigner::new());
        let pubkey = signer.public_key_bytes();
        let (_dir, path) = tmp_db();
        let log = PersistentAuditLog::open(&path, signer.clone()).unwrap();
        for i in 0..5 {
            log.record(
                "test_op",
                Some("e"),
                Some(&format!("id-{}", i)),
                Some("u"),
                serde_json::json!({"i": i}),
            )
            .await
            .unwrap();
        }

        let mut buf: Vec<u8> = Vec::new();
        let vk = VerifyKey::from_bytes(&pubkey).unwrap();
        let written = {
            let conn = log.conn.lock().unwrap();
            super::export_jsonl(&conn, Some(&vk), &mut buf).unwrap()
        };
        assert_eq!(written, 5);

        let lines: Vec<&[u8]> = buf
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(lines.len(), 5);
        for (i, raw) in lines.iter().enumerate() {
            let line: ExportLine = serde_json::from_slice(raw).unwrap();
            assert_eq!(line.seq, (i as u64) + 1);
            assert!(line.verified, "row {} should verify", i + 1);
            assert_eq!(line.action, "test_op");
        }
    }

    #[tokio::test]
    async fn export_marks_unverified_when_no_pubkey() {
        let signer = Arc::new(EventSigner::new());
        let (_dir, path) = tmp_db();
        let log = PersistentAuditLog::open(&path, signer).unwrap();
        log.record("op", None, None, None, serde_json::json!({}))
            .await
            .unwrap();

        let mut buf: Vec<u8> = Vec::new();
        let written = {
            let conn = log.conn.lock().unwrap();
            super::export_jsonl(&conn, None, &mut buf).unwrap()
        };
        assert_eq!(written, 1);
        let line: ExportLine =
            serde_json::from_slice(buf.split(|&b| b == b'\n').next().unwrap()).unwrap();
        // Without a public key, signature verification can't pass — verified
        // must be false.
        assert!(!line.verified);
    }
}

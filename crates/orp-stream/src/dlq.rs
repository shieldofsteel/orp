//! Dead Letter Queue (DLQ) — spec Section 8.4.
//!
//! RocksDB-backed persistent store for events that failed processing.
//! Survives binary restart. Supports manual inspection and retry.
//!
//! Key   = `<event_id>`
//! Value = JSON-encoded `DlqEntry`

use chrono::{DateTime, Utc};
use rocksdb::{Options, DB};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DlqError {
    #[error("RocksDB error: {0}")]
    Rocks(#[from] rocksdb::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("Bincode error: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("Peer id too long: {0} bytes (max 65535)")]
    PeerIdTooLong(usize),
}

pub type DlqResult<T> = Result<T, DlqError>;

/// A single DLQ record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlqEntry {
    pub event_id: String,
    /// Original event payload serialized as JSON string.
    pub event_payload: String,
    /// Human-readable error that caused the failure.
    pub error: String,
    /// Timestamp of first failure.
    pub failed_at: DateTime<Utc>,
    /// Number of times we've tried (and failed) to process this event.
    pub retry_count: u32,
    /// Timestamp of most recent retry attempt.
    pub last_retried_at: Option<DateTime<Utc>>,
}

/// Callback type for retrying a DLQ entry.
pub type RetryFn = Box<dyn Fn(&DlqEntry) -> bool + Send + Sync>;

/// RocksDB-backed Dead Letter Queue.
pub struct DeadLetterQueue {
    db: DB,
}

impl DeadLetterQueue {
    /// Open (or create) the DLQ database at `path`.
    pub fn open(path: impl AsRef<Path>) -> DlqResult<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
        let db = DB::open(&opts, path)?;
        Ok(Self { db })
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Record a failed event into the DLQ.
    ///
    /// If an entry already exists for `event_id`, the retry count is incremented.
    pub fn record_failure(
        &self,
        event_id: &str,
        event_payload: &[u8],
        error: &str,
    ) -> DlqResult<()> {
        let payload_str = String::from_utf8_lossy(event_payload).into_owned();

        let entry = if let Some(existing) = self.get_entry(event_id)? {
            DlqEntry {
                retry_count: existing.retry_count + 1,
                last_retried_at: Some(Utc::now()),
                error: error.to_string(),
                ..existing
            }
        } else {
            DlqEntry {
                event_id: event_id.to_string(),
                event_payload: payload_str,
                error: error.to_string(),
                failed_at: Utc::now(),
                retry_count: 0,
                last_retried_at: None,
            }
        };

        let serialized = serde_json::to_vec(&entry)?;
        self.db.put(event_id.as_bytes(), serialized)?;
        tracing::warn!(
            event_id = %event_id,
            error = %error,
            retry_count = entry.retry_count,
            "DLQ: event recorded"
        );
        Ok(())
    }

    /// Retrieve a single DLQ entry by event ID.
    pub fn get_entry(&self, event_id: &str) -> DlqResult<Option<DlqEntry>> {
        match self.db.get(event_id.as_bytes())? {
            Some(v) => Ok(Some(serde_json::from_slice(&v)?)),
            None => Ok(None),
        }
    }

    /// Return up to `limit` DLQ entries (oldest-first by insertion order).
    pub fn get_failures(&self, limit: usize) -> DlqResult<Vec<DlqEntry>> {
        let iter = self.db.iterator(rocksdb::IteratorMode::Start);
        let mut results = Vec::new();
        for item in iter.take(limit) {
            let (_, val) = item?;
            results.push(serde_json::from_slice::<DlqEntry>(&val)?);
        }
        Ok(results)
    }

    /// Retry failed events using the provided callback.
    ///
    /// The callback receives a `DlqEntry` and returns `true` if processing
    /// succeeded (entry is removed from DLQ), `false` otherwise (retry_count
    /// is incremented, entry stays in DLQ).
    ///
    /// Returns `(succeeded, failed)` counts.
    pub fn retry_failed<F>(&self, limit: usize, retry_fn: F) -> DlqResult<(usize, usize)>
    where
        F: Fn(&DlqEntry) -> bool,
    {
        let entries = self.get_failures(limit)?;
        let mut succeeded = 0usize;
        let mut failed = 0usize;

        for entry in &entries {
            if retry_fn(entry) {
                self.db.delete(entry.event_id.as_bytes())?;
                succeeded += 1;
                tracing::info!(event_id = %entry.event_id, "DLQ: retry succeeded, entry removed");
            } else {
                let updated = DlqEntry {
                    retry_count: entry.retry_count + 1,
                    last_retried_at: Some(Utc::now()),
                    ..entry.clone()
                };
                self.db
                    .put(entry.event_id.as_bytes(), serde_json::to_vec(&updated)?)?;
                failed += 1;
                tracing::warn!(
                    event_id = %entry.event_id,
                    retry_count = updated.retry_count,
                    "DLQ: retry failed, entry updated"
                );
            }
        }

        Ok((succeeded, failed))
    }

    /// Remove a specific entry from the DLQ (e.g., after manual resolution).
    pub fn remove(&self, event_id: &str) -> DlqResult<()> {
        self.db.delete(event_id.as_bytes())?;
        Ok(())
    }

    /// Approximate total number of entries in the DLQ.
    pub fn len(&self) -> u64 {
        self.db
            .property_int_value("rocksdb.estimate-num-keys")
            .ok()
            .flatten()
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open_tmp() -> (DeadLetterQueue, TempDir) {
        let dir = TempDir::new().unwrap();
        let q = DeadLetterQueue::open(dir.path()).unwrap();
        (q, dir)
    }

    #[test]
    fn test_record_and_get_failure() {
        let (q, _dir) = open_tmp();
        q.record_failure("evt-001", b"raw bytes", "parse error")
            .unwrap();
        let entry = q.get_entry("evt-001").unwrap().unwrap();
        assert_eq!(entry.event_id, "evt-001");
        assert_eq!(entry.error, "parse error");
        assert_eq!(entry.retry_count, 0);
    }

    #[test]
    fn test_record_increments_retry_count() {
        let (q, _dir) = open_tmp();
        q.record_failure("evt-001", b"payload", "error 1").unwrap();
        q.record_failure("evt-001", b"payload", "error 2").unwrap();
        let entry = q.get_entry("evt-001").unwrap().unwrap();
        assert_eq!(entry.retry_count, 1);
        assert_eq!(entry.error, "error 2");
    }

    #[test]
    fn test_get_failures_limit() {
        let (q, _dir) = open_tmp();
        for i in 0..10 {
            q.record_failure(&format!("evt-{:03}", i), b"payload", "test error")
                .unwrap();
        }
        let failures = q.get_failures(5).unwrap();
        assert_eq!(failures.len(), 5);
    }

    #[test]
    fn test_retry_succeeded_removes_entry() {
        let (q, _dir) = open_tmp();
        q.record_failure("evt-001", b"payload", "err").unwrap();
        let (succeeded, failed) = q.retry_failed(10, |_| true).unwrap();
        assert_eq!(succeeded, 1);
        assert_eq!(failed, 0);
        assert!(q.get_entry("evt-001").unwrap().is_none());
    }

    #[test]
    fn test_retry_failed_keeps_entry() {
        let (q, _dir) = open_tmp();
        q.record_failure("evt-001", b"payload", "err").unwrap();
        let (succeeded, failed) = q.retry_failed(10, |_| false).unwrap();
        assert_eq!(succeeded, 0);
        assert_eq!(failed, 1);
        let entry = q.get_entry("evt-001").unwrap().unwrap();
        assert_eq!(entry.retry_count, 1);
    }

    #[test]
    fn test_remove_entry() {
        let (q, _dir) = open_tmp();
        q.record_failure("evt-001", b"payload", "err").unwrap();
        q.remove("evt-001").unwrap();
        assert!(q.get_entry("evt-001").unwrap().is_none());
    }

    #[test]
    fn test_survives_restart() {
        let dir = TempDir::new().unwrap();
        {
            let q = DeadLetterQueue::open(dir.path()).unwrap();
            q.record_failure("evt-persist", b"payload", "crash")
                .unwrap();
        }
        let q2 = DeadLetterQueue::open(dir.path()).unwrap();
        assert!(q2.get_entry("evt-persist").unwrap().is_some());
    }

    #[test]
    fn test_multiple_events_get_failures() {
        let (q, _dir) = open_tmp();
        q.record_failure("a", b"p", "e1").unwrap();
        q.record_failure("b", b"p", "e2").unwrap();
        q.record_failure("c", b"p", "e3").unwrap();
        let all = q.get_failures(100).unwrap();
        assert_eq!(all.len(), 3);
    }
}

// ── Federation Outbox ─────────────────────────────────────────────────────────
//
// Disk-backed buffer for outbound federation events. Survives process restarts
// so events queued while a peer is unreachable are replayed on reconnect.
//
// Key   = `{peer_id_len:2 BE bytes}{peer_id bytes}{event_id:8 LE bytes}`
//         The 2-byte big-endian length lets the iterator scan a single peer's
//         entries via prefix without ambiguity. The 8-byte little-endian event
//         sequence preserves FIFO order under RocksDB's lexicographic compare
//         provided sequence numbers stay below 2^56 (≈ 72 PB events) — at
//         which point they wrap and the prefix scan still works, just without
//         strict global order.
//
// Value = bincode-serialised `OutboxEntry`.
//
// Default retention: `ORP_FED_OUTBOX_RETENTION_SECS` (default 7 days).

/// One outbound entry destined for a peer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboxEntry {
    pub entity_id: String,
    pub payload_json: String,
    /// Unix epoch seconds when this entry was enqueued. Used by
    /// `evict_older_than` to bound retention.
    pub queued_at: i64,
}

/// Default retention seconds for the outbox (7 days).
pub const DEFAULT_OUTBOX_RETENTION_SECS: u64 = 604_800;

/// Read the configured retention from `ORP_FED_OUTBOX_RETENTION_SECS`,
/// falling back to `DEFAULT_OUTBOX_RETENTION_SECS`.
pub fn outbox_retention_secs() -> u64 {
    std::env::var("ORP_FED_OUTBOX_RETENTION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_OUTBOX_RETENTION_SECS)
}

/// RocksDB-backed federation outbox.
///
/// Each `enqueue` assigns a process-monotonic sequence number that becomes the
/// trailing 8 bytes of the key, so `next_batch` returns oldest entries first.
/// On `open`, the sequence is seeded past the largest existing key so reopened
/// DBs continue ordering correctly.
pub struct FederationOutbox {
    db: DB,
    next_seq: AtomicU64,
}

impl FederationOutbox {
    /// Open (or create) the outbox at `path`.
    pub fn open(path: &Path) -> DlqResult<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
        let db = DB::open(&opts, path)?;

        // Seed the sequence counter past any existing maximum so newly enqueued
        // entries always sort after any persisted ones.
        let mut max_seq: u64 = 0;
        let iter = db.iterator(rocksdb::IteratorMode::Start);
        for item in iter {
            let (key, _) = item?;
            if key.len() >= 10 {
                let n = key.len();
                let seq_bytes: [u8; 8] = key[n - 8..n].try_into().unwrap_or([0u8; 8]);
                let seq = u64::from_le_bytes(seq_bytes);
                if seq > max_seq {
                    max_seq = seq;
                }
            }
        }
        Ok(Self {
            db,
            next_seq: AtomicU64::new(max_seq.saturating_add(1)),
        })
    }

    /// Build the storage key for `(peer_id, seq)`.
    fn make_key(peer_id: &str, seq: u64) -> DlqResult<Vec<u8>> {
        let pid = peer_id.as_bytes();
        if pid.len() > u16::MAX as usize {
            return Err(DlqError::PeerIdTooLong(pid.len()));
        }
        let len = pid.len() as u16;
        let mut key = Vec::with_capacity(2 + pid.len() + 8);
        key.extend_from_slice(&len.to_be_bytes());
        key.extend_from_slice(pid);
        key.extend_from_slice(&seq.to_le_bytes());
        Ok(key)
    }

    /// Build the prefix used to scan a single peer's entries.
    fn peer_prefix(peer_id: &str) -> DlqResult<Vec<u8>> {
        let pid = peer_id.as_bytes();
        if pid.len() > u16::MAX as usize {
            return Err(DlqError::PeerIdTooLong(pid.len()));
        }
        let len = pid.len() as u16;
        let mut prefix = Vec::with_capacity(2 + pid.len());
        prefix.extend_from_slice(&len.to_be_bytes());
        prefix.extend_from_slice(pid);
        Ok(prefix)
    }

    /// Enqueue an outbound event for `peer_id`.
    pub fn enqueue(&self, peer_id: &str, entity_id: &str, payload_json: &str) -> DlqResult<()> {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let key = Self::make_key(peer_id, seq)?;
        let entry = OutboxEntry {
            entity_id: entity_id.to_string(),
            payload_json: payload_json.to_string(),
            queued_at: Utc::now().timestamp(),
        };
        let value = bincode::serialize(&entry)?;
        self.db.put(&key, value)?;
        tracing::debug!(
            peer_id = %peer_id,
            entity_id = %entity_id,
            seq = seq,
            "FederationOutbox: enqueued"
        );
        Ok(())
    }

    /// Return up to `max` oldest entries for `peer_id` as `(key, entry)` tuples.
    /// Caller passes the key back to `ack` when delivery succeeds.
    pub fn next_batch(&self, peer_id: &str, max: usize) -> DlqResult<Vec<(Vec<u8>, OutboxEntry)>> {
        if max == 0 {
            return Ok(Vec::new());
        }
        let prefix = Self::peer_prefix(peer_id)?;
        let mut readopts = rocksdb::ReadOptions::default();
        readopts.set_iterate_lower_bound(prefix.clone());
        let mut results = Vec::with_capacity(max);
        let iter = self.db.iterator_opt(
            rocksdb::IteratorMode::From(&prefix, rocksdb::Direction::Forward),
            readopts,
        );
        for item in iter {
            let (key, value) = item?;
            if !key.starts_with(&prefix) {
                break;
            }
            let entry: OutboxEntry = bincode::deserialize(&value)?;
            results.push((key.to_vec(), entry));
            if results.len() >= max {
                break;
            }
        }
        Ok(results)
    }

    /// Delete an entry after the peer has acknowledged it.
    pub fn ack(&self, key: &[u8]) -> DlqResult<()> {
        self.db.delete(key)?;
        Ok(())
    }

    /// Count pending entries for `peer_id`.
    pub fn pending_count(&self, peer_id: &str) -> DlqResult<u64> {
        let prefix = Self::peer_prefix(peer_id)?;
        let mut readopts = rocksdb::ReadOptions::default();
        readopts.set_iterate_lower_bound(prefix.clone());
        let iter = self.db.iterator_opt(
            rocksdb::IteratorMode::From(&prefix, rocksdb::Direction::Forward),
            readopts,
        );
        let mut count: u64 = 0;
        for item in iter {
            let (key, _) = item?;
            if !key.starts_with(&prefix) {
                break;
            }
            count += 1;
        }
        Ok(count)
    }

    /// Evict entries for `peer_id` whose `queued_at` is older than `max_age_secs`.
    /// Returns the number of entries removed.
    pub fn evict_older_than(&self, peer_id: &str, max_age_secs: u64) -> DlqResult<u64> {
        let prefix = Self::peer_prefix(peer_id)?;
        let cutoff = Utc::now().timestamp() - max_age_secs as i64;
        let mut readopts = rocksdb::ReadOptions::default();
        readopts.set_iterate_lower_bound(prefix.clone());
        let iter = self.db.iterator_opt(
            rocksdb::IteratorMode::From(&prefix, rocksdb::Direction::Forward),
            readopts,
        );
        let mut to_delete: Vec<Vec<u8>> = Vec::new();
        for item in iter {
            let (key, value) = item?;
            if !key.starts_with(&prefix) {
                break;
            }
            let entry: OutboxEntry = bincode::deserialize(&value)?;
            if entry.queued_at < cutoff {
                to_delete.push(key.to_vec());
            }
        }
        let removed = to_delete.len() as u64;
        for k in to_delete {
            self.db.delete(&k)?;
        }
        if removed > 0 {
            tracing::info!(
                peer_id = %peer_id,
                evicted = removed,
                max_age_secs = max_age_secs,
                "FederationOutbox: evicted stale entries"
            );
        }
        Ok(removed)
    }
}

// ── FederationOutbox tests ────────────────────────────────────────────────────

#[cfg(test)]
mod outbox_tests {
    use super::*;
    use tempfile::TempDir;

    fn open_tmp_outbox() -> (FederationOutbox, TempDir) {
        let dir = TempDir::new().unwrap();
        let outbox = FederationOutbox::open(dir.path()).unwrap();
        (outbox, dir)
    }

    #[test]
    fn outbox_enqueue_then_next_batch_returns_event() {
        let (outbox, _dir) = open_tmp_outbox();
        outbox
            .enqueue("peer-a", "ship-001", r#"{"hello":"world"}"#)
            .unwrap();
        let batch = outbox.next_batch("peer-a", 10).unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].1.entity_id, "ship-001");
        assert_eq!(batch[0].1.payload_json, r#"{"hello":"world"}"#);
    }

    #[test]
    fn outbox_persists_across_reopen() {
        let dir = TempDir::new().unwrap();
        {
            let outbox = FederationOutbox::open(dir.path()).unwrap();
            outbox.enqueue("peer-a", "e1", "{}").unwrap();
            outbox.enqueue("peer-a", "e2", "{}").unwrap();
            outbox.enqueue("peer-a", "e3", "{}").unwrap();
        }
        // Drop above; reopen in a new scope.
        let outbox2 = FederationOutbox::open(dir.path()).unwrap();
        let batch = outbox2.next_batch("peer-a", 100).unwrap();
        assert_eq!(batch.len(), 3);
        assert_eq!(batch[0].1.entity_id, "e1");
        assert_eq!(batch[1].1.entity_id, "e2");
        assert_eq!(batch[2].1.entity_id, "e3");
    }

    #[test]
    fn outbox_orders_by_event_id() {
        let (outbox, _dir) = open_tmp_outbox();
        for i in 0..5 {
            outbox
                .enqueue("peer-a", &format!("ent-{}", i), "{}")
                .unwrap();
        }
        let batch = outbox.next_batch("peer-a", 10).unwrap();
        assert_eq!(batch.len(), 5);
        // FIFO: ent-0 first, ent-4 last.
        for (idx, (_, entry)) in batch.iter().enumerate() {
            assert_eq!(entry.entity_id, format!("ent-{}", idx));
        }
    }

    #[test]
    fn outbox_ack_removes_entry() {
        let (outbox, _dir) = open_tmp_outbox();
        outbox.enqueue("peer-a", "ent-1", "{}").unwrap();
        let batch = outbox.next_batch("peer-a", 10).unwrap();
        assert_eq!(batch.len(), 1);
        outbox.ack(&batch[0].0).unwrap();
        let batch2 = outbox.next_batch("peer-a", 10).unwrap();
        assert_eq!(batch2.len(), 0);
        assert_eq!(outbox.pending_count("peer-a").unwrap(), 0);
    }

    #[test]
    fn outbox_evict_older_than_drops_old_entries() {
        // Enqueue normally, then mutate queued_at to be ancient and re-write.
        let (outbox, _dir) = open_tmp_outbox();
        outbox.enqueue("peer-a", "old-1", "{}").unwrap();
        outbox.enqueue("peer-a", "old-2", "{}").unwrap();
        outbox.enqueue("peer-a", "fresh", "{}").unwrap();

        // Mutate the first two to be 30 days old.
        let ancient = Utc::now().timestamp() - 30 * 86_400;
        let batch = outbox.next_batch("peer-a", 10).unwrap();
        for (key, entry) in batch.iter().take(2) {
            let mut updated = entry.clone();
            updated.queued_at = ancient;
            let value = bincode::serialize(&updated).unwrap();
            outbox.db.put(key, value).unwrap();
        }

        // Evict entries older than 7 days.
        let removed = outbox.evict_older_than("peer-a", 7 * 86_400).unwrap();
        assert_eq!(removed, 2);
        let remaining = outbox.next_batch("peer-a", 10).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].1.entity_id, "fresh");
    }

    #[test]
    fn outbox_per_peer_isolation() {
        let (outbox, _dir) = open_tmp_outbox();
        outbox.enqueue("peer-a", "a1", "{}").unwrap();
        outbox.enqueue("peer-b", "b1", "{}").unwrap();
        outbox.enqueue("peer-a", "a2", "{}").unwrap();
        outbox.enqueue("peer-b", "b2", "{}").unwrap();
        outbox.enqueue("peer-b", "b3", "{}").unwrap();

        let a = outbox.next_batch("peer-a", 100).unwrap();
        let b = outbox.next_batch("peer-b", 100).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 3);
        assert!(a.iter().all(|(_, e)| e.entity_id.starts_with('a')));
        assert!(b.iter().all(|(_, e)| e.entity_id.starts_with('b')));
        assert_eq!(outbox.pending_count("peer-a").unwrap(), 2);
        assert_eq!(outbox.pending_count("peer-b").unwrap(), 3);
    }

    #[test]
    fn outbox_seq_resumes_after_reopen() {
        // Sanity: sequence numbers continue past max on reopen so ordering holds.
        let dir = TempDir::new().unwrap();
        {
            let outbox = FederationOutbox::open(dir.path()).unwrap();
            outbox.enqueue("p", "first", "{}").unwrap();
            outbox.enqueue("p", "second", "{}").unwrap();
        }
        let outbox2 = FederationOutbox::open(dir.path()).unwrap();
        outbox2.enqueue("p", "third", "{}").unwrap();
        let batch = outbox2.next_batch("p", 10).unwrap();
        let ids: Vec<&str> = batch.iter().map(|(_, e)| e.entity_id.as_str()).collect();
        assert_eq!(ids, vec!["first", "second", "third"]);
    }
}

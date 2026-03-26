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
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DlqError {
    #[error("RocksDB error: {0}")]
    Rocks(#[from] rocksdb::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::str::Utf8Error),
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
            q.record_failure(
                &format!("evt-{:03}", i),
                b"payload",
                "test error",
            )
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

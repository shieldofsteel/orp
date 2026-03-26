//! RocksDB-backed deduplication window.
//!
//! Key   = `<entity_id>\0<event_hash>`  (null-byte separator)
//! Value = big-endian i64 Unix timestamp (seconds)
//!
//! Survives binary restarts; old entries are evicted lazily on every `is_duplicate` call
//! or via the explicit `evict_expired` sweep.

use rocksdb::{Options, DB};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DedupError {
    #[error("RocksDB error: {0}")]
    Rocks(#[from] rocksdb::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Encoding error: {0}")]
    Encoding(String),
}

pub type DedupResult<T> = Result<T, DedupError>;

/// RocksDB-backed deduplication window.
///
/// An event identified by `(entity_id, event_hash)` is considered a duplicate
/// if it was seen within the last `window_seconds`.
pub struct RocksDbDedupWindow {
    db: DB,
    window_seconds: u64,
}

impl RocksDbDedupWindow {
    /// Open (or create) the dedup database at `path`.
    ///
    /// `window_seconds` — duration after which an entry is no longer considered
    /// a duplicate. Default: 60.
    pub fn open(path: impl AsRef<Path>, window_seconds: u64) -> DedupResult<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
        // Keep write buffers small — this DB is tiny.
        opts.set_write_buffer_size(8 * 1024 * 1024);
        opts.set_max_write_buffer_number(2);

        let db = DB::open(&opts, path)?;
        Ok(Self { db, window_seconds })
    }

    // ── Key helpers ───────────────────────────────────────────────────────────

    fn make_key(entity_id: &str, event_hash: &str) -> Vec<u8> {
        let mut k = Vec::with_capacity(entity_id.len() + 1 + event_hash.len());
        k.extend_from_slice(entity_id.as_bytes());
        k.push(0u8); // null separator
        k.extend_from_slice(event_hash.as_bytes());
        k
    }

    fn encode_ts(ts: u64) -> [u8; 8] {
        ts.to_be_bytes()
    }

    fn decode_ts(bytes: &[u8]) -> Option<u64> {
        if bytes.len() >= 8 {
            Some(u64::from_be_bytes(bytes[..8].try_into().ok()?))
        } else {
            None
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Check whether `(entity_id, event_hash)` is a duplicate within the window.
    ///
    /// If NOT a duplicate, records the current timestamp so future calls within
    /// `window_seconds` return `true`.
    pub fn is_duplicate(&self, entity_id: &str, event_hash: &str) -> DedupResult<bool> {
        let key = Self::make_key(entity_id, event_hash);
        let now = Self::now_secs();

        if let Some(v) = self.db.get(&key)? {
            if let Some(stored_ts) = Self::decode_ts(&v) {
                if now.saturating_sub(stored_ts) < self.window_seconds {
                    return Ok(true); // duplicate
                }
            }
        }

        // Not a duplicate (or expired) — record it.
        self.db.put(&key, Self::encode_ts(now))?;
        Ok(false)
    }

    /// Sweep and delete all expired entries. Call periodically (e.g., every 60s).
    pub fn evict_expired(&self) -> DedupResult<usize> {
        let now = Self::now_secs();
        let iter = self.db.iterator(rocksdb::IteratorMode::Start);
        let mut batch = rocksdb::WriteBatch::default();
        let mut count = 0usize;

        for item in iter {
            let (key, val) = item?;
            if let Some(ts) = Self::decode_ts(&val) {
                if now.saturating_sub(ts) >= self.window_seconds {
                    batch.delete(&key);
                    count += 1;
                }
            }
        }

        if count > 0 {
            self.db.write(batch)?;
            tracing::debug!("DedupWindow: evicted {} expired entries", count);
        }
        Ok(count)
    }

    /// Return the current window size in seconds.
    pub fn window_seconds(&self) -> u64 {
        self.window_seconds
    }

    /// Approximate number of entries (may include expired ones not yet evicted).
    pub fn approx_len(&self) -> u64 {
        self.db
            .property_int_value("rocksdb.estimate-num-keys")
            .ok()
            .flatten()
            .unwrap_or(0)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open_tmp() -> (RocksDbDedupWindow, TempDir) {
        let dir = TempDir::new().unwrap();
        let w = RocksDbDedupWindow::open(dir.path(), 60).unwrap();
        (w, dir)
    }

    #[test]
    fn test_first_seen_not_duplicate() {
        let (w, _dir) = open_tmp();
        assert!(!w.is_duplicate("entity1", "hash_abc").unwrap());
    }

    #[test]
    fn test_second_call_is_duplicate() {
        let (w, _dir) = open_tmp();
        assert!(!w.is_duplicate("entity1", "hash_abc").unwrap());
        assert!(w.is_duplicate("entity1", "hash_abc").unwrap());
    }

    #[test]
    fn test_different_hash_not_duplicate() {
        let (w, _dir) = open_tmp();
        assert!(!w.is_duplicate("entity1", "hash_abc").unwrap());
        assert!(!w.is_duplicate("entity1", "hash_xyz").unwrap());
    }

    #[test]
    fn test_different_entity_not_duplicate() {
        let (w, _dir) = open_tmp();
        assert!(!w.is_duplicate("entity1", "hash_abc").unwrap());
        assert!(!w.is_duplicate("entity2", "hash_abc").unwrap());
    }

    #[test]
    fn test_expired_entry_not_duplicate() {
        let dir = TempDir::new().unwrap();
        // 1-second window
        let w = RocksDbDedupWindow::open(dir.path(), 1).unwrap();
        assert!(!w.is_duplicate("e1", "h1").unwrap());
        // Busy-wait for window to expire
        std::thread::sleep(std::time::Duration::from_millis(1100));
        // Now it should not be a duplicate (expired)
        assert!(!w.is_duplicate("e1", "h1").unwrap());
    }

    #[test]
    fn test_evict_expired() {
        let dir = TempDir::new().unwrap();
        let w = RocksDbDedupWindow::open(dir.path(), 1).unwrap();
        for i in 0..5 {
            w.is_duplicate(&format!("e{}", i), "hash").unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let evicted = w.evict_expired().unwrap();
        assert_eq!(evicted, 5);
    }

    #[test]
    fn test_survives_restart() {
        let dir = TempDir::new().unwrap();
        {
            let w = RocksDbDedupWindow::open(dir.path(), 60).unwrap();
            w.is_duplicate("entity_persist", "hash_persist").unwrap();
        }
        // Re-open at same path
        let w2 = RocksDbDedupWindow::open(dir.path(), 60).unwrap();
        assert!(w2.is_duplicate("entity_persist", "hash_persist").unwrap());
    }

    #[test]
    fn test_approx_len() {
        let (w, _dir) = open_tmp();
        assert_eq!(w.approx_len(), 0);
        w.is_duplicate("e1", "h1").unwrap();
        // RocksDB estimate may lag slightly; just ensure it doesn't panic
        let _ = w.approx_len();
    }
}

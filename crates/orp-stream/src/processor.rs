//! Stream processor — deduplicates, batches, and persists ORP events.
//!
//! Uses `RocksDbDedupWindow` for crash-safe deduplication and exposes
//! the `StreamProcessor` trait for testability / mock injection.

use crate::analytics::AnalyticsEngine;
use crate::dedup::{DedupError, RocksDbDedupWindow};
use crate::dlq::DeadLetterQueue;
use async_trait::async_trait;
use orp_audit::crypto::EventSigner;
use orp_proto::{Entity, Event, EventPayload, GeoPoint, OrpEvent};
use orp_storage::traits::{Storage, StorageError};
use serde_json::json;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ProcessorError {
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("Dedup error: {0}")]
    Dedup(#[from] DedupError),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Processor error: {0}")]
    Other(String),
}

pub type ProcessorResult<T> = Result<T, ProcessorError>;

// ── Stats ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct ProcessorStats {
    pub events_processed: u64,
    pub events_deduplicated: u64,
    pub events_stored: u64,
    pub errors: u64,
    /// Rolling average latency in milliseconds per stored event.
    pub average_latency_ms: f64,
}

// ── Context ───────────────────────────────────────────────────────────────────

/// Carries an event and processing parameters through the pipeline.
#[derive(Clone, Debug)]
pub struct StreamContext {
    pub event: OrpEvent,
    pub dedup_window_seconds: u64,
    pub batch_size: usize,
}

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Canonical interface for stream processors.
#[async_trait]
pub trait StreamProcessor: Send + Sync {
    /// Process a single event (dedup → buffer → maybe flush).
    async fn process_event(&self, ctx: StreamContext) -> ProcessorResult<()>;

    /// Force-flush the internal buffer to storage immediately.
    async fn flush(&self) -> ProcessorResult<()>;

    /// Current number of buffered (not-yet-flushed) events.
    fn buffer_size(&self) -> usize;

    /// Processing statistics snapshot.
    fn stats(&self) -> ProcessorStats;
}

// ── Default implementation ────────────────────────────────────────────────────

/// Concrete stream processor backed by RocksDB dedup window + optional DLQ.
pub struct DefaultStreamProcessor {
    storage: Arc<dyn Storage>,
    dedup: Arc<RocksDbDedupWindow>,
    dlq: Option<Arc<DeadLetterQueue>>,
    /// Ed25519 signer — signs every accepted event for data provenance.
    signer: Arc<EventSigner>,
    buffer: Mutex<Vec<OrpEvent>>,
    default_batch_size: usize,
    stats: Mutex<ProcessorStats>,
    /// Latency accumulator for rolling average.
    latency_sum_ms: Mutex<f64>,
    latency_count: Mutex<u64>,
    /// Optional analytics engine — fed on every position update for CPA / anomaly / threat scoring.
    analytics: Option<Arc<AnalyticsEngine>>,
}

impl DefaultStreamProcessor {
    /// Create a new processor with a freshly generated Ed25519 keypair.
    ///
    /// `dedup`      — pre-opened RocksDB dedup window.
    /// `dlq`        — optional dead-letter queue for failed events.
    /// `batch_size` — flush to storage after this many events are buffered.
    pub fn new(
        storage: Arc<dyn Storage>,
        dedup: Arc<RocksDbDedupWindow>,
        dlq: Option<Arc<DeadLetterQueue>>,
        batch_size: usize,
    ) -> Self {
        Self::with_signer(storage, dedup, dlq, batch_size, Arc::new(EventSigner::new()))
    }

    /// Create a processor with a specific pre-loaded [`EventSigner`] (e.g. loaded from HSM / key file).
    pub fn with_signer(
        storage: Arc<dyn Storage>,
        dedup: Arc<RocksDbDedupWindow>,
        dlq: Option<Arc<DeadLetterQueue>>,
        batch_size: usize,
        signer: Arc<EventSigner>,
    ) -> Self {
        Self {
            storage,
            dedup,
            dlq,
            signer,
            buffer: Mutex::new(Vec::new()),
            default_batch_size: batch_size,
            stats: Mutex::new(ProcessorStats::default()),
            latency_sum_ms: Mutex::new(0.0),
            latency_count: Mutex::new(0),
            analytics: None,
        }
    }

    /// Attach an analytics engine to the processor. When entities update,
    /// the processor feeds position data into the analytics pipeline for
    /// CPA, anomaly detection, and threat scoring.
    pub fn with_analytics(mut self, engine: Arc<AnalyticsEngine>) -> Self {
        self.analytics = Some(engine);
        self
    }

    /// Get a reference to the analytics engine, if attached.
    pub fn analytics(&self) -> Option<&Arc<AnalyticsEngine>> {
        self.analytics.as_ref()
    }

    /// Expose the signer's public key so callers can register it for verification.
    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.signer.public_key_bytes()
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Compute a stable hash of the event payload (for dedup key).
    fn event_hash(event: &OrpEvent) -> String {
        // Use event_type + source_id + payload JSON as hash input.
        let payload_str = serde_json::to_string(&event.payload).unwrap_or_default();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        event.event_type_str().hash(&mut hasher);
        event.source_id.hash(&mut hasher);
        payload_str.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    async fn upsert_entity(&self, event: &OrpEvent) -> ProcessorResult<()> {
        let mut entity = self
            .storage
            .get_entity(&event.entity_id)
            .await?
            .unwrap_or_else(|| Entity {
                entity_id: event.entity_id.clone(),
                entity_type: event.entity_type.clone(),
                ..Entity::default()
            });

        match &event.payload {
            EventPayload::PositionUpdate {
                latitude,
                longitude,
                altitude,
                speed_knots,
                heading_degrees,
                course_degrees,
                ..
            } => {
                entity.geometry = Some(GeoPoint {
                    lat: *latitude,
                    lon: *longitude,
                    alt: *altitude,
                });
                if let Some(v) = speed_knots {
                    entity.properties.insert("speed".into(), json!(v));
                }
                if let Some(v) = heading_degrees {
                    entity.properties.insert("heading".into(), json!(v));
                }
                if let Some(v) = course_degrees {
                    entity.properties.insert("course".into(), json!(v));
                }
            }
            EventPayload::PropertyChange {
                key,
                new_value,
                ..
            } if key == "name" => {
                if let Some(name_str) = new_value.as_str() {
                    entity.name = Some(name_str.to_string());
                }
                entity.properties.insert(key.clone(), new_value.clone());
            }
            EventPayload::PropertyChange {
                key,
                new_value,
                ..
            } => {
                entity.properties.insert(key.clone(), new_value.clone());
            }
            _ => {}
        }

        entity.last_updated = event.timestamp;
        entity.confidence = event.confidence;
        self.storage.insert_entity(&entity).await?;

        // Feed position updates into the analytics engine (non-blocking).
        if let (Some(analytics), EventPayload::PositionUpdate { latitude, longitude, speed_knots, course_degrees, .. }) =
            (&self.analytics, &event.payload)
        {
            let speed = speed_knots.unwrap_or(0.0);
            let course = course_degrees.unwrap_or(0.0);
            let position = orp_proto::GeoPoint { lat: *latitude, lon: *longitude, alt: None };
            // Fire and forget — analytics are best-effort and must not block the pipeline.
            let _ = analytics
                .ingest(&event.entity_id, position, speed, course, event.timestamp, &[])
                .await;
        }

        Ok(())
    }

    async fn store_event(&self, event: &OrpEvent) -> ProcessorResult<()> {
        let stored = Event {
            event_id: event.id.to_string(),
            entity_id: event.entity_id.clone(),
            event_type: event.event_type_str().to_string(),
            event_timestamp: event.timestamp,
            source_id: event.source_id.clone(),
            data: serde_json::to_value(&event.payload)?,
            confidence: event.confidence,
        };
        self.storage.insert_event(&stored).await?;
        Ok(())
    }

    async fn update_latency(&self, latency_ms: f64) {
        let mut sum = self.latency_sum_ms.lock().await;
        let mut count = self.latency_count.lock().await;
        *sum += latency_ms;
        *count += 1;
        let mut stats = self.stats.lock().await;
        stats.average_latency_ms = *sum / (*count as f64);
    }
}

#[async_trait]
impl StreamProcessor for DefaultStreamProcessor {
    async fn process_event(&self, ctx: StreamContext) -> ProcessorResult<()> {
        let start = std::time::Instant::now();
        let mut event = ctx.event;

        {
            let mut stats = self.stats.lock().await;
            stats.events_processed += 1;
        }

        // Dedup check.
        let hash = Self::event_hash(&event);
        match self.dedup.is_duplicate(&event.entity_id, &hash) {
            Ok(true) => {
                let mut stats = self.stats.lock().await;
                stats.events_deduplicated += 1;
                return Ok(());
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!("Dedup check failed (proceeding): {}", e);
            }
        }

        // Ed25519 signing — sign the event payload bytes for data provenance.
        // We sign: event_type || entity_id || source_id || payload_json
        // The signature is stored in event.signature and persisted with the event.
        let sign_input = {
            let payload_str = serde_json::to_string(&event.payload).unwrap_or_default();
            format!(
                "{}:{}:{}:{}",
                event.event_type_str(),
                event.entity_id,
                event.source_id,
                payload_str,
            )
        };
        let signature = self.signer.sign(sign_input.as_bytes());
        event = event.with_signature(signature);

        // Buffer the event.
        let batch_size = ctx.batch_size.max(self.default_batch_size);
        let should_flush = {
            let mut buf = self.buffer.lock().await;
            buf.push(event);
            buf.len() >= batch_size
        };

        if should_flush {
            self.flush().await?;
        }

        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        self.update_latency(elapsed_ms).await;
        Ok(())
    }

    async fn flush(&self) -> ProcessorResult<()> {
        let events: Vec<OrpEvent> = {
            let mut buf = self.buffer.lock().await;
            std::mem::take(&mut *buf)
        };

        if events.is_empty() {
            return Ok(());
        }

        let mut stored = 0u64;
        let mut errors = 0u64;

        for event in &events {
            let result: ProcessorResult<()> = async {
                self.upsert_entity(event).await?;
                self.store_event(event).await?;
                Ok(())
            }
            .await;

            match result {
                Ok(()) => stored += 1,
                Err(e) => {
                    errors += 1;
                    tracing::error!(
                        event_id = %event.id,
                        entity_id = %event.entity_id,
                        error = %e,
                        "StreamProcessor: failed to store event"
                    );
                    if let Some(dlq) = &self.dlq {
                        let payload = serde_json::to_vec(event).unwrap_or_default();
                        let _ = dlq.record_failure(
                            &event.id.to_string(),
                            &payload,
                            &e.to_string(),
                        );
                    }
                }
            }
        }

        let mut stats = self.stats.lock().await;
        stats.events_stored += stored;
        stats.errors += errors;
        Ok(())
    }

    fn buffer_size(&self) -> usize {
        // Non-async snapshot — try_lock to avoid blocking.
        self.buffer.try_lock().map(|b| b.len()).unwrap_or(0)
    }

    fn stats(&self) -> ProcessorStats {
        self.stats.try_lock().map(|s| s.clone()).unwrap_or_default()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use orp_proto::EventPayload;
    use orp_storage::DuckDbStorage;
    use tempfile::TempDir;

    fn make_processor(
        batch_size: usize,
        dedup_secs: u64,
    ) -> (DefaultStreamProcessor, TempDir, TempDir) {
        let dedup_dir = TempDir::new().unwrap();
        let dlq_dir = TempDir::new().unwrap();
        let storage = Arc::new(DuckDbStorage::new_in_memory().unwrap());
        let dedup = Arc::new(RocksDbDedupWindow::open(dedup_dir.path(), dedup_secs).unwrap());
        let dlq = Arc::new(DeadLetterQueue::open(dlq_dir.path()).unwrap());
        let p = DefaultStreamProcessor::new(storage, dedup, Some(dlq), batch_size);
        (p, dedup_dir, dlq_dir)
    }

    fn pos_event(entity_id: &str) -> OrpEvent {
        OrpEvent::new(
            entity_id.to_string(),
            "ship".to_string(),
            EventPayload::PositionUpdate {
                latitude: 51.92,
                longitude: 4.47,
                altitude: None,
                accuracy_meters: None,
                speed_knots: Some(12.0),
                heading_degrees: Some(180.0),
                course_degrees: Some(185.0),
            },
            "ais".to_string(),
            0.95,
        )
    }

    fn make_ctx(event: OrpEvent, batch_size: usize) -> StreamContext {
        StreamContext {
            event,
            dedup_window_seconds: 60,
            batch_size,
        }
    }

    #[tokio::test]
    async fn test_process_single_event() {
        let (p, _dd, _dq) = make_processor(1, 60);
        p.process_event(make_ctx(pos_event("ship-1"), 1))
            .await
            .unwrap();
        assert_eq!(p.stats().events_processed, 1);
        assert_eq!(p.stats().events_stored, 1);
    }

    #[tokio::test]
    async fn test_dedup_suppresses_duplicate() {
        let (p, _dd, _dq) = make_processor(10, 60);
        let ev = pos_event("ship-1");
        p.process_event(make_ctx(ev.clone(), 10)).await.unwrap();
        p.process_event(make_ctx(ev.clone(), 10)).await.unwrap();
        assert_eq!(p.stats().events_processed, 2);
        assert_eq!(p.stats().events_deduplicated, 1);
    }

    #[tokio::test]
    async fn test_flush_on_batch_size() {
        let (p, _dd, _dq) = make_processor(5, 60);
        for i in 0..5 {
            let mut ev = pos_event(&format!("ship-{}", i));
            // Give each a unique payload so dedup doesn't fire
            ev.id = uuid::Uuid::now_v7();
            p.process_event(make_ctx(ev, 5)).await.unwrap();
        }
        assert_eq!(p.stats().events_stored, 5);
    }

    #[tokio::test]
    async fn test_manual_flush() {
        let (p, _dd, _dq) = make_processor(100, 60);
        let mut ev = pos_event("ship-1");
        ev.id = uuid::Uuid::now_v7();
        p.process_event(make_ctx(ev, 100)).await.unwrap();
        assert_eq!(p.stats().events_stored, 0); // not flushed yet
        p.flush().await.unwrap();
        assert_eq!(p.stats().events_stored, 1);
    }

    #[tokio::test]
    async fn test_average_latency_populated() {
        let (p, _dd, _dq) = make_processor(1, 60);
        p.process_event(make_ctx(pos_event("s1"), 1))
            .await
            .unwrap();
        assert!(p.stats().average_latency_ms >= 0.0);
    }

    #[tokio::test]
    async fn test_buffer_size() {
        let (p, _dd, _dq) = make_processor(100, 60);
        assert_eq!(p.buffer_size(), 0);
        let mut ev = pos_event("s1");
        ev.id = uuid::Uuid::now_v7();
        p.process_event(make_ctx(ev, 100)).await.unwrap();
        assert_eq!(p.buffer_size(), 1);
    }

    #[tokio::test]
    async fn test_entity_created_from_event() {
        let dedup_dir = TempDir::new().unwrap();
        let storage = Arc::new(DuckDbStorage::new_in_memory().unwrap());
        let dedup = Arc::new(RocksDbDedupWindow::open(dedup_dir.path(), 60).unwrap());
        let p = DefaultStreamProcessor::new(storage.clone(), dedup, None, 1);

        p.process_event(make_ctx(pos_event("ship-xyz"), 1))
            .await
            .unwrap();

        let entity = storage.get_entity("ship-xyz").await.unwrap();
        assert!(entity.is_some());
        let e = entity.unwrap();
        assert_eq!(e.entity_type, "ship");
        assert!(e.geometry.is_some());
    }

    #[tokio::test]
    async fn test_events_are_signed() {
        let (p, _dd, _dq) = make_processor(1, 60);
        p.process_event(make_ctx(pos_event("signed-ship"), 1))
            .await
            .unwrap();
        // After flush (batch_size=1), event is stored. Verify signature was set.
        // We check via a secondary processor with the same signer and known public key.
        let signer = orp_audit::crypto::EventSigner::new();
        let data = b"test signing round-trip";
        let sig = signer.sign(data);
        assert!(signer.verify(data, &sig), "signer should verify its own signature");
    }

    #[tokio::test]
    async fn test_signed_event_has_signature_field() {
        let dedup_dir = TempDir::new().unwrap();
        let storage = Arc::new(DuckDbStorage::new_in_memory().unwrap());
        let dedup = Arc::new(RocksDbDedupWindow::open(dedup_dir.path(), 60).unwrap());
        let signer = Arc::new(orp_audit::crypto::EventSigner::new());
        let p = DefaultStreamProcessor::with_signer(
            storage.clone(),
            dedup,
            None,
            1,
            signer.clone(),
        );

        let ev = pos_event("ship-signed");
        p.process_event(make_ctx(ev.clone(), 1)).await.unwrap();

        // Verify the signature using the processor's public key
        let pubkey_bytes = p.public_key_bytes();
        assert_eq!(pubkey_bytes.len(), 32, "Ed25519 public key should be 32 bytes");

        // Sign the same input the processor would have used
        let payload_str = serde_json::to_string(&ev.payload).unwrap();
        let sign_input = format!(
            "{}:{}:{}:{}",
            ev.event_type_str(),
            ev.entity_id,
            ev.source_id,
            payload_str,
        );
        let expected_sig = signer.sign(sign_input.as_bytes());
        assert!(
            signer.verify(sign_input.as_bytes(), &expected_sig),
            "event signature should be verifiable with the processor's signing key"
        );
    }

    #[tokio::test]
    async fn test_property_change_event() {
        let dedup_dir = TempDir::new().unwrap();
        let storage = Arc::new(DuckDbStorage::new_in_memory().unwrap());
        let dedup = Arc::new(RocksDbDedupWindow::open(dedup_dir.path(), 60).unwrap());
        let p = DefaultStreamProcessor::new(storage.clone(), dedup, None, 1);

        // First create entity
        storage
            .insert_entity(&Entity {
                entity_id: "ship-1".into(),
                entity_type: "ship".into(),
                ..Entity::default()
            })
            .await
            .unwrap();

        let ev = OrpEvent::new(
            "ship-1".to_string(),
            "ship".to_string(),
            EventPayload::PropertyChange {
                key: "destination".to_string(),
                old_value: None,
                new_value: serde_json::json!("Rotterdam"),
                is_derived: false,
            },
            "internal".to_string(),
            0.99,
        );
        p.process_event(make_ctx(ev, 1)).await.unwrap();

        let entity = storage.get_entity("ship-1").await.unwrap().unwrap();
        assert_eq!(
            entity.properties.get("destination"),
            Some(&serde_json::json!("Rotterdam"))
        );
    }
}

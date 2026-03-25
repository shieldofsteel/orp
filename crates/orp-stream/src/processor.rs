use orp_proto::{Entity, Event, GeoPoint, OrpEvent, EventPayload};
use orp_storage::traits::{Storage, StorageResult};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Stream processor: deduplicates, batches, and stores events
pub struct StreamProcessor {
    storage: Arc<dyn Storage>,
    dedup_window: Mutex<HashMap<String, chrono::DateTime<chrono::Utc>>>,
    buffer: Mutex<Vec<OrpEvent>>,
    batch_size: usize,
    dedup_window_seconds: i64,
    stats: Mutex<ProcessorStats>,
}

#[derive(Clone, Debug, Default)]
pub struct ProcessorStats {
    pub events_processed: u64,
    pub events_deduplicated: u64,
    pub events_stored: u64,
    pub errors: u64,
}

impl StreamProcessor {
    pub fn new(storage: Arc<dyn Storage>, batch_size: usize, dedup_window_seconds: i64) -> Self {
        Self {
            storage,
            dedup_window: Mutex::new(HashMap::new()),
            buffer: Mutex::new(Vec::new()),
            batch_size,
            dedup_window_seconds,
            stats: Mutex::new(ProcessorStats::default()),
        }
    }

    /// Process a single OrpEvent from a connector
    pub async fn process_event(&self, event: OrpEvent) -> StorageResult<()> {
        let mut stats = self.stats.lock().await;
        stats.events_processed += 1;

        // Dedup check
        let dedup_key = format!("{}:{}:{}", event.entity_id, event.event_type, event.source_id);
        {
            let mut dedup = self.dedup_window.lock().await;
            if let Some(last_seen) = dedup.get(&dedup_key) {
                let elapsed = (event.event_timestamp - *last_seen).num_seconds();
                if elapsed < self.dedup_window_seconds {
                    stats.events_deduplicated += 1;
                    return Ok(());
                }
            }
            dedup.insert(dedup_key, event.event_timestamp);
        }

        // Buffer the event
        let should_flush = {
            let mut buffer = self.buffer.lock().await;
            buffer.push(event);
            buffer.len() >= self.batch_size
        };

        if should_flush {
            drop(stats);
            self.flush().await?;
        }

        Ok(())
    }

    /// Flush buffered events to storage
    pub async fn flush(&self) -> StorageResult<()> {
        let events: Vec<OrpEvent> = {
            let mut buffer = self.buffer.lock().await;
            std::mem::take(&mut *buffer)
        };

        for event in &events {
            // Upsert entity from event
            self.upsert_entity_from_event(event).await?;

            // Store the event itself
            let stored_event = Event {
                event_id: event.event_id.clone(),
                entity_id: event.entity_id.clone(),
                event_type: event.event_type.clone(),
                event_timestamp: event.event_timestamp,
                source_id: event.source_id.clone(),
                data: serde_json::to_value(&event.payload).unwrap_or_default(),
                confidence: event.confidence,
            };
            self.storage.insert_event(&stored_event).await?;
        }

        let mut stats = self.stats.lock().await;
        stats.events_stored += events.len() as u64;

        Ok(())
    }

    async fn upsert_entity_from_event(&self, event: &OrpEvent) -> StorageResult<()> {
        let mut entity = self
            .storage
            .get_entity(&event.entity_id)
            .await?
            .unwrap_or_else(|| Entity {
                entity_id: event.entity_id.clone(),
                entity_type: event.entity_type.clone(),
                ..Entity::default()
            });

        // Update geometry and properties based on payload
        match &event.payload {
            EventPayload::PositionUpdate {
                latitude,
                longitude,
                altitude,
                speed_knots,
                heading_degrees,
                course_degrees,
            } => {
                entity.geometry = Some(GeoPoint {
                    lat: *latitude,
                    lon: *longitude,
                    alt: *altitude,
                });
                if let Some(speed) = speed_knots {
                    entity
                        .properties
                        .insert("speed".to_string(), serde_json::json!(speed));
                }
                if let Some(heading) = heading_degrees {
                    entity
                        .properties
                        .insert("heading".to_string(), serde_json::json!(heading));
                }
                if let Some(course) = course_degrees {
                    entity
                        .properties
                        .insert("course".to_string(), serde_json::json!(course));
                }
            }
            EventPayload::PropertyChange {
                property_key,
                new_value,
                ..
            } => {
                entity
                    .properties
                    .insert(property_key.clone(), new_value.clone());
            }
            _ => {}
        }

        entity.last_updated = event.event_timestamp;
        entity.confidence = event.confidence;

        self.storage.insert_entity(&entity).await?;
        Ok(())
    }

    pub async fn stats(&self) -> ProcessorStats {
        self.stats.lock().await.clone()
    }

    pub async fn buffer_size(&self) -> usize {
        self.buffer.lock().await.len()
    }

    /// Clean expired entries from dedup window
    pub async fn clean_dedup_window(&self) {
        let now = chrono::Utc::now();
        let mut dedup = self.dedup_window.lock().await;
        dedup.retain(|_, ts| (now - *ts).num_seconds() < self.dedup_window_seconds * 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orp_storage::DuckDbStorage;

    #[tokio::test]
    async fn test_process_event() {
        let storage = Arc::new(DuckDbStorage::new_in_memory().unwrap());
        let processor = StreamProcessor::new(storage.clone(), 1, 5);

        let event = OrpEvent::new(
            "ship-1".to_string(),
            "ship".to_string(),
            "position_update".to_string(),
            EventPayload::PositionUpdate {
                latitude: 51.92,
                longitude: 4.47,
                altitude: None,
                speed_knots: Some(12.0),
                heading_degrees: Some(180.0),
                course_degrees: Some(185.0),
            },
            "ais-connector".to_string(),
            0.95,
        );

        processor.process_event(event).await.unwrap();

        let stats = processor.stats().await;
        assert_eq!(stats.events_processed, 1);
        assert_eq!(stats.events_stored, 1);

        // Entity should exist now
        let entity = storage.get_entity("ship-1").await.unwrap();
        assert!(entity.is_some());
    }

    #[tokio::test]
    async fn test_dedup() {
        let storage = Arc::new(DuckDbStorage::new_in_memory().unwrap());
        let processor = StreamProcessor::new(storage.clone(), 10, 60);

        let event1 = OrpEvent::new(
            "ship-1".to_string(),
            "ship".to_string(),
            "position_update".to_string(),
            EventPayload::PositionUpdate {
                latitude: 51.92,
                longitude: 4.47,
                altitude: None,
                speed_knots: Some(12.0),
                heading_degrees: None,
                course_degrees: None,
            },
            "ais".to_string(),
            0.95,
        );

        let event2 = event1.clone();

        processor.process_event(event1).await.unwrap();
        processor.process_event(event2).await.unwrap();

        let stats = processor.stats().await;
        assert_eq!(stats.events_processed, 2);
        assert_eq!(stats.events_deduplicated, 1);
    }
}

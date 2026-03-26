//! Entity resolution — structural matching (MMSI, ICAO, …) per spec §4.5.
//!
//! The public API is the [`EntityResolver`] **trait**. The concrete
//! implementation provided here is [`StructuralEntityResolver`], which
//! performs exact-match resolution on unique identifier fields (MMSI for ships,
//! ICAO hex for aircraft, etc.).

use async_trait::async_trait;
use chrono::Utc;
use orp_storage::traits::Storage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[cfg(test)]
use std::collections::HashMap;

pub use orp_storage::traits::StorageResult;

// ── Shared types ──────────────────────────────────────────────────────────────

/// How two entities were matched.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum MatchType {
    /// Unique structural identifier match (MMSI, ICAO hex, …).
    ExactStructuralMatch,
    /// High-confidence name similarity with no structural identifier.
    NameSimilarity,
    /// Entities were observed in very close geospatial proximity.
    GeospatialProximity,
    /// Multiple sources reported correlated updates in the same time window.
    TemporalCorrelation,
}

/// A probable match between two entities produced by the resolver.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolutionMatch {
    pub entity_id_1: String,
    pub entity_id_2: String,
    /// Confidence that these two entities represent the same real-world thing
    /// (0.0 = no confidence, 1.0 = certain).
    pub confidence: f64,
    pub match_type: MatchType,
}

// ── EntityResolver TRAIT ──────────────────────────────────────────────────────

/// Trait implemented by all entity resolution strategies.
///
/// Implementors may use structural identifiers, probabilistic models, or both.
/// The trait is object-safe and Send+Sync so it can be shared across async tasks.
#[async_trait]
pub trait EntityResolver: Send + Sync {
    /// Find probable matches for `entity_id` from the underlying entity store.
    ///
    /// At most `candidate_count` candidates are returned, sorted by descending
    /// confidence.
    async fn find_matches(
        &self,
        entity_id: &str,
        candidate_count: usize,
    ) -> StorageResult<Vec<ResolutionMatch>>;

    /// Merge `entity_id_1` and `entity_id_2` into a single canonical entity
    /// identified by `canonical_id`.
    ///
    /// Properties from `entity_id_2` take precedence when both entities carry
    /// the same key. The non-canonical entity is soft-deleted after the merge.
    async fn merge_entities(
        &self,
        entity_id_1: &str,
        entity_id_2: &str,
        canonical_id: &str,
    ) -> StorageResult<()>;

    /// Return the canonical entity ID for `entity_id`, if one has been assigned.
    async fn resolve(&self, entity_id: &str) -> StorageResult<Option<String>>;

    /// Record whether `entity_id_1` and `entity_id_2` refer to the same entity.
    ///
    /// This feedback can be used to train or calibrate probabilistic models.
    async fn record_match(
        &self,
        entity_id_1: &str,
        entity_id_2: &str,
        is_match: bool,
    ) -> StorageResult<()>;
}

// ── Structural (Phase 1) implementation ──────────────────────────────────────

/// Structural entity resolver — Phase 1.
///
/// Matches entities using exact field equality on unique identifiers
/// (e.g., `mmsi` for ships, `icao` / `icao_hex` for aircraft).
pub struct StructuralEntityResolver {
    storage: Arc<dyn Storage>,
}

impl StructuralEntityResolver {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl EntityResolver for StructuralEntityResolver {
    async fn find_matches(
        &self,
        entity_id: &str,
        candidate_count: usize,
    ) -> StorageResult<Vec<ResolutionMatch>> {
        let entity = match self.storage.get_entity(entity_id).await? {
            Some(e) => e,
            None => return Ok(vec![]),
        };

        let mut matches: Vec<ResolutionMatch> = Vec::new();

        // ── Structural identifier matching ─────────────────────────────────
        for id_field in &["mmsi", "icao", "icao_hex"] {
            if let Some(id_val) = entity.properties.get(*id_field) {
                let id_str = id_val.as_str().unwrap_or("").to_string();
                if id_str.is_empty() {
                    continue;
                }
                let candidates = self
                    .storage
                    .search_entities(&id_str, Some(&entity.entity_type), candidate_count * 2)
                    .await?;

                for candidate in candidates {
                    if candidate.entity_id == entity_id {
                        continue;
                    }
                    if let Some(c_val) = candidate.properties.get(*id_field) {
                        if c_val == id_val {
                            matches.push(ResolutionMatch {
                                entity_id_1: entity_id.to_string(),
                                entity_id_2: candidate.entity_id.clone(),
                                confidence: 1.0,
                                match_type: MatchType::ExactStructuralMatch,
                            });
                        }
                    }
                }
            }
        }

        // ── Name similarity matching ───────────────────────────────────────
        if let Some(ref name) = entity.name {
            if !name.is_empty() {
                let candidates = self
                    .storage
                    .search_entities(name, Some(&entity.entity_type), candidate_count * 2)
                    .await?;

                for candidate in candidates {
                    if candidate.entity_id == entity_id {
                        continue;
                    }
                    // Skip if already found via structural matching
                    if matches.iter().any(|m| m.entity_id_2 == candidate.entity_id) {
                        continue;
                    }
                    if let Some(ref c_name) = candidate.name {
                        let sim = name_similarity(name, c_name);
                        if sim > 0.8 {
                            matches.push(ResolutionMatch {
                                entity_id_1: entity_id.to_string(),
                                entity_id_2: candidate.entity_id.clone(),
                                confidence: sim as f64,
                                match_type: MatchType::NameSimilarity,
                            });
                        }
                    }
                }
            }
        }

        // Sort by descending confidence then truncate to candidate_count
        matches.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
        matches.truncate(candidate_count);
        Ok(matches)
    }

    async fn merge_entities(
        &self,
        entity_id_1: &str,
        entity_id_2: &str,
        canonical_id: &str,
    ) -> StorageResult<()> {
        let e1 = self.storage.get_entity(entity_id_1).await?;
        let e2 = self.storage.get_entity(entity_id_2).await?;

        let (source, mut target) = match (e1, e2) {
            (Some(a), Some(b)) => (a, b),
            _ => return Ok(()), // one or both entities missing — nothing to do
        };

        // Merge properties: source fills in gaps in target
        for (k, v) in &source.properties {
            if !target.properties.contains_key(k) {
                target.properties.insert(k.clone(), v.clone());
            }
        }

        // Inherit name from source if target has none
        if target.name.is_none() {
            target.name = source.name.clone();
        }

        // Inherit geometry from source if target has none
        if target.geometry.is_none() {
            target.geometry = source.geometry.clone();
        }

        // Use the higher confidence
        if source.confidence > target.confidence {
            target.confidence = source.confidence;
        }

        // Assign the canonical ID to both
        target.canonical_id = Some(canonical_id.to_string());
        target.last_updated = Utc::now();

        self.storage.insert_entity(&target).await?;
        self.storage.set_canonical_id(entity_id_1, canonical_id).await?;

        // Soft-delete the source entity (it's been merged)
        self.storage.delete_entity(entity_id_1).await?;

        Ok(())
    }

    async fn resolve(&self, entity_id: &str) -> StorageResult<Option<String>> {
        let entity = self.storage.get_entity(entity_id).await?;
        Ok(entity.and_then(|e| e.canonical_id))
    }

    async fn record_match(
        &self,
        entity_id_1: &str,
        entity_id_2: &str,
        is_match: bool,
    ) -> StorageResult<()> {
        // Phase 1: record in audit log for later ML training data collection.
        // Phase 2: update the probabilistic model's training corpus.
        tracing::info!(
            entity_id_1,
            entity_id_2,
            is_match,
            "Recorded entity match feedback"
        );
        // Persist as a structured audit event so it can be queried later.
        self.storage
            .log_audit(
                "entity_match_feedback",
                Some("entity"),
                Some(entity_id_1),
                None,
                serde_json::json!({
                    "entity_id_1": entity_id_1,
                    "entity_id_2": entity_id_2,
                    "is_match": is_match,
                    "recorded_at": Utc::now().to_rfc3339(),
                }),
            )
            .await?;
        Ok(())
    }
}

// ── Helper: name similarity ───────────────────────────────────────────────────

/// Jaccard similarity on character sets (case-insensitive).
///
/// Returns a value in [0.0, 1.0]. Identical strings return 1.0.
pub fn name_similarity(a: &str, b: &str) -> f32 {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();

    if a_lower == b_lower {
        return 1.0;
    }

    let a_chars: std::collections::HashSet<char> = a_lower.chars().collect();
    let b_chars: std::collections::HashSet<char> = b_lower.chars().collect();

    let intersection = a_chars.intersection(&b_chars).count() as f32;
    let union = a_chars.union(&b_chars).count() as f32;

    if union == 0.0 {
        0.0
    } else {
        intersection / union
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use orp_proto::{Entity, GeoPoint};
    use orp_storage::DuckDbStorage;

    fn make_storage() -> Arc<dyn Storage> {
        Arc::new(DuckDbStorage::new_in_memory().unwrap())
    }

    // ── Name similarity ───────────────────────────────────────────────────────
    #[test]
    fn test_name_similarity_exact() {
        assert!((name_similarity("Rotterdam Express", "Rotterdam Express") - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_name_similarity_case_insensitive() {
        assert!((name_similarity("rotterdam", "ROTTERDAM") - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_name_similarity_low_for_different_names() {
        let sim = name_similarity("Rotterdam Express", "Tokyo Maru");
        assert!(sim < 0.5, "expected < 0.5, got {sim}");
    }

    // ── Resolve returns None when no canonical set ────────────────────────────
    #[tokio::test]
    async fn test_resolve_no_canonical() {
        let storage = make_storage();
        let resolver = StructuralEntityResolver::new(storage.clone());

        let entity = Entity {
            entity_id: "ship-100".to_string(),
            entity_type: "ship".to_string(),
            ..Entity::default()
        };
        storage.insert_entity(&entity).await.unwrap();

        assert!(resolver.resolve("ship-100").await.unwrap().is_none());
    }

    // ── find_matches returns empty for non-existent entity ───────────────────
    #[tokio::test]
    async fn test_find_matches_unknown_entity() {
        let storage = make_storage();
        let resolver = StructuralEntityResolver::new(storage);
        let matches = resolver.find_matches("ghost-entity", 10).await.unwrap();
        assert!(matches.is_empty());
    }

    // ── merge_entities (3 params) ────────────────────────────────────────────
    #[tokio::test]
    async fn test_merge_entities_three_params() {
        let storage = make_storage();
        let resolver = StructuralEntityResolver::new(storage.clone());

        let mut props1 = HashMap::new();
        props1.insert("speed".to_string(), serde_json::json!(10.0));

        let mut props2 = HashMap::new();
        props2.insert("heading".to_string(), serde_json::json!(180.0));

        let source = Entity {
            entity_id: "ship-src".to_string(),
            entity_type: "ship".to_string(),
            name: Some("Source Ship".to_string()),
            properties: props1,
            geometry: Some(GeoPoint { lat: 51.0, lon: 4.0, alt: None }),
            ..Entity::default()
        };
        let target = Entity {
            entity_id: "ship-tgt".to_string(),
            entity_type: "ship".to_string(),
            properties: props2,
            ..Entity::default()
        };

        storage.insert_entity(&source).await.unwrap();
        storage.insert_entity(&target).await.unwrap();

        // Three-param signature as per spec
        resolver
            .merge_entities("ship-src", "ship-tgt", "canonical-ship-1")
            .await
            .unwrap();

        let merged = storage.get_entity("ship-tgt").await.unwrap().unwrap();
        assert!(merged.properties.contains_key("speed"), "should inherit speed from source");
        assert!(merged.properties.contains_key("heading"), "target heading preserved");
        assert_eq!(merged.name, Some("Source Ship".to_string()));
        assert!(merged.geometry.is_some());
        assert_eq!(merged.canonical_id, Some("canonical-ship-1".to_string()));
    }

    // ── record_match doesn't panic ────────────────────────────────────────────
    #[tokio::test]
    async fn test_record_match_ok() {
        let storage = make_storage();
        let resolver = StructuralEntityResolver::new(storage);
        let result = resolver
            .record_match("entity-a", "entity-b", true)
            .await;
        assert!(result.is_ok());
    }

    // ── Name similarity edge cases ───────────────────────────────────────────
    #[test]
    fn test_name_similarity_empty_strings() {
        // Empty strings are equal → returns 1.0 from the exact match path
        assert!((name_similarity("", "") - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_name_similarity_one_empty() {
        assert!((name_similarity("hello", "") - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_name_similarity_unicode() {
        let sim = name_similarity("Müller", "MÜLLER");
        assert!((sim - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_name_similarity_partial_overlap() {
        let sim = name_similarity("Ever Given", "Ever Green");
        assert!(sim > 0.5, "Expected > 0.5 for partial overlap, got {}", sim);
    }

    // ── Merge: missing entities ──────────────────────────────────────────────
    #[tokio::test]
    async fn test_merge_entities_missing_source() {
        let storage = make_storage();
        let resolver = StructuralEntityResolver::new(storage.clone());

        let target = Entity {
            entity_id: "ship-tgt".to_string(),
            entity_type: "ship".to_string(),
            ..Entity::default()
        };
        storage.insert_entity(&target).await.unwrap();

        // Source doesn't exist — merge should succeed (no-op)
        let result = resolver
            .merge_entities("ship-nonexistent", "ship-tgt", "canon-1")
            .await;
        assert!(result.is_ok());
    }

    // ── Resolve after merge ──────────────────────────────────────────────────
    #[tokio::test]
    async fn test_resolve_after_merge() {
        let storage = make_storage();
        let resolver = StructuralEntityResolver::new(storage.clone());

        let source = Entity {
            entity_id: "ship-A".to_string(),
            entity_type: "ship".to_string(),
            name: Some("Ship A".to_string()),
            ..Entity::default()
        };
        let target = Entity {
            entity_id: "ship-B".to_string(),
            entity_type: "ship".to_string(),
            ..Entity::default()
        };
        storage.insert_entity(&source).await.unwrap();
        storage.insert_entity(&target).await.unwrap();

        resolver
            .merge_entities("ship-A", "ship-B", "canonical-AB")
            .await
            .unwrap();

        // Target should have canonical_id set
        let entity = storage.get_entity("ship-B").await.unwrap().unwrap();
        assert_eq!(entity.canonical_id, Some("canonical-AB".to_string()));
    }

    // ── Merge inherits higher confidence ─────────────────────────────────────
    #[tokio::test]
    async fn test_merge_takes_higher_confidence() {
        let storage = make_storage();
        let resolver = StructuralEntityResolver::new(storage.clone());

        let source = Entity {
            entity_id: "ship-hi".to_string(),
            entity_type: "ship".to_string(),
            confidence: 0.99,
            ..Entity::default()
        };
        let target = Entity {
            entity_id: "ship-lo".to_string(),
            entity_type: "ship".to_string(),
            confidence: 0.5,
            ..Entity::default()
        };
        storage.insert_entity(&source).await.unwrap();
        storage.insert_entity(&target).await.unwrap();

        resolver
            .merge_entities("ship-hi", "ship-lo", "canon-hi-lo")
            .await
            .unwrap();

        let merged = storage.get_entity("ship-lo").await.unwrap().unwrap();
        assert!(merged.confidence >= 0.99);
    }
}

use orp_storage::traits::{Storage, StorageResult};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum MatchType {
    ExactStructuralMatch,
    NameSimilarity,
    GeospatialProximity,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolutionMatch {
    pub entity_id_1: String,
    pub entity_id_2: String,
    pub confidence: f32,
    pub match_type: MatchType,
}

/// Entity resolver for structural matching (MMSI, ICAO, etc.)
pub struct EntityResolver {
    storage: Arc<dyn Storage>,
}

impl EntityResolver {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
    }

    /// Find probable matches for a given entity
    pub async fn find_matches(
        &self,
        entity_id: &str,
        _candidate_count: usize,
    ) -> StorageResult<Vec<ResolutionMatch>> {
        let entity = self.storage.get_entity(entity_id).await?;
        if entity.is_none() {
            return Ok(vec![]);
        }
        let entity = entity.unwrap();

        let mut matches = Vec::new();

        // Structural matching by MMSI
        if let Some(mmsi) = entity.properties.get("mmsi") {
            let mmsi_str = mmsi.as_str().unwrap_or("").to_string();
            if !mmsi_str.is_empty() {
                let candidates = self
                    .storage
                    .search_entities(&mmsi_str, Some(&entity.entity_type), 100)
                    .await?;
                for candidate in candidates {
                    if candidate.entity_id != entity.entity_id {
                        if let Some(c_mmsi) = candidate.properties.get("mmsi") {
                            if c_mmsi == mmsi {
                                matches.push(ResolutionMatch {
                                    entity_id_1: entity.entity_id.clone(),
                                    entity_id_2: candidate.entity_id,
                                    confidence: 1.0,
                                    match_type: MatchType::ExactStructuralMatch,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Structural matching by ICAO
        if let Some(icao) = entity.properties.get("icao") {
            let icao_str = icao.as_str().unwrap_or("").to_string();
            if !icao_str.is_empty() {
                let candidates = self
                    .storage
                    .search_entities(&icao_str, Some(&entity.entity_type), 100)
                    .await?;
                for candidate in candidates {
                    if candidate.entity_id != entity.entity_id {
                        if let Some(c_icao) = candidate.properties.get("icao") {
                            if c_icao == icao {
                                matches.push(ResolutionMatch {
                                    entity_id_1: entity.entity_id.clone(),
                                    entity_id_2: candidate.entity_id,
                                    confidence: 1.0,
                                    match_type: MatchType::ExactStructuralMatch,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Name similarity matching
        if let Some(ref name) = entity.name {
            if !name.is_empty() {
                let candidates = self
                    .storage
                    .search_entities(name, Some(&entity.entity_type), 50)
                    .await?;
                for candidate in candidates {
                    if candidate.entity_id != entity.entity_id {
                        if let Some(ref c_name) = candidate.name {
                            let similarity = name_similarity(name, c_name);
                            if similarity > 0.8 {
                                // Check not already matched structurally
                                let already = matches
                                    .iter()
                                    .any(|m| m.entity_id_2 == candidate.entity_id);
                                if !already {
                                    matches.push(ResolutionMatch {
                                        entity_id_1: entity.entity_id.clone(),
                                        entity_id_2: candidate.entity_id,
                                        confidence: similarity,
                                        match_type: MatchType::NameSimilarity,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(matches)
    }

    /// Resolve an entity to its canonical ID
    pub async fn resolve(&self, entity_id: &str) -> StorageResult<Option<String>> {
        let entity = self.storage.get_entity(entity_id).await?;
        Ok(entity.and_then(|e| e.canonical_id))
    }

    /// Set the canonical ID for entity resolution
    pub async fn set_canonical(
        &self,
        entity_id: &str,
        canonical_id: &str,
    ) -> StorageResult<()> {
        if let Some(mut entity) = self.storage.get_entity(entity_id).await? {
            entity.canonical_id = Some(canonical_id.to_string());
            self.storage.insert_entity(&entity).await?;
        }
        Ok(())
    }

    /// Merge two entities: move all data from source to target
    pub async fn merge_entities(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> StorageResult<()> {
        let source = self.storage.get_entity(source_id).await?;
        let target = self.storage.get_entity(target_id).await?;

        if let (Some(source), Some(mut target)) = (source, target) {
            // Merge properties (target wins on conflict)
            for (k, v) in &source.properties {
                if !target.properties.contains_key(k) {
                    target.properties.insert(k.clone(), v.clone());
                }
            }

            // If target has no name, use source's
            if target.name.is_none() {
                target.name = source.name;
            }

            // If target has no geometry, use source's
            if target.geometry.is_none() {
                target.geometry = source.geometry;
            }

            // Use higher confidence
            if source.confidence > target.confidence {
                target.confidence = source.confidence;
            }

            target.last_updated = chrono::Utc::now();
            self.storage.insert_entity(&target).await?;

            // Soft-delete the source
            self.storage.delete_entity(source_id).await?;
        }

        Ok(())
    }
}

/// Simple name similarity based on common characters ratio
fn name_similarity(a: &str, b: &str) -> f32 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use orp_proto::{Entity, GeoPoint};
    use orp_storage::DuckDbStorage;
    use std::collections::HashMap;

    #[test]
    fn test_name_similarity_exact() {
        assert!((name_similarity("Rotterdam Express", "Rotterdam Express") - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_name_similarity_case() {
        assert!((name_similarity("rotterdam", "ROTTERDAM") - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_name_similarity_different() {
        let sim = name_similarity("Rotterdam Express", "Tokyo Maru");
        assert!(sim < 0.5);
    }

    #[tokio::test]
    async fn test_resolve_canonical() {
        let storage: Arc<dyn Storage> = Arc::new(DuckDbStorage::new_in_memory().unwrap());
        let resolver = EntityResolver::new(storage.clone());

        let entity = Entity {
            entity_id: "ship-1".to_string(),
            entity_type: "ship".to_string(),
            ..Entity::default()
        };
        storage.insert_entity(&entity).await.unwrap();

        // No canonical initially
        assert!(resolver.resolve("ship-1").await.unwrap().is_none());

        // Set canonical
        resolver.set_canonical("ship-1", "canonical-ship-1").await.unwrap();
        assert_eq!(
            resolver.resolve("ship-1").await.unwrap(),
            Some("canonical-ship-1".to_string())
        );
    }

    #[tokio::test]
    async fn test_find_matches_none() {
        let storage: Arc<dyn Storage> = Arc::new(DuckDbStorage::new_in_memory().unwrap());
        let resolver = EntityResolver::new(storage);

        let matches = resolver.find_matches("nonexistent", 10).await.unwrap();
        assert!(matches.is_empty());
    }

    #[tokio::test]
    async fn test_merge_entities() {
        let storage: Arc<dyn Storage> = Arc::new(DuckDbStorage::new_in_memory().unwrap());
        let resolver = EntityResolver::new(storage.clone());

        let mut props1 = HashMap::new();
        props1.insert("speed".to_string(), serde_json::json!(10.0));

        let mut props2 = HashMap::new();
        props2.insert("heading".to_string(), serde_json::json!(180.0));

        let source = Entity {
            entity_id: "ship-source".to_string(),
            entity_type: "ship".to_string(),
            name: Some("Source Ship".to_string()),
            properties: props1,
            geometry: Some(GeoPoint {
                lat: 51.0,
                lon: 4.0,
                alt: None,
            }),
            ..Entity::default()
        };
        let target = Entity {
            entity_id: "ship-target".to_string(),
            entity_type: "ship".to_string(),
            properties: props2,
            ..Entity::default()
        };

        storage.insert_entity(&source).await.unwrap();
        storage.insert_entity(&target).await.unwrap();

        resolver
            .merge_entities("ship-source", "ship-target")
            .await
            .unwrap();

        let merged = storage.get_entity("ship-target").await.unwrap().unwrap();
        // Should have both properties
        assert!(merged.properties.contains_key("speed"));
        assert!(merged.properties.contains_key("heading"));
        // Should have name from source
        assert_eq!(merged.name, Some("Source Ship".to_string()));
        // Should have geometry from source
        assert!(merged.geometry.is_some());
    }
}

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
                // Search for other entities with the same MMSI
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
}

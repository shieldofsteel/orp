use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEntry {
    pub sequence_number: u64,
    pub timestamp: DateTime<Utc>,
    pub operation: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<String>,
    pub user_id: Option<String>,
    pub details: serde_json::Value,
    pub previous_hash: String,
    pub content_hash: String,
}

pub struct AuditLog {
    entries: Vec<AuditEntry>,
    counter: u64,
}

impl AuditLog {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            counter: 0,
        }
    }

    pub fn log(
        &mut self,
        operation: &str,
        entity_type: Option<&str>,
        entity_id: Option<&str>,
        user_id: Option<&str>,
        details: serde_json::Value,
    ) -> &AuditEntry {
        self.counter += 1;
        let timestamp = Utc::now();

        let previous_hash = self
            .entries
            .last()
            .map(|e| e.content_hash.clone())
            .unwrap_or_else(|| "genesis".to_string());

        let hash_input = format!(
            "{}||{}||{}||{}",
            self.counter,
            operation,
            timestamp.to_rfc3339(),
            details
        );
        let content_hash = compute_sha256(&hash_input);

        let entry = AuditEntry {
            sequence_number: self.counter,
            timestamp,
            operation: operation.to_string(),
            entity_type: entity_type.map(String::from),
            entity_id: entity_id.map(String::from),
            user_id: user_id.map(String::from),
            details,
            previous_hash,
            content_hash,
        };

        self.entries.push(entry);
        self.entries.last().unwrap()
    }

    /// Verify the integrity of the hash chain
    pub fn verify(&self) -> bool {
        for (i, entry) in self.entries.iter().enumerate() {
            // Check previous hash linkage
            if i == 0 {
                if entry.previous_hash != "genesis" {
                    return false;
                }
            } else if entry.previous_hash != self.entries[i - 1].content_hash {
                return false;
            }

            // Verify content hash
            let hash_input = format!(
                "{}||{}||{}||{}",
                entry.sequence_number,
                entry.operation,
                entry.timestamp.to_rfc3339(),
                entry.details
            );
            let expected_hash = compute_sha256(&hash_input);
            if entry.content_hash != expected_hash {
                return false;
            }
        }
        true
    }

    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

fn compute_sha256(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_log_chain() {
        let mut log = AuditLog::new();

        log.log(
            "entity_created",
            Some("ship"),
            Some("mmsi:123456"),
            Some("system"),
            serde_json::json!({"name": "Test Ship"}),
        );

        log.log(
            "property_updated",
            Some("ship"),
            Some("mmsi:123456"),
            Some("ais-connector"),
            serde_json::json!({"speed": 12.5}),
        );

        assert_eq!(log.len(), 2);
        assert!(log.verify(), "Hash chain should be valid");
    }

    #[test]
    fn test_audit_log_tamper_detection() {
        let mut log = AuditLog::new();

        log.log(
            "entity_created",
            Some("ship"),
            Some("mmsi:123"),
            None,
            serde_json::json!({}),
        );

        log.log(
            "entity_updated",
            Some("ship"),
            Some("mmsi:123"),
            None,
            serde_json::json!({"speed": 10}),
        );

        assert!(log.verify());

        // Tamper with an entry
        if let Some(entry) = log.entries.first_mut() {
            entry.operation = "tampered".to_string();
        }

        assert!(!log.verify(), "Tampered log should fail verification");
    }
}

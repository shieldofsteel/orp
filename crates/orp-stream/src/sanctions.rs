//! OFAC SDN Sanctions Screening Engine
//!
//! Loads and indexes OFAC Specially Designated Nationals (SDN) data and performs:
//! - Exact MMSI/IMO matching (highest confidence)
//! - Fuzzy name matching via Levenshtein distance
//! - Background auto-reload every 24 hours if the source file changes
//!
//! # Usage
//! ```rust,no_run
//! use orp_stream::sanctions::{SanctionsDatabase, SanctionsQuery};
//!
//! # async fn example() -> anyhow::Result<()> {
//! let db = SanctionsDatabase::load_from_json("/path/to/sanctions.json").await?;
//! let result = db.check_entity(SanctionsQuery {
//!     name: Some("ATLANTIC SHADOW".to_string()),
//!     mmsi: Some("123456789".to_string()),
//!     imo: None,
//! }).await;
//! println!("Matched: {}, Risk: {:?}", result.matched, result.risk_level);
//! # Ok(())
//! # }
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;
use tokio::time::{self, Duration};

// ── Risk level ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SanctionsRiskLevel {
    /// No match found.
    Clear,
    /// Fuzzy name match only (score < 90).
    PossibleMatch,
    /// Strong fuzzy name match (score ≥ 90) or multiple alias hits.
    ProbableMatch,
    /// Exact MMSI/IMO hit or perfect name match (score = 100).
    ConfirmedHit,
}

// ── SDN entry ─────────────────────────────────────────────────────────────────

/// A single entry from the OFAC SDN list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SdnEntry {
    /// Canonical SDN name (all-caps as published by OFAC).
    pub name: String,
    /// Entity type (e.g. "vessel", "individual", "entity").
    pub sdn_type: String,
    /// Sanctions programs (e.g. ["IRAN", "SDGT"]).
    pub programs: Vec<String>,
    /// Known aliases / alternate names.
    pub aliases: Vec<String>,
    /// Addresses associated with the entity.
    pub addresses: Vec<String>,
    /// IMO number if this is a vessel.
    pub imo: Option<String>,
    /// MMSI number if this is a vessel.
    pub mmsi: Option<String>,
    /// Additional ID numbers (passport, DUNS, etc.).
    pub id_numbers: Vec<String>,
    /// Date added to the SDN list.
    pub listing_date: Option<String>,
}

// ── Match result ──────────────────────────────────────────────────────────────

/// A single SDN match.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SanctionsMatch {
    /// Canonical name from the SDN list.
    pub sdn_name: String,
    /// Entity type from the SDN list.
    pub sdn_type: String,
    /// Sanctions programs this entity appears in.
    pub programs: Vec<String>,
    /// Fuzzy name match score (0–100). 100 = exact.
    pub score: u8,
    /// True if the match was via an exact MMSI or IMO hit (highest confidence).
    pub id_match: bool,
    /// The alias that matched (if an alias triggered the match, not the primary name).
    pub matched_alias: Option<String>,
}

/// Result of a sanctions check.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SanctionsResult {
    pub matched: bool,
    pub matches: Vec<SanctionsMatch>,
    pub risk_level: SanctionsRiskLevel,
    pub checked_at: DateTime<Utc>,
}

impl SanctionsResult {
    fn no_match() -> Self {
        Self {
            matched: false,
            matches: vec![],
            risk_level: SanctionsRiskLevel::Clear,
            checked_at: Utc::now(),
        }
    }
}

// ── Query ─────────────────────────────────────────────────────────────────────

/// Input for a sanctions check.
#[derive(Clone, Debug, Default)]
pub struct SanctionsQuery {
    /// Entity name to check.
    pub name: Option<String>,
    /// MMSI (Maritime Mobile Service Identity).
    pub mmsi: Option<String>,
    /// IMO number.
    pub imo: Option<String>,
}

// ── JSON format ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct JsonSanctionsFile {
    entries: Vec<SdnEntry>,
}

// ── Internal indexes ──────────────────────────────────────────────────────────

struct SanctionsIndex {
    entries: Vec<SdnEntry>,
    /// Normalised name → entry indices
    name_index: HashMap<String, Vec<usize>>,
    /// MMSI → entry index
    mmsi_index: HashMap<String, usize>,
    /// IMO → entry index
    imo_index: HashMap<String, usize>,
    loaded_at: DateTime<Utc>,
    source_mtime: Option<SystemTime>,
}

impl SanctionsIndex {
    fn build(entries: Vec<SdnEntry>, source_mtime: Option<SystemTime>) -> Self {
        let mut name_index: HashMap<String, Vec<usize>> = HashMap::new();
        let mut mmsi_index: HashMap<String, usize> = HashMap::new();
        let mut imo_index: HashMap<String, usize> = HashMap::new();

        for (i, entry) in entries.iter().enumerate() {
            // Primary name
            name_index
                .entry(normalise(&entry.name))
                .or_default()
                .push(i);

            // Aliases
            for alias in &entry.aliases {
                name_index.entry(normalise(alias)).or_default().push(i);
            }

            // MMSI
            if let Some(mmsi) = &entry.mmsi {
                let key = normalise_id(mmsi);
                if !key.is_empty() {
                    mmsi_index.insert(key, i);
                }
            }

            // IMO
            if let Some(imo) = &entry.imo {
                let key = normalise_id(imo);
                if !key.is_empty() {
                    imo_index.insert(key, i);
                }
            }
        }

        Self {
            entries,
            name_index,
            mmsi_index,
            imo_index,
            loaded_at: Utc::now(),
            source_mtime,
        }
    }
}

// ── Database ──────────────────────────────────────────────────────────────────

/// OFAC SDN sanctions database with fuzzy matching and background reload.
#[derive(Clone)]
pub struct SanctionsDatabase {
    inner: Arc<RwLock<SanctionsIndex>>,
    /// Path to the source file (for auto-reload).
    source_path: Option<PathBuf>,
    /// Minimum fuzzy score (0–100) to report a name match.
    pub name_match_threshold: u8,
}

impl SanctionsDatabase {
    // ── Construction ──────────────────────────────────────────────────────────

    /// Create an empty database (useful for testing).
    pub fn empty() -> Self {
        let index = SanctionsIndex::build(vec![], None);
        Self {
            inner: Arc::new(RwLock::new(index)),
            source_path: None,
            name_match_threshold: 75,
        }
    }

    /// Load from OFAC SDN CSV format.
    ///
    /// Expected columns (0-indexed):
    /// 0: SDN name, 1: type, 2: programs (semicolon-sep), 3: aliases (semicolon-sep),
    /// 4: addresses (semicolon-sep), 5: IMO, 6: MMSI, 7: other IDs (semicolon-sep),
    /// 8: listing date
    pub async fn load_from_csv(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mtime = file_mtime(&path);
        let entries = parse_csv(&path)?;
        let index = SanctionsIndex::build(entries, mtime);
        let db = Self {
            inner: Arc::new(RwLock::new(index)),
            source_path: Some(path),
            name_match_threshold: 75,
        };
        Ok(db)
    }

    /// Load from JSON format (see [`JsonSanctionsFile`]).
    pub async fn load_from_json(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mtime = file_mtime(&path);
        let entries = parse_json(&path)?;
        let index = SanctionsIndex::build(entries, mtime);
        let db = Self {
            inner: Arc::new(RwLock::new(index)),
            source_path: Some(path),
            name_match_threshold: 75,
        };
        Ok(db)
    }

    /// Load from an already-parsed list of entries (e.g. fetched from an API).
    pub fn from_entries(entries: Vec<SdnEntry>) -> Self {
        let index = SanctionsIndex::build(entries, None);
        Self {
            inner: Arc::new(RwLock::new(index)),
            source_path: None,
            name_match_threshold: 75,
        }
    }

    // ── Reload ────────────────────────────────────────────────────────────────

    /// Reload from source file if the file has changed since last load.
    pub async fn reload_if_changed(&self) -> anyhow::Result<bool> {
        let path = match &self.source_path {
            Some(p) => p.clone(),
            None => return Ok(false),
        };

        let current_mtime = file_mtime(&path);
        let stored_mtime = self.inner.read().await.source_mtime;

        if current_mtime == stored_mtime {
            return Ok(false);
        }

        let entries = if path.extension().and_then(|e| e.to_str()) == Some("csv") {
            parse_csv(&path)?
        } else {
            parse_json(&path)?
        };

        let new_index = SanctionsIndex::build(entries, current_mtime);
        *self.inner.write().await = new_index;
        Ok(true)
    }

    /// Spawn a background task that checks the file every 24 hours and reloads if changed.
    pub fn spawn_auto_reload(&self) {
        let db = self.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(24 * 3600));
            interval.tick().await; // skip immediate tick
            loop {
                interval.tick().await;
                match db.reload_if_changed().await {
                    Ok(true) => tracing::info!("sanctions: database reloaded from disk"),
                    Ok(false) => tracing::debug!("sanctions: no changes, skipping reload"),
                    Err(e) => tracing::warn!("sanctions: reload failed: {e}"),
                }
            }
        });
    }

    // ── Query ─────────────────────────────────────────────────────────────────

    /// Check an entity against the sanctions database.
    ///
    /// Priority:
    /// 1. Exact MMSI/IMO match → `ConfirmedHit`
    /// 2. Exact name match (score 100) → `ConfirmedHit`
    /// 3. Strong fuzzy match (score ≥ 90) → `ProbableMatch`
    /// 4. Moderate fuzzy match (score ≥ threshold) → `PossibleMatch`
    pub async fn check_entity(&self, query: SanctionsQuery) -> SanctionsResult {
        let index = self.inner.read().await;
        let mut matches: Vec<SanctionsMatch> = Vec::new();

        // 1. Exact ID matching (MMSI / IMO)
        if let Some(mmsi) = &query.mmsi {
            let key = normalise_id(mmsi);
            if let Some(&idx) = index.mmsi_index.get(&key) {
                let entry = &index.entries[idx];
                matches.push(SanctionsMatch {
                    sdn_name: entry.name.clone(),
                    sdn_type: entry.sdn_type.clone(),
                    programs: entry.programs.clone(),
                    score: 100,
                    id_match: true,
                    matched_alias: None,
                });
            }
        }

        if let Some(imo) = &query.imo {
            let key = normalise_id(imo);
            if let Some(&idx) = index.imo_index.get(&key) {
                let entry = &index.entries[idx];
                // Avoid duplicating if already caught by MMSI
                let already = matches.iter().any(|m| m.sdn_name == entry.name && m.id_match);
                if !already {
                    matches.push(SanctionsMatch {
                        sdn_name: entry.name.clone(),
                        sdn_type: entry.sdn_type.clone(),
                        programs: entry.programs.clone(),
                        score: 100,
                        id_match: true,
                        matched_alias: None,
                    });
                }
            }
        }

        // 2. Fuzzy name matching
        if let Some(name) = &query.name {
            let norm_query = normalise(name);
            if !norm_query.is_empty() {
                let threshold = self.name_match_threshold;
                for (i, entry) in index.entries.iter().enumerate() {
                    // Skip entries already matched by ID
                    let already_id_matched = matches
                        .iter()
                        .any(|m| m.sdn_name == entry.name && m.id_match);

                    // Check primary name
                    let primary_norm = normalise(&entry.name);
                    let primary_score = fuzzy_score(&norm_query, &primary_norm);

                    // Check aliases
                    let best_alias = entry
                        .aliases
                        .iter()
                        .map(|a| (a.clone(), fuzzy_score(&norm_query, &normalise(a))))
                        .max_by_key(|(_, s)| *s);

                    let (score, matched_alias) = match best_alias {
                        Some((alias, alias_score)) if alias_score > primary_score => {
                            (alias_score, Some(alias))
                        }
                        _ => (primary_score, None),
                    };

                    if score >= threshold && !already_id_matched {
                        matches.push(SanctionsMatch {
                            sdn_name: entry.name.clone(),
                            sdn_type: entry.sdn_type.clone(),
                            programs: entry.programs.clone(),
                            score,
                            id_match: false,
                            matched_alias,
                        });
                    }
                    let _ = i; // suppress unused warning
                }
            }
        }

        // Deduplicate: if the same SDN name appears via both ID and fuzzy, keep ID match
        matches.sort_by(|a, b| {
            b.id_match
                .cmp(&a.id_match)
                .then(b.score.cmp(&a.score))
        });
        matches.dedup_by(|a, b| a.sdn_name == b.sdn_name && b.id_match);

        if matches.is_empty() {
            return SanctionsResult::no_match();
        }

        let risk_level = compute_risk(&matches);
        SanctionsResult {
            matched: true,
            matches,
            risk_level,
            checked_at: Utc::now(),
        }
    }

    /// Number of entries currently loaded.
    pub async fn entry_count(&self) -> usize {
        self.inner.read().await.entries.len()
    }

    /// When the database was last loaded.
    pub async fn loaded_at(&self) -> DateTime<Utc> {
        self.inner.read().await.loaded_at
    }
}

// ── Risk level calculation ────────────────────────────────────────────────────

fn compute_risk(matches: &[SanctionsMatch]) -> SanctionsRiskLevel {
    // Any exact ID hit → ConfirmedHit
    if matches.iter().any(|m| m.id_match) {
        return SanctionsRiskLevel::ConfirmedHit;
    }
    let best_score = matches.iter().map(|m| m.score).max().unwrap_or(0);
    if best_score == 100 {
        SanctionsRiskLevel::ConfirmedHit
    } else if best_score >= 90 {
        SanctionsRiskLevel::ProbableMatch
    } else {
        SanctionsRiskLevel::PossibleMatch
    }
}

// ── String helpers ────────────────────────────────────────────────────────────

/// Normalise a name for comparison: uppercase, collapse whitespace, strip punctuation.
fn normalise(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase()
}

/// Normalise an ID: strip all non-alphanumeric characters, uppercase.
fn normalise_id(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_uppercase()
}

// ── Levenshtein / fuzzy score ─────────────────────────────────────────────────

/// Compute Levenshtein edit distance between two strings.
/// Operates on Unicode characters (not bytes).
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();

    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    // Two-row DP (O(n) space)
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (curr[j - 1] + 1)
                .min(prev[j] + 1)
                .min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

/// Convert Levenshtein distance to a 0–100 similarity score.
/// Score = max(0, 100 - 100 * distance / max_len).
pub fn fuzzy_score(a: &str, b: &str) -> u8 {
    if a == b {
        return 100;
    }
    let max_len = a.len().max(b.len());
    if max_len == 0 {
        return 100;
    }
    let dist = levenshtein(a, b);
    let ratio = dist as f64 / max_len as f64;
    let score = ((1.0 - ratio) * 100.0).round() as i64;
    score.clamp(0, 100) as u8
}

// ── File helpers ──────────────────────────────────────────────────────────────

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

// ── CSV parser ────────────────────────────────────────────────────────────────

fn parse_csv(path: &Path) -> anyhow::Result<Vec<SdnEntry>> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();

    for (line_no, line) in reader.lines().enumerate() {
        let line = line?;
        // Skip header row
        if line_no == 0 && line.to_lowercase().contains("name") {
            continue;
        }
        let fields: Vec<&str> = line.splitn(9, ',').collect();
        if fields.len() < 2 {
            continue;
        }

        let get = |idx: usize| fields.get(idx).map(|s| s.trim().trim_matches('"')).unwrap_or("");
        let split_semi = |s: &str| -> Vec<String> {
            s.split(';')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        };

        entries.push(SdnEntry {
            name: get(0).to_string(),
            sdn_type: get(1).to_string(),
            programs: split_semi(get(2)),
            aliases: split_semi(get(3)),
            addresses: split_semi(get(4)),
            imo: Some(get(5).to_string()).filter(|s| !s.is_empty()),
            mmsi: Some(get(6).to_string()).filter(|s| !s.is_empty()),
            id_numbers: split_semi(get(7)),
            listing_date: Some(get(8).to_string()).filter(|s| !s.is_empty()),
        });
    }

    Ok(entries)
}

// ── JSON parser ───────────────────────────────────────────────────────────────

fn parse_json(path: &Path) -> anyhow::Result<Vec<SdnEntry>> {
    let data = std::fs::read_to_string(path)?;
    let file: JsonSanctionsFile = serde_json::from_str(&data)?;
    Ok(file.entries)
}

// ══════════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Levenshtein ──────────────────────────────────────────────────────────

    #[test]
    fn levenshtein_identical() {
        assert_eq!(levenshtein("HELLO", "HELLO"), 0);
    }

    #[test]
    fn levenshtein_empty_strings() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("ABC", ""), 3);
        assert_eq!(levenshtein("", "ABC"), 3);
    }

    #[test]
    fn levenshtein_one_edit() {
        // One substitution
        assert_eq!(levenshtein("CAT", "BAT"), 1);
        // One deletion
        assert_eq!(levenshtein("KITTEN", "KITTEN"), 0);
        assert_eq!(levenshtein("KITTEN", "KITTN"), 1);
        // One insertion
        assert_eq!(levenshtein("KITTEN", "KITTENS"), 1);
    }

    #[test]
    fn levenshtein_classic_example() {
        // "kitten" → "sitting" = 3
        assert_eq!(levenshtein("KITTEN", "SITTING"), 3);
    }

    #[test]
    fn levenshtein_completely_different() {
        let d = levenshtein("ABCDEF", "ZYXWVU");
        assert_eq!(d, 6);
    }

    // ── fuzzy_score ──────────────────────────────────────────────────────────

    #[test]
    fn fuzzy_score_exact_match() {
        assert_eq!(fuzzy_score("ATLANTIC SHADOW", "ATLANTIC SHADOW"), 100);
    }

    #[test]
    fn fuzzy_score_one_char_diff() {
        // 1 edit in 15 chars → ~93%
        let s = fuzzy_score("ATLANTIC SHADOW", "ATLANTIC SHABOW");
        assert!(s >= 90, "expected ≥90, got {s}");
    }

    #[test]
    fn fuzzy_score_completely_different() {
        let s = fuzzy_score("AAAAAAA", "BBBBBBB");
        assert!(s <= 10, "expected ≤10, got {s}");
    }

    #[test]
    fn fuzzy_score_empty_strings() {
        assert_eq!(fuzzy_score("", ""), 100);
        assert_eq!(fuzzy_score("ABC", ""), 0);
    }

    #[test]
    fn fuzzy_score_typo_tolerance() {
        // Common vessel name typo
        let s = fuzzy_score("IRAN ARYA", "IRAN ARIA");
        assert!(s >= 80, "expected ≥80, got {s}");
    }

    // ── normalise ────────────────────────────────────────────────────────────

    #[test]
    fn normalise_strips_punctuation() {
        assert_eq!(normalise("M/V ATLAS"), "M V ATLAS");
    }

    #[test]
    fn normalise_collapses_whitespace() {
        assert_eq!(normalise("  IRAN   VESSEL  "), "IRAN VESSEL");
    }

    #[test]
    fn normalise_id_strips_non_alphanum() {
        assert_eq!(normalise_id("IMO-1234567"), "IMO1234567");
        assert_eq!(normalise_id("123 456 789"), "123456789");
    }

    // ── SanctionsDatabase ────────────────────────────────────────────────────

    fn make_db() -> SanctionsDatabase {
        let entries = vec![
            SdnEntry {
                name: "SHADOW TIDE".to_string(),
                sdn_type: "vessel".to_string(),
                programs: vec!["IRAN".to_string()],
                aliases: vec!["SHADOW WAVE".to_string(), "DARK TIDE".to_string()],
                addresses: vec![],
                imo: Some("9876543".to_string()),
                mmsi: Some("123456789".to_string()),
                id_numbers: vec![],
                listing_date: Some("2023-01-15".to_string()),
            },
            SdnEntry {
                name: "ARCTIC PHANTOM".to_string(),
                sdn_type: "vessel".to_string(),
                programs: vec!["DPRK".to_string()],
                aliases: vec!["ARCTIC GHOST".to_string()],
                addresses: vec![],
                imo: Some("1234567".to_string()),
                mmsi: None,
                id_numbers: vec![],
                listing_date: None,
            },
            SdnEntry {
                name: "ROGUE STAR TRADING LLC".to_string(),
                sdn_type: "entity".to_string(),
                programs: vec!["SDGT".to_string(), "TCO".to_string()],
                aliases: vec!["ROGUE STAR TRADING".to_string()],
                addresses: vec!["Dubai, UAE".to_string()],
                imo: None,
                mmsi: None,
                id_numbers: vec!["DUNS: 123456789".to_string()],
                listing_date: Some("2024-03-01".to_string()),
            },
        ];
        SanctionsDatabase::from_entries(entries)
    }

    #[tokio::test]
    async fn check_exact_mmsi_match() {
        let db = make_db();
        let result = db
            .check_entity(SanctionsQuery {
                mmsi: Some("123456789".to_string()),
                ..Default::default()
            })
            .await;
        assert!(result.matched);
        assert!(result.matches[0].id_match);
        assert_eq!(result.matches[0].score, 100);
        assert_eq!(result.risk_level, SanctionsRiskLevel::ConfirmedHit);
    }

    #[tokio::test]
    async fn check_exact_imo_match() {
        let db = make_db();
        let result = db
            .check_entity(SanctionsQuery {
                imo: Some("9876543".to_string()),
                ..Default::default()
            })
            .await;
        assert!(result.matched);
        assert!(result.matches[0].id_match);
        assert_eq!(result.risk_level, SanctionsRiskLevel::ConfirmedHit);
    }

    #[tokio::test]
    async fn check_exact_name_match() {
        let db = make_db();
        let result = db
            .check_entity(SanctionsQuery {
                name: Some("SHADOW TIDE".to_string()),
                ..Default::default()
            })
            .await;
        assert!(result.matched);
        assert_eq!(result.matches[0].score, 100);
        assert_eq!(result.risk_level, SanctionsRiskLevel::ConfirmedHit);
    }

    #[tokio::test]
    async fn check_alias_match() {
        let db = make_db();
        let result = db
            .check_entity(SanctionsQuery {
                name: Some("DARK TIDE".to_string()),
                ..Default::default()
            })
            .await;
        assert!(result.matched);
        assert!(result.matches[0].matched_alias.is_some());
    }

    #[tokio::test]
    async fn check_fuzzy_name_match() {
        let db = make_db();
        // One character off
        let result = db
            .check_entity(SanctionsQuery {
                name: Some("SHADOW TYDE".to_string()),
                ..Default::default()
            })
            .await;
        assert!(result.matched);
        assert!(result.matches[0].score >= 75);
    }

    #[tokio::test]
    async fn check_no_match() {
        let db = make_db();
        let result = db
            .check_entity(SanctionsQuery {
                name: Some("COMPLETELY UNRELATED VESSEL".to_string()),
                mmsi: Some("999999999".to_string()),
                imo: Some("0000000".to_string()),
            })
            .await;
        assert!(!result.matched);
        assert_eq!(result.risk_level, SanctionsRiskLevel::Clear);
    }

    #[tokio::test]
    async fn check_empty_query() {
        let db = make_db();
        let result = db.check_entity(SanctionsQuery::default()).await;
        assert!(!result.matched);
    }

    #[tokio::test]
    async fn entry_count_correct() {
        let db = make_db();
        assert_eq!(db.entry_count().await, 3);
    }

    #[tokio::test]
    async fn id_match_takes_precedence_over_fuzzy() {
        let db = make_db();
        // Query with both MMSI (exact hit) and unrelated name
        let result = db
            .check_entity(SanctionsQuery {
                name: Some("TOTALLY DIFFERENT NAME".to_string()),
                mmsi: Some("123456789".to_string()),
                imo: None,
            })
            .await;
        assert!(result.matched);
        // The ID match should be first and have highest confidence
        let first = &result.matches[0];
        assert!(first.id_match);
        assert_eq!(first.score, 100);
    }

    #[tokio::test]
    async fn mmsi_normalised_for_matching() {
        let db = make_db();
        // With dashes/spaces
        let result = db
            .check_entity(SanctionsQuery {
                mmsi: Some("123-456-789".to_string()),
                ..Default::default()
            })
            .await;
        assert!(result.matched);
        assert!(result.matches[0].id_match);
    }
}

//! Multi-domain classification labels and CAPCO banner formatting.
//!
//! Federal IL5/IL6 + NATO COSMIC procurement requires tracked-object
//! classification with banner enforcement on every API surface and a
//! dominance ordering for ABAC rule evaluation. This module provides:
//!
//! * [`Level`] — the partial-ordered set of US + NATO classification levels
//!   that ORP recognises today. Ordering is built from `PartialOrd`: any
//!   `Level::S` dominates any `Level::CUI`, etc. NATO peers (`NU/NR/NC/NS`)
//!   slot in next to their US equivalents per the NSA / NARA mapping.
//! * [`Classification`] — a level plus optional SCI / dissemination-control
//!   markings. Ed25519-signable via serde so it survives the audit chain.
//! * [`Classification::banner`] — produces a CAPCO-compliant single-line
//!   banner string suitable for HTTP `X-Classification`, WS subprotocol
//!   negotiation, and CLI output.
//! * [`Classification::dominates`] — the ABAC predicate. A user with
//!   clearance `TS//SI//NOFORN` dominates an event marked `S//NOFORN`,
//!   but does NOT dominate `S//ATOMAL` unless the user holds ATOMAL.
//!
//! This layer is *advisory*: it does not implement classification-aware
//! storage encryption (handled separately by F7) and does not hook the
//! audit log signature (the level is part of the signed pre-image once a
//! caller adds it to their audit metadata). It exists so callers across
//! the workspace agree on the type and can compose with each other.

use serde::{Deserialize, Serialize};

/// Maximum total banner length (CAPCO line) — fits any combination ORP
/// supports in a single HTTP header.
pub const MAX_BANNER_LEN: usize = 256;

/// Classification level taxonomy supported by ORP.
///
/// Variants are ordered by `derive(PartialOrd, Ord)` so larger numeric
/// discriminants dominate smaller ones — the natural reading is also the
/// security order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Level {
    /// UNCLASSIFIED.
    U,
    /// CONTROLLED UNCLASSIFIED INFORMATION (US executive order 13556).
    CUI,
    /// NATO RESTRICTED — NARA maps to CUI tier per cui registry.
    NR,
    /// CONFIDENTIAL.
    C,
    /// NATO CONFIDENTIAL.
    NC,
    /// SECRET.
    S,
    /// NATO SECRET.
    NS,
    /// TOP SECRET.
    TS,
    /// COSMIC TOP SECRET — highest NATO tier.
    CTS,
}

impl Level {
    /// CAPCO short name used in banners. Includes spaces where appropriate
    /// so a banner like `"TOP SECRET//NOFORN"` reads correctly.
    pub fn capco(self) -> &'static str {
        match self {
            Level::U => "UNCLASSIFIED",
            Level::CUI => "CUI",
            Level::NR => "NATO RESTRICTED",
            Level::C => "CONFIDENTIAL",
            Level::NC => "NATO CONFIDENTIAL",
            Level::S => "SECRET",
            Level::NS => "NATO SECRET",
            Level::TS => "TOP SECRET",
            Level::CTS => "COSMIC TOP SECRET",
        }
    }
}

/// Full classification with optional SCI compartments and dissemination
/// controls. Default = `UNCLASSIFIED` so `Classification::default()` always
/// yields the safest annotation an event can carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Classification {
    pub level: Level,
    /// SCI compartments / sub-controls — `SI`, `TK`, `HCS`, `KLONDIKE`, etc.
    /// Each entry must be ASCII alphanumeric + `-_/` (validated on parse).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sci: Vec<String>,
    /// Dissemination controls — `NOFORN`, `REL TO USA, GBR`, `ORCON`, etc.
    /// Stored verbatim; banners join with `/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dissem: Vec<String>,
    /// `true` if this carries the NATO `ATOMAL` qualifier (parallel to SCI).
    #[serde(default, skip_serializing_if = "is_false")]
    pub atomal: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Default for Classification {
    fn default() -> Self {
        Self {
            level: Level::U,
            sci: Vec::new(),
            dissem: Vec::new(),
            atomal: false,
        }
    }
}

impl Classification {
    /// Convenience constructor for plain levels.
    pub fn new(level: Level) -> Self {
        Self {
            level,
            ..Self::default()
        }
    }

    /// Add an SCI compartment after validation. Returns the modified
    /// classification on success and an error string on bad input —
    /// chainable in builder fashion.
    pub fn with_sci(mut self, sci: impl Into<String>) -> Result<Self, ClassificationError> {
        let sci = sci.into().trim().to_string();
        if sci.is_empty() || sci.len() > 32 {
            return Err(ClassificationError::Invalid(format!(
                "SCI compartment '{sci}' has invalid length"
            )));
        }
        if !sci
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'/' | b' '))
        {
            return Err(ClassificationError::Invalid(format!(
                "SCI compartment '{sci}' contains illegal characters"
            )));
        }
        self.sci.push(sci);
        Ok(self)
    }

    /// Add a dissemination control after validation.
    pub fn with_dissem(mut self, dissem: impl Into<String>) -> Result<Self, ClassificationError> {
        let d = dissem.into().trim().to_string();
        if d.is_empty() || d.len() > 64 {
            return Err(ClassificationError::Invalid(format!(
                "dissem control '{d}' has invalid length"
            )));
        }
        // Allow `REL TO USA, GBR, AUS` — commas + spaces are legal.
        if !d.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b' ' | b',' | b'/' | b'-' | b'_' | b'.')
        }) {
            return Err(ClassificationError::Invalid(format!(
                "dissem control '{d}' contains illegal characters"
            )));
        }
        self.dissem.push(d);
        Ok(self)
    }

    /// Mark NATO ATOMAL.
    pub fn with_atomal(mut self) -> Self {
        self.atomal = true;
        self
    }

    /// CAPCO single-line banner. Format: `LEVEL[//SCI][//DISSEM]`.
    /// Capped at [`MAX_BANNER_LEN`] characters; truncation never happens
    /// in normal use because each component is itself bounded.
    pub fn banner(&self) -> String {
        let mut out = String::with_capacity(64);
        out.push_str(self.level.capco());
        // SCI compartments — joined by `/`, prefixed by `//`.
        let mut sci = self.sci.clone();
        if self.atomal {
            sci.push("ATOMAL".to_string());
        }
        if !sci.is_empty() {
            out.push_str("//");
            out.push_str(&sci.join("/"));
        }
        if !self.dissem.is_empty() {
            out.push_str("//");
            out.push_str(&self.dissem.join("/"));
        }
        if out.len() > MAX_BANNER_LEN {
            out.truncate(MAX_BANNER_LEN);
        }
        out
    }

    /// Subject (user) clearance dominates resource (event) classification
    /// when:
    /// 1. Subject's level ≥ resource's level AND
    /// 2. Subject holds every SCI compartment the resource carries AND
    /// 3. Subject holds the ATOMAL qualifier if the resource carries it.
    ///
    /// Dissemination controls are advisory and not enforced by this
    /// predicate — they require their own per-control logic (NOFORN
    /// requires citizenship metadata; ORCON requires originator opt-in).
    pub fn dominates(&self, resource: &Classification) -> bool {
        if self.level < resource.level {
            return false;
        }
        for compartment in &resource.sci {
            if !self.sci.iter().any(|owned| owned == compartment) {
                return false;
            }
        }
        if resource.atomal && !self.atomal {
            return false;
        }
        true
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClassificationError {
    #[error("invalid classification: {0}")]
    Invalid(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_ordering_is_total_and_security_correct() {
        assert!(Level::TS > Level::S);
        assert!(Level::S > Level::C);
        assert!(Level::C > Level::CUI);
        assert!(Level::CUI > Level::U);
        assert!(Level::CTS > Level::TS);
        // NATO equivalents slot between US tiers.
        assert!(Level::NS < Level::TS);
        assert!(Level::NS >= Level::S);
    }

    #[test]
    fn banner_renders_capco() {
        assert_eq!(Classification::new(Level::U).banner(), "UNCLASSIFIED");
        assert_eq!(Classification::new(Level::TS).banner(), "TOP SECRET");
        let c = Classification::new(Level::TS)
            .with_sci("SI")
            .unwrap()
            .with_sci("TK")
            .unwrap()
            .with_dissem("NOFORN")
            .unwrap();
        assert_eq!(c.banner(), "TOP SECRET//SI/TK//NOFORN");
    }

    #[test]
    fn banner_appends_atomal_to_sci() {
        let c = Classification::new(Level::CTS).with_atomal();
        assert_eq!(c.banner(), "COSMIC TOP SECRET//ATOMAL");
    }

    #[test]
    fn dominates_simple_levels() {
        let ts = Classification::new(Level::TS);
        let s = Classification::new(Level::S);
        let u = Classification::new(Level::U);
        assert!(ts.dominates(&s));
        assert!(ts.dominates(&u));
        assert!(!u.dominates(&s));
        assert!(s.dominates(&s)); // equal level dominates
    }

    #[test]
    fn dominates_requires_sci_match() {
        let resource = Classification::new(Level::TS).with_sci("SI").unwrap();
        let subject_no_sci = Classification::new(Level::TS);
        assert!(
            !subject_no_sci.dominates(&resource),
            "TS without SI must NOT see TS//SI"
        );
        let subject_with_sci = Classification::new(Level::TS).with_sci("SI").unwrap();
        assert!(subject_with_sci.dominates(&resource));
    }

    #[test]
    fn dominates_requires_atomal_match() {
        let resource = Classification::new(Level::CTS).with_atomal();
        let subject = Classification::new(Level::CTS);
        assert!(
            !subject.dominates(&resource),
            "CTS without ATOMAL must NOT see CTS//ATOMAL"
        );
        let subject_atomal = Classification::new(Level::CTS).with_atomal();
        assert!(subject_atomal.dominates(&resource));
    }

    #[test]
    fn rejects_illegal_sci_chars() {
        let err = Classification::new(Level::S)
            .with_sci("BAD;DROP TABLE")
            .unwrap_err();
        assert!(format!("{err}").contains("illegal characters"));
    }

    #[test]
    fn dissem_rel_to_with_commas_is_allowed() {
        let c = Classification::new(Level::S)
            .with_dissem("REL TO USA, GBR, AUS")
            .unwrap();
        assert!(c.banner().contains("REL TO USA, GBR, AUS"));
    }

    #[test]
    fn serde_roundtrip_omits_empty_lists() {
        let c = Classification::new(Level::S);
        let json = serde_json::to_string(&c).unwrap();
        // No `"sci":[]` clutter.
        assert_eq!(json, r#"{"level":"S"}"#);
        let parsed: Classification = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.level, Level::S);
        assert!(parsed.sci.is_empty());
        assert!(parsed.dissem.is_empty());
        assert!(!parsed.atomal);
    }

    #[test]
    fn banner_has_hard_cap() {
        let mut c = Classification::new(Level::TS);
        for i in 0..50 {
            c = c.with_sci(format!("Compartment{i}")).unwrap();
        }
        let banner = c.banner();
        assert!(banner.len() <= MAX_BANNER_LEN);
    }
}

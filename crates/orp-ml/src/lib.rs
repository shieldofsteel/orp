//! ORP machine-learning scorers — pluggable models for the stream hot path.
//!
//! # What this is
//!
//! `orp-ml` is the ML seam for ORP. It exposes a single trait,
//! [`AnomalyScorer`], that any model — rule-based, statistical, deep — can
//! implement. The stream processor takes a `Arc<dyn AnomalyScorer>` and calls
//! it on every position update; that's the whole integration surface.
//!
//! # Why a seam first, models second
//!
//! ORP ships a hand-coded rule-based anomaly score in
//! `orp_stream::analytics::score_anomaly`. It works, but it's blind to
//! first-time deviations because the standard deviation it builds from
//! history *grows* whenever an anomalous sample is folded in, masking the
//! anomaly. We don't want to throw the rule-based score out — operators rely
//! on its explainability — but we do want to *augment* it with models that
//! can learn distributions the rule code can't express.
//!
//! The trait is the moat. The models that ride on it (Isolation Forest,
//! quantile heuristics, future neural scorers) are interchangeable parts.
//!
//! # What ships here
//!
//! - [`NullScorer`] — zero-cost default; returns 0.0 always. Useful when
//!   ML is disabled or for benchmarking the rule engine in isolation.
//! - [`OnlineQuantileScorer`] — model-free per-feature p99.5 flagger over a
//!   rolling 2048-sample window. Fast, dependency-free, useful as a baseline.
//! - [`IsolationForestScorer`] — small in-house Isolation Forest. Models
//!   are trained offline (or via `IsolationForestModel::fit`) and serialised
//!   with `bincode`, so the runtime can `include_bytes!` a pre-trained model.
//! - [`features`] — feature extractors that turn an entity track into a
//!   fixed-length `Vec<f32>` suitable for any of the scorers above.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use thiserror::Error;

pub mod features;

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum MlError {
    #[error("Feature dimension mismatch: model expects {expected}, got {got}")]
    FeatureDimMismatch { expected: usize, got: usize },
    #[error("Failed to deserialize model: {0}")]
    Deserialize(String),
    #[error("Failed to serialize model: {0}")]
    Serialize(String),
    #[error("Invalid model: {0}")]
    InvalidModel(String),
}

pub type MlResult<T> = Result<T, MlError>;

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Canonical interface for anomaly scorers.
///
/// Implementors must be `Send + Sync` so a single instance can be shared
/// across the async hot path via `Arc<dyn AnomalyScorer>`.
pub trait AnomalyScorer: Send + Sync {
    /// Returns a score in `[0, 100]` where higher means more anomalous.
    fn score(&self, features: &[f32]) -> f32;

    /// Human-readable model identifier for logs / metrics / audit trail.
    fn model_id(&self) -> &str;

    /// Number of features the scorer expects, for sanity-checking.
    fn feature_dim(&self) -> usize;
}

// ── NullScorer ────────────────────────────────────────────────────────────────

/// A scorer that always returns 0. Use this when ML scoring is disabled —
/// it lets the processor call the seam unconditionally without paying any
/// real cost.
#[derive(Clone, Debug, Default)]
pub struct NullScorer;

impl AnomalyScorer for NullScorer {
    fn score(&self, _features: &[f32]) -> f32 {
        0.0
    }
    fn model_id(&self) -> &str {
        "null-v0"
    }
    fn feature_dim(&self) -> usize {
        0
    }
}

// ── OnlineQuantileScorer ──────────────────────────────────────────────────────

const QUANTILE_BUFFER_CAP: usize = 2048;
const QUANTILE_TARGET: f32 = 0.995;

/// Per-feature streaming p99.5 anomaly flagger.
///
/// Maintains a rolling buffer of the last `N=2048` samples per feature and
/// computes p99.5 from that buffer on demand. Any feature value above its
/// p99.5 contributes to the score; the score is the fraction of axes
/// triggered, scaled to `[0, 100]`.
///
/// During warmup (fewer than 64 samples) the scorer always returns 0 — it
/// will not flag noise on a cold start.
pub struct OnlineQuantileScorer {
    model_id: String,
    feature_dim: usize,
    /// Per-feature ring buffer, protected by a single Mutex so updates are
    /// race-free. Hot-path contention is acceptable; the critical section
    /// is O(N) once per call but bounded by `QUANTILE_BUFFER_CAP`.
    buffers: Mutex<Vec<Vec<f32>>>,
}

impl OnlineQuantileScorer {
    pub fn new(model_id: impl Into<String>, feature_dim: usize) -> Self {
        Self {
            model_id: model_id.into(),
            feature_dim,
            buffers: Mutex::new(vec![Vec::with_capacity(QUANTILE_BUFFER_CAP); feature_dim]),
        }
    }

    /// Returns the number of samples seen for the first feature axis.
    /// Useful for tests and metrics.
    pub fn samples_seen(&self) -> usize {
        match self.buffers.lock() {
            Ok(b) => b.first().map(|v| v.len()).unwrap_or(0),
            Err(_) => 0,
        }
    }
}

impl AnomalyScorer for OnlineQuantileScorer {
    fn score(&self, features: &[f32]) -> f32 {
        if features.len() != self.feature_dim {
            tracing::warn!(
                model = %self.model_id,
                expected = self.feature_dim,
                got = features.len(),
                "OnlineQuantileScorer: feature dim mismatch, returning 0",
            );
            return 0.0;
        }
        let mut bufs = match self.buffers.lock() {
            Ok(b) => b,
            Err(_) => return 0.0,
        };

        // Compute score against the *current* distribution before folding the
        // new sample in — otherwise an extreme value contaminates its own
        // p99.5 estimate.
        let mut triggered = 0usize;
        let mut warming_up = false;
        for (i, &f) in features.iter().enumerate() {
            let buf = &bufs[i];
            if buf.len() < 64 {
                warming_up = true;
                continue;
            }
            let mut sorted: Vec<f32> = buf.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let idx =
                ((sorted.len() as f32) * QUANTILE_TARGET).floor() as usize;
            let cutoff = sorted[idx.min(sorted.len().saturating_sub(1))];
            if f.abs() > cutoff.abs() {
                triggered += 1;
            }
        }

        // Fold sample in (ring buffer).
        for (i, &f) in features.iter().enumerate() {
            let buf = &mut bufs[i];
            if buf.len() >= QUANTILE_BUFFER_CAP {
                buf.remove(0);
            }
            buf.push(f);
        }

        if warming_up || self.feature_dim == 0 {
            return 0.0;
        }
        (triggered as f32 / self.feature_dim as f32) * 100.0
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn feature_dim(&self) -> usize {
        self.feature_dim
    }
}

// ── IsolationForestScorer ─────────────────────────────────────────────────────

/// One node of an isolation tree. Either an internal split or a leaf.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum IfNode {
    Split {
        feature: usize,
        threshold: f32,
        left: Box<IfNode>,
        right: Box<IfNode>,
    },
    Leaf {
        size: u32,
    },
}

/// Serializable Isolation Forest model.
///
/// Train offline via [`IsolationForestModel::fit`] then serialize with
/// `bincode`. The runtime loads with [`IsolationForestScorer::from_bytes`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IsolationForestModel {
    pub feature_dim: usize,
    pub sample_size: u32,
    pub trees: Vec<IfNode>,
}

impl IsolationForestModel {
    /// Fit a forest with `n_trees` trees on `data`, each grown from a random
    /// sub-sample of `sample_size` rows. Uses a simple LCG so training is
    /// deterministic given `seed`; fine for our use-case (reproducible
    /// fixtures, deterministic tests). Not cryptographically random — and
    /// shouldn't be.
    pub fn fit(data: &[Vec<f32>], n_trees: usize, sample_size: usize, seed: u64) -> MlResult<Self> {
        if data.is_empty() {
            return Err(MlError::InvalidModel("empty training set".into()));
        }
        let feature_dim = data[0].len();
        if feature_dim == 0 {
            return Err(MlError::InvalidModel("zero-dim training data".into()));
        }
        if data.iter().any(|r| r.len() != feature_dim) {
            return Err(MlError::InvalidModel("ragged training rows".into()));
        }
        let sample_size = sample_size.min(data.len()).max(1);
        let height_limit = ((sample_size as f32).log2().ceil() as usize).max(1);

        let mut rng = Lcg::new(seed);
        let mut trees = Vec::with_capacity(n_trees);
        for _ in 0..n_trees {
            // Sample with replacement — adequate for small sample_size.
            let mut subset = Vec::with_capacity(sample_size);
            for _ in 0..sample_size {
                let idx = rng.next_usize() % data.len();
                subset.push(data[idx].clone());
            }
            trees.push(build_tree(&subset, 0, height_limit, &mut rng));
        }
        Ok(Self {
            feature_dim,
            sample_size: sample_size as u32,
            trees,
        })
    }

    /// Serialize to bytes for embedding via `include_bytes!`.
    pub fn to_bytes(&self) -> MlResult<Vec<u8>> {
        bincode::serialize(self).map_err(|e| MlError::Serialize(e.to_string()))
    }
}

/// Runtime-side scorer wrapping a trained [`IsolationForestModel`].
#[derive(Debug)]
pub struct IsolationForestScorer {
    model_id: String,
    model: IsolationForestModel,
    /// Pre-computed normalisation constant `c(sample_size)`.
    c_norm: f32,
}

impl IsolationForestScorer {
    /// Construct from a serialized [`IsolationForestModel`] (bincode bytes).
    pub fn from_bytes(model_id: &str, bytes: &[u8], feature_dim: usize) -> MlResult<Self> {
        let model: IsolationForestModel = bincode::deserialize(bytes)
            .map_err(|e| MlError::Deserialize(e.to_string()))?;
        if model.feature_dim != feature_dim {
            return Err(MlError::FeatureDimMismatch {
                expected: model.feature_dim,
                got: feature_dim,
            });
        }
        let c_norm = avg_path_length(model.sample_size as f32).max(1e-6);
        Ok(Self {
            model_id: model_id.to_string(),
            model,
            c_norm,
        })
    }

    /// Construct directly from an in-memory model (skips serialisation round-trip).
    pub fn from_model(model_id: &str, model: IsolationForestModel) -> MlResult<Self> {
        let c_norm = avg_path_length(model.sample_size as f32).max(1e-6);
        Ok(Self {
            model_id: model_id.to_string(),
            model,
            c_norm,
        })
    }
}

impl AnomalyScorer for IsolationForestScorer {
    fn score(&self, features: &[f32]) -> f32 {
        if features.len() != self.model.feature_dim {
            tracing::warn!(
                model = %self.model_id,
                expected = self.model.feature_dim,
                got = features.len(),
                "IsolationForestScorer: feature dim mismatch, returning 0",
            );
            return 0.0;
        }
        if self.model.trees.is_empty() {
            return 0.0;
        }
        let mut total_path = 0.0f32;
        for tree in &self.model.trees {
            total_path += path_length(tree, features, 0);
        }
        let avg_path = total_path / (self.model.trees.len() as f32);
        // Standard IF anomaly score: s(x, n) = 2 ^ (-E[h(x)] / c(n))
        // Score in [0, 1]; we map to [0, 100].
        let s = (-avg_path / self.c_norm).exp2();
        (s * 100.0).clamp(0.0, 100.0)
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn feature_dim(&self) -> usize {
        self.model.feature_dim
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Tiny linear-congruential RNG so the crate stays dep-free at runtime
/// (rand is dev-only). Numerical recipes parameters; do not use for
/// cryptographic purposes.
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        // Avoid the degenerate state == 0 case.
        Self {
            state: seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1),
        }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }
    fn next_usize(&mut self) -> usize {
        self.next_u64() as usize
    }
    fn next_f32(&mut self) -> f32 {
        // 24-bit mantissa precision is plenty.
        ((self.next_u64() >> 40) as f32) / ((1u32 << 24) as f32)
    }
}

fn build_tree(rows: &[Vec<f32>], depth: usize, height_limit: usize, rng: &mut Lcg) -> IfNode {
    if depth >= height_limit || rows.len() <= 1 {
        return IfNode::Leaf {
            size: rows.len() as u32,
        };
    }
    let feature_dim = rows[0].len();
    // Pick a random feature with a non-degenerate range; bail out if none.
    let mut feature = rng.next_usize() % feature_dim;
    let mut min_v = rows[0][feature];
    let mut max_v = min_v;
    for r in rows.iter() {
        let v = r[feature];
        if v < min_v {
            min_v = v;
        }
        if v > max_v {
            max_v = v;
        }
    }
    if (max_v - min_v).abs() < f32::EPSILON {
        // Try one more feature axis; if everything is constant, give up.
        for _ in 0..feature_dim {
            feature = (feature + 1) % feature_dim;
            min_v = rows[0][feature];
            max_v = min_v;
            for r in rows.iter() {
                let v = r[feature];
                if v < min_v {
                    min_v = v;
                }
                if v > max_v {
                    max_v = v;
                }
            }
            if (max_v - min_v).abs() >= f32::EPSILON {
                break;
            }
        }
        if (max_v - min_v).abs() < f32::EPSILON {
            return IfNode::Leaf {
                size: rows.len() as u32,
            };
        }
    }
    let threshold = min_v + rng.next_f32() * (max_v - min_v);
    let mut left_rows = Vec::new();
    let mut right_rows = Vec::new();
    for r in rows.iter() {
        if r[feature] < threshold {
            left_rows.push(r.clone());
        } else {
            right_rows.push(r.clone());
        }
    }
    if left_rows.is_empty() || right_rows.is_empty() {
        return IfNode::Leaf {
            size: rows.len() as u32,
        };
    }
    IfNode::Split {
        feature,
        threshold,
        left: Box::new(build_tree(&left_rows, depth + 1, height_limit, rng)),
        right: Box::new(build_tree(&right_rows, depth + 1, height_limit, rng)),
    }
}

fn path_length(node: &IfNode, x: &[f32], depth: usize) -> f32 {
    match node {
        IfNode::Leaf { size } => depth as f32 + avg_path_length(*size as f32),
        IfNode::Split {
            feature,
            threshold,
            left,
            right,
        } => {
            if x[*feature] < *threshold {
                path_length(left, x, depth + 1)
            } else {
                path_length(right, x, depth + 1)
            }
        }
    }
}

/// Average path length of an unsuccessful BST search — the standard
/// normalisation constant `c(n)` from the Liu / Ting / Zhou paper.
fn avg_path_length(n: f32) -> f32 {
    if n <= 1.0 {
        return 0.0;
    }
    let h = (n - 1.0).ln() + 0.5772156649; // Euler–Mascheroni constant
    2.0 * h - 2.0 * (n - 1.0) / n
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    #[test]
    fn null_scorer_returns_zero() {
        let s = NullScorer;
        assert_eq!(s.score(&[1.0, 2.0, 3.0]), 0.0);
        assert_eq!(s.score(&[]), 0.0);
        assert_eq!(s.model_id(), "null-v0");
        assert_eq!(s.feature_dim(), 0);
    }

    #[test]
    fn online_quantile_warmup_returns_zero() {
        let s = OnlineQuantileScorer::new("test", 3);
        // Below warmup threshold — should return 0 even on extreme values.
        for _ in 0..32 {
            let score = s.score(&[1.0, 2.0, 3.0]);
            assert_eq!(score, 0.0);
        }
    }

    #[test]
    fn online_quantile_flags_after_warmup() {
        let s = OnlineQuantileScorer::new("test", 2);
        // Feed 200 samples drawn from a tight Gaussian-ish distribution.
        let mut rng = StdRng::seed_from_u64(42);
        for _ in 0..200 {
            let x: f32 = rng.gen_range(-1.0..1.0);
            let y: f32 = rng.gen_range(-1.0..1.0);
            s.score(&[x, y]);
        }
        // Now hit it with a far-out-of-distribution sample.
        let outlier_score = s.score(&[100.0, 100.0]);
        assert!(
            outlier_score > 50.0,
            "expected outlier score >50, got {}",
            outlier_score
        );
    }

    #[test]
    fn online_quantile_dim_mismatch_returns_zero() {
        let s = OnlineQuantileScorer::new("test", 3);
        // Wrong feature dim — gracefully returns 0 without panicking.
        let score = s.score(&[1.0, 2.0]);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn isolation_forest_bincode_roundtrip() {
        // Tiny training set: a tight cluster around the origin in 4-D.
        let mut rng = StdRng::seed_from_u64(7);
        let data: Vec<Vec<f32>> = (0..256)
            .map(|_| {
                (0..4)
                    .map(|_| rng.gen_range(-0.5..0.5_f32))
                    .collect()
            })
            .collect();
        let model = IsolationForestModel::fit(&data, 32, 64, 1).unwrap();
        let bytes = model.to_bytes().unwrap();
        assert!(!bytes.is_empty());
        let scorer = IsolationForestScorer::from_bytes("if-test", &bytes, 4).unwrap();
        assert_eq!(scorer.model_id(), "if-test");
        assert_eq!(scorer.feature_dim(), 4);

        // In-distribution sample should score lower than a far outlier.
        let normal_score = scorer.score(&[0.1, -0.1, 0.0, 0.05]);
        let outlier_score = scorer.score(&[10.0, 10.0, 10.0, 10.0]);
        assert!(
            outlier_score > normal_score,
            "outlier score {} should exceed normal {}",
            outlier_score,
            normal_score
        );
    }

    #[test]
    fn isolation_forest_feature_dim_mismatch_errors() {
        let data: Vec<Vec<f32>> = (0..32).map(|i| vec![i as f32, (i * 2) as f32]).collect();
        let model = IsolationForestModel::fit(&data, 8, 16, 0).unwrap();
        let bytes = model.to_bytes().unwrap();
        let err = IsolationForestScorer::from_bytes("if", &bytes, 5).unwrap_err();
        assert!(matches!(err, MlError::FeatureDimMismatch { expected: 2, got: 5 }));
    }

    #[test]
    fn isolation_forest_score_in_range() {
        let mut rng = StdRng::seed_from_u64(11);
        let data: Vec<Vec<f32>> = (0..128)
            .map(|_| (0..3).map(|_| rng.gen_range(0.0..1.0_f32)).collect())
            .collect();
        let model = IsolationForestModel::fit(&data, 16, 32, 99).unwrap();
        let scorer = IsolationForestScorer::from_model("if", model).unwrap();
        for _ in 0..20 {
            let pt: Vec<f32> = (0..3).map(|_| rng.gen_range(-2.0..2.0_f32)).collect();
            let s = scorer.score(&pt);
            assert!((0.0..=100.0).contains(&s), "score out of range: {}", s);
        }
    }

    #[test]
    fn isolation_forest_dim_mismatch_score_returns_zero() {
        let data: Vec<Vec<f32>> = (0..32).map(|i| vec![i as f32, (i + 1) as f32]).collect();
        let model = IsolationForestModel::fit(&data, 8, 16, 0).unwrap();
        let scorer = IsolationForestScorer::from_model("if", model).unwrap();
        // Wrong number of features — should be a soft failure, not a panic.
        let s = scorer.score(&[1.0]);
        assert_eq!(s, 0.0);
    }

    #[test]
    fn isolation_forest_empty_training_errors() {
        let err = IsolationForestModel::fit(&[], 4, 8, 0).unwrap_err();
        assert!(matches!(err, MlError::InvalidModel(_)));
    }

    #[test]
    fn isolation_forest_ragged_training_errors() {
        let data: Vec<Vec<f32>> = vec![vec![1.0, 2.0], vec![1.0, 2.0, 3.0]];
        let err = IsolationForestModel::fit(&data, 4, 8, 0).unwrap_err();
        assert!(matches!(err, MlError::InvalidModel(_)));
    }
}

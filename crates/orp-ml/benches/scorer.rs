//! Throughput benchmarks for [`OnlineQuantileScorer`].
//!
//! Establishes a baseline for the post-refactor O(log N) implementation.
//! The previous clone-and-sort implementation was O(D × N log N) per call
//! with N up to 2048 and D ≈ 8; the audit flagged it as the bottleneck of
//! the entire stream pipeline at 1 k events/sec. We don't have the old
//! implementation in-tree to A/B against, but we record the new
//! steady-state cost so any future regression here is visible.
//!
//! Run with:
//!
//! ```sh
//! CARGO_TARGET_DIR=$ORP_TARGET cargo bench -p orp-ml --bench scorer
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use orp_ml::{AnomalyScorer, OnlineQuantileScorer};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn bench_score_steady_state(c: &mut Criterion) {
    // Match the production processor's feature-vector size (8-D kinematic
    // features) and the rolling window cap (2048).
    let feature_dim = 8usize;

    let mut group = c.benchmark_group("OnlineQuantileScorer::score");
    for &warmup in &[0usize, 64, 2048] {
        let scorer = OnlineQuantileScorer::new("bench", feature_dim);
        let mut rng = StdRng::seed_from_u64(0xBEEF_CAFE);

        // Pre-warm to the requested fill level so we're benching the
        // steady-state hot path, not the cold-start branch.
        for _ in 0..warmup {
            let f: Vec<f32> = (0..feature_dim)
                .map(|_| rng.gen_range(-1.0..1.0_f32))
                .collect();
            scorer.score(&f);
        }

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::from_parameter(warmup), &warmup, |b, _w| {
            let mut rng = StdRng::seed_from_u64(0xC0DE_F00D);
            b.iter(|| {
                let f: [f32; 8] = [
                    rng.gen_range(-1.0..1.0_f32),
                    rng.gen_range(-1.0..1.0_f32),
                    rng.gen_range(-1.0..1.0_f32),
                    rng.gen_range(-1.0..1.0_f32),
                    rng.gen_range(-1.0..1.0_f32),
                    rng.gen_range(-1.0..1.0_f32),
                    rng.gen_range(-1.0..1.0_f32),
                    rng.gen_range(-1.0..1.0_f32),
                ];
                black_box(scorer.score(black_box(&f)))
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_score_steady_state);
criterion_main!(benches);

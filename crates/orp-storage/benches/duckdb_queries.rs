//! DuckDB query-path benches for orp-storage.
//!
//! Pre-populates a 100k-row dataset (NOT 1M — keeps the bench under a minute
//! on dev hardware while still exercising the query planner past the
//! "everything in cache" regime). 1M can be re-enabled by setting the
//! `ORP_BENCH_LARGE` env var.
//!
//! Measured paths:
//!   * Point lookup by entity_id (typed-index path).
//!   * Range query by entity_type with ordering (drives the
//!     `last_updated` index).
//!   * Geo bounding-box / radius query (lat/lon fallback if spatial
//!     extension isn't loaded — most common case in single-binary builds).

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use orp_proto::Entity;
use orp_storage::traits::Storage;
use orp_storage::DuckDbStorage;
use std::collections::HashMap;
use std::sync::Arc;

fn dataset_size() -> usize {
    if std::env::var("ORP_BENCH_LARGE").is_ok() {
        1_000_000
    } else {
        100_000
    }
}

fn make_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn make_entity(i: usize) -> Entity {
    let mut properties = HashMap::new();
    properties.insert("mmsi".to_string(), serde_json::json!(200_000_000 + i));
    properties.insert("speed".to_string(), serde_json::json!((i % 30) as f32));
    Entity {
        entity_id: format!("ship-{:08}", i),
        entity_type: "ship".to_string(),
        canonical_id: None,
        name: Some(format!("Vessel-{}", i)),
        properties,
        confidence: 0.95,
        created_at: chrono::Utc::now(),
        last_updated: chrono::Utc::now(),
        geometry: Some(orp_proto::GeoPoint {
            // Spread points around Rotterdam in a ~1° box so radius queries
            // have varied selectivity.
            lat: 51.5 + (i as f64 * 0.000_011) % 1.0,
            lon: 3.5 + (i as f64 * 0.000_023) % 2.0,
            alt: None,
        }),
        is_active: true,
    }
}

fn populate(storage: Arc<DuckDbStorage>, n: usize) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        for i in 0..n {
            let e = make_entity(i);
            storage.insert_entity(&e).await.unwrap();
        }
    });
}

fn populated_storage() -> Arc<DuckDbStorage> {
    let storage = Arc::new(DuckDbStorage::new_in_memory().expect("open duckdb"));
    populate(storage.clone(), dataset_size());
    storage
}

// ── Benches ──────────────────────────────────────────────────────────────────

fn bench_point_lookup(c: &mut Criterion) {
    let rt = make_runtime();
    let storage = populated_storage();
    let n = dataset_size();
    let target_id = format!("ship-{:08}", n / 2);

    let mut group = c.benchmark_group("duckdb_query");
    group.throughput(Throughput::Elements(1));
    group.sample_size(50);
    group.bench_function("point_lookup_by_id", |b| {
        b.iter(|| {
            let target = target_id.clone();
            rt.block_on(async {
                let _ = black_box(storage.get_entity(&target).await.unwrap());
            });
        })
    });
    group.finish();
}

fn bench_range_query(c: &mut Criterion) {
    let rt = make_runtime();
    let storage = populated_storage();

    let mut group = c.benchmark_group("duckdb_query");
    // 1k results returned per call (LIMIT 1000) — typical UI list page.
    group.throughput(Throughput::Elements(1_000));
    group.sample_size(20);
    group.bench_function("get_entities_by_type_limit_1000", |b| {
        b.iter_batched(
            || (),
            |()| {
                rt.block_on(async {
                    let v = storage
                        .get_entities_by_type("ship", 1_000, 0)
                        .await
                        .unwrap();
                    black_box(v);
                });
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

fn bench_geo_radius(c: &mut Criterion) {
    let rt = make_runtime();
    let storage = populated_storage();

    let mut group = c.benchmark_group("duckdb_query");
    // 50 km radius around Rotterdam — selectivity ~few % of dataset.
    group.sample_size(20);
    group.bench_function("entities_in_radius_50km", |b| {
        b.iter(|| {
            rt.block_on(async {
                let v = storage
                    .get_entities_in_radius(51.92, 4.47, 50.0, Some("ship"))
                    .await
                    .unwrap();
                black_box(v);
            });
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_point_lookup,
    bench_range_query,
    bench_geo_radius
);
criterion_main!(benches);

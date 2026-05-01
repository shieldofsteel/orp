//! DuckDB write-path benches for orp-storage.
//!
//! Designed to expose the perf hot paths called out in
//! `project_orp_perf_hotpath.md`:
//!   * single global `Mutex<Connection>` serialising every writer
//!   * per-property INSERT loop (one statement per property, no batching)
//!
//! Each bench uses `tempfile::TempDir` for the on-disk DuckDB file; the
//! directory lives only for the lifetime of the bench so we never pollute
//! the working tree. `iter_batched` carries the setup cost outside the
//! measured region.

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use orp_proto::Entity;
use orp_storage::traits::Storage;
use orp_storage::DuckDbStorage;
use std::collections::HashMap;
use std::sync::Arc;

const N_INSERTS: usize = 1_000;
const PROPS_PER_ENTITY: usize = 10;
const N_THREADS: usize = 4;

fn make_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(N_THREADS)
        .enable_all()
        .build()
        .unwrap()
}

fn make_entity(id: usize, props: usize) -> Entity {
    let mut properties = HashMap::new();
    for k in 0..props {
        properties.insert(
            format!("prop_{}", k),
            serde_json::json!(format!("value_{}_{}", id, k)),
        );
    }
    Entity {
        entity_id: format!("ent-{:08}", id),
        entity_type: "ship".to_string(),
        canonical_id: None,
        name: Some(format!("Ship-{}", id)),
        properties,
        confidence: 0.95,
        created_at: chrono::Utc::now(),
        last_updated: chrono::Utc::now(),
        geometry: Some(orp_proto::GeoPoint {
            lat: 51.92 + (id as f64 * 0.0001),
            lon: 4.47 + (id as f64 * 0.0001),
            alt: None,
        }),
        is_active: true,
    }
}

fn fresh_storage() -> Arc<DuckDbStorage> {
    // In-memory keeps each iteration deterministic (no journal flush),
    // matching the docs claim that perf depends on the storage API itself
    // rather than disk I/O.
    Arc::new(DuckDbStorage::new_in_memory().expect("open in-memory duckdb"))
}

// ── Benches ──────────────────────────────────────────────────────────────────

fn bench_single_thread_insert(c: &mut Criterion) {
    let rt = make_runtime();
    let entities: Vec<Entity> = (0..N_INSERTS).map(|i| make_entity(i, 3)).collect();

    let mut group = c.benchmark_group("duckdb_insert");
    group.throughput(Throughput::Elements(N_INSERTS as u64));
    group.sample_size(10);
    group.bench_function("single_thread_1k_entities_3_props", |b| {
        b.iter_batched(
            fresh_storage,
            |storage| {
                rt.block_on(async {
                    for e in &entities {
                        storage.insert_entity(e).await.unwrap();
                    }
                });
                black_box(storage)
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

fn bench_concurrent_insert(c: &mut Criterion) {
    let rt = make_runtime();
    let entities: Vec<Entity> = (0..N_INSERTS).map(|i| make_entity(i, 3)).collect();
    let chunk = entities.len() / N_THREADS;

    let mut group = c.benchmark_group("duckdb_insert");
    group.throughput(Throughput::Elements(N_INSERTS as u64));
    group.sample_size(10);
    group.bench_function("concurrent_4threads_1k_entities_3_props", |b| {
        b.iter_batched(
            fresh_storage,
            |storage| {
                rt.block_on(async {
                    let mut handles = Vec::with_capacity(N_THREADS);
                    for t in 0..N_THREADS {
                        let s = storage.clone();
                        let slice: Vec<Entity> = entities
                            .iter()
                            .skip(t * chunk)
                            .take(chunk)
                            .cloned()
                            .collect();
                        handles.push(tokio::spawn(async move {
                            for e in &slice {
                                s.insert_entity(e).await.unwrap();
                            }
                        }));
                    }
                    for h in handles {
                        h.await.unwrap();
                    }
                });
                black_box(storage)
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

fn bench_per_property_loop(c: &mut Criterion) {
    // Exposes the per-property INSERT loop in `insert_entity`: 100 entities ×
    // 10 properties each = 1_000 property INSERTs from the inner loop. This
    // is the workload the v0.3.0 hot-path doc flags as the dominant cost
    // when ingesting AIS / ADS-B / MAVLink frames at line rate.
    let rt = make_runtime();
    let entities: Vec<Entity> = (0..100).map(|i| make_entity(i, PROPS_PER_ENTITY)).collect();

    let mut group = c.benchmark_group("duckdb_insert");
    group.throughput(Throughput::Elements((100 * PROPS_PER_ENTITY) as u64));
    group.sample_size(10);
    group.bench_function("per_property_loop_100x10", |b| {
        b.iter_batched(
            fresh_storage,
            |storage| {
                rt.block_on(async {
                    for e in &entities {
                        storage.insert_entity(e).await.unwrap();
                    }
                });
                black_box(storage)
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_single_thread_insert,
    bench_concurrent_insert,
    bench_per_property_loop
);
criterion_main!(benches);

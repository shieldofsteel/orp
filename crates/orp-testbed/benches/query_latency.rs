use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use orp_query::QueryExecutor;
use orp_storage::DuckDbStorage;
use orp_testbed::{generate_synthetic_ports, generate_synthetic_ships};
use std::sync::Arc;
use tokio::runtime::Runtime;

fn setup_storage_with_data(ship_count: usize, port_count: usize) -> Arc<DuckDbStorage> {
    let storage = Arc::new(DuckDbStorage::new_in_memory().expect("Failed to create DuckDB"));
    let rt = Runtime::new().unwrap();

    // Insert synthetic ships
    let ships = generate_synthetic_ships(ship_count);
    for ship in &ships {
        rt.block_on(storage.insert_entity(ship)).expect("insert ship");
    }

    // Insert synthetic ports
    let ports = generate_synthetic_ports(port_count);
    for port in &ports {
        rt.block_on(storage.insert_entity(port)).expect("insert port");
    }

    storage
}

fn bench_simple_query(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_latency");
    group.sample_size(50);

    for &entity_count in &[100, 500, 1000] {
        let storage = setup_storage_with_data(entity_count, 10);
        let executor = QueryExecutor::new(storage.clone());

        let rt = Runtime::new().unwrap();

        group.bench_with_input(
            BenchmarkId::new("simple_match", entity_count),
            &entity_count,
            |b, _| {
                b.iter(|| {
                    rt.block_on(async {
                        executor
                            .execute("MATCH (s:Ship) WHERE s.speed > 10 RETURN s.id, s.name, s.speed")
                            .await
                            .expect("query failed");
                    });
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("geospatial_near", entity_count),
            &entity_count,
            |b, _| {
                b.iter(|| {
                    rt.block_on(async {
                        executor
                            .execute(
                                "MATCH (s:Ship) WHERE NEAR(s, lat=51.9, lon=4.5, radius_km=100) RETURN s.id, s.name",
                            )
                            .await
                            .expect("query failed");
                    });
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("return_all", entity_count),
            &entity_count,
            |b, _| {
                b.iter(|| {
                    rt.block_on(async {
                        executor
                            .execute("MATCH (s:Ship) RETURN s.id, s.name")
                            .await
                            .expect("query failed");
                    });
                });
            },
        );
    }

    group.finish();
}

fn bench_entity_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("entity_lookup");
    group.sample_size(100);

    let storage = setup_storage_with_data(1000, 10);
    let rt = Runtime::new().unwrap();

    group.bench_function("get_entity_by_id", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _ = storage.get_entity("ship-500").await;
            });
        });
    });

    group.bench_function("get_entities_by_type", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _ = storage.get_entities_by_type("Ship", 100, 0).await;
            });
        });
    });

    group.bench_function("search_entities", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _ = storage.search_entities("Maersk", Some("Ship"), 50).await;
            });
        });
    });

    group.bench_function("count_entities", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _ = storage.count_entities().await;
            });
        });
    });

    group.finish();
}

// Use the Storage trait methods
use orp_storage::Storage;

criterion_group!(benches, bench_simple_query, bench_entity_lookup);
criterion_main!(benches);

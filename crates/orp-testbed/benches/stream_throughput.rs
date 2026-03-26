use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use orp_proto::{EventPayload, OrpEvent};
use orp_storage::DuckDbStorage;
use orp_stream::{DefaultStreamProcessor, RocksDbDedupWindow, StreamContext, StreamProcessor};
use std::sync::Arc;
use tokio::runtime::Runtime;

fn create_position_event(i: usize) -> OrpEvent {
    OrpEvent::new(
        format!("ship-{}", i % 1000),
        "Ship".to_string(),
        EventPayload::PositionUpdate {
            latitude: 50.0 + (i as f64 % 100.0) * 0.05,
            longitude: 2.0 + (i as f64 % 80.0) * 0.05,
            altitude: None,
            accuracy_meters: None,
            speed_knots: Some(10.0 + (i as f64 % 20.0)),
            heading_degrees: Some((i as f64 * 37.0) % 360.0),
            course_degrees: Some((i as f64 * 37.0) % 360.0),
        },
        "ais-bench".to_string(),
        0.95,
    )
}

fn make_processor() -> impl StreamProcessor {
    let storage = Arc::new(DuckDbStorage::new_in_memory().expect("create storage"));
    let dedup_dir = tempfile::TempDir::new().unwrap();
    let dedup = Arc::new(RocksDbDedupWindow::open(dedup_dir.path(), 60).unwrap());
    // Keep dedup_dir alive by leaking it (benchmark only)
    std::mem::forget(dedup_dir);
    DefaultStreamProcessor::new(storage, dedup, None, 100)
}

fn bench_stream_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("stream_throughput");
    group.sample_size(20);

    for &batch_count in &[100, 500, 1000] {
        group.throughput(Throughput::Elements(batch_count as u64));

        group.bench_with_input(
            BenchmarkId::new("process_events", batch_count),
            &batch_count,
            |b, &count| {
                let rt = Runtime::new().unwrap();

                b.iter(|| {
                    rt.block_on(async {
                        let processor = make_processor();

                        for i in 0..count {
                            let event = create_position_event(i);
                            let ctx = StreamContext {
                                event,
                                dedup_window_seconds: 60,
                                batch_size: 100,
                            };
                            let _ = processor.process_event(ctx).await;
                        }
                    });
                });
            },
        );
    }

    group.finish();
}

fn bench_dedup_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("dedup_overhead");
    group.sample_size(30);

    let rt = Runtime::new().unwrap();

    group.bench_function("with_duplicates", |b| {
        b.iter(|| {
            rt.block_on(async {
                let processor = make_processor();

                for i in 0..200 {
                    let mut event = create_position_event(i % 100);
                    if i >= 100 {
                        event.id = uuid::Uuid::now_v7();
                    }
                    let ctx = StreamContext {
                        event,
                        dedup_window_seconds: 60,
                        batch_size: 100,
                    };
                    let _ = processor.process_event(ctx).await;
                }
            });
        });
    });

    group.bench_function("unique_events", |b| {
        b.iter(|| {
            rt.block_on(async {
                let processor = make_processor();

                for i in 0..200 {
                    let event = create_position_event(i);
                    let ctx = StreamContext {
                        event,
                        dedup_window_seconds: 60,
                        batch_size: 100,
                    };
                    let _ = processor.process_event(ctx).await;
                }
            });
        });
    });

    group.finish();
}

fn bench_ingestion_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("ingestion_pipeline");
    group.sample_size(10);
    group.throughput(Throughput::Elements(1000));

    group.bench_function("full_pipeline_1000_events", |b| {
        let rt = Runtime::new().unwrap();

        b.iter(|| {
            rt.block_on(async {
                let processor = make_processor();

                for i in 0..1000 {
                    let event = create_position_event(i);
                    let ctx = StreamContext {
                        event,
                        dedup_window_seconds: 60,
                        batch_size: 50,
                    };
                    let _ = processor.process_event(ctx).await;
                }
            });
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_stream_throughput,
    bench_dedup_overhead,
    bench_ingestion_pipeline
);
criterion_main!(benches);

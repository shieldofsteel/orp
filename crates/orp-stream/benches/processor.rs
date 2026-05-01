//! End-to-end stream processor bench for orp-stream.
//!
//! Pipeline measured: dedup window (RocksDB) → signing → buffer → flush to
//! storage (in-memory DuckDB). Anomaly scorer is the default `NullScorer`
//! so we don't measure the ML stage here (it has its own bench in
//! `orp-ml` once that suite lands).
//!
//! Output: events/sec for a stream of 1k synthetic position updates.

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use orp_proto::{EventPayload, OrpEvent};
use orp_storage::DuckDbStorage;
use orp_stream::dedup::RocksDbDedupWindow;
use orp_stream::dlq::DeadLetterQueue;
use orp_stream::processor::{DefaultStreamProcessor, StreamContext, StreamProcessor};
use std::sync::Arc;
use tempfile::TempDir;

const N_EVENTS: usize = 1_000;

fn make_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn pos_event(i: usize) -> OrpEvent {
    OrpEvent::new(
        format!("ship-{:06}", i),
        "ship".to_string(),
        EventPayload::PositionUpdate {
            latitude: 51.92 + (i as f64 * 0.0001),
            longitude: 4.47 + (i as f64 * 0.0001),
            altitude: None,
            accuracy_meters: None,
            speed_knots: Some(12.0 + (i as f64 % 10.0)),
            heading_degrees: Some(180.0),
            course_degrees: Some(185.0),
        },
        "bench-source".to_string(),
        0.95,
    )
}

fn make_processor() -> (DefaultStreamProcessor, TempDir, TempDir) {
    let dedup_dir = TempDir::new().unwrap();
    let dlq_dir = TempDir::new().unwrap();
    let storage = Arc::new(DuckDbStorage::new_in_memory().unwrap());
    let dedup = Arc::new(RocksDbDedupWindow::open(dedup_dir.path(), 60).unwrap());
    let dlq = Arc::new(DeadLetterQueue::open(dlq_dir.path()).unwrap());
    let p = DefaultStreamProcessor::new(storage, dedup, Some(dlq), 64);
    (p, dedup_dir, dlq_dir)
}

fn bench_pipeline(c: &mut Criterion) {
    let rt = make_runtime();
    let mut group = c.benchmark_group("processor");
    group.throughput(Throughput::Elements(N_EVENTS as u64));
    group.sample_size(10);
    group.bench_function("end_to_end_1k_position_events", |b| {
        b.iter_batched(
            make_processor,
            |(processor, _dedup_dir, _dlq_dir)| {
                rt.block_on(async {
                    for i in 0..N_EVENTS {
                        let ctx = StreamContext {
                            event: pos_event(i),
                            dedup_window_seconds: 60,
                            batch_size: 64,
                        };
                        processor.process_event(ctx).await.unwrap();
                    }
                    processor.flush().await.unwrap();
                });
                black_box(processor);
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

criterion_group!(benches, bench_pipeline);
criterion_main!(benches);

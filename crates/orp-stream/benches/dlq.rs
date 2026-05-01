//! Dead Letter Queue benches for orp-stream.
//!
//! RocksDB-backed DLQ. Three workloads:
//!   * Enqueue 10k events.
//!   * Drain 10k events (retry + remove).
//!   * Concurrent enqueue + drain on 4 threads each.
//!
//! Tempfile lifetime spans the whole bench iteration so RocksDB has a
//! consistent on-disk footprint to flush against.

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use orp_stream::dlq::DeadLetterQueue;
use std::sync::Arc;
use std::thread;
use tempfile::TempDir;

const N_EVENTS: usize = 10_000;

fn fresh_dlq() -> (Arc<DeadLetterQueue>, TempDir) {
    let dir = TempDir::new().expect("tmpdir");
    let dlq = DeadLetterQueue::open(dir.path()).expect("open dlq");
    (Arc::new(dlq), dir)
}

fn bench_enqueue(c: &mut Criterion) {
    let mut group = c.benchmark_group("dlq");
    group.throughput(Throughput::Elements(N_EVENTS as u64));
    group.sample_size(10);
    group.bench_function("record_failure_10k", |b| {
        b.iter_batched(
            fresh_dlq,
            |(dlq, _dir)| {
                for i in 0..N_EVENTS {
                    let id = format!("evt-{:06}", i);
                    dlq.record_failure(&id, b"bench payload", "synthetic error")
                        .unwrap();
                }
                black_box(dlq);
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

fn bench_drain(c: &mut Criterion) {
    let mut group = c.benchmark_group("dlq");
    group.throughput(Throughput::Elements(N_EVENTS as u64));
    group.sample_size(10);
    group.bench_function("drain_via_retry_10k", |b| {
        b.iter_batched(
            || {
                let (dlq, dir) = fresh_dlq();
                for i in 0..N_EVENTS {
                    let id = format!("evt-{:06}", i);
                    dlq.record_failure(&id, b"payload", "err").unwrap();
                }
                (dlq, dir)
            },
            |(dlq, _dir)| {
                // retry_fn returns true → entry removed.
                let _ = dlq.retry_failed(N_EVENTS, |_| true).unwrap();
                black_box(dlq);
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

fn bench_concurrent_enqueue_drain(c: &mut Criterion) {
    let mut group = c.benchmark_group("dlq");
    group.throughput(Throughput::Elements((N_EVENTS * 2) as u64));
    group.sample_size(10);
    group.bench_function("concurrent_4producers_4drainers_10k", |b| {
        b.iter_batched(
            fresh_dlq,
            |(dlq, _dir)| {
                let producers = 4;
                let consumers = 4;
                let per_producer = N_EVENTS / producers;
                let mut threads = Vec::with_capacity(producers + consumers);
                for p in 0..producers {
                    let q = dlq.clone();
                    threads.push(thread::spawn(move || {
                        for i in 0..per_producer {
                            let id = format!("p{}-evt-{:06}", p, i);
                            q.record_failure(&id, b"payload", "err").unwrap();
                        }
                    }));
                }
                for _ in 0..consumers {
                    let q = dlq.clone();
                    threads.push(thread::spawn(move || {
                        // Each drainer pops a small batch; multiple iterations
                        // keep them busy while producers are still writing.
                        for _ in 0..32 {
                            let _ = q.retry_failed(64, |_| true).unwrap();
                        }
                    }));
                }
                for t in threads {
                    t.join().unwrap();
                }
                // Final drain to remove anything left over.
                let _ = dlq.retry_failed(N_EVENTS, |_| true).unwrap();
                black_box(dlq);
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_enqueue,
    bench_drain,
    bench_concurrent_enqueue_drain
);
criterion_main!(benches);

# ORP — Post-v0.2.0 Benchmark Baseline

Generated: 2026-05-01
Branch: `feat/v0.3.0-bench-suite`
Commit: (initial bench suite — v0.3.0 hardening wave 1)
Hardware: Apple M-series (`darwin/aarch64`)

This file records the starting numbers for the v0.3.0 benchmark suite. Regressions of >10% on any tracked metric should fail CI on perf-tagged PRs (see [`docs/BENCHES.md`](../docs/BENCHES.md)).

Numbers below are from a single-machine sanity run with `cargo bench -- --quick` on Apple M-series hardware (`darwin/aarch64`). They are intentionally captured at `--quick` (10-sample) rather than the default 100-sample setting so the baseline is reproducible in CI without 5-minute waits.

## Reproduce

```bash
git checkout feat/v0.3.0-bench-suite
cargo bench -p orp-connector --bench parsers -- --quick
cargo bench -p orp-storage --bench duckdb_writes -- --quick
cargo bench -p orp-storage --bench duckdb_queries -- --quick
cargo bench -p orp-stream --bench dlq -- --quick
cargo bench -p orp-stream --bench processor -- --quick
cargo bench -p orp-query --bench qparse -- --quick
```

## Parser throughput (orp-connector)

Captured 2026-05-01 on Apple M-series (`darwin/aarch64`), `--quick` mode (10 samples). Values shown are the criterion median; see the `[low high]` 95% CI for noise envelope.

| Bench | Workload | Metric | Initial baseline (median) | 95% CI |
|-------|---------|--------|---------------------------|--------|
| `nmea/parse_sentence_10k` | 10k mixed GGA/RMC/VTG sentences | MB/s | **77.9 MiB/s** | [74.8, 93.5] |
| `ais/parse_msg_types_1_4_5_9_18_27_10k` | 10k AIS sentences (msg types 1, 4, 5, 9, 18, 27) | MB/s | **57.3 MiB/s** | [54.1, 74.9] |
| `ais/decode_payload_only_10k_t1` | 10k pure 6-bit decode + type-1 layout | MB/s | **338.4 MiB/s** | [335.7, 349.8] |
| `cot/parse_xml_10k` | 10k ~1KB MIL-STD-2525 CoT messages | MB/s | **167.3 MiB/s** | [167.0, 168.6] |
| `mavlink/decode_v2_heartbeat_and_position_10k` | 10k MAVLink v2 frames (HB + GLOBAL_POSITION_INT) | MB/s | **135.6 MiB/s** | [134.6, 139.7] |
| `grib/section7_unpack_template_5_0_10k_msgs` | 10k Section-7 unpacks of a 256-point 16-bit packed grid | MB/s | **718.1 MiB/s** | [717.6, 720.1] |

## DuckDB writes (orp-storage)

| Bench | Workload | Metric | Initial baseline |
|-------|---------|--------|------------------|
| `duckdb_insert/single_thread_1k_entities_3_props` | 1k entities × 3 properties | inserts/sec | _see latest_quick_run.md_ |
| `duckdb_insert/concurrent_4threads_1k_entities_3_props` | 1k entities, 4 concurrent producers | inserts/sec | _see latest_quick_run.md_ |
| `duckdb_insert/per_property_loop_100x10` | 100 entities × 10 properties (per-prop INSERT loop) | properties/sec | _see latest_quick_run.md_ |

## DuckDB queries (orp-storage)

| Bench | Workload | Metric | Initial baseline |
|-------|---------|--------|------------------|
| `duckdb_query/point_lookup_by_id` | 1× lookup against a 100k-row table | µs/lookup | _see latest_quick_run.md_ |
| `duckdb_query/get_entities_by_type_limit_1000` | type filter + ORDER BY + LIMIT 1000 | µs/query | _see latest_quick_run.md_ |
| `duckdb_query/entities_in_radius_50km` | 50 km haversine radius query | µs/query | _see latest_quick_run.md_ |

For 1M-row variants, set `ORP_BENCH_LARGE=1`.

## Stream pipeline (orp-stream)

| Bench | Workload | Metric | Initial baseline |
|-------|---------|--------|------------------|
| `dlq/record_failure_10k` | 10k DLQ enqueues | events/sec | _see latest_quick_run.md_ |
| `dlq/drain_via_retry_10k` | 10k DLQ drains | events/sec | _see latest_quick_run.md_ |
| `dlq/concurrent_4producers_4drainers_10k` | 4×4 concurrent | events/sec | _see latest_quick_run.md_ |
| `processor/end_to_end_1k_position_events` | dedup → sign → buffer → DuckDB flush | events/sec | _see latest_quick_run.md_ |

## Query parser (orp-query)

| Bench | Workload | Metric | Initial baseline |
|-------|---------|--------|------------------|
| `orpql/parse_100_representative_queries` | 100 queries spanning simple/conjunction/aggregate/relationship | MB/s | _see latest_quick_run.md_ |

## Notes on what's expected to be slow today

These numbers are pre-optimisation. Three known bottlenecks (per `project_orp_perf_hotpath.md`) are intentionally exposed by this suite:

1. `single_thread_1k_entities_3_props` and `concurrent_4threads_1k_entities_3_props` will show poor scaling ratio — the `Mutex<Connection>` serialises every writer onto a single thread. Concurrent throughput should approach single-thread throughput, not 4× it.
2. `per_property_loop_100x10` is dominated by the `SELECT nextval(...) + INSERT` round-trip per property; a single multi-row INSERT (~30 LoC change) should give a 5–10× boost.
3. End-to-end stream throughput is bounded by 1 + 2 above plus the audit-log doubling on every flush.

Track those three numbers through v0.3.x — they are the intended unlocks.

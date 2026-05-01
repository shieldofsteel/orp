# Benchmarks — ORP

ORP uses [`criterion`](https://github.com/bheisler/criterion.rs) for performance benchmarks. This document covers how to run them, when to run them, and the policy for what counts as a regression.

## TL;DR

```bash
# All benches (5–10 min on a recent dev machine)
cargo bench --workspace

# A single crate
cargo bench -p orp-connector

# A single bench file
cargo bench -p orp-connector --bench parsers

# A single benchmark function (regex match)
cargo bench -p orp-connector --bench parsers -- nmea

# Quick mode — runs each measurement 10x instead of 100x; useful as a sanity check
cargo bench -p orp-connector --bench parsers -- --quick

# Save a named baseline for later comparison
cargo bench --workspace -- --save-baseline before-fix

# Compare against that baseline after a change
cargo bench --workspace -- --baseline before-fix
```

Criterion writes HTML reports to `target/criterion/report/index.html`.

## Layout

| Crate | Bench file | What it measures |
|-------|-----------|------------------|
| `orp-connector` | `benches/parsers.rs` | NMEA / AIS / CoT / MAVLink / GRIB parser throughput in MB/s |
| `orp-storage` | `benches/duckdb_writes.rs` | Single-thread, multi-thread, and per-property INSERT paths |
| `orp-storage` | `benches/duckdb_queries.rs` | Point lookup, range query, geo radius |
| `orp-stream` | `benches/dlq.rs` | DLQ enqueue / drain / concurrent |
| `orp-stream` | `benches/processor.rs` | End-to-end pipeline events/sec |
| `orp-query` | `benches/qparse.rs` | ORP-QL parser throughput |

All bench fixtures are generated at runtime — no on-disk fixtures, no test data drift.

## Single-binary discipline

`criterion` is a `[dev-dependencies]` only. It does **not** ship in the release binary. `cargo build --release` produces the same artifact whether or not you've ever run a bench.

## When to run benches

| Trigger | What to run |
|---------|------------|
| Local change to a parser / storage / pipeline hot path | `cargo bench -p <crate>` for the touched crate |
| PR labeled `perf` | full `cargo bench --workspace` against `main` baseline |
| Pre-release | full suite, archive output as the new baseline in `benches/baseline.md` |
| Investigating a perf regression report | re-run the affected bench with `--save-baseline before` against `main`, apply the suspected fix, re-run with `--baseline before` |

CI does **not** auto-run the full bench suite on every PR — the suite is too slow (5–10 min) and noisy on shared runners. Instead, perf-tagged PRs run a curated subset:

```bash
cargo bench -p orp-connector --bench parsers -- --noplot
cargo bench -p orp-storage --bench duckdb_writes -- --noplot
cargo bench -p orp-stream --bench dlq -- --noplot
```

## Regression policy

A PR is considered a perf regression if any of these tracked metrics worsen by more than **10%** vs the post-v0.2.0 baseline (`benches/baseline.md`):

| Metric | Source bench |
|--------|-------------|
| NMEA throughput, MB/s | `parsers::nmea/parse_sentence_10k` |
| AIS throughput, MB/s | `parsers::ais/parse_msg_types_*` |
| CoT throughput, MB/s | `parsers::cot/parse_xml_10k` |
| MAVLink throughput, MB/s | `parsers::mavlink/decode_v2_*` |
| GRIB Section 7 throughput, MB/s | `parsers::grib/section7_unpack_*` |
| Single-thread insert ops/sec | `duckdb_insert::single_thread_*` |
| Concurrent insert ops/sec | `duckdb_insert::concurrent_4threads_*` |
| Point lookup p50 (µs) | `duckdb_query::point_lookup_by_id` |
| ORP-QL parse MB/s | `orpql::parse_100_*` |

A PR that intentionally trades throughput for correctness (e.g. tightening signature verification) must update `benches/baseline.md` in the same commit, with a one-line note on **why** the new number is acceptable.

## Adding a new bench

1. Drop the file in `crates/<crate>/benches/<name>.rs`.
2. Add an entry to that crate's `Cargo.toml`:
   ```toml
   [[bench]]
   name = "<name>"
   harness = false
   ```
3. Use `criterion_group!` + `criterion_main!`, and prefer `Throughput::Bytes` so output is human-readable.
4. Setup work goes in `iter_batched`'s setup closure with `BatchSize::SmallInput`, never inside the measured iterator.
5. If your bench needs on-disk state, use `tempfile::TempDir` — never write under the source tree.
6. Run it once locally and update `benches/baseline.md`.

## Known hot paths covered (and not covered)

Covered by this suite:

- Parser throughput per protocol (5 protocols)
- DuckDB single-thread / concurrent / per-property INSERT
- DuckDB point / range / geo queries
- DLQ enqueue / drain / concurrent
- Stream processor end-to-end
- ORP-QL parse

**Not yet covered** (next-step worthy — see `project_orp_perf_hotpath.md`):

- `Storage::graph_query` per-call `GraphEngine::new` cost
- Audit-log write amplification on every ingest / query
- Federation outbox throughput
- WebSocket broadcast fan-out under N subscribers
- Adapter end-to-end (UDP socket → SourceEvent) for MAVLink / NMEA

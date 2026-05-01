//! ORP-QL parser bench for orp-query.
//!
//! 100 representative queries spanning simple → complex (geo functions,
//! aggregates, multi-relationship patterns). Reports queries/sec.
//!
//! Run a single bench:
//!   cargo bench -p orp-query --bench qparse

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use orp_query::parse_orpql;

fn build_query_corpus() -> Vec<String> {
    let mut q = Vec::with_capacity(100);

    // ── Simple MATCH/RETURN ──────────────────────────────────────────────────
    for i in 0..15 {
        q.push(format!("MATCH (s:Ship) WHERE s.speed > {} RETURN s.id", i));
    }
    for label in &["Ship", "Aircraft", "Port", "Sensor", "Person"] {
        q.push(format!("MATCH (e:{}) RETURN e.id, e.name LIMIT 100", label));
    }

    // ── Geospatial filters ───────────────────────────────────────────────────
    let geos = [
        "MATCH (s:Ship) WHERE NEAR(s, lat=51.9225, lon=4.2706, radius_km=50) RETURN s.id, s.name",
        "MATCH (s:Ship) WHERE NEAR(s, lat=40.7128, lon=-74.0060, radius_km=10) RETURN s.id",
        "MATCH (s:Ship) WHERE WITHIN(s, min_lat=48.0, min_lon=-5.0, max_lat=55.0, max_lon=10.0) RETURN s.id",
        "MATCH (a:Aircraft) WHERE NEAR(a, lat=37.6, lon=-122.4, radius_km=200) RETURN a.id, a.altitude",
        "MATCH (s:Ship) WHERE NEAR(s, lat=1.29, lon=103.85, radius_km=25) RETURN s.id",
    ];
    for g in &geos {
        q.push(g.to_string());
    }

    // ── Conjunctions / multiple predicates ───────────────────────────────────
    let conjs = [
        "MATCH (s:Ship) WHERE s.speed > 10 AND s.speed < 30 RETURN s.id",
        "MATCH (s:Ship) WHERE s.speed > 5 AND s.heading > 180 RETURN s.id, s.heading",
        "MATCH (s:Ship) WHERE s.is_active = true AND s.speed > 0 RETURN s.id, s.name",
        "MATCH (s:Ship) WHERE s.ship_type = \"tanker\" AND s.speed < 25 RETURN s.id",
        "MATCH (s:Ship) WHERE s.mmsi >= 200000000 AND s.mmsi < 300000000 RETURN s.id",
    ];
    for c in &conjs {
        q.push(c.to_string());
    }

    // ── ORDER BY / LIMIT ────────────────────────────────────────────────────
    let ordered = [
        "MATCH (s:Ship) RETURN s.id ORDER BY s.speed ASC",
        "MATCH (s:Ship) RETURN s.id ORDER BY s.speed DESC",
        "MATCH (a:Aircraft) RETURN a.id ORDER BY a.altitude DESC LIMIT 50",
        "MATCH (s:Ship) WHERE s.speed > 0 RETURN s.id ORDER BY s.speed DESC LIMIT 25",
        "MATCH (e:Event) RETURN e.id ORDER BY e.timestamp DESC LIMIT 100",
    ];
    for o in &ordered {
        q.push(o.to_string());
    }

    // ── Aggregates ──────────────────────────────────────────────────────────
    let aggs = [
        "MATCH (s:Ship) RETURN COUNT(s) as total",
        "MATCH (s:Ship) RETURN COUNT(s) as ship_count",
        "MATCH (s:Ship) RETURN AVG(s.speed) as avg_speed",
        "MATCH (s:Ship) RETURN MAX(s.speed) as fastest",
        "MATCH (s:Ship) RETURN MIN(s.speed) as slowest",
        "MATCH (s:Ship) RETURN SUM(s.speed) as total_speed",
        "MATCH (a:Aircraft) RETURN AVG(a.altitude) as avg_alt",
        "MATCH (s:Ship) WHERE s.ship_type = \"tanker\" RETURN COUNT(s) as tankers",
    ];
    for a in &aggs {
        q.push(a.to_string());
    }

    // ── Pattern with relationship traversal ─────────────────────────────────
    let rels = [
        "MATCH (s:Ship)-[:HEADING_TO]->(p:Port {name: \"Rotterdam\"}) RETURN s.id, s.name",
        "MATCH (s:Ship)-[:OWNED_BY]->(c:Company) RETURN s.id, c.name",
        "MATCH (a:Aircraft)-[:OPERATED_BY]->(o:Operator) RETURN a.icao, o.name",
        "MATCH (p:Person)-[:WORKS_AT]->(c:Company {name: \"Acme\"}) RETURN p.name",
        "MATCH (a:Asset)-[:LOCATED_IN]->(z:Zone {name: \"restricted\"}) RETURN a.id",
    ];
    for r in &rels {
        q.push(r.to_string());
    }

    // ── Property filters in MATCH ───────────────────────────────────────────
    let props = [
        "MATCH (s:Ship {mmsi: \"123456789\"}) RETURN s.id",
        "MATCH (a:Aircraft {icao: \"A1B2C3\"}) RETURN a.id, a.callsign",
        "MATCH (p:Port {country: \"NL\"}) RETURN p.id, p.name",
        "MATCH (s:Sensor {sensor_type: \"radar\"}) RETURN s.id",
        "MATCH (e:Event {event_type: \"alert_triggered\"}) RETURN e.id",
    ];
    for p in &props {
        q.push(p.to_string());
    }

    // ── Mixed-case / lowercased keywords (parser robustness path) ───────────
    let mixed = [
        "match (s:ship) where s.speed > 10 return s.id",
        "Match (S:Ship) WHERE s.speed > 5 Return s.id Order By s.speed Limit 100",
        "MATCH (s:Ship) RETURN s.id, s.name, s.speed, s.heading, s.lat, s.lon, s.confidence",
    ];
    for m in &mixed {
        q.push(m.to_string());
    }

    // ── Return raw entity / no entity_type ──────────────────────────────────
    q.push("MATCH (s) RETURN s".to_string());
    q.push("MATCH (s:Ship) RETURN s".to_string());

    // Pad to exactly 100.
    while q.len() < 100 {
        q.push(format!(
            "MATCH (s:Ship) WHERE s.speed > {} RETURN s.id LIMIT {}",
            q.len(),
            q.len()
        ));
    }
    q.truncate(100);
    q
}

fn bench_qparse(c: &mut Criterion) {
    let queries = build_query_corpus();
    let total_bytes: u64 = queries.iter().map(|q| q.len() as u64).sum();

    let mut group = c.benchmark_group("orpql");
    group.throughput(Throughput::Bytes(total_bytes));
    group.sample_size(50);
    group.bench_function("parse_100_representative_queries", |b| {
        b.iter(|| {
            for q in &queries {
                let _ = black_box(parse_orpql(q));
            }
        })
    });
    group.finish();
}

criterion_group!(benches, bench_qparse);
criterion_main!(benches);

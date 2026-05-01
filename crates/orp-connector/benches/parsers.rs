//! Parser hot-path benches for orp-connector.
//!
//! Each parser is exercised over 10k synthetic-but-realistic records.
//! `Throughput::Bytes` is configured so criterion reports MB/s — the metric
//! we care about for ingest scaling.
//!
//! Coverage:
//!   * NMEA RMC / GGA / VTG sentences (the AIS-feeding surface format)
//!   * AIS message types 1, 4, 5, 9, 18, 27 (binary-armored 6-bit payloads)
//!   * CoT XML (typical ~1KB MIL-STD-2525 message)
//!   * MAVLink HEARTBEAT + GLOBAL_POSITION_INT (binary v2 frames)
//!   * GRIB Section 7 (synthetic — Section-7 unpacker is the hot path)
//!
//! Run a single bench:
//!   cargo bench -p orp-connector --bench parsers -- nmea
//!
//! All fixtures are generated at runtime (no on-disk fixtures); this keeps the
//! repo small and avoids the "test data drifted" maintenance burden.

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use mavlink::common::{
    MavAutopilot, MavModeFlag, MavState, MavType, GLOBAL_POSITION_INT_DATA, HEARTBEAT_DATA,
};
use mavlink::{write_v2_msg, MavHeader};
use orp_connector::adapters::cot::parse_cot_xml;
use orp_connector::adapters::grib::{unpack_template_5_0, GribDataRepresentation};
use orp_connector::adapters::mavlink::MavlinkConnector;
use orp_connector::adapters::nmea::{ais_decoder, parse_sentence, AisAssembler};

const N_RECORDS: usize = 10_000;

// ── NMEA fixtures ────────────────────────────────────────────────────────────

fn nmea_compute_checksum(body: &str) -> u8 {
    let mut cs: u8 = 0;
    for b in body.bytes() {
        cs ^= b;
    }
    cs
}

fn nmea_sentence(body: &str) -> String {
    format!("${}*{:02X}", body, nmea_compute_checksum(body))
}

fn build_nmea_sentences() -> Vec<String> {
    // Mix of GGA / RMC / VTG, with slightly perturbed coords so the parser
    // doesn't get caught short-circuiting on identical fixtures.
    let mut out = Vec::with_capacity(N_RECORDS);
    for i in 0..N_RECORDS {
        let lat_min = 21.6802 + (i as f64 * 0.0001) % 1.0;
        let kind = i % 3;
        let body = match kind {
            0 => format!(
                "GPGGA,092750.000,53{:.4},N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,",
                lat_min
            ),
            1 => format!(
                "GPRMC,092750.000,A,53{:.4},N,00630.3372,W,0.02,31.66,280511,,,A",
                lat_min
            ),
            _ => "GPVTG,054.7,T,034.4,M,005.5,N,010.2,K".to_string(),
        };
        out.push(nmea_sentence(&body));
    }
    out
}

// ── AIS fixtures ─────────────────────────────────────────────────────────────

/// Pack `(value, bits)` fields MSB-first into a 6-bit AIVDM payload.
fn pack_aivdm_payload(fields: &[(u64, usize)]) -> (String, u8) {
    let mut bits: Vec<bool> = Vec::new();
    for (val, w) in fields {
        for shift in (0..*w).rev() {
            bits.push((val >> shift) & 1 == 1);
        }
    }
    let total = bits.len();
    let pad = (6 - (total % 6)) % 6;
    let mut s = String::with_capacity((total + pad) / 6);
    for chunk in 0..(total + pad) / 6 {
        let mut v = 0u8;
        for j in 0..6 {
            let i = chunk * 6 + j;
            v = (v << 1) | (if i < total && bits[i] { 1 } else { 0 });
        }
        s.push(if v < 40 {
            (v + 48) as char
        } else {
            (v + 56) as char
        });
    }
    (s, pad as u8)
}

fn aivdm_sentence(fields: &[(u64, usize)]) -> String {
    let (payload, fill) = pack_aivdm_payload(fields);
    let body = format!("AIVDM,1,1,,A,{},{}", payload, fill);
    format!("!{}*{:02X}", body, nmea_compute_checksum(&body))
}

fn signed_mask(value: i64, bits: usize) -> u64 {
    (value as u64) & ((1u64 << bits) - 1)
}

/// Build one fixture per AIS msg type that we currently support.
fn build_ais_fixtures() -> [String; 6] {
    // Type 1 — Class A position report.
    let t1 = aivdm_sentence(&[
        (1, 6),
        (0, 2),
        (211_378_120, 30),
        (0, 4),
        (0, 8),
        (125, 10),                         // sog 12.5 kn
        (1, 1),                            // pos accuracy
        (signed_mask(2_692_752, 28), 28),  // lon 4.487920 (×600000)
        (signed_mask(31_152_000, 27), 27), // lat 51.92 (×600000)
        (2450, 12),                        // cog 245.0
        (245, 9),                          // heading
        (0, 6),
        (0, 4),
        (0, 1),
        (0, 19),
    ]);

    // Type 4 — base station report.
    let t4 = aivdm_sentence(&[
        (4, 6),
        (0, 2),
        (3_660_057, 30),
        (2024, 14),
        (3, 4),
        (15, 5),
        (12, 5),
        (34, 6),
        (56, 6),
        (1, 1),
        (signed_mask(-73_500_000, 28), 28),
        (signed_mask(22_500_000, 27), 27),
        (1, 4),
        (0, 10),
        (0, 1),
        (0, 19),
    ]);

    // Type 5 — voyage / static data (multi-part in real life; here we keep
    // it as a single 426-bit payload so the harness measures decode work).
    let t5 = aivdm_sentence(&[
        (5, 6),
        (0, 2),
        (211_378_120, 30),
        (0, 2),
        (9_876_543, 30),
        // call sign (7 chars × 6 bits)
        (b'A' as u64 - 64, 6),
        (b'B' as u64 - 64, 6),
        (b'C' as u64 - 64, 6),
        (b'1' as u64, 6),
        (b'2' as u64, 6),
        (b'3' as u64, 6),
        (0, 6),
        // vessel name (20 chars × 6 bits) — "MAERSK SEATRADE   "
        (b'M' as u64 - 64, 6),
        (b'A' as u64 - 64, 6),
        (b'E' as u64 - 64, 6),
        (b'R' as u64 - 64, 6),
        (b'S' as u64 - 64, 6),
        (b'K' as u64 - 64, 6),
        (b' ' as u64, 6),
        (b'S' as u64 - 64, 6),
        (b'E' as u64 - 64, 6),
        (b'A' as u64 - 64, 6),
        (b'T' as u64 - 64, 6),
        (b'R' as u64 - 64, 6),
        (b'A' as u64 - 64, 6),
        (b'D' as u64 - 64, 6),
        (b'E' as u64 - 64, 6),
        (0, 6),
        (0, 6),
        (0, 6),
        (0, 6),
        (0, 6),
        (71, 8), // ship_type = container
        (0, 32),
        (0, 4),
        (12, 4), // ETA month
        (15, 5), // ETA day
        (12, 5), // ETA hr
        (0, 6),  // ETA min
        (75, 8), // draught 7.5m
        // destination (20 chars × 6 bits)
        (b'R' as u64 - 64, 6),
        (b'O' as u64 - 64, 6),
        (b'T' as u64 - 64, 6),
        (b'T' as u64 - 64, 6),
        (b'E' as u64 - 64, 6),
        (b'R' as u64 - 64, 6),
        (b'D' as u64 - 64, 6),
        (b'A' as u64 - 64, 6),
        (b'M' as u64 - 64, 6),
        (0, 6),
        (0, 6),
        (0, 6),
        (0, 6),
        (0, 6),
        (0, 6),
        (0, 6),
        (0, 6),
        (0, 6),
        (0, 6),
        (0, 6),
        (0, 1), // dte
        (0, 1), // spare
    ]);

    // Type 9 — SAR aircraft.
    let t9 = aivdm_sentence(&[
        (9, 6),
        (0, 2),
        (111_222_333, 30),
        (800, 12),
        (200, 10),
        (1, 1),
        (signed_mask(600_000, 28), 28),
        (signed_mask(30_300_000, 27), 27),
        (2700, 12),
        (30, 6),
        (0, 8),
        (0, 1),
        (0, 3),
        (0, 1),
        (1, 1),
        (0, 20),
    ]);

    // Type 18 — Class B position report.
    let t18 = aivdm_sentence(&[
        (18, 6),
        (0, 2),
        (244_820_583, 30),
        (0, 8),   // reserved
        (83, 10), // sog 8.3 kn
        (1, 1),
        (signed_mask(2_592_000, 28), 28),  // lon 4.32 (×600000)
        (signed_mask(31_134_000, 27), 27), // lat 51.89 (×600000)
        (1800, 12),                        // cog 180.0
        (180, 9),                          // heading
        (0, 6),
        (0, 2),
        (0, 1),
        (0, 1),
        (0, 1),
        (0, 1),
        (0, 1),
        (0, 1),
        (0, 20),
    ]);

    // Type 27 — long-range AIS broadcast (96 bits).
    let t27 = aivdm_sentence(&[
        (27, 6),
        (0, 2),
        (305_160_000, 30),
        (1, 1),
        (0, 1),
        (3, 4),
        (signed_mask(2_550, 18), 18),  // lon (4.25)
        (signed_mask(31_080, 17), 17), // lat (51.80)
        (5, 6),
        (315, 9),
        (0, 1),
        (0, 1),
    ]);

    [t1, t4, t5, t9, t18, t27]
}

fn build_ais_dataset(fixtures: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(N_RECORDS);
    for i in 0..N_RECORDS {
        out.push(fixtures[i % fixtures.len()].clone());
    }
    out
}

// ── CoT fixtures ─────────────────────────────────────────────────────────────

fn build_cot_xml() -> String {
    // Typical ~1KB MIL-STD-2525 CoT message used by ATAK / WinTAK.
    r#"<?xml version="1.0" encoding="UTF-8"?>
<event version="2.0" uid="ALPHA-1" type="a-f-G-U-C" how="m-g"
       time="2026-03-26T12:00:00Z" start="2026-03-26T12:00:00Z"
       stale="2026-03-26T12:05:00Z">
  <point lat="38.8977" lon="-77.0365" hae="50.0" ce="10.0" le="5.0" />
  <detail>
    <contact callsign="Alpha Squad" />
    <__group name="Cyan" role="Team Lead" />
    <status battery="85" readiness="true" />
    <track course="270.5" speed="3.2" />
    <takv version="4.10.0.0" platform="ATAK-CIV" device="Pixel 7" os="Android 14" />
    <precisionlocation altsrc="GPS" geopointsrc="GPS" />
    <remarks>On patrol near objective; observed 2 unknown contacts moving north-east at 5kts. Maintaining standoff distance per ROE. Will report further contact updates every 60 seconds. Battery at 85%, comms green.</remarks>
    <link relation="p-p" type="a-f-G-U-C" uid="BRAVO-1" parent_callsign="Bravo Squad" />
  </detail>
</event>"#
        .to_string()
}

fn build_cot_dataset() -> Vec<String> {
    let xml = build_cot_xml();
    (0..N_RECORDS).map(|_| xml.clone()).collect()
}

// ── MAVLink fixtures ─────────────────────────────────────────────────────────

fn encode_v2(hdr: MavHeader, msg: &mavlink::common::MavMessage) -> Vec<u8> {
    let mut buf = Vec::with_capacity(280);
    write_v2_msg(&mut buf, hdr, msg).expect("encode mavlink v2 frame");
    buf
}

fn build_mavlink_frames() -> Vec<Vec<u8>> {
    let hdr = MavHeader {
        system_id: 1,
        component_id: 1,
        sequence: 0,
    };

    let heartbeat = mavlink::common::MavMessage::HEARTBEAT(HEARTBEAT_DATA {
        custom_mode: 4,
        mavtype: MavType::MAV_TYPE_QUADROTOR,
        autopilot: MavAutopilot::MAV_AUTOPILOT_PX4,
        base_mode: MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED
            | MavModeFlag::MAV_MODE_FLAG_AUTO_ENABLED,
        system_status: MavState::MAV_STATE_ACTIVE,
        mavlink_version: 3,
    });

    let pos = mavlink::common::MavMessage::GLOBAL_POSITION_INT(GLOBAL_POSITION_INT_DATA {
        time_boot_ms: 123_456,
        lat: 473_977_419,
        lon: 85_455_934,
        alt: 488_123,
        relative_alt: 12_345,
        vx: 250,
        vy: -100,
        vz: 50,
        hdg: 9250,
    });

    let hb_bytes = encode_v2(hdr, &heartbeat);
    let pos_bytes = encode_v2(hdr, &pos);

    let mut out = Vec::with_capacity(N_RECORDS);
    for i in 0..N_RECORDS {
        if i % 2 == 0 {
            out.push(hb_bytes.clone());
        } else {
            out.push(pos_bytes.clone());
        }
    }
    out
}

// ── GRIB Section 7 fixtures ──────────────────────────────────────────────────

/// Build a Section 5 + Section 7 fixture: 256 packed 16-bit values
/// representing temperature samples on a small grid.
fn build_grib_section7() -> (GribDataRepresentation, Vec<u8>, usize) {
    let n: usize = 256; // 16x16 grid
    let drep = GribDataRepresentation {
        num_data_points: n as u32,
        template_number: 0,
        reference_value: 273.15,
        binary_scale_factor: 0,
        decimal_scale_factor: 2,
        bits_per_value: 16,
    };
    // Pack n values as big-endian u16s with simple synthetic temp variation.
    let mut packed: Vec<u8> = Vec::with_capacity(n * 2);
    for i in 0..n {
        let v: u16 = ((i as u32 * 37) % 65535) as u16;
        packed.extend_from_slice(&v.to_be_bytes());
    }
    (drep, packed, n)
}

// ── Bench harnesses ──────────────────────────────────────────────────────────

fn bench_nmea(c: &mut Criterion) {
    let sentences = build_nmea_sentences();
    let total_bytes: u64 = sentences.iter().map(|s| s.len() as u64).sum();

    let mut group = c.benchmark_group("nmea");
    group.throughput(Throughput::Bytes(total_bytes));
    group.sample_size(20);
    group.bench_function("parse_sentence_10k", |b| {
        b.iter_batched(
            || (sentences.clone(), AisAssembler::default()),
            |(sentences, mut ais)| {
                for s in &sentences {
                    let _ = black_box(parse_sentence(s, &mut ais));
                }
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

fn bench_ais(c: &mut Criterion) {
    let fixtures = build_ais_fixtures();
    let dataset = build_ais_dataset(&fixtures);
    let total_bytes: u64 = dataset.iter().map(|s| s.len() as u64).sum();

    let mut group = c.benchmark_group("ais");
    group.throughput(Throughput::Bytes(total_bytes));
    group.sample_size(20);
    group.bench_function("parse_msg_types_1_4_5_9_18_27_10k", |b| {
        b.iter_batched(
            || (dataset.clone(), AisAssembler::default()),
            |(dataset, mut ais)| {
                for s in &dataset {
                    let _ = black_box(parse_sentence(s, &mut ais));
                }
            },
            BatchSize::SmallInput,
        )
    });
    // Pure binary decode path (skips checksum + sentence framing) — isolates
    // the 6-bit decoder cost so we can spot regressions in the bit-buffer
    // logic separately from the NMEA framing.
    group.bench_function("decode_payload_only_10k_t1", |b| {
        // Strip down to the inner payload of a synthetic type-1 fixture.
        let s = &fixtures[0];
        // !AIVDM,1,1,,A,<payload>,<fill>*<cs>
        let parts: Vec<&str> = s.trim_start_matches('!').split(',').collect();
        let payload = parts[5].to_string();
        let fill: u8 = parts[6].split('*').next().unwrap().parse().unwrap();
        b.iter(|| {
            for _ in 0..N_RECORDS {
                let bytes = ais_decoder::decode_payload(&payload, fill).unwrap();
                let total_bits = payload.len() * 6 - fill as usize;
                let buf = ais_decoder::make_buffer(&bytes, total_bits);
                let _ = black_box(ais_decoder::decode_type_1_2_3(&buf));
            }
        })
    });
    group.finish();
}

fn bench_cot(c: &mut Criterion) {
    let dataset = build_cot_dataset();
    let total_bytes: u64 = dataset.iter().map(|s| s.len() as u64).sum();

    let mut group = c.benchmark_group("cot");
    group.throughput(Throughput::Bytes(total_bytes));
    group.sample_size(20);
    group.bench_function("parse_xml_10k", |b| {
        b.iter(|| {
            for s in &dataset {
                let _ = black_box(parse_cot_xml(s));
            }
        })
    });
    group.finish();
}

fn bench_mavlink(c: &mut Criterion) {
    let frames = build_mavlink_frames();
    let total_bytes: u64 = frames.iter().map(|f| f.len() as u64).sum();

    let mut group = c.benchmark_group("mavlink");
    group.throughput(Throughput::Bytes(total_bytes));
    group.sample_size(20);
    group.bench_function("decode_v2_heartbeat_and_position_10k", |b| {
        b.iter(|| {
            for f in &frames {
                let _ = black_box(MavlinkConnector::decode_v2_datagram(f));
            }
        })
    });
    group.finish();
}

fn bench_grib(c: &mut Criterion) {
    let (drep, payload, n) = build_grib_section7();
    let total_bytes_per_iter = (payload.len() * N_RECORDS) as u64;

    let mut group = c.benchmark_group("grib");
    group.throughput(Throughput::Bytes(total_bytes_per_iter));
    group.sample_size(20);
    group.bench_function("section7_unpack_template_5_0_10k_msgs", |b| {
        b.iter(|| {
            for _ in 0..N_RECORDS {
                let v = unpack_template_5_0(&drep, &payload, n).unwrap();
                let _ = black_box(v);
            }
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_nmea,
    bench_ais,
    bench_cot,
    bench_mavlink,
    bench_grib
);
criterion_main!(benches);

//! Integration test: walk the entire KLOT fixture through the loop
//! and confirm we land on the same messages danielway's parser
//! reports. Gated on `RADISH_NEXRAD_FIXTURE_DIR` so CI without the
//! corpus simply skips. Marked `#[ignore]` so default `cargo test`
//! doesn't pay for the bzip2 round-trip.

use std::path::PathBuf;

use super::messages::decode_messages;
use super::record::{decompress_all, split_ldm_records};

fn klot_fixture() -> Option<PathBuf> {
    let dir = std::env::var_os("RADISH_NEXRAD_FIXTURE_DIR")?;
    let candidate = PathBuf::from(dir).join("KLOT20251210_102338_V06");
    candidate.is_file().then_some(candidate)
}

#[test]
#[ignore = "needs RADISH_NEXRAD_FIXTURE_DIR"]
fn walks_klot_fixture_and_finds_msg31_records() {
    let Some(path) = klot_fixture() else {
        eprintln!("skipping: RADISH_NEXRAD_FIXTURE_DIR not set");
        return;
    };
    let bytes = std::fs::read(&path).expect("read fixture");

    // Skip the 24-byte AR2V volume header, split LDM records, decompress.
    let records = split_ldm_records(&bytes).expect("split");
    assert!(!records.is_empty(), "expected at least one LDM record");
    let payloads = decompress_all(&records).expect("decompress");

    // Count messages per type across all records.
    let mut total_messages = 0usize;
    let mut msg31_count = 0usize;
    let mut msg2_count = 0usize;
    let mut msg5_count = 0usize;
    for (record_idx, payload) in payloads.iter().enumerate() {
        let messages = decode_messages(payload)
            .unwrap_or_else(|e| panic!("record {record_idx}: decode_messages failed: {e}"));
        total_messages += messages.len();
        for msg in &messages {
            use super::header::MessageType::*;
            match msg.header.message_type {
                DigitalRadarDataGenericFormat => msg31_count += 1,
                RdaStatusData => msg2_count += 1,
                VolumeCoveragePattern => msg5_count += 1,
                _ => {}
            }
        }
    }

    // KLOT VCP-212 fixture observed counts on a green run:
    //   61 records, 7206 total messages, 7200 MSG_31, 1 MSG_2, 1 MSG_5.
    // Each NEXRAD volume has exactly one MSG_2 (RDA status) and one
    // MSG_5 (VCP definition). MSG_31 count varies by file but is
    // roughly num_radials_per_sweep × num_sweeps. Tightened
    // assertions catch regressions where the loop stops early on
    // trailing zero-padding (pre-fix: stopped after ~6 messages).
    eprintln!(
        "KLOT fixture summary: {} records, {} total messages \
         ({} MSG_31, {} MSG_2, {} MSG_5)",
        payloads.len(),
        total_messages,
        msg31_count,
        msg2_count,
        msg5_count,
    );
    assert!(
        msg31_count >= 6000,
        "expected ≥6000 MSG_31 messages, found {msg31_count}"
    );
    assert_eq!(msg2_count, 1, "exactly one MSG_2 (RDA status) per volume");
    assert_eq!(
        msg5_count, 1,
        "exactly one MSG_5 (VCP definition) per volume"
    );
    // Sanity: typed counts should sum near total. Anything else
    // is a Skip(_) — small finite count is fine.
    assert!(
        msg31_count + msg2_count + msg5_count <= total_messages,
        "typed counts exceed total — accumulator bug"
    );
}

/// Phase 3: confirm we don't just *count* MSG_31s but actually
/// parse them through `msg31::parse`. Sample the first MSG_31 in
/// the file and verify its header fields decode to plausible
/// values matching the KLOT fixture.
#[test]
#[ignore = "needs RADISH_NEXRAD_FIXTURE_DIR"]
fn typed_msg31_parser_on_klot_fixture_yields_plausible_radials() {
    use super::messages::MessagePayload;

    let Some(path) = klot_fixture() else {
        eprintln!("skipping: RADISH_NEXRAD_FIXTURE_DIR not set");
        return;
    };
    let bytes = std::fs::read(&path).expect("read fixture");
    let records = split_ldm_records(&bytes).expect("split");
    let payloads = decompress_all(&records).expect("decompress");

    let mut samples = Vec::new();
    'outer: for payload in &payloads {
        let messages = decode_messages(payload).expect("decode");
        for msg in messages {
            if let MessagePayload::Msg31(boxed) = msg.payload {
                samples.push(*boxed);
                if samples.len() >= 3 {
                    break 'outer;
                }
            }
        }
    }

    assert!(
        samples.len() >= 3,
        "expected ≥3 MSG_31 samples, got {}",
        samples.len()
    );

    for (i, m) in samples.iter().enumerate() {
        // ICAO must be 4 ASCII chars from the KLOT fixture's filename.
        assert_eq!(
            &m.header.radar_identifier, b"KLOT",
            "sample {i} radar_identifier"
        );
        // Modified Julian date for 2025-12-10 = 20_433.
        assert_eq!(m.header.modified_julian_date, 20_433);
        // Plausible radial collection time (within a single day).
        assert!(
            m.header.collection_time_ms < 86_400_000,
            "sample {i} collection_time_ms out of range: {}",
            m.header.collection_time_ms
        );
        // Plausible radial physical fields.
        assert!(
            (0.0..=360.0).contains(&m.header.azimuth_angle_degrees),
            "sample {i} azimuth_angle out of range: {}",
            m.header.azimuth_angle_degrees
        );
        assert!(
            (-1.0..=25.0).contains(&m.header.elevation_angle_degrees),
            "sample {i} elevation_angle out of range: {}",
            m.header.elevation_angle_degrees
        );
        // KLOT VCP-212 is a precip mode — every radial carries REF.
        assert!(
            m.reflectivity.is_some(),
            "sample {i} should have reflectivity block"
        );
        // First radial in any volume carries the volume info block.
        if i == 0 {
            assert!(m.volume.is_some(), "first MSG_31 must carry VOL block");
            let v = m.volume.unwrap();
            // KLOT is at ~41.6N, -88.1W per public archive.
            assert!(
                (40.0..=43.0).contains(&v.latitude_degrees),
                "first radial VOL latitude out of range: {}",
                v.latitude_degrees
            );
            assert!(
                (-90.0..=-86.0).contains(&v.longitude_degrees),
                "first radial VOL longitude out of range: {}",
                v.longitude_degrees
            );
        }
    }
}

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

/// Phase 4: confirm typed MSG_2 + MSG_5 parsers fire on the KLOT
/// fixture and produce plausible values. Each volume has exactly
/// one MSG_2 and one MSG_5; we extract both and pin a few fields.
#[test]
#[ignore = "needs RADISH_NEXRAD_FIXTURE_DIR"]
fn typed_msg2_and_msg5_parsers_on_klot_fixture_yield_plausible_values() {
    use super::messages::MessagePayload;

    let Some(path) = klot_fixture() else {
        eprintln!("skipping: RADISH_NEXRAD_FIXTURE_DIR not set");
        return;
    };
    let bytes = std::fs::read(&path).expect("read fixture");
    let records = split_ldm_records(&bytes).expect("split");
    let payloads = decompress_all(&records).expect("decompress");

    let mut msg2 = None;
    let mut msg5 = None;
    for payload in &payloads {
        let messages = decode_messages(payload).expect("decode");
        for msg in messages {
            match msg.payload {
                MessagePayload::Msg2(boxed) if msg2.is_none() => msg2 = Some(*boxed),
                MessagePayload::Msg5(boxed) if msg5.is_none() => msg5 = Some(*boxed),
                _ => {}
            }
        }
    }

    let m2 = msg2.expect("KLOT volume must carry MSG_2");
    let m5 = msg5.expect("KLOT volume must carry MSG_5");

    // MSG_2: KLOT VCP-212 fixture, build 19+ era. Plausible bounds.
    assert!(
        m2.rda_build_number >= 1900 && m2.rda_build_number <= 2400,
        "rda_build_number out of range: {}",
        m2.rda_build_number
    );
    assert!(
        m2.average_transmitter_power_w < 2_000,
        "tx power suspiciously high: {}",
        m2.average_transmitter_power_w
    );
    // ICD HW 8 / Appendix C: VCP magnitudes 1..767. ICD HW 8 sign
    // convention encodes local vs remote pattern selection — we
    // don't care which here, just that the magnitude is in range.
    let vcp_mag = m2.volume_coverage_pattern_number.unsigned_abs();
    assert!(
        (1..=767).contains(&vcp_mag),
        "VCP magnitude out of ICD range 1..767: {vcp_mag}"
    );
    // status_version is bumped per ICD revision; non-zero in modern files.
    assert!(
        m2.status_version >= 1,
        "status_version: {}",
        m2.status_version
    );

    // MSG_5 should advertise the same VCP as MSG_2's selected pattern.
    assert_eq!(
        m5.pattern_number, vcp_mag,
        "MSG_5 pattern_number ({}) should match MSG_2 VCP magnitude ({})",
        m5.pattern_number, vcp_mag
    );
    assert!(
        (1..=32).contains(&m5.number_of_elevation_cuts),
        "elevation count out of range: {}",
        m5.number_of_elevation_cuts
    );
    assert_eq!(
        m5.elevation_cuts.len(),
        m5.number_of_elevation_cuts as usize,
        "elevation_cuts vec length must match header count"
    );
    // First cut elevation should be ≈ 0.5° (KLOT VCP-212 lowest tilt).
    let first_cut_deg = m5.elevation_cuts[0].elevation_angle_degrees();
    assert!(
        (0.4..=1.0).contains(&first_cut_deg),
        "first cut elevation_angle_degrees out of range: {first_cut_deg}"
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

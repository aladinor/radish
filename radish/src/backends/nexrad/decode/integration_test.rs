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

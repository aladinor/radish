//! In-tree NEXRAD Level 2 byte-level decoder. **Not yet wired to the
//! production read path** — see plan `0003-internal-nexrad-decoder`.
//!
//! Phase 1-4 already merged: byte-cursor primitives, LDM record
//! splitter + bzip2 decompression, Volume Header parser,
//! MessageHeader iteration loop with the boundary-resync fix, and
//! typed parsers for MSG_2 (RDA Status), MSG_5 (VCP), MSG_31
//! (radial data with VOL/ELV/RAD/REF/VEL/SW/ZDR/PHI/RHO/CFP).
//!
//! Phase 5 (this PR) ties it all together: `decode_volume(bytes) ->
//! Scan` produces a radish-internal `Scan` with sweeps grouped by
//! ICD §3.2.4.17 radial_status markers. Phase 6 adds parity tests
//! against `danielway/nexrad`. Phase 7 swaps the runtime call site
//! in `adapter.rs::convert_scan` to use this entry point.

pub(super) mod error;
pub(super) mod header;
pub(super) mod messages;
pub(super) mod model;
pub(super) mod products;
pub(super) mod reader;
pub(super) mod record;
pub(super) mod volume;

#[cfg(test)]
mod integration_test;

use error::Result;
use messages::{decode_messages, MessagePayload};
use model::{group_radials_into_sweeps, Radial, Scan, Site};
use record::{decompress_all, split_ldm_records};
use volume::parse as parse_volume_header;

/// End-to-end decode of an in-memory NEXRAD Level 2 buffer:
///
/// 1. Optional 24-byte AR2V volume header (skipped if absent).
/// 2. LDM record split → bzip2 decompress → message stream.
/// 3. Per-message typed dispatch via `decode_messages` (Phase 2-4).
/// 4. Aggregate radials, MSG_2 (RDA status), MSG_5 (VCP), site.
/// 5. Group radials into sweeps via ICD radial_status markers.
///
/// Errors with `MissingCoveragePattern` if the file's MSG_5 isn't
/// present — without it we can't emit per-elevation classifier
/// flags downstream.
/// Phase-instrumented variant of [`decode_volume`]. Prints
/// per-phase wall-clock to stderr. Bench-only; the main path
/// stays uninstrumented.
pub(crate) fn decode_volume_with_phase_timing(bytes: &[u8]) -> Result<Scan> {
    use std::time::Instant;
    let t0 = Instant::now();
    let _ = parse_volume_header(bytes);
    let records = split_ldm_records(bytes)?;
    let t1 = Instant::now();
    let payloads = decompress_all(&records)?;
    let t2 = Instant::now();

    let mut radials: Vec<Radial> = Vec::new();
    let mut coverage_pattern = None;
    let mut rda_status = None;
    let mut site: Option<Site> = None;

    for payload in &payloads {
        let messages = decode_messages(payload)?;
        for msg in messages {
            match msg.payload {
                MessagePayload::Msg31(boxed) => {
                    let mut m = *boxed;
                    if site.is_none() {
                        if let Some(vol) = m.volume.take() {
                            site = Some(Site::from_vol(m.header.radar_identifier, &vol));
                        }
                    }
                    radials.push(Radial::from_msg31(m));
                }
                MessagePayload::Msg2(boxed) if rda_status.is_none() => {
                    rda_status = Some(*boxed);
                }
                MessagePayload::Msg5(boxed) if coverage_pattern.is_none() => {
                    coverage_pattern = Some(*boxed);
                }
                _ => {}
            }
        }
    }
    let t3 = Instant::now();

    let coverage_pattern =
        coverage_pattern.ok_or(error::NexradDecodeError::MissingCoveragePattern)?;
    let sweeps = group_radials_into_sweeps(radials);
    let t4 = Instant::now();

    let to_ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1000.0;
    eprintln!(
        "  phase  split_ldm:           {:>6.2} ms",
        to_ms(t0, t1)
    );
    eprintln!(
        "  phase  decompress_all:      {:>6.2} ms",
        to_ms(t1, t2)
    );
    eprintln!(
        "  phase  decode_messages+own: {:>6.2} ms  (typed parse + Radial::from_msg31 gate copies)",
        to_ms(t2, t3)
    );
    eprintln!(
        "  phase  group_into_sweeps:   {:>6.2} ms",
        to_ms(t3, t4)
    );
    eprintln!(
        "  phase  TOTAL:               {:>6.2} ms",
        to_ms(t0, t4)
    );

    Ok(Scan {
        coverage_pattern,
        sweeps,
        site,
        rda_status,
    })
}

pub(crate) fn decode_volume(bytes: &[u8]) -> Result<Scan> {
    // Volume header is optional but useful for the ICAO fallback
    // when no MSG_31 has been seen yet. We discard it for now;
    // the per-radial DataHeader carries the same identifier.
    let _ = parse_volume_header(bytes);

    let records = split_ldm_records(bytes)?;
    let payloads = decompress_all(&records)?;

    let mut radials: Vec<Radial> = Vec::new();
    let mut coverage_pattern = None;
    let mut rda_status = None;
    let mut site: Option<Site> = None;

    for payload in &payloads {
        let messages = decode_messages(payload)?;
        for msg in messages {
            match msg.payload {
                MessagePayload::Msg31(boxed) => {
                    let mut m = *boxed;
                    // First MSG_31 that carries a VOL block defines
                    // the site. Take the VOL block out of the Msg31
                    // before we consume the rest into an owned Radial.
                    if site.is_none() {
                        if let Some(vol) = m.volume.take() {
                            site = Some(Site::from_vol(m.header.radar_identifier, &vol));
                        }
                    }
                    radials.push(Radial::from_msg31(m));
                }
                MessagePayload::Msg2(boxed) if rda_status.is_none() => {
                    rda_status = Some(*boxed);
                }
                MessagePayload::Msg5(boxed) if coverage_pattern.is_none() => {
                    coverage_pattern = Some(*boxed);
                }
                _ => {}
            }
        }
    }

    let coverage_pattern =
        coverage_pattern.ok_or(error::NexradDecodeError::MissingCoveragePattern)?;
    let sweeps = group_radials_into_sweeps(radials);

    Ok(Scan {
        coverage_pattern,
        sweeps,
        site,
        rda_status,
    })
}

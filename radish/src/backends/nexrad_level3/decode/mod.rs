//! NEXRAD Level 3 (NIDS) byte-level decoder — top-level orchestration.
//! Mirrors `nexrad_level3.py:422-557`'s `decode()` for message
//! header/PDB/bzip2/packet-16, extended in Phase 3 with packet AF1F and
//! the message-code-driven product table ported from xradar #392.
//!
//! Layout (a halfword is 2 bytes, big-endian; the ICD numbers them 1-based
//! within each block):
//!
//! ```text
//! text        WMO header + AWIPS id, CR CR LF separated
//! MHB   18 B  message code, date/time, length, block count
//! PDB  102 B  site lat/lon/height, product code, VCP, elevation, data scale
//! payload     usually bzip2 -> symbology block -> layer -> radial packet
//! ```

mod bytes;
pub(crate) mod error;
mod header;
mod legacy16;
mod pdb;
pub(crate) mod products;
mod symbology;

pub(crate) use error::{Level3DecodeError, Result};
pub(crate) use header::{find_awips_token, find_message_header};

use chrono::{DateTime, Utc};
use ndarray::Array2;

use self::bytes::read_i16_be;
use self::products::DecodeScheme;

/// Codes below this are sentinels (0 = below threshold, 1 = range folded)
/// for the packet-16/AF1F linear schemes — never data. The single source
/// of truth for the ICD's floor-code convention: [`linear_declared_scale`]
/// below, [`pdb::value_scaling_float`]'s `value_min` formula, and
/// [`DeclaredLinearScale::data_floor_code`] (which carries it on to
/// [`crate::model::DeclaredScale`] at the model boundary) all read this one
/// constant rather than each redeclaring the literal `2`.
pub(crate) const DATA_FLOOR_CODE: u8 = 2;

/// The scale a packet-16/AF1F linear-coded moment declares, verbatim —
/// what becomes [`crate::model::DeclaredScale`] at the model boundary.
/// `None` for schemes with no simple linear declaration (`Legacy16`,
/// `ClassInt`) — those still populate `codes` and `physical`, just not
/// this.
pub(crate) struct DeclaredLinearScale {
    pub value_min: f32,
    pub value_increment: f32,
    pub n_levels: u16,
    /// Always [`DATA_FLOOR_CODE`] today — carried per-instance (rather than
    /// a caller re-reading the constant) so [`crate::model::DeclaredScale`]
    /// stays a plain field-for-field copy at the model boundary.
    pub data_floor_code: u8,
}

/// One decoded NIDS product — the Rust analogue of the Python oracle's
/// `Level3Sweep` (`nexrad_level3.py:229-282`), extended for the broader
/// product set.
pub(crate) struct DecodedProduct {
    /// 3-letter Level 3 site id, e.g. `"LOT"` (NOT the 4-letter ICAO id) —
    /// always available: any well-formed 6-char text-header token carries
    /// it, independent of whether the product letter is recognised.
    pub site: String,
    /// The 3-character AWIPS product token, e.g. `"N0B"`, when the text
    /// header carried a well-formed one (it always should for a real NIDS
    /// file — see [`header::find_awips_token`]).
    pub awips_id: String,
    pub moment: &'static str,
    /// 0..5 tilt ordinal, populated only for the subset of products with
    /// an S3-verified AWIPS-letter table (`products::tilt_letter_lookup`)
    /// — see `decode::products`'s module doc for why this isn't guessed
    /// for the rest. The real elevation (`elevation_deg`) is always
    /// populated regardless of whether `tilt` resolves.
    pub tilt: Option<usize>,
    pub message_code: i16,
    pub vcp: u16,
    pub elevation_deg: f64,
    pub scan_time: DateTime<Utc>,
    pub lat: f64,
    pub lon: f64,
    pub height_m: f64,
    /// Ray CENTRES, degrees, in scan order.
    pub azimuths: Vec<f64>,
    /// `(n_radials, n_bins)` verbatim codes — packet 16's raw bytes, or
    /// packet AF1F's RLE expanded to the same per-gate shape. Always `u8`:
    /// packet 16/AF1F never exceed 8 bits per gate (AF1F's nibble codes
    /// are 0-15). Packet 28 is a different `u16` geometry this backend
    /// doesn't decode yet (`Level3DecodeError::UnsupportedPacketCode`).
    pub codes: Array2<u8>,
    pub first_gate_m: f32,
    pub gate_spacing_m: f32,
    /// The product's own linear scale, when it has one (`LinearHw`/
    /// `FloatScale` — the original 6 AtmoScale products plus 94/155).
    pub declared_scale: Option<DeclaredLinearScale>,
    /// Physical values, always populated regardless of decode scheme —
    /// every backend consumer expects `MomentData::data` to be usable.
    pub physical: Array2<f32>,
}

/// Decode one NIDS product. Loud rather than lenient throughout — a
/// mis-parsed product would yield a plausible-looking sweep pointed at the
/// wrong place, far worse than a decode error a caller can skip past.
pub(crate) fn decode(raw: &[u8]) -> Result<DecodedProduct> {
    // Message header first — it's what tells us which product this is,
    // and (since Phase 3) that no longer depends on the AWIPS token at
    // all. `find_message_header` only accepts a KNOWN code
    // (`products::is_known_message_code`), so `products::spec_for` below
    // is always `Some`.
    let mhb = find_message_header(raw)?;
    let message_code =
        read_i16_be(raw, mhb).expect("find_message_header validated this offset is readable");
    let spec = products::spec_for(message_code)
        .expect("message_code passed find_message_header's known-code check");

    if !products::packet_family_implemented(message_code) {
        return Err(Level3DecodeError::UnsupportedProduct { code: message_code });
    }

    // The text-header token supplies the site string (always) and,
    // for the verified subset, a tilt ordinal — never the moment, which
    // is `spec.moment` above regardless of what letter shows up.
    let (awips_id, site) = find_awips_token(raw)?;
    let tilt = products::tilt_letter_lookup(awips_id.as_bytes());

    let pdb = mhb + 18;
    // Without this, every PDB read below would need its own truncation
    // check. A short/partial read (51-95 bytes was the fuzzer-found case
    // in the oracle) must be a clean error here, not a panic later.
    if raw.len() < pdb + 102 {
        return Err(Level3DecodeError::TruncatedBeforePdb {
            have: raw.len(),
            need: pdb + 102,
        });
    }

    let fields = pdb::read_pdb_fields(raw, pdb)?;
    let scan_time = header::scan_time(fields.scan_days, fields.scan_seconds)?;

    if fields.product_code != message_code {
        return Err(Level3DecodeError::ProductCodeMismatch {
            product_code: fields.product_code,
            message_code,
        });
    }
    if !(-90.0..=90.0).contains(&fields.lat) || !(-180.0..=180.0).contains(&fields.lon) {
        return Err(Level3DecodeError::ImplausibleSitePosition {
            lat: fields.lat,
            lon: fields.lon,
        });
    }

    let payload = &raw[pdb + 102..];
    let body = symbology::decompress_payload(payload)?;
    let symbology::SymbologyResult {
        azimuths,
        codes,
        range_scale_raw,
    } = symbology::decode_symbology(&body)?;

    // Elevation: 0.0 for surface/volume products, matching xradar's
    // `get_elevation` (`has_elevation: false` products carry no angle).
    let elevation_deg = if spec.has_elevation {
        fields.elevation_deg
    } else {
        0.0
    };

    // Gate geometry: a fixed ICD bin size for elevation-bearing products
    // (the packet's own range-scale field is `cos(elevation) * 1000` for
    // those, not metres — `docs/NEXRAD_LEVEL3_WASM.md` §4.4); the packet's
    // own field directly for surface products, where it genuinely is the
    // range scale in metres (matching xradar's `get_range`, "packet range
    // scale is exact" for surface products).
    let gate_spacing_m = spec.bin_size.unwrap_or(range_scale_raw as f32);
    let first_gate_m = gate_spacing_m / 2.0;

    let (declared_scale, physical) = match spec.decode {
        DecodeScheme::LinearHw => {
            let (value_min, value_increment, n_levels) = pdb::value_scaling_linear_hw(raw, pdb)?;
            linear_declared_scale(&codes, value_min, value_increment, n_levels)?
        }
        DecodeScheme::FloatScale => {
            let (value_min, value_increment, n_levels) = pdb::value_scaling_float(raw, pdb)?;
            linear_declared_scale(&codes, value_min, value_increment, n_levels)?
        }
        DecodeScheme::Legacy16 => {
            let threshold = pdb::threshold_data(raw, pdb)?;
            let levels = legacy16::decode_legacy16(&threshold, spec.post_scale);
            // AF1F codes are always 0-15 (a 4-bit RLE nibble) — see
            // `symbology::decode_packet_af1f`.
            let physical = codes.mapv(|code| levels[(code & 0x0F) as usize]);
            (None, physical)
        }
        DecodeScheme::ClassInt => {
            // Categorical: code 0 is "no data", every other code indexes a
            // fixed NWS category table this backend doesn't own (see
            // `docs/NEXRAD_LEVEL3_WASM.md`'s categorical-products note).
            let physical = codes.mapv(|code| if code == 0 { f32::NAN } else { code as f32 });
            (None, physical)
        }
        DecodeScheme::Precip | DecodeScheme::Rate => {
            unreachable!("packet_family_implemented gates Precip/Rate out above")
        }
    };

    Ok(DecodedProduct {
        site,
        awips_id,
        moment: spec.moment,
        tilt,
        message_code,
        vcp: fields.vcp,
        elevation_deg,
        scan_time,
        lat: fields.lat,
        lon: fields.lon,
        height_m: fields.height_m,
        azimuths,
        codes,
        first_gate_m,
        gate_spacing_m,
        declared_scale,
        physical,
    })
}

/// Shared by [`DecodeScheme::LinearHw`] and [`DecodeScheme::FloatScale`]:
/// validate `value_increment`, derive `physical` from `codes`
/// (`DATA_FLOOR_CODE` and above is real data; below is a sentinel -> NaN),
/// and package the declared scale for the model boundary.
fn linear_declared_scale(
    codes: &Array2<u8>,
    value_min: f32,
    value_increment: f32,
    n_levels: u16,
) -> Result<(Option<DeclaredLinearScale>, Array2<f32>)> {
    if value_increment <= 0.0 {
        return Err(Level3DecodeError::NonPositiveIncrement(value_increment));
    }
    let physical = codes.mapv(|code| {
        if code >= DATA_FLOOR_CODE {
            value_min + (code - DATA_FLOOR_CODE) as f32 * value_increment
        } else {
            f32::NAN
        }
    });
    Ok((
        Some(DeclaredLinearScale {
            value_min,
            value_increment,
            n_levels,
            data_floor_code: DATA_FLOOR_CODE,
        }),
        physical,
    ))
}

/// Build a complete, minimal, valid NIDS product byte-for-byte: text
/// header with an AWIPS id, MHB, PDB (integer-scaled reflectivity), and an
/// uncompressed one-radial packet-16 symbology block. Used to smoke-test
/// the full `decode()` pipeline without a real fixture, and re-exported
/// (`pub(crate)`, test-only) for `nexrad_level3::mod`'s and `adapter`'s own
/// test modules to reuse rather than duplicate — Phase 4 adds byte-for-byte
/// parity against the Python oracle on real fixtures.
#[cfg(test)]
pub(crate) fn build_minimal_n0b() -> Vec<u8> {
    let mut raw = Vec::new();
    // Text header: AWIPS id token, then padding to reach the MHB.
    raw.extend_from_slice(b"N0BLOT\r\r\n");
    while raw.len() < 30 {
        raw.push(b' ');
    }
    let mhb = raw.len();
    raw.extend_from_slice(&153i16.to_be_bytes()); // message code
    raw.extend_from_slice(&[0u8; 16]); // rest of the 18-byte MHB, unread
    assert_eq!(raw.len(), mhb + 18);

    let pdb = raw.len();
    let mut pdb_bytes = vec![0u8; 102];
    // hw1: the PDB's opening `-1` divider — `find_message_header`
    // validates against this at `mhb + 18` (== `pdb + 0`).
    pdb_bytes[0..2].copy_from_slice(&(-1i16).to_be_bytes());
    // hw2 lat x1000 (i32 at halfword offset 2 -> byte 2).
    pdb_bytes[2..6].copy_from_slice(&41_881i32.to_be_bytes());
    // hw4 lon x1000.
    pdb_bytes[6..10].copy_from_slice(&(-88_084i32).to_be_bytes());
    // hw6 height, feet.
    pdb_bytes[10..12].copy_from_slice(&650i16.to_be_bytes());
    // hw7 product code (must equal the message code).
    pdb_bytes[12..14].copy_from_slice(&153i16.to_be_bytes());
    // hw9 vcp.
    pdb_bytes[16..18].copy_from_slice(&215i16.to_be_bytes());
    // hw12 scan days (u16, >=1).
    pdb_bytes[22..24].copy_from_slice(&20000u16.to_be_bytes());
    // hw13 scan seconds (i32).
    pdb_bytes[24..28].copy_from_slice(&3600i32.to_be_bytes());
    // hw21 elevation x10.
    pdb_bytes[40..42].copy_from_slice(&5i16.to_be_bytes());
    // hw22 value_min x10 = -32.0 dBZ.
    pdb_bytes[42..44].copy_from_slice(&(-320i16).to_be_bytes());
    // hw23 value_increment x10 = 0.5 dB.
    pdb_bytes[44..46].copy_from_slice(&5i16.to_be_bytes());
    // hw24 n_levels.
    pdb_bytes[46..48].copy_from_slice(&254i16.to_be_bytes());
    raw.extend_from_slice(&pdb_bytes);
    assert_eq!(raw.len(), pdb + 102);

    // Uncompressed symbology block (no "BZ" magic -> passthrough).
    let mut body = Vec::new();
    body.extend_from_slice(&(-1i16).to_be_bytes());
    body.extend_from_slice(&1i16.to_be_bytes());
    body.extend_from_slice(&[0u8; 6]);
    body.extend_from_slice(&[0u8; 6]);
    body.extend_from_slice(&16u16.to_be_bytes()); // packet_code
    body.extend_from_slice(&0u16.to_be_bytes()); // first_bin
    body.extend_from_slice(&4u16.to_be_bytes()); // n_bins
    body.extend_from_slice(&999i16.to_be_bytes());
    body.extend_from_slice(&998i16.to_be_bytes());
    body.extend_from_slice(&999u16.to_be_bytes());
    body.extend_from_slice(&1u16.to_be_bytes()); // n_radials
    body.extend_from_slice(&4u16.to_be_bytes()); // n_bytes for the one radial
    body.extend_from_slice(&0u16.to_be_bytes()); // start_angle
    body.extend_from_slice(&10u16.to_be_bytes()); // delta (1.0 deg)
    body.extend_from_slice(&[2, 3, 4, 5]); // codes
    raw.extend_from_slice(&body);

    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_a_minimal_synthetic_n0b_end_to_end() {
        let raw = build_minimal_n0b();
        let product = decode(&raw).unwrap();
        assert_eq!(product.awips_id, "N0B");
        assert_eq!(product.site, "LOT");
        assert_eq!(product.moment, "DBZH");
        assert_eq!(product.tilt, Some(0));
        assert_eq!(product.message_code, 153);
        assert_eq!(product.vcp, 215);
        assert!((product.elevation_deg - 0.5).abs() < 1e-6);
        assert!((product.lat - 41.881).abs() < 1e-6);
        assert!((product.lon - (-88.084)).abs() < 1e-6);
        let scale = product.declared_scale.as_ref().unwrap();
        assert_eq!(scale.value_min, -32.0);
        assert_eq!(scale.value_increment, 0.5);
        assert_eq!(scale.n_levels, 254);
        assert_eq!(product.gate_spacing_m, 250.0);
        assert_eq!(product.first_gate_m, 125.0);
        assert_eq!(product.codes.shape(), &[1, 4]);
        assert_eq!(product.codes.row(0).to_vec(), vec![2, 3, 4, 5]);
        assert!((product.azimuths[0] - 0.5).abs() < 1e-9); // (0 + 10/2.0) / 10.0
        assert_eq!(
            product.physical.row(0).to_vec(),
            vec![-32.0, -31.5, -31.0, -30.5]
        );
    }

    #[test]
    fn decode_rejects_product_code_message_code_mismatch() {
        let mut raw = build_minimal_n0b();
        // Corrupt the PDB's product-code halfword (hw7, at mhb+18+12).
        let mhb = raw
            .windows(2)
            .position(|w| w == 153i16.to_be_bytes())
            .unwrap();
        let pdb = mhb + 18;
        raw[pdb + 12..pdb + 14].copy_from_slice(&99i16.to_be_bytes());
        assert!(matches!(
            decode(&raw),
            Err(Level3DecodeError::ProductCodeMismatch { .. })
        ));
    }

    #[test]
    fn decode_rejects_truncated_input() {
        let raw = build_minimal_n0b();
        assert!(matches!(
            decode(&raw[..raw.len() / 2]),
            Err(Level3DecodeError::TruncatedBeforePdb { .. })
        ));
    }

    #[test]
    fn decode_resolves_tilt_none_for_unverified_letters_but_still_decodes() {
        // Swap the AWIPS letter to a code with no verified tilt table
        // (still message code 153, so decode succeeds; tilt is just
        // unresolved rather than guessed).
        let mut raw = build_minimal_n0b();
        raw[0..6].copy_from_slice(b"N0HLOT");
        let product = decode(&raw).unwrap();
        assert_eq!(product.awips_id, "N0H");
        assert_eq!(product.tilt, None);
        assert_eq!(product.moment, "DBZH"); // still resolved via message code
    }

    /// Just enough to reach `find_message_header` + `packet_family_implemented`
    /// — no PDB or symbology block needed, since a deferred product errors
    /// out before either is read.
    fn build_header_only(message_code: i16) -> Vec<u8> {
        let mut raw = b"N0ZLOT\r\r\n".to_vec();
        while raw.len() < 30 {
            raw.push(b' ');
        }
        raw.extend_from_slice(&message_code.to_be_bytes());
        raw.extend_from_slice(&[0u8; 16]);
        raw.extend_from_slice(&(-1i16).to_be_bytes()); // PDB divider
                                                       // Trailing slack: `find_message_header`'s scan window is
                                                       // `0..min(len,128)-20` (exclusive), so a header with nothing after
                                                       // its own divider check sits just outside the window — see
                                                       // `sniff.rs`'s `with_message_header` helper for the same fix.
        raw.extend_from_slice(&[0u8; 8]);
        raw
    }

    #[test]
    fn decode_returns_unsupported_product_for_a_deferred_code_end_to_end() {
        // 170 (surface precip, packet type unconfirmed this pass) and 176
        // (packet 28 / XDR, u16 raw levels) are both known message codes
        // (`is_known_message_code` — `find_message_header` locates them)
        // but deliberately not implemented — `decode()` must refuse them
        // before ever reaching the AWIPS-token or PDB stage, not panic or
        // silently misdecode.
        for code in [170i16, 176, 177] {
            let raw = build_header_only(code);
            assert!(
                matches!(
                    decode(&raw),
                    Err(Level3DecodeError::UnsupportedProduct { code: c }) if c == code
                ),
                "message code {code} should be UnsupportedProduct"
            );
        }
    }

    /// A full, valid packet-AF1F / `Legacy16` product (message code 19,
    /// legacy reflectivity) — text header, MHB, PDB with a `threshold_data`
    /// flag/value table, and an RLE-encoded symbology block. Exercises the
    /// integration `decode_legacy16_af1f_product_end_to_end` needs: PDB
    /// `threshold_data` read -> `legacy16::decode_legacy16` -> AF1F RLE
    /// expand -> `codes.mapv` with the `& 0x0F` mask, all wired together
    /// exactly as `decode()` does it (each piece already has isolated unit
    /// tests elsewhere; this is the only place they run as one pipeline).
    fn build_minimal_legacy19() -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"N0RLOT\r\r\n");
        while raw.len() < 30 {
            raw.push(b' ');
        }
        let mhb = raw.len();
        raw.extend_from_slice(&19i16.to_be_bytes());
        raw.extend_from_slice(&[0u8; 16]);
        assert_eq!(raw.len(), mhb + 18);

        let pdb = raw.len();
        let mut pdb_bytes = vec![0u8; 102];
        pdb_bytes[0..2].copy_from_slice(&(-1i16).to_be_bytes()); // divider
        pdb_bytes[2..6].copy_from_slice(&41_881i32.to_be_bytes()); // lat
        pdb_bytes[6..10].copy_from_slice(&(-88_084i32).to_be_bytes()); // lon
        pdb_bytes[10..12].copy_from_slice(&650i16.to_be_bytes()); // height, ft
        pdb_bytes[12..14].copy_from_slice(&19i16.to_be_bytes()); // product code
        pdb_bytes[16..18].copy_from_slice(&215i16.to_be_bytes()); // vcp
        pdb_bytes[22..24].copy_from_slice(&20000u16.to_be_bytes()); // scan days
        pdb_bytes[24..28].copy_from_slice(&3600i32.to_be_bytes()); // scan seconds
        pdb_bytes[40..42].copy_from_slice(&5i16.to_be_bytes()); // elevation x10
                                                                // threshold_data (halfwords 22-37, bytes 42..74): 16 (flag, value)
                                                                // pairs. flag=0x00 for every level -> positive, unscaled, so
                                                                // levels[i] == i (given post_scale = 1.0 for message code 19).
        for i in 0..16u8 {
            pdb_bytes[42 + 2 * i as usize] = 0x00; // flag
            pdb_bytes[42 + 2 * i as usize + 1] = i; // value
        }
        raw.extend_from_slice(&pdb_bytes);
        assert_eq!(raw.len(), pdb + 102);

        // Packet AF1F, one radial, 4 bins: RLE (run=2,color=3),(run=2,color=7)
        // -> codes [3,3,7,7].
        let mut body = Vec::new();
        body.extend_from_slice(&(-1i16).to_be_bytes());
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&[0u8; 6]);
        body.extend_from_slice(&[0u8; 6]);
        body.extend_from_slice(&(-20705i16).to_be_bytes()); // AF1F packet code
        body.extend_from_slice(&0u16.to_be_bytes()); // first_bin (unchecked)
        body.extend_from_slice(&4u16.to_be_bytes()); // n_bins
        body.extend_from_slice(&999i16.to_be_bytes());
        body.extend_from_slice(&998i16.to_be_bytes());
        body.extend_from_slice(&999u16.to_be_bytes());
        body.extend_from_slice(&1u16.to_be_bytes()); // n_radials
        body.extend_from_slice(&1u16.to_be_bytes()); // n_bytes: 1 halfword = 2 RLE bytes
        body.extend_from_slice(&0u16.to_be_bytes()); // start_angle
        body.extend_from_slice(&10u16.to_be_bytes()); // delta
        body.push((2u8 << 4) | 3);
        body.push((2u8 << 4) | 7);
        raw.extend_from_slice(&body);

        raw
    }

    #[test]
    fn decode_legacy16_af1f_product_end_to_end() {
        let raw = build_minimal_legacy19();
        let product = decode(&raw).unwrap();
        assert_eq!(product.moment, "DBZH");
        assert_eq!(product.message_code, 19);
        // Legacy16 has no simple linear declared scale.
        assert!(product.declared_scale.is_none());
        assert_eq!(product.codes.row(0).to_vec(), vec![3, 3, 7, 7]);
        // levels[i] == i for the all-zero-flag threshold table built above.
        assert_eq!(product.physical.row(0).to_vec(), vec![3.0, 3.0, 7.0, 7.0]);
        // message code 19's ICD bin size (`ProductSpec.bin_size`), not the
        // packet-16-only 250 m constant.
        assert_eq!(product.gate_spacing_m, 1000.0);
    }

    /// A full, valid packet-16 / `ClassInt` (categorical) product (message
    /// code 165, hydrometeor classification).
    fn build_minimal_hclass165() -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"N0HLOT\r\r\n");
        while raw.len() < 30 {
            raw.push(b' ');
        }
        let mhb = raw.len();
        raw.extend_from_slice(&165i16.to_be_bytes());
        raw.extend_from_slice(&[0u8; 16]);
        assert_eq!(raw.len(), mhb + 18);

        let pdb = raw.len();
        let mut pdb_bytes = vec![0u8; 102];
        pdb_bytes[0..2].copy_from_slice(&(-1i16).to_be_bytes());
        pdb_bytes[2..6].copy_from_slice(&41_881i32.to_be_bytes());
        pdb_bytes[6..10].copy_from_slice(&(-88_084i32).to_be_bytes());
        pdb_bytes[10..12].copy_from_slice(&650i16.to_be_bytes());
        pdb_bytes[12..14].copy_from_slice(&165i16.to_be_bytes());
        pdb_bytes[16..18].copy_from_slice(&215i16.to_be_bytes());
        pdb_bytes[22..24].copy_from_slice(&20000u16.to_be_bytes());
        pdb_bytes[24..28].copy_from_slice(&3600i32.to_be_bytes());
        pdb_bytes[40..42].copy_from_slice(&5i16.to_be_bytes());
        raw.extend_from_slice(&pdb_bytes);
        assert_eq!(raw.len(), pdb + 102);

        let mut body = Vec::new();
        body.extend_from_slice(&(-1i16).to_be_bytes());
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&[0u8; 6]);
        body.extend_from_slice(&[0u8; 6]);
        body.extend_from_slice(&16u16.to_be_bytes()); // packet 16
        body.extend_from_slice(&0u16.to_be_bytes()); // first_bin
        body.extend_from_slice(&4u16.to_be_bytes()); // n_bins
        body.extend_from_slice(&999i16.to_be_bytes());
        body.extend_from_slice(&998i16.to_be_bytes());
        body.extend_from_slice(&999u16.to_be_bytes());
        body.extend_from_slice(&1u16.to_be_bytes()); // n_radials
        body.extend_from_slice(&4u16.to_be_bytes()); // n_bytes
        body.extend_from_slice(&0u16.to_be_bytes());
        body.extend_from_slice(&10u16.to_be_bytes());
        body.extend_from_slice(&[0, 5, 10, 0]); // codes: no-data, 5, 10, no-data
        raw.extend_from_slice(&body);

        raw
    }

    #[test]
    fn decode_class_int_product_end_to_end() {
        let raw = build_minimal_hclass165();
        let product = decode(&raw).unwrap();
        assert_eq!(product.moment, "HCLASS");
        assert!(product.declared_scale.is_none());
        assert_eq!(product.codes.row(0).to_vec(), vec![0, 5, 10, 0]);
        let physical = product.physical.row(0).to_vec();
        assert!(physical[0].is_nan()); // code 0 = no data
        assert_eq!(physical[1], 5.0); // code IS the category index
        assert_eq!(physical[2], 10.0);
        assert!(physical[3].is_nan());
    }
}

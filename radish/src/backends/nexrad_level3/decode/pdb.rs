//! Product Description Block (PDB) field reads: site position, VCP,
//! elevation, and the two-form value scaling. Mirrors
//! `nexrad_level3.py:367-466`'s halfword arithmetic and validation.
//!
//! Halfwords are numbered 1-based within the PDB, per the ICD — halfword
//! `index` starts at byte offset `pdb + 2 * (index - 1)`.

use super::bytes::{read_i16_be, read_i32_be, read_u16_be};
use super::error::{Level3DecodeError, Result};
use super::DATA_FLOOR_CODE;

fn hw_i16(raw: &[u8], pdb: usize, index: usize) -> Result<i16> {
    read_i16_be(raw, pdb + 2 * (index - 1)).ok_or(Level3DecodeError::PdbFieldOutOfBounds { index })
}

fn hw_u16(raw: &[u8], pdb: usize, index: usize) -> Result<u16> {
    read_u16_be(raw, pdb + 2 * (index - 1)).ok_or(Level3DecodeError::PdbFieldOutOfBounds { index })
}

fn hw_i32(raw: &[u8], pdb: usize, index: usize) -> Result<i32> {
    read_i32_be(raw, pdb + 2 * (index - 1)).ok_or(Level3DecodeError::PdbFieldOutOfBounds { index })
}

/// Read a big-endian `f32` scale/offset pair starting at halfword `index`
/// (4 bytes each, back to back — the float scale form's halfwords 22-25).
fn hw_f32_pair(raw: &[u8], pdb: usize, index: usize) -> Result<(f32, f32)> {
    let off = pdb + 2 * (index - 1);
    let bytes = raw
        .get(off..off + 8)
        .ok_or(Level3DecodeError::PdbFieldOutOfBounds { index })?;
    let scale = f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let offset = f32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    Ok((scale, offset))
}

/// The PDB fields this decoder reads, before validation.
pub(crate) struct PdbFields {
    pub lat: f64,
    pub lon: f64,
    pub height_m: f64,
    pub product_code: i16,
    pub vcp: u16,
    pub scan_days: u16,
    pub scan_seconds: i32,
    pub elevation_deg: f64,
}

/// Read the fixed PDB fields this decoder needs. Caller must already have
/// verified `raw.len() >= pdb + 102` (`nexrad_level3.py:438-441`) — every
/// field read here (halfwords 2-21) sits within that span, so failures
/// here indicate a logic bug rather than a short read, but reads stay
/// checked regardless (see [`Level3DecodeError::PdbFieldOutOfBounds`]).
pub(crate) fn read_pdb_fields(raw: &[u8], pdb: usize) -> Result<PdbFields> {
    Ok(PdbFields {
        lat: hw_i32(raw, pdb, 2)? as f64 / 1000.0,
        lon: hw_i32(raw, pdb, 4)? as f64 / 1000.0,
        // The PDB carries height in feet.
        height_m: hw_i16(raw, pdb, 6)? as f64 * 0.3048,
        product_code: hw_i16(raw, pdb, 7)?,
        vcp: hw_u16(raw, pdb, 9)?,
        scan_days: hw_u16(raw, pdb, 12)?,
        scan_seconds: hw_i32(raw, pdb, 13)?,
        // f64, matching `nexrad_level3.py:454`'s `hw(21) / 10` — computing
        // this in f32 (as an earlier version did) loses precision the
        // oracle never had: e.g. 31 / 10.0f32 rounds to a value that
        // widens to 3.0999999046325684 in f64, not 3.1. Narrow to f32 only
        // at the one point that genuinely needs it — the per-radial
        // `Coordinates.elevation` array (`adapter.rs`).
        elevation_deg: hw_i16(raw, pdb, 21)? as f64 / 10.0,
    })
}

/// `(value_min, value_increment, n_levels)` from the PDB's integer
/// (`LinearHw`) scale form — halfwords 22/23/24: min x10, increment x10,
/// level count. Mirrors `nexrad_level3.py:396-401`.
///
/// Split from the float form into its own function (rather than one
/// `value_scaling(scheme)` with a 4-armed `unreachable!()` for the
/// schemes it can't handle) so calling it with the wrong scheme is a type
/// error caught by `decode/mod.rs`'s match arms, not a runtime panic on
/// real product input — see `decode/products.rs`'s `DecodeScheme` doc
/// comment for why this crate treats that distinction as worth the extra
/// function.
pub(crate) fn value_scaling_linear_hw(raw: &[u8], pdb: usize) -> Result<(f32, f32, u16)> {
    let value_min = hw_i16(raw, pdb, 22)? as f32 / 10.0;
    let value_increment = hw_i16(raw, pdb, 23)? as f32 / 10.0;
    // Not otherwise validated (matching the oracle — only
    // `value_increment` is checked, in `decode()` after this returns). A
    // negative reading here would indicate a malformed file already
    // headed for a different error upstream (or, for a well-formed one,
    // never happens).
    let n_levels = hw_i16(raw, pdb, 24)? as u16;
    Ok((value_min, value_increment, n_levels))
}

/// `(value_min, value_increment, n_levels)` from the PDB's float
/// (`FloatScale`) scale form — halfwords 22-25: a big-endian `f32` scale
/// then offset. See `docs/NEXRAD_LEVEL3_WASM.md` §4.3 for why this and
/// [`value_scaling_linear_hw`] are dispatched on an enum rather than a
/// heuristic on the values. Mirrors `nexrad_level3.py:403-419`.
///
/// Verified against real products from `s3://unidata-nexrad-level3`
/// (the oracle's own docstring, `nexrad_level3.py:386-390`):
///
/// ```text
/// N0X (159)  scale 16   offset 128    -> -7.875   .. 7.9375
/// N0C (161)  scale 300  offset -60.5  ->  0.20833 .. 1.05167
/// N0K (163)  scale 20   offset 43     -> -2.05    .. 10.6
/// ```
pub(crate) fn value_scaling_float(raw: &[u8], pdb: usize) -> Result<(f32, f32, u16)> {
    const DATA_LEVEL_COUNT: u16 = 254;

    let (scale, offset) = hw_f32_pair(raw, pdb, 22)?;
    if !scale.is_finite() || scale == 0.0 {
        return Err(Level3DecodeError::NonFiniteScale(scale));
    }
    if !offset.is_finite() {
        return Err(Level3DecodeError::NonFiniteOffset(offset));
    }
    // The level count is ASSERTED, not read: the float form spends
    // halfwords 22-25 on the scale/offset pair, so there is no
    // level-count slot where the integer form keeps one —
    // `DATA_FLOOR_CODE`..255 is what the encoding means
    // (`nexrad_level3.py:408-419`).
    Ok((
        (DATA_FLOOR_CODE as f32 - offset) / scale,
        1.0 / scale,
        DATA_LEVEL_COUNT,
    ))
}

/// 32-byte `threshold_data` field, PDB halfwords 22-37 — the raw bytes
/// [`crate::backends::nexrad_level3::decode::legacy16::decode_legacy16`]
/// needs. Caller must already have verified `raw.len() >= pdb + 102`.
pub(crate) fn threshold_data(raw: &[u8], pdb: usize) -> Result<[u8; 32]> {
    let off = pdb + 2 * (22 - 1);
    raw.get(off..off + 32)
        .map(|b| b.try_into().expect("slice of len 32"))
        .ok_or(Level3DecodeError::PdbFieldOutOfBounds { index: 22 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdb_bytes_for_linear_float(scale: f32, offset: f32) -> Vec<u8> {
        // halfwords 1..21 as zero padding, then the f32 pair at 22-25.
        let mut buf = vec![0u8; 2 * 21];
        buf.extend_from_slice(&scale.to_be_bytes());
        buf.extend_from_slice(&offset.to_be_bytes());
        buf
    }

    #[test]
    fn value_scaling_float_form_matches_oracle_docstring_n0x() {
        let raw = pdb_bytes_for_linear_float(16.0, 128.0);
        let (min, inc, n) = value_scaling_float(&raw, 0).unwrap();
        assert!((min - (-7.875)).abs() < 1e-4);
        assert!((inc - 0.0625).abs() < 1e-6);
        assert_eq!(n, 254);
    }

    #[test]
    fn value_scaling_float_form_matches_oracle_docstring_n0c() {
        let raw = pdb_bytes_for_linear_float(300.0, -60.5);
        let (min, inc, _n) = value_scaling_float(&raw, 0).unwrap();
        assert!((min - 0.20833).abs() < 1e-4);
        assert!((inc - (1.0 / 300.0)).abs() < 1e-6);
    }

    #[test]
    fn value_scaling_float_form_matches_oracle_docstring_n0k() {
        let raw = pdb_bytes_for_linear_float(20.0, 43.0);
        let (min, inc, _n) = value_scaling_float(&raw, 0).unwrap();
        assert!((min - (-2.05)).abs() < 1e-4);
        assert!((inc - 0.05).abs() < 1e-6);
    }

    #[test]
    fn value_scaling_rejects_zero_scale() {
        let raw = pdb_bytes_for_linear_float(0.0, 128.0);
        assert!(matches!(
            value_scaling_float(&raw, 0),
            Err(Level3DecodeError::NonFiniteScale(_))
        ));
    }

    #[test]
    fn value_scaling_rejects_non_finite_scale() {
        // Distinct from the zero-scale case above: NaN/Infinity satisfy
        // `scale == 0.0`'s negation but must still be rejected by the
        // `!scale.is_finite()` half of the same check.
        let raw = pdb_bytes_for_linear_float(f32::NAN, 128.0);
        assert!(matches!(
            value_scaling_float(&raw, 0),
            Err(Level3DecodeError::NonFiniteScale(_))
        ));
        let raw_inf = pdb_bytes_for_linear_float(f32::INFINITY, 128.0);
        assert!(matches!(
            value_scaling_float(&raw_inf, 0),
            Err(Level3DecodeError::NonFiniteScale(_))
        ));
    }

    #[test]
    fn value_scaling_rejects_non_finite_offset() {
        let raw = pdb_bytes_for_linear_float(16.0, f32::NAN);
        assert!(matches!(
            value_scaling_float(&raw, 0),
            Err(Level3DecodeError::NonFiniteOffset(_))
        ));
    }

    #[test]
    fn value_scaling_integer_form_reads_min_increment_levels() {
        let mut raw = vec![0u8; 2 * 24];
        // halfword 22 = -320 (min x10 -> -32.0), 23 = 5 (increment x10 -> 0.5), 24 = 254.
        raw[2 * 21..2 * 22].copy_from_slice(&(-320i16).to_be_bytes());
        raw[2 * 22..2 * 23].copy_from_slice(&5i16.to_be_bytes());
        raw[2 * 23..2 * 24].copy_from_slice(&254i16.to_be_bytes());
        let (min, inc, n) = value_scaling_linear_hw(&raw, 0).unwrap();
        assert_eq!(min, -32.0);
        assert_eq!(inc, 0.5);
        assert_eq!(n, 254);
    }

    #[test]
    fn read_pdb_fields_decodes_lat_lon_height_vcp() {
        let mut raw = vec![0u8; 2 * 21];
        raw[2..6].copy_from_slice(&41_881i32.to_be_bytes()); // hw2 lat x1000
        raw[6..10].copy_from_slice(&(-88_084i32).to_be_bytes()); // hw4 lon x1000
        raw[2 * 5..2 * 6].copy_from_slice(&650i16.to_be_bytes()); // hw6 height (ft)
        raw[2 * 6..2 * 7].copy_from_slice(&153i16.to_be_bytes()); // hw7 product code
        raw[2 * 8..2 * 9].copy_from_slice(&215i16.to_be_bytes()); // hw9 vcp
        let fields = read_pdb_fields(&raw, 0).unwrap();
        assert!((fields.lat - 41.881).abs() < 1e-6);
        assert!((fields.lon - (-88.084)).abs() < 1e-6);
        assert!((fields.height_m - 650.0 * 0.3048).abs() < 1e-6);
        assert_eq!(fields.product_code, 153);
        assert_eq!(fields.vcp, 215);
    }
}

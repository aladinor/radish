//! Per-IRIS-data-type calibration helpers.
//!
//! Each IRIS data type has its own encoding rule. Most are linear:
//! `value = (raw - offset) / scale`. Velocity is Nyquist-scaled, phase is
//! mapped to [-180°, 180°], KDP is an exponential of the wavelength, and a
//! couple of types mask a no-data sentinel. Helpers here mirror xradar's
//! `iris.py` `decode_*` family (`decode_array`, `decode_vel`, `decode_width`,
//! `decode_phidp`, `decode_kdp`, `decode_sqi`, …) so per-gate values match
//! xradar's output to floating-point precision.
//!
//! **No-data policy (matches xradar).** xradar only masks where a data
//! type's `func`/`fkw` says so: `DB_VEL` (8-bit) carries `mask: 0.0`;
//! `decode_sqi` (RHOHV/SQI) yields `NaN` *naturally* from `sqrt` of a
//! negative; `decode_kdp` masks its `0`/`-1` sentinels. Every `decode_array`
//! type — reflectivity, ZDR, width, PHIDP, and all 2-byte variants including
//! `DB_VEL2` — is **never** masked: a raw `0` decodes to a finite (possibly
//! very low) value. Earlier revisions blanket-masked `raw == 0 → NaN` for all
//! types, which over-masked power/phase moments relative to xradar (issue
//! #28). We now mask only where xradar does.
//!
//! Each helper takes a `raw: u16` (we widen 8-bit raws on read) plus a
//! [`DecodeParams`] carrying the per-volume `nyquist_ms` (velocity-style
//! decoders) and `wavelength_cm` (KDP). Non-dependent helpers ignore them.

/// Per-volume scalars threaded into every decoder. `Copy` so the per-gate
/// hot loop passes it by value without borrowing.
#[derive(Clone, Copy, Debug)]
pub(super) struct DecodeParams {
    /// Nyquist velocity (m/s), used by the velocity/width decoders.
    pub nyquist_ms: f32,
    /// Radar wavelength in centimetres, used by the KDP decoder.
    pub wavelength_cm: f32,
}

/// Function pointer type for per-gate decoders. The signature carries the
/// per-volume [`DecodeParams`] so velocity- and KDP-style helpers can use
/// `nyquist`/`wavelength`; other helpers ignore it.
pub(super) type Decoder = fn(raw: u16, params: DecodeParams) -> f32;

/// Pass-through (no decode applied) — used for IRIS-only moments whose
/// calibration xradar lists with `func: None` (e.g. DB_HCLASS, DB_DBTE8,
/// DB_DBZE8). We keep the `raw == 0 → NaN` sentinel here because these are
/// integer/categorical fields where 0 means "no value"; they are out of
/// scope for the xradar power-moment parity work.
pub(super) const DECODE_NONE: Decoder = |raw, _p| {
    if raw == 0 {
        f32::NAN
    } else {
        raw as f32
    }
};

/// Linear 16-bit decoder, `(raw - 32768) / 100`. xradar decodes the modern
/// 2-byte reflectivity, velocity, ZDR and KDP types identically — plain
/// `decode_array(scale=100, offset=-32768)` with no mask — so the per-moment
/// `DECODE_*_2BYTE` aliases below all point here rather than repeat the
/// formula.
pub(super) const DECODE_LINEAR_2BYTE: Decoder = |raw, _p| (raw as f32 - 32768.0) / 100.0;

/// 8-bit DBZ-family decoder: scale=2.0, offset=-64.0. Used for DB_DBT,
/// DB_DBZ, DB_DBZC. No masking (xradar `decode_array`, no `mask`).
pub(super) const DECODE_DBZ_8BIT: Decoder = |raw, _p| (raw as f32 - 64.0) / 2.0;

/// 16-bit DBZ-family decoder (DB_DBT2, DB_DBZ2). See [`DECODE_LINEAR_2BYTE`].
pub(super) const DECODE_DBZ_2BYTE: Decoder = DECODE_LINEAR_2BYTE;

/// 8-bit velocity decoder: `value = nyquist * (raw - 128) / 127`.
///
/// xradar's `decode_vel` carries `mask: 0.0`, but that mask does **not**
/// surface as `NaN` in xradar's output: `np.ma.masked_equal(raw, 0)` leaves
/// the underlying datum untouched, and the masked arithmetic skips it, so the
/// final value at `raw == 0` collapses to `0.0` m/s (verified byte-for-byte
/// against `open_iris_datatree`, issue #28). We mirror that exactly — mapping
/// `raw == 0 → 0.0`, not `NaN` and not the formula's `-nyquist·128/127`.
pub(super) const DECODE_VEL_8BIT: Decoder = |raw, p| {
    if raw == 0 {
        0.0
    } else {
        p.nyquist_ms * (raw as f32 - 128.0) / 127.0
    }
};

/// 16-bit velocity decoder (DB_VEL2, m/s). See [`DECODE_LINEAR_2BYTE`].
pub(super) const DECODE_VEL_2BYTE: Decoder = DECODE_LINEAR_2BYTE;

/// 8-bit spectrum-width decoder: width = nyquist * raw / 256. No masking.
pub(super) const DECODE_WIDTH_8BIT: Decoder = |raw, p| p.nyquist_ms * (raw as f32) / 256.0;

/// 16-bit spectrum-width decoder: scale=100.0 (no offset, no mask).
pub(super) const DECODE_WIDTH_2BYTE: Decoder = |raw, _p| (raw as f32) / 100.0;

/// 8-bit ZDR decoder: scale=16.0, offset=-128.0. No masking.
pub(super) const DECODE_ZDR_8BIT: Decoder = |raw, _p| (raw as f32 - 128.0) / 16.0;

/// 16-bit ZDR decoder (DB_ZDR2). See [`DECODE_LINEAR_2BYTE`].
pub(super) const DECODE_ZDR_2BYTE: Decoder = DECODE_LINEAR_2BYTE;

/// 8-bit PHIDP decoder: 180.0 * (raw - 1) / 254.0 (degrees). No masking
/// (xradar `decode_phidp`, no `mask`).
pub(super) const DECODE_PHIDP_8BIT: Decoder = |raw, _p| 180.0 * (raw as f32 - 1.0) / 254.0;

/// 16-bit PHIDP decoder: 360.0 * raw / 65535 - 180.0 (degrees, full range).
/// No masking.
pub(super) const DECODE_PHIDP_2BYTE: Decoder = |raw, _p| 360.0 * (raw as f32) / 65535.0 - 180.0;

/// 8-bit RHOHV/SQI decoder: sqrt((raw - 1) / 253.0). xradar's `decode_sqi`
/// masks nothing explicitly — `raw == 0` falls out as `sqrt(-1/253) = NaN`
/// naturally, so we just apply the formula.
pub(super) const DECODE_RHOHV_8BIT: Decoder = |raw, _p| ((raw as f32 - 1.0) / 253.0).sqrt();

/// 16-bit RHOHV decoder: (raw - 1) / 65533.0. No masking (xradar
/// `decode_array`, offset=-1, no `mask`).
pub(super) const DECODE_RHOHV_2BYTE: Decoder = |raw, _p| (raw as f32 - 1.0) / 65533.0;

/// 8-bit KDP decoder, mirroring xradar's `decode_kdp` (4.4.20). The raw byte
/// is a *signed* int8: `0` and `-1` are the no-data sentinels (→ NaN),
/// `-128` maps to exactly 0, and everything else takes the exponential
/// transform divided by the radar wavelength (cm):
///   `-0.25 * sign(d) * 600 ** ((127 - |d|) / 126) / wavelength`.
pub(super) const DECODE_KDP_8BIT: Decoder = |raw, p| {
    let d = (raw as u8) as i8;
    if d == 0 || d == -1 || p.wavelength_cm <= 0.0 {
        // raw 0/-1 are the IRIS no-data sentinels; a non-positive wavelength
        // means the radar's calibration wasn't decoded, so KDP is undefined
        // (guards the divide below from producing ±inf).
        f32::NAN
    } else if d == -128 {
        0.0
    } else {
        let df = d as f32;
        let v = -0.25 * df.signum() * 600f32.powf((127.0 - df.abs()) / 126.0);
        v / p.wavelength_cm
    }
};

/// 16-bit KDP decoder (DB_KDP2). See [`DECODE_LINEAR_2BYTE`].
pub(super) const DECODE_KDP_2BYTE: Decoder = DECODE_LINEAR_2BYTE;

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience params with the given nyquist and a fixed S-band-ish
    /// wavelength (10 cm) for KDP tests.
    fn params(nyquist_ms: f32) -> DecodeParams {
        DecodeParams {
            nyquist_ms,
            wavelength_cm: 10.0,
        }
    }

    /// DBZ 8-bit is unmasked now: `raw=0 → (0-64)/2 = -32 dBZ` (finite),
    /// `raw=128 → 32 dBZ`, `raw=64 → 0 dBZ`.
    #[test]
    fn decode_dbz_8bit_pin_known_values() {
        assert_eq!(DECODE_DBZ_8BIT(0, params(0.0)), -32.0);
        assert_eq!(DECODE_DBZ_8BIT(128, params(0.0)), 32.0);
        assert_eq!(DECODE_DBZ_8BIT(64, params(0.0)), 0.0);
    }

    /// Velocity: `raw=0 → 0.0 m/s` (xradar's `mask:0.0` collapses to the
    /// underlying 0, not NaN), `raw=128 → 0 m/s`, `raw=255 → +nyquist`.
    #[test]
    fn decode_vel_8bit_with_nyquist_25() {
        let p = params(25.0);
        assert_eq!(DECODE_VEL_8BIT(0, p), 0.0);
        assert!((DECODE_VEL_8BIT(128, p) - 0.0).abs() < 1e-6);
        assert!((DECODE_VEL_8BIT(255, p) - 25.0).abs() < 1e-6);
    }

    /// PHIDP 8-bit is unmasked: `raw=0 → 180*(0-1)/254 ≈ -0.71°` (finite),
    /// `raw=128 → ~90°`.
    #[test]
    fn decode_phidp_8bit_values() {
        let v0 = DECODE_PHIDP_8BIT(0, params(0.0));
        assert!(
            (v0 - (-0.7086614)).abs() < 1e-4,
            "expected ~-0.71°, got {v0}"
        );
        let v = DECODE_PHIDP_8BIT(128, params(0.0));
        assert!((v - 90.0).abs() < 0.1, "expected ~90°, got {v}");
    }

    /// 16-bit DBZ is unmasked: `raw=0 → -327.68`, `raw=65535 → 327.67`,
    /// `raw=32768 → 0 dBZ`.
    #[test]
    fn decode_dbz_2byte_unmasked() {
        assert_eq!(DECODE_DBZ_2BYTE(0, params(0.0)), -327.68);
        assert!((DECODE_DBZ_2BYTE(65535, params(0.0)) - 327.67).abs() < 1e-2);
        assert_eq!(DECODE_DBZ_2BYTE(32768, params(0.0)), 0.0);
    }

    /// RHOHV 8-bit: `raw=0 → sqrt(-1/253) = NaN` (natural, matches
    /// `decode_sqi`); `raw=254 → sqrt(253/253) = 1.0`.
    #[test]
    fn decode_rhohv_8bit_max_is_one() {
        assert!(DECODE_RHOHV_8BIT(0, params(0.0)).is_nan());
        let v = DECODE_RHOHV_8BIT(254, params(0.0));
        assert!((v - 1.0).abs() < 1e-6, "expected 1.0, got {v}");
    }

    /// KDP 8-bit: `0`/`255(-1)` → NaN, `128(-128)` → exactly 0, and a known
    /// signed value matches xradar's exponential transform at λ=10 cm.
    #[test]
    fn decode_kdp_8bit_pin_known_values() {
        let p = params(10.0);
        assert!(DECODE_KDP_8BIT(0, p).is_nan());
        assert!(DECODE_KDP_8BIT(255, p).is_nan()); // -1
        assert_eq!(DECODE_KDP_8BIT(128, p), 0.0); // -128

        // d = 100 (positive): -0.25 * 1 * 600^((127-100)/126) / 10
        let expected = -0.25 * 600f32.powf((127.0 - 100.0) / 126.0) / 10.0;
        let got = DECODE_KDP_8BIT(100, p);
        assert!(
            (got - expected).abs() < 1e-6,
            "expected {expected}, got {got}"
        );
    }

    /// A missing/zero wavelength must not produce ±inf — KDP is undefined.
    #[test]
    fn decode_kdp_8bit_zero_wavelength_is_nan() {
        let p = DecodeParams {
            nyquist_ms: 0.0,
            wavelength_cm: 0.0,
        };
        assert!(DECODE_KDP_8BIT(100, p).is_nan());
    }

    /// When PRF is unavailable nyquist falls back to 0, zeroing velocity and
    /// width rather than producing garbage.
    #[test]
    fn decode_vel_width_8bit_zero_nyquist() {
        let p = params(0.0);
        assert_eq!(DECODE_VEL_8BIT(200, p), 0.0);
        assert_eq!(DECODE_WIDTH_8BIT(200, p), 0.0);
    }
}

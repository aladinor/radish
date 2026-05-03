//! Per-IRIS-data-type calibration helpers.
//!
//! Each IRIS data type has its own encoding rule. Most are linear:
//! `value = (raw - offset) / scale`. Velocity is Nyquist-scaled, phase is
//! mapped to [-180°, 180°], and a few cap-related types use explicit
//! masking. Helpers here mirror xradar's `iris.py` `decode_*` family
//! (`decode_array`, `decode_vel`, `decode_width`, `decode_phidp`, etc.)
//! so per-gate values match xradar's output to floating-point precision.
//!
//! Each helper takes a `raw: u16` (we widen 8-bit raws on read) plus a
//! `nyquist: f32` for velocity-style decoders that need it. Returns
//! `f32::NAN` for the IRIS "no data" sentinel (raw == 0 for most types,
//! raw == 0 OR raw == 65535 for some 16-bit types).

/// Function pointer type for per-gate decoders. The signature carries
/// `nyquist` so velocity-style helpers can scale by it; non-velocity
/// helpers just ignore the argument.
pub(super) type Decoder = fn(raw: u16, nyquist: f32) -> f32;

/// Pass-through (no decode applied) — used for moments whose calibration
/// is either not yet implemented or genuinely linear-1:1 (xradar lists
/// these with `func: None`). We still map raw == 0 → NaN to honour the
/// no-data sentinel convention.
pub(super) const DECODE_NONE: Decoder = |raw, _nyq| {
    if raw == 0 {
        f32::NAN
    } else {
        raw as f32
    }
};

/// 8-bit DBZ-family decoder: scale=2.0, offset=-64.0. Used for DB_DBT,
/// DB_DBZ, DB_DBZC. raw==0 is the no-data sentinel.
pub(super) const DECODE_DBZ_8BIT: Decoder = |raw, _nyq| {
    if raw == 0 {
        f32::NAN
    } else {
        (raw as f32 - 64.0) / 2.0
    }
};

/// 16-bit DBZ-family decoder: scale=100.0, offset=-32768.0. Used for
/// DB_DBT2, DB_DBZ2.
pub(super) const DECODE_DBZ_2BYTE: Decoder = |raw, _nyq| {
    if raw == 0 || raw == 65535 {
        f32::NAN
    } else {
        (raw as f32 - 32768.0) / 100.0
    }
};

/// 8-bit velocity decoder. xradar's `decode_vel`:
///   `value = nyquist * (raw - 128) / 127`
/// raw==0 is the sentinel.
pub(super) const DECODE_VEL_8BIT: Decoder = |raw, nyq| {
    if raw == 0 {
        f32::NAN
    } else {
        nyq * (raw as f32 - 128.0) / 127.0
    }
};

/// 16-bit velocity decoder: scale=100.0, offset=-32768.0 (m/s).
pub(super) const DECODE_VEL_2BYTE: Decoder = |raw, _nyq| {
    if raw == 0 || raw == 65535 {
        f32::NAN
    } else {
        (raw as f32 - 32768.0) / 100.0
    }
};

/// 8-bit spectrum-width decoder: width = nyquist * raw / 256. raw==0 sentinel.
pub(super) const DECODE_WIDTH_8BIT: Decoder = |raw, nyq| {
    if raw == 0 {
        f32::NAN
    } else {
        nyq * (raw as f32) / 256.0
    }
};

/// 16-bit spectrum-width decoder: scale=100.0 (no offset).
pub(super) const DECODE_WIDTH_2BYTE: Decoder = |raw, _nyq| {
    if raw == 0 {
        f32::NAN
    } else {
        (raw as f32) / 100.0
    }
};

/// 8-bit ZDR decoder: scale=16.0, offset=-128.0.
pub(super) const DECODE_ZDR_8BIT: Decoder = |raw, _nyq| {
    if raw == 0 {
        f32::NAN
    } else {
        (raw as f32 - 128.0) / 16.0
    }
};

/// 16-bit ZDR decoder: scale=100.0, offset=-32768.0.
pub(super) const DECODE_ZDR_2BYTE: Decoder = |raw, _nyq| {
    if raw == 0 || raw == 65535 {
        f32::NAN
    } else {
        (raw as f32 - 32768.0) / 100.0
    }
};

/// 8-bit PHIDP decoder: 180.0 * (raw - 1) / 254.0 (degrees).
pub(super) const DECODE_PHIDP_8BIT: Decoder = |raw, _nyq| {
    if raw == 0 {
        f32::NAN
    } else {
        180.0 * (raw as f32 - 1.0) / 254.0
    }
};

/// 16-bit PHIDP decoder: 360.0 * raw / 65535 - 180.0 (degrees, full range).
pub(super) const DECODE_PHIDP_2BYTE: Decoder = |raw, _nyq| {
    if raw == 0 || raw == 65535 {
        f32::NAN
    } else {
        360.0 * (raw as f32) / 65535.0 - 180.0
    }
};

/// 8-bit RHOHV decoder: sqrt((raw - 1) / 253.0).
pub(super) const DECODE_RHOHV_8BIT: Decoder = |raw, _nyq| {
    if raw == 0 {
        f32::NAN
    } else {
        ((raw as f32 - 1.0) / 253.0).sqrt()
    }
};

/// 16-bit RHOHV decoder: (raw - 1) / 65533.0.
pub(super) const DECODE_RHOHV_2BYTE: Decoder = |raw, _nyq| {
    if raw == 0 || raw == 65535 {
        f32::NAN
    } else {
        (raw as f32 - 1.0) / 65533.0
    }
};

#[cfg(test)]
mod tests {
    use super::*;

    /// xradar's DECODE_DBZ_8BIT formula: `raw=0 → NaN; raw=128 → (128-64)/2 = 32 dBZ`.
    #[test]
    fn decode_dbz_8bit_pin_known_values() {
        assert!(DECODE_DBZ_8BIT(0, 0.0).is_nan());
        assert_eq!(DECODE_DBZ_8BIT(128, 0.0), 32.0);
        assert_eq!(DECODE_DBZ_8BIT(64, 0.0), 0.0);
    }

    /// `raw=128 (mid-range) → 0 m/s; raw=255 → ~+nyquist; raw=1 → ~-nyquist`.
    #[test]
    fn decode_vel_8bit_with_nyquist_25() {
        let nyq = 25.0;
        assert!(DECODE_VEL_8BIT(0, nyq).is_nan());
        // raw=128 → (128-128)/127 * 25 = 0
        assert!((DECODE_VEL_8BIT(128, nyq) - 0.0).abs() < 1e-6);
        // raw=255 → (255-128)/127 * 25 = +25
        assert!((DECODE_VEL_8BIT(255, nyq) - 25.0).abs() < 1e-6);
    }

    /// PHIDP 8-bit: raw=128 (mid) → 180*(128-1)/254 = ~89.96°
    #[test]
    fn decode_phidp_8bit_mid_range() {
        assert!(DECODE_PHIDP_8BIT(0, 0.0).is_nan());
        let v = DECODE_PHIDP_8BIT(128, 0.0);
        assert!((v - 90.0).abs() < 0.1, "expected ~90°, got {v}");
    }

    /// 16-bit DBZ at raw=32768 → 0 dBZ (mid-scale)
    #[test]
    fn decode_dbz_2byte_zero_at_midrange() {
        assert!(DECODE_DBZ_2BYTE(0, 0.0).is_nan());
        assert!(DECODE_DBZ_2BYTE(65535, 0.0).is_nan());
        assert_eq!(DECODE_DBZ_2BYTE(32768, 0.0), 0.0);
    }

    /// 8-bit RHOHV: raw=254 → sqrt(253/253) = 1.0
    #[test]
    fn decode_rhohv_8bit_max_is_one() {
        assert!(DECODE_RHOHV_8BIT(0, 0.0).is_nan());
        let v = DECODE_RHOHV_8BIT(254, 0.0);
        assert!((v - 1.0).abs() < 1e-6, "expected 1.0, got {v}");
    }
}

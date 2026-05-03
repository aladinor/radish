//! Mapping from IRIS data-type bytes to ODIM moment names + per-byte
//! decoder selection.
//!
//! Source: xradar's `iris_mapping` (`xradar/io/backends/iris.py:82`) and
//! `SIGMET_DATA_TYPES` (line 1755+). Every IRIS data-type id in
//! `iris_mapping` shows up here under the same ODIM short name xradar
//! emits, with the same calibration formula. Ids that aren't in
//! `iris_mapping` (`DB_HCLASS`, `DB_DBTE8`, `DB_DBZE8`, …) get emitted
//! under their IRIS short name with no ODIM translation, again
//! mirroring xradar.
//!
//! CF metadata strings (units / standard_name / long_name) for the
//! ODIM-mapped moments live in `crate::backends::common::metadata`.
//! Unmapped moments emit minimal CF attrs (units only, derived from
//! the calibration helper) since there's no ODIM standard name for
//! them.

use crate::backends::common::{meta_for, OdimMomentMeta};

use super::calibration::{
    Decoder, DECODE_DBZ_2BYTE, DECODE_DBZ_8BIT, DECODE_NONE, DECODE_PHIDP_2BYTE, DECODE_PHIDP_8BIT,
    DECODE_RHOHV_2BYTE, DECODE_RHOHV_8BIT, DECODE_VEL_2BYTE, DECODE_VEL_8BIT, DECODE_WIDTH_2BYTE,
    DECODE_WIDTH_8BIT, DECODE_ZDR_2BYTE, DECODE_ZDR_8BIT,
};

/// One entry in the IRIS data-type table.
///
/// `data_type_id` is the bit position in `dsp_data_mask` and the byte
/// the per-ray header records. `bytes_per_bin` is 1 (most legacy types)
/// or 2 (modern double-precision variants).
///
/// `output_name` is what we emit as the variable name in the resulting
/// xarray Dataset. For ODIM-mapped types it's the ODIM short name
/// (`"DBZH"`, `"VRADH"`, …). For unmapped types it's the IRIS short
/// name (`"DB_HCLASS"`, `"DB_DBTE8"`, …) — same convention xradar uses.
#[allow(dead_code)]
pub(super) struct SigmetMoment {
    /// IRIS data-type id (e.g. 2 = DB_DBZ, 9 = DB_DBZ2).
    pub(super) data_type_id: u8,
    /// Source name from the IRIS spec (e.g. `"DB_DBZ"`).
    pub(super) iris_name: &'static str,
    /// Variable name to emit in the xarray Dataset. ODIM short name for
    /// types in xradar's `iris_mapping`; IRIS short name otherwise.
    pub(super) output_name: &'static str,
    /// Number of raw bytes per gate (1 or 2).
    pub(super) bytes_per_bin: u8,
    /// Decoder applied to each gate's raw value. Returns `f32::NAN` for
    /// "no data" sentinels (raw == 0 or raw == 1, depending on type).
    pub(super) decoder: Decoder,
    /// Units string for the unmapped (IRIS-short-name) variant. For
    /// ODIM-mapped types this is unused — CF metadata comes from
    /// `common::metadata::TABLE` keyed on `output_name`.
    pub(super) units: &'static str,
}

/// Full mapping table.
///
/// **IDs verified against `xradar.io.backends.iris.SIGMET_DATA_TYPES`**
/// (a previous draft had three IDs swapped: 17 was DB_PHIDP2 instead of
/// DB_VELC, 19 was DB_SQI instead of DB_RHOHV, 20 was DB_RHOHV instead
/// of DB_RHOHV2 — see commit history). Re-syncing the IDs is the only
/// reliable way to keep parity with xradar across IRIS fixtures, since
/// the IRIS ICD numbers data types densely and a single off-by-one
/// silently mis-decodes whole moments.
pub(super) const SUPPORTED_MOMENTS: &[SigmetMoment] = &[
    // 8-bit legacy variants ------------------------------------------------
    SigmetMoment {
        data_type_id: 1,
        iris_name: "DB_DBT",
        output_name: "DBTH",
        bytes_per_bin: 1,
        decoder: DECODE_DBZ_8BIT,
        units: "",
    },
    SigmetMoment {
        data_type_id: 2,
        iris_name: "DB_DBZ",
        output_name: "DBZH",
        bytes_per_bin: 1,
        decoder: DECODE_DBZ_8BIT,
        units: "",
    },
    SigmetMoment {
        data_type_id: 3,
        iris_name: "DB_VEL",
        output_name: "VRADH",
        bytes_per_bin: 1,
        decoder: DECODE_VEL_8BIT,
        units: "",
    },
    SigmetMoment {
        data_type_id: 4,
        iris_name: "DB_WIDTH",
        output_name: "WRADH",
        bytes_per_bin: 1,
        decoder: DECODE_WIDTH_8BIT,
        units: "",
    },
    SigmetMoment {
        data_type_id: 5,
        iris_name: "DB_ZDR",
        output_name: "ZDR",
        bytes_per_bin: 1,
        decoder: DECODE_ZDR_8BIT,
        units: "",
    },
    SigmetMoment {
        data_type_id: 14,
        iris_name: "DB_KDP",
        output_name: "KDP",
        bytes_per_bin: 1,
        decoder: DECODE_NONE,
        units: "",
    },
    SigmetMoment {
        data_type_id: 16,
        iris_name: "DB_PHIDP",
        output_name: "PHIDP",
        bytes_per_bin: 1,
        decoder: DECODE_PHIDP_8BIT,
        units: "",
    },
    SigmetMoment {
        data_type_id: 18,
        iris_name: "DB_SQI",
        output_name: "SQIH",
        bytes_per_bin: 1,
        decoder: DECODE_NONE,
        units: "",
    },
    SigmetMoment {
        data_type_id: 19,
        iris_name: "DB_RHOHV",
        output_name: "RHOHV",
        bytes_per_bin: 1,
        decoder: DECODE_RHOHV_8BIT,
        units: "",
    },
    // 16-bit modern variants -----------------------------------------------
    SigmetMoment {
        data_type_id: 8,
        iris_name: "DB_DBT2",
        output_name: "DBTH",
        bytes_per_bin: 2,
        decoder: DECODE_DBZ_2BYTE,
        units: "",
    },
    SigmetMoment {
        data_type_id: 9,
        iris_name: "DB_DBZ2",
        output_name: "DBZH",
        bytes_per_bin: 2,
        decoder: DECODE_DBZ_2BYTE,
        units: "",
    },
    SigmetMoment {
        data_type_id: 10,
        iris_name: "DB_VEL2",
        output_name: "VRADH",
        bytes_per_bin: 2,
        decoder: DECODE_VEL_2BYTE,
        units: "",
    },
    SigmetMoment {
        data_type_id: 11,
        iris_name: "DB_WIDTH2",
        output_name: "WRADH",
        bytes_per_bin: 2,
        decoder: DECODE_WIDTH_2BYTE,
        units: "",
    },
    SigmetMoment {
        data_type_id: 12,
        iris_name: "DB_ZDR2",
        output_name: "ZDR",
        bytes_per_bin: 2,
        decoder: DECODE_ZDR_2BYTE,
        units: "",
    },
    SigmetMoment {
        data_type_id: 15,
        iris_name: "DB_KDP2",
        output_name: "KDP",
        bytes_per_bin: 2,
        decoder: DECODE_NONE,
        units: "",
    },
    SigmetMoment {
        data_type_id: 20,
        iris_name: "DB_RHOHV2",
        output_name: "RHOHV",
        bytes_per_bin: 2,
        decoder: DECODE_RHOHV_2BYTE,
        units: "",
    },
    SigmetMoment {
        data_type_id: 23,
        iris_name: "DB_SQI2",
        output_name: "SQIH",
        bytes_per_bin: 2,
        decoder: DECODE_NONE,
        units: "",
    },
    SigmetMoment {
        data_type_id: 24,
        iris_name: "DB_PHIDP2",
        output_name: "PHIDP",
        bytes_per_bin: 2,
        decoder: DECODE_PHIDP_2BYTE,
        units: "",
    },
    SigmetMoment {
        data_type_id: 66,
        iris_name: "DB_SNR16",
        output_name: "SNRH",
        bytes_per_bin: 2,
        decoder: DECODE_NONE,
        units: "",
    },
    // Unmapped (no ODIM short name in xradar's `iris_mapping`) — emit
    // under the IRIS short name. Calibration matches xradar's
    // `SIGMET_DATA_TYPES` decode_array fkw entries.
    SigmetMoment {
        data_type_id: 55,
        iris_name: "DB_HCLASS",
        output_name: "DB_HCLASS",
        bytes_per_bin: 1,
        decoder: DECODE_NONE,
        units: "1",
    },
    SigmetMoment {
        data_type_id: 71,
        iris_name: "DB_DBTE8",
        output_name: "DB_DBTE8",
        bytes_per_bin: 1,
        decoder: DECODE_DBZ_8BIT,
        units: "dBZ",
    },
    SigmetMoment {
        data_type_id: 73,
        iris_name: "DB_DBZE8",
        output_name: "DB_DBZE8",
        bytes_per_bin: 1,
        decoder: DECODE_DBZ_8BIT,
        units: "dBZ",
    },
];

/// Look up a moment definition by its IRIS data-type id. Returns `None`
/// for unknown / unsupported ids — adapter callers skip those rather
/// than failing the whole decode.
pub(super) fn moment_for_id(data_type_id: u8) -> Option<&'static SigmetMoment> {
    SUPPORTED_MOMENTS
        .iter()
        .find(|m| m.data_type_id == data_type_id)
}

/// Look up CF metadata for an ODIM-mapped moment. Returns `None` for
/// unmapped types (DB_HCLASS, DB_DBTE8, DB_DBZE8) — those emit using
/// the IRIS short name as the variable name and the per-row `units`
/// field as the only CF attr.
pub(super) fn cf_metadata_for(moment: &SigmetMoment) -> Option<&'static OdimMomentMeta> {
    meta_for(moment.output_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbz_maps_to_dbzh() {
        let m = moment_for_id(2).expect("DB_DBZ is supported");
        assert_eq!(m.iris_name, "DB_DBZ");
        assert_eq!(m.output_name, "DBZH");
        assert_eq!(m.bytes_per_bin, 1);
    }

    #[test]
    fn dbz2_and_dbz_share_output_name() {
        let m1 = moment_for_id(2).unwrap();
        let m2 = moment_for_id(9).unwrap();
        assert_eq!(m1.output_name, m2.output_name);
        assert_eq!(m1.bytes_per_bin, 1);
        assert_eq!(m2.bytes_per_bin, 2);
    }

    /// Pin the IDs that previously had errors. If any of these regress,
    /// the silent-mis-decode bug from the audit re-introduces.
    #[test]
    fn correct_ids_for_polarimetric_moments() {
        // 18: DB_SQI (uint8)        — was incorrectly 19 in v0
        // 19: DB_RHOHV (uint8)      — was incorrectly 20
        // 20: DB_RHOHV2 (uint16)    — was incorrectly 22
        // 24: DB_PHIDP2 (uint16)    — was incorrectly 17
        // 66: DB_SNR16 (uint16)     — was incorrectly 21
        assert_eq!(moment_for_id(18).unwrap().iris_name, "DB_SQI");
        assert_eq!(moment_for_id(19).unwrap().iris_name, "DB_RHOHV");
        assert_eq!(moment_for_id(20).unwrap().iris_name, "DB_RHOHV2");
        assert_eq!(moment_for_id(24).unwrap().iris_name, "DB_PHIDP2");
        assert_eq!(moment_for_id(66).unwrap().iris_name, "DB_SNR16");
    }

    /// The unmapped types (no ODIM equivalent) emit under their IRIS
    /// short name — pin that contract so a future ODIM expansion
    /// doesn't accidentally rename them.
    #[test]
    fn unmapped_types_emit_iris_short_name() {
        assert_eq!(moment_for_id(55).unwrap().output_name, "DB_HCLASS");
        assert_eq!(moment_for_id(71).unwrap().output_name, "DB_DBTE8");
        assert_eq!(moment_for_id(73).unwrap().output_name, "DB_DBZE8");
    }

    #[test]
    fn cf_metadata_lookup_for_odim_mapped_moments() {
        for m in SUPPORTED_MOMENTS {
            // ODIM-mapped moments must have CF metadata; unmapped ones
            // explicitly don't (None). Use the carved-out `units` field
            // to identify the unmapped subset.
            if m.units.is_empty() {
                assert!(
                    cf_metadata_for(m).is_some(),
                    "no CF metadata for ODIM name {:?} (IRIS {})",
                    m.output_name,
                    m.iris_name,
                );
            } else {
                assert!(
                    cf_metadata_for(m).is_none(),
                    "unmapped moment {} should not have CF metadata",
                    m.iris_name,
                );
            }
        }
    }

    #[test]
    fn unknown_iris_id_returns_none() {
        assert!(moment_for_id(99).is_none());
    }
}

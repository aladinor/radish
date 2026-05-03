//! Mapping from IRIS data-type bytes to ODIM moment names + per-byte
//! decoder selection.
//!
//! Source: xradar's `iris_mapping` (`xradar/io/backends/iris.py:82`) and
//! `SIGMET_DATA_TYPES` (line 1755+). `*` 8-bit and `*2` 16-bit variants
//! both map to the same ODIM short name; the table here disambiguates by
//! recording the decoder + bytes-per-bin per IRIS data-type id.
//!
//! CF metadata strings (units / standard_name / long_name) live in
//! `crate::backends::common::metadata` so every backend producing the
//! same ODIM moment shares the same xarray attributes.

use crate::backends::common::{meta_for, OdimMomentMeta};

use super::calibration::{Decoder, DECODE_DBZ_2BYTE, DECODE_DBZ_8BIT, DECODE_NONE, DECODE_PHIDP_2BYTE, DECODE_PHIDP_8BIT, DECODE_RHOHV_2BYTE, DECODE_RHOHV_8BIT, DECODE_VEL_2BYTE, DECODE_VEL_8BIT, DECODE_WIDTH_2BYTE, DECODE_WIDTH_8BIT, DECODE_ZDR_2BYTE, DECODE_ZDR_8BIT};

/// One entry in the IRIS data-type table. The `data_type_id` is the
/// position bit in `dsp_data_mask` and the byte the per-ray header
/// records. `bytes_per_bin` is 1 (most legacy types) or 2 (modern
/// double-precision variants suffixed `2`).
pub(super) struct SigmetMoment {
    /// IRIS data-type id (e.g. 2 = DB_DBZ, 9 = DB_DBZ2).
    pub(super) data_type_id: u8,
    /// Source name from the IRIS spec (e.g. `"DB_DBZ"`).
    pub(super) iris_name: &'static str,
    /// ODIM short name we surface this moment as (e.g. `"DBZH"`).
    pub(super) odim_name: &'static str,
    /// Number of raw bytes per gate (1 or 2).
    pub(super) bytes_per_bin: u8,
    /// Decoder applied to each gate's raw value. Returns `f32::NAN` for
    /// "no data" sentinels (raw == 0 or raw == 1, depending on type).
    pub(super) decoder: Decoder,
}

/// Full mapping table. Order is significant only insofar as it controls
/// the variable-emission order in the resulting xarray Dataset; we follow
/// xradar's `SIGMET_DATA_TYPES` ordering.
pub(super) const SUPPORTED_MOMENTS: &[SigmetMoment] = &[
    // 8-bit legacy variants ------------------------------------------------
    SigmetMoment { data_type_id: 1,  iris_name: "DB_DBT",    odim_name: "DBTH",  bytes_per_bin: 1, decoder: DECODE_DBZ_8BIT },
    SigmetMoment { data_type_id: 2,  iris_name: "DB_DBZ",    odim_name: "DBZH",  bytes_per_bin: 1, decoder: DECODE_DBZ_8BIT },
    SigmetMoment { data_type_id: 3,  iris_name: "DB_VEL",    odim_name: "VRADH", bytes_per_bin: 1, decoder: DECODE_VEL_8BIT },
    SigmetMoment { data_type_id: 4,  iris_name: "DB_WIDTH",  odim_name: "WRADH", bytes_per_bin: 1, decoder: DECODE_WIDTH_8BIT },
    SigmetMoment { data_type_id: 5,  iris_name: "DB_ZDR",    odim_name: "ZDR",   bytes_per_bin: 1, decoder: DECODE_ZDR_8BIT },
    SigmetMoment { data_type_id: 14, iris_name: "DB_KDP",    odim_name: "KDP",   bytes_per_bin: 1, decoder: DECODE_NONE },
    SigmetMoment { data_type_id: 16, iris_name: "DB_PHIDP",  odim_name: "PHIDP", bytes_per_bin: 1, decoder: DECODE_PHIDP_8BIT },
    SigmetMoment { data_type_id: 19, iris_name: "DB_SQI",    odim_name: "SQIH",  bytes_per_bin: 1, decoder: DECODE_NONE },
    SigmetMoment { data_type_id: 20, iris_name: "DB_RHOHV",  odim_name: "RHOHV", bytes_per_bin: 1, decoder: DECODE_RHOHV_8BIT },
    // 16-bit modern variants -----------------------------------------------
    SigmetMoment { data_type_id: 8,  iris_name: "DB_DBT2",   odim_name: "DBTH",  bytes_per_bin: 2, decoder: DECODE_DBZ_2BYTE },
    SigmetMoment { data_type_id: 9,  iris_name: "DB_DBZ2",   odim_name: "DBZH",  bytes_per_bin: 2, decoder: DECODE_DBZ_2BYTE },
    SigmetMoment { data_type_id: 10, iris_name: "DB_VEL2",   odim_name: "VRADH", bytes_per_bin: 2, decoder: DECODE_VEL_2BYTE },
    SigmetMoment { data_type_id: 11, iris_name: "DB_WIDTH2", odim_name: "WRADH", bytes_per_bin: 2, decoder: DECODE_WIDTH_2BYTE },
    SigmetMoment { data_type_id: 12, iris_name: "DB_ZDR2",   odim_name: "ZDR",   bytes_per_bin: 2, decoder: DECODE_ZDR_2BYTE },
    SigmetMoment { data_type_id: 15, iris_name: "DB_KDP2",   odim_name: "KDP",   bytes_per_bin: 2, decoder: DECODE_NONE },
    SigmetMoment { data_type_id: 17, iris_name: "DB_PHIDP2", odim_name: "PHIDP", bytes_per_bin: 2, decoder: DECODE_PHIDP_2BYTE },
    SigmetMoment { data_type_id: 21, iris_name: "DB_SNR16",  odim_name: "SNRH",  bytes_per_bin: 2, decoder: DECODE_NONE },
    SigmetMoment { data_type_id: 22, iris_name: "DB_RHOHV2", odim_name: "RHOHV", bytes_per_bin: 2, decoder: DECODE_RHOHV_2BYTE },
];

/// Look up a moment definition by its IRIS data-type id. Returns `None`
/// for unknown / unsupported ids — adapter callers skip those rather
/// than failing the whole decode.
pub(super) fn moment_for_id(data_type_id: u8) -> Option<&'static SigmetMoment> {
    SUPPORTED_MOMENTS
        .iter()
        .find(|m| m.data_type_id == data_type_id)
}

/// Look up the CF metadata for the ODIM moment a given IRIS data-type
/// produces. Wraps the central `common::metadata::TABLE`.
pub(super) fn cf_metadata_for(moment: &SigmetMoment) -> Option<&'static OdimMomentMeta> {
    meta_for(moment.odim_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbz_maps_to_dbzh() {
        let m = moment_for_id(2).expect("DB_DBZ is supported");
        assert_eq!(m.iris_name, "DB_DBZ");
        assert_eq!(m.odim_name, "DBZH");
        assert_eq!(m.bytes_per_bin, 1);
    }

    #[test]
    fn dbz2_and_dbz_share_odim_name() {
        // 8-bit and 16-bit variants both surface as DBZH.
        let m1 = moment_for_id(2).unwrap();
        let m2 = moment_for_id(9).unwrap();
        assert_eq!(m1.odim_name, m2.odim_name);
        assert_eq!(m1.bytes_per_bin, 1);
        assert_eq!(m2.bytes_per_bin, 2);
    }

    #[test]
    fn cf_metadata_lookup_succeeds_for_every_supported_moment() {
        for m in SUPPORTED_MOMENTS {
            assert!(
                cf_metadata_for(m).is_some(),
                "no CF metadata for ODIM name {:?} (IRIS {})",
                m.odim_name,
                m.iris_name,
            );
        }
    }

    #[test]
    fn unknown_iris_id_returns_none() {
        assert!(moment_for_id(99).is_none());
    }
}

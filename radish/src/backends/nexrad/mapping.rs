//! Mapping from NEXRAD `Product`s to radish/ODIM moment names and CF metadata.
//!
//! ODIM short names come from `radish_types::moments` so the same canonical
//! constants are used across backends. The longer per-moment strings
//! (`standard_name`, `long_name`, `units`) match xradar's NEXRAD reader byte
//! for byte so an `xr.DataTree` produced by either engine has the same
//! per-DataArray attribute set.

use nexrad_model::data::Product;
use radish_types::moments;

/// Per-moment metadata used when translating a NEXRAD `Product` into a radish `MomentData`.
pub(super) struct MomentMeta {
    pub odim_name: &'static str,
    pub units: &'static str,
    pub standard_name: &'static str,
    pub long_name: &'static str,
}

/// `CCORH` isn't in `radish_types::moments` (which mirrors the CfRadial2 short
/// names, where this product is undefined). Keep it as a local constant for the
/// NEXRAD-specific `Product::ClutterFilterPower` mapping.
const CCORH: &str = "CCORH";

/// Map a NEXRAD `Product` to its ODIM moment name and CF metadata. Strings are
/// chosen to match xradar's `nexrad_mapping` table verbatim — keep this in
/// sync with `xradar/io/backends/nexrad_level2.py` so engine-swap users see
/// identical per-DataArray attrs.
pub(super) fn moment_meta(product: Product) -> MomentMeta {
    match product {
        Product::Reflectivity => MomentMeta {
            odim_name: moments::DBZH,
            units: "dBZ",
            standard_name: "radar_equivalent_reflectivity_factor_h",
            long_name: "Equivalent reflectivity factor H",
        },
        Product::Velocity => MomentMeta {
            odim_name: moments::VRADH,
            units: "meters per seconds",
            standard_name: "radial_velocity_of_scatterers_away_from_instrument_h",
            long_name: "Radial velocity of scatterers away from instrument H",
        },
        Product::SpectrumWidth => MomentMeta {
            odim_name: moments::WRADH,
            units: "meters per seconds",
            standard_name: "radar_doppler_spectrum_width_h",
            long_name: "Doppler spectrum width H",
        },
        Product::DifferentialReflectivity => MomentMeta {
            odim_name: moments::ZDR,
            units: "dB",
            standard_name: "radar_differential_reflectivity_hv",
            long_name: "Log differential reflectivity H/V",
        },
        Product::DifferentialPhase => MomentMeta {
            odim_name: moments::PHIDP,
            units: "degrees",
            standard_name: "radar_differential_phase_hv",
            long_name: "Differential phase HV",
        },
        Product::CorrelationCoefficient => MomentMeta {
            odim_name: moments::RHOHV,
            units: "unitless",
            standard_name: "radar_correlation_coefficient_hv",
            long_name: "Correlation coefficient HV",
        },
        Product::ClutterFilterPower => MomentMeta {
            odim_name: CCORH,
            units: "unitless",
            standard_name: "clutter_correction_h",
            long_name: "Clutter Correction H",
        },
    }
}

/// Products radish surfaces from a NEXRAD sweep, in a stable order so DataTree
/// variable order is deterministic across runs. The full set matches xradar's
/// NEXRAD reader exactly, including `ClutterFilterPower` (CCORH).
pub(super) const SUPPORTED_PRODUCTS: &[Product] = &[
    Product::Reflectivity,
    Product::Velocity,
    Product::SpectrumWidth,
    Product::DifferentialReflectivity,
    Product::DifferentialPhase,
    Product::CorrelationCoefficient,
    Product::ClutterFilterPower,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflectivity_maps_to_dbzh_with_xradar_strings() {
        let m = moment_meta(Product::Reflectivity);
        assert_eq!(m.odim_name, "DBZH");
        assert_eq!(m.units, "dBZ");
        assert_eq!(m.standard_name, "radar_equivalent_reflectivity_factor_h");
        assert_eq!(m.long_name, "Equivalent reflectivity factor H");
    }

    #[test]
    fn velocity_uses_xradar_units_string() {
        // xradar uses the unusual "meters per seconds" (with trailing s);
        // we mirror it verbatim so engine-swap parity holds.
        assert_eq!(moment_meta(Product::Velocity).units, "meters per seconds");
        assert_eq!(
            moment_meta(Product::SpectrumWidth).units,
            "meters per seconds"
        );
    }

    #[test]
    fn rho_uses_unitless_not_empty_string() {
        // xradar emits the literal "unitless" rather than "" for unit-free moments.
        assert_eq!(
            moment_meta(Product::CorrelationCoefficient).units,
            "unitless"
        );
        assert_eq!(moment_meta(Product::ClutterFilterPower).units, "unitless");
    }

    #[test]
    fn ccorh_present_in_supported_products() {
        assert!(SUPPORTED_PRODUCTS.contains(&Product::ClutterFilterPower));
    }

    #[test]
    fn names_match_radish_types_constants() {
        assert_eq!(moment_meta(Product::Reflectivity).odim_name, moments::DBZH);
        assert_eq!(moment_meta(Product::Velocity).odim_name, moments::VRADH);
        assert_eq!(
            moment_meta(Product::CorrelationCoefficient).odim_name,
            moments::RHOHV
        );
    }
}

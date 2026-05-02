//! Mapping from NEXRAD `Product`s to radish/ODIM moment names and metadata.
//!
//! ODIM short names come from `radish_types::moments` so the same canonical
//! constants are used across backends.

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
/// NEXRAD-specific `Product::ClutterFilterPower` mapping to match xradar.
const CCORH: &str = "CCORH";

/// Map a NEXRAD `Product` to its ODIM moment name and metadata. Aligned with
/// `xradar/io/backends/nexrad_level2.py::nexrad_mapping` so that consumers
/// switching engines see the same variable names.
pub(super) fn moment_meta(product: Product) -> MomentMeta {
    match product {
        Product::Reflectivity => MomentMeta {
            odim_name: moments::DBZH,
            units: "dBZ",
            standard_name: "equivalent_reflectivity_factor",
            long_name: "Equivalent reflectivity factor (horizontal channel)",
        },
        Product::Velocity => MomentMeta {
            odim_name: moments::VRADH,
            units: "m/s",
            standard_name: "radial_velocity_of_scatterers_away_from_instrument",
            long_name: "Radial velocity (horizontal channel)",
        },
        Product::SpectrumWidth => MomentMeta {
            odim_name: moments::WRADH,
            units: "m/s",
            standard_name: "doppler_spectrum_width",
            long_name: "Doppler spectrum width (horizontal channel)",
        },
        Product::DifferentialReflectivity => MomentMeta {
            odim_name: moments::ZDR,
            units: "dB",
            standard_name: "log_differential_reflectivity_hv",
            long_name: "Differential reflectivity",
        },
        Product::DifferentialPhase => MomentMeta {
            odim_name: moments::PHIDP,
            units: "degrees",
            standard_name: "differential_phase_hv",
            long_name: "Differential propagation phase",
        },
        Product::CorrelationCoefficient => MomentMeta {
            odim_name: moments::RHOHV,
            units: "",
            standard_name: "cross_correlation_ratio_hv",
            long_name: "Cross-correlation coefficient",
        },
        Product::ClutterFilterPower => MomentMeta {
            odim_name: CCORH,
            units: "dB",
            standard_name: "clutter_correction_horizontal",
            long_name: "Clutter filter power removed",
        },
    }
}

/// Products radish currently surfaces from a NEXRAD sweep, in a stable order
/// so DataTree variable order is deterministic across runs. ClutterFilterPower
/// is intentionally excluded — xradar surfaces it but it's a less commonly used
/// dual-pol clutter diagnostic; add to the list if a consumer needs it.
pub(super) const SUPPORTED_PRODUCTS: &[Product] = &[
    Product::Reflectivity,
    Product::Velocity,
    Product::SpectrumWidth,
    Product::DifferentialReflectivity,
    Product::DifferentialPhase,
    Product::CorrelationCoefficient,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflectivity_maps_to_dbzh() {
        let m = moment_meta(Product::Reflectivity);
        assert_eq!(m.odim_name, "DBZH");
        assert_eq!(m.units, "dBZ");
    }

    #[test]
    fn velocity_maps_to_vradh() {
        let m = moment_meta(Product::Velocity);
        assert_eq!(m.odim_name, "VRADH");
        assert_eq!(m.units, "m/s");
    }

    #[test]
    fn spectrum_width_maps_to_wradh() {
        let m = moment_meta(Product::SpectrumWidth);
        assert_eq!(m.odim_name, "WRADH");
    }

    #[test]
    fn rho_has_no_units() {
        assert_eq!(moment_meta(Product::CorrelationCoefficient).units, "");
    }

    #[test]
    fn names_match_radish_types_constants() {
        // Sanity: if anyone changes radish_types::moments, this catches the drift.
        assert_eq!(moment_meta(Product::Reflectivity).odim_name, moments::DBZH);
        assert_eq!(moment_meta(Product::Velocity).odim_name, moments::VRADH);
        assert_eq!(moment_meta(Product::CorrelationCoefficient).odim_name, moments::RHOHV);
    }
}

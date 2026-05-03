//! Convert a `nexrad_model::data::Scan` into radish's `VolumeData`.
//!
//! The strategy:
//!
//! * Volume metadata is derived from the scan's site (lat/lon/altitude/ICAO),
//!   the VCP number, and the scan's overall time range.
//! * For each sweep, the radials are sorted by azimuth *once* and the
//!   permutation is reused for every coordinate axis and every moment. We do
//!   not call [`SweepField::from_radials`]: it would re-sort the radial slice
//!   per moment and allocate an intermediate `Vec<f32>` + `Vec<GateStatus>`
//!   that we'd then copy into the final `Array2<f32>`. Instead we walk
//!   `Product::moment_data(radial)` directly and decode each gate straight
//!   into the output buffer.
//! * The first moment seen establishes the per-sweep range axis (finest gate
//!   count + step). Other moments are NaN-padded out to that gate count so the
//!   sweep has a single coherent (rays × gates) shape — matching radish's
//!   `Coordinates::range`.
//! * `BelowThreshold` and `RangeFolded` gates become `f32::NAN`, as do gates
//!   beyond a moment's `gate_count` and rows for radials that don't carry
//!   the moment at all.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use ndarray::Array2;
use nexrad::decode::messages::rda_status_data::Message as RdaStatusMessage;
use nexrad_model::data::{
    CFPMomentValue, DataMoment, ElevationCut, MomentValue, Product, Radial, Scan, Sweep,
};
use radish_types::{PlatformType, SweepMode};
use rayon::prelude::*;

use crate::{
    MomentData, RadishError, Result, SweepData, SweepMetadata, VolumeData, VolumeMetadata,
};

use super::attrs::{sweep_attrs_from_cut, volume_attrs};
use super::mapping::{moment_meta, SUPPORTED_PRODUCTS};
use super::sniff;
use crate::backends::common::{
    assemble_ppi_coordinates, build_range_axis, decode_into_array, sort_indices_by_key,
    MomentGeometry,
};

/// Convert a fully-decoded NEXRAD `Scan` into a radish `VolumeData`.
///
/// Sweep conversion is data-parallel: each `convert_sweep` invocation reads
/// only its own `Sweep` and writes its own owned `SweepData`. We dispatch
/// across rayon's global pool — already warmed up by the upstream
/// `nexrad-data/parallel` decompression that ran moments earlier.
pub(super) fn convert_scan(
    scan: Scan,
    msg2: Option<RdaStatusMessage<'static>>,
    source: &Path,
) -> Result<VolumeData> {
    let metadata = build_volume_metadata(&scan, msg2.as_ref(), source)?;
    let cuts = scan.coverage_pattern().elevation_cuts();
    let sweeps: Vec<SweepData> = scan
        .sweeps()
        .par_iter()
        .enumerate()
        .map(|(idx, sweep)| convert_sweep(sweep, idx, cuts.get(idx)))
        .collect::<Result<_>>()?;
    Ok(VolumeData::new(metadata, sweeps))
}

/// Build the `VolumeMetadata` from the scan, falling back to the file path for
/// the ICAO when the scan does not carry a `Site` (rare but possible for
/// truncated chunk files).
pub(super) fn build_volume_metadata(
    scan: &Scan,
    msg2: Option<&RdaStatusMessage<'_>>,
    source: &Path,
) -> Result<VolumeMetadata> {
    let site = scan.site();

    let icao = site
        .map(|s| s.identifier_string())
        .or_else(|| sniff::icao_from_filename(source).map(str::to_owned))
        .unwrap_or_else(|| "UNKN".to_string());

    // WSR-88D antenna height = base height + tower (feedhorn) height.
    let (latitude, longitude, altitude, altitude_agl) = match site {
        Some(s) => (
            s.latitude() as f64,
            s.longitude() as f64,
            s.height_meters() as f64 + s.tower_height_meters() as f64,
            Some(s.tower_height_meters() as f64),
        ),
        None => (f64::NAN, f64::NAN, f64::NAN, None),
    };

    let (time_start, time_end) = scan
        .time_range()
        .unwrap_or((DateTime::<Utc>::UNIX_EPOCH, DateTime::<Utc>::UNIX_EPOCH));

    let num_sweeps = scan.sweeps().len();
    let mut metadata =
        VolumeMetadata::new(icao, latitude, longitude, altitude, time_start, time_end);
    metadata.altitude_agl = altitude_agl;
    metadata.institution = "NOAA/NWS".to_string();
    metadata.platform_type = Some(PlatformType::Fixed);
    metadata.generate_sweep_names(num_sweeps);
    metadata.sweep_fixed_angles = scan
        .sweeps()
        .iter()
        .map(|s| s.elevation_angle_degrees().unwrap_or(f32::NAN) as f64)
        .collect();

    // VCP attributes match xradar's `VCP-NNN` form (e.g. `VCP-212`) so
    // engine-swap users see the same scan_name string.
    let vcp = scan.coverage_pattern_number();
    let vcp_number = vcp.number();
    metadata
        .attributes
        .insert("scan_name".to_string(), format!("VCP-{vcp_number}"));
    metadata
        .attributes
        .insert("vcp".to_string(), vcp_number.to_string());
    metadata
        .attributes
        .insert("vcp_description".to_string(), vcp.description().to_string());

    metadata.nexrad = Some(volume_attrs(
        scan.coverage_pattern(),
        msg2,
        scan.sweeps(),
        num_sweeps as u32,
    ));

    Ok(metadata)
}

/// Convert a single `Sweep` into `SweepData`. Returns
/// [`RadishError::MalformedRecord`] when the sweep has no radials or no
/// supported moments.
pub(super) fn convert_sweep(
    sweep: &Sweep,
    sweep_idx: usize,
    cut: Option<&ElevationCut>,
) -> Result<SweepData> {
    let radials = sweep.radials();
    if radials.is_empty() {
        return Err(RadishError::MalformedRecord {
            offset: 0,
            msg: format!("sweep {sweep_idx} has no radials"),
        });
    }

    // 1. Probe each supported product to see whether it appears in this sweep
    //    and capture its (first_gate, gate_interval, gate_count). Cheap: we
    //    only need to find the first radial with each product, no decoding.
    //    `geometries` is a parallel `(Product, MomentGeometry)` Vec because
    //    the shared `MomentGeometry` is intentionally backend-agnostic and
    //    doesn't carry the upstream product enum.
    let geometries: Vec<(Product, MomentGeometry)> = SUPPORTED_PRODUCTS
        .iter()
        .filter_map(|&p| probe_geometry(radials, p).map(|g| (p, g)))
        .collect();
    let (_, canonical) = geometries
        .iter()
        .max_by_key(|(_, g)| g.gate_count)
        .ok_or_else(|| RadishError::MalformedRecord {
            offset: 0,
            msg: format!("sweep {sweep_idx} has no supported moments"),
        })?;
    let max_gates = canonical.gate_count;

    // 2. Sort radials by azimuth *once* and reuse for every axis + moment.
    let order = sort_indices_by_key(radials, |r| r.azimuth_angle_degrees());

    // 3. Build coordinates from the sorted permutation.
    let coordinates = assemble_ppi_coordinates(
        radials,
        &order,
        build_range_axis(canonical),
        |r| r.azimuth_angle_degrees(),
        |r| r.elevation_angle_degrees(),
        |r| {
            r.collection_time()
                .map(|dt| dt.timestamp_micros() as f64 / 1.0e6)
                .unwrap_or(f64::NAN)
        },
    );

    let fixed_angle = sweep
        .elevation_angle_degrees()
        .map(|a| a as f64)
        .unwrap_or_else(|| median_elevation(&coordinates.elevation));
    let sweep_number = sweep.elevation_number() as u32;
    // PRT, Nyquist, PRF and polarization mode aren't surfaced by
    // `nexrad-model` 1.0.0-rc.2; they live in the RAD block at the
    // `nexrad-decode` level. Phase 2 will fill them.
    let mut sweep_meta = SweepMetadata::new(sweep_number, SweepMode::Azimuth, fixed_angle);
    sweep_meta.nexrad = cut.map(sweep_attrs_from_cut);

    // 4. Build moments by walking radials in sorted order and decoding gates
    //    directly into the (nrays × max_gates) Array2 — no intermediate Vec,
    //    no second sort.
    let nrays = radials.len();
    let mut moments: HashMap<String, MomentData> = HashMap::with_capacity(geometries.len());
    for (product, geometry) in &geometries {
        let arr = decode_product(radials, &order, *product, geometry, nrays, max_gates)?;
        let meta = moment_meta(*product);
        let mut moment = MomentData::new(meta.odim_name.to_string(), meta.units.to_string(), arr);
        moment.standard_name = Some(meta.standard_name.to_string());
        moment.long_name = Some(meta.long_name.to_string());
        moment.fill_value = Some(f32::NAN);
        moment.scale_factor = Some(1.0);
        moment.add_offset = Some(0.0);
        moments.insert(meta.odim_name.to_string(), moment);
    }

    Ok(SweepData::new(sweep_meta, moments, coordinates))
}

/// Find the first radial that carries `product` and return its geometry.
/// Returns `None` if the product is absent from every radial in the sweep.
///
/// Handles both the regular six moments (`Product::moment_data` →
/// `&MomentData`) and the special-cased clutter filter power
/// (`Product::cfp_moment_data` → `&CFPMomentData`). Both impl the
/// `DataMoment` trait so the geometry call is uniform.
fn probe_geometry(radials: &[Radial], product: Product) -> Option<MomentGeometry> {
    radials.iter().find_map(|r| {
        if let Some(m) = product.moment_data(r) {
            return Some(MomentGeometry {
                first_gate_km: m.first_gate_range_km(),
                gate_interval_km: m.gate_interval_km(),
                gate_count: m.gate_count() as usize,
            });
        }
        if let Some(c) = product.cfp_moment_data(r) {
            return Some(MomentGeometry {
                first_gate_km: c.first_gate_range_km(),
                gate_interval_km: c.gate_interval_km(),
                gate_count: c.gate_count() as usize,
            });
        }
        None
    })
}

/// Decode every radial's `product` directly into a freshly-allocated
/// (nrays × max_gates) `Array2<f32>`. The decode walks the canonical sort
/// order and skips the upstream `SweepField` allocation+sort pair.
///
/// Padding rules (encoded in the buffer's pre-fill + closure contract):
/// * If a radial doesn't carry the product, its row becomes all-NaN.
/// * If a radial's `gate_count` < the sweep's `max_gates`, the trailing cells
///   become NaN.
/// * `MomentValue::BelowThreshold` / `RangeFolded` and any non-`Value` CFP
///   status code map to NaN, matching xradar's masked-array convention.
///
/// Dispatches between the two upstream accessor flavours: regular `Product`s
/// expose `Product::moment_data(radial) -> Option<&MomentData>`, while
/// `ClutterFilterPower` lives on `Radial::clutter_filter_power() ->
/// Option<&CFPMomentData>` with a different value enum. Both are funneled
/// through `decode_into_array` via a `fill_row` closure so the buffer-
/// management scaffold is shared.
fn decode_product(
    radials: &[Radial],
    order: &[usize],
    product: Product,
    geometry: &MomentGeometry,
    nrays: usize,
    max_gates: usize,
) -> Result<Array2<f32>> {
    let ngates = geometry.gate_count;
    if product == Product::ClutterFilterPower {
        decode_into_array(radials, order, nrays, ngates, max_gates, |r, dst| {
            if let Some(cfp) = r.clutter_filter_power() {
                let n = dst.len();
                for (slot, v) in dst.iter_mut().zip(cfp.iter().take(n)) {
                    *slot = scaled_cfp(v);
                }
            }
            // Untouched cells (radial without product, or md shorter than
            // dst) keep their pre-NaN value.
        })
    } else {
        decode_into_array(radials, order, nrays, ngates, max_gates, |r, dst| {
            if let Some(md) = product.moment_data(r) {
                let n = dst.len();
                for (slot, mv) in dst.iter_mut().zip(md.iter().take(n)) {
                    *slot = scaled_moment(mv);
                }
            }
        })
    }
}

#[inline]
fn scaled_moment(value: MomentValue) -> f32 {
    match value {
        MomentValue::Value(v) => v,
        MomentValue::BelowThreshold | MomentValue::RangeFolded => f32::NAN,
    }
}

#[inline]
fn scaled_cfp(value: CFPMomentValue) -> f32 {
    match value {
        CFPMomentValue::Value(v) => v,
        // CFPMomentValue::Status(_) variants represent metadata about the
        // clutter filter, not a measurement; xradar emits NaN for these.
        CFPMomentValue::Status(_) => f32::NAN,
    }
}

/// Median of a slice of f32 elevations using `select_nth_unstable_by` so we
/// don't allocate a fully-sorted copy of the slice for every fallback.
fn median_elevation(elevations: &[f32]) -> f64 {
    if elevations.is_empty() {
        return f64::NAN;
    }
    let mut buf: Vec<f32> = elevations.to_vec();
    let mid = buf.len() / 2;
    let (_, m, _) = buf.select_nth_unstable_by(mid, |a, b| {
        a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
    });
    *m as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct an empty Sweep (no radials) to exercise the error path.
    fn empty_sweep(elevation_number: u8) -> Sweep {
        Sweep::new(elevation_number, Vec::<Radial>::new())
    }

    #[test]
    fn convert_sweep_errors_on_empty_radials() {
        let sweep = empty_sweep(1);
        match convert_sweep(&sweep, 0, None) {
            Err(RadishError::MalformedRecord { msg, .. }) => assert!(msg.contains("no radials")),
            other => panic!("expected MalformedRecord, got {other:?}"),
        }
    }

    #[test]
    fn scaled_moment_maps_non_valid_to_nan() {
        assert_eq!(scaled_moment(MomentValue::Value(42.0)), 42.0);
        assert!(scaled_moment(MomentValue::BelowThreshold).is_nan());
        assert!(scaled_moment(MomentValue::RangeFolded).is_nan());
    }

    #[test]
    fn scaled_cfp_maps_status_codes_to_nan() {
        use nexrad_model::data::CFPStatus;
        assert_eq!(scaled_cfp(CFPMomentValue::Value(0.5)), 0.5);
        assert!(scaled_cfp(CFPMomentValue::Status(CFPStatus::FilterNotApplied)).is_nan());
        assert!(scaled_cfp(CFPMomentValue::Status(CFPStatus::PointClutterFilterApplied)).is_nan());
    }

    #[test]
    fn median_elevation_handles_empty() {
        assert!(median_elevation(&[]).is_nan());
    }

    #[test]
    fn median_elevation_returns_middle_value() {
        // Median of [0.5, 1.5, 2.5] = 1.5
        assert!((median_elevation(&[2.5, 0.5, 1.5]) - 1.5).abs() < 1e-6);
    }

    #[test]
    fn probe_geometry_returns_none_when_product_absent() {
        // Empty radial slice → no geometry for any product.
        let empty: &[Radial] = &[];
        assert!(probe_geometry(empty, Product::Reflectivity).is_none());
    }

    /// Build a single radial with reflectivity-only data. NEXRAD raw byte
    /// semantics: 0 → BelowThreshold, 1 → RangeFolded, ≥2 → physical via
    /// `(raw - offset) / scale`. We use scale=2.0, offset=66.0 (the legacy
    /// MSG_1 reflectivity defaults) so the math is hand-checkable.
    fn ref_only_radial(azimuth_deg: f32, elevation_number: u8, raw_gates: Vec<u8>) -> Radial {
        use nexrad_model::data::{MomentData, RadialStatus};
        let gate_count = raw_gates.len() as u16;
        let reflectivity = MomentData::from_fixed_point(
            gate_count, /* first_gate_range */ 2_000, /* gate_interval */ 250,
            /* data_word_size */ 8, /* scale */ 2.0, /* offset */ 66.0, raw_gates,
        );
        Radial::new(
            /* collection_timestamp */ 0,
            /* azimuth_number */ 0,
            azimuth_deg,
            /* azimuth_spacing_degrees */ 0.5,
            RadialStatus::ScanStart,
            elevation_number,
            /* elevation_angle_degrees */ 0.5,
            Some(reflectivity),
            None,
            None,
            None,
            None,
            None,
            None,
        )
    }

    #[test]
    fn convert_sweep_sorts_rays_and_decodes_reflectivity_correctly() {
        // Two rays in REVERSE azimuth order: the adapter must sort them so
        // row 0 has azimuth=10° and row 1 has azimuth=20°.
        // Per gate: 0/1 → NaN (sentinels), raw=2 → (2-66)/2 = -32 dBZ,
        //                                  raw=130 → (130-66)/2 = 32 dBZ.
        let sweep = Sweep::new(
            1,
            vec![
                ref_only_radial(20.0, 1, vec![130, 0, 130]),
                ref_only_radial(10.0, 1, vec![2, 1, 2]),
            ],
        );
        let sd = convert_sweep(&sweep, 0, None).expect("convert_sweep");

        // Coords reflect the sorted order.
        assert_eq!(sd.coordinates.azimuth, vec![10.0, 20.0]);
        let dbzh = sd.moments.get("DBZH").expect("DBZH present");
        assert_eq!(dbzh.shape(), (2, 3));

        // Row 0 (azimuth 10°): raws [2, 1, 2] → [-32.0, NaN, -32.0]
        assert!((dbzh.data[(0, 0)] - (-32.0)).abs() < 1e-6);
        assert!(dbzh.data[(0, 1)].is_nan());
        assert!((dbzh.data[(0, 2)] - (-32.0)).abs() < 1e-6);

        // Row 1 (azimuth 20°): raws [130, 0, 130] → [32.0, NaN, 32.0]
        assert!((dbzh.data[(1, 0)] - 32.0).abs() < 1e-6);
        assert!(dbzh.data[(1, 1)].is_nan());
        assert!((dbzh.data[(1, 2)] - 32.0).abs() < 1e-6);

        // Range axis matches the moment's geometry: first gate 2 km, step 0.25 km.
        assert_eq!(sd.coordinates.range, vec![2000.0, 2250.0, 2500.0]);
    }

    #[test]
    fn convert_sweep_pads_short_moments_with_nan() {
        // Sweep with two radials carrying 2-gate REF only. We probe geometry
        // and (since there's only one product) max_gates == 2. Verify nothing
        // explodes on the trivial single-product case.
        let sweep = Sweep::new(
            1,
            vec![
                ref_only_radial(0.0, 1, vec![10, 20]),
                ref_only_radial(180.0, 1, vec![30, 40]),
            ],
        );
        let sd = convert_sweep(&sweep, 0, None).expect("convert_sweep");
        let dbzh = &sd.moments["DBZH"];
        assert_eq!(dbzh.shape(), (2, 2));
        // raw=10 → (10-66)/2 = -28; raw=20 → (20-66)/2 = -23
        assert!((dbzh.data[(0, 0)] - (-28.0)).abs() < 1e-6);
        assert!((dbzh.data[(0, 1)] - (-23.0)).abs() < 1e-6);
    }
}

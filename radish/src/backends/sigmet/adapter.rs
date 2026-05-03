//! Convert a [`super::decode::DecodedVolume`] into radish's `VolumeData`.
//!
//! The shape mirrors the NEXRAD adapter: parallel sweep conversion via
//! `rayon`, sort rays by azimuth (PPI) or elevation (RHI) once, reuse the
//! permutation for every coord axis and every moment buffer. All the
//! buffer-management logic lives in `backends::common::*` so this file
//! is mostly format-specific glue.

use std::collections::HashMap;
use std::path::Path;

use ndarray::Array2;
use radish_types::SweepMode;
use rayon::prelude::*;

use crate::backends::common::{
    assemble_ppi_coordinates, decode_into_array, sort_indices_by_key,
};
use crate::{
    MomentData, RadishError, Result, SweepData, SweepMetadata, VolumeData, VolumeMetadata,
};

use super::decode::{DecodedRay, DecodedSweep, DecodedVolume};
use super::mapping::{cf_metadata_for, moment_for_id, SUPPORTED_MOMENTS};
use super::structs::ScanMode;

pub(super) fn convert_volume(decoded: DecodedVolume, source: &Path) -> Result<VolumeData> {
    let metadata = build_volume_metadata(&decoded, source)?;
    // Build a stable index list of (data_type_id, ODIM name) pairs in the
    // order SUPPORTED_MOMENTS dictates so DataTree variable order is
    // deterministic across runs. We only emit moments that any ray
    // actually carries — a missing-from-mask moment never appears.
    let active_ids: Vec<u8> = SUPPORTED_MOMENTS
        .iter()
        .map(|m| m.data_type_id)
        .filter(|id| {
            decoded
                .sweeps
                .iter()
                .flat_map(|s| s.rays.iter())
                .any(|r| r.moments.contains_key(id))
        })
        .collect();

    let sweeps: Vec<SweepData> = decoded
        .sweeps
        .par_iter()
        .enumerate()
        .map(|(idx, sweep)| convert_sweep(sweep, idx, decoded.scan_mode, &decoded.range_axis_m, &active_ids))
        .collect::<Result<_>>()?;

    Ok(VolumeData::new(metadata, sweeps))
}

pub(super) fn build_volume_metadata(
    decoded: &DecodedVolume,
    source: &Path,
) -> Result<VolumeMetadata> {
    let instrument_name = if decoded.site_name.is_empty() {
        source
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("UNKN")
            .to_string()
    } else {
        decoded.site_name.clone()
    };

    let mut metadata = VolumeMetadata::new(
        instrument_name,
        decoded.latitude_deg,
        decoded.longitude_deg,
        decoded.altitude_m,
        decoded.volume_start_time,
        decoded
            .sweeps
            .last()
            .map(|s| s.start_time)
            .unwrap_or(decoded.volume_start_time),
    );
    metadata.institution = "Vaisala / SIGMET".to_string();
    metadata.platform_type = Some(radish_types::PlatformType::Fixed);
    metadata.generate_sweep_names(decoded.sweeps.len());
    metadata.sweep_fixed_angles = decoded
        .sweeps
        .iter()
        .map(|s| s.fixed_angle_deg as f64)
        .collect();
    metadata
        .attributes
        .insert("scan_name".to_string(), decoded.task_name.clone());
    metadata
        .attributes
        .insert("scan_mode".to_string(), decoded.scan_mode.label().to_string());
    metadata.attributes.insert(
        "iris_version".to_string(),
        decoded.iris_version.clone(),
    );
    Ok(metadata)
}

pub(super) fn convert_sweep_at(
    decoded: &DecodedVolume,
    idx: usize,
) -> Option<Result<SweepData>> {
    let sweep = decoded.sweeps.get(idx)?;
    let active_ids: Vec<u8> = SUPPORTED_MOMENTS
        .iter()
        .map(|m| m.data_type_id)
        .filter(|id| sweep.rays.iter().any(|r| r.moments.contains_key(id)))
        .collect();
    Some(convert_sweep(
        sweep,
        idx,
        decoded.scan_mode,
        &decoded.range_axis_m,
        &active_ids,
    ))
}

fn convert_sweep(
    sweep: &DecodedSweep,
    sweep_idx: usize,
    scan_mode: ScanMode,
    range_axis_m: &[f32],
    active_ids: &[u8],
) -> Result<SweepData> {
    let rays = &sweep.rays;
    if rays.is_empty() {
        return Err(RadishError::MalformedRecord {
            offset: 0,
            msg: format!("sweep {sweep_idx} has no rays"),
        });
    }

    // Sort rays by primary angle: azimuth for PPI (and Other/unknown,
    // safe fallback), elevation for RHI.
    let order = match scan_mode {
        ScanMode::Rhi => sort_indices_by_key(rays, |r| r.elevation_deg),
        _ => sort_indices_by_key(rays, |r| r.azimuth_deg),
    };

    let coordinates = assemble_ppi_coordinates(
        rays,
        &order,
        range_axis_m.to_vec(),
        |r: &DecodedRay| r.azimuth_deg,
        |r: &DecodedRay| r.elevation_deg,
        |r: &DecodedRay| r.time_offset_s as f64,
    );

    let nrays = rays.len();
    let max_gates = range_axis_m.len();
    let mut moments: HashMap<String, MomentData> = HashMap::with_capacity(active_ids.len());
    let mut emitted: HashMap<String, ()> = HashMap::new();
    for &data_type_id in active_ids {
        let m = match moment_for_id(data_type_id) {
            Some(m) => m,
            None => continue,
        };
        // Skip duplicate ODIM emissions: 8-bit and 16-bit variants both
        // map to e.g. DBZH; we keep the first one we encounter (the
        // SUPPORTED_MOMENTS ordering puts 8-bit before 16-bit).
        if emitted.contains_key(m.odim_name) {
            continue;
        }

        let arr = build_moment_array(rays, &order, data_type_id, nrays, max_gates)?;
        let meta = cf_metadata_for(m).ok_or_else(|| RadishError::Conversion(format!(
            "no CF metadata for ODIM moment {:?}",
            m.odim_name
        )))?;

        let mut moment = MomentData::new(
            meta.odim_name.to_string(),
            meta.units.to_string(),
            arr,
        );
        moment.standard_name = Some(meta.standard_name.to_string());
        moment.long_name = Some(meta.long_name.to_string());
        moment.fill_value = Some(f32::NAN);
        moment.scale_factor = Some(1.0);
        moment.add_offset = Some(0.0);
        moments.insert(meta.odim_name.to_string(), moment);
        emitted.insert(meta.odim_name.to_string(), ());
    }

    let mut meta = SweepMetadata::new(
        sweep.sweep_number,
        match scan_mode {
            ScanMode::Rhi => SweepMode::Elevation,
            _ => SweepMode::Azimuth,
        },
        sweep.fixed_angle_deg as f64,
    );
    meta.follow_mode = None;
    meta.prt_mode = None;
    Ok(SweepData::new(meta, moments, coordinates))
}

/// Build the `Array2<f32>` for one moment by walking rays in `order` and
/// copying their pre-decoded gates into the per-row slice.
fn build_moment_array(
    rays: &[DecodedRay],
    order: &[usize],
    data_type_id: u8,
    nrays: usize,
    max_gates: usize,
) -> Result<Array2<f32>> {
    decode_into_array(rays, order, nrays, max_gates, max_gates, |ray, dst| {
        if let Some(gates) = ray.moments.get(&data_type_id) {
            let n = dst.len().min(gates.len());
            dst[..n].copy_from_slice(&gates[..n]);
        }
    })
}

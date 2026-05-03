//! Format-agnostic helpers shared by every `RadarBackend` implementation.
//!
//! This module hosts the small utilities that every radar-format adapter
//! re-discovers when it normalises a third-party decoder's output into
//! radish's [`crate::VolumeData`] / [`crate::SweepData`] / [`crate::Coordinates`]
//! model:
//!
//! * **Buffer assembly** ([`buffer::decode_into_array`]) — pre-fill a
//!   `(nrays × max_gates)` `Array2<f32>` with NaN, walk a per-ray order, hand
//!   each row to a closure that fills available cells. Untouched cells stay
//!   NaN. Generic over the per-ray item type so any decoder can plug in.
//! * **Sorting** ([`sort::sort_indices_by_key`]) — stable index permutation
//!   for "sort rays by azimuth (PPI)" / "sort rays by elevation (RHI)" /
//!   etc., without moving the ray data itself.
//! * **Coordinate assembly** ([`coords::assemble_ppi_coordinates`]) — given a
//!   sort permutation + per-ray angle/time getters, build the
//!   `azimuth`/`elevation`/`time`/`range` vectors that go into a
//!   [`crate::Coordinates`].
//! * **Geometry** ([`geometry::MomentGeometry`]) — `(first_gate_km,
//!   gate_interval_km, gate_count)` triple captured from a moment's first
//!   ray; used to size buffers and pick the canonical range axis when a
//!   sweep mixes resolutions.
//!
//! **Contract:** code in this module must be format-agnostic. No NEXRAD or
//! IRIS / sigmet types in the signatures, no Rust crates that only one
//! backend uses. If a helper drifts toward format-specific behaviour, move
//! it back into the backend's own directory.

pub(crate) mod buffer;
pub(crate) mod coords;
pub(crate) mod geometry;
pub(crate) mod metadata;
pub(crate) mod sniff;
pub(crate) mod sort;

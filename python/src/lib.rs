//! Python bindings for radish.
//!
//! ## Ownership model
//!
// PyO3 0.22's `#[pymethods]` macro expands `PyResult<T>` returns with an
// implicit `From::from`-based error conversion that clippy 1.92+ flags as
// useless. The conversion is part of PyO3's type-erasure plumbing, not our
// own code, so we silence it crate-wide rather than scattering allows.
#![allow(clippy::useless_conversion)]
//!
//! Moment arrays and per-sweep state are *moved* out of the wrapper on first
//! access rather than cloned. A typical xarray-driven open touches every
//! sweep and every moment exactly once, and each moment array is on the
//! order of megabytes — cloning them all (the previous behaviour) was the
//! single largest cost in the wall-clock benchmark.
//!
//! Practical impact:
//!
//! * `VolumeData.get_sweep(i)` consumes sweep `i`. Calling it twice on the
//!   same index raises `RuntimeError`.
//! * `SweepData.get_moment(name)` consumes the moment. A second call returns
//!   `None`.
//! * `MomentData.data()` consumes the underlying numpy buffer (handed off
//!   via `PyArray2::from_owned_array_bound`). A second call raises.
//! * Read-only metadata (`name`, `units`, `shape`, `num_rays`, `num_sweeps`,
//!   coordinates, etc.) is cached side-by-side and remains accessible after
//!   consumption.

use std::collections::HashMap;
use std::path::Path;

use ndarray::Array2;
use numpy::{PyArray1, PyArray2};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use radish::{
    backends::{CfRadial1Backend, NexradBackend, RadarBackend},
    Coordinates, MomentData as RustMomentData, SweepData as RustSweepData,
    SweepMetadata, VolumeData as RustVolumeData, VolumeMetadata as RustVolumeMetadata,
};

#[pyclass(name = "VolumeMetadata")]
#[derive(Clone)]
pub struct PyVolumeMetadata {
    inner: RustVolumeMetadata,
}

#[pymethods]
impl PyVolumeMetadata {
    #[getter]
    fn instrument_name(&self) -> &str {
        &self.inner.instrument_name
    }

    #[getter]
    fn latitude(&self) -> f64 {
        self.inner.latitude
    }

    #[getter]
    fn longitude(&self) -> f64 {
        self.inner.longitude
    }

    #[getter]
    fn altitude(&self) -> f64 {
        self.inner.altitude
    }

    #[getter]
    fn sweep_fixed_angles(&self) -> Vec<f64> {
        self.inner.sweep_fixed_angles.clone()
    }

    #[getter]
    fn num_sweeps(&self) -> usize {
        self.inner.sweep_group_names.len()
    }

    #[getter]
    fn institution(&self) -> &str {
        &self.inner.institution
    }

    /// Free-form attribute bag populated by the backend (e.g. NEXRAD writes
    /// `scan_name`, `vcp`, `vcp_description` here). Cloned into a fresh
    /// `dict` so Python callers can mutate without affecting the volume.
    #[getter]
    fn attributes(&self) -> std::collections::HashMap<String, String> {
        self.inner.attributes.clone()
    }

    #[getter]
    fn volume_number(&self) -> u32 {
        self.inner.volume_number
    }

    /// ISO 8601 timestamp with `Z` suffix (e.g. `"2026-03-10T23:14:12Z"`),
    /// matching xradar's `time_coverage_start` root variable format.
    #[getter]
    fn time_coverage_start(&self) -> String {
        self.inner
            .time_coverage_start
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    }

    #[getter]
    fn time_coverage_end(&self) -> String {
        self.inner
            .time_coverage_end
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    }

    /// FM301 platform-type string (`"fixed"`, `"vehicle"`, ...). Returns
    /// `"fixed"` when not specified — that's the WSR-88D / ground-radar
    /// default and matches xradar.
    #[getter]
    fn platform_type(&self) -> String {
        self.inner
            .platform_type
            .map(|p| p.to_string())
            .unwrap_or_else(|| "fixed".to_string())
    }

    fn __repr__(&self) -> String {
        format!(
            "VolumeMetadata(instrument='{}', lat={:.4}, lon={:.4}, alt={:.1}, sweeps={})",
            self.inner.instrument_name,
            self.inner.latitude,
            self.inner.longitude,
            self.inner.altitude,
            self.num_sweeps()
        )
    }
}

/// Python wrapper for `MomentData`.
///
/// The (rays × gates) `Array2<f32>` is moved out of `data` on the first
/// `data()` call via `PyArray2::from_owned_array_bound`, which transfers
/// ownership to numpy with no `memcpy`. A second call raises.
#[pyclass(name = "MomentData")]
pub struct PyMomentData {
    name: String,
    units: String,
    shape: (usize, usize),
    data: Option<Array2<f32>>,
}

impl PyMomentData {
    fn from_inner(m: RustMomentData) -> Self {
        let shape = m.shape();
        Self {
            name: m.name,
            units: m.units,
            shape,
            data: Some(m.data),
        }
    }
}

#[pymethods]
impl PyMomentData {
    #[getter]
    fn name(&self) -> &str {
        &self.name
    }

    #[getter]
    fn units(&self) -> &str {
        &self.units
    }

    #[getter]
    fn shape(&self) -> (usize, usize) {
        self.shape
    }

    /// Hand the moment array to numpy without copying. Single-use: a second
    /// call raises `RuntimeError`.
    fn data<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyArray2<f32>>> {
        let arr = self.data.take().ok_or_else(|| {
            PyRuntimeError::new_err("MomentData.data() has already been consumed")
        })?;
        Ok(PyArray2::from_owned_array_bound(py, arr))
    }

    fn __repr__(&self) -> String {
        let (nrays, ngates) = self.shape;
        let state = if self.data.is_some() { "owned" } else { "consumed" };
        format!(
            "MomentData(name='{}', units='{}', shape=({nrays}, {ngates}), {state})",
            self.name, self.units,
        )
    }
}

/// Python wrapper for `SweepData`.
///
/// Moment slots are moved out on first `get_moment(name)` call. Coordinates
/// are exposed as numpy views via `PyArray1::from_slice_bound` (one C-level
/// `memcpy`); cheap enough that we don't bother moving them.
#[pyclass(name = "SweepData")]
pub struct PySweepData {
    metadata: SweepMetadata,
    coordinates: Coordinates,
    moment_order: Vec<String>,
    moments: HashMap<String, Option<RustMomentData>>,
}

impl PySweepData {
    fn from_inner(s: RustSweepData) -> Self {
        // Stable iteration order, alphabetical so DataTree variable order
        // is deterministic regardless of the moments' HashMap state.
        let mut moment_order: Vec<String> = s.moments.keys().cloned().collect();
        moment_order.sort();
        let moments = s
            .moments
            .into_iter()
            .map(|(k, v)| (k, Some(v)))
            .collect();
        Self {
            metadata: s.metadata,
            coordinates: s.coordinates,
            moment_order,
            moments,
        }
    }
}

#[pymethods]
impl PySweepData {
    #[getter]
    fn sweep_number(&self) -> u32 {
        self.metadata.sweep_number
    }

    #[getter]
    fn fixed_angle(&self) -> f64 {
        self.metadata.fixed_angle
    }

    /// CfRadial2 / FM301 `sweep_mode` value (e.g. `"azimuth_surveillance"`).
    /// Always set; defaults to `"azimuth_surveillance"` for NEXRAD PPI volumes.
    #[getter]
    fn sweep_mode(&self) -> String {
        self.metadata.sweep_mode.to_string()
    }

    /// CfRadial2 / FM301 `prt_mode` value (`"fixed"`, `"staggered"`, `"dual"`).
    /// Falls back to `"not_set"` when the source format doesn't surface it
    /// (matches xradar's convention so engine-swap users see the same shape).
    #[getter]
    fn prt_mode(&self) -> String {
        self.metadata
            .prt_mode
            .map(|m| m.to_string())
            .unwrap_or_else(|| "not_set".to_string())
    }

    /// CfRadial2 / FM301 `follow_mode` value (`"none"`, `"sun"`, ...).
    /// Falls back to `"not_set"` when the source format doesn't surface it.
    #[getter]
    fn follow_mode(&self) -> String {
        self.metadata
            .follow_mode
            .map(|m| m.to_string())
            .unwrap_or_else(|| "not_set".to_string())
    }

    #[getter]
    fn num_rays(&self) -> usize {
        self.coordinates.num_rays()
    }

    #[getter]
    fn num_gates(&self) -> usize {
        self.coordinates.num_gates()
    }

    fn moment_names(&self) -> Vec<String> {
        self.moment_order.clone()
    }

    /// Take a moment by name. Returns `None` if the moment never existed *or*
    /// has already been taken.
    fn get_moment(&mut self, name: &str) -> Option<PyMomentData> {
        let slot = self.moments.get_mut(name)?;
        slot.take().map(PyMomentData::from_inner)
    }

    #[getter]
    fn azimuth<'py>(&self, py: Python<'py>) -> Bound<'py, PyArray1<f32>> {
        PyArray1::from_slice_bound(py, &self.coordinates.azimuth)
    }

    #[getter]
    fn elevation<'py>(&self, py: Python<'py>) -> Bound<'py, PyArray1<f32>> {
        PyArray1::from_slice_bound(py, &self.coordinates.elevation)
    }

    #[getter]
    fn range<'py>(&self, py: Python<'py>) -> Bound<'py, PyArray1<f32>> {
        PyArray1::from_slice_bound(py, &self.coordinates.range)
    }

    #[getter]
    fn time<'py>(&self, py: Python<'py>) -> Bound<'py, PyArray1<f64>> {
        PyArray1::from_slice_bound(py, &self.coordinates.time)
    }

    fn __repr__(&self) -> String {
        format!(
            "SweepData(sweep={}, angle={:.2}°, rays={}, gates={}, moments={})",
            self.sweep_number(),
            self.fixed_angle(),
            self.num_rays(),
            self.num_gates(),
            self.moment_order.len()
        )
    }
}

/// Python wrapper for `VolumeData`.
///
/// Sweeps are moved out on first `get_sweep(i)` call; the volume metadata is
/// kept around so it stays cheaply accessible after every sweep is consumed
/// (`num_sweeps`, `metadata`, `__repr__`).
#[pyclass(name = "VolumeData")]
pub struct PyVolumeData {
    metadata: RustVolumeMetadata,
    sweeps: Vec<Option<RustSweepData>>,
}

impl PyVolumeData {
    fn from_inner(v: RustVolumeData) -> Self {
        Self {
            metadata: v.metadata,
            sweeps: v.sweeps.into_iter().map(Some).collect(),
        }
    }
}

#[pymethods]
impl PyVolumeData {
    #[getter]
    fn metadata(&self) -> PyVolumeMetadata {
        PyVolumeMetadata {
            inner: self.metadata.clone(),
        }
    }

    #[getter]
    fn num_sweeps(&self) -> usize {
        self.sweeps.len()
    }

    /// Take a sweep by index. Each sweep can only be taken once; calling
    /// twice raises `RuntimeError` so the caller learns about the bug
    /// instead of silently getting a re-decoded copy.
    fn get_sweep(&mut self, index: usize) -> PyResult<PySweepData> {
        let slot = self.sweeps.get_mut(index).ok_or_else(|| {
            PyRuntimeError::new_err(format!("Invalid sweep index: {index}"))
        })?;
        let sweep = slot.take().ok_or_else(|| {
            PyRuntimeError::new_err(format!("Sweep {index} has already been consumed"))
        })?;
        Ok(PySweepData::from_inner(sweep))
    }

    fn __repr__(&self) -> String {
        let consumed = self.sweeps.iter().filter(|s| s.is_none()).count();
        format!(
            "VolumeData(instrument='{}', sweeps={}, consumed={consumed})",
            self.metadata.instrument_name,
            self.sweeps.len(),
        )
    }
}

/// Read a CfRadial1 file
#[pyfunction]
fn read_cfradial1(path: &str) -> PyResult<PyVolumeData> {
    CfRadial1Backend::new()
        .read_volume(Path::new(path))
        .map(PyVolumeData::from_inner)
        .map_err(|e| PyRuntimeError::new_err(format!("Failed to read file: {e}")))
}

/// Scan a CfRadial1 file for metadata only
#[pyfunction]
fn scan_cfradial1(path: &str) -> PyResult<PyVolumeMetadata> {
    CfRadial1Backend::new()
        .scan_file(Path::new(path))
        .map(|inner| PyVolumeMetadata { inner })
        .map_err(|e| PyRuntimeError::new_err(format!("Failed to scan file: {e}")))
}

/// Read a NEXRAD Level 2 (Archive II / AR2V) file
#[pyfunction]
fn read_nexrad(path: &str) -> PyResult<PyVolumeData> {
    NexradBackend::new()
        .read_volume(Path::new(path))
        .map(PyVolumeData::from_inner)
        .map_err(|e| PyRuntimeError::new_err(format!("Failed to read NEXRAD file: {e}")))
}

/// Scan a NEXRAD Level 2 file for metadata only
#[pyfunction]
fn scan_nexrad(path: &str) -> PyResult<PyVolumeMetadata> {
    NexradBackend::new()
        .scan_file(Path::new(path))
        .map(|inner| PyVolumeMetadata { inner })
        .map_err(|e| PyRuntimeError::new_err(format!("Failed to scan NEXRAD file: {e}")))
}

#[pymodule]
fn _radish(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyVolumeData>()?;
    m.add_class::<PyVolumeMetadata>()?;
    m.add_class::<PySweepData>()?;
    m.add_class::<PyMomentData>()?;
    m.add_function(wrap_pyfunction!(read_cfradial1, m)?)?;
    m.add_function(wrap_pyfunction!(scan_cfradial1, m)?)?;
    m.add_function(wrap_pyfunction!(read_nexrad, m)?)?;
    m.add_function(wrap_pyfunction!(scan_nexrad, m)?)?;
    Ok(())
}

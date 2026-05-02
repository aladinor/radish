//! NEXRAD Level 2 (Archive II / AR2V) backend.
//!
//! Adapts the upstream `nexrad-model` decoded `Scan` representation into
//! radish's `VolumeData` model. The upstream crate already returns
//! physical-units `f32` per gate, so the adapter does no scaling: it only
//! reshapes flat per-moment buffers into `Array2<f32>`, assembles the per-sweep
//! `Coordinates`, and surfaces site/VCP metadata.
//!
//! See `plans/0001-nexrad-level2-backend.md` for the design and the Phase 0
//! benchmark (≈25× faster decode than xradar on the user's KLOT fixture).

use std::path::Path;

use crate::{backends::RadarBackend, RadishError, Result, SweepData, VolumeData, VolumeMetadata};

mod adapter;
mod mapping;
pub(crate) mod sniff;

/// Backend for NEXRAD Level 2 Archive II files (AR2V).
pub struct NexradBackend;

impl NexradBackend {
    /// Create a new backend instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for NexradBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl RadarBackend for NexradBackend {
    fn name(&self) -> &str {
        "nexrad_level2"
    }

    fn description(&self) -> &str {
        "NOAA NEXRAD Level II (Archive II / AR2V)"
    }

    fn supported_extensions(&self) -> &[&str] {
        sniff::EXTENSIONS
    }

    /// NEXRAD files are commonly distributed without an extension, so the
    /// default extension match is supplemented with `AR2V` magic-byte and
    /// canonical filename-pattern checks.
    fn can_read(&self, path: &Path) -> bool {
        sniff::looks_like_nexrad(path)
    }

    fn scan_file(&self, path: &Path) -> Result<VolumeMetadata> {
        // Phase 1 does a full decode then drops sweeps. A truly cheap path
        // would read only the 24-byte volume header (ICAO + datetime) plus
        // MSG_5 (VCP) — but `radish::VolumeMetadata` also wants lat/lon and
        // per-sweep angles, which need at least one decompressed LDM chunk.
        // Re-evaluate when a user needs sub-100 ms scan.
        adapter::build_volume_metadata(&decode_scan(path)?, path)
    }

    fn read_sweep(&self, path: &Path, sweep_idx: usize) -> Result<SweepData> {
        let scan = decode_scan(path)?;
        let sweep = scan
            .sweeps()
            .get(sweep_idx)
            .ok_or(RadishError::InvalidSweepIndex(sweep_idx))?;
        adapter::convert_sweep(sweep, sweep_idx)
    }

    fn read_volume(&self, path: &Path) -> Result<VolumeData> {
        adapter::convert_scan(decode_scan(path)?, path)
    }
}

/// Single decode entry point that avoids the extra `Vec<u8>` clone the upstream
/// `nexrad::load_file` does internally (it reads into a `Vec`, then
/// `File::new(data.to_vec())` clones it a second time before decompressing).
/// Going through `nexrad::data::volume::File::new(data)` directly hands the
/// owned buffer to the decoder with no second copy.
fn decode_scan(path: &Path) -> Result<nexrad_model::data::Scan> {
    let data = std::fs::read(path)?;
    let file = nexrad::data::volume::File::new(data)
        .decompress()
        .map_err(|e| RadishError::Decode(e.to_string()))?;
    file.scan().map_err(|e| RadishError::Decode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_description_are_static() {
        let b = NexradBackend::new();
        assert_eq!(b.name(), "nexrad_level2");
        assert!(b.description().contains("NEXRAD"));
    }

    #[test]
    fn extensions_include_ar2v() {
        let b = NexradBackend::new();
        assert!(b.supported_extensions().contains(&"ar2v"));
    }

    #[test]
    fn can_read_extensionless_canonical_filename() {
        let b = NexradBackend::new();
        // Filename-pattern branch — no I/O.
        assert!(b.can_read(Path::new("/data/KLOT20260310_231412_V06")));
    }

    #[test]
    fn cannot_read_random_extension() {
        let b = NexradBackend::new();
        assert!(!b.can_read(Path::new("/data/foo.txt")));
    }
}

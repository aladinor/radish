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

use nexrad::decode::messages::rda_status_data::Message as RdaStatusMessage;
use nexrad::decode::messages::MessageContents;

use crate::{backends::RadarBackend, RadishError, Result, SweepData, VolumeData, VolumeMetadata};

mod adapter;
mod attrs;
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
        let (scan, msg2) = decode_scan_and_msg2(path)?;
        adapter::build_volume_metadata(&scan, msg2.as_ref(), path)
    }

    fn read_sweep(&self, path: &Path, sweep_idx: usize) -> Result<SweepData> {
        let (scan, _) = decode_scan_and_msg2(path)?;
        let sweep = scan
            .sweeps()
            .get(sweep_idx)
            .ok_or(RadishError::InvalidSweepIndex(sweep_idx))?;
        let cut = scan.coverage_pattern().elevation_cuts().get(sweep_idx);
        adapter::convert_sweep(sweep, sweep_idx, cut)
    }

    fn read_volume(&self, path: &Path) -> Result<VolumeData> {
        let (scan, msg2) = decode_scan_and_msg2(path)?;
        adapter::convert_scan(scan, msg2, path)
    }
}

/// Decode the file once and return both the high-level `Scan` (moments + VCP)
/// and the optional first MSG_2 (RDA Status) message.
///
/// The upstream `File::scan()` silently drops MSG_2 — see
/// `nexrad-data/src/volume/file.rs` line 137 (`_ => {}`). To populate the
/// xradar-parity root attrs (`avset_enabled`, `rda_build_number`, etc.) we
/// re-walk records sequentially and early-return at the first MSG_2. The cost
/// is one extra LDM chunk decompression (~120 KB max) and one `messages()`
/// call — bounded under 5 ms on typical fixtures, well below the noise floor
/// of the wall-clock benchmark vs. xradar.
fn decode_scan_and_msg2(
    path: &Path,
) -> Result<(nexrad_model::data::Scan, Option<RdaStatusMessage<'static>>)> {
    let data = std::fs::read(path)?;
    let file = nexrad::data::volume::File::new(data)
        .decompress()
        .map_err(|e| RadishError::Decode(e.to_string()))?;
    let scan = file
        .scan()
        .map_err(|e| RadishError::Decode(e.to_string()))?;
    let msg2 = first_msg2(&file).unwrap_or(None);
    Ok((scan, msg2))
}

/// Walk records sequentially, decompressing only as far as needed, and return
/// the first MSG_2 message. Returns `Ok(None)` if the file has no MSG_2 and
/// propagates errors otherwise.
fn first_msg2(file: &nexrad::data::volume::File) -> Result<Option<RdaStatusMessage<'static>>> {
    use nexrad::data::volume::Record;
    let records = file
        .records()
        .map_err(|e| RadishError::Decode(e.to_string()))?;
    for record in records {
        let record = if record.compressed() {
            record
                .decompress()
                .map_err(|e| RadishError::Decode(e.to_string()))?
        } else {
            Record::new(record.data().to_vec())
        };
        let messages = record
            .messages()
            .map_err(|e| RadishError::Decode(e.to_string()))?;
        for message in messages {
            if let MessageContents::RDAStatusData(m) = message.into_contents() {
                return Ok(Some(m.into_owned()));
            }
        }
    }
    Ok(None)
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

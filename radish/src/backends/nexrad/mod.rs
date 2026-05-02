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

    /// Magic-byte sniff for in-memory buffers — recognises raw `AR2V` and
    /// gzip-wrapped (older `*.gz` archive volume) buffers.
    fn can_read_bytes(&self, head: &[u8]) -> bool {
        sniff::looks_like_ar2v_bytes(head)
    }

    /// Decode a NEXRAD Level 2 volume from a single in-memory byte buffer.
    ///
    /// Convenience entry point for the common case of "fetch the whole file
    /// from S3 / HTTP / fsspec, then decode" — equivalent to xradar's
    /// `xradar.io.open_nexradlevel2_datatree(data)` when given one bytes
    /// object. If the buffer is gzip-compressed (older `*.gz` archive
    /// volumes), the upstream `File::decompress()` handles it transparently.
    fn read_bytes_volume(&self, data: Vec<u8>) -> Result<VolumeData> {
        let (scan, msg2) = decode_bytes(data)?;
        adapter::convert_scan(scan, msg2, Path::new("<bytes>"))
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

impl NexradBackend {
    /// Decode a NEXRAD Level 2 volume from a sequence of chunk byte buffers.
    ///
    /// Each volume from NOAA's live `unidata-nexrad-level2-chunks` S3 bucket
    /// is split into many small files that arrive seconds after each scan
    /// (vs. minutes for the assembled `noaa-nexrad-level2` archive):
    ///
    /// * `S` — volume header + first metadata records (start)
    /// * `I00..In` — LDM-compressed sweep data, numbered in scan order
    /// * `E` — end-of-volume marker
    ///
    /// The chunks must arrive **in scan order** — the same contract as
    /// `xradar.io.open_nexradlevel2_datatree(list_of_bytes)`. Concatenating
    /// `[S, I00, I01, ..., E]` reconstitutes a complete Archive II file
    /// byte-for-byte (volume header from `S`, then the LDM record stream
    /// across all chunks), so we hand it to the same decoder. A truncated
    /// volume (no `E`, or only the first few `I` chunks) decodes whatever
    /// rays survive — incomplete trailing sweeps come through with fewer
    /// rays than the VCP's expected count.
    pub fn read_chunks_volume(&self, chunks: Vec<Vec<u8>>) -> Result<VolumeData> {
        let combined = concat_chunks(chunks);
        let (scan, msg2) = decode_bytes(combined)?;
        adapter::convert_scan(scan, msg2, Path::new("<chunks>"))
    }
}

/// Concatenate chunk buffers into a single owned `Vec<u8>` with one
/// allocation. The implementation is straightforward but worth keeping in
/// one place: chunk count can be in the dozens for a typical KXXX volume,
/// so pre-sizing the destination matters.
fn concat_chunks(chunks: Vec<Vec<u8>>) -> Vec<u8> {
    let total: usize = chunks.iter().map(|c| c.len()).sum();
    let mut out = Vec::with_capacity(total);
    for chunk in chunks {
        out.extend_from_slice(&chunk);
    }
    out
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
    decode_bytes(std::fs::read(path)?)
}

/// Take ownership of an in-memory Archive II buffer, decompress (if gzipped),
/// run the upstream `Scan` decode, and grab the first MSG_2. Same contract
/// as [`decode_scan_and_msg2`], factored out so both the file path and the
/// chunks path share the pipeline.
fn decode_bytes(
    data: Vec<u8>,
) -> Result<(nexrad_model::data::Scan, Option<RdaStatusMessage<'static>>)> {
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

    #[test]
    fn concat_chunks_preserves_bytes() {
        // Pinning the assembly contract: concat must be a flat byte-for-byte
        // join, no header rewriting. If a future "smart" implementation
        // tries to strip per-chunk headers, this test catches it.
        let a = vec![0x41, 0x52, 0x32, 0x56]; // "AR2V" magic
        let b = vec![4, 5, 6, 7, 8, 9];
        let c = vec![10, 11];
        let out = concat_chunks(vec![a.clone(), b.clone(), c.clone()]);
        let expected: Vec<u8> = a.into_iter().chain(b).chain(c).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn concat_chunks_empty_returns_empty() {
        let out = concat_chunks(Vec::<Vec<u8>>::new());
        assert!(out.is_empty());
    }

    /// Real-fixture round-trip test: read the entire fixture as bytes, split
    /// at arbitrary byte boundaries, hand the splits to `read_chunks_volume`,
    /// and verify the resulting volume metadata matches the path-based read.
    /// Gated on `RADISH_NEXRAD_FIXTURE` like the other integration tests.
    #[test]
    fn read_chunks_volume_round_trips_full_file() {
        let path = match std::env::var("RADISH_NEXRAD_FIXTURE") {
            Ok(p) => p,
            Err(_) => return,
        };
        let bytes = std::fs::read(&path).expect("read fixture");
        // Three-way split at thirds — any in-buffer split should round-trip
        // because we just concat them back.
        let n = bytes.len();
        let chunks = vec![
            bytes[..n / 3].to_vec(),
            bytes[n / 3..2 * n / 3].to_vec(),
            bytes[2 * n / 3..].to_vec(),
        ];

        let backend = NexradBackend::new();
        let from_path = backend.read_volume(Path::new(&path)).expect("path decode");
        let from_chunks = backend.read_chunks_volume(chunks).expect("chunks decode");

        assert_eq!(
            from_path.metadata.instrument_name,
            from_chunks.metadata.instrument_name
        );
        assert_eq!(from_path.num_sweeps(), from_chunks.num_sweeps());
        assert_eq!(
            from_path.metadata.sweep_fixed_angles,
            from_chunks.metadata.sweep_fixed_angles
        );
        // Pin the MSG_2 / MSG_5 attrs round-trip as well — this is the surface
        // most likely to silently degrade if the chunked path drops a record.
        let na = from_path
            .metadata
            .nexrad
            .expect("path: nexrad attrs present");
        let nb = from_chunks
            .metadata
            .nexrad
            .expect("chunks: nexrad attrs present");
        assert_eq!(na, nb);
    }
}

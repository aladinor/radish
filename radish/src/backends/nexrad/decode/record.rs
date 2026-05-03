//! LDM (Local Data Manager) record splitting + bzip2 decompression.
//!
//! NEXRAD Level 2 Archive II files (modern, post-2016) are written as:
//!
//! ```text
//! [optional 24-byte Volume Header]
//! [LDM record N]:  i32 size_be  ||  bzip2-compressed message stream
//! [LDM record N+1]: ...
//! ```
//!
//! The `size_be` prefix is the compressed-payload length. A negative
//! sign-bit signals a "control" record (per ICD §3.5) — we treat
//! its absolute value as the size and decompress unconditionally;
//! the decompressed payload is normal NEXRAD messages either way.
//!
//! **Pre-2016 CTM (2432-byte fixed frames) is not supported** — see
//! `NexradDecodeError::UnsupportedRecordFormat` and plan 0004.
//!
//! The decompression path is wired through rayon's `par_iter` so we
//! preserve the ~6× speedup that `nexrad-data/parallel` provides
//! upstream (see `CLAUDE.md`'s performance gotcha note).

use std::io::Read;

use byteorder::{BigEndian, ByteOrder};
use bzip2::read::BzDecoder;
use rayon::prelude::*;

use super::error::{NexradDecodeError, Result};

/// One LDM record's compressed payload, as a borrowed slice into the
/// caller's `&[u8]`. `offset` is the start of the size-prefix in the
/// original file, surfaced in errors so a caller can pinpoint a
/// specific bad record.
#[derive(Debug, Clone)]
pub(crate) struct LdmRecord<'a> {
    pub(crate) offset: usize,
    pub(crate) compressed: &'a [u8],
}

/// Split an LDM-formatted byte stream into a vector of records,
/// already pointing at each record's compressed payload. The
/// optional 24-byte Volume Header (if present at byte 0) is
/// **skipped**; callers parse it separately via
/// `volume::parse_volume_header(&bytes[..24])`.
///
/// LDM detection: the first 24 bytes look like a Volume Header if
/// they start with `AR2V` (per ICD §3.4.1). Otherwise we assume the
/// file starts directly at an LDM size-prefix.
pub(crate) fn split_ldm_records(bytes: &[u8]) -> Result<Vec<LdmRecord<'_>>> {
    let mut pos = if bytes.len() >= 4 && &bytes[..4] == b"AR2V" {
        24
    } else {
        0
    };

    if bytes.len() < pos + 4 {
        return Err(NexradDecodeError::UnsupportedRecordFormat);
    }

    let mut records = Vec::new();
    while pos + 4 <= bytes.len() {
        let raw_size = BigEndian::read_i32(&bytes[pos..pos + 4]);
        let size = raw_size.unsigned_abs() as usize;
        let payload_start = pos + 4;
        let payload_end =
            payload_start
                .checked_add(size)
                .ok_or(NexradDecodeError::MalformedHeader {
                    offset: pos,
                    reason: "LDM record size overflow",
                })?;
        if payload_end > bytes.len() {
            return Err(NexradDecodeError::UnexpectedEof {
                offset: pos,
                needed: 4 + size,
                available: bytes.len().saturating_sub(pos),
            });
        }
        records.push(LdmRecord {
            offset: pos,
            compressed: &bytes[payload_start..payload_end],
        });
        pos = payload_end;
    }
    Ok(records)
}

/// Decompress a single LDM record's payload. Returns the
/// uncompressed message stream as an owned `Vec<u8>`.
pub(crate) fn decompress(record: &LdmRecord<'_>) -> Result<Vec<u8>> {
    let mut decoder = BzDecoder::new(record.compressed);
    // Most modern records decompress to ~250 KB; preallocate to
    // dodge a couple of `Vec` regrowths on the hot path.
    let mut out = Vec::with_capacity(record.compressed.len() * 4);
    decoder
        .read_to_end(&mut out)
        .map_err(|source| NexradDecodeError::Decompression {
            offset: record.offset,
            source,
        })?;
    Ok(out)
}

/// Decompress every record in parallel via rayon. The output vector
/// preserves input order — `decompress_all(records)[i]` corresponds
/// to `records[i]`.
pub(crate) fn decompress_all(records: &[LdmRecord<'_>]) -> Result<Vec<Vec<u8>>> {
    records.par_iter().map(decompress).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bzip2::write::BzEncoder;
    use bzip2::Compression;
    use std::io::Write;

    /// Wrap `payload` as one LDM record (size-prefixed, bzip2-compressed).
    fn make_ldm_record(payload: &[u8]) -> Vec<u8> {
        let mut compressed = Vec::new();
        let mut enc = BzEncoder::new(&mut compressed, Compression::default());
        enc.write_all(payload).unwrap();
        enc.finish().unwrap();
        let mut frame = Vec::with_capacity(4 + compressed.len());
        frame.extend_from_slice(&(compressed.len() as i32).to_be_bytes());
        frame.extend_from_slice(&compressed);
        frame
    }

    #[test]
    fn split_one_record_with_no_volume_header() {
        let payload = b"hello, world";
        let frame = make_ldm_record(payload);
        let records = split_ldm_records(&frame).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].offset, 0);
    }

    #[test]
    fn split_skips_ar2v_volume_header() {
        let payload = b"hello";
        let frame = make_ldm_record(payload);
        let mut bytes = b"AR2V0006.001-XYZWXYZWXYZW".to_vec(); // 24 bytes
        bytes.truncate(24);
        bytes.extend_from_slice(&frame);
        let records = split_ldm_records(&bytes).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].offset, 24);
    }

    #[test]
    fn split_multiple_records_preserves_order() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&make_ldm_record(b"first"));
        let off1 = bytes.len();
        bytes.extend_from_slice(&make_ldm_record(b"second"));
        let off2 = bytes.len();
        bytes.extend_from_slice(&make_ldm_record(b"third"));
        let records = split_ldm_records(&bytes).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].offset, 0);
        assert_eq!(records[1].offset, off1);
        assert_eq!(records[2].offset, off2);
    }

    #[test]
    fn split_handles_negative_size_control_record() {
        // ICD §3.5: a negative sign-bit on the size signals a control
        // record, but the absolute value is still the payload length.
        let payload = b"control-record-payload";
        let mut compressed = Vec::new();
        let mut enc = BzEncoder::new(&mut compressed, Compression::default());
        enc.write_all(payload).unwrap();
        enc.finish().unwrap();
        let mut frame = Vec::with_capacity(4 + compressed.len());
        let signed_size = -(compressed.len() as i32);
        frame.extend_from_slice(&signed_size.to_be_bytes());
        frame.extend_from_slice(&compressed);
        let records = split_ldm_records(&frame).unwrap();
        assert_eq!(records.len(), 1);
        // Decompresses to the original payload regardless of sign.
        let out = decompress(&records[0]).unwrap();
        assert_eq!(out, payload);
    }

    #[test]
    fn split_errors_on_truncated_payload() {
        let mut frame = make_ldm_record(b"hello");
        frame.truncate(frame.len() - 4);
        let err = split_ldm_records(&frame).unwrap_err();
        assert!(
            matches!(err, NexradDecodeError::UnexpectedEof { .. }),
            "expected UnexpectedEof, got {err:?}"
        );
    }

    #[test]
    fn decompress_round_trips_payload() {
        let payload = b"the quick brown fox jumps over the lazy dog";
        let frame = make_ldm_record(payload);
        let records = split_ldm_records(&frame).unwrap();
        let out = decompress(&records[0]).unwrap();
        assert_eq!(out, payload);
    }

    #[test]
    fn decompress_all_preserves_order() {
        let payloads: Vec<&[u8]> = vec![b"alpha", b"bravo", b"charlie", b"delta"];
        let mut bytes = Vec::new();
        for p in &payloads {
            bytes.extend_from_slice(&make_ldm_record(p));
        }
        let records = split_ldm_records(&bytes).unwrap();
        let outs = decompress_all(&records).unwrap();
        for (i, want) in payloads.iter().enumerate() {
            assert_eq!(&outs[i][..], *want, "record {i} order");
        }
    }

    #[test]
    fn decompress_errors_on_bad_bzip2_stream() {
        // Construct a "record" whose payload isn't valid bzip2.
        let bytes = {
            let payload = vec![0xFFu8; 32];
            let mut frame = Vec::with_capacity(4 + payload.len());
            frame.extend_from_slice(&(payload.len() as i32).to_be_bytes());
            frame.extend_from_slice(&payload);
            frame
        };
        let records = split_ldm_records(&bytes).unwrap();
        assert!(matches!(
            decompress(&records[0]).unwrap_err(),
            NexradDecodeError::Decompression { .. }
        ));
    }
}

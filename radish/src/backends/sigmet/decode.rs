//! Top-level walker for IRIS RAW.
//!
//! Strategy:
//!
//! 1. Find the `INGEST_HEADER` (id=23). It's at offset 0 if the file is
//!    bare; at offset 6144 if a `PRODUCT_HDR` (id=27) wraps it.
//! 2. Parse `INGEST_CONFIGURATION` (offset +12) and `TASK_CONFIGURATION`
//!    (offset +12+480) for site / task / range / scan-mode metadata plus
//!    the `dsp_data_mask` bitfield identifying enabled moments.
//! 3. Skip past the rest of the metadata records — the first sweep-data
//!    record is at offset `(num_metadata_records) * 6144`. For the common
//!    case (PRODUCT_HDR + INGEST_HEADER), that's record 2 = offset 12288.
//! 4. Walk sweep-data records. Each record starts with a 12-byte
//!    `RAW_PROD_BHDR` identifying its sweep number and where the first
//!    ray in the record starts. The first record of each (sweep, moment)
//!    block is preceded by an `INGEST_DATA_HEADER` (one per active moment).
//! 5. Per-ray: 12-byte `RAY_HEADER` + RLE-compressed bin data terminated
//!    by a `cmp_val=1, MSB=0` marker. Bin width is 1 or 2 bytes depending
//!    on the moment's data type.
//!
//! Bin RLE encoding (ported from xradar's `get_compression_code`):
//!
//! * code is read as a signed 16-bit LE int.
//! * If the high bit is set (code is negative as i16): "copy" mode —
//!   `cmp_val = (code as u16) & 0x7FFF` is the number of 16-bit words of
//!   literal data following.
//! * If the high bit is clear (code is positive): "skip" mode —
//!   `cmp_val` is the number of 16-bit words to skip (treated as missing
//!   data, written as zeros so the per-data-type decoder can map raw==0
//!   to NaN).
//! * `cmp_val == 1, MSB clear` is the end-of-ray sentinel.
//!
//! Cross-record handling: the RLE stream is logically continuous across
//! record boundaries, but every 6144-byte record begins with a
//! `RAW_PROD_BHDR` that must be skipped. `RecordReader` below abstracts
//! that — calls return successive 16-bit code words / data words while
//! transparently jumping the per-record header.

use std::collections::HashMap;

use byteorder::{LittleEndian, ReadBytesExt};
use chrono::{DateTime, Utc};

use crate::{RadishError, Result};

use super::calibration::Decoder;
use super::mapping::{moment_for_id, SigmetMoment};
use super::structs::{
    bin2_to_degrees, IngestConfiguration, IngestDataHeader, RawProdBhdr, ScanMode, StructureHeader,
    TaskConfiguration, INGEST_CONFIGURATION_BYTES, RECORD_BYTES, STRUCTURE_HEADER_BYTES,
    STRUCT_ID_INGEST_HEADER, STRUCT_ID_PRODUCT_HDR,
};

/// Top-level decoded volume: metadata + per-sweep per-ray decoded gates.
#[derive(Debug, Clone)]
pub(super) struct DecodedVolume {
    pub site_name: String,
    pub iris_version: String,
    pub task_name: String,
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub altitude_m: f64,
    pub volume_start_time: DateTime<Utc>,
    pub scan_mode: ScanMode,
    pub nyquist_velocity_ms: f32,
    pub prf_hz: f32,
    pub unambiguous_range_m: f32,
    /// Per-gate range axis in metres.
    pub range_axis_m: Vec<f32>,
    pub gate_count: usize,
    pub sweeps: Vec<DecodedSweep>,
}

/// One sweep's worth of decoded rays.
#[derive(Debug, Clone)]
pub(super) struct DecodedSweep {
    pub sweep_number: u32,
    pub fixed_angle_deg: f32,
    pub start_time: DateTime<Utc>,
    pub rays: Vec<DecodedRay>,
}

/// One ray's azimuth/elevation/time + per-moment decoded f32 bins.
#[derive(Debug, Clone)]
pub(super) struct DecodedRay {
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    pub time_offset_s: f32,
    /// Per IRIS data-type id (matches `SigmetMoment.data_type_id`),
    /// `Vec<f32>` of length `gate_count` (NaN for missing/skip cells).
    pub moments: HashMap<u8, Vec<f32>>,
}

// ---- top-level entry ----------------------------------------------------

/// Decode an in-memory IRIS RAW buffer.
pub(super) fn parse_volume(data: &[u8]) -> Result<DecodedVolume> {
    if data.len() < STRUCTURE_HEADER_BYTES {
        return Err(RadishError::MalformedRecord {
            offset: 0,
            msg: "file too short for STRUCTURE_HEADER".to_string(),
        });
    }

    // Locate the INGEST_HEADER. Two layouts handled:
    //   bare      : INGEST_HEADER at offset 0
    //   wrapped   : PRODUCT_HDR (id=27) record 0, INGEST_HEADER record 1
    let first = StructureHeader::parse(&data[..STRUCTURE_HEADER_BYTES])?;
    let (ingest_header_offset, metadata_records) = match first.structure_identifier {
        STRUCT_ID_INGEST_HEADER => (0usize, 1usize),
        STRUCT_ID_PRODUCT_HDR => (RECORD_BYTES, 2usize),
        other => {
            return Err(RadishError::InvalidFormat(format!(
                "unexpected structure_identifier {other}; expected 23 (INGEST_HEADER) or 27 (PRODUCT_HDR)"
            )))
        }
    };

    if data.len() < ingest_header_offset + STRUCTURE_HEADER_BYTES + INGEST_CONFIGURATION_BYTES {
        return Err(RadishError::MalformedRecord {
            offset: ingest_header_offset as u64,
            msg: "INGEST_HEADER does not fit in file".to_string(),
        });
    }

    // Confirm the INGEST_HEADER's STRUCTURE_HEADER.
    let ingest_struct_header = StructureHeader::parse(
        &data[ingest_header_offset..ingest_header_offset + STRUCTURE_HEADER_BYTES],
    )?;
    if ingest_struct_header.structure_identifier != STRUCT_ID_INGEST_HEADER {
        return Err(RadishError::InvalidFormat(format!(
            "expected INGEST_HEADER (id=23) at offset {ingest_header_offset}, got id={}",
            ingest_struct_header.structure_identifier
        )));
    }

    // INGEST_CONFIGURATION starts immediately after INGEST_HEADER's STRUCTURE_HEADER.
    let cfg_off = ingest_header_offset + STRUCTURE_HEADER_BYTES;
    let cfg = IngestConfiguration::parse(&data[cfg_off..cfg_off + INGEST_CONFIGURATION_BYTES])?;

    // TASK_CONFIGURATION starts after INGEST_CONFIGURATION.
    let task_off = cfg_off + INGEST_CONFIGURATION_BYTES;
    if data.len() < task_off + 1892 {
        return Err(RadishError::MalformedRecord {
            offset: task_off as u64,
            msg: "TASK_CONFIGURATION does not fit in file".to_string(),
        });
    }
    let task_buf_len = (data.len() - task_off).min(2612);
    let task = TaskConfiguration::parse(&data[task_off..task_off + task_buf_len])?;

    // Build the active moment list from dsp_data_mask. Each set bit
    // identifies a SIGMET_DATA_TYPES id present in the volume. xradar's
    // `_data_types_from_dsp_mask` (iris.py:430) walks bits 0..127 across
    // four mask words; we pack those into a single u128 in
    // `TaskConfiguration::parse` and iterate the same range here.
    //
    // Moments not yet implemented in our `SUPPORTED_MOMENTS` table
    // (DB_HCLASS, DB_DBTE8, DB_DBZE8, etc.) get filtered out by
    // `moment_for_id` returning None — but we still need to know how
    // many INGEST_DATA_HEADERs / RLE ray streams the file carries so
    // the sweep walker can advance past them. That count is the total
    // bit-set count, not just the supported subset.
    // `active_data_type_ids` is the FULL list of IRIS data-type ids
    // present in the file, in ascending bit-order. The encoder writes
    // one INGEST_DATA_HEADER per active id and interleaves rays in the
    // same order. We must walk every id to keep the RLE stream in sync,
    // even for ids we don't decode (e.g. DB_HCLASS, DB_DBTE8). The
    // adapter later filters down to the supported subset using
    // `moment_for_id`.
    let active_data_type_ids: Vec<u8> = (0..128)
        .filter(|&bit| (task.dsp_data_mask >> bit) & 1 == 1)
        .map(|bit| bit as u8)
        .collect();

    // Per-gate range axis: `range_first_bin_cm + i * step_output_bins_cm`,
    // converted to metres. `bins_output` is the number of OUTPUT gates.
    let gate_count = task.bins_output as usize;
    let first_gate_m = task.range_first_bin_cm as f32 / 100.0;
    let step_m = task.step_output_bins_cm as f32 / 100.0;
    let range_axis_m: Vec<f32> = (0..gate_count)
        .map(|i| first_gate_m + (i as f32) * step_m)
        .collect();

    // Walk sweep-data records starting at the first record after metadata.
    let data_start = metadata_records * RECORD_BYTES;
    let sweeps = parse_sweeps(
        data,
        data_start,
        task.sweeps_per_volume as usize,
        gate_count,
        &active_data_type_ids,
        task.nyquist_velocity_ms,
    )?;

    Ok(DecodedVolume {
        site_name: cfg.site_name,
        iris_version: cfg.iris_version,
        task_name: task.task_name,
        latitude_deg: cfg.latitude_deg,
        longitude_deg: cfg.longitude_deg,
        altitude_m: (cfg.altitude_radar_cm as f64 / 100.0).max(
            (cfg.height_site_m + cfg.height_radar_m) as f64,
        ),
        volume_start_time: cfg.volume_scan_start_time,
        scan_mode: task.scan_mode,
        nyquist_velocity_ms: task.nyquist_velocity_ms,
        prf_hz: task.prf_hz,
        unambiguous_range_m: task.unambiguous_range_m,
        range_axis_m,
        gate_count,
        sweeps,
    })
}

// ---- sweep / ray walker -------------------------------------------------

/// Cross-record reader. Consumes 16-bit LE words from the sweep-data
/// stream, transparently skipping the 12-byte `RAW_PROD_BHDR` at every
/// 6144-byte record boundary.
struct RecordReader<'a> {
    data: &'a [u8],
    /// Current absolute byte position in `data`.
    pos: usize,
    /// Inclusive end of the sweep-data region we're allowed to read from.
    end: usize,
}

impl<'a> RecordReader<'a> {
    fn new(data: &'a [u8], start: usize, end: usize) -> Self {
        Self {
            data,
            pos: start,
            end: end.min(data.len()),
        }
    }

    /// True when we've consumed all available data.
    fn is_done(&self) -> bool {
        self.pos >= self.end
    }

    fn record_offset_of(&self, pos: usize) -> usize {
        (pos / RECORD_BYTES) * RECORD_BYTES
    }

    /// If `self.pos` is at the start of a record, skip the 12-byte
    /// `RAW_PROD_BHDR`. Idempotent — only the head of a record gets skipped.
    fn skip_record_header_if_at_boundary(&mut self) {
        if self.pos < self.end && self.pos % RECORD_BYTES == 0 {
            self.pos += 12; // RAW_PROD_BHDR
        }
    }

    /// Read one little-endian u16 word, skipping record headers as needed.
    fn read_u16(&mut self) -> Result<u16> {
        self.skip_record_header_if_at_boundary();
        if self.pos + 2 > self.end {
            return Err(RadishError::MalformedRecord {
                offset: self.pos as u64,
                msg: "unexpected EOF reading u16".to_string(),
            });
        }
        let lo = self.data[self.pos] as u16;
        let hi = self.data[self.pos + 1] as u16;
        self.pos += 2;
        // If our 2-byte read straddled a record boundary, advance past
        // the next record's header so the *next* read sees data, not
        // RAW_PROD_BHDR. (In practice the IRIS encoder aligns codes on
        // record boundaries so straddling shouldn't happen, but defensive.)
        if self.record_offset_of(self.pos - 2) != self.record_offset_of(self.pos) {
            // Already crossed; skip header at our new record's start.
            // (No-op if `pos` is already past the header.)
            self.skip_record_header_if_at_boundary();
        }
        Ok(lo | (hi << 8))
    }

    /// Read a signed i16 (sign of the compression code matters).
    fn read_i16(&mut self) -> Result<i16> {
        self.read_u16().map(|v| v as i16)
    }

    /// Skip `n` bytes (only used for ray-header skip when explicitly
    /// requested; not used by RLE).
    #[allow(dead_code)]
    fn skip(&mut self, n: usize) -> Result<()> {
        if self.pos + n > self.end {
            return Err(RadishError::MalformedRecord {
                offset: self.pos as u64,
                msg: format!("skip past end: {n} bytes"),
            });
        }
        self.pos += n;
        Ok(())
    }
}

/// Parse all sweeps in the volume.
///
/// `active_data_type_ids` is the full set of IRIS data-type ids present
/// in the file (from the DSP data mask). We walk every id so the RLE
/// stream stays in sync — but only ids whose `moment_for_id` lookup
/// succeeds get their decoded values stashed; unsupported ids are read
/// past with `DECODE_NONE` and discarded.
fn parse_sweeps(
    data: &[u8],
    data_start: usize,
    sweeps_per_volume: usize,
    gate_count: usize,
    active_data_type_ids: &[u8],
    nyquist_ms: f32,
) -> Result<Vec<DecodedSweep>> {
    let mut sweeps = Vec::with_capacity(sweeps_per_volume);
    let mut cursor = data_start;

    for sweep_idx in 0..sweeps_per_volume {
        if cursor >= data.len() {
            break;
        }

        // Each sweep starts with a record carrying RAW_PROD_BHDR + N
        // INGEST_DATA_HEADERs (one per active moment), followed by ray data.
        let bhdr = RawProdBhdr::parse(&data[cursor..cursor + 12])?;
        if bhdr.sweep_number as i64 != (sweep_idx as i64) + 1 {
            // Some volumes index sweeps from 0 or 1 depending on encoder; we
            // accept both. If we're really off, abort the walk.
            if bhdr.sweep_number != 0 && bhdr.sweep_number as usize != sweep_idx + 1 {
                break;
            }
        }

        // Parse one INGEST_DATA_HEADER per active id. They sit immediately
        // after the RAW_PROD_BHDR in the record. Use the IDH's own
        // `bits_per_bin` field to drive RLE width — it's authoritative
        // even when the id isn't in our SUPPORTED_MOMENTS table.
        let mut idh_off = cursor + 12;
        let mut idhs: Vec<IngestDataHeader> = Vec::with_capacity(active_data_type_ids.len());
        for _ in active_data_type_ids {
            if idh_off + 76 > data.len() {
                break;
            }
            let idh = IngestDataHeader::parse(&data[idh_off..idh_off + 76])?;
            idhs.push(idh);
            idh_off += 76;
        }

        let nrays = idhs
            .first()
            .map(|h| h.number_rays_per_sweep as usize)
            .unwrap_or(360);
        let fixed_angle = idhs.first().map(|h| h.fixed_angle_deg).unwrap_or(0.0);
        let start_time = idhs
            .first()
            .map(|h| h.sweep_start_time)
            .unwrap_or_else(DateTime::<Utc>::default);

        let sweep_end = data.len();
        let mut reader = RecordReader::new(data, idh_off, sweep_end);

        let mut rays: Vec<DecodedRay> = (0..nrays)
            .map(|_| DecodedRay {
                azimuth_deg: f32::NAN,
                elevation_deg: f32::NAN,
                time_offset_s: 0.0,
                moments: HashMap::with_capacity(active_data_type_ids.len()),
            })
            .collect();

        for ray_idx in 0..nrays {
            for (moment_pos, idh) in idhs.iter().enumerate() {
                if reader.is_done() {
                    break;
                }
                let bytes_per_bin = (idh.bits_per_bin as usize) / 8;
                if bytes_per_bin == 0 {
                    // Encoder said "0 bits" — nothing to decode, skip.
                    continue;
                }
                let supported = moment_for_id(idh.data_type);
                let decoder: Decoder = supported
                    .map(|m| m.decoder)
                    .unwrap_or(super::calibration::DECODE_NONE);
                if let Some((az, el, t_off, decoded)) =
                    decode_one_ray(&mut reader, gate_count, bytes_per_bin, decoder, nyquist_ms)?
                {
                    if moment_pos == 0 {
                        rays[ray_idx].azimuth_deg = az;
                        rays[ray_idx].elevation_deg = el;
                        rays[ray_idx].time_offset_s = t_off;
                    }
                    if let Some(m) = supported {
                        rays[ray_idx].moments.insert(m.data_type_id, decoded);
                    }
                } else if let Some(m) = supported {
                    rays[ray_idx]
                        .moments
                        .insert(m.data_type_id, vec![f32::NAN; gate_count]);
                }
            }
        }

        sweeps.push(DecodedSweep {
            sweep_number: (sweep_idx + 1) as u32,
            fixed_angle_deg: fixed_angle,
            start_time,
            rays,
        });

        // Advance cursor to the next record-aligned position past where
        // the reader stopped. The next sweep's RAW_PROD_BHDR is at the
        // start of the next record we haven't yet visited.
        let next = ((reader.pos + RECORD_BYTES - 1) / RECORD_BYTES) * RECORD_BYTES;
        cursor = next;
    }

    Ok(sweeps)
}

/// Decode one ray. RLE stream output layout:
///   word 0: az_start  (BIN2)
///   word 1: el_start  (BIN2)
///   word 2: az_stop   (BIN2)
///   word 3: el_stop   (BIN2)
///   word 4: rbins     (UINT2)
///   word 5: dtime     (UINT2 — seconds within sweep)
///   words 6..N: bin data (8-bit moments pack 2 gates per word; 16-bit
///                         moments use one word per gate).
///
/// xradar treats the whole ray (header + bins) as a single RLE stream
/// terminated by `cmp_val=1, MSB=0`. We follow the same scheme: allocate
/// `6 + gate_count_words` of u16 storage, run the RLE decoder, then
/// interpret the first 6 words as the header.
///
/// Returns `Ok(None)` if the very first code is the missing-ray sentinel.
fn decode_one_ray(
    reader: &mut RecordReader<'_>,
    gate_count: usize,
    bytes_per_bin: usize,
    decoder: Decoder,
    nyquist_ms: f32,
) -> Result<Option<(f32, f32, f32, Vec<f32>)>> {
    if reader.is_done() {
        return Ok(None);
    }

    // xradar (`iris.py:3525`): per-ray buffer is **gate_count + 6 int16
    // words regardless of bytes_per_bin**. For 8-bit moments the int16
    // array is later viewed as `(2,) uint8` and sliced `[:, :gate_count]`,
    // so half the int16 words are dropped — but the RLE stream still
    // encodes the full `gate_count + 6` int16 word stride per ray.
    let buf_len = gate_count + 6;
    let mut raw_words: Vec<u16> = vec![0; buf_len];
    let mut word_pos = 0usize;

    // Peek the first compression code. If it's `cmp_val=1, MSB=0` we're
    // looking at a missing-ray sentinel — return None so the caller fills
    // an all-NaN row.
    let first_code = reader.read_i16()?;
    let first_msb = first_code < 0;
    let first_val = if first_msb {
        (first_code as i32 + 32768) as usize
    } else {
        first_code as usize
    };
    if !first_msb && first_val == 1 {
        return Ok(None);
    }

    // Process the first code we already read.
    apply_rle_step(reader, &mut raw_words, &mut word_pos, first_msb, first_val)?;

    // Continue until the end-of-ray sentinel.
    loop {
        let code = reader.read_i16()?;
        let cmp_msb = code < 0;
        let cmp_val = if cmp_msb {
            (code as i32 + 32768) as usize
        } else {
            code as usize
        };
        if !cmp_msb && cmp_val == 1 {
            break;
        }
        apply_rle_step(reader, &mut raw_words, &mut word_pos, cmp_msb, cmp_val)?;
    }

    // RAY_HEADER from the first 6 words.
    let az_start = raw_words[0];
    let el_start = raw_words[1];
    let az_stop = raw_words[2];
    let _el_stop = raw_words[3];
    let _rbins = raw_words[4];
    let dtime = raw_words[5];

    // Bin data from words 6..end.
    let bin_words = &raw_words[6..];
    let mut decoded = Vec::with_capacity(gate_count);
    if bytes_per_bin == 1 {
        // Two 8-bit gates per 16-bit word: low byte = gate 2k, high byte = gate 2k+1.
        for w in bin_words.iter().take(gate_count.div_ceil(2)) {
            decoded.push(decoder((w & 0xFF) as u16, nyquist_ms));
            if decoded.len() < gate_count {
                decoded.push(decoder((w >> 8) as u16, nyquist_ms));
            }
        }
    } else {
        for w in bin_words.iter().take(gate_count) {
            decoded.push(decoder(*w, nyquist_ms));
        }
    }
    decoded.resize(gate_count, f32::NAN);

    let azimuth = (bin2_to_degrees(az_start) as f32 + bin2_to_degrees(az_stop) as f32) * 0.5;
    let elevation = bin2_to_degrees(el_start) as f32;
    let time_offset_s = dtime as f32;

    Ok(Some((azimuth, elevation, time_offset_s, decoded)))
}

/// Apply one RLE step (copy `cmp_val` words OR skip `cmp_val` words).
/// Splits out from the loop so the first peeked code can reuse the logic.
fn apply_rle_step(
    reader: &mut RecordReader<'_>,
    raw_words: &mut [u16],
    word_pos: &mut usize,
    cmp_msb: bool,
    cmp_val: usize,
) -> Result<()> {
    if cmp_msb {
        for _ in 0..cmp_val {
            if *word_pos >= raw_words.len() {
                let _ = reader.read_u16()?; // drain past end (encoder over-shoot)
                continue;
            }
            raw_words[*word_pos] = reader.read_u16()?;
            *word_pos += 1;
        }
    } else {
        *word_pos += cmp_val;
    }
    if *word_pos > raw_words.len() + 64 {
        return Err(RadishError::MalformedRecord {
            offset: reader.pos as u64,
            msg: format!(
                "RLE decode wrote {} past end of {}-word ray buffer",
                *word_pos - raw_words.len(),
                raw_words.len()
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A buffer that's not even big enough for STRUCTURE_HEADER.
    #[test]
    fn parse_volume_rejects_short_buffer() {
        let result = parse_volume(&[0u8; 4]);
        assert!(result.is_err());
    }

    /// A buffer with a non-IRIS structure_identifier should be rejected
    /// with InvalidFormat (not a malformed-record).
    #[test]
    fn parse_volume_rejects_unknown_structure_identifier() {
        let mut buf = vec![0u8; 4096];
        // id=99 LE
        buf[0] = 0x63;
        buf[1] = 0x00;
        let result = parse_volume(&buf);
        match result {
            Err(RadishError::InvalidFormat(_)) => (),
            other => panic!("expected InvalidFormat, got {other:?}"),
        }
    }
}

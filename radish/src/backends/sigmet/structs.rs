//! On-wire struct parsers for IRIS RAW.
//!
//! Field layouts ported verbatim from xradar's `iris.py` dictionaries
//! (see line 510+ for `STRUCTURE_HEADER`, 1308 for `INGEST_CONFIGURATION`,
//! 1606 for `INGEST_HEADER`, etc.). All fields are little-endian. We use
//! explicit `byteorder::ReadBytesExt::read_iN::<LittleEndian>()` calls
//! rather than `bytemuck`-style `repr(C, packed)` casts so each offset is
//! visible at the call site — debugging an offset mismatch on a real
//! fixture is far easier this way (the Phase 0 spike caught one such
//! mistake immediately).
//!
//! Conventions used here:
//! * `BIN2` (unsigned 16-bit angle): `degrees = raw * 360 / 65536`
//! * `BIN4` (signed 32-bit angle):   `degrees = raw * 180 / 2^31`
//! * `YMDS_TIME` (12 B): seconds + millis + status + year + month + day
//!
//! Sizes (from xradar dict sums):
//! * STRUCTURE_HEADER     : 12 B
//! * INGEST_CONFIGURATION : 480 B
//! * TASK_SCHED_INFO      : 120 B
//! * TASK_DSP_INFO        : 320 B
//! * TASK_CALIB_INFO      : 320 B
//! * TASK_RANGE_INFO      : 160 B
//! * TASK_SCAN_INFO       : 320 B
//! * TASK_MISC_INFO       : 320 B
//! * TASK_END_INFO        : 320 B
//! * TASK_CONFIGURATION   : sum of the above (1880 B core + padding to 2612 B)
//! * INGEST_HEADER        : STRUCTURE_HEADER + spare(12) + INGEST_CONFIGURATION + TASK_CONFIGURATION + spare = 4884 B

use byteorder::{LittleEndian, ReadBytesExt};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use std::io::{Cursor, Read};

use crate::{RadishError, Result};

/// Size of one IRIS record on disk.
pub(super) const RECORD_BYTES: usize = 6144;

/// Size of `STRUCTURE_HEADER` in bytes.
pub(super) const STRUCTURE_HEADER_BYTES: usize = 12;
/// Size of `INGEST_CONFIGURATION` in bytes.
pub(super) const INGEST_CONFIGURATION_BYTES: usize = 480;

/// `structure_identifier` value indicating an `INGEST_HEADER`.
pub(super) const STRUCT_ID_INGEST_HEADER: i16 = 23;
/// `structure_identifier` value indicating a `PRODUCT_HDR`.
pub(super) const STRUCT_ID_PRODUCT_HDR: i16 = 27;

/// `STRUCTURE_HEADER` (12 B). Every IRIS sub-structure begins with one.
#[derive(Debug, Clone, Copy)]
pub(super) struct StructureHeader {
    pub structure_identifier: i16,
    pub format_version: i16,
    pub bytes_in_structure: i32,
    #[allow(dead_code)]
    pub reserved: i16,
    #[allow(dead_code)]
    pub flag: i16,
}

impl StructureHeader {
    pub fn parse(mut buf: &[u8]) -> Result<Self> {
        if buf.len() < STRUCTURE_HEADER_BYTES {
            return Err(RadishError::MalformedRecord {
                offset: 0,
                msg: format!(
                    "STRUCTURE_HEADER needs {STRUCTURE_HEADER_BYTES} bytes, got {}",
                    buf.len()
                ),
            });
        }
        Ok(Self {
            structure_identifier: read_i16_le(&mut buf)?,
            format_version: read_i16_le(&mut buf)?,
            bytes_in_structure: read_i32_le(&mut buf)?,
            reserved: read_i16_le(&mut buf)?,
            flag: read_i16_le(&mut buf)?,
        })
    }
}

/// Decoded subset of `INGEST_CONFIGURATION` used by the adapter.
#[derive(Debug, Clone)]
pub(super) struct IngestConfiguration {
    pub iris_version: String,
    pub site_name: String,
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub height_site_m: i16,
    pub height_radar_m: i16,
    pub altitude_radar_cm: i32,
    pub volume_scan_start_time: DateTime<Utc>,
    pub task_config_count: i16,
}

impl IngestConfiguration {
    /// Parse `INGEST_CONFIGURATION` from the 480-byte sub-buffer that
    /// follows `STRUCTURE_HEADER` inside `INGEST_HEADER`. Field offsets
    /// match xradar's dict layout (verified on a real CHI fixture in the
    /// Phase 0 spike).
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < INGEST_CONFIGURATION_BYTES {
            return Err(RadishError::MalformedRecord {
                offset: 0,
                msg: format!(
                    "INGEST_CONFIGURATION needs {INGEST_CONFIGURATION_BYTES} bytes, got {}",
                    buf.len()
                ),
            });
        }
        let mut c = Cursor::new(buf);

        // Layout (cumulative offsets in INGEST_CONFIGURATION):
        //   off 0   filename(80)
        //   off 80  number_files(2) + number_sweeps_completed(2) + total_size(4)
        //   off 88  volume_scan_start_time(12 = YMDS_TIME)
        //   off 100 spare_0(12)
        //   off 112 ray_header_bytes(2) + extended_ray_header_bytes(2)
        //   off 116 number_task_config_table(2) + playback_version(2)
        //   off 120 spare_1(4)
        //   off 124 iris_version(8)
        //   off 132 hardware_site(16)
        //   off 148 gmt_offset_minutes_local(2)
        //   off 150 site_name(16)
        //   off 166 gmt_offset_minutes_standard(2)
        //   off 168 latitude_radar(BIN4)
        //   off 172 longitude_radar(BIN4)
        //   off 176 height_site(2) + height_radar(2)
        //   off 180 (...)
        //   off 192 altitude_radar(SINT4)
        //   off 196 (...) — velocity, antenna offsets, fault status, etc.

        c.set_position(88);
        let volume_scan_start_time = read_ymds_time(&mut c)?;

        c.set_position(116);
        let task_config_count = c.read_i16::<LittleEndian>()?;

        c.set_position(124);
        let iris_version = read_fixed_string(&mut c, 8)?;

        c.set_position(150);
        let site_name = read_fixed_string(&mut c, 16)?;

        c.set_position(168);
        let lat_raw = c.read_i32::<LittleEndian>()?;
        let lon_raw = c.read_i32::<LittleEndian>()?;
        let height_site_m = c.read_i16::<LittleEndian>()?;
        let height_radar_m = c.read_i16::<LittleEndian>()?;

        c.set_position(192);
        let altitude_radar_cm = c.read_i32::<LittleEndian>()?;

        Ok(Self {
            iris_version,
            site_name,
            latitude_deg: bin4_to_degrees(lat_raw),
            longitude_deg: bin4_to_degrees(lon_raw),
            height_site_m,
            height_radar_m,
            altitude_radar_cm,
            volume_scan_start_time,
            task_config_count,
        })
    }
}

/// Decoded subset of `TASK_CONFIGURATION` used by the adapter.
#[derive(Debug, Clone)]
pub(super) struct TaskConfiguration {
    pub task_name: String,
    pub scan_mode: ScanMode,
    pub sweeps_per_volume: u16,
    /// Bitmask of enabled IRIS data-type ids. xradar's `DSP_DATA_MASK`
    /// stores four UINT4 mask words (`mask_word_0..3`) covering bits
    /// 0..127, with `extended_header_type` interleaved between word_0
    /// and word_1. We pack them into a single `u128` so the adapter can
    /// iterate `0..128` and pick out IDs above 31 (DB_HCLASS=55,
    /// DB_DBTE8=71, DB_DBZE8=73, etc. — common in real CHI fixtures).
    pub dsp_data_mask: u128,
    pub nyquist_velocity_ms: f32,
    pub prf_hz: f32,
    pub unambiguous_range_m: f32,
    /// First gate distance in centimetres.
    pub range_first_bin_cm: i32,
    /// Last gate distance in centimetres.
    pub range_last_bin_cm: i32,
    /// Step between gates (cm) on the OUTPUT side after binning.
    pub step_output_bins_cm: i32,
    /// Number of OUTPUT bins per ray (after step / number_input_bins reduction).
    pub bins_output: u16,
    /// Per-sweep target elevation angles (degrees), one per sweep.
    pub sweep_fixed_angles_deg: Vec<f32>,
}

/// Distilled scan mode. The ICD has more values; we collapse them into
/// the shapes the adapter actually treats differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScanMode {
    Ppi,
    Rhi,
    Other(u16),
}

impl ScanMode {
    pub fn label(&self) -> &'static str {
        match self {
            ScanMode::Ppi => "PPI",
            ScanMode::Rhi => "RHI",
            ScanMode::Other(_) => "OTHER",
        }
    }
}

impl TaskConfiguration {
    pub fn parse(buf: &[u8]) -> Result<Self> {
        // TASK_CONFIGURATION layout (cumulative offsets, including the
        // leading STRUCTURE_HEADER which IS part of the block):
        //   off 0     STRUCTURE_HEADER (12)
        //   off 12    TASK_SCHED_INFO  (120)
        //   off 132   TASK_DSP_INFO    (320)
        //   off 452   TASK_CALIB_INFO  (320)
        //   off 772   TASK_RANGE_INFO  (160)
        //   off 932   TASK_SCAN_INFO   (320)
        //   off 1252  TASK_MISC_INFO   (320)
        //   off 1572  TASK_END_INFO    (320)
        //   off 1892  comments(720)
        if buf.len() < 1892 {
            return Err(RadishError::MalformedRecord {
                offset: 0,
                msg: format!("TASK_CONFIGURATION too short: {} bytes", buf.len()),
            });
        }
        const TASK_DSP_INFO_OFF: u64 = 132;
        const TASK_RANGE_INFO_OFF: u64 = 772;
        const TASK_SCAN_INFO_OFF: u64 = 932;
        const TASK_END_INFO_OFF: u64 = 1572;

        let mut c = Cursor::new(buf);

        // ----- TASK_DSP_INFO -----
        // Inside TASK_DSP_INFO (xradar `DSP_DATA_MASK` lives in
        // TASK_DSP_INFO at offset 4):
        //   off 0   iris_task_id (4)
        //   off 4   mask_word_0          (UINT4)  ← bits 0..31
        //   off 8   extended_header_type (UINT4)
        //   off 12  mask_word_1          (UINT4)  ← bits 32..63
        //   off 16  mask_word_2          (UINT4)  ← bits 64..95
        //   off 20  mask_word_3          (UINT4)  ← bits 96..127
        //   off 24  mask_word_4          (UINT4)  (unused for data-type lookup)
        //   off 28  ... DSP options
        // `_data_types_from_dsp_mask` (iris.py:430) ignores word_4 and
        // `extended_header_type`, so we mirror that.
        c.set_position(TASK_DSP_INFO_OFF + 4);
        let mask_word_0 = c.read_u32::<LittleEndian>()? as u128;
        let _extended_header_type = c.read_u32::<LittleEndian>()?;
        let mask_word_1 = c.read_u32::<LittleEndian>()? as u128;
        let mask_word_2 = c.read_u32::<LittleEndian>()? as u128;
        let mask_word_3 = c.read_u32::<LittleEndian>()? as u128;
        let dsp_data_mask = mask_word_0
            | (mask_word_1 << 32)
            | (mask_word_2 << 64)
            | (mask_word_3 << 96);

        // PRF (UINT4 Hz) lives somewhere in TASK_DSP_INFO. Exact offset is
        // version-dependent; xradar's iris.py reads it via
        // `task_dsp_info`'s dict, which puts `prf` at offset 136 in the
        // typical Build 8.x layout. We accept that as best-effort; if
        // unset, fall back to deriving unambiguous_range from
        // range_last_bin_cm.
        c.set_position(TASK_DSP_INFO_OFF + 136);
        let prf_hz = c.read_i32::<LittleEndian>()?.max(0) as f32;

        // ----- nyquist velocity -----
        // xradar derives this from PRF + radar wavelength. We don't have
        // wavelength reliably parsed yet; PR-B follow-up will extract it
        // from TASK_CALIB_INFO. Leave 0 for now — only the 8-bit VEL and
        // WIDTH decoders use it, and a CHI-style fixture using the 16-bit
        // variants is unaffected.
        let nyquist_velocity_ms = 0.0_f32;

        // ----- TASK_RANGE_INFO -----
        //   off 0  range_first_bin (SINT4)
        //   off 4  range_last_bin  (SINT4)
        //   off 8  number_input_bins  (SINT2)
        //   off 10 number_output_bins (SINT2)
        //   off 12 step_input_bins    (SINT4)
        //   off 16 step_output_bins   (SINT4)
        c.set_position(TASK_RANGE_INFO_OFF);
        let range_first_bin_cm = c.read_i32::<LittleEndian>()?;
        let range_last_bin_cm = c.read_i32::<LittleEndian>()?;
        let _number_input_bins = c.read_i16::<LittleEndian>()?;
        let bins_output = c.read_u16::<LittleEndian>()?;
        let _step_input_bins = c.read_i32::<LittleEndian>()?;
        let step_output_bins_cm = c.read_i32::<LittleEndian>()?;

        let unambiguous_range_m = if prf_hz > 0.0 {
            299_792_458.0_f32 / (2.0 * prf_hz)
        } else {
            range_last_bin_cm as f32 / 100.0
        };

        // ----- TASK_SCAN_INFO -----
        // Layout (xradar lines 1773+):
        //   off 0   antenna_scan_mode  (UINT2)
        //   off 2   desired_angular_resolution (SINT2)
        //   off 4   spare_0 (2)
        //   off 6   sweep_number (SINT2)  ← total sweeps in volume
        //   off 8   scan_info (200 B, mode-dependent)
        c.set_position(TASK_SCAN_INFO_OFF);
        let scan_mode_raw = c.read_u16::<LittleEndian>()?;
        let scan_mode = match scan_mode_raw {
            1 | 4 => ScanMode::Ppi,
            2 => ScanMode::Rhi,
            other => ScanMode::Other(other),
        };

        c.set_position(TASK_SCAN_INFO_OFF + 6);
        let sweep_number_total = c.read_i16::<LittleEndian>()?;
        let sweeps_per_volume = sweep_number_total.max(0) as u16;

        c.set_position(TASK_SCAN_INFO_OFF + 8);
        let max_sweeps = sweeps_per_volume.min(40) as usize;
        let mut sweep_fixed_angles_deg = Vec::with_capacity(max_sweeps);
        for _ in 0..max_sweeps {
            let raw = c.read_u16::<LittleEndian>()?;
            sweep_fixed_angles_deg.push(bin2_to_degrees(raw) as f32);
        }

        // ----- TASK_END_INFO -----
        //   off 0   task_major_number (SINT2)
        //   off 2   task_minor_number (SINT2)
        //   off 4   task_configuration_file_name (string_dict(12))
        //   off 16  task_description (string_dict(80))
        //   …
        let task_name =
            read_fixed_string_at(buf, TASK_END_INFO_OFF as usize + 4, 12)?;

        Ok(Self {
            task_name,
            scan_mode,
            sweeps_per_volume,
            dsp_data_mask,
            nyquist_velocity_ms,
            prf_hz,
            unambiguous_range_m,
            range_first_bin_cm,
            range_last_bin_cm,
            step_output_bins_cm,
            bins_output,
            sweep_fixed_angles_deg,
        })
    }
}

/// `RAW_PROD_BHDR` (12 B) — appears at the start of every record carrying
/// sweep data, identifying which sweep / which ray-byte-offset within
/// the record.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(super) struct RawProdBhdr {
    pub record_number: i16,
    pub sweep_number: i16,
    pub first_ray_byte_offset: i16,
    pub sweep_ray_number: i16,
    pub flags: i16,
    pub spare: i16,
}

impl RawProdBhdr {
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < 12 {
            return Err(RadishError::MalformedRecord {
                offset: 0,
                msg: "RAW_PROD_BHDR needs 12 bytes".to_string(),
            });
        }
        let mut c = Cursor::new(buf);
        Ok(Self {
            record_number: c.read_i16::<LittleEndian>()?,
            sweep_number: c.read_i16::<LittleEndian>()?,
            first_ray_byte_offset: c.read_i16::<LittleEndian>()?,
            sweep_ray_number: c.read_i16::<LittleEndian>()?,
            flags: c.read_i16::<LittleEndian>()?,
            spare: c.read_i16::<LittleEndian>()?,
        })
    }
}

/// `INGEST_DATA_HEADER` (76 B) — one per (sweep, moment) pair, recorded
/// at the start of the first record carrying that pair. Layout matches
/// xradar's `INGEST_DATA_HEADER` dict (`iris.py:1634`).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct IngestDataHeader {
    pub structure_header: StructureHeader,
    pub sweep_start_time: DateTime<Utc>,
    pub sweep_number: i16,
    pub number_rays_per_sweep: i16,
    pub first_ray_index: i16,
    pub number_rays_file_expected: i16,
    pub number_rays_file_written: i16,
    pub fixed_angle_deg: f32,
    pub bits_per_bin: i16,
    pub data_type: u8,
}

impl IngestDataHeader {
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < 76 {
            return Err(RadishError::MalformedRecord {
                offset: 0,
                msg: format!("INGEST_DATA_HEADER too short: {} bytes", buf.len()),
            });
        }
        let structure_header = StructureHeader::parse(&buf[..STRUCTURE_HEADER_BYTES])?;

        // INGEST_DATA_HEADER contents after STRUCTURE_HEADER (cumulative
        // offsets within the 76-byte block):
        //   off 12  sweep_start_time (YMDS_TIME, 12 B)
        //   off 24  sweep_number             (SINT2)
        //   off 26  number_rays_per_sweep    (SINT2)
        //   off 28  first_ray_index          (SINT2)
        //   off 30  number_rays_file_expected(SINT2)
        //   off 32  number_rays_file_written (SINT2)
        //   off 34  fixed_angle              (BIN2)
        //   off 36  bits_per_bin             (SINT2)
        //   off 38  data_type                (UINT2 — low byte = id)
        //   off 40  spare_0                  (36 B)
        let mut c = Cursor::new(&buf[STRUCTURE_HEADER_BYTES..]);
        let sweep_start_time = read_ymds_time(&mut c)?;
        let sweep_number = c.read_i16::<LittleEndian>()?;
        let number_rays_per_sweep = c.read_i16::<LittleEndian>()?;
        let first_ray_index = c.read_i16::<LittleEndian>()?;
        let number_rays_file_expected = c.read_i16::<LittleEndian>()?;
        let number_rays_file_written = c.read_i16::<LittleEndian>()?;
        let fixed_angle_raw = c.read_u16::<LittleEndian>()?;
        let bits_per_bin = c.read_i16::<LittleEndian>()?;
        let data_type_u16 = c.read_u16::<LittleEndian>()?;

        Ok(Self {
            structure_header,
            sweep_start_time,
            sweep_number,
            number_rays_per_sweep,
            first_ray_index,
            number_rays_file_expected,
            number_rays_file_written,
            fixed_angle_deg: bin2_to_degrees(fixed_angle_raw) as f32,
            bits_per_bin,
            data_type: (data_type_u16 & 0xFF) as u8,
        })
    }
}

// ---- low-level helpers --------------------------------------------------

/// IRIS BIN4 angle: signed 4-byte value, scaled by 2^31 / 180°.
pub(super) fn bin4_to_degrees(raw: i32) -> f64 {
    raw as f64 * 180.0 / 2_147_483_648.0
}

/// IRIS BIN2 angle: unsigned 2-byte value, scaled by 2^16 / 360°.
pub(super) fn bin2_to_degrees(raw: u16) -> f64 {
    raw as f64 * 360.0 / 65_536.0
}

fn read_i16_le(buf: &mut &[u8]) -> Result<i16> {
    Ok(buf.read_i16::<LittleEndian>()?)
}

fn read_i32_le(buf: &mut &[u8]) -> Result<i32> {
    Ok(buf.read_i32::<LittleEndian>()?)
}

fn read_fixed_string<R: Read>(r: &mut R, len: usize) -> Result<String> {
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(decode_fixed_string(&buf))
}

fn read_fixed_string_at(buf: &[u8], offset: usize, len: usize) -> Result<String> {
    if offset + len > buf.len() {
        return Err(RadishError::MalformedRecord {
            offset: offset as u64,
            msg: format!("string at offset {offset} runs past buffer"),
        });
    }
    Ok(decode_fixed_string(&buf[offset..offset + len]))
}

fn decode_fixed_string(buf: &[u8]) -> String {
    let trimmed: Vec<u8> = buf
        .iter()
        .take_while(|&&b| b != 0)
        .copied()
        .collect();
    String::from_utf8_lossy(&trimmed).trim_end().to_string()
}

/// Read a 12-byte `YMDS_TIME` and return UTC. Layout:
///   seconds (SINT4)
///   millis  (UINT2 — top 4 bits flags)
///   year    (SINT2)
///   month   (SINT2)
///   day     (SINT2)
fn read_ymds_time<R: Read>(r: &mut R) -> Result<DateTime<Utc>> {
    let secs = r.read_i32::<LittleEndian>()?;
    let _millis_and_flags = r.read_u16::<LittleEndian>()?;
    let year = r.read_i16::<LittleEndian>()?;
    let month = r.read_i16::<LittleEndian>()?;
    let day = r.read_i16::<LittleEndian>()?;

    if year <= 0 || month <= 0 || day <= 0 {
        return Ok(DateTime::<Utc>::UNIX_EPOCH);
    }
    let date = NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32).ok_or_else(|| {
        RadishError::MalformedRecord {
            offset: 0,
            msg: format!("invalid YMDS date: {year}-{month}-{day}"),
        }
    })?;
    let dt = date
        .and_hms_opt(0, 0, 0)
        .and_then(|d| d.checked_add_signed(chrono::Duration::seconds(secs as i64)));
    let dt = dt.ok_or_else(|| RadishError::MalformedRecord {
        offset: 0,
        msg: format!("invalid YMDS time: secs={secs}"),
    })?;
    Ok(Utc.from_utc_datetime(&dt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_header_round_trip() {
        // Hand-craft an INGEST_HEADER STRUCTURE_HEADER:
        // id=23 (LE), version=4, bytes_in_structure=4884, reserved=0, flag=0
        let buf = [
            0x17, 0x00, 0x04, 0x00, 0x14, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let sh = StructureHeader::parse(&buf).expect("parse");
        assert_eq!(sh.structure_identifier, 23);
        assert_eq!(sh.format_version, 4);
        assert_eq!(sh.bytes_in_structure, 4884);
    }

    #[test]
    fn structure_header_too_short_returns_error() {
        let buf = [0x17, 0x00, 0x04, 0x00];
        assert!(StructureHeader::parse(&buf).is_err());
    }

    #[test]
    fn bin4_to_degrees_pinned_values() {
        // 0 → 0°; 2^30 → 45°; -2^30 → -45°
        assert_eq!(bin4_to_degrees(0), 0.0);
        assert!((bin4_to_degrees(1 << 30) - 45.0).abs() < 1e-9);
        assert!((bin4_to_degrees(-(1 << 30)) - (-45.0)).abs() < 1e-9);
    }

    #[test]
    fn bin2_to_degrees_pinned_values() {
        // raw=0 → 0°, raw=32768 → 180°, raw=49152 → 270°
        assert_eq!(bin2_to_degrees(0), 0.0);
        assert!((bin2_to_degrees(32768) - 180.0).abs() < 1e-6);
        assert!((bin2_to_degrees(49152) - 270.0).abs() < 1e-6);
    }

    #[test]
    fn decode_fixed_string_strips_nulls_and_trailing_whitespace() {
        assert_eq!(decode_fixed_string(b"hello\0\0\0"), "hello");
        assert_eq!(decode_fixed_string(b"hi   "), "hi");
        assert_eq!(decode_fixed_string(b""), "");
    }
}

//! MSG_31 per-radial data header (ICD §3.2.4.17.1 Table XVII-A).
//!
//! Layout: 32 bytes of fixed fields followed by 10 × `u32` data
//! block pointers (one per possible block, set to 0 if unused).
//!
//! | Offset | Bytes | Field                                                  |
//! |-------:|------:|--------------------------------------------------------|
//! |      0 |     4 | Radar identifier (ICAO, 4 ASCII chars)                 |
//! |      4 |     4 | Collection time (ms past midnight GMT)                 |
//! |      8 |     2 | Modified Julian Date                                   |
//! |     10 |     2 | Azimuth Number (1..720 within elevation scan)          |
//! |     12 |     4 | Azimuth Angle (degrees, f32)                           |
//! |     16 |     1 | Compression Indicator (0=none, 1=BZIP2, 2=zlib, 3=fut) |
//! |     17 |     1 | Spare (halfword alignment)                             |
//! |     18 |     2 | Radial Length (uncompressed bytes incl. header)        |
//! |     20 |     1 | Azimuth Resolution (1=0.5°, 2=1.0°)                    |
//! |     21 |     1 | Radial Status (0=elev start, 2=elev end, ...)          |
//! |     22 |     1 | Elevation Number (1..32 within volume scan)            |
//! |     23 |     1 | Cut Sector Number (0..3)                               |
//! |     24 |     4 | Elevation Angle (degrees, f32)                         |
//! |     28 |     1 | Radial Spot Blanking Status                            |
//! |     29 |     1 | Azimuth Indexing Mode (0=none, 1..100 = 0.01..1.00°)   |
//! |     30 |     2 | Data Block Count (4..10)                               |
//! |  32-71 |    40 | Data Block Pointers ×10 (u32 each, 0 = unused)         |
//!
//! Pointers are byte offsets **relative to the start of the
//! MessageHeader** (i.e. relative to the 12-byte TCM prefix's first
//! byte, _not_ to this header). Layout of the pointer array (per
//! ICD Table XVII-A, in order):
//!
//! | Index | Block                                |
//! |------:|--------------------------------------|
//! |     0 | Volume Data Constant (VOL)           |
//! |     1 | Elevation Data Constant (ELV)        |
//! |     2 | Radial Data Constant (RAD)           |
//! |     3 | Moment "REF"                         |
//! |     4 | Moment "VEL"                         |
//! |     5 | Moment "SW"                          |
//! |     6 | Moment "ZDR"                         |
//! |     7 | Moment "PHI"                         |
//! |     8 | Moment "RHO"                         |
//! |     9 | Moment "CFP"                         |

use crate::backends::nexrad::decode::error::Result;
use crate::backends::nexrad::decode::reader::SliceReader;

/// Width of the fixed (non-pointer) portion of the header.
pub(crate) const FIXED_HEADER_SIZE: usize = 32;

/// Number of data block pointers following the fixed header.
pub(crate) const POINTER_COUNT: usize = 10;

/// Width of one pointer.
pub(crate) const POINTER_SIZE: usize = 4;

/// Total wire width of the data header (fixed fields + pointer array).
pub(crate) const TOTAL_HEADER_SIZE: usize = FIXED_HEADER_SIZE + POINTER_COUNT * POINTER_SIZE;

/// Pointer-array index for each block type. Values are stable per
/// ICD Table XVII-A and used by the dispatcher to route each
/// non-zero pointer to the right parser.
pub(crate) const PTR_VOL: usize = 0;
pub(crate) const PTR_ELV: usize = 1;
pub(crate) const PTR_RAD: usize = 2;
pub(crate) const PTR_REF: usize = 3;
pub(crate) const PTR_VEL: usize = 4;
pub(crate) const PTR_SW: usize = 5;
pub(crate) const PTR_ZDR: usize = 6;
pub(crate) const PTR_PHI: usize = 7;
pub(crate) const PTR_RHO: usize = 8;
pub(crate) const PTR_CFP: usize = 9;

/// One MSG_31 radial's fixed header (no pointers — those live in
/// `pointers`). Field types follow ICD Table XVII-A.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DataHeader {
    pub(crate) radar_identifier: [u8; 4],
    pub(crate) collection_time_ms: u32,
    pub(crate) modified_julian_date: u16,
    pub(crate) azimuth_number: u16,
    pub(crate) azimuth_angle_degrees: f32,
    pub(crate) compression_indicator: u8,
    pub(crate) radial_length: u16,
    pub(crate) azimuth_resolution_spacing: u8,
    pub(crate) radial_status: u8,
    pub(crate) elevation_number: u8,
    pub(crate) cut_sector_number: u8,
    pub(crate) elevation_angle_degrees: f32,
    pub(crate) radial_spot_blanking_status: u8,
    pub(crate) azimuth_indexing_mode: u8,
    pub(crate) data_block_count: u16,
    /// Byte offsets relative to the start of the message (the
    /// 12-byte TCM prefix's first byte), one per block index per
    /// ICD Table XVII-A. Zero means "block absent for this
    /// radial."
    pub(crate) pointers: [u32; POINTER_COUNT],
}

impl DataHeader {
    /// Parse exactly `TOTAL_HEADER_SIZE` (72) bytes from `reader`.
    pub(crate) fn read(reader: &mut SliceReader<'_>) -> Result<Self> {
        let radar_id_bytes = reader.take_bytes(4)?;
        let mut radar_identifier = [0u8; 4];
        radar_identifier.copy_from_slice(radar_id_bytes);

        let collection_time_ms = reader.read_u32_be()?;
        let modified_julian_date = reader.read_u16_be()?;
        let azimuth_number = reader.read_u16_be()?;
        let azimuth_angle_degrees = reader.read_f32_be()?;
        let compression_indicator = reader.read_u8()?;
        let _spare = reader.read_u8()?;
        let radial_length = reader.read_u16_be()?;
        let azimuth_resolution_spacing = reader.read_u8()?;
        let radial_status = reader.read_u8()?;
        let elevation_number = reader.read_u8()?;
        let cut_sector_number = reader.read_u8()?;
        let elevation_angle_degrees = reader.read_f32_be()?;
        let radial_spot_blanking_status = reader.read_u8()?;
        let azimuth_indexing_mode = reader.read_u8()?;
        let data_block_count = reader.read_u16_be()?;

        let mut pointers = [0u32; POINTER_COUNT];
        for slot in pointers.iter_mut() {
            *slot = reader.read_u32_be()?;
        }

        Ok(Self {
            radar_identifier,
            collection_time_ms,
            modified_julian_date,
            azimuth_number,
            azimuth_angle_degrees,
            compression_indicator,
            radial_length,
            azimuth_resolution_spacing,
            radial_status,
            elevation_number,
            cut_sector_number,
            elevation_angle_degrees,
            radial_spot_blanking_status,
            azimuth_indexing_mode,
            data_block_count,
            pointers,
        })
    }

    /// Site identifier as a UTF-8 string. NEXRAD ICAOs are pure
    /// ASCII so the lossy conversion is safe in practice.
    pub(crate) fn icao_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.radar_identifier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 72-byte header buffer with KLOT's typical Build-19
    /// values. Used by every header test below.
    fn klot_sample() -> Vec<u8> {
        let mut buf = Vec::with_capacity(TOTAL_HEADER_SIZE);
        buf.extend_from_slice(b"KLOT"); // radar_identifier
        buf.extend_from_slice(&12_345_678u32.to_be_bytes()); // collection_time_ms
        buf.extend_from_slice(&20_405u16.to_be_bytes()); // modified_julian_date
        buf.extend_from_slice(&123u16.to_be_bytes()); // azimuth_number
        buf.extend_from_slice(&147.5_f32.to_be_bytes()); // azimuth_angle
        buf.push(0); // compression_indicator (uncompressed)
        buf.push(0); // spare
        buf.extend_from_slice(&9944u16.to_be_bytes()); // radial_length
        buf.push(2); // azimuth_resolution (1.0°)
        buf.push(1); // radial_status (intermediate)
        buf.push(11); // elevation_number
        buf.push(0); // cut_sector_number
        buf.extend_from_slice(&5.098_f32.to_be_bytes()); // elevation_angle
        buf.push(0); // spot_blanking
        buf.push(0); // azimuth_indexing_mode
        buf.extend_from_slice(&10u16.to_be_bytes()); // data_block_count
                                                     // 10 pointers, all populated.
        for idx in 0..POINTER_COUNT as u32 {
            // arbitrary plausible offsets
            buf.extend_from_slice(&(120 + idx * 1000).to_be_bytes());
        }
        debug_assert_eq!(buf.len(), TOTAL_HEADER_SIZE);
        buf
    }

    #[test]
    fn read_consumes_exactly_72_bytes() {
        let bytes = klot_sample();
        let mut r = SliceReader::new(&bytes);
        let _ = DataHeader::read(&mut r).unwrap();
        assert_eq!(r.position(), TOTAL_HEADER_SIZE);
    }

    #[test]
    fn round_trip_decodes_klot_fields() {
        let bytes = klot_sample();
        let mut r = SliceReader::new(&bytes);
        let h = DataHeader::read(&mut r).unwrap();
        assert_eq!(&h.radar_identifier, b"KLOT");
        assert_eq!(h.icao_str(), "KLOT");
        assert_eq!(h.collection_time_ms, 12_345_678);
        assert_eq!(h.modified_julian_date, 20_405);
        assert_eq!(h.azimuth_number, 123);
        assert!((h.azimuth_angle_degrees - 147.5).abs() < 1e-6);
        assert_eq!(h.compression_indicator, 0);
        assert_eq!(h.radial_length, 9944);
        assert_eq!(h.azimuth_resolution_spacing, 2);
        assert_eq!(h.radial_status, 1);
        assert_eq!(h.elevation_number, 11);
        assert_eq!(h.cut_sector_number, 0);
        assert!((h.elevation_angle_degrees - 5.098).abs() < 1e-6);
        assert_eq!(h.radial_spot_blanking_status, 0);
        assert_eq!(h.azimuth_indexing_mode, 0);
        assert_eq!(h.data_block_count, 10);
        // Every pointer populated to its expected value.
        for idx in 0..POINTER_COUNT {
            assert_eq!(h.pointers[idx], 120 + idx as u32 * 1000);
        }
    }

    #[test]
    fn read_errors_on_short_input() {
        let bytes = klot_sample();
        let mut r = SliceReader::new(&bytes[..32]);
        assert!(DataHeader::read(&mut r).is_err());
    }

    #[test]
    fn unused_pointers_are_preserved_as_zero() {
        let mut bytes = klot_sample();
        // Zero out the CFP pointer (index 9, byte offset 32+36=68).
        let cfp_ptr_offset = FIXED_HEADER_SIZE + PTR_CFP * POINTER_SIZE;
        bytes[cfp_ptr_offset..cfp_ptr_offset + 4].copy_from_slice(&0u32.to_be_bytes());
        let mut r = SliceReader::new(&bytes);
        let h = DataHeader::read(&mut r).unwrap();
        assert_eq!(h.pointers[PTR_CFP], 0, "CFP absent → pointer is 0");
        assert_ne!(h.pointers[PTR_REF], 0, "other pointers untouched");
    }
}

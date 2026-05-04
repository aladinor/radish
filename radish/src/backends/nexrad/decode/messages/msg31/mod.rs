//! MSG_31 (Digital Radar Data Generic Format) parser — ICD §3.2.4.17.
//!
//! Parsing strategy:
//!
//! 1. Read the 32-byte fixed data header + 10 × `u32` pointers
//!    (`header.rs`).
//! 2. For each non-zero pointer, seek the reader to that offset
//!    (which is **relative to the start of the message** — i.e. to
//!    the 12-byte TCM prefix's first byte) and decode the block at
//!    that index per ICD Table XVII-A:
//!    * indices 0..3 → VOL / ELV / RAD info blocks
//!    * indices 3..9 → REF / VEL / SW / ZDR / PHI / RHO moment blocks
//!    * index 9 → CFP block
//! 3. Pointer order in the file is _not guaranteed_ (per ICD); we
//!    sort and walk in order so the reader always advances forward.
//!
//! Layout note: every block starts with a 4-byte `DataBlockId`
//! (`b'D'` + 3-byte ASCII name). `info_blocks::DataBlockId::read`
//! consumes those 4 bytes; the per-block parsers then read their
//! payload. We do **not** trust the `DataBlockId.name`
//! programmatically — block routing is by **pointer index** per
//! ICD Table XVII-A, with the name preserved only for debugging.

pub(crate) mod cfp;
pub(crate) mod header;
pub(crate) mod info_blocks;
pub(crate) mod moment;

use crate::backends::nexrad::decode::error::{NexradDecodeError, Result};
use crate::backends::nexrad::decode::reader::SliceReader;

use cfp::CfpBlock;
#[cfg(test)]
use header::TOTAL_HEADER_SIZE;
use header::{
    DataHeader, POINTER_COUNT, PTR_CFP, PTR_ELV, PTR_PHI, PTR_RAD, PTR_REF, PTR_RHO, PTR_SW,
    PTR_VEL, PTR_VOL, PTR_ZDR,
};
use info_blocks::{DataBlockId, ElevationBlock, RadialBlock, VolumeBlock};
use moment::MomentBlock;

/// Decoded MSG_31 message (one radial). Borrowed gate slices live
/// in the `MomentBlock` / `CfpBlock` payloads so the whole struct
/// references the underlying record buffer.
#[derive(Debug)]
pub(crate) struct Msg31<'a> {
    pub(crate) header: DataHeader,
    pub(crate) volume: Option<VolumeBlock>,
    pub(crate) elevation: Option<ElevationBlock>,
    pub(crate) radial: Option<RadialBlock>,
    pub(crate) reflectivity: Option<MomentBlock<'a>>,
    pub(crate) velocity: Option<MomentBlock<'a>>,
    pub(crate) spectrum_width: Option<MomentBlock<'a>>,
    pub(crate) zdr: Option<MomentBlock<'a>>,
    pub(crate) phi: Option<MomentBlock<'a>>,
    pub(crate) rho: Option<MomentBlock<'a>>,
    pub(crate) cfp: Option<CfpBlock<'a>>,
}

/// Parse a single MSG_31 message. The reader must be positioned at
/// the first byte of the data header — i.e. immediately after the
/// 28-byte combined TCM + Table II header. `message_start_offset`
/// is the offset of the message's first byte in the input buffer
/// (the start of the TCM prefix); pointer values are resolved
/// against it.
pub(crate) fn parse<'a>(
    reader: &mut SliceReader<'a>,
    message_start_offset: usize,
) -> Result<Msg31<'a>> {
    let header_offset = reader.position();
    debug_assert!(
        header_offset >= message_start_offset,
        "reader is somewhere before the message — bad call site",
    );
    let header = DataHeader::read(reader)?;

    // Resolve pointers against the message start. Sort by ascending
    // offset so the reader can walk forward without seeking back.
    let mut indexed: Vec<(usize, u32)> = header
        .pointers
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, ptr)| *ptr != 0)
        .collect();
    indexed.sort_by_key(|(_, ptr)| *ptr);

    let mut volume: Option<VolumeBlock> = None;
    let mut elevation: Option<ElevationBlock> = None;
    let mut radial: Option<RadialBlock> = None;
    let mut reflectivity: Option<MomentBlock<'a>> = None;
    let mut velocity: Option<MomentBlock<'a>> = None;
    let mut spectrum_width: Option<MomentBlock<'a>> = None;
    let mut zdr: Option<MomentBlock<'a>> = None;
    let mut phi: Option<MomentBlock<'a>> = None;
    let mut rho: Option<MomentBlock<'a>> = None;
    let mut cfp: Option<CfpBlock<'a>> = None;

    for (idx, ptr) in indexed {
        let target = message_start_offset.checked_add(ptr as usize).ok_or(
            NexradDecodeError::InvalidPointerOffset {
                block: pointer_label(idx),
                offset: ptr,
                message_size: 0,
            },
        )?;
        // Seek forward to the block's start. Pointers must be
        // non-decreasing per the sort above; if a future ICD
        // revision shuffles them, the reader's `try_skip_to`
        // refuses to move backward.
        reader.try_skip_to(target)?;

        // Every block is preceded by a 4-byte DataBlockId. Read
        // and discard (the block-routing decision is by index, not
        // by name; the name is debug-only).
        let _id = DataBlockId::read(reader)?;
        match idx {
            PTR_VOL => volume = Some(VolumeBlock::read(reader)?),
            PTR_ELV => elevation = Some(ElevationBlock::read(reader)?),
            PTR_RAD => radial = Some(RadialBlock::read(reader)?),
            PTR_REF => reflectivity = Some(MomentBlock::read(reader)?),
            PTR_VEL => velocity = Some(MomentBlock::read(reader)?),
            PTR_SW => spectrum_width = Some(MomentBlock::read(reader)?),
            PTR_ZDR => zdr = Some(MomentBlock::read(reader)?),
            PTR_PHI => phi = Some(MomentBlock::read(reader)?),
            PTR_RHO => rho = Some(MomentBlock::read(reader)?),
            PTR_CFP => cfp = Some(CfpBlock::read(reader)?),
            _ => unreachable!("pointer index always < {POINTER_COUNT}"),
        }
    }

    // `header_offset` is consumed by the debug_assert above; once
    // Phase 7 wires the parser into the adapter, the adapter will
    // also use it to compute relative file positions for error
    // reporting.
    let _ = header_offset;

    Ok(Msg31 {
        header,
        volume,
        elevation,
        radial,
        reflectivity,
        velocity,
        spectrum_width,
        zdr,
        phi,
        rho,
        cfp,
    })
}

fn pointer_label(idx: usize) -> &'static str {
    match idx {
        PTR_VOL => "VOL",
        PTR_ELV => "ELV",
        PTR_RAD => "RAD",
        PTR_REF => "REF",
        PTR_VEL => "VEL",
        PTR_SW => "SW",
        PTR_ZDR => "ZDR",
        PTR_PHI => "PHI",
        PTR_RHO => "RHO",
        PTR_CFP => "CFP",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal MSG_31 payload (header + REF block only).
    /// `message_start_offset` here is 0 since the synthetic input
    /// has no TCM/Table-II header in front.
    fn synth_msg31_with_ref_only(num_gates: u16) -> Vec<u8> {
        let header_offset = 0usize;
        let ref_offset = TOTAL_HEADER_SIZE;
        // REF block: 4-byte DataBlockId + 24-byte descriptor + N gates
        let mut buf = Vec::new();

        // Build header (with only REF pointer set).
        buf.extend_from_slice(b"KLOT");
        buf.extend_from_slice(&0u32.to_be_bytes()); // collection_time_ms
        buf.extend_from_slice(&20_405u16.to_be_bytes()); // date
        buf.extend_from_slice(&1u16.to_be_bytes()); // azimuth_number
        buf.extend_from_slice(&0.5_f32.to_be_bytes()); // azimuth_angle
        buf.push(0); // compression
        buf.push(0); // spare
        buf.extend_from_slice(
            &((TOTAL_HEADER_SIZE + 4 + 24 + num_gates as usize) as u16).to_be_bytes(),
        ); // radial_length
        buf.push(2); // az_resolution
        buf.push(1); // radial_status
        buf.push(1); // elevation_number
        buf.push(0); // cut_sector
        buf.extend_from_slice(&0.5_f32.to_be_bytes()); // elevation_angle
        buf.push(0); // spot_blanking
        buf.push(0); // azimuth_indexing
        buf.extend_from_slice(&4u16.to_be_bytes()); // data_block_count

        // 10 pointers — only REF (index 3) is non-zero.
        for idx in 0..POINTER_COUNT {
            let ptr = if idx == PTR_REF { ref_offset as u32 } else { 0 };
            buf.extend_from_slice(&ptr.to_be_bytes());
        }
        debug_assert_eq!(buf.len(), TOTAL_HEADER_SIZE);

        // REF block at offset TOTAL_HEADER_SIZE.
        buf.extend_from_slice(b"DREF"); // DataBlockId
                                        // Descriptor (24 bytes)
        buf.extend_from_slice(&0u32.to_be_bytes()); // reserved
        buf.extend_from_slice(&num_gates.to_be_bytes());
        buf.extend_from_slice(&2_000u16.to_be_bytes()); // first gate 2.000 km
        buf.extend_from_slice(&250u16.to_be_bytes()); // interval 0.250 km
        buf.extend_from_slice(&0u16.to_be_bytes()); // tover
        buf.extend_from_slice(&0_i16.to_be_bytes()); // snr
        buf.push(0); // ctrl flags
        buf.push(8); // 8-bit gates
        buf.extend_from_slice(&2.0_f32.to_be_bytes()); // scale
        buf.extend_from_slice(&66.0_f32.to_be_bytes()); // offset
                                                        // Gate bytes — sequential 2..2+num_gates so we can verify decode.
        for n in 0..num_gates {
            buf.push((2 + n) as u8);
        }
        debug_assert_eq!(buf.len(), TOTAL_HEADER_SIZE + 4 + 24 + num_gates as usize);
        let _ = header_offset;
        buf
    }

    #[test]
    fn parse_msg31_with_ref_only_yields_no_other_blocks() {
        let bytes = synth_msg31_with_ref_only(8);
        let mut r = SliceReader::new(&bytes);
        let msg = parse(&mut r, 0).expect("parse");
        assert_eq!(&msg.header.radar_identifier, b"KLOT");
        assert!(msg.reflectivity.is_some());
        assert!(msg.velocity.is_none());
        assert!(msg.volume.is_none());
        assert!(msg.cfp.is_none());

        // 8 gates: raws 2..9 → values (raw - 66) / 2.
        let ref_block = msg.reflectivity.unwrap();
        let values: Vec<_> = ref_block.iter().collect();
        assert_eq!(values.len(), 8);
        for (i, v) in values.iter().enumerate() {
            match v {
                moment::MomentValue::Value(x) => {
                    let expected = (2.0 + i as f32 - 66.0) / 2.0;
                    assert!((x - expected).abs() < 1e-6, "gate {i}: {x} vs {expected}");
                }
                other => panic!("gate {i}: expected Value, got {other:?}"),
            }
        }
    }

    #[test]
    fn parse_handles_pointers_in_arbitrary_order() {
        // Build a header with REF at offset A, ELV at offset B,
        // where B < A. The dispatcher must sort by offset, walk
        // forward only, and still populate both fields correctly.
        let header_offset = 0usize;

        // ELV block (8 bytes payload + 4 byte id) at the lower offset.
        let elv_offset = TOTAL_HEADER_SIZE;
        // REF block (28-byte descriptor + 4 byte id + 4 gates) at the higher offset.
        let ref_offset = elv_offset + 4 + 8;

        let mut buf = Vec::new();
        // Header
        buf.extend_from_slice(b"KLOT");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&0_f32.to_be_bytes());
        buf.push(0);
        buf.push(0);
        buf.extend_from_slice(&500u16.to_be_bytes());
        buf.push(2);
        buf.push(1);
        buf.push(1);
        buf.push(0);
        buf.extend_from_slice(&0_f32.to_be_bytes());
        buf.push(0);
        buf.push(0);
        buf.extend_from_slice(&5u16.to_be_bytes());
        // Pointers: ELV gets the lower offset, REF the higher.
        // The header serializes them in pointer-index order
        // (VOL=0, ELV=1, RAD=2, REF=3, ...) so ELV at index 1 and
        // REF at index 3. Even though REF appears later in the
        // pointer array, its byte offset can be later or earlier;
        // here we make sure ELV's offset < REF's offset so the
        // sort-by-offset walk visits ELV first.
        for idx in 0..POINTER_COUNT {
            let ptr = match idx {
                PTR_ELV => elv_offset as u32,
                PTR_REF => ref_offset as u32,
                _ => 0,
            };
            buf.extend_from_slice(&ptr.to_be_bytes());
        }

        // ELV: 4-byte id + 8-byte payload (lrtup, atmos, calib).
        buf.extend_from_slice(b"DELV");
        buf.extend_from_slice(&12u16.to_be_bytes()); // lrtup
        buf.extend_from_slice(&(-15_i16).to_be_bytes());
        buf.extend_from_slice(&36.0_f32.to_be_bytes());

        // REF: 4-byte id + 24-byte descriptor + 4 gates.
        buf.extend_from_slice(b"DREF");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&4u16.to_be_bytes()); // gate count
        buf.extend_from_slice(&2_000u16.to_be_bytes());
        buf.extend_from_slice(&250u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&0_i16.to_be_bytes());
        buf.push(0);
        buf.push(8);
        buf.extend_from_slice(&2.0_f32.to_be_bytes());
        buf.extend_from_slice(&66.0_f32.to_be_bytes());
        buf.extend_from_slice(&[2, 3, 4, 5]);

        let mut r = SliceReader::new(&buf);
        let msg = parse(&mut r, header_offset).expect("parse");
        assert!(msg.elevation.is_some(), "ELV pointer was non-zero");
        assert!(msg.reflectivity.is_some(), "REF pointer was non-zero");
        let e = msg.elevation.unwrap();
        assert_eq!(e.lrtup, 12);
        assert!((e.atmospheric_attenuation_db_per_km - (-0.015)).abs() < 1e-6);
    }
}

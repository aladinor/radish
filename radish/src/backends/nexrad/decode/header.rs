//! NEXRAD `MessageHeader` (ICD 2620002AA Table II) — 16 bytes that
//! precede every message in the decompressed record stream.
//!
//! Wire layout (big-endian):
//!
//! | Offset | Bytes | Field                                          |
//! |-------:|------:|------------------------------------------------|
//! |      0 |     2 | Message size (in 16-bit halfwords)             |
//! |      2 |     1 | Channel byte (RDA/RPG/control)                 |
//! |      3 |     1 | Message type (Table II)                        |
//! |      4 |     2 | Message sequence number                        |
//! |      6 |     2 | Julian date (since Jan 1, 1970)                |
//! |      8 |     4 | Milliseconds since midnight                    |
//! |     12 |     2 | Number of segments (for fixed-frame msgs)      |
//! |     14 |     2 | Segment number (1-based, for fixed-frame msgs) |
//!
//! `bytemuck::Pod` would only help if NEXRAD were little-endian (and
//! we'd skip the BE conversions). We're not, so we read each field
//! explicitly. Same approach as the existing sigmet backend.

use super::error::Result;
use super::reader::SliceReader;

/// Size of the on-wire header. Asserted against `read` consumption
/// in tests so a future field rearrangement can't drift the loop.
pub(crate) const SIZE: usize = 16;

/// Frame width for fixed-segment messages (ICD §3.5.2). Variable-
/// length messages (e.g. MSG_31) are not bound by this — the loop
/// uses `MessageHeader::message_size_bytes()` instead.
pub(crate) const SEGMENT_FRAME_SIZE: usize = 2432;

/// Decoded message header. Fields are decoded once on parse; raw
/// halfword counts are converted to bytes via `message_size_bytes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MessageHeader {
    /// Total message size **in halfwords** as it appears on the
    /// wire. Use `message_size_bytes()` for byte arithmetic.
    pub(crate) message_size_halfwords: u16,
    pub(crate) channel: u8,
    pub(crate) message_type: MessageType,
    pub(crate) sequence_number: u16,
    pub(crate) julian_date: u16,
    pub(crate) milliseconds: u32,
    pub(crate) segment_count: u16,
    pub(crate) segment_number: u16,
}

impl MessageHeader {
    pub(crate) fn read(reader: &mut SliceReader<'_>) -> Result<Self> {
        let message_size_halfwords = reader.read_u16_be()?;
        let channel = reader.read_u8()?;
        let type_byte = reader.read_u8()?;
        let message_type = MessageType::from_u8(type_byte);
        let sequence_number = reader.read_u16_be()?;
        let julian_date = reader.read_u16_be()?;
        let milliseconds = reader.read_u32_be()?;
        let segment_count = reader.read_u16_be()?;
        let segment_number = reader.read_u16_be()?;
        Ok(Self {
            message_size_halfwords,
            channel,
            message_type,
            sequence_number,
            julian_date,
            milliseconds,
            segment_count,
            segment_number,
        })
    }

    /// Total message size in bytes (`halfwords * 2`).
    pub(crate) fn message_size_bytes(&self) -> usize {
        usize::from(self.message_size_halfwords) * 2
    }

    /// Whether this message uses the 2432-byte fixed-frame layout
    /// (per ICD §3.5.2). MSG_31 is variable-length; the explicitly
    /// known fixed-frame types (MSG_1, MSG_2, MSG_5, MSG_15, MSG_18)
    /// return `true`. `Skip(_)` returns `false` so the loop walks
    /// the declared `message_size_bytes` rather than assuming a
    /// 2432-byte frame for codes we don't understand.
    pub(crate) fn segmented(&self) -> bool {
        matches!(
            self.message_type,
            MessageType::DigitalRadarDataLegacy
                | MessageType::RdaStatusData
                | MessageType::VolumeCoveragePattern
                | MessageType::ClutterFilterMap
                | MessageType::RdaAdaptationData
        )
    }
}

/// Message type codes per ICD 2620002AA Table II. Variants are
/// limited to the ones radish actually consumes; everything else
/// folds into `Skip(u8)` so the loop walks the bytes without
/// pretending we understand them.
///
/// `Skip(0)` covers the zero-padding type-byte that some files
/// emit between records. Truly unknown codes (i.e. anything outside
/// ICD Table II) also fold into `Skip` for forward compatibility —
/// the alternative is to fail loudly on every spec revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageType {
    /// Type 1 — legacy radial data (pre-2008). Decoder support is
    /// deferred to plan 0004; the loop walks the bytes.
    DigitalRadarDataLegacy,
    /// Type 2 — RDA Status data.
    RdaStatusData,
    /// Type 5 — Volume Coverage Pattern.
    VolumeCoveragePattern,
    /// Type 15 — Clutter Filter Map (segmented; not consumed by
    /// radish today but emitted by some files).
    ClutterFilterMap,
    /// Type 18 — RDA Adaptation Data.
    RdaAdaptationData,
    /// Type 31 — Generic format radial data (modern, post-2008).
    /// **The variable-length type that Phase 3 will parse.**
    DigitalRadarDataGenericFormat,
    /// Anything else: zero-padding, control records, types we
    /// haven't implemented. The loop walks past these via the
    /// declared message size.
    Skip(u8),
}

impl MessageType {
    pub(crate) fn from_u8(code: u8) -> Self {
        match code {
            1 => Self::DigitalRadarDataLegacy,
            2 => Self::RdaStatusData,
            5 => Self::VolumeCoveragePattern,
            15 => Self::ClutterFilterMap,
            18 => Self::RdaAdaptationData,
            31 => Self::DigitalRadarDataGenericFormat,
            other => Self::Skip(other),
        }
    }
}

impl std::fmt::Display for MessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DigitalRadarDataLegacy => write!(f, "MSG_1 (legacy radial)"),
            Self::RdaStatusData => write!(f, "MSG_2 (RDA status)"),
            Self::VolumeCoveragePattern => write!(f, "MSG_5 (VCP)"),
            Self::ClutterFilterMap => write!(f, "MSG_15 (clutter filter map)"),
            Self::RdaAdaptationData => write!(f, "MSG_18 (RDA adaptation)"),
            Self::DigitalRadarDataGenericFormat => write!(f, "MSG_31 (generic radial)"),
            Self::Skip(code) => write!(f, "MSG_{code} (skipped)"),
        }
    }
}

#[allow(dead_code)] // field 0 read indirectly via `header_size_compile_time_check`.
struct AssertHeaderSize([(); SIZE - 16]);

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct 16 bytes that decode to a header with the given
    /// type byte, message size (halfwords), and segment_count.
    fn synth(type_byte: u8, halfwords: u16, segment_count: u16) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0..2].copy_from_slice(&halfwords.to_be_bytes());
        bytes[2] = 0; // channel
        bytes[3] = type_byte;
        bytes[12..14].copy_from_slice(&segment_count.to_be_bytes());
        bytes[14..16].copy_from_slice(&1u16.to_be_bytes()); // segment_number
        bytes
    }

    #[test]
    fn message_size_bytes_doubles_halfwords() {
        let bytes = synth(31, 100, 1);
        let mut r = SliceReader::new(&bytes);
        let h = MessageHeader::read(&mut r).unwrap();
        assert_eq!(h.message_size_bytes(), 200);
    }

    #[test]
    fn message_type_dispatch_matches_icd_table_ii() {
        let cases = [
            (1u8, MessageType::DigitalRadarDataLegacy),
            (2, MessageType::RdaStatusData),
            (5, MessageType::VolumeCoveragePattern),
            (15, MessageType::ClutterFilterMap),
            (18, MessageType::RdaAdaptationData),
            (31, MessageType::DigitalRadarDataGenericFormat),
            (0, MessageType::Skip(0)),
            (3, MessageType::Skip(3)),
            (255, MessageType::Skip(255)),
        ];
        for (code, expected) in cases {
            assert_eq!(MessageType::from_u8(code), expected, "code {code}");
        }
    }

    #[test]
    fn segmented_returns_false_only_for_msg31() {
        let make = |t: u8| {
            let bytes = synth(t, 0, 1);
            let mut r = SliceReader::new(&bytes);
            MessageHeader::read(&mut r).unwrap().segmented()
        };
        assert!(!make(31), "MSG_31 is variable-length, not segmented");
        assert!(make(2));
        assert!(make(5));
        assert!(make(15));
    }

    #[test]
    fn read_consumes_exactly_16_bytes() {
        let bytes = synth(31, 100, 1);
        let mut r = SliceReader::new(&bytes);
        let _ = MessageHeader::read(&mut r).unwrap();
        assert_eq!(r.position(), SIZE);
    }

    #[test]
    fn read_errors_on_short_input() {
        let bytes = synth(31, 100, 1);
        let mut r = SliceReader::new(&bytes[..8]);
        assert!(MessageHeader::read(&mut r).is_err());
    }
}

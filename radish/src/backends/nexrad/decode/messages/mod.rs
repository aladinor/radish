//! NEXRAD message-stream iterator with explicit boundary resync.
//!
//! This module is the load-bearing fix for the upstream
//! `nexrad-decode 1.0.0-rc.3` phantom-radial bug
//! (`/tmp/radish-phantom-radials-bug.md`). The upstream loop trusts
//! every variable-length parser to leave the byte cursor at exactly
//! `offset + message_size_bytes()`. Our loop calls
//! `reader.try_skip_to(target)` after every parse, idempotent on
//! exact-match, snap-forward on under-read, error on past-buffer.
//!
//! Phase 2 scope: the loop walks every message and emits a typed
//! header plus a borrowed-payload slice. **The actual MSG_31 / MSG_2
//! / MSG_5 parsers are stubbed** (Phase 3+4 wires them up). This is
//! deliberate — the boundary fix lives in the loop, not in any
//! particular parser, and we want the loop tested in isolation
//! before parser bugs can confound the picture.

use super::error::{NexradDecodeError, Result};
use super::header::{
    MessageHeader, LOGICAL_HEADER_SIZE, SEGMENT_FRAME_SIZE, SIZE as HEADER_SIZE, TCM_PREFIX_SIZE,
};
use super::reader::SliceReader;

/// One decoded message: its header plus a payload slice. Payload
/// boundaries are determined by `header.message_size_bytes()` (or
/// the segment-frame width for fixed-frame messages); the loop
/// guarantees the slice lands exactly on the boundary the header
/// declared.
#[derive(Debug)]
pub(crate) struct Message<'a> {
    pub(crate) header: MessageHeader,
    pub(crate) payload: MessagePayload<'a>,
    /// Byte offset of the message in the input stream (start of
    /// the header).
    pub(crate) offset: usize,
    /// Total span of the message in bytes (header + payload),
    /// equal to `target - offset` for variable-length and equal
    /// to `SEGMENT_FRAME_SIZE` for non-final segmented messages.
    pub(crate) size: usize,
}

/// Payload variants carried alongside the header.
///
/// Phase 2 emits everything as `Raw` (the bytes between the header
/// and the next message boundary) so the loop's resync semantics can
/// be exercised without depending on any per-message parser. Phase
/// 3+4 will introduce typed variants (`Msg31`, `Msg2`, `Msg5`) and
/// keep `Raw` as a fallback for `MessageType::Skip(_)`.
#[derive(Debug)]
#[allow(dead_code)] // SegmentedFragment / Reassembled used by tests + later phases.
pub(crate) enum MessagePayload<'a> {
    /// Raw bytes from the message header to the message boundary.
    Raw(&'a [u8]),
    /// One frame of a fixed-segment message that hasn't been
    /// reassembled yet. The accumulator hands it back to the loop
    /// when it isn't ready to emit the assembled message.
    SegmentedFragment {
        segment_number: u16,
        segment_count: u16,
        bytes: &'a [u8],
    },
    /// Final reassembled payload for a multi-segment message —
    /// every fragment's bytes concatenated in segment order.
    Reassembled(Vec<u8>),
}

/// Walk an in-memory NEXRAD message stream end-to-end. Allocates
/// once for the result vector; payloads are borrowed from `bytes`
/// where possible (Reassembled is the one allocation per multi-
/// segment message).
pub(crate) fn decode_messages(bytes: &[u8]) -> Result<Vec<Message<'_>>> {
    let mut reader = SliceReader::new(bytes);
    let mut messages = Vec::new();
    let mut accumulator = SegmentAccumulator::new();

    while reader.remaining().len() >= HEADER_SIZE {
        let offset = reader.position();
        let header = MessageHeader::read(&mut reader)?;
        // Per-message wire stride for variable-length messages is
        // `TCM_PREFIX_SIZE + message_size_bytes` — the 12-byte TCM
        // prefix is in addition to the size declared by the Table II
        // header (ICD §3.2.4.1 footnote). For fixed-frame messages
        // the stride is `SEGMENT_FRAME_SIZE` regardless of the
        // declared message_size.
        let target = if header.segmented() {
            offset.saturating_add(SEGMENT_FRAME_SIZE)
        } else {
            offset.saturating_add(TCM_PREFIX_SIZE + header.message_size_bytes())
        };

        if !header.segmented() {
            // Variable-length path. Phase 3 will replace
            // `take_payload_bytes` with the typed MSG_31 parser and
            // keep the resync behaviour identical.
            let payload_bytes = take_payload_bytes(&mut reader, &header, offset, bytes.len())?;
            // ─── THE FIX ───
            // Always resync to the declared boundary, regardless of
            // whether the parser consumed exactly that many bytes.
            // Idempotent if we landed exactly on it; snaps forward
            // on under-read; errors `UnexpectedEof` if the declared
            // size exceeds the buffer length.
            reader.try_skip_to(target)?;
            messages.push(Message {
                header,
                payload: MessagePayload::Raw(payload_bytes),
                offset,
                size: target - offset,
            });
            continue;
        }

        // Fixed-segment path. The declared `message_size` is the
        // logical Table II message size (16-byte header + payload);
        // the on-wire frame is `SEGMENT_FRAME_SIZE = 2432` bytes
        // including the TCM prefix and trailing padding.
        let logical_size = header.message_size_bytes();
        if logical_size < LOGICAL_HEADER_SIZE {
            // Zero/too-small declared size: a real LDM record has
            // trailing zero-padded 2432-byte frames after the last
            // semantically-meaningful message (the bzip2-block
            // alignment). Walk past them without yielding a Message
            // — they're not malformed, just empty.
            reader.try_skip_to(target)?;
            continue;
        }
        let payload_size = logical_size - LOGICAL_HEADER_SIZE;
        let payload_bytes = reader.take_bytes(payload_size)?;
        // After header (28 bytes) + payload, advance to the next
        // SEGMENT_FRAME_SIZE boundary.
        reader.try_skip_to(target)?;

        // Single-segment fixed-frame messages emit immediately;
        // multi-segment ones go through the accumulator.
        if header.segment_count <= 1 {
            messages.push(Message {
                header,
                payload: MessagePayload::Raw(payload_bytes),
                offset,
                size: SEGMENT_FRAME_SIZE,
            });
            continue;
        }

        if let Some(reassembled) = accumulator.feed(header, payload_bytes)? {
            messages.push(Message {
                header,
                payload: MessagePayload::Reassembled(reassembled),
                offset,
                size: SEGMENT_FRAME_SIZE,
            });
        }
    }

    Ok(messages)
}

/// Pull the variable-length payload out as a raw slice. Reader is
/// expected to be at `offset + HEADER_SIZE` (i.e. just past the
/// 28-byte combined TCM + Table II header). Returns
/// `message_size_bytes - LOGICAL_HEADER_SIZE` payload bytes — the
/// portion of the message after the Table II header.
///
/// Phase 3 replaces this with a typed dispatch over
/// `header.message_type()`.
fn take_payload_bytes<'a>(
    reader: &mut SliceReader<'a>,
    header: &MessageHeader,
    offset: usize,
    buf_len: usize,
) -> Result<&'a [u8]> {
    let logical_size = header.message_size_bytes();
    if logical_size < LOGICAL_HEADER_SIZE {
        return Err(NexradDecodeError::MalformedHeader {
            offset,
            reason: "declared message_size smaller than the 16-byte Table II header",
        });
    }
    let payload_size = logical_size - LOGICAL_HEADER_SIZE;
    // Total wire span = TCM prefix + logical message.
    let target = offset.saturating_add(TCM_PREFIX_SIZE + logical_size);
    if target > buf_len {
        return Err(NexradDecodeError::UnexpectedEof {
            offset,
            needed: TCM_PREFIX_SIZE + logical_size,
            available: buf_len.saturating_sub(offset),
        });
    }
    reader.take_bytes(payload_size)
}

/// Reassembles multi-segment fixed-frame messages (e.g. MSG_5 spans
/// 2-3 segments on most VCPs; MSG_15 spans many).
///
/// State machine:
///
/// * **Idle** — no message in progress. First fragment must be
///   `segment_number == 1`; otherwise we log a warning (via the
///   error path on `feed`) and stay idle.
/// * **In progress** — accumulating fragments in segment order. On
///   the final segment (`segment_number == segment_count`) yields
///   the assembled payload and returns to Idle.
struct SegmentAccumulator {
    expected_count: u16,
    next_segment: u16,
    sequence_number: u16,
    payloads: Vec<u8>,
}

impl SegmentAccumulator {
    fn new() -> Self {
        Self {
            expected_count: 0,
            next_segment: 0,
            sequence_number: 0,
            payloads: Vec::new(),
        }
    }

    fn is_active(&self) -> bool {
        self.expected_count > 0
    }

    /// Feed one segment. Returns `Some(reassembled_payload)` when
    /// the accumulator just finished a multi-segment message; `None`
    /// while still mid-stream.
    fn feed(&mut self, header: MessageHeader, payload: &[u8]) -> Result<Option<Vec<u8>>> {
        if !self.is_active() {
            // First segment opens the accumulation.
            if header.segment_number != 1 {
                // Fragment arrived without segment 1 — drop it. We
                // could surface a warning, but radish's downstream
                // doesn't care about MSG_15/18 reassembly today.
                return Ok(None);
            }
            self.expected_count = header.segment_count;
            self.next_segment = 1;
            self.sequence_number = header.sequence_number;
            self.payloads.clear();
        }

        if header.sequence_number != self.sequence_number
            || header.segment_count != self.expected_count
            || header.segment_number != self.next_segment
        {
            // Out-of-order or mismatched segment — abandon the
            // current accumulation and start over from the next
            // segment_number==1.
            self.reset();
            return Ok(None);
        }

        self.payloads.extend_from_slice(payload);
        self.next_segment += 1;

        if header.segment_number == header.segment_count {
            let out = std::mem::take(&mut self.payloads);
            self.reset();
            return Ok(Some(out));
        }
        Ok(None)
    }

    fn reset(&mut self) {
        self.expected_count = 0;
        self.next_segment = 0;
        self.sequence_number = 0;
        self.payloads.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::super::header::{
        MessageType, LOGICAL_HEADER_SIZE, TCM_PREFIX_SIZE, VARIABLE_LENGTH_MESSAGE_SIZE,
    };
    use super::*;
    use rstest::rstest;

    /// Build a single variable-length (Type 31) message:
    ///
    /// * 12-byte zero-filled TCM prefix (ICD §3.1.3).
    /// * 16-byte Table II header with `message_size_halfwords =
    ///   declared_halfwords` and `message_type = 31`.
    /// * `(declared_halfwords * 2 - 16)` bytes of zero-filled payload.
    ///
    /// On-wire span = `12 + declared_halfwords * 2`.
    fn synthesize_msg31(declared_halfwords: u16) -> Vec<u8> {
        let logical_size = (declared_halfwords as usize) * 2;
        assert!(
            logical_size >= LOGICAL_HEADER_SIZE,
            "logical_size must accommodate the 16-byte Table II header"
        );
        let total = TCM_PREFIX_SIZE + logical_size;
        let mut buf = vec![0u8; total];
        // bytes 0..12 are the zero-filled TCM prefix.
        buf[12..14].copy_from_slice(&declared_halfwords.to_be_bytes());
        buf[15] = 31; // message_type
        buf[24..26].copy_from_slice(&1u16.to_be_bytes()); // segment_count
        buf[26..28].copy_from_slice(&1u16.to_be_bytes()); // segment_number
        buf
    }

    fn synthesize_two_msg31(decl1: u16, decl2: u16) -> Vec<u8> {
        let mut buf = synthesize_msg31(decl1);
        buf.extend_from_slice(&synthesize_msg31(decl2));
        buf
    }

    /// **The load-bearing test.** Pin variable-length boundary resync
    /// across pairs of MSG_31 messages with varying declared sizes.
    /// Each MSG_31's wire span = `12 + halfwords * 2`, so the second
    /// message's offset must be `12 + first_halfwords * 2`.
    #[rstest]
    #[case::equal_sizes(32, 32, TCM_PREFIX_SIZE + 64)]
    #[case::large_then_small(40, 32, TCM_PREFIX_SIZE + 80)]
    #[case::small_then_large(32, 40, TCM_PREFIX_SIZE + 64)]
    fn boundary_resync_pins_second_message_offset(
        #[case] first_halfwords: u16,
        #[case] second_halfwords: u16,
        #[case] expected_second_offset: usize,
    ) {
        let bytes = synthesize_two_msg31(first_halfwords, second_halfwords);
        let messages = decode_messages(&bytes).expect("decode");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].offset, 0);
        assert_eq!(messages[1].offset, expected_second_offset);
        assert_eq!(
            messages[0].size,
            TCM_PREFIX_SIZE + (first_halfwords as usize) * 2
        );
        assert_eq!(
            messages[1].size,
            TCM_PREFIX_SIZE + (second_halfwords as usize) * 2
        );
    }

    #[test]
    fn boundary_resync_target_past_buffer_returns_unexpected_eof() {
        // 100 halfwords → declared logical size 200 bytes, wire span
        // 12 + 200 = 212. Truncate to 50 to provoke EOF.
        let bytes = synthesize_msg31(100);
        let truncated = &bytes[..50];
        let err = decode_messages(truncated).expect_err("should fail");
        assert!(
            matches!(err, NexradDecodeError::UnexpectedEof { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn empty_input_yields_empty_message_list() {
        assert!(decode_messages(&[]).unwrap().is_empty());
    }

    #[test]
    fn input_smaller_than_header_terminates_cleanly() {
        // 8 bytes < HEADER_SIZE (28) → loop never enters body.
        let bytes = vec![0u8; 8];
        assert!(decode_messages(&bytes).unwrap().is_empty());
    }

    /// `Skip(_)` with the variable-length sentinel set walks via the
    /// extended size encoding (`segment_count` MSB | `segment_number`
    /// LSB). The next message must read its header from the
    /// boundary the loop resynced to.
    #[test]
    fn skip_with_sentinel_advances_via_extended_size() {
        // First message: type 99 (Skip), segment_size = 0xFFFF, then
        // segment_count/segment_number encode 64 bytes wire-size of
        // the message (= 12 TCM prefix + 16 logical header + 36
        // payload? no — the extended size is the LOGICAL size; total
        // wire = 12 + 64 = 76).
        let logical_size: u32 = 64;
        let segment_count = ((logical_size >> 16) & 0xFFFF) as u16;
        let segment_number = (logical_size & 0xFFFF) as u16;
        let total_wire = TCM_PREFIX_SIZE + logical_size as usize;

        let mut buf = vec![0u8; total_wire];
        buf[12..14].copy_from_slice(&VARIABLE_LENGTH_MESSAGE_SIZE.to_be_bytes());
        buf[15] = 99; // unknown type → Skip(99)
        buf[24..26].copy_from_slice(&segment_count.to_be_bytes());
        buf[26..28].copy_from_slice(&segment_number.to_be_bytes());

        // Second message: a vanilla MSG_31, 32 halfwords.
        buf.extend_from_slice(&synthesize_msg31(32));

        let messages = decode_messages(&buf).expect("decode");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].header.message_type, MessageType::Skip(99));
        assert_eq!(messages[0].size, total_wire);
        assert_eq!(messages[1].offset, total_wire);
        assert_eq!(
            messages[1].header.message_type,
            MessageType::DigitalRadarDataGenericFormat
        );
    }

    /// Build one fixed-segment frame with the given type byte,
    /// segment_size halfwords, segment_count, and segment_number.
    /// Marks the first payload byte with `marker` so reassembly
    /// tests can detect ordering.
    fn synthesize_fixed_frame(
        type_byte: u8,
        seq_number: u16,
        size_halfwords: u16,
        segment_count: u16,
        segment_number: u16,
        marker: u8,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; SEGMENT_FRAME_SIZE];
        // bytes 0..12 are zero TCM prefix.
        buf[12..14].copy_from_slice(&size_halfwords.to_be_bytes());
        buf[15] = type_byte;
        buf[16..18].copy_from_slice(&seq_number.to_be_bytes());
        buf[24..26].copy_from_slice(&segment_count.to_be_bytes());
        buf[26..28].copy_from_slice(&segment_number.to_be_bytes());
        // Marker at the first payload byte (right after the 28-byte
        // combined header).
        buf[HEADER_SIZE] = marker;
        buf
    }

    /// Reassembly sanity check: build a 2-segment MSG_5 (one of the
    /// real fixed-frame cases — VCP definitions span 2-3 frames in
    /// practice) and verify the loop emits one reassembled message.
    #[test]
    fn segmented_message_reassembles_in_order() {
        // Each segment: 16-byte logical header + 2400-byte payload
        // = 2416 byte logical size. Total fixed-frame width = 2432.
        let segment_size_hw: u16 = ((LOGICAL_HEADER_SIZE + 2400) / 2) as u16;
        let mut buf = synthesize_fixed_frame(5, 77, segment_size_hw, 2, 1, 0xAA);
        buf.extend_from_slice(&synthesize_fixed_frame(5, 77, segment_size_hw, 2, 2, 0xBB));

        let messages = decode_messages(&buf).expect("decode");
        // The accumulator suppresses segment 1 (partial) and emits
        // segment 2 with the reassembled payload.
        assert_eq!(messages.len(), 1, "got {} messages", messages.len());
        let MessagePayload::Reassembled(p) = &messages[0].payload else {
            panic!("expected Reassembled, got {:?}", messages[0].payload);
        };
        assert_eq!(p.len(), 2400 + 2400, "two segments × 2400 byte payloads");
        assert_eq!(p[0], 0xAA, "segment 1 first byte preserved");
        assert_eq!(p[2400], 0xBB, "segment 2 starts at byte 2400 of reassembly");
    }

    /// Real LDM records have trailing zero-padded 2432-byte frames
    /// after the last semantically-meaningful message (bzip2
    /// block-alignment artifact). The loop must walk past them
    /// without yielding a Message and without erroring on the
    /// "declared message_size smaller than the 16-byte Table II
    /// header" check.
    #[test]
    fn trailing_zero_padded_frames_are_walked_silently() {
        // One real MSG_5 segment, then 3 frames of all zeros.
        let segment_size_hw: u16 = ((LOGICAL_HEADER_SIZE + 100) / 2) as u16;
        let mut buf = synthesize_fixed_frame(5, 1, segment_size_hw, 1, 1, 0xAA);
        buf.extend(vec![0u8; SEGMENT_FRAME_SIZE * 3]);

        let messages = decode_messages(&buf).expect("decode");
        assert_eq!(
            messages.len(),
            1,
            "only the real MSG_5 should be yielded; zero-padded \
             trailers are walked silently"
        );
        assert_eq!(
            messages[0].header.message_type,
            MessageType::VolumeCoveragePattern
        );
    }

    #[test]
    fn proptest_decode_messages_never_panics_on_random_input() {
        // Property: for any byte input, decode_messages returns a
        // Result and doesn't panic. Binary parsers are notorious for
        // crashing on malformed input — pin the no-panic invariant.
        use proptest::prelude::*;
        proptest!(|(bytes in prop::collection::vec(any::<u8>(), 0..8192))| {
            let _ = decode_messages(&bytes);
        });
    }
}

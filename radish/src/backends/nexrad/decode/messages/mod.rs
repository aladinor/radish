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
use super::header::{MessageHeader, SEGMENT_FRAME_SIZE, SIZE as HEADER_SIZE};
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
        let target = offset.saturating_add(header.message_size_bytes());

        if !header.segmented() {
            // Variable-length path (currently MSG_31 only). Phase 3
            // will replace `take_payload_bytes` with the typed parser
            // and keep the resync behaviour identical.
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

        // Fixed-segment path (every other ICD type). Each segment is
        // exactly SEGMENT_FRAME_SIZE bytes; the declared
        // `message_size` is the *logical* assembled size, which can
        // be smaller than the frame's payload area.
        let payload_size = header.message_size_bytes().saturating_sub(HEADER_SIZE);
        let payload_bytes = reader.take_bytes(payload_size)?;
        let consumed = HEADER_SIZE + payload_size;
        if consumed < SEGMENT_FRAME_SIZE {
            reader.advance(SEGMENT_FRAME_SIZE - consumed)?;
        }

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

/// Pull `header.message_size_bytes() - HEADER_SIZE` bytes out as a
/// raw payload slice. Phase 3 replaces this with a typed dispatch
/// over `header.message_type()`.
fn take_payload_bytes<'a>(
    reader: &mut SliceReader<'a>,
    header: &MessageHeader,
    offset: usize,
    buf_len: usize,
) -> Result<&'a [u8]> {
    let size = header.message_size_bytes();
    if size < HEADER_SIZE {
        return Err(NexradDecodeError::MalformedHeader {
            offset,
            reason: "declared message_size smaller than 16-byte header",
        });
    }
    let payload_size = size - HEADER_SIZE;
    let target = offset.saturating_add(size);
    if target > buf_len {
        return Err(NexradDecodeError::UnexpectedEof {
            offset,
            needed: size,
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
    use super::super::header::MessageType;
    use super::*;
    use rstest::rstest;

    /// Build a single MSG_31 (variable-length) message with the
    /// declared `message_size_halfwords` and a payload of length
    /// `decoded_payload_size` bytes (must be ≤ declared payload).
    /// Pads the rest of the declared size with zero bytes.
    fn synthesize_msg31(declared_halfwords: u16) -> Vec<u8> {
        let total_bytes = (declared_halfwords as usize) * 2;
        assert!(total_bytes >= HEADER_SIZE);
        let mut buf = vec![0u8; total_bytes];
        buf[0..2].copy_from_slice(&declared_halfwords.to_be_bytes());
        buf[3] = 31; // message_type
        buf[12..14].copy_from_slice(&1u16.to_be_bytes()); // segment_count
        buf[14..16].copy_from_slice(&1u16.to_be_bytes()); // segment_number
        buf
    }

    /// Two back-to-back MSG_31s. Caller picks each one's declared
    /// size; the bytes are zero-padded. Cursor-position drift on
    /// the first decoder shouldn't affect the second one's read.
    fn synthesize_two_msg31(decl1_halfwords: u16, decl2_halfwords: u16) -> Vec<u8> {
        let mut buf = synthesize_msg31(decl1_halfwords);
        buf.extend_from_slice(&synthesize_msg31(decl2_halfwords));
        buf
    }

    /// **The load-bearing test.** Pin all four reader-position
    /// outcomes after a variable-length parse. `take_payload_bytes`
    /// always reads exactly the declared payload bytes today, so
    /// these cases are pinned via the synth helpers — but if a
    /// future Phase 3 parser does anything different, the
    /// `try_skip_to(target)` call must still leave the cursor on
    /// the next message boundary.
    #[rstest]
    // First MSG_31 declares 32 halfwords (= 64 bytes), second declares 32 too.
    // Loop iteration 2 must read header from offset 64.
    #[case::exact_match(32, 32, 64)]
    // First declares larger payload than second → second offset shifts.
    #[case::large_then_small(40, 32, 80)]
    #[case::small_then_large(32, 40, 64)]
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
        assert_eq!(messages[0].size, first_halfwords as usize * 2);
        assert_eq!(messages[1].size, second_halfwords as usize * 2);
    }

    #[test]
    fn boundary_resync_target_past_buffer_returns_unexpected_eof() {
        // Declared size says 100 halfwords (200 bytes) but we only
        // have 50 bytes of input.
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
        // 8 bytes < 16-byte header → loop condition fails on entry.
        let bytes = vec![0u8; 8];
        assert!(decode_messages(&bytes).unwrap().is_empty());
    }

    #[test]
    fn skip_message_types_advance_to_next_boundary() {
        // Type 0 is `MessageType::Skip(0)`. The loop should walk
        // past it and read the second header from the next boundary.
        let mut bytes = synthesize_msg31(32);
        bytes[3] = 0; // change first message's type to "skip"
        bytes.extend_from_slice(&synthesize_msg31(32));
        let messages = decode_messages(&bytes).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].header.message_type, MessageType::Skip(0));
        assert_eq!(
            messages[1].header.message_type,
            MessageType::DigitalRadarDataGenericFormat
        );
    }

    /// Reassembly sanity check: build a 2-segment MSG_5 and verify
    /// the loop emits one logical message with payloads concatenated.
    #[test]
    fn segmented_message_reassembles_in_order() {
        // Segment 1: header + 2416 bytes payload (zero-padded to
        // SEGMENT_FRAME_SIZE = 2432).
        // Segment 2: header + 100 bytes payload (zero-padded same).
        let mut buf = vec![0u8; SEGMENT_FRAME_SIZE];
        // Segment 1
        let total_size_halfwords: u16 = (HEADER_SIZE as u16 + 2416) / 2;
        buf[0..2].copy_from_slice(&total_size_halfwords.to_be_bytes());
        buf[3] = 5; // MSG_5
        buf[4..6].copy_from_slice(&77u16.to_be_bytes()); // sequence_number
        buf[12..14].copy_from_slice(&2u16.to_be_bytes()); // segment_count
        buf[14..16].copy_from_slice(&1u16.to_be_bytes()); // segment_number
                                                          // Mark first byte of segment 1's payload so we can detect order.
        buf[HEADER_SIZE] = 0xAA;

        // Segment 2
        buf.extend(vec![0u8; SEGMENT_FRAME_SIZE]);
        let off2 = SEGMENT_FRAME_SIZE;
        let seg2_size_halfwords: u16 = (HEADER_SIZE as u16 + 100) / 2;
        buf[off2..off2 + 2].copy_from_slice(&seg2_size_halfwords.to_be_bytes());
        buf[off2 + 3] = 5;
        buf[off2 + 4..off2 + 6].copy_from_slice(&77u16.to_be_bytes());
        buf[off2 + 12..off2 + 14].copy_from_slice(&2u16.to_be_bytes());
        buf[off2 + 14..off2 + 16].copy_from_slice(&2u16.to_be_bytes());
        buf[off2 + HEADER_SIZE] = 0xBB;

        let messages = decode_messages(&buf).expect("decode");
        // The accumulator suppresses segment 1 (partial) and emits
        // segment 2 with the reassembled payload.
        assert_eq!(messages.len(), 1, "got {} messages", messages.len());
        let MessagePayload::Reassembled(p) = &messages[0].payload else {
            panic!("expected Reassembled, got {:?}", messages[0].payload);
        };
        assert_eq!(p.len(), 2416 + 100);
        assert_eq!(p[0], 0xAA, "segment 1 first byte preserved");
        assert_eq!(p[2416], 0xBB, "segment 2 starts at byte 2416");
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

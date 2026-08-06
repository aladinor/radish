//! bzip2 decompression (incremental, hard-capped) and symbology-block ->
//! packet-16 (digital radial data array) -> (azimuths, codes) decode.
//! Mirrors `nexrad_level3.py:468-638`.

use ndarray::Array2;

use super::bytes::{read_i16_be, read_u16_be};
use super::error::{Level3DecodeError, Result};

/// The largest real product decompresses to ~1.3 MB. This bounds a
/// decompression bomb, not a guess at product size: a ~1.3 KB crafted
/// payload can expand to gigabytes otherwise, in a context (a browser tab)
/// where the attacker controls the input — see `docs/NEXRAD_LEVEL3_WASM.md`
/// §4.7. One-shot `read_to_end`-style decompression would reopen exactly
/// this hole; `nexrad::decode::record::decompress`'s pattern (fine for
/// Level 2's institutionally-trusted S3 objects) must NOT be reused here.
pub(crate) const MAX_DECOMPRESSED_BYTES: usize = 8 << 20;

/// Digital Radial Data Array Packet — raw per-gate bytes.
const RADIAL_PACKET_CODE: i16 = 16;

/// Run-Length-Encoded radial packet (`0xAF1F` as signed big-endian `i16`) —
/// legacy 8/16-level products. `0xAF1F` doesn't fit `u16`'s complement
/// story the way the ICD prints it; reading the packet-code halfword as
/// `i16` (matching every other signed halfword in this decoder) gives
/// `-20705`, which is what xradar #392 compares against too.
const AF1F_PACKET_CODE: i16 = -20705;

/// WMO/NOAAPORT end-of-message trailer: the header separator plus ETX.
/// Present on the dual-pol products in the Unidata bucket, absent on
/// reflectivity.
const WMO_TERMINATOR: &[u8] = b"\r\r\n\x03";

/// Hard cap on `n_radials * n_bins` (the codes array's cell count) —
/// checked BEFORE `Array2::zeros` allocates, in `u64` so it can't wrap on
/// a 32-bit `usize` target (`wasm32-unknown-unknown`, this crate's actual
/// deployment target for this backend). The largest real super-resolution
/// product is 720 x 1840 = 1,324,800 cells; this gives >3x headroom for
/// products this backend doesn't cover yet while still keeping a crafted
/// `n_radials = n_bins = 65535` (up to ~4.3 billion cells) far out of
/// reach. A `u16 x u16` product can reach `u32::MAX` on its own — well
/// past what a 32-bit `usize` multiply survives without wrapping — so
/// this check must run in a wider type, not `usize`.
pub(crate) const MAX_GATE_COUNT: u64 = 4_000_000;

/// Decompress the payload after the PDB — either raw (no `BZ` magic) or a
/// bzip2 stream — capped at [`MAX_DECOMPRESSED_BYTES`] either way.
pub(crate) fn decompress_payload(payload: &[u8]) -> Result<Vec<u8>> {
    if payload.len() >= 2 && &payload[..2] == b"BZ" {
        decompress_bzip2_capped(payload, MAX_DECOMPRESSED_BYTES)
    } else if payload.len() > MAX_DECOMPRESSED_BYTES {
        // The cap otherwise only applies on the compressed branch, so an
        // uncompressed payload would be unbounded — not an amplification
        // bomb (the attacker has to send every byte), but nothing else
        // bounds the input size either (`nexrad_level3.py:469-476`).
        Err(Level3DecodeError::UncompressedPayloadTooLarge {
            len: payload.len(),
            cap: MAX_DECOMPRESSED_BYTES,
        })
    } else {
        Ok(payload.to_vec())
    }
}

/// Incremental bzip2 decompression with a hard output cap, checked
/// *before* it's exceeded — not a one-shot decompress-then-check.
///
/// Decompresses into a FIXED-SIZE scratch slice (`Decompress::decompress`,
/// not `decompress_vec`) rather than growing `output` via
/// `Vec::reserve`/`spare_capacity_mut`: `decompress_vec`'s own doc comment
/// says it "will fill the space after its current length up to its
/// capacity" — i.e. it isn't bounded by how much headroom this function
/// *asked* for, only by whatever `Vec::reserve`'s amortized growth
/// actually allocated, which can overshoot the requested amount by up to
/// ~2x. Decompressing into `&mut scratch[..want]` instead bounds each
/// call's output to *exactly* `want` bytes, so `output.len()` can only
/// ever grow by `want` (never more) per iteration, regardless of what
/// `Vec`'s allocator does internally.
fn decompress_bzip2_capped(payload: &[u8], cap: usize) -> Result<Vec<u8>> {
    use bzip2::{Decompress, Status};

    let mut decompressor = Decompress::new(false);
    let mut output: Vec<u8> = Vec::new();
    // Output requested per call, and the scratch buffer it's decompressed
    // into. Real products are well under 1.3 MB, so this finishes in a
    // handful of iterations.
    const CHUNK: usize = 64 * 1024;
    let mut scratch = vec![0u8; CHUNK];

    loop {
        if output.len() >= cap {
            return Err(Level3DecodeError::Bzip2ExceedsCap { cap });
        }
        let want = CHUNK.min(cap - output.len());
        let consumed_before = decompressor.total_in() as usize;
        let produced_before = decompressor.total_out() as usize;
        let remaining = payload.get(consumed_before..).unwrap_or(&[]);
        let status = decompressor
            .decompress(remaining, &mut scratch[..want])
            .map_err(|e| Level3DecodeError::Bzip2Corrupt(e.to_string()))?;
        // `total_out()` is cumulative across the whole stream; the delta
        // is exactly how many bytes THIS call wrote into `scratch`.
        let produced = decompressor.total_out() as usize - produced_before;
        output.extend_from_slice(&scratch[..produced]);

        if status == Status::StreamEnd {
            let consumed = decompressor.total_in() as usize;
            let trailing = payload.get(consumed..).unwrap_or(&[]);
            check_trailing(trailing)?;
            return Ok(output);
        }
        // Non-terminal status: either more input remains (loop again), or
        // the stream is genuinely truncated — it consumed every byte we
        // gave it without reaching its own end. THREE outcomes, matching
        // the oracle's `decompressor.eof` branch (`nexrad_level3.py:484-495`):
        // reached the end (handled above), truncated mid-stream (here), or
        // about to hit the cap (the check at the top of the next
        // iteration).
        let consumed = decompressor.total_in() as usize;
        if consumed >= payload.len() && output.len() < cap {
            return Err(Level3DecodeError::Bzip2Truncated);
        }
    }
}

/// A WMO/NOAAPORT end-of-message terminator after the bzip2 stream is NOT
/// a second stream. Matched exactly (after stripping NUL padding) rather
/// than trimmed to whatever is left, because a stream cut in half and
/// concatenated with something else must still be refused — see
/// `nexrad_level3.py:496-525`. Three forms tolerated: the full terminator,
/// a bare ETX (some products carry only that), or either followed by NUL
/// padding to a block boundary.
fn check_trailing(trailing: &[u8]) -> Result<()> {
    let trimmed = trim_trailing_nul(trailing);
    if trimmed.is_empty() || trimmed == WMO_TERMINATOR || trimmed == &WMO_TERMINATOR[3..] {
        Ok(())
    } else {
        Err(Level3DecodeError::UnexpectedTrailingBytes {
            trailing_len: trimmed.len(),
        })
    }
}

fn trim_trailing_nul(b: &[u8]) -> &[u8] {
    let mut end = b.len();
    while end > 0 && b[end - 1] == 0 {
        end -= 1;
    }
    &b[..end]
}

/// The 16-byte symbology block header + layer header + 14-byte radial
/// packet header, common to packet codes 16 and `AF1F` (ICD Figures 3-10
/// and 3-11c share this layout; only what follows byte 30 differs).
struct PacketHeader {
    packet_code: i16,
    first_bin: u16,
    n_bins: u16,
    n_radials: u16,
    /// The packet's own range-scale halfword, verbatim. For elevation-
    /// bearing products it's `floor(1000 * cos(elevation))`, NOT metres
    /// (`docs/NEXRAD_LEVEL3_WASM.md` §4.4) — those products' gate spacing
    /// comes from `products::ProductSpec::bin_size` instead. For surface
    /// products (`bin_size: None`) this field genuinely IS the range scale
    /// in metres, used directly.
    range_scale_raw: u16,
}

/// Parse the shared block/layer/packet header, ending right after the
/// packet header (byte offset 30) — where per-radial data begins for both
/// packet 16 and `AF1F`.
fn read_packet_header(body: &[u8]) -> Result<PacketHeader> {
    if body.len() < 30 {
        return Err(Level3DecodeError::SymbologyTruncated { len: body.len() });
    }
    let divider = read_i16_be(body, 0).expect("checked len >= 30 above");
    let block_id = read_i16_be(body, 2).expect("checked len >= 30 above");
    if divider != -1 || block_id != 1 {
        return Err(Level3DecodeError::BadSymbologyHeader { divider, block_id });
    }

    // Skip the block header (10 B) and the layer header (6 B).
    let base = 16;
    let packet_code = read_i16_be(body, base).expect("checked len >= 30 above");
    let first_bin = read_u16_be(body, base + 2).expect("checked len >= 30 above");
    let n_bins = read_u16_be(body, base + 4).expect("checked len >= 30 above");
    // `_i_c` (base+6, i16), `_j_c` (base+8, i16) — unread, matching the
    // oracle's discard.
    let range_scale_raw = read_u16_be(body, base + 10).expect("checked len >= 30 above");
    let n_radials = read_u16_be(body, base + 12).expect("checked len >= 30 above");

    if n_radials == 0 || n_bins == 0 {
        return Err(Level3DecodeError::EmptySweep { n_radials, n_bins });
    }
    // Bound the codes-array ALLOCATION ITSELF, before either packet-16 or
    // AF1F's own per-radial parsing runs — see `MAX_GATE_COUNT`'s doc
    // comment for why this can't be replaced by packet 16's "bytes
    // available" check alone (AF1F's RLE compression breaks that
    // equivalence).
    let cells = n_radials as u64 * n_bins as u64;
    if cells > MAX_GATE_COUNT {
        return Err(Level3DecodeError::GridTooLarge {
            n_radials,
            n_bins,
            cells,
            max: MAX_GATE_COUNT,
        });
    }
    Ok(PacketHeader {
        packet_code,
        first_bin,
        n_bins,
        n_radials,
        range_scale_raw,
    })
}

/// Symbology block -> (azimuths, codes). One layer, one radial packet —
/// packet 16 (raw per-gate bytes) or `AF1F` (RLE-encoded nibbles,
/// expanded here to the same `u8`-per-gate shape). Packet 28 (XDR,
/// `u16` raw levels) is a different geometry this backend doesn't decode
/// yet — see [`Level3DecodeError::UnsupportedPacketCode`]. Azimuths are
/// ray CENTRES in degrees (`start + delta/2`), computed in `f64` to match
/// the oracle's own `np.float64` arithmetic. Mirrors
/// `nexrad_level3.py:560-638` (packet 16) and xradar #392's
/// `_read_packet16`/`_read_packet_af1f`.
pub(crate) fn decode_symbology(body: &[u8]) -> Result<SymbologyResult> {
    let header = read_packet_header(body)?;
    let (azimuths, codes) = match header.packet_code {
        RADIAL_PACKET_CODE => decode_packet16(body, &header)?,
        AF1F_PACKET_CODE => decode_packet_af1f(body, &header)?,
        other => return Err(Level3DecodeError::UnsupportedPacketCode { code: other }),
    };
    Ok(SymbologyResult {
        azimuths,
        codes,
        range_scale_raw: header.range_scale_raw,
    })
}

/// Decoded symbology output, plus the packet's own range-scale halfword
/// (verbatim — see [`PacketHeader::range_scale_raw`] for when it's usable
/// as metres vs. when it's `cos(elevation) * 1000`).
pub(crate) struct SymbologyResult {
    pub azimuths: Vec<f64>,
    pub codes: Array2<u8>,
    pub range_scale_raw: u16,
}

/// Ray CENTRE in degrees — `start_angle`/`delta` are tenths of a degree.
/// Shared by packet 16 and `AF1F`'s per-radial header; see
/// `docs/NEXRAD_LEVEL3_WASM.md` §4.5 for why the centre, not the stored
/// leading edge.
fn azimuth_centre_deg(start_angle: u16, delta: u16) -> f64 {
    (start_angle as f64 + delta as f64 / 2.0) / 10.0
}

/// Read one radial's shared 6-byte per-radial header — `(count,
/// start_angle, delta)`, where `count` is a raw byte count for packet 16
/// but a halfword count for `AF1F` (each caller names it accordingly).
/// Advances `*off` past the header and validates the angular width before
/// returning, matching both callers' original per-radial order: read,
/// advance, then check width.
fn read_radial_prefix(
    body: &[u8],
    off: &mut usize,
    radial: u16,
    n_radials: u16,
) -> Result<(u16, u16, u16)> {
    if *off + 6 > body.len() {
        return Err(Level3DecodeError::TruncatedAtRadial { radial, n_radials });
    }
    let count = read_u16_be(body, *off).expect("checked off + 6 <= len above");
    let start_angle = read_u16_be(body, *off + 2).expect("checked off + 6 <= len above");
    let delta = read_u16_be(body, *off + 4).expect("checked off + 6 <= len above");
    *off += 6;

    // Tenths of a degree; a radial wider than 10 deg is not a radial.
    if !(delta > 0 && delta <= 100) {
        return Err(Level3DecodeError::ImplausibleRadialWidth {
            radial,
            width_deg: delta as f32 / 10.0,
        });
    }
    Ok((count, start_angle, delta))
}

/// Packet 16: raw per-gate bytes, `n_bytes` (>= `n_bins`) declared per
/// radial ahead of its data.
fn decode_packet16(body: &[u8], header: &PacketHeader) -> Result<(Vec<f64>, Array2<u8>)> {
    let PacketHeader {
        first_bin,
        n_bins,
        n_radials,
        ..
    } = *header;
    if first_bin != 0 {
        return Err(Level3DecodeError::NonZeroFirstBin(first_bin));
    }

    // Bound the declaration against the bytes that must fill it, BEFORE
    // allocating: both dimensions are u16 straight off the wire, so an
    // 180-byte input could otherwise demand up to 65535 x 65535 = 4 GiB.
    // Computed in u64 — `read_packet_header`'s `MAX_GATE_COUNT` check
    // already rejects a grid this large before this function is even
    // called, but this stays overflow-safe independent of that ordering:
    // a raw `usize` multiply here would wrap on a 32-bit target (wasm32)
    // and silently pass a `needed` far smaller than the real requirement.
    let mut off = 30;
    let needed_u64 = n_radials as u64 * (6 + n_bins as u64);
    let available = body.len().saturating_sub(off);
    if needed_u64 > available as u64 {
        return Err(Level3DecodeError::AllocationExceedsBody {
            n_radials,
            n_bins,
            needed: needed_u64.min(usize::MAX as u64) as usize,
            available,
        });
    }

    let mut azimuths = vec![0.0f64; n_radials as usize];
    let mut codes = Array2::<u8>::zeros((n_radials as usize, n_bins as usize));

    for radial in 0..n_radials {
        let (n_bytes, start_angle, delta) = read_radial_prefix(body, &mut off, radial, n_radials)?;
        if n_bytes < n_bins || off + n_bytes as usize > body.len() {
            return Err(Level3DecodeError::RadialByteCountMismatch {
                radial,
                n_bytes,
                n_bins,
            });
        }
        azimuths[radial as usize] = azimuth_centre_deg(start_angle, delta);
        let row = &body[off..off + n_bins as usize];
        codes
            .row_mut(radial as usize)
            .as_slice_mut()
            .expect("row of a freshly-allocated Array2::zeros is always contiguous")
            .copy_from_slice(row);
        off += n_bytes as usize;
    }
    Ok((azimuths, codes))
}

/// Packet `AF1F`: run-length-encoded nibbles. Each radial declares its RLE
/// span as a **halfword count** (`n_bytes * 2` = the byte span), not a
/// direct byte count like packet 16. Each RLE byte packs a 4-bit colour
/// (low nibble, the raw code 0-15) and a 4-bit run length (high nibble);
/// expanding every byte's run must land on exactly `n_bins` gates per
/// radial. Mirrors xradar #392's `_read_packet_af1f`.
fn decode_packet_af1f(body: &[u8], header: &PacketHeader) -> Result<(Vec<f64>, Array2<u8>)> {
    let PacketHeader {
        n_bins, n_radials, ..
    } = *header;
    // AF1F has no `first_bin` concept in the ICD the way packet 16 does —
    // xradar doesn't check it either — so it's read (in `read_packet_header`)
    // but not validated here.

    let mut off = 30;
    let mut azimuths = vec![0.0f64; n_radials as usize];
    let mut codes = Array2::<u8>::zeros((n_radials as usize, n_bins as usize));

    for radial in 0..n_radials {
        let (n_halfwords, start_angle, delta) =
            read_radial_prefix(body, &mut off, radial, n_radials)?;
        let rle_byte_len = n_halfwords as usize * 2;
        if off + rle_byte_len > body.len() {
            return Err(Level3DecodeError::RadialByteCountMismatch {
                radial,
                n_bytes: n_halfwords,
                n_bins,
            });
        }
        azimuths[radial as usize] = azimuth_centre_deg(start_angle, delta);

        let rle_bytes = &body[off..off + rle_byte_len];
        let mut gate = 0usize;
        let mut row = codes.row_mut(radial as usize);
        for &b in rle_bytes {
            let color = b & 0x0F;
            let run = (b >> 4) as usize;
            if run == 0 {
                continue;
            }
            let end = gate + run;
            if end > n_bins as usize {
                break; // caught by the total-mismatch check below
            }
            for cell in row.iter_mut().take(end).skip(gate) {
                *cell = color;
            }
            gate = end;
        }
        if gate != n_bins as usize {
            return Err(Level3DecodeError::RleExpansionMismatch {
                radial,
                expanded: gate,
                expected: n_bins,
            });
        }
        off += rle_byte_len;
    }
    Ok((azimuths, codes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal one-radial packet-16 symbology block: block header
    /// (10 B) + layer header (6 B) + packet header (14 B) + one radial (6 B
    /// header + `n_bins` code bytes).
    fn build_symbology(n_bins: u16, codes: &[u8], start_angle: u16, delta: u16) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&(-1i16).to_be_bytes()); // divider
        body.extend_from_slice(&1i16.to_be_bytes()); // block_id
        body.extend_from_slice(&[0u8; 6]); // rest of block header
        body.extend_from_slice(&[0u8; 6]); // layer header
        body.extend_from_slice(&16u16.to_be_bytes()); // packet_code
        body.extend_from_slice(&0u16.to_be_bytes()); // first_bin
        body.extend_from_slice(&n_bins.to_be_bytes()); // n_bins
        body.extend_from_slice(&999i16.to_be_bytes()); // _i_c (unused)
        body.extend_from_slice(&998i16.to_be_bytes()); // _j_c (unused)
        body.extend_from_slice(&999u16.to_be_bytes()); // _scale (unused, cos(el)*1000)
        body.extend_from_slice(&1u16.to_be_bytes()); // n_radials
        body.extend_from_slice(&(n_bins).to_be_bytes()); // n_bytes for the radial
        body.extend_from_slice(&start_angle.to_be_bytes());
        body.extend_from_slice(&delta.to_be_bytes());
        body.extend_from_slice(codes);
        body
    }

    #[test]
    fn decode_symbology_computes_ray_centre_not_start_angle() {
        let body = build_symbology(4, &[2, 3, 4, 5], 100, 10); // 10.0 deg start, 1.0 deg wide
        let result = decode_symbology(&body).unwrap();
        let (azimuths, codes) = (result.azimuths, result.codes);
        assert_eq!(azimuths.len(), 1);
        // centre = (100 + 10/2.0) / 10.0 = 10.5, NOT the 10.0 start angle.
        assert!((azimuths[0] - 10.5).abs() < 1e-9);
        assert_eq!(codes.row(0).to_vec(), vec![2, 3, 4, 5]);
    }

    #[test]
    fn decode_symbology_rejects_unsupported_packet_code() {
        let mut body = build_symbology(2, &[2, 3], 0, 10);
        body[16..18].copy_from_slice(&12i16.to_be_bytes()); // some other packet code
        assert!(matches!(
            decode_symbology(&body),
            Err(Level3DecodeError::UnsupportedPacketCode { code: 12 })
        ));
    }

    #[test]
    fn decode_symbology_rejects_packet_28_as_unsupported() {
        // Confirms packet 28 (XDR, u16 raw levels) is deliberately
        // deferred rather than misread as packet 16 or AF1F.
        let mut body = build_symbology(2, &[2, 3], 0, 10);
        body[16..18].copy_from_slice(&28i16.to_be_bytes());
        assert!(matches!(
            decode_symbology(&body),
            Err(Level3DecodeError::UnsupportedPacketCode { code: 28 })
        ));
    }

    #[test]
    fn decode_symbology_rejects_a_crafted_grid_before_allocating() {
        // Regression test for a real bug this backend shipped with: a
        // ~100-byte crafted body declaring n_radials = n_bins = 65535
        // (packet 16) requests a ~4 GiB `Array2::<u8>::zeros` allocation
        // with no body data behind it at all. Must be rejected by
        // `GridTooLarge` before any allocation is attempted — if this
        // regresses, the test process aborts or OOMs rather than failing
        // cleanly.
        let mut body = build_symbology(2, &[2, 3], 0, 10);
        body[20..22].copy_from_slice(&65535u16.to_be_bytes()); // n_bins
        body[28..30].copy_from_slice(&65535u16.to_be_bytes()); // n_radials
        assert!(matches!(
            decode_symbology(&body),
            Err(Level3DecodeError::GridTooLarge {
                n_radials: 65535,
                n_bins: 65535,
                ..
            })
        ));
    }

    #[test]
    fn decode_packet_af1f_rejects_a_crafted_grid_before_allocating() {
        // Same regression, packet AF1F: RLE compression means a tiny body
        // can legitimately declare a huge n_radials/n_bins (unlike packet
        // 16, whose per-radial byte count alone would reject this) — the
        // `GridTooLarge` guard in `read_packet_header` is what catches it
        // here, not a body-bytes-available check.
        let rle = [(2u8 << 4) | 5, (2u8 << 4) | 9];
        let mut body = build_af1f_symbology(4, &rle, 100, 10);
        body[20..22].copy_from_slice(&65535u16.to_be_bytes()); // n_bins
        body[28..30].copy_from_slice(&65535u16.to_be_bytes()); // n_radials
        assert!(matches!(
            decode_symbology(&body),
            Err(Level3DecodeError::GridTooLarge {
                n_radials: 65535,
                n_bins: 65535,
                ..
            })
        ));
    }

    #[test]
    fn decode_packet16_rejects_a_grid_under_the_cap_but_over_the_body() {
        // A grid small enough to pass `GridTooLarge` (well under
        // MAX_GATE_COUNT) but that still claims more radials than the
        // body actually has bytes for — `AllocationExceedsBody`, packet
        // 16's own per-radial-byte-count guard, not the grid-size one.
        let mut body = build_symbology(2, &[2, 3], 0, 10);
        body[28..30].copy_from_slice(&1000u16.to_be_bytes()); // n_radials, body has 1
        assert!(matches!(
            decode_symbology(&body),
            Err(Level3DecodeError::AllocationExceedsBody {
                n_radials: 1000,
                ..
            })
        ));
    }

    #[test]
    fn decode_symbology_rejects_nonzero_first_bin() {
        let mut body = build_symbology(2, &[2, 3], 0, 10);
        body[18..20].copy_from_slice(&1u16.to_be_bytes());
        assert!(matches!(
            decode_symbology(&body),
            Err(Level3DecodeError::NonZeroFirstBin(1))
        ));
    }

    #[test]
    fn decode_symbology_rejects_implausible_radial_width() {
        let body = build_symbology(2, &[2, 3], 0, 101); // > 10 deg
        assert!(matches!(
            decode_symbology(&body),
            Err(Level3DecodeError::ImplausibleRadialWidth { .. })
        ));
        let body_zero = build_symbology(2, &[2, 3], 0, 0);
        assert!(matches!(
            decode_symbology(&body_zero),
            Err(Level3DecodeError::ImplausibleRadialWidth { .. })
        ));
    }

    #[test]
    fn decode_symbology_rejects_truncated_body() {
        assert!(matches!(
            decode_symbology(b"short"),
            Err(Level3DecodeError::SymbologyTruncated { .. })
        ));
    }

    #[test]
    fn decode_symbology_tolerates_pad_byte_beyond_n_bins() {
        // n_bytes (5) > n_bins (4): a real odd-bin-count radial's pad byte.
        // Only the first n_bins bytes should be read into codes.
        let mut body = Vec::new();
        body.extend_from_slice(&(-1i16).to_be_bytes());
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&[0u8; 6]);
        body.extend_from_slice(&[0u8; 6]);
        body.extend_from_slice(&16u16.to_be_bytes());
        body.extend_from_slice(&0u16.to_be_bytes());
        body.extend_from_slice(&4u16.to_be_bytes()); // n_bins = 4
        body.extend_from_slice(&999i16.to_be_bytes());
        body.extend_from_slice(&998i16.to_be_bytes());
        body.extend_from_slice(&999u16.to_be_bytes());
        body.extend_from_slice(&1u16.to_be_bytes()); // n_radials = 1
        body.extend_from_slice(&5u16.to_be_bytes()); // n_bytes = 5 (pad byte)
        body.extend_from_slice(&0u16.to_be_bytes());
        body.extend_from_slice(&10u16.to_be_bytes());
        body.extend_from_slice(&[7, 8, 9, 10, 0xFF]); // 5 bytes, last is pad
        let result = decode_symbology(&body).unwrap();
        let codes = result.codes;
        assert_eq!(codes.row(0).to_vec(), vec![7, 8, 9, 10]);
    }

    #[test]
    fn decompress_payload_passes_through_uncompressed_bytes() {
        let payload = b"not bzip2 data";
        assert_eq!(decompress_payload(payload).unwrap(), payload.to_vec());
    }

    #[test]
    fn decompress_payload_rejects_oversized_uncompressed() {
        let payload = vec![0u8; MAX_DECOMPRESSED_BYTES + 1];
        assert!(matches!(
            decompress_payload(&payload),
            Err(Level3DecodeError::UncompressedPayloadTooLarge { .. })
        ));
    }

    #[test]
    fn decompress_bzip2_capped_round_trips_a_real_stream() {
        use bzip2::write::BzEncoder;
        use bzip2::Compression;
        use std::io::Write;

        let original = b"NEXRAD Level 3 test payload, repeated ".repeat(200);
        let mut encoder = BzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&original).unwrap();
        let compressed = encoder.finish().unwrap();

        let out = decompress_bzip2_capped(&compressed, MAX_DECOMPRESSED_BYTES).unwrap();
        assert_eq!(out, original);
    }

    #[test]
    fn decompress_bzip2_capped_rejects_truncated_stream() {
        use bzip2::write::BzEncoder;
        use bzip2::Compression;
        use std::io::Write;

        let original = b"NEXRAD Level 3 test payload, repeated ".repeat(200);
        let mut encoder = BzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&original).unwrap();
        let compressed = encoder.finish().unwrap();
        let truncated = &compressed[..compressed.len() / 2];

        assert!(matches!(
            decompress_bzip2_capped(truncated, MAX_DECOMPRESSED_BYTES),
            Err(Level3DecodeError::Bzip2Truncated)
        ));
    }

    #[test]
    fn decompress_bzip2_capped_enforces_the_cap() {
        use bzip2::write::BzEncoder;
        use bzip2::Compression;
        use std::io::Write;

        // Highly compressible input so the compressed stream is tiny but
        // the decompressed output is well over a small cap.
        let original = vec![b'A'; 1_000_000];
        let mut encoder = BzEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&original).unwrap();
        let compressed = encoder.finish().unwrap();

        assert!(matches!(
            decompress_bzip2_capped(&compressed, 1024),
            Err(Level3DecodeError::Bzip2ExceedsCap { cap: 1024 })
        ));
    }

    #[test]
    fn check_trailing_tolerates_wmo_terminator_and_bare_etx() {
        assert!(check_trailing(b"").is_ok());
        assert!(check_trailing(WMO_TERMINATOR).is_ok());
        assert!(check_trailing(b"\x03").is_ok());
        assert!(check_trailing(b"\r\r\n\x03\x00\x00").is_ok()); // NUL-padded
        assert!(check_trailing(b"garbage").is_err());
    }

    /// Build a minimal one-radial packet-AF1F symbology block: the same
    /// 30-byte shared header as `build_symbology`, but with `AF1F_PACKET_CODE`
    /// and an RLE-encoded radial (`rle` = concatenated (run<<4 | color)
    /// bytes) instead of raw per-gate bytes.
    fn build_af1f_symbology(n_bins: u16, rle: &[u8], start_angle: u16, delta: u16) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&(-1i16).to_be_bytes());
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&[0u8; 6]);
        body.extend_from_slice(&[0u8; 6]);
        body.extend_from_slice(&AF1F_PACKET_CODE.to_be_bytes());
        body.extend_from_slice(&0u16.to_be_bytes()); // first_bin (unchecked for AF1F)
        body.extend_from_slice(&n_bins.to_be_bytes());
        body.extend_from_slice(&999i16.to_be_bytes());
        body.extend_from_slice(&998i16.to_be_bytes());
        body.extend_from_slice(&999u16.to_be_bytes());
        body.extend_from_slice(&1u16.to_be_bytes()); // n_radials
                                                     // n_bytes is a HALFWORD count for AF1F, so byte len / 2 (rle.len()
                                                     // must be even for a well-formed test fixture).
        assert_eq!(
            rle.len() % 2,
            0,
            "test rle payload must be halfword-aligned"
        );
        body.extend_from_slice(&((rle.len() / 2) as u16).to_be_bytes());
        body.extend_from_slice(&start_angle.to_be_bytes());
        body.extend_from_slice(&delta.to_be_bytes());
        body.extend_from_slice(rle);
        body
    }

    #[test]
    fn decode_packet_af1f_expands_runs_to_the_declared_bin_count() {
        // 4 bins: run=2 color=5, run=2 color=9 -> [5,5,9,9]
        let rle = [(2u8 << 4) | 5, (2u8 << 4) | 9];
        let body = build_af1f_symbology(4, &rle, 100, 10);
        let result = decode_symbology(&body).unwrap();
        let (azimuths, codes) = (result.azimuths, result.codes);
        assert!((azimuths[0] - 10.5).abs() < 1e-9);
        assert_eq!(codes.row(0).to_vec(), vec![5, 5, 9, 9]);
    }

    #[test]
    fn decode_packet_af1f_rejects_runs_that_dont_sum_to_n_bins() {
        // Declares 4 bins but the RLE only expands to 3 (1 + 2). `n_bytes`
        // is a halfword count on the wire, so the RLE span itself must
        // stay even-length; the mismatch is in the *expansion*, not the
        // byte count.
        let rle = [(1u8 << 4) | 7, (2u8 << 4) | 3];
        let body = build_af1f_symbology(4, &rle, 0, 10);
        assert!(matches!(
            decode_symbology(&body),
            Err(Level3DecodeError::RleExpansionMismatch {
                expanded: 3,
                expected: 4,
                ..
            })
        ));
    }

    #[test]
    fn decode_packet_af1f_tolerates_zero_length_runs() {
        // A zero-run byte contributes nothing but isn't itself invalid.
        let rle = [3u8, (2u8 << 4) | 5];
        let body = build_af1f_symbology(2, &rle, 0, 10);
        let result = decode_symbology(&body).unwrap();
        let codes = result.codes;
        assert_eq!(codes.row(0).to_vec(), vec![5, 5]);
    }
}

//! Errors raised by the NEXRAD Level 3 (NIDS) byte-level decoder.
//!
//! One variant per `Level3Error` raise site in the Python oracle
//! (`radar-animation/src/server/nexrad_level3.py`) this backend must match
//! byte-for-byte on — see `docs/NEXRAD_LEVEL3_WASM.md`. Kept 1:1 with the
//! oracle deliberately: the parity test suite (Phase 4) needs to assert
//! "this input fails to decode" as precisely as "this input decodes to
//! these bytes", and a merged catch-all variant would blur which check
//! actually fired.

use thiserror::Error;

/// Errors that can surface while decoding a NEXRAD Level 3 (NIDS) product.
/// `pub(crate)`, not `pub`: never referenced outside the `decode` module
/// tree — the `backends::nexrad_level3` boundary wraps it into
/// `RadishError::Decode(String)` via `Display`, matching how the `nexrad`
/// backend wraps its own internal `NexradDecodeError`.
#[derive(Debug, Error)]
pub(crate) enum Level3DecodeError {
    /// The text header didn't contain any 6-character alphanumeric token
    /// at all. Since Phase 3 this is no longer about product recognition
    /// (moved to the message code, `products::spec_for`) — a malformed or
    /// missing token means no site string is available at all.
    #[error("no AWIPS product id in the text header")]
    NoAwipsToken,

    /// No offset in the first 128 bytes both matched a known message code
    /// and had the PDB's `-1` divider 18 bytes later.
    #[error("no digital radial message header found (known message codes {known:?})")]
    NoMessageHeader { known: Vec<u16> },

    /// Fewer than `need` bytes before the product description block ends
    /// — reading past this would hit `struct.unpack`-equivalent UB.
    #[error("truncated before the product description block ({have} bytes, need {need})")]
    TruncatedBeforePdb { have: usize, need: usize },

    /// PDB halfword 7 (product code) disagreed with the message header's
    /// code — the file is internally inconsistent.
    #[error("product code {product_code} disagrees with message code {message_code}")]
    ProductCodeMismatch {
        product_code: i16,
        message_code: i16,
    },

    /// Decoded site lat/lon isn't a point on Earth.
    #[error("site position ({lat}, {lon}) is not on Earth")]
    ImplausibleSitePosition { lat: f64, lon: f64 },

    /// The float32 scale form's `value_increment` came out non-positive.
    #[error("non-positive value increment {0}")]
    NonPositiveIncrement(f32),

    /// The float32 scale form's declared `scale` is zero or non-finite.
    #[error("non-finite or zero value scale {0}")]
    NonFiniteScale(f32),

    /// The float32 scale form's declared `offset` is non-finite.
    #[error("non-finite value offset {0}")]
    NonFiniteOffset(f32),

    /// PDB `days`/`seconds` don't form a plausible date/time.
    #[error("implausible scan date/time (days={days}, seconds={seconds})")]
    ImplausibleScanTime { days: u16, seconds: i32 },

    /// Decoded scan time is further in the future than
    /// `MAX_SCAN_SKEW_SECONDS` allows — corruption, not clock skew.
    #[error("scan time is {days_ahead:.0} days in the future")]
    ScanTimeTooFarAhead { days_ahead: f64 },

    /// An uncompressed payload (no `BZ` magic) exceeded the decompression
    /// cap on its own — nothing else bounds a raw payload's size.
    #[error("uncompressed payload of {len} bytes exceeds the {cap}-byte decompression cap")]
    UncompressedPayloadTooLarge { len: usize, cap: usize },

    /// The bzip2 stream ended without reaching its own end-of-stream
    /// marker — a short/partial read, not a size violation.
    #[error("bzip2 stream ends mid-payload")]
    Bzip2Truncated,

    /// Incremental bzip2 decompression hit the hard byte cap before the
    /// stream finished — the decompression-bomb guard firing.
    #[error("payload exceeds the {cap}-byte decompression cap")]
    Bzip2ExceedsCap { cap: usize },

    /// bzip2 decompression failed for a reason other than truncation or
    /// exceeding the cap (corrupt stream).
    #[error("corrupt bzip2 payload: {0}")]
    Bzip2Corrupt(String),

    /// Bytes remained after the bzip2 stream ended that weren't a
    /// tolerated WMO/NOAAPORT terminator — a sign of a body cut in half
    /// and concatenated with something else.
    #[error("{trailing_len} unexpected bytes after the bzip2 stream")]
    UnexpectedTrailingBytes { trailing_len: usize },

    /// The decompressed symbology block is shorter than its own fixed
    /// header.
    #[error("symbology block truncated ({len} bytes)")]
    SymbologyTruncated { len: usize },

    /// The symbology block's divider/block-id pair didn't match the ICD's
    /// fixed values.
    #[error("bad symbology header (divider={divider}, block_id={block_id})")]
    BadSymbologyHeader { divider: i16, block_id: i16 },

    /// The packet declared zero radials or zero bins.
    #[error("empty sweep ({n_radials} radials x {n_bins} bins)")]
    EmptySweep { n_radials: u16, n_bins: u16 },

    /// The packet's first-bin offset wasn't 0 — never observed in
    /// practice, and honouring a nonzero value would shift every gate.
    #[error("first range bin {0} is not 0; unsupported")]
    NonZeroFirstBin(u16),

    /// `n_radials * (6 + n_bins)` exceeds the bytes actually remaining in
    /// the body — the pre-allocation guard against a multi-GiB allocation
    /// from a tiny crafted input.
    #[error("{n_radials}x{n_bins} needs {needed} bytes, only {available} remain")]
    AllocationExceedsBody {
        n_radials: u16,
        n_bins: u16,
        needed: usize,
        available: usize,
    },

    /// The body ended before every declared radial's 6-byte header could
    /// be read.
    #[error("truncated at radial {radial}/{n_radials}")]
    TruncatedAtRadial { radial: u16, n_radials: u16 },

    /// A radial's declared angular width is 0 or wider than 10 degrees.
    #[error("radial {radial} has an implausible width {width_deg} deg")]
    ImplausibleRadialWidth { radial: u16, width_deg: f32 },

    /// A radial declared fewer bytes than the packet's bin count, or its
    /// declared byte count runs past the end of the body.
    #[error("radial {radial} declares {n_bytes} bytes for {n_bins} bins")]
    RadialByteCountMismatch {
        radial: u16,
        n_bytes: u16,
        n_bins: u16,
    },

    /// A PDB halfword read fell outside the buffer. Should be unreachable
    /// in practice — callers check `raw.len() >= pdb + 102` before reading
    /// any PDB field, and every field this decoder reads sits within that
    /// span — but reads stay checked rather than trusting that invariant
    /// silently, matching this crate's no-panics-on-untrusted-input
    /// discipline (`docs/NEXRAD_LEVEL3_WASM.md` §4.7).
    #[error("PDB halfword {index} is out of bounds")]
    PdbFieldOutOfBounds { index: usize },

    /// The symbology layer's packet code is neither the digital radial
    /// array (16) nor RLE radials (`AF1F`) — most commonly the generic
    /// data packet (28, XDR: DPR rate / hybrid hydrometeor class), which
    /// this backend does not decode yet (its raw levels are `u16`, not
    /// `u8` — see `decode::products`'s module doc and
    /// `plans/0011-nexrad-level3-wasm-backend.md` Phase 3's exit notes).
    #[error("unsupported symbology packet code {code} (0x{code:04x})")]
    UnsupportedPacketCode { code: i16 },

    /// The message code is recognised (`products::is_known_message_code`)
    /// but this pass deliberately didn't implement its packet family —
    /// the 5 surface precipitation products (170/172-175, packet type not
    /// independently confirmed this pass) or the packet-28 products
    /// (176/177). See [`Self::UnsupportedPacketCode`] and Phase 3's exit
    /// notes.
    #[error("NEXRAD Level 3 message code {code} is a known product but its packet family isn't implemented yet")]
    UnsupportedProduct { code: i16 },

    /// A packet-AF1F (RLE) radial's runs didn't sum to exactly `expected`
    /// bins — either a malformed input or (rarer) a genuinely different
    /// encoding this decoder doesn't handle.
    #[error("radial {radial} RLE runs expand to {expanded} bins, expected {expected}")]
    RleExpansionMismatch {
        radial: u16,
        expanded: usize,
        expected: u16,
    },

    /// `n_radials * n_bins` (the codes array this file would allocate)
    /// exceeds [`MAX_GATE_COUNT`](super::symbology::MAX_GATE_COUNT) —
    /// checked BEFORE allocating, in `u64`, and independent of how many
    /// body bytes are actually present. `AllocationExceedsBody` alone
    /// isn't enough for packet `AF1F`: RLE compression means a tiny body
    /// can legitimately declare a huge `n_radials`/`n_bins`, so bounding
    /// only against "bytes available" (as packet 16's per-radial check
    /// does) can't catch it — this bounds the allocation itself instead.
    #[error("{n_radials}x{n_bins} = {cells} gates exceeds the {max}-gate cap")]
    GridTooLarge {
        n_radials: u16,
        n_bins: u16,
        cells: u64,
        max: u64,
    },
}

/// Convenience alias used throughout the decode module.
pub(crate) type Result<T> = std::result::Result<T, Level3DecodeError>;

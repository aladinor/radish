//! The product table this backend can decode: message code -> moment,
//! decode scheme, and geometry. Plus a small, SEPARATE tilt-ordinal table
//! for the 6 products whose AWIPS letter -> tilt mapping was independently
//! verified against real files pulled from `s3://unidata-nexrad-level3`.
//!
//! **Two tables, on purpose, for two different questions:**
//!
//! - "What moment/decode scheme does message code N carry?" — [`PRODUCTS`],
//!   keyed by message code. Ported from xradar #392's `PRODUCT_TABLE`
//!   (openradar/xradar, commit `fcc31ae`), which is the breadth reference
//!   for the 20 codes beyond the original 6 — see `docs/NEXRAD_LEVEL3_WASM.md`
//!   §3. This is the ONLY correct way to resolve moment: a message
//!   code identifies the *format*, and unlike tilt, every file with that
//!   code carries the *same* moment regardless of which tilt it's at.
//! - "Which of the 6 canonical tilt ordinals (0-5) does AWIPS letter L at
//!   this file's site belong to?" — [`TILT_LETTER_TABLE`] /
//!   [`tilt_letter_lookup`]. It only covers the 6 letters (`B`,`G`,`U`,`X`,
//!   `C`,`K`) independently confirmed present in the bucket. Fabricating
//!   AWIPS letters for the other 20 codes was deliberately NOT done here —
//!   this repo has no S3-verified source for them the way the original 6
//!   have, and the standing rule for this decoder is "every added code
//!   needs a fixture or a verified source, or it is untested surface."
//!   `DecodedProduct::tilt` is `Option<usize>`: `Some` only when this
//!   table resolves it, `None` otherwise — never guessed from elevation
//!   angle. The real elevation always comes from the PDB regardless.

/// `(AWIPS tilt prefix, ordinal)` — elevation order, matching the byte-level
/// oracle's own tilt-prefix table.
pub(crate) const TILT_PREFIXES: [&str; 6] = ["N0", "NA", "N1", "NB", "N2", "N3"];

/// How a product's raw levels become physical values. Six schemes, matching
/// xradar #392's `ProductSpec.decode` string values exactly
/// (`linear_hw`/`float_scale`/`precip`/`rate`/`legacy16`/`class_int`) — an
/// enum here, not a string, for the same reason `docs/NEXRAD_LEVEL3_WASM.md`
/// §4.3 gives for the original int/float split: the type system should make
/// an unrepresentable scheme unrepresentable, not merely undetected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecodeScheme {
    /// Halfwords 22/23 as `i16`: `scale = hw23/10 * post_scale`,
    /// `offset = (hw22/10 - 2*hw23/10) * post_scale`. Codes 94/99/153/154/155.
    LinearHw,
    /// Halfwords 22-25 as two big-endian `f32` (scale, offset); physical =
    /// `(raw - offset) / scale`. Codes 159/161/163.
    FloatScale,
    /// Same byte layout as `FloatScale`, but the post-decode factor is
    /// fixed at `0.01 in -> mm` regardless of `post_scale` — surface
    /// accumulation products (170/172-175). **Deferred** — see Phase 3's
    /// exit notes; packet type unconfirmed, not implemented this pass.
    Precip,
    /// Same as `Precip` but the factor is `1 in -> mm` (code 176, DPR
    /// rate). **Deferred** — packet 28 (XDR) not implemented this pass.
    Rate,
    /// The 16-level flag-encoded legacy scheme: `threshold_data` is 16
    /// `(flag, value)` byte pairs, one entry per possible raw level 0-15.
    /// Codes 19/20/25/27/28/30/56/78/79/80, all via packet AF1F.
    Legacy16,
    /// Categorical — codes index a fixed label table, not a linear scale.
    /// Code 165 (packet 16) is implemented; code 177 (packet 28, "hybrid"
    /// hydrometeor classification) is **deferred** with 176.
    ClassInt,
}

/// One message code's decode parameters. Mirrors xradar's `ProductSpec`
/// (`nexrad_level3.py` in xradar, i.e. `xradar/io/backends/nexrad_level3.py`)
/// field for field.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProductSpec {
    pub moment: &'static str,
    pub decode: DecodeScheme,
    /// Range bin size in metres, when the ICD resolution is fixed and
    /// known. `None` means: use the packet header's own range-scale field
    /// directly — correct ONLY for surface products, where (unlike
    /// elevation-bearing ones) that field is not
    /// `floor(1000 * cos(elevation))`. See `docs/NEXRAD_LEVEL3_WASM.md` §4.4.
    pub bin_size: Option<f32>,
    /// Whether this product carries a real elevation angle (PDB
    /// `halfword_30`-equivalent) vs. `0.0` for surface/volume products.
    pub has_elevation: bool,
    /// Unit-conversion factor applied after the raw scale/offset — e.g.
    /// knots -> m/s for legacy velocity products.
    pub post_scale: f32,
}

/// 1 international knot in m/s.
const KT_TO_MS: f32 = 0.514444;

/// Message code -> product spec. Ported from xradar #392's `PRODUCT_TABLE`.
/// Codes marked "deferred" below decode their PDB fields (so
/// `find_message_header`/`read_pdb_fields` work) but symbology decode
/// returns [`super::error::Level3DecodeError`]'s unsupported-product path —
/// see this module's own doc comment and `UnsupportedPacketCode`'s doc for
/// why (packet 28's `u16` raw levels are a real model mismatch with this
/// backend's `u8`-based contract, not just unimplemented).
pub(crate) const PRODUCTS: &[(i16, ProductSpec)] = &[
    // --- packet AF1F (RLE), legacy 16-level scheme ---
    (
        19,
        ProductSpec {
            moment: "DBZH",
            decode: DecodeScheme::Legacy16,
            bin_size: Some(1000.0),
            has_elevation: true,
            post_scale: 1.0,
        },
    ),
    (
        20,
        ProductSpec {
            moment: "DBZH",
            decode: DecodeScheme::Legacy16,
            bin_size: Some(2000.0),
            has_elevation: true,
            post_scale: 1.0,
        },
    ),
    (
        25,
        ProductSpec {
            moment: "VRADH",
            decode: DecodeScheme::Legacy16,
            bin_size: Some(250.0),
            has_elevation: true,
            post_scale: KT_TO_MS,
        },
    ),
    (
        27,
        ProductSpec {
            moment: "VRADH",
            decode: DecodeScheme::Legacy16,
            bin_size: Some(1000.0),
            has_elevation: true,
            post_scale: KT_TO_MS,
        },
    ),
    (
        28,
        ProductSpec {
            moment: "WRADH",
            decode: DecodeScheme::Legacy16,
            bin_size: Some(250.0),
            has_elevation: true,
            post_scale: KT_TO_MS,
        },
    ),
    (
        30,
        ProductSpec {
            moment: "WRADH",
            decode: DecodeScheme::Legacy16,
            bin_size: Some(1000.0),
            has_elevation: true,
            post_scale: KT_TO_MS,
        },
    ),
    (
        56,
        ProductSpec {
            moment: "SRMV",
            decode: DecodeScheme::Legacy16,
            bin_size: Some(1000.0),
            has_elevation: true,
            post_scale: KT_TO_MS,
        },
    ),
    (
        78,
        ProductSpec {
            moment: "ACCUM",
            decode: DecodeScheme::Legacy16,
            bin_size: None,
            has_elevation: false,
            post_scale: 25.4, // 1 in -> mm
        },
    ),
    (
        79,
        ProductSpec {
            moment: "ACCUM",
            decode: DecodeScheme::Legacy16,
            bin_size: None,
            has_elevation: false,
            post_scale: 25.4,
        },
    ),
    (
        80,
        ProductSpec {
            moment: "ACCUM",
            decode: DecodeScheme::Legacy16,
            bin_size: None,
            has_elevation: false,
            post_scale: 25.4,
        },
    ),
    // --- packet 16, linear halfword scale ---
    (
        94,
        ProductSpec {
            moment: "DBZH",
            decode: DecodeScheme::LinearHw,
            bin_size: Some(1000.0),
            has_elevation: true,
            post_scale: 1.0,
        },
    ),
    (
        99,
        ProductSpec {
            moment: "VRADH",
            decode: DecodeScheme::LinearHw,
            bin_size: Some(250.0),
            has_elevation: true,
            post_scale: 1.0,
        },
    ),
    (
        153,
        ProductSpec {
            moment: "DBZH",
            decode: DecodeScheme::LinearHw,
            bin_size: Some(250.0),
            has_elevation: true,
            post_scale: 1.0,
        },
    ),
    (
        154,
        ProductSpec {
            moment: "VRADH",
            decode: DecodeScheme::LinearHw,
            bin_size: Some(250.0),
            has_elevation: true,
            post_scale: 1.0,
        },
    ),
    (
        155,
        ProductSpec {
            moment: "WRADH",
            decode: DecodeScheme::LinearHw,
            bin_size: Some(250.0),
            has_elevation: true,
            post_scale: 1.0,
        },
    ),
    // --- packet 16, float scale/offset (dual-pol) ---
    (
        159,
        ProductSpec {
            moment: "ZDR",
            decode: DecodeScheme::FloatScale,
            bin_size: Some(250.0),
            has_elevation: true,
            post_scale: 1.0,
        },
    ),
    (
        161,
        ProductSpec {
            moment: "RHOHV",
            decode: DecodeScheme::FloatScale,
            bin_size: Some(250.0),
            has_elevation: true,
            post_scale: 1.0,
        },
    ),
    (
        163,
        ProductSpec {
            moment: "KDP",
            decode: DecodeScheme::FloatScale,
            bin_size: Some(250.0),
            has_elevation: true,
            post_scale: 1.0,
        },
    ),
    // --- packet 16, categorical ---
    (
        165,
        ProductSpec {
            moment: "HCLASS",
            decode: DecodeScheme::ClassInt,
            bin_size: Some(250.0),
            has_elevation: true,
            post_scale: 1.0,
        },
    ),
    // --- deferred: packet type not confirmed / packet 28 (XDR) ---
    (
        170,
        ProductSpec {
            moment: "ACCUM",
            decode: DecodeScheme::Precip,
            bin_size: None,
            has_elevation: false,
            post_scale: 1.0,
        },
    ),
    (
        172,
        ProductSpec {
            moment: "ACCUM",
            decode: DecodeScheme::Precip,
            bin_size: None,
            has_elevation: false,
            post_scale: 1.0,
        },
    ),
    (
        173,
        ProductSpec {
            moment: "ACCUM",
            decode: DecodeScheme::Precip,
            bin_size: None,
            has_elevation: false,
            post_scale: 1.0,
        },
    ),
    (
        174,
        ProductSpec {
            moment: "ACCUM",
            decode: DecodeScheme::Precip,
            bin_size: None,
            has_elevation: false,
            post_scale: 1.0,
        },
    ),
    (
        175,
        ProductSpec {
            moment: "ACCUM",
            decode: DecodeScheme::Precip,
            bin_size: None,
            has_elevation: false,
            post_scale: 1.0,
        },
    ),
    (
        176,
        ProductSpec {
            moment: "RATE",
            decode: DecodeScheme::Rate,
            bin_size: None,
            has_elevation: false,
            post_scale: 1.0,
        },
    ),
    (
        177,
        ProductSpec {
            moment: "HCLASS",
            decode: DecodeScheme::ClassInt,
            bin_size: Some(250.0),
            has_elevation: false,
            post_scale: 1.0,
        },
    ),
];

/// Message codes whose symbology packet type this pass confirmed and
/// implemented (packet 16 or AF1F, both `u8` raw levels). Codes in
/// [`PRODUCTS`] but not here decode their PDB fine but return
/// [`super::error::Level3DecodeError::UnsupportedProduct`] at the
/// symbology stage — see the module doc for why (packet 28's `u16` raw
/// levels don't fit this backend's `u8` code model yet, and the precip
/// codes' packet type wasn't independently confirmed this pass).
///
/// Exhaustively matched on `Option<DecodeScheme>`, not a blacklist: a
/// future `DecodeScheme` variant forces a compile error here instead of
/// silently reaching `decode/mod.rs`'s `unreachable!()` on real product
/// input — the same "make the unrepresentable state a compile error"
/// discipline [`DecodeScheme`]'s own doc comment asks for.
pub(crate) fn packet_family_implemented(code: i16) -> bool {
    match decode_scheme_for(code) {
        None => false,
        Some(DecodeScheme::LinearHw | DecodeScheme::FloatScale | DecodeScheme::Legacy16) => true,
        // Code 165 is packet 16 (implemented); code 177 is the same
        // decode scheme but arrives via packet 28 (XDR, u16 raw levels —
        // deferred, see the module doc). `ClassInt` alone can't tell them
        // apart; the message code can.
        Some(DecodeScheme::ClassInt) => code != 177,
        Some(DecodeScheme::Precip | DecodeScheme::Rate) => false,
    }
}

/// `(AWIPS letter, tilts)` for the 6 products with an S3-verified AWIPS-id
/// table — Phase 2's original table, unchanged. `letter` is the AWIPS id's
/// third character (`N0B` -> `B`); `tilts` are indices into
/// [`TILT_PREFIXES`].
const ALL_TILTS: &[usize] = &[0, 1, 2, 3, 4, 5];
const LOWER_TILTS: &[usize] = &[0, 1, 2];
const UPPER_TILTS: &[usize] = &[3, 4, 5];

const TILT_LETTER_TABLE: &[(u8, &[usize])] = &[
    (b'B', ALL_TILTS),   // DBZH, 153
    (b'G', LOWER_TILTS), // VRADH, 154
    (b'U', UPPER_TILTS), // VRADH, 99
    (b'X', ALL_TILTS),   // ZDR, 159
    (b'C', ALL_TILTS),   // RHOHV, 161
    (b'K', ALL_TILTS),   // KDP, 163
];

/// Tilt ordinal (0-5) for a 3-character AWIPS product id (`"N0B"` -> `0`),
/// or `None` if it isn't one of [`TILT_LETTER_TABLE`]'s verified letters —
/// deliberately narrower than [`PRODUCTS`]; see the module doc.
pub(crate) fn tilt_letter_lookup(awips_product: &[u8]) -> Option<usize> {
    if awips_product.len() != 3 {
        return None;
    }
    let (prefix, letter) = awips_product.split_at(2);
    let &(_, tilts) = TILT_LETTER_TABLE.iter().find(|(l, _)| [*l] == letter)?;
    tilts
        .iter()
        .find(|&&tilt| TILT_PREFIXES[tilt].as_bytes() == prefix)
        .copied()
}

/// Whether `message_code` is one this backend recognises at all (decodable
/// now, or deferred with a clear error) — derived from [`PRODUCTS`].
pub(crate) fn is_known_message_code(code: i16) -> bool {
    PRODUCTS.iter().any(|(c, _)| *c == code)
}

/// All known message codes, for error messages.
pub(crate) fn known_message_codes() -> Vec<u16> {
    let mut codes: Vec<u16> = PRODUCTS.iter().map(|(c, _)| *c as u16).collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

/// The full spec for a known message code.
pub(crate) fn spec_for(code: i16) -> Option<&'static ProductSpec> {
    PRODUCTS.iter().find(|(c, _)| *c == code).map(|(_, s)| s)
}

/// Decode scheme for a known message code. `None` if `code` isn't one of
/// [`PRODUCTS`]' codes.
pub(crate) fn decode_scheme_for(code: i16) -> Option<DecodeScheme> {
    spec_for(code).map(|s| s.decode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilt_letter_lookup_resolves_reflectivity_tilts() {
        assert_eq!(tilt_letter_lookup(b"N0B"), Some(0));
        assert_eq!(tilt_letter_lookup(b"NAB"), Some(1));
        assert_eq!(tilt_letter_lookup(b"N3B"), Some(5));
    }

    #[test]
    fn tilt_letter_lookup_splits_velocity_by_tilt() {
        assert_eq!(tilt_letter_lookup(b"N0G"), Some(0));
        assert_eq!(tilt_letter_lookup(b"N1G"), Some(2));
        // N0U / NBG don't exist — the split is (0,1,2)->G, (3,4,5)->U.
        assert_eq!(tilt_letter_lookup(b"N0U"), None);
        assert_eq!(tilt_letter_lookup(b"NBG"), None);
        assert_eq!(tilt_letter_lookup(b"NBU"), Some(3));
        assert_eq!(tilt_letter_lookup(b"N3U"), Some(5));
    }

    #[test]
    fn tilt_letter_lookup_resolves_dual_pol() {
        assert_eq!(tilt_letter_lookup(b"N0X"), Some(0));
        assert_eq!(tilt_letter_lookup(b"N0C"), Some(0));
        assert_eq!(tilt_letter_lookup(b"N0K"), Some(0));
    }

    #[test]
    fn tilt_letter_lookup_rejects_unverified_letters() {
        // Hydro class (165) has no S3-verified AWIPS letter in this table
        // — deliberately unresolved, not guessed.
        assert_eq!(tilt_letter_lookup(b"N0H"), None);
        assert_eq!(tilt_letter_lookup(b"XYZ"), None);
        assert_eq!(tilt_letter_lookup(b"N0"), None); // wrong length
    }

    #[test]
    fn known_message_codes_covers_all_26_products() {
        let codes = known_message_codes();
        assert_eq!(codes.len(), 26);
        assert!(codes.contains(&153));
        assert!(codes.contains(&19));
        assert!(codes.contains(&177));
    }

    #[test]
    fn decode_scheme_matches_xradar_table_for_a_sample() {
        assert_eq!(decode_scheme_for(153), Some(DecodeScheme::LinearHw));
        assert_eq!(decode_scheme_for(159), Some(DecodeScheme::FloatScale));
        assert_eq!(decode_scheme_for(19), Some(DecodeScheme::Legacy16));
        assert_eq!(decode_scheme_for(165), Some(DecodeScheme::ClassInt));
        assert_eq!(decode_scheme_for(170), Some(DecodeScheme::Precip));
        assert_eq!(decode_scheme_for(176), Some(DecodeScheme::Rate));
        assert_eq!(decode_scheme_for(9999), None);
    }

    #[test]
    fn packet_family_implemented_covers_16_and_af1f_only() {
        // packet 16
        assert!(packet_family_implemented(153));
        assert!(packet_family_implemented(165));
        // packet AF1F
        assert!(packet_family_implemented(19));
        assert!(packet_family_implemented(78));
        // deferred: precip (packet unconfirmed) and packet 28 (XDR, u16)
        assert!(!packet_family_implemented(170));
        assert!(!packet_family_implemented(176));
        assert!(!packet_family_implemented(177));
        assert!(!packet_family_implemented(9999));
    }
}

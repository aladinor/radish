//! NEXRAD Level 3 (NIDS) decode parity — value-parity against xradar's
//! `NEXRADLevel3File.raw_data`, on real fixtures, for the two product
//! families the local byte-exact oracle (`test_nexrad_level3_parity.rs`'s
//! Tier 1) cannot cover at all: it hard-rejects anything but packet 16
//! (see `docs/NEXRAD_LEVEL3_WASM.md`), so packet `AF1F` (N0S) has no
//! Tier-1 real-file coverage, and this adds two more packet-16 tilts
//! (N1B/N2B) for grid-shape diversity.
//!
//! **This is a distinct evidence tier from Tier 1**, not a stronger or
//! weaker version of it: the local oracle in `nexrad_level3.py` is a
//! from-scratch, independently authored decoder radish is checked
//! byte-for-byte against; xradar's reader is a *different* independent
//! implementation of the same ICD, used here only for the fields both
//! sides can define unambiguously — the raw per-gate byte grid
//! (`raw_data`/`raw_codes`, identical before either side applies a
//! scale or category table) and the ray-center azimuth array (both
//! readers use the same `start + width/2` convention). It does **not**
//! cross-check physical values, units, or scale/offset — xradar
//! deliberately diverges from Py-ART on some of those (see xradar#392's
//! own PR body), and reconciling that is out of scope here.
//!
//! **One documented divergence, not a bug on radish's side**: on an odd
//! `n_bins`, NEXRAD ICD 2620001AC Figure 3-11c Note 1 says packet 16 pads
//! each radial to a halfword boundary with one inert trailing byte — the
//! packet header's `n_bins` is still the true gate count. radish honors
//! this (see `decode_symbology_tolerates_pad_byte_beyond_n_bins` in
//! `decode/symbology.rs`); xradar's PR-392 branch (commit `9c8826c`)
//! does not, and reports the padded, one-wider shape instead. Confirmed
//! against real data before trusting either side: for both odd-`n_bins`
//! fixtures in this corpus (N1B, N3B — see `test_nexrad_level3_parity.rs`
//! for N3B), the "extra" byte is 0 on every single radial, and 11 other
//! real fixtures with even `n_bins` show zero header/byte-count
//! disagreement at all — a clean, ICD-explained pattern, not sampling
//! noise. `generate_expected_xradar.py` trims that documented pad column
//! before hashing, so the sidecars here already reflect the ICD-correct
//! value; no special-casing needed in the comparison below.
//!
//! **Pinned to a specific commit, not a live dependency.** The
//! `expected/*.xradar.json` sidecars are committed output — see
//! `radish/tests/fixtures/nexrad_level3/generate_expected_xradar.py`'s
//! module doc for the exact pinned commit and how to regenerate. CI
//! never builds or runs xradar's PR branch; these tests read only the
//! committed JSON.
//!
//! Also covers three real files (DPR, HHC, DAA) radish deliberately does
//! not implement (packet 28/XDR + a second categorical family) — no
//! oracle needed for those, just confirming a **real**, not synthetic,
//! product of each currently-unsupported kind rejects cleanly through
//! the public API rather than panicking or silently misdecoding. The
//! synthetic-byte version of this same check lives in
//! `test_nexrad_level3_xradar_vectors.rs`.
//!
//! Fixture-parity cases are `#[ignore]`d and skip cleanly (not fail) when
//! `RADISH_NEXRAD_LEVEL3_FIXTURE_DIR` is unset or a file is missing —
//! same discipline as `test_nexrad_level3_parity.rs`.

mod common;

use ndarray::Array2;
use radish::backends::{NexradLevel3Backend, RadarBackend};
use rstest::rstest;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use common::{fixture_path, hex32, load_expected_file};

/// Mirrors the fields `generate_expected_xradar.py` writes.
#[derive(Debug, Deserialize)]
struct XradarExpected {
    fixture: String,
    message_code: u16,
    n_radials: usize,
    n_bins: usize,
    azimuths: Vec<f32>,
    codes_sha256: String,
}

fn expected_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/nexrad_level3/expected")
}

/// Same computation as `test_nexrad_level3_parity.rs`'s `codes_sha256` —
/// duplicated rather than shared across test binaries (each integration
/// test binary compiles independently; `tests/common/mod.rs` is the
/// established sharing mechanism, and this one function isn't worth
/// promoting into it for two call sites).
fn codes_sha256(codes: &Array2<u8>) -> String {
    let flat = codes
        .as_slice()
        .expect("freshly-decoded Array2<u8> is always contiguous, row-major");
    hex32(&Sha256::digest(flat).into())
}

#[rstest]
#[case::n0s_af1f("LOT_N0S_2026_07_17_19_30_15")]
#[case::n0h_classint("LOT_N0H_2026_07_17_19_30_15")]
#[case::n1b("LOT_N1B_2026_07_17_19_30_15")]
#[case::n2b("LOT_N2B_2026_07_17_19_30_15")]
#[ignore = "needs RADISH_NEXRAD_LEVEL3_FIXTURE_DIR; see CORPUS.md"]
fn decode_matches_xradar_raw_data(#[case] name: &str) {
    let Some(path) = fixture_path("RADISH_NEXRAD_LEVEL3_FIXTURE_DIR", name) else {
        eprintln!("skipping {name}: RADISH_NEXRAD_LEVEL3_FIXTURE_DIR not set or file missing");
        return;
    };
    let expected: XradarExpected =
        load_expected_file(&expected_dir(), &format!("{name}.xradar.json"));
    assert_eq!(expected.fixture, name, "sidecar/fixture name mismatch");

    let bytes = std::fs::read(&path).expect("read fixture");
    let backend = NexradLevel3Backend::new();
    let volume = backend
        .read_bytes_volume(bytes)
        .unwrap_or_else(|e| panic!("{name}: decode failed: {e}"));
    let sweep = &volume.sweeps[0];

    let nids = sweep
        .metadata
        .nids
        .as_ref()
        .unwrap_or_else(|| panic!("{name}: sweep.metadata.nids is None"));
    assert_eq!(
        nids.message_code, expected.message_code,
        "{name}: message_code"
    );
    assert_eq!(sweep.num_rays(), expected.n_radials, "{name}: n_radials");
    assert_eq!(sweep.num_gates(), expected.n_bins, "{name}: n_bins");
    assert_eq!(
        sweep.coordinates.azimuth, expected.azimuths,
        "{name}: azimuth array diverges from xradar's raw_data-derived azimuths"
    );

    // Exactly one moment per NIDS product — grab it without assuming its
    // canonical name, since this test spans two different families.
    let (_, moment) = sweep
        .moments
        .iter()
        .next()
        .unwrap_or_else(|| panic!("{name}: no moment decoded"));
    let codes = moment
        .raw_codes
        .as_ref()
        .unwrap_or_else(|| panic!("{name}: raw_codes is None"));
    let actual_sha = codes_sha256(codes);
    assert_eq!(
        actual_sha, expected.codes_sha256,
        "{name}: codes array diverges from xradar's raw_data (SHA-256 mismatch)"
    );
}

/// Real (not synthetic) bytes for three products radish deliberately
/// does not implement — packet 28/XDR (DPR) and two categorical-family
/// products it doesn't ship a table for (HHC, DAA). Confirms the public
/// API rejects each with `RadishError::Decode`, not a panic or a
/// silently-wrong decode — the same claim
/// `test_nexrad_level3_xradar_vectors.rs`'s
/// `deferred_packet28_and_precip_codes_reject_through_the_public_backend_api`
/// makes with hand-built bytes, now against what the RPG actually emits.
#[rstest]
#[case::dpr_packet28("LOT_DPR_2026_07_17_19_30_15")]
#[case::hhc_categorical("LOT_HHC_2026_07_17_19_30_15")]
#[case::daa_accumulation("LOT_DAA_2026_07_17_19_30_15")]
#[ignore = "needs RADISH_NEXRAD_LEVEL3_FIXTURE_DIR; see CORPUS.md"]
fn unimplemented_real_products_reject_cleanly(#[case] name: &str) {
    let Some(path) = fixture_path("RADISH_NEXRAD_LEVEL3_FIXTURE_DIR", name) else {
        eprintln!("skipping {name}: RADISH_NEXRAD_LEVEL3_FIXTURE_DIR not set or file missing");
        return;
    };
    let bytes = std::fs::read(&path).expect("read fixture");
    let backend = NexradLevel3Backend::new();
    let err = backend
        .read_bytes_volume(bytes)
        .expect_err(&format!("{name}: expected a clean rejection, got Ok"));
    match err {
        radish::RadishError::Decode(_) => {}
        other => panic!("{name}: expected RadishError::Decode, got {other:?}"),
    }
}

/// Sabotage-verify for [`codes_sha256`] — same discipline as
/// `test_nexrad_level3_parity.rs`'s version, duplicated for the same
/// reason that function is (see its doc comment).
#[test]
fn sabotage_codes_sha256_mismatch_is_detected() {
    let good = Array2::from_shape_vec((2, 4), vec![2u8, 3, 4, 5, 6, 7, 8, 9]).unwrap();
    let mut sabotaged = good.clone();
    sabotaged[[0, 3]] ^= 0xFF;

    let good_hash = codes_sha256(&good);
    let sabotaged_hash = codes_sha256(&sabotaged);

    assert_ne!(
        good_hash, sabotaged_hash,
        "a single-byte perturbation must change the SHA-256 digest"
    );
}

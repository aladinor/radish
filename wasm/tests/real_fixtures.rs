//! Real-fixture confirmation for `decode_nexrad_level3`, complementing
//! `decode.rs`'s synthetic-byte suite in this same directory (see that
//! file's module doc for why both exist).
//!
//! Gated behind the `real-fixture-tests` Cargo feature (see
//! `wasm/Cargo.toml`), OFF by default — `include_bytes!` needs the
//! fixture path at COMPILE time (wasm32-unknown-unknown has no runtime
//! filesystem access, unlike the native-target Rust integration tests'
//! `RADISH_NEXRAD_LEVEL3_FIXTURE_DIR`-at-test-time convention in
//! `radish/tests/`), so a plain `cargo test -p radish-wasm --target
//! wasm32-unknown-unknown` must never require a fixture directory to even
//! compile. Run explicitly:
//!
//! ```text
//! RADISH_NEXRAD_LEVEL3_FIXTURE_DIR=~/.cache/radish/fixtures/nexrad_level3 \
//! CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!     cargo test -p radish-wasm --target wasm32-unknown-unknown \
//!         --features real-fixture-tests --test real_fixtures
//! ```
//!
//! Fixture names/hashes match `radish/tests/fixtures/CORPUS.md`'s xradar
//! cross-check corpus exactly — no new fixtures introduced, no new
//! provenance to track.

#![cfg(feature = "real-fixture-tests")]

use wasm_bindgen_test::*;

use radish_wasm::decode_nexrad_level3;

// Node is this crate's `wasm-bindgen-test` default already — see
// decode.rs's matching note.

const DAA_BYTES: &[u8] = include_bytes!(concat!(
    env!("RADISH_NEXRAD_LEVEL3_FIXTURE_DIR"),
    "/LOT_DAA_2026_07_17_19_30_15"
));
const DPR_BYTES: &[u8] = include_bytes!(concat!(
    env!("RADISH_NEXRAD_LEVEL3_FIXTURE_DIR"),
    "/LOT_DPR_2026_07_17_19_30_15"
));
const HHC_BYTES: &[u8] = include_bytes!(concat!(
    env!("RADISH_NEXRAD_LEVEL3_FIXTURE_DIR"),
    "/LOT_HHC_2026_07_17_19_30_15"
));

#[wasm_bindgen_test]
fn real_daa_decodes_precip_u8() {
    let product = decode_nexrad_level3(DAA_BYTES).expect("real DAA object should decode");
    assert_eq!(product.moment(), "ACCUM");
    assert_eq!(product.message_code(), 170);
    assert_eq!(product.codes_width(), 8);
    assert_eq!(product.n_radials(), 360);
    assert_eq!(product.n_bins(), 920);
    assert_eq!(product.codes().length(), 360 * 920);
    assert_eq!(product.codes_u16().length(), 0);
}

#[wasm_bindgen_test]
fn real_dpr_decodes_rate_u16_via_packet28() {
    let product = decode_nexrad_level3(DPR_BYTES).expect("real DPR object should decode");
    assert_eq!(product.moment(), "RATE");
    assert_eq!(product.message_code(), 176);
    assert_eq!(product.codes_width(), 16);
    assert_eq!(product.n_radials(), 360);
    assert_eq!(product.n_bins(), 920);
    assert_eq!(product.gate_spacing_m(), 250.0);
    assert_eq!(product.first_gate_m(), 125.0);
    assert_eq!(product.codes_u16().length(), 360 * 920);
    assert_eq!(product.codes().length(), 0);
    // Centred per NEXRAD ICD 2620001AC Appendix E Figure E-4 (see
    // docs/NEXRAD_LEVEL3_WASM.md §4.5/§11) — 360 radials, 1 deg apart,
    // starting at 0.5, not 0.0.
    let az = product.azimuths();
    assert!((az.to_vec()[0] - 0.5).abs() < 1e-6);
}

#[wasm_bindgen_test]
fn real_hhc_decodes_classint_via_packet16_not_packet28() {
    // The correction this pass made: HHC/177 is packet 16 (u8), not
    // packet 28 (u16) — this plan's own design doc originally assumed
    // the opposite. Locks the real-data behaviour in at the wasm
    // boundary specifically, not just the Rust-internal decode path
    // `radish/tests/test_nexrad_level3_xradar_oracle.rs` already covers.
    let product = decode_nexrad_level3(HHC_BYTES).expect("real HHC object should decode");
    assert_eq!(product.moment(), "HCLASS");
    assert_eq!(product.message_code(), 177);
    assert_eq!(product.codes_width(), 8);
    assert_eq!(product.codes().length(), 360 * 920);
    assert_eq!(product.codes_u16().length(), 0);
}

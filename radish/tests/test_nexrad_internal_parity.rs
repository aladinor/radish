//! Phase 6 parity tests: decode the corpus through both radish's
//! internal decoder and `danielway/nexrad`, then compare per-radial
//! structural counts.
//!
//! All tests in this file are `#[ignore]` so the default
//! `cargo test` doesn't pay for the bzip2 + parse round-trip.
//! Gated on `RADISH_NEXRAD_FIXTURE_DIR` — see
//! `radish/tests/fixtures/CORPUS.md`.
//!
//! Two complementary asserts:
//!
//! * **`klot_structural_parity_with_danielway_nexrad`** — pins
//!   per-sweep ray-count equality on the happy-path file
//!   (KLOT VCP-32 / 12). On a clean fixture both decoders agree.
//!
//! * **`kilx_structural_parity_with_danielway_nexrad`** — pins
//!   per-sweep ray-count equality on `KILX20230629_154426_V06`,
//!   which is the file flagged as "phantom-radial" upstream. After
//!   ICD §3.2.4.17 field-by-field analysis (monotonic timestamps,
//!   sequential azimuth_numbers, valid `radial_status=1`,
//!   `spot_blank=0`), the two "extra" radials in sweep_10 are
//!   genuinely on-wire and our decoder + danielway both correctly
//!   read all 360. xradar's 358 comes from a stride bug in its
//!   parser (`(recnum - 134) // 120` hard-codes 120 messages per
//!   LDM record, but LDM 49 of this fixture has 122 = 120 MSG_31 +
//!   2 MSG_2). Both our decoder and danielway produce the on-wire
//!   truth; xradar is the off-by-2 outlier.

#![cfg(test)]
// The `nexrad` and `nexrad-data` crates are already runtime
// dependencies of radish, so we don't need a separate dev-dep —
// just import their public types here.

use std::path::PathBuf;

fn corpus_dir() -> Option<PathBuf> {
    std::env::var_os("RADISH_NEXRAD_FIXTURE_DIR").map(PathBuf::from)
}

fn klot_path() -> Option<PathBuf> {
    let p = corpus_dir()?.join("KLOT20251210_102338_V06");
    p.is_file().then_some(p)
}

fn kilx_path() -> Option<PathBuf> {
    let p = corpus_dir()?.join("KILX20230629_154426_V06");
    p.is_file().then_some(p)
}

/// Decode the file via `danielway/nexrad`. Returns sweep + radial
/// counts so the parity check can compare against radish without
/// crossing the `pub(crate)` boundary into our `decode` module.
fn danielway_summary(path: &std::path::Path) -> (usize, Vec<usize>) {
    let bytes = std::fs::read(path).expect("read fixture");
    let file = nexrad::data::volume::File::new(bytes)
        .decompress()
        .expect("decompress");
    let scan = file.scan().expect("scan");
    let sweeps = scan.sweeps();
    let per_sweep_ray_counts = sweeps.iter().map(|s| s.radials().len()).collect();
    (sweeps.len(), per_sweep_ray_counts)
}

/// Parity: every-radial structural equality on KLOT (clean
/// fixture, no skipped azimuth_numbers). On a clean file both
/// decoders must agree on:
///
/// * number of sweeps
/// * per-sweep radial count
/// * total radial count
///
/// The deeper gate-by-gate parity lives in the inline
/// `cargo test --lib` integration tests
/// (`backends::nexrad::decode::integration_test`). That layer is
/// where we exercise our internal decoder's typed types directly;
/// here we just confirm the structural shape matches.
#[test]
#[ignore = "needs RADISH_NEXRAD_FIXTURE_DIR + danielway/nexrad parity dep"]
fn klot_structural_parity_with_danielway_nexrad() {
    let Some(path) = klot_path() else {
        eprintln!("skipping: KLOT fixture not found in RADISH_NEXRAD_FIXTURE_DIR");
        return;
    };

    let (their_sweep_count, their_rays_per_sweep) = danielway_summary(&path);
    let their_total: usize = their_rays_per_sweep.iter().sum();

    // Our decoder is reachable via the runtime path (NexradBackend),
    // since Phase 7 hasn't swapped it yet. For parity we have to
    // call radish's existing `read_volume` and count *its* radials
    // — that's also what danielway returns since they both go
    // through the upstream today.
    use radish::backends::{NexradBackend, RadarBackend};
    let backend = NexradBackend::new();
    let our = backend.read_volume(&path).expect("our read_volume");
    let our_sweep_count = our.num_sweeps();
    let our_rays_per_sweep: Vec<usize> = our
        .sweeps
        .iter()
        .map(|s| s.coordinates.azimuth.len())
        .collect();
    let our_total: usize = our_rays_per_sweep.iter().sum();

    eprintln!(
        "KLOT parity: ours={} sweeps ({} rays), danielway={} sweeps ({} rays)",
        our_sweep_count, our_total, their_sweep_count, their_total
    );

    // Note: until Phase 7 wires our internal decoder into the
    // runtime path, this test is comparing danielway-against-
    // danielway (radish's read_volume currently routes through
    // them too). It will become a real parity gate once Phase 7
    // lands.
    assert_eq!(our_sweep_count, their_sweep_count);
    assert_eq!(our_rays_per_sweep, their_rays_per_sweep);
}

/// **KILX structural parity.** Pins both decoders to the same
/// on-wire ground truth: 6840 MSG_31 records total, 360 in
/// sweep_10 (full 1° azimuth circle). ICD §3.2.4.17 field
/// analysis confirms every radial in elevation 11 is valid
/// (monotonic collection-time, sequential azimuth_number,
/// `radial_status=1`, `spot_blank=0`).
///
/// Pre-Phase-7 this is a danielway-against-danielway tautology
/// since `radish::read_volume` still routes through danielway.
/// Post-Phase-7 it becomes a real per-sweep-count parity gate:
/// our internal `decode_volume` must produce the same 6840 / 360
/// counts.
///
/// Note: xradar reports 6838 / 358 on this fixture due to a
/// stride bug in its byte walker
/// (`xradar/io/backends/nexrad_level2.py:397`), where the formula
/// `(recnum - 134) // 120` hard-codes 120 messages per LDM
/// record. LDM record 49 of this fixture contains 122 messages
/// (120 MSG_31 + 2 MSG_2), so xradar drops 2 valid MSG_31s at
/// the LDM boundary. Both danielway and our decoder correctly
/// walk all 122.
#[test]
#[ignore = "needs RADISH_NEXRAD_FIXTURE_DIR"]
fn kilx_structural_parity_with_danielway_nexrad() {
    let Some(path) = kilx_path() else {
        eprintln!("skipping: KILX fixture not found in RADISH_NEXRAD_FIXTURE_DIR");
        return;
    };

    let (their_sweep_count, their_rays_per_sweep) = danielway_summary(&path);
    let their_total: usize = their_rays_per_sweep.iter().sum();

    eprintln!(
        "KILX danielway: {} sweeps, {} total rays, sweep_10 has {} rays",
        their_sweep_count,
        their_total,
        their_rays_per_sweep.get(10).copied().unwrap_or(0)
    );

    // Pin the ground-truth on-wire counts. If danielway ever
    // changes (or our decoder regresses) this assertion catches
    // it.
    assert_eq!(their_total, 6840, "expected 6840 on-wire MSG_31 records");
    assert_eq!(
        their_rays_per_sweep[10], 360,
        "expected 360 rays in sweep_10 (full 1° circle)"
    );

    use radish::backends::{NexradBackend, RadarBackend};
    let backend = NexradBackend::new();
    let our = backend.read_volume(&path).expect("our read_volume");
    let our_sweep_count = our.num_sweeps();
    let our_rays_per_sweep: Vec<usize> = our
        .sweeps
        .iter()
        .map(|s| s.coordinates.azimuth.len())
        .collect();

    assert_eq!(our_sweep_count, their_sweep_count);
    assert_eq!(our_rays_per_sweep, their_rays_per_sweep);
}

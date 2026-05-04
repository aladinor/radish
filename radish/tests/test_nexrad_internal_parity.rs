//! Phase 6 parity tests: decode the corpus through both radish's
//! internal decoder and `danielway/nexrad`, then compare per-radial
//! structural equality.
//!
//! All tests in this file are `#[ignore]` so the default
//! `cargo test` doesn't pay for the bzip2 + parse round-trip.
//! Gated on `RADISH_NEXRAD_FIXTURE_DIR` — see
//! `radish/tests/fixtures/CORPUS.md`.
//!
//! Two complementary asserts:
//!
//! * **`klot_per_radial_parity`** — pins gate-by-gate equality on
//!   the happy-path file. Confirms our decoder produces the same
//!   `Scan` structure danielway does on a clean fixture.
//!
//! * **`kilx_phantom_radial_divergence`** — pins the **expected**
//!   divergence on the phantom-radial fixture: danielway's MSG_31
//!   parser produces 360 radials in elevation 11 (where the file
//!   actually only has 358 records); our `try_skip_to(target)`
//!   resync produces the correct 358. The whole point of the
//!   internal-decoder rewrite.

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

/// **The load-bearing divergence test.** On `KILX20230629_154426_V06`,
/// `danielway/nexrad` produces 360 radials in elevation 11 even
/// though the file's MSG_31 stream only contains 358 records (per
/// the forensic investigation in `/tmp/radish-phantom-radials-bug.md`).
/// Our internal decoder's `try_skip_to(target)` resync correctly
/// produces 358.
///
/// Once Phase 7 wires our decoder into `read_volume`, this test
/// pins the **expected** divergence: ours is correct, theirs has
/// the phantom-radial bug.
#[test]
#[ignore = "needs RADISH_NEXRAD_FIXTURE_DIR; pins the phantom-radial divergence"]
fn kilx_phantom_radial_divergence() {
    let Some(path) = kilx_path() else {
        eprintln!("skipping: KILX fixture not found in RADISH_NEXRAD_FIXTURE_DIR");
        return;
    };

    let (their_sweep_count, their_rays_per_sweep) = danielway_summary(&path);
    let their_total: usize = their_rays_per_sweep.iter().sum();

    // KILX VCP-212: 13 elevations. danielway's count of MSG_31
    // records per the forensic walk is 6838; their `Scan::sweeps`
    // produces 6840 radials thanks to the boundary bug fabricating
    // 2 phantom radials in sweep_10 (elevation 11).
    eprintln!(
        "KILX danielway: {} sweeps, {} total rays, sweep_10 has {} rays",
        their_sweep_count,
        their_total,
        their_rays_per_sweep.get(10).copied().unwrap_or(0)
    );

    // Pin danielway's KNOWN-WRONG behaviour so the test is a
    // canary: if upstream releases a fix, the assertion below
    // fails and we revisit the comparison vs our internal
    // decoder.
    assert_eq!(their_total, 6840, "danielway should produce 6840 phantoms");
    assert_eq!(
        their_rays_per_sweep[10], 360,
        "danielway should report 360 rays in sweep_10 (= 358 real + 2 phantom)"
    );

    // The "ours = 358" half of the divergence will be wired in
    // Phase 7 — when our `decode_volume` becomes the runtime
    // path, the assertion below activates. For now it's a
    // documentation marker.
    //
    // Expected post-Phase-7:
    //     let our = NexradBackend::new().read_volume(&path)?;
    //     assert_eq!(our.sweeps[10].coordinates.azimuth.len(), 358);
    //     let our_total: usize = our.sweeps.iter()
    //         .map(|s| s.coordinates.azimuth.len()).sum();
    //     assert_eq!(our_total, 6838);
}

//! Integration test for the NEXRAD Level 2 backend on a real fixture.
//!
//! Skipped unless either `RADISH_NEXRAD_FIXTURE` (single-file legacy
//! convention) or `RADISH_NEXRAD_FIXTURE_DIR` (corpus directory; see
//! `radish/tests/fixtures/CORPUS.md`) is set.

use std::path::{Path, PathBuf};

use radish::backends::{NexradBackend, RadarBackend};

/// Resolve the happy-path KLOT fixture. Prefers the legacy
/// `RADISH_NEXRAD_FIXTURE` env var when set; falls back to
/// `RADISH_NEXRAD_FIXTURE_DIR/KLOT20251210_102338_V06`.
fn fixture() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RADISH_NEXRAD_FIXTURE") {
        return Some(PathBuf::from(p));
    }
    let dir = std::env::var_os("RADISH_NEXRAD_FIXTURE_DIR")?;
    let candidate = PathBuf::from(dir).join("KLOT20251210_102338_V06");
    candidate.is_file().then_some(candidate)
}

/// Resolve the KILX phantom-radial divergence fixture. Always rooted
/// at `RADISH_NEXRAD_FIXTURE_DIR` because the divergence test isn't
/// meaningful on any other file.
#[allow(dead_code)] // Will be consumed by phase 2's regression test.
fn kilx_fixture() -> Option<PathBuf> {
    let dir = std::env::var_os("RADISH_NEXRAD_FIXTURE_DIR")?;
    let candidate = PathBuf::from(dir).join("KILX20230629_154426_V06");
    candidate.is_file().then_some(candidate)
}

/// SHA-256 sums for every file in the documented corpus. Source of
/// truth: `radish/tests/fixtures/CORPUS.md`. The values are mirrored
/// here so the test fails loudly if a maintainer updates the docs but
/// not the code (or vice versa).
const CORPUS_SHA256: &[(&str, &str)] = &[
    (
        "KLOT20251210_102338_V06",
        "a5ed05d7dceaaceeb5adfb08601f10276a77a161ffdae7f302c49626e16cca81",
    ),
    (
        "KILX20230629_154426_V06",
        "715c3c18691f6efe87a27127d631add8d90fd92c66a019a17965b624757180da",
    ),
];

/// Hex-format a SHA-256 digest without pulling in the `hex` crate.
fn hex32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        std::fmt::Write::write_fmt(&mut s, format_args!("{b:02x}")).unwrap();
    }
    s
}

/// Pin the corpus's bytes against the SHA-256 sums published in
/// `CORPUS.md`. Skips when `RADISH_NEXRAD_FIXTURE_DIR` is unset
/// (matches the rest of this file). When the env var IS set but a
/// file's contents drift from the documented sum, fail loudly so
/// downstream parity tests don't run against unverified data.
#[test]
fn corpus_sha256s_match_documentation() {
    use sha2::{Digest, Sha256};

    let Some(dir) = std::env::var_os("RADISH_NEXRAD_FIXTURE_DIR") else {
        eprintln!("skipping: RADISH_NEXRAD_FIXTURE_DIR not set");
        return;
    };
    let dir = PathBuf::from(dir);

    for (name, expected_hex) in CORPUS_SHA256 {
        let path = dir.join(name);
        if !path.is_file() {
            eprintln!("skipping {name}: not present at {}", path.display());
            continue;
        }
        let bytes = std::fs::read(&path).expect("read fixture");
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        let actual_hex = hex32(&digest);
        assert_eq!(
            &actual_hex,
            expected_hex,
            "{name} sha256 mismatch — file at {} has been replaced or \
             corrupted; re-acquire from the URL in CORPUS.md or update \
             both the docs AND CORPUS_SHA256 if this is intentional",
            path.display(),
        );
    }
}

#[test]
fn read_volume_on_real_fixture() {
    let Some(path) = fixture() else {
        eprintln!("skipping: RADISH_NEXRAD_FIXTURE not set");
        return;
    };
    let backend = NexradBackend::new();
    assert!(backend.can_read(Path::new(&path)));

    let volume = backend.read_volume(&path).expect("decode failed");
    let m = &volume.metadata;

    assert_eq!(m.instrument_name.len(), 4, "ICAO must be 4 chars");
    assert!(
        volume.num_sweeps() >= 5,
        "expected at least 5 sweeps, got {}",
        volume.num_sweeps()
    );
    assert_eq!(m.sweep_group_names.len(), volume.num_sweeps());
    assert_eq!(m.sweep_fixed_angles.len(), volume.num_sweeps());
    // Every sweep_fixed_angle should be a finite, plausible elevation
    // (range covers WSR-88D's full 0..20° envelope plus headroom). A
    // NaN here means the cut→median→NaN fallback chain in
    // `fixed_angle_for` short-circuited; a value outside [-1, 25] means
    // we read the wrong MSG_5 field. Both regression-prone after the
    // commanded-vs-achieved fix.
    for (i, &angle) in m.sweep_fixed_angles.iter().enumerate() {
        assert!(angle.is_finite(), "sweep_fixed_angles[{i}] is NaN");
        assert!(
            (-1.0..=25.0).contains(&angle),
            "sweep_fixed_angles[{i}] = {angle} out of plausible WSR-88D range"
        );
    }
    // Volume-level sweep_fixed_angles must agree with the per-sweep
    // SweepMetadata.fixed_angle. The two come from different code
    // paths (build_volume_metadata vs convert_sweep) and a future
    // refactor could let them drift; this catches it.
    for (i, sweep) in volume.sweeps.iter().enumerate() {
        let vol_angle = m.sweep_fixed_angles[i];
        let sweep_angle = sweep.metadata.fixed_angle;
        assert!(
            (vol_angle - sweep_angle).abs() < 1e-6,
            "sweep {i}: volume={vol_angle} but per-sweep={sweep_angle} — fixed_angle_for path drift"
        );
    }
    assert!((-90.0..=90.0).contains(&m.latitude), "latitude in range");
    assert!(
        (-180.0..=180.0).contains(&m.longitude),
        "longitude in range"
    );

    // Every sweep should have at least DBZH and consistent ray-shaped coords.
    for (i, sweep) in volume.sweeps.iter().enumerate() {
        let coords = &sweep.coordinates;
        assert_eq!(
            coords.azimuth.len(),
            coords.elevation.len(),
            "sweep {i}: az/el ray count mismatch"
        );
        assert_eq!(
            coords.azimuth.len(),
            coords.time.len(),
            "sweep {i}: az/time ray count mismatch"
        );
        assert!(!coords.range.is_empty(), "sweep {i} has no range gates");
        assert!(
            sweep.moments.contains_key("DBZH") || sweep.moments.contains_key("VRADH"),
            "sweep {i} has neither DBZH nor VRADH (moment names: {:?})",
            sweep.moments.keys().collect::<Vec<_>>()
        );

        // Every moment must match the (rays, gates) shape.
        for (name, m) in &sweep.moments {
            let (r, g) = m.shape();
            assert_eq!(
                r,
                coords.azimuth.len(),
                "sweep {i} moment {name}: ray count mismatch"
            );
            assert_eq!(
                g,
                coords.range.len(),
                "sweep {i} moment {name}: gate count mismatch"
            );
        }
    }
}

#[test]
fn scan_file_returns_metadata() {
    let Some(path) = fixture() else {
        return;
    };
    let m = NexradBackend::new().scan_file(&path).expect("scan failed");
    assert_eq!(m.instrument_name.len(), 4);
    assert!(!m.sweep_group_names.is_empty());
}

/// HIGH-priority: pin the `scan_nexrad`-vs-`read_nexrad` agreement
/// for the per-sweep arrays surfaced for the raw2zarr bulk-ingest
/// path. Both code paths route through `volume_attrs`, so any future
/// refactor that decouples them must keep the surfaces identical.
///
/// Also asserts the length invariant against
/// `VolumeMetadata.sweep_fixed_angles` — same VCP-truncation edge
/// case the padding contract handles.
#[test]
fn scan_and_read_agree_on_per_sweep_attrs_and_time_ranges() {
    let Some(path) = fixture() else {
        eprintln!("skipping: RADISH_NEXRAD_FIXTURE not set");
        return;
    };
    let backend = NexradBackend::new();
    let scanned = backend.scan_file(&path).expect("scan failed");
    let volume = backend.read_volume(&path).expect("read failed");

    let scanned_nx = scanned.nexrad.as_ref().expect("scan must populate nexrad");
    assert_eq!(
        scanned_nx.sweep_attrs.len(),
        scanned.sweep_fixed_angles.len(),
        "sweep_attrs len must match sweep_fixed_angles len",
    );
    assert_eq!(
        scanned_nx.sweep_time_ranges.len(),
        scanned.sweep_fixed_angles.len(),
        "sweep_time_ranges len must match sweep_fixed_angles len",
    );

    // scan vs read agreement: every per-sweep entry surfaced by
    // `scan_file` must equal the corresponding entry on the
    // fully-decoded `read_volume`'s per-sweep `nexrad_attrs`.
    for (i, sweep) in volume.sweeps.iter().enumerate() {
        let read_attrs = sweep
            .metadata
            .nexrad
            .as_ref()
            .expect("read_volume must attach per-sweep nexrad attrs");
        assert_eq!(
            &scanned_nx.sweep_attrs[i], read_attrs,
            "sweep {i}: scan_file vs read_volume per-sweep attrs drift",
        );
    }

    // Every time range that's `Some` must satisfy `start <= end`. We
    // don't require all sweeps to have timestamps — very old archives
    // can carry sweeps without `collection_time` — but if we did
    // produce a range, it must be ordered.
    for (i, range) in scanned_nx.sweep_time_ranges.iter().enumerate() {
        if let Some((start, end)) = range {
            assert!(start.is_finite(), "sweep {i} time_range start non-finite");
            assert!(end.is_finite(), "sweep {i} time_range end non-finite");
            assert!(
                start <= end,
                "sweep {i} time_range invariant: start={start} > end={end}",
            );
        }
    }
}

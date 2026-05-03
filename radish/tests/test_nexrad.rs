//! Integration test for the NEXRAD Level 2 backend on a real fixture.
//!
//! Skipped unless `RADISH_NEXRAD_FIXTURE` points at an Archive II file.

use std::path::Path;

use radish::backends::{NexradBackend, RadarBackend};

fn fixture() -> Option<std::path::PathBuf> {
    std::env::var_os("RADISH_NEXRAD_FIXTURE").map(Into::into)
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

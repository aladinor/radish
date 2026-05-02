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

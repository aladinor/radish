"""End-to-end and parity tests for the NEXRAD Level 2 backend.

These tests are gated on `RADISH_NEXRAD_FIXTURE`; see `conftest.py`.
"""

import numpy as np
import pytest

import radish


def test_read_nexrad_returns_volume(nexrad_fixture):
    vol = radish.read_nexrad(nexrad_fixture)
    assert vol.num_sweeps >= 5
    md = vol.metadata
    assert len(md.instrument_name) == 4
    assert -90 <= md.latitude <= 90
    assert -180 <= md.longitude <= 180
    # Moments mapping: at least one of DBZH/VRADH should appear in any sweep.
    s0 = vol.get_sweep(0)
    assert s0.num_rays > 0
    assert s0.num_gates > 0
    assert "DBZH" in s0.moment_names() or "VRADH" in s0.moment_names()


def test_scan_nexrad_returns_metadata(nexrad_fixture):
    md = radish.scan_nexrad(nexrad_fixture)
    assert len(md.instrument_name) == 4
    assert md.num_sweeps >= 5


def test_xarray_engine_radish_dispatches_to_nexrad(nexrad_fixture):
    xr = pytest.importorskip("xarray")
    dt = xr.open_datatree(nexrad_fixture, engine="radish")
    sweep_keys = [k for k in dt.children if k.startswith("sweep_")]
    assert len(sweep_keys) >= 5
    s0 = dt[sweep_keys[0]]
    assert "DBZH" in s0.data_vars or "VRADH" in s0.data_vars
    assert "azimuth" in s0.coords
    assert "elevation" in s0.coords
    assert "range" in s0.coords
    # Root attrs should advertise xradar-style provenance for NEXRAD.
    root_attrs = dt.attrs
    assert root_attrs.get("source", "").startswith("NEXRAD")


def test_radish_matches_xradar_per_moment(nexrad_fixture):
    """Acceptance gate: numerically match xradar within tight tolerance."""
    xr = pytest.importorskip("xarray")
    xradar = pytest.importorskip("xradar")

    rd = xr.open_datatree(nexrad_fixture, engine="radish")
    xd = xradar.io.open_nexradlevel2_datatree(nexrad_fixture)

    rd_keys = sorted(k for k in rd.children if k.startswith("sweep_"))
    xd_keys = sorted(k for k in xd.children if k.startswith("sweep_"))
    common = [k for k in rd_keys if k in xd_keys]
    assert common, f"no common sweep groups: rd={rd_keys} xd={xd_keys}"

    # xradar represents below-threshold/no-data with negative sentinels (e.g.
    # DBZH=-33.0) while radish (via nexrad-decode) uses NaN. Both are valid
    # representations; we only verify that the *valid-gate* physical values
    # agree. Per-moment plausibility ranges below filter the sentinels.
    plausibility = {
        "DBZH": (-32.5, 95.5),
        "VRADH": (-100.0, 100.0),
        "WRADH": (0.0, 30.0),
        "ZDR": (-7.875, 7.9375),
        "PHIDP": (-180.0, 360.0),
        "RHOHV": (0.2, 1.05),
    }
    # Align by azimuth bin so ray-count mismatches (e.g. AVSET truncation or
    # one reader dropping a duplicate ray) become explicit and the comparison
    # still happens on the rays that exist in both. Tolerance is half the
    # super-res spacing (0.25°) so each radish ray maps to its nearest xradar
    # ray with no ambiguity at 0.5° spacing.
    AZ_TOL = 0.25
    compared = 0
    skipped_rays_total = 0
    for k in common:
        rd_ds = rd[k]
        xd_ds = xd[k]
        rd_az = np.asarray(rd_ds["azimuth"].values, dtype=np.float64)
        xd_az = np.asarray(xd_ds["azimuth"].values, dtype=np.float64)

        # Both should already be azimuth-sorted, but don't rely on it.
        rd_order = np.argsort(rd_az)
        xd_order = np.argsort(xd_az)
        rd_az_s = rd_az[rd_order]
        xd_az_s = xd_az[xd_order]

        # For each radish ray, find the closest xradar ray. Mark unmatched.
        idx = np.searchsorted(xd_az_s, rd_az_s)
        idx_lo = np.clip(idx - 1, 0, len(xd_az_s) - 1)
        idx_hi = np.clip(idx, 0, len(xd_az_s) - 1)
        d_lo = np.abs(xd_az_s[idx_lo] - rd_az_s)
        d_hi = np.abs(xd_az_s[idx_hi] - rd_az_s)
        nearest = np.where(d_lo <= d_hi, idx_lo, idx_hi)
        nearest_dist = np.minimum(d_lo, d_hi)
        rd_keep = nearest_dist <= AZ_TOL
        xd_keep_idx = nearest[rd_keep]
        skipped = int((~rd_keep).sum()) + max(0, len(xd_az_s) - len(xd_keep_idx))
        skipped_rays_total += skipped

        # If we couldn't match anything, that's a real failure for this sweep.
        assert rd_keep.any(), (
            f"sweep {k}: no rays matched within {AZ_TOL}° "
            f"(rd n={len(rd_az_s)}, xd n={len(xd_az_s)})"
        )
        # Allow up to 1% of rays to be unmatched (AVSET / dropped-duplicate edge
        # cases); flag larger discrepancies.
        max_unmatched = max(1, int(0.01 * max(len(rd_az_s), len(xd_az_s))))
        n_unmatched = int((~rd_keep).sum()) + (len(xd_az_s) - len(xd_keep_idx))
        assert n_unmatched <= max_unmatched, (
            f"sweep {k}: {n_unmatched} unmatched rays exceeds {max_unmatched} "
            f"(rd n={len(rd_az_s)}, xd n={len(xd_az_s)})"
        )

        # Sanity check: matched azimuths agree.
        np.testing.assert_allclose(
            rd_az_s[rd_keep], xd_az_s[xd_keep_idx], atol=AZ_TOL,
            err_msg=f"sweep {k}: matched azimuths disagree"
        )

        for m, (lo, hi) in plausibility.items():
            if m not in rd_ds.data_vars or m not in xd_ds.data_vars:
                continue
            rd_arr = np.asarray(rd_ds[m].values, dtype=np.float64)[rd_order][rd_keep]
            xd_arr = np.asarray(xd_ds[m].values, dtype=np.float64)[xd_order][xd_keep_idx]
            # Gate count may differ between readers (super-res REF in some
            # sweeps): compare on the overlap.
            if rd_arr.shape != xd_arr.shape:
                n_gates = min(rd_arr.shape[1], xd_arr.shape[1])
                rd_arr = rd_arr[:, :n_gates]
                xd_arr = xd_arr[:, :n_gates]
            # Mask anything either side considers "missing" (NaN OR outside the
            # plausibility window). Compare only on the intersection.
            valid = (
                np.isfinite(rd_arr) & np.isfinite(xd_arr)
                & (rd_arr >= lo) & (rd_arr <= hi)
                & (xd_arr >= lo) & (xd_arr <= hi)
            )
            if not valid.any():
                continue
            np.testing.assert_allclose(
                rd_arr[valid], xd_arr[valid], atol=1e-3,
                err_msg=(
                    f"sweep {k} moment {m}: valid gates disagree "
                    f"({valid.sum()} compared)"
                )
            )
            compared += 1
    assert compared > 0, "expected to compare at least one moment"
    print(
        f"\nparity: {compared} (sweep,moment) pairs verified, "
        f"{skipped_rays_total} unmatched rays skipped across {len(common)} sweeps"
    )


def test_engine_radish_still_handles_cfradial1():
    """Regression: radish engine should still detect/handle .nc files."""
    from radish.backends.xarray_backend import _detect_format
    assert _detect_format("foo.nc") == "cfradial1"
    assert _detect_format("foo.nc4") == "cfradial1"
    assert _detect_format("foo.netcdf") == "cfradial1"
    assert _detect_format("KLOT20260310_231412_V06") == "nexrad"
    assert _detect_format("foo.ar2v") == "nexrad"
    assert _detect_format("foo.txt") is None

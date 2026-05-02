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
    # Root identifies the radar (4-char ICAO) and the scan strategy.
    assert len(dt.attrs.get("instrument_name", "")) == 4
    assert dt.attrs.get("scan_name", "").startswith("VCP-")


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


def test_read_nexrad_chunks_round_trips_full_file(nexrad_fixture):
    """Splitting the fixture into N byte chunks and feeding them to
    `read_nexrad_chunks` must reconstruct the same VolumeData as the
    path-based read. This is the core invariant of the chunks API: any
    in-buffer split round-trips via concatenation."""
    with open(nexrad_fixture, "rb") as f:
        data = f.read()
    n = len(data)
    chunks = [data[: n // 4], data[n // 4 : n // 2], data[n // 2 : 3 * n // 4], data[3 * n // 4 :]]

    v_path = radish.read_nexrad(nexrad_fixture)
    v_chunks = radish.read_nexrad_chunks(chunks)

    assert v_path.metadata.instrument_name == v_chunks.metadata.instrument_name
    assert v_path.num_sweeps == v_chunks.num_sweeps
    # MSG_2 / MSG_5 attrs must be identical — that's the surface most
    # likely to silently degrade if the chunked decode dropped a record.
    a, b = v_path.metadata.nexrad_attrs, v_chunks.metadata.nexrad_attrs
    assert a.dynamic_scan_type == b.dynamic_scan_type
    assert a.rda_build_number == b.rda_build_number
    assert a.actual_elevation_cuts == b.actual_elevation_cuts
    assert a.avset_enabled == b.avset_enabled


def test_open_nexrad_chunks_datatree_accepts_paths(nexrad_fixture):
    """Path-like entries in the chunks list should be read eagerly and the
    resulting DataTree must match the regular engine path."""
    import xarray as xr
    dt_chunks = radish.open_nexrad_chunks_datatree([nexrad_fixture])
    dt_file = xr.open_datatree(nexrad_fixture, engine="radish")
    assert sorted(dt_chunks.children) == sorted(dt_file.children)
    assert sorted(dt_chunks.attrs.keys()) == sorted(dt_file.attrs.keys())
    # Per-sweep dim sizes must agree.
    for skey in dt_file.children:
        if not skey.startswith("sweep_"):
            continue
        assert dict(dt_chunks[skey].sizes) == dict(dt_file[skey].sizes)


def test_open_nexrad_chunks_datatree_accepts_bytes(nexrad_fixture):
    """`bytes` entries should be passed through verbatim. We split the
    fixture into two equal halves so the concatenated buffer is identical
    to the original file."""
    import xarray as xr
    with open(nexrad_fixture, "rb") as f:
        data = f.read()
    n = len(data)
    dt_chunks = radish.open_nexrad_chunks_datatree([data[: n // 2], data[n // 2 :]])
    dt_file = xr.open_datatree(nexrad_fixture, engine="radish")
    assert sorted(dt_chunks.children) == sorted(dt_file.children)
    assert dt_chunks.attrs.get("scan_name") == dt_file.attrs.get("scan_name")


def test_engine_radish_still_handles_cfradial1():
    """Regression: radish engine should still detect/handle .nc files."""
    from radish.backends.xarray_backend import _detect_format
    assert _detect_format("foo.nc") == "cfradial1"
    assert _detect_format("foo.nc4") == "cfradial1"
    assert _detect_format("foo.netcdf") == "cfradial1"
    assert _detect_format("KLOT20260310_231412_V06") == "nexrad"
    assert _detect_format("foo.ar2v") == "nexrad"
    assert _detect_format("foo.txt") is None


def test_sweep_emits_fm301_scalar_variables(nexrad_fixture):
    """CfRadial2 / WMO FM301 spec: each sweep group must carry sweep_mode,
    sweep_number, sweep_fixed_angle, prt_mode, and follow_mode as 0-d
    variables (NOT attributes). Pins the structural contract so downstream
    CfRadial2 validators don't reject the output."""
    xr = pytest.importorskip("xarray")
    dt = xr.open_datatree(nexrad_fixture, engine="radish")
    for sweep_key in (k for k in dt.children if k.startswith("sweep_")):
        s = dt[sweep_key]
        for var in ("sweep_mode", "sweep_number", "sweep_fixed_angle", "prt_mode", "follow_mode"):
            assert var in s.data_vars, f"{sweep_key}: missing FM301 var {var!r}"
            assert s[var].ndim == 0, f"{sweep_key}: {var!r} must be 0-d"
            if var == "sweep_mode":
                assert str(s[var].values) == "azimuth_surveillance"
            elif var == "prt_mode":
                assert str(s[var].values) in {"fixed", "staggered", "dual", "not_set"}
            elif var == "follow_mode":
                assert str(s[var].values) in {
                    "none", "sun", "vehicle", "aircraft", "target", "manual", "not_set",
                }
        # Old attribute-based form must NOT come back.
        assert "fixed_angle" not in s.attrs, f"{sweep_key}: fixed_angle attr leaked"
        assert "sweep_number" not in s.attrs, f"{sweep_key}: sweep_number attr leaked"


def test_root_emits_msg2_msg5_attrs(nexrad_fixture):
    """Phase B: every MSG_2 / MSG_5 root attr xradar emits is on radish's root,
    with the same Python type (str / int / float / bool — never numpy scalars)."""
    xr = pytest.importorskip("xarray")
    dt = xr.open_datatree(nexrad_fixture, engine="radish")

    expected_types = {
        "dynamic_scan_type": str,
        "mpda_vcp": bool,
        "base_tilt_vcp": bool,
        "num_base_tilts": int,
        "vcp_truncated": bool,
        "vcp_sequence_active": bool,
        "number_elevation_cuts": int,
        "doppler_velocity_resolution": float,
        "vcp_pulse_width": str,
        "avset_enabled": bool,
        "ebc_enabled": bool,
        "super_res_status": int,
        "rda_build_number": int,
        "operational_mode": int,
        "actual_elevation_cuts": int,
    }
    for key, ty in expected_types.items():
        assert key in dt.attrs, f"missing root attr {key!r}"
        # `bool` must be checked first because `isinstance(True, int)` is True.
        actual = dt.attrs[key]
        if ty is bool:
            assert isinstance(actual, bool), f"{key!r}: expected bool, got {type(actual)}"
        elif ty is int:
            assert isinstance(actual, int) and not isinstance(actual, bool), (
                f"{key!r}: expected int, got {type(actual)}"
            )
        elif ty is float:
            assert isinstance(actual, float), f"{key!r}: expected float, got {type(actual)}"
        else:
            assert isinstance(actual, ty), f"{key!r}: expected {ty}, got {type(actual)}"

    # `actual_elevation_cuts` must equal the number of decoded sweeps; xradar
    # uses the same definition.
    n_sweeps = sum(1 for k in dt.children if k.startswith("sweep_"))
    assert dt.attrs["actual_elevation_cuts"] == n_sweeps


def test_sweep_emits_msg5_per_cut_attrs(nexrad_fixture):
    """Phase B: every MSG_5 elevation-cut attr xradar emits per sweep is on
    every radish sweep group with the right Python type."""
    xr = pytest.importorskip("xarray")
    dt = xr.open_datatree(nexrad_fixture, engine="radish")

    expected_types = {
        "waveform_type": str,
        "channel_config": str,
        "super_resolution": int,
        "sails_cut": bool,
        "sails_sequence_number": int,
        "mrle_cut": bool,
        "mrle_sequence_number": int,
        "mpda_cut": bool,
        "base_tilt_cut": bool,
    }
    valid_waveforms = {
        "contiguous_surveillance", "contiguous_doppler",
        "batch", "staggered_pulse_pair", "not_applicable",
    }
    valid_channels = {"constant_phase", "random_phase", "sz2_phase_coding"}

    for sweep_key in (k for k in dt.children if k.startswith("sweep_")):
        s = dt[sweep_key]
        for key, ty in expected_types.items():
            assert key in s.attrs, f"{sweep_key}: missing attr {key!r}"
            actual = s.attrs[key]
            if ty is bool:
                assert isinstance(actual, bool), (
                    f"{sweep_key}.{key!r}: expected bool, got {type(actual)}"
                )
            elif ty is int:
                assert isinstance(actual, int) and not isinstance(actual, bool), (
                    f"{sweep_key}.{key!r}: expected int, got {type(actual)}"
                )
            else:
                assert isinstance(actual, ty), (
                    f"{sweep_key}.{key!r}: expected {ty}, got {type(actual)}"
                )
        assert s.attrs["waveform_type"] in valid_waveforms
        assert s.attrs["channel_config"] in valid_channels


def test_root_attrs_match_xradar(nexrad_fixture):
    """Phase B acceptance gate: the MSG_2 / MSG_5 root attrs we emit equal
    xradar's values verbatim."""
    pytest.importorskip("xarray")
    xradar = pytest.importorskip("xradar")
    import xarray as xr  # noqa: E402

    rd = xr.open_datatree(nexrad_fixture, engine="radish")
    xd = xradar.io.open_nexradlevel2_datatree(nexrad_fixture)

    keys = (
        "dynamic_scan_type", "mpda_vcp", "base_tilt_vcp", "num_base_tilts",
        "vcp_truncated", "vcp_sequence_active", "number_elevation_cuts",
        "doppler_velocity_resolution", "vcp_pulse_width", "avset_enabled",
        "ebc_enabled", "super_res_status", "rda_build_number",
        "operational_mode", "actual_elevation_cuts",
    )
    for k in keys:
        if k not in xd.attrs:
            # xradar may omit a key when the source field is zero — skip.
            continue
        rd_v, xd_v = rd.attrs[k], xd.attrs[k]
        assert rd_v == xd_v, f"root attr {k!r}: radish={rd_v!r} xradar={xd_v!r}"


def test_sweep_attrs_match_xradar(nexrad_fixture):
    """Phase B acceptance gate: per-sweep MSG_5 attrs match xradar's values
    on the sweeps both readers produce."""
    pytest.importorskip("xarray")
    xradar = pytest.importorskip("xradar")
    import xarray as xr  # noqa: E402

    rd = xr.open_datatree(nexrad_fixture, engine="radish")
    xd = xradar.io.open_nexradlevel2_datatree(nexrad_fixture)

    rd_keys = sorted(k for k in rd.children if k.startswith("sweep_"))
    xd_keys = sorted(k for k in xd.children if k.startswith("sweep_"))
    common = [k for k in rd_keys if k in xd_keys]
    assert common, "no sweep groups in common"

    keys = (
        "waveform_type", "channel_config", "super_resolution",
        "sails_cut", "sails_sequence_number",
        "mrle_cut", "mrle_sequence_number",
        "mpda_cut", "base_tilt_cut",
    )
    for sweep_key in common:
        rs, xs = rd[sweep_key], xd[sweep_key]
        for k in keys:
            if k not in xs.attrs:
                continue
            rd_v, xd_v = rs.attrs[k], xs.attrs[k]
            assert rd_v == xd_v, (
                f"{sweep_key}.{k!r}: radish={rd_v!r} xradar={xd_v!r}"
            )


def test_radish_matches_xradar_structure(nexrad_fixture):
    """Pin the structural parity with `xradar.io.open_nexradlevel2_datatree`:
    same dim names, same coord set + dtypes, same per-DataArray CF attrs
    (units / standard_name / long_name), same root data_vars, same
    variable set per sweep. Numeric values may differ within tolerance
    (different missing-data sentinels, etc.) — that's covered by the
    parity test above. Sweep-level attrs (waveform_type, etc.) and the
    15 MSG_2/MSG_5 root attrs are out of scope here; they need the
    lower-level `nexrad-decode` API and land in a follow-up commit.
    """
    xr = pytest.importorskip("xarray")
    xradar = pytest.importorskip("xradar")

    rd = xr.open_datatree(nexrad_fixture, engine="radish")
    xd = xradar.io.open_nexradlevel2_datatree(nexrad_fixture)

    # Root data_vars set + dtypes match.
    assert set(rd.data_vars) == set(xd.data_vars)
    for v in rd.data_vars:
        assert rd[v].dtype == xd[v].dtype, f"root[{v}] dtype mismatch: {rd[v].dtype} vs {xd[v].dtype}"

    rd_keys = sorted(k for k in rd.children if k.startswith("sweep_"))
    xd_keys = sorted(k for k in xd.children if k.startswith("sweep_"))
    assert rd_keys == xd_keys, "sweep group keys differ"

    for k in rd_keys:
        rs, xs = rd[k], xd[k]
        # Dim names match exactly. Per-sweep dim *lengths* may differ by a
        # few rays on AVSET-truncated sweeps (xradar drops to expected cut
        # count; radish keeps everything decoded). Allow that.
        assert set(rs.sizes) == set(xs.sizes), f"{k}: dim names differ"
        for d in rs.sizes:
            rd_n, xd_n = rs.sizes[d], xs.sizes[d]
            assert abs(rd_n - xd_n) <= max(3, int(0.01 * max(rd_n, xd_n))), (
                f"{k}.{d}: lengths differ too much: rd={rd_n} xd={xd_n}"
            )
        # Coord set + dim layout + dtype.
        assert set(rs.coords) == set(xs.coords), f"{k}: coords differ"
        for c in rs.coords:
            assert rs[c].dims == xs[c].dims, f"{k}.{c}: coord dims differ"
            assert rs[c].dtype == xs[c].dtype, f"{k}.{c}: coord dtype differ"
        # Variable set + dtype + dims for every moment / scalar var.
        assert set(rs.data_vars) == set(xs.data_vars), f"{k}: data_vars differ"
        for v in rs.data_vars:
            assert rs[v].dims == xs[v].dims, f"{k}.{v}: dims differ"
            # Per-DataArray CF metadata: units / standard_name / long_name.
            for key in ("units", "standard_name", "long_name"):
                if key in xs[v].attrs:
                    assert rs[v].attrs.get(key) == xs[v].attrs[key], (
                        f"{k}.{v}.attrs[{key!r}]: {rs[v].attrs.get(key)!r} != {xs[v].attrs[key]!r}"
                    )

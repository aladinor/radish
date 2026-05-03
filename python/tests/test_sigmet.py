"""End-to-end tests for the Sigmet/IRIS RAW backend.

Gated on `RADISH_SIGMET_FIXTURE`; see `conftest.py::sigmet_fixture`.
"""

import numpy as np
import pytest

import radish


def test_read_sigmet_returns_volume(sigmet_fixture):
    vol = radish.read_sigmet(sigmet_fixture)
    assert vol.num_sweeps >= 1
    md = vol.metadata
    assert md.instrument_name  # non-empty
    assert -90 <= md.latitude <= 90
    assert -180 <= md.longitude <= 180
    s0 = vol.get_sweep(0)
    assert s0.num_rays > 0
    assert s0.num_gates > 0
    # Sigmet typically writes at least one of DBZH / DBTH.
    moments = s0.moment_names()
    assert any(m in moments for m in ("DBZH", "DBTH"))


def test_scan_sigmet_returns_metadata(sigmet_fixture):
    md = radish.scan_sigmet(sigmet_fixture)
    assert md.instrument_name
    assert md.num_sweeps >= 1


def test_xarray_engine_radish_dispatches_to_sigmet(sigmet_fixture):
    xr = pytest.importorskip("xarray")
    dt = xr.open_datatree(sigmet_fixture, engine="radish")
    sweep_keys = [k for k in dt.children if k.startswith("sweep_")]
    assert len(sweep_keys) >= 1
    s0 = dt[sweep_keys[0]]
    assert "azimuth" in s0.coords
    assert "elevation" in s0.coords
    assert "range" in s0.coords
    # IRIS-shaped root attrs (xradar's open_iris_datatree contract).
    assert dt.attrs.get("Conventions") == "None"
    assert "task_name" in dt.attrs
    assert "iris_version" in dt.attrs
    # Per-sweep `sweep_mode` and `sweep_fixed_angle` are FM301 0-d
    # data_vars (matching xradar's IRIS shape), not Dataset.attrs —
    # `.attrs` stays empty for sigmet sweeps for parity.
    assert s0.attrs == {}, f"sigmet per-sweep attrs should be empty, got {dict(s0.attrs)}"
    assert str(s0["sweep_mode"].values) in ("azimuth_surveillance", "rhi")
    assert float(s0["sweep_fixed_angle"].values) >= 0.0


def test_radish_open_datatree_path(sigmet_fixture):
    """`radish.open_datatree(path)` auto-detects sigmet."""
    pytest.importorskip("xarray")
    dt = radish.open_datatree(sigmet_fixture)
    assert any(k.startswith("sweep_") for k in dt.children)
    assert dt.attrs.get("scan_mode") in ("PPI", "RHI", "OTHER")


def test_radish_open_datatree_bytes(sigmet_fixture):
    """`radish.open_datatree(<bytes>)` auto-detects sigmet via magic bytes."""
    pytest.importorskip("xarray")
    with open(sigmet_fixture, "rb") as f:
        buf = f.read()
    dt = radish.open_datatree(buf)
    assert any(k.startswith("sweep_") for k in dt.children)


def test_detect_backend_sigmet(sigmet_fixture):
    assert radish.detect_backend(sigmet_fixture) == "sigmet"


def test_sigmet_volume_attrs_populated(sigmet_fixture):
    """SigmetVolumeAttrs carries the IRIS metadata block."""
    vol = radish.read_sigmet(sigmet_fixture)
    sattrs = vol.metadata.sigmet_attrs
    assert sattrs is not None
    assert isinstance(sattrs.task_name, str)
    assert sattrs.iris_version  # non-empty for valid IRIS files
    assert sattrs.scan_mode in ("PPI", "RHI", "OTHER")
    # PRF and unambiguous range are positive when wavelength was extracted;
    # either way, they should be finite (no inf/nan from a parse error).
    assert np.isfinite(sattrs.prf_hz)
    assert np.isfinite(sattrs.unambiguous_range_m)


def test_sigmet_sweep_attrs_populated(sigmet_fixture):
    """SigmetSweepAttrs is filled in on every sweep."""
    vol = radish.read_sigmet(sigmet_fixture)
    s0 = vol.get_sweep(0)
    sweep_attrs = s0.sigmet_attrs
    assert sweep_attrs is not None
    assert sweep_attrs.sweep_mode in ("azimuth_surveillance", "rhi")
    # Fixed angles are degrees in [0, 360).
    assert 0.0 <= sweep_attrs.fixed_angle_deg < 360.0


def test_sigmet_moment_set_matches_xradar(sigmet_fixture):
    """End-to-end parity: every 2-D moment xradar emits, radish emits too,
    and vice versa.

    Promotes the notebook spot-check (`smoke_test_sigmet.ipynb` cell on
    the `radish-only` / `xradar-only` set diff) into a CI-runnable
    regression. Skipped if `xradar` isn't installed.
    """
    xr = pytest.importorskip("xarray")
    xradar = pytest.importorskip("xradar")

    rd = xr.open_datatree(sigmet_fixture, engine="radish")
    # xradar's IRIS reader needs a str path (PosixPath fails on its
    # `_check_iris_file` memmap helper).
    xd = xradar.io.open_iris_datatree(str(sigmet_fixture))

    sweep_keys = sorted(k for k in rd.children if k.startswith("sweep_"))
    assert sweep_keys, "radish produced no sweeps"
    common_sweeps = [k for k in sweep_keys if k in xd.children]
    assert common_sweeps, "no overlapping sweep groups between readers"

    for skey in common_sweeps:
        rd_vars = {v for v in rd[skey].data_vars if rd[skey][v].ndim == 2}
        xd_vars = {v for v in xd[skey].data_vars if xd[skey][v].ndim == 2}
        radish_only = rd_vars - xd_vars
        xradar_only = xd_vars - rd_vars
        assert not radish_only, (
            f"{skey}: radish emits moments xradar doesn't: {sorted(radish_only)}"
        )
        assert not xradar_only, (
            f"{skey}: xradar emits moments radish doesn't: {sorted(xradar_only)} — "
            "likely a missing entry in `SUPPORTED_MOMENTS` or `iris_mapping_ids_match_xradar_table`"
        )

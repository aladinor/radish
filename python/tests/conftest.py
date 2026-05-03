"""pytest fixtures for radish tests."""

import os
from pathlib import Path

import pytest


def _nexrad_dir() -> Path | None:
    """Resolve the NEXRAD fixture directory from
    `RADISH_NEXRAD_FIXTURE_DIR`, returning ``None`` if unset/missing.
    See ``radish/tests/fixtures/CORPUS.md`` for the expected layout.
    """
    raw = os.environ.get("RADISH_NEXRAD_FIXTURE_DIR")
    if not raw:
        return None
    p = Path(raw).expanduser()
    return p if p.is_dir() else None


@pytest.fixture
def nexrad_fixture():
    """Path to a NEXRAD Level 2 fixture file, or skip if unset.

    Resolution order:
    1. ``RADISH_NEXRAD_FIXTURE`` (single explicit file path) — the
       legacy convention; honoured for back-compat.
    2. ``RADISH_NEXRAD_FIXTURE_DIR/KLOT20251210_102338_V06`` — the
       canonical happy-path fixture documented in CORPUS.md.
    """
    path = os.environ.get("RADISH_NEXRAD_FIXTURE")
    if path and os.path.exists(path):
        return path
    fdir = _nexrad_dir()
    if fdir is not None:
        candidate = fdir / "KLOT20251210_102338_V06"
        if candidate.exists():
            return str(candidate)
    pytest.skip(
        "neither RADISH_NEXRAD_FIXTURE nor "
        "RADISH_NEXRAD_FIXTURE_DIR is set/populated — see "
        "radish/tests/fixtures/CORPUS.md"
    )


@pytest.fixture
def nexrad_kilx_fixture():
    """Path to the KILX phantom-radial divergence fixture, or skip.

    Required by the regression test that pins ``sweep_10.num_rays == 358``
    against the file's true MSG_31 record count (versus the upstream
    `nexrad-decode 1.0.0-rc.3` 360-ray bug). See CORPUS.md.
    """
    fdir = _nexrad_dir()
    if fdir is None:
        pytest.skip(
            "RADISH_NEXRAD_FIXTURE_DIR not set — see "
            "radish/tests/fixtures/CORPUS.md"
        )
    candidate = fdir / "KILX20230629_154426_V06"
    if not candidate.exists():
        pytest.skip(
            f"KILX fixture missing at {candidate} — see "
            "radish/tests/fixtures/CORPUS.md to download"
        )
    return str(candidate)


@pytest.fixture
def sigmet_fixture():
    """Path to a Sigmet/IRIS RAW fixture file, or skip if unset."""
    path = os.environ.get("RADISH_SIGMET_FIXTURE")
    if not path or not os.path.exists(path):
        pytest.skip(
            "RADISH_SIGMET_FIXTURE not set or missing — set it to a "
            "Sigmet/IRIS RAW file to run Sigmet tests"
        )
    return path

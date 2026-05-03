"""pytest fixtures for radish tests."""

import os
import pytest


@pytest.fixture
def nexrad_fixture():
    """Path to a NEXRAD Level 2 fixture file, or skip if unset."""
    path = os.environ.get("RADISH_NEXRAD_FIXTURE")
    if not path or not os.path.exists(path):
        pytest.skip(
            "RADISH_NEXRAD_FIXTURE not set or missing — set it to a "
            "NEXRAD Archive II file to run NEXRAD tests"
        )
    return path


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

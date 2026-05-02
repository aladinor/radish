"""
Radish: High-performance weather radar data library

A Rust-powered library for reading weather radar data with Python bindings.
"""

# Lower-level building blocks (Rust → Python). Most users should reach for
# `radish.open_datatree` / `radish.open_dataset` instead — these stay
# exported for power users who want a `VolumeData` directly without
# building an xarray tree, or for callers who want explicit format
# selection on a path. `read_nexrad_bytes` is intentionally **not** here
# (it's reachable as `radish._radish.read_nexrad_bytes` for the dispatcher's
# internal use; the public way to decode bytes is `radish.open_datatree(data)`).
from radish._radish import (
    MomentData,
    NexradSweepAttrs,
    NexradVolumeAttrs,
    SweepData,
    VolumeData,
    VolumeMetadata,
    read_cfradial1,
    read_nexrad,
    read_nexrad_chunks,
    scan_cfradial1,
    scan_nexrad,
)

# Canonical entry points: format-agnostic, input-shape-agnostic.
from radish._open import detect_backend, open_dataset, open_datatree

__version__ = "0.1.0"

__all__ = [
    # Data model
    "VolumeData",
    "VolumeMetadata",
    "SweepData",
    "MomentData",
    "NexradVolumeAttrs",
    "NexradSweepAttrs",
    # Canonical entry points
    "open_datatree",
    "open_dataset",
    "detect_backend",
    # Lower-level building blocks (per-format, path-only readers)
    "read_cfradial1",
    "scan_cfradial1",
    "read_nexrad",
    "scan_nexrad",
    "read_nexrad_chunks",
]

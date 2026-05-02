"""
Radish: High-performance weather radar data library

A Rust-powered library for reading weather radar data with Python bindings.
"""

from radish._radish import (
    VolumeData,
    VolumeMetadata,
    SweepData,
    MomentData,
    NexradVolumeAttrs,
    NexradSweepAttrs,
    read_cfradial1,
    scan_cfradial1,
    read_nexrad,
    scan_nexrad,
    read_nexrad_chunks,
)
from radish.backends.xarray_backend import open_nexrad_chunks_datatree

__version__ = "0.1.0"

__all__ = [
    "VolumeData",
    "VolumeMetadata",
    "SweepData",
    "MomentData",
    "NexradVolumeAttrs",
    "NexradSweepAttrs",
    "read_cfradial1",
    "scan_cfradial1",
    "read_nexrad",
    "scan_nexrad",
    "read_nexrad_chunks",
    "open_nexrad_chunks_datatree",
]

"""Xarray backend for radish.

Supports two file formats today:

* CfRadial1 NetCDF (extensions ``.nc``, ``.nc4``, ``.netcdf``)
* NEXRAD Level 2 Archive II (extension ``.ar2``/``.ar2v``, the ``AR2V`` magic
  bytes, or the canonical ``KXXX########_######`` filename pattern used by
  NOAA's NEXRAD archive)

The backend dispatches to the matching radish reader at open time. Sweep
groups are produced as ``/sweep_0``, ``/sweep_1``, ... so the result is shape-
compatible with ``xradar.io.open_nexradlevel2_datatree``.
"""

import os
import re
from typing import Any, Dict, Iterable, Optional
import numpy as np

try:
    import xarray as xr
    from xarray.backends import BackendEntrypoint
    from xarray.core import indexing
    XARRAY_AVAILABLE = True
except ImportError:
    XARRAY_AVAILABLE = False
    BackendEntrypoint = object  # type: ignore

try:
    from datatree import DataTree
    DATATREE_AVAILABLE = True
except ImportError:
    DATATREE_AVAILABLE = False
    # xarray ≥ 2024 also exposes DataTree natively
    try:
        from xarray import DataTree  # type: ignore
        DATATREE_AVAILABLE = True
    except ImportError:
        pass

from radish import read_cfradial1, read_nexrad, VolumeData

_NEXRAD_NAME_RE = re.compile(r"[A-Z]{4}\d{8}_\d{6}(_V\d{2})?$")
_CFRADIAL_EXTS = (".nc", ".nc4", ".netcdf")
_NEXRAD_EXTS = (".ar2", ".ar2v")


def _detect_format(filename_or_obj) -> Optional[str]:
    """Return ``"nexrad"``, ``"cfradial1"``, or ``None``.

    Three signals (any one is enough):
      1. file extension match,
      2. AR2V magic bytes,
      3. canonical NEXRAD filename pattern.
    """
    try:
        path = os.fspath(filename_or_obj)
    except (TypeError, AttributeError):
        return None
    lower = path.lower()
    if lower.endswith(_CFRADIAL_EXTS):
        return "cfradial1"
    if lower.endswith(_NEXRAD_EXTS):
        return "nexrad"
    if _NEXRAD_NAME_RE.search(os.path.basename(path)):
        return "nexrad"
    try:
        with open(path, "rb") as f:
            if f.read(4) == b"AR2V":
                return "nexrad"
    except OSError:
        pass
    return None


def _read_volume(path: str, fmt: str):
    if fmt == "nexrad":
        return read_nexrad(path)
    return read_cfradial1(path)


def _parse_sweep_index(group: Optional[str], num_sweeps: int) -> int:
    """Resolve a ``group="sweep_N"`` argument to a sweep index, defaulting to 0
    when no group is given. Raises ``ValueError`` for malformed or out-of-range
    group names rather than silently selecting sweep 0.
    """
    if not group:
        return 0
    if not group.startswith("sweep_"):
        raise ValueError(
            f"unrecognised group {group!r}; expected 'sweep_<N>' or no group"
        )
    suffix = group.split("_", 1)[1]
    try:
        idx = int(suffix)
    except ValueError as e:
        raise ValueError(f"sweep group {group!r} is not numeric") from e
    if not (0 <= idx < num_sweeps):
        raise ValueError(
            f"sweep group {group!r} out of range [0, {num_sweeps})"
        )
    return idx


class RadishBackendEntrypoint(BackendEntrypoint):
    """Xarray backend for reading radar files with radish"""

    description = "Read weather radar data files using the radish library"
    url = "https://github.com/mgrover1/radish"

    # xarray's plugin discovery introspects the signature and rejects *args/**kwargs.
    open_dataset_parameters: tuple = (
        "filename_or_obj",
        "drop_variables",
        "group",
    )

    def open_dataset(
        self,
        filename_or_obj,
        *,
        drop_variables: Optional[Iterable[str]] = None,
        group: Optional[str] = None,
    ):
        """
        Open a single dataset (sweep).

        For multi-sweep files, use open_datatree instead.
        """
        path = os.fspath(filename_or_obj)
        fmt = _detect_format(filename_or_obj) or "cfradial1"
        volume = _read_volume(path, fmt)
        sweep_idx = _parse_sweep_index(group, volume.num_sweeps)
        sweep = volume.get_sweep(sweep_idx)
        return self._sweep_to_dataset(sweep, volume.metadata)

    def open_datatree(
        self,
        filename_or_obj,
        *,
        drop_variables: Optional[Iterable[str]] = None,
    ):
        """
        Open a radar volume as a DataTree with multiple sweeps.

        Returns a DataTree with:
        - Root group: volume metadata
        - sweep_N groups: individual sweep data
        """
        if not DATATREE_AVAILABLE:
            raise ImportError(
                "DataTree support requires xarray>=2024.10 or the legacy "
                "datatree package. Install with: pip install -U xarray"
            )

        path = os.fspath(filename_or_obj)
        fmt = _detect_format(filename_or_obj) or "cfradial1"
        volume = _read_volume(path, fmt)

        datasets = {"/": self._create_root_dataset(volume.metadata, fmt)}
        for i in range(volume.num_sweeps):
            sweep = volume.get_sweep(i)
            datasets[f"/sweep_{i}"] = self._sweep_to_dataset(sweep, volume.metadata)
        return DataTree.from_dict(datasets)

    def _create_root_dataset(self, metadata, fmt: str = "cfradial1") -> "xr.Dataset":
        """Create root dataset with volume metadata."""
        coords = {
            "latitude": metadata.latitude,
            "longitude": metadata.longitude,
            "altitude": metadata.altitude,
        }
        data_vars = {
            "sweep_fixed_angle": (["sweep"], np.array(metadata.sweep_fixed_angles)),
        }
        attrs: Dict[str, Any] = {
            "instrument_name": metadata.instrument_name,
        }
        if fmt == "nexrad":
            # Match xradar's `open_nexradlevel2_datatree` root attribute set
            # so engine-swap users see the same shape. xradar fills several of
            # these from MSG_2/MSG_5 (RDA status / VCP) which `nexrad-model`
            # 1.0.0-rc.2 doesn't surface; until Phase 3 wires the lower-level
            # API, those slots are explicitly empty rather than missing.
            extra = (
                dict(metadata.attributes)
                if getattr(metadata, "attributes", None)
                else {}
            )
            attrs.update(
                {
                    "Conventions": "ODIM_H5/V2_2",
                    "version": "",
                    "title": "",
                    "institution": getattr(metadata, "institution", "NOAA/NWS"),
                    "references": "",
                    "source": "NEXRAD Level 2 Archive",
                    "history": "",
                    "comment": "",
                    "scan_name": extra.get("scan_name", ""),
                    "vcp": extra.get("vcp", ""),
                    "vcp_description": extra.get("vcp_description", ""),
                }
            )
        else:
            attrs["Conventions"] = "CF/Radial"
        return xr.Dataset(data_vars=data_vars, coords=coords, attrs=attrs)

    def _sweep_to_dataset(self, sweep, volume_metadata) -> "xr.Dataset":
        """Convert a sweep to an xarray Dataset.

        ``sweep.azimuth``/``elevation``/``range``/``time`` come back as numpy
        arrays directly from PyO3 (one C-level memcpy each), so no
        ``np.array(...)`` wrapping is needed.
        """
        coords = {
            "azimuth": (["time"], sweep.azimuth),
            "elevation": (["time"], sweep.elevation),
            "range": (["range"], sweep.range),
        }

        data_vars = {}
        for moment_name in sweep.moment_names():
            moment = sweep.get_moment(moment_name)
            if moment is not None:
                data_vars[moment_name] = (
                    ["time", "range"],
                    moment.data(),
                    {"units": moment.units},
                )

        # Attributes
        attrs = {
            "sweep_number": int(sweep.sweep_number),
            "fixed_angle": float(sweep.fixed_angle),
            "instrument_name": volume_metadata.instrument_name,
        }

        return xr.Dataset(data_vars=data_vars, coords=coords, attrs=attrs)

    @classmethod
    def guess_can_open(cls, filename_or_obj):
        """Return True if radish can open this file (CfRadial1 or NEXRAD L2)."""
        return _detect_format(filename_or_obj) is not None


# For backwards compatibility
RadishBackend = RadishBackendEntrypoint

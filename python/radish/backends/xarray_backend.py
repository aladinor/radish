"""Xarray backend for radish.

Supports two file formats today:

* CfRadial1 NetCDF (extensions ``.nc``, ``.nc4``, ``.netcdf``)
* NEXRAD Level 2 Archive II (extension ``.ar2``/``.ar2v``, the ``AR2V`` magic
  bytes, or the canonical ``KXXX########_######`` filename pattern used by
  NOAA's NEXRAD archive)

The backend dispatches to the matching radish reader at open time. The
NEXRAD path is built to match the structural shape of
``xradar.io.open_nexradlevel2_datatree`` byte-for-byte at the structural
level — same dim names (``azimuth`` not ``time``), same coord set + attrs,
same per-DataArray CF metadata, same variable set. Numeric values may
differ within tolerance (e.g. radish uses NaN for missing data while
xradar uses negative-float sentinels), but the trees are interchangeable
for any code that walks dim names / coord names / variable names /
attribute keys.
"""

import os
import re
from typing import Any, ClassVar, Dict, Iterable, Optional, Tuple

import numpy as np
import pandas as pd

try:
    import xarray as xr
    from xarray.backends import BackendEntrypoint

    XARRAY_AVAILABLE = True
except ImportError:
    XARRAY_AVAILABLE = False
    BackendEntrypoint = object  # type: ignore

try:
    from datatree import DataTree

    DATATREE_AVAILABLE = True
except ImportError:
    DATATREE_AVAILABLE = False
    # xarray ≥ 2024.10 also exposes DataTree natively
    try:
        from xarray import DataTree  # type: ignore

        DATATREE_AVAILABLE = True
    except ImportError:
        pass

from radish import read_cfradial1, read_nexrad

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


# Per-moment CF metadata. Strings are chosen to match xradar's NEXRAD reader
# verbatim so engine-swap users see identical per-DataArray attrs. The Rust
# adapter's `mapping.rs::moment_meta` is the source of truth for the moment
# names and gets these same strings when constructing the underlying
# `MomentData.units` field; we look those up here too because the xarray
# layer needs `standard_name` and `long_name` which the Rust `MomentData`
# struct exposes in `attributes` but PyO3 doesn't surface (today).
_MOMENT_CF_METADATA: Dict[str, Dict[str, str]] = {
    "DBZH": {
        "standard_name": "radar_equivalent_reflectivity_factor_h",
        "long_name": "Equivalent reflectivity factor H",
    },
    "VRADH": {
        "standard_name": "radial_velocity_of_scatterers_away_from_instrument_h",
        "long_name": "Radial velocity of scatterers away from instrument H",
    },
    "WRADH": {
        "standard_name": "radar_doppler_spectrum_width_h",
        "long_name": "Doppler spectrum width H",
    },
    "ZDR": {
        "standard_name": "radar_differential_reflectivity_hv",
        "long_name": "Log differential reflectivity H/V",
    },
    "PHIDP": {
        "standard_name": "radar_differential_phase_hv",
        "long_name": "Differential phase HV",
    },
    "RHOHV": {
        "standard_name": "radar_correlation_coefficient_hv",
        "long_name": "Correlation coefficient HV",
    },
    "CCORH": {
        "standard_name": "clutter_correction_h",
        "long_name": "Clutter Correction H",
    },
}


def _moment_cf_attrs(moment_name: str, units: str) -> Dict[str, str]:
    """Resolve the full CF attribute set for a moment.

    `units` comes from the Rust `MomentData.units` field (already the
    xradar string). `standard_name` and `long_name` are looked up in the
    static table above; unknown moments fall back to an attrs dict with
    just `units` so the variable is at least minimally annotated.
    """
    base = {"long_name": moment_name, "units": units, "standard_name": moment_name}
    overrides = _MOMENT_CF_METADATA.get(moment_name)
    if overrides:
        base.update(overrides)
    return base


def _parse_sweep_index(group: Optional[str], num_sweeps: int) -> int:
    """Resolve a ``group="sweep_N"`` argument to a sweep index, defaulting to 0
    when no group is given. Raises ``ValueError`` for malformed or out-of-range
    group names rather than silently selecting sweep 0.
    """
    if not group:
        return 0
    if not group.startswith("sweep_"):
        raise ValueError(f"unrecognised group {group!r}; expected 'sweep_<N>' or no group")
    suffix = group.split("_", 1)[1]
    try:
        idx = int(suffix)
    except ValueError as e:
        raise ValueError(f"sweep group {group!r} is not numeric") from e
    if not (0 <= idx < num_sweeps):
        raise ValueError(f"sweep group {group!r} out of range [0, {num_sweeps})")
    return idx


class RadishBackendEntrypoint(BackendEntrypoint):
    """Xarray backend for reading radar files with radish"""

    description = "Read weather radar data files using the radish library"
    url = "https://github.com/mgrover1/radish"

    # xarray's plugin discovery introspects the signature and rejects *args/**kwargs.
    # `BackendEntrypoint.open_dataset_parameters` is a class variable; mark ours
    # `ClassVar` too so mypy doesn't read this as an instance-attribute override.
    open_dataset_parameters: ClassVar[Optional[Tuple[str, ...]]] = (
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
        """Create root dataset with volume metadata.

        For NEXRAD: matches xradar's root layout — site coords plus the
        radar-station attribute set; sweep-level information lives in the
        ``/sweep_N`` groups, not at the root. CfRadial1 keeps a top-level
        ``sweep_fixed_angle(sweep)`` array for backward compatibility with
        the original radish backend.
        """
        coords = {
            "latitude": metadata.latitude,
            "longitude": metadata.longitude,
            "altitude": metadata.altitude,
        }
        data_vars: Dict[str, Any] = {}
        if fmt != "nexrad":
            # Pre-existing CfRadial1 shape; xradar's NEXRAD reader doesn't
            # advertise a root-level sweep_fixed_angle array (each sweep
            # group has its own 0-d copy via FM301), so we skip it for
            # NEXRAD to avoid an extra `sweep` dim leaking into sweep_0.
            data_vars["sweep_fixed_angle"] = (
                ["sweep"],
                np.array(metadata.sweep_fixed_angles),
            )
        attrs: Dict[str, Any] = {
            "instrument_name": metadata.instrument_name,
        }
        if fmt == "nexrad":
            # Mirror xradar's `open_nexradlevel2_datatree` root attribute set
            # exactly. xradar emits these as the literal string "None" rather
            # than asserting any convention — they don't claim CfRadial2 /
            # FM301 compliance. The richer keys (super_res_status,
            # rda_build_number, dynamic_scan_type, avset_enabled, ...) come
            # from MSG_2/MSG_5 and need the lower-level `nexrad-decode` API;
            # those land in a follow-up commit.
            extra = dict(metadata.attributes) if getattr(metadata, "attributes", None) else {}
            attrs.update(
                {
                    "Conventions": "None",
                    "version": "None",
                    "title": "None",
                    "institution": "None",
                    "references": "None",
                    "source": "None",
                    "history": "None",
                    "comment": "im/exported using radish",
                    "scan_name": extra.get("scan_name", ""),
                }
            )
            # 0-d root data_vars that xradar exposes alongside the latitude/
            # longitude/altitude coords. NEXRAD is always a fixed ground
            # radar, so `instrument_type = 'radar'` is hard-coded; the
            # rest come from `VolumeMetadata`.
            data_vars.update(
                {
                    "volume_number": ((), int(metadata.volume_number)),
                    "platform_type": ((), str(metadata.platform_type)),
                    "instrument_type": ((), "radar"),
                    "time_coverage_start": ((), str(metadata.time_coverage_start)),
                    "time_coverage_end": ((), str(metadata.time_coverage_end)),
                }
            )
        else:
            attrs["Conventions"] = "CF/Radial"
        return xr.Dataset(data_vars=data_vars, coords=coords, attrs=attrs)

    def _sweep_to_dataset(self, sweep, volume_metadata) -> "xr.Dataset":
        """Convert a sweep to an xarray Dataset matching xradar's NEXRAD shape.

        Concretely:

        * ray dim is ``azimuth`` (xradar/ODIM convention), not ``time``;
        * ``azimuth`` and ``elevation`` are float64 coords on the ``azimuth`` dim;
        * ``time`` is a ``datetime64[ns]`` coord on the ``azimuth`` dim;
        * ``range`` is a float32 coord on the ``range`` dim with the standard
          radar attrs (``meters_between_gates``, ``meters_to_center_of_first_gate``,
          ``spacing_is_constant``);
        * each moment is a float64 ``(azimuth, range)`` DataArray with
          ``units``/``standard_name``/``long_name`` matching xradar verbatim
          (see ``mapping.rs::moment_meta``);
        * ``sweep_mode``, ``sweep_number``, ``sweep_fixed_angle``, ``prt_mode``,
          and ``follow_mode`` are 0-d data variables (FM301-style scalar vars).

        Per-sweep MSG_5 attrs (``waveform_type``, ``channel_config``,
        ``super_resolution``, ``sails_cut``, ``mrle_cut``, ``mpda_cut``,
        ``base_tilt_cut``) are deferred to a follow-up commit that drops to
        ``nexrad-decode`` for the VCP message.
        """
        # `sweep.time` is float64 seconds-since-epoch from PyO3; xradar wants
        # datetime64[ns] for the time coord. pandas' to_datetime handles the
        # conversion crisply (xarray hard-depends on pandas anyway).
        time_dt = pd.to_datetime(np.asarray(sweep.time, dtype=np.float64), unit="s").to_numpy()

        coords = {
            "azimuth": (
                ["azimuth"],
                np.asarray(sweep.azimuth, dtype=np.float64),
                {
                    "standard_name": "ray_azimuth_angle",
                    "long_name": "azimuth_angle_from_true_north",
                    "units": "degrees",
                    "axis": "radial_azimuth_coordinate",
                },
            ),
            "elevation": (
                ["azimuth"],
                np.asarray(sweep.elevation, dtype=np.float64),
                {
                    "standard_name": "ray_elevation_angle",
                    "long_name": "elevation_angle_from_horizontal_plane",
                    "units": "degrees",
                    "axis": "radial_elevation_coordinate",
                },
            ),
            "time": (["azimuth"], time_dt, {"standard_name": "time"}),
            "range": (
                ["range"],
                np.asarray(sweep.range, dtype=np.float32),
                {
                    "units": "meters",
                    "standard_name": "projection_range_coordinate",
                    "long_name": "range_to_measurement_volume",
                    "axis": "radial_range_coordinate",
                    "meters_between_gates": np.float32(
                        sweep.range[1] - sweep.range[0] if len(sweep.range) > 1 else 0.0
                    ),
                    "spacing_is_constant": "true",
                    "meters_to_center_of_first_gate": np.float32(sweep.range[0]),
                },
            ),
        }

        data_vars: Dict[str, Any] = {}
        for moment_name in sweep.moment_names():
            moment = sweep.get_moment(moment_name)
            if moment is None:
                continue
            data_vars[moment_name] = (
                ["azimuth", "range"],
                moment.data().astype(np.float64),
                _moment_cf_attrs(moment_name, moment.units),
            )

        # FM301 scalar sweep variables (also matches xradar).
        data_vars["sweep_mode"] = ((), sweep.sweep_mode)
        data_vars["sweep_number"] = ((), int(sweep.sweep_number))
        data_vars["sweep_fixed_angle"] = ((), float(sweep.fixed_angle))
        data_vars["prt_mode"] = ((), sweep.prt_mode)
        data_vars["follow_mode"] = ((), sweep.follow_mode)

        # Sweep-level attrs left empty for now; populated from MSG_5 in
        # a follow-up commit.
        return xr.Dataset(data_vars=data_vars, coords=coords, attrs={})

    @staticmethod
    def _data_var_dims(name: str):
        # Helper kept for forward compatibility / readability.
        return ["azimuth", "range"]

    @classmethod
    def guess_can_open(cls, filename_or_obj):
        """Return True if radish can open this file (CfRadial1 or NEXRAD L2)."""
        return _detect_format(filename_or_obj) is not None


# For backwards compatibility
RadishBackend = RadishBackendEntrypoint

"""Unified entry points: ``radish.open_datatree`` / ``radish.open_dataset``.

One canonical entry takes any input shape (path, raw bytes, file-like,
chunk list) and any backend (auto-detected or explicit ``backend="nexrad"``)
and produces an ``xarray.DataTree`` or ``xarray.Dataset``. The
``xr.open_datatree(path, engine="radish")`` plugin entry-point delegates
into the same code path so existing callers keep working.

Adding a new format backend is now a two-step ritual:

1. Implement the Rust ``RadarBackend`` trait (in particular
   ``can_read_bytes`` for the in-memory path).
2. Register the backend's per-shape Rust→Python reader entries in
   ``_DISPATCH`` below. No new Python helpers, no new top-level names.
"""

from __future__ import annotations

import os
from typing import Any, Callable, Dict, Iterable, Optional, Tuple

from radish._radish import (
    auto_backend_name,
    auto_backend_name_for_bytes,
    read_cfradial1,
    read_nexrad,
    read_nexrad_bytes,
    read_nexrad_chunks,
)

# Type aliases for input shapes the dispatcher recognises.
InputShape = str  # one of the constants below
SHAPE_PATH = "path"
SHAPE_BYTES = "bytes"
SHAPE_FILELIKE = "file-like"
SHAPE_CHUNKS = "chunk-list"

# How many bytes to peek when sniffing a file-like or buffer. 16 covers
# every magic we currently sniff (HDF5 = 8, AR2V = 4, gzip = 2).
_PEEK_BYTES = 16


def _is_bytes_like(obj: Any) -> bool:
    return isinstance(obj, (bytes, bytearray, memoryview))


def _is_path_like(obj: Any) -> bool:
    return isinstance(obj, str) or hasattr(obj, "__fspath__")


def _is_file_like(obj: Any) -> bool:
    """A `.read()`-having object that isn't already a path-like wrapper."""
    return hasattr(obj, "read") and not _is_path_like(obj)


def _is_chunk_iterable(obj: Any) -> bool:
    """An iterable of chunk-like items.

    Strings/bytes themselves are technically iterable, so we exclude them
    explicitly — otherwise a `bytes` payload would be misclassified as a
    chunk list of single-byte chunks.
    """
    if _is_bytes_like(obj) or _is_path_like(obj) or _is_file_like(obj):
        return False
    try:
        first = next(iter(obj))
    except (TypeError, StopIteration):
        return False
    return _is_bytes_like(first) or _is_path_like(first)


def _classify_shape(input_obj: Any) -> InputShape:
    """Return one of the SHAPE_* constants for ``input_obj``."""
    if _is_bytes_like(input_obj):
        return SHAPE_BYTES
    if _is_path_like(input_obj):
        return SHAPE_PATH
    if _is_file_like(input_obj):
        return SHAPE_FILELIKE
    if _is_chunk_iterable(input_obj):
        return SHAPE_CHUNKS
    raise TypeError(
        f"Unsupported input type for radish.open_datatree: {type(input_obj).__name__}. "
        "Expected path-like (str / os.PathLike), bytes-like, file-like with "
        ".read(), or an iterable of bytes/paths."
    )


def _materialize_chunks(chunks: Iterable[Any]) -> "list[bytes]":
    """Collapse a chunk iterable into a list of `bytes`, reading paths eagerly."""
    out: list[bytes] = []
    for c in chunks:
        if _is_bytes_like(c):
            out.append(bytes(c))
        elif _is_path_like(c):
            with open(os.fspath(c), "rb") as f:
                out.append(f.read())
        else:
            raise TypeError(f"Each chunk must be bytes or a path; got {type(c).__name__}")
    return out


def _peek_filelike(file_obj: Any) -> bytes:
    """Read up to `_PEEK_BYTES` from a file-like and rewind so the next read
    sees the same buffer. Falls back to consuming the stream if `seek` fails.
    """
    head = file_obj.read(_PEEK_BYTES)
    if hasattr(file_obj, "seek"):
        try:
            file_obj.seek(-len(head), 1)
        except (OSError, ValueError):
            # Non-seekable file-like (some HTTP responses, etc.) — caller
            # ends up reading the rest of the stream and we'd lose `head`.
            # Reattach the prefix by wrapping in BytesIO downstream.
            return head
    return head


def detect_backend(input_obj: Any) -> Optional[str]:
    """Identify which radish backend will own this input.

    Returns the canonical backend name (``"nexrad_level2"`` /
    ``"cfradial1"``) or ``None`` when no backend recognises the input.
    Useful for routing decisions outside the open path. Doesn't decode.
    """
    try:
        shape = _classify_shape(input_obj)
    except TypeError:
        return None
    if shape == SHAPE_PATH:
        try:
            return auto_backend_name(os.fspath(input_obj))
        except RuntimeError:
            return None
    if shape == SHAPE_BYTES:
        try:
            return auto_backend_name_for_bytes(bytes(input_obj[:_PEEK_BYTES]))
        except RuntimeError:
            return None
    if shape == SHAPE_FILELIKE:
        head = _peek_filelike(input_obj)
        try:
            return auto_backend_name_for_bytes(head)
        except RuntimeError:
            return None
    if shape == SHAPE_CHUNKS:
        # Chunk streams are NEXRAD-specific (no other format defines a
        # multi-file LDM stream). Match the Rust side's contract.
        return "nexrad_level2"
    return None


# Dispatch table: (backend_name, input_shape) -> reader callable returning
# `radish.VolumeData`. Each cell is one Rust→Python entry point. Adding a
# format means appending its rows here.
_DISPATCH: Dict[Tuple[str, InputShape], Callable[[Any], Any]] = {
    ("nexrad_level2", SHAPE_PATH): lambda obj: read_nexrad(os.fspath(obj)),
    ("nexrad_level2", SHAPE_BYTES): lambda obj: read_nexrad_bytes(bytes(obj)),
    ("nexrad_level2", SHAPE_FILELIKE): lambda obj: read_nexrad_bytes(obj.read()),
    ("nexrad_level2", SHAPE_CHUNKS): lambda obj: read_nexrad_chunks(_materialize_chunks(obj)),
    ("cfradial1", SHAPE_PATH): lambda obj: read_cfradial1(os.fspath(obj)),
}


def _read_volume(input_obj: Any, backend: Optional[str]):
    """Pick the right Rust reader for `(backend, shape)` and decode."""
    shape = _classify_shape(input_obj)

    if backend is None:
        # Auto-detect format from the input. Chunk lists short-circuit
        # (only NEXRAD has a multi-file stream); the other shapes go
        # through the Rust auto-detect.
        if shape == SHAPE_PATH:
            backend = auto_backend_name(os.fspath(input_obj))
        elif shape == SHAPE_BYTES:
            backend = auto_backend_name_for_bytes(bytes(input_obj[:_PEEK_BYTES]))
        elif shape == SHAPE_FILELIKE:
            head = _peek_filelike(input_obj)
            backend = auto_backend_name_for_bytes(head)
        elif shape == SHAPE_CHUNKS:
            backend = "nexrad_level2"

    if backend is None:
        # Every classify branch above sets `backend` (or raises via Rust);
        # unreachable in practice, but keeps mypy honest about the dict
        # lookup below requiring a concrete string key.
        raise ValueError(f"could not determine backend for input shape {shape!r}")
    reader = _DISPATCH.get((backend, shape))
    if reader is None:
        raise ValueError(
            f"radish backend {backend!r} does not support input shape {shape!r}. "
            f"(Common: cfradial1 only accepts paths — `libnetcdf` doesn't expose "
            f"an in-memory open. Pass a path or use NEXRAD bytes/chunks instead.)"
        )
    return reader(input_obj)


def open_datatree(
    input: Any,
    backend: Optional[str] = None,
    *,
    drop_variables: Optional[Iterable[str]] = None,
) -> "Any":  # xarray.DataTree
    """Open a radar volume as an ``xarray.DataTree``.

    Parameters
    ----------
    input
        One of: a path-like (``str`` / ``os.PathLike``), raw ``bytes`` /
        ``bytearray`` / ``memoryview``, a file-like with ``.read()``, or an
        iterable of bytes/paths (NEXRAD chunk stream).
    backend
        ``"nexrad_level2"`` (or alias ``"nexrad"``) or ``"cfradial1"``.
        ``None`` (default) auto-detects from the input.
    drop_variables
        Reserved for API parity with xarray's plugin ``open_datatree``;
        currently unused.

    Returns
    -------
    xarray.DataTree

    Examples
    --------
    >>> import radish
    >>> dt = radish.open_datatree("KLOT20260310_231412_V06")
    >>> dt = radish.open_datatree(open("file.gz", "rb").read())   # bytes
    >>> dt = radish.open_datatree([s, i01, i02, e])               # chunks
    >>> dt = radish.open_datatree("foo.nc", backend="cfradial1")  # explicit
    """
    backend = _normalize_backend(backend)
    volume = _read_volume(input, backend)
    return _build_datatree(volume, _format_for_root(backend or _infer_backend_from_volume(volume)))


def open_dataset(
    input: Any,
    backend: Optional[str] = None,
    *,
    group: Optional[str] = None,
    drop_variables: Optional[Iterable[str]] = None,
) -> "Any":  # xarray.Dataset
    """Open a single sweep as an ``xarray.Dataset``. Use ``open_datatree`` for
    multi-sweep volumes.

    See :func:`open_datatree` for input-shape and ``backend`` semantics.
    """
    backend = _normalize_backend(backend)
    volume = _read_volume(input, backend)
    # Lazy import: the xarray entrypoint module imports from here, so the
    # reverse dependency only resolves at call time.
    from radish.backends.xarray_backend import RadishBackendEntrypoint

    entry = RadishBackendEntrypoint()
    sweep_idx = (
        entry._parse_sweep_index(group, volume.num_sweeps)
        if hasattr(entry, "_parse_sweep_index")
        else _parse_sweep_index_fallback(group, volume.num_sweeps)
    )
    sweep = volume.get_sweep(sweep_idx)
    return entry._sweep_to_dataset(sweep, volume.metadata)


# ---- internals -----------------------------------------------------------


_BACKEND_ALIASES: Dict[str, str] = {
    "nexrad": "nexrad_level2",
    "nexrad_level2": "nexrad_level2",
    "cfradial1": "cfradial1",
}


def _normalize_backend(backend: Optional[str]) -> Optional[str]:
    if backend is None:
        return None
    canonical = _BACKEND_ALIASES.get(backend.lower())
    if canonical is None:
        valid = ", ".join(sorted(set(_BACKEND_ALIASES.values())))
        raise ValueError(f"Unknown backend {backend!r}. Valid: {valid}")
    return canonical


def _format_for_root(backend_name: Optional[str]) -> str:
    """Translate a backend name to the format key used by
    `_create_root_dataset`. The xarray backend currently switches on the
    string `"nexrad"` for NEXRAD-specific root attrs."""
    if backend_name is None:
        return "cfradial1"  # safe default — produces minimal root
    if backend_name == "nexrad_level2":
        return "nexrad"
    return backend_name


def _infer_backend_from_volume(volume: Any) -> str:
    """Best-effort backend identification when we never sniffed (chunk-
    list inputs go straight to read_nexrad_chunks). Matches the
    `_format_for_root` contract.
    """
    if getattr(volume.metadata, "nexrad_attrs", None) is not None:
        return "nexrad_level2"
    return "cfradial1"


def _build_datatree(volume: Any, fmt: str):
    """Build an `xr.DataTree` from a `VolumeData`. Reuses the xarray
    backend's existing `_volume_to_datatree` helper so the dim/coord/attr
    layout is identical to `xr.open_datatree(path, engine="radish")`.
    """
    from radish.backends.xarray_backend import RadishBackendEntrypoint

    return RadishBackendEntrypoint()._volume_to_datatree(volume, fmt)


def _parse_sweep_index_fallback(group: Optional[str], num_sweeps: int) -> int:
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


__all__ = ["open_datatree", "open_dataset", "detect_backend"]

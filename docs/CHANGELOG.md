# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

_Nothing here yet — open a PR to start populating._

## [0.2.0] - 2026-05-03

### Added

- **NEXRAD: per-sweep MSG_5 attrs and time ranges on `scan_nexrad`** —
  adds `sweep_attrs: Vec<NexradSweepAttrs>` and
  `sweep_time_ranges: Vec<Option<(f64, f64)>>` to `NexradVolumeAttrs`,
  populated by both `scan_nexrad` (metadata-only) and `read_nexrad`
  (full decode). Lets downstream bulk-ingest callers classify
  SAILS / MRLE / MPDA / base-tilt slices and find sweep boundaries
  without paying for per-ray decode. PyO3 surface adds two matching
  getters on `PyNexradVolumeAttrs`. Time ranges are Unix seconds for
  `pandas.to_datetime(t, unit="s")` round-trips. (#7)
- **`docs/` folder** consolidating long-form documentation —
  `ARCHITECTURE.md`, `GETTING_STARTED.md`, `PROJECT_SUMMARY.md`, plus
  the new `CHANGELOG.md` and `README.md` index. The repo root stays
  scoped to operational entry points. (#10)
- **`docs/CHANGELOG.md`** kicks off formal release-note tracking
  (Keep a Changelog 1.1.0 / SemVer 2.0). (#10)
- **`docs/RELEASING.md`** — release walkthrough lives next to the
  rest of the long-form docs. (#9)
- **`scripts/bump-version.sh`** — keeps `Cargo.toml` workspace version
  and `python/pyproject.toml` in lockstep during version bumps. (#9)
- **Crate-level rustdoc** for `radish` significantly expanded;
  intra-doc links now resolve cleanly. (#10)
- **PyO3 docstrings** on every public class/function so
  `help(radish.read_nexrad)` and IPython `?` work as expected. (#10)
- **`releases/tag/...` and `compare/...` ignore patterns** in
  `.github/markdown-link-check-config.json` so Keep-a-Changelog
  footers don't 404 in the window between a CHANGELOG bump and the
  GitHub release/tag actually existing. (#10)

### Changed

- **PyPI distribution renamed to `radish-rs`** (atmoscale account,
  `alfonso@atmoscale.ai`). The Python import path stays `radish`. (#9)
- **Release pipeline modernized** — `release.yml` now uses OIDC
  trusted publishing on PyPI, manylinux_2_28 wheels, gated
  `create-release` job (only fires on tag push, not
  `workflow_dispatch` from a feature branch), and
  `generate-import-lib` for cross-platform builds. (#9)
- **Wheel matrix trimmed** to Linux x86_64 + macOS x86_64 + macOS
  arm64. Linux aarch64 (cross-compile linker can't find aarch64
  hdf5/netcdf) and Windows (`hdf5-metno-sys` vcpkg static-md issues)
  are deferred; sdist is the fallback for those platforms. (#9)
- **Long-form docs moved into `docs/`** — cross-references in
  `CLAUDE.md`, `docs/GETTING_STARTED.md`, and `docs/PROJECT_SUMMARY.md`
  updated to the new paths. (#10)

### Fixed

- **NEXRAD `sweep_fixed_angle` parity** — was returning the
  achieved median (`Sweep::elevation_angle_degrees()`) instead of
  xradar's commanded MSG_5 (`ElevationCut::elevation_angle_degrees()`),
  diverging by up to ~0.18°. New `fixed_angle_for(cut, sweep)` helper
  prefers the commanded angle and falls back to the median-of-radials.
  Result: byte-identical to xradar on the KLOT fixture. (#8)

### Removed

- **`plans/` directory** removed from version control and added to
  `.gitignore` along with `.claude/` — these were author-private
  working notes that didn't belong in the repo. (#10)

## [0.1.0] - 2026-05-03

First public release on PyPI as
[`radish-rs`](https://pypi.org/project/radish-rs/0.1.0/).

### Added

- **Core data model**: `VolumeData`, `SweepData`, `MomentData`,
  `Coordinates`, `VolumeMetadata`, `SweepMetadata` — normalized to
  CfRadial2 / FM301.
- **Backend trait system**: `RadarBackend` trait with `scan_file`,
  `read_sweep`, `read_volume`, plus `auto_backend()` /
  `auto_backend_for_bytes()` dispatchers and a `can_read_bytes` /
  `read_bytes_volume` extension for in-memory inputs.
- **CfRadial1 backend** (NetCDF input). Migrated from `netcdf` 0.9
  to 0.12 and from the `hdf5` crate to `hdf5-metno`.
- **NEXRAD Level 2 backend** (Archive II input):
  - MSG_2 + MSG_5 + MSG_31 decoding.
  - Parallel LDM bzip2 decompression via the upstream `nexrad`
    crate's `parallel` feature (mandatory; see `CLAUDE.md` for the
    ~6× throughput gotcha if disabled).
  - `read_nexrad_bytes` for in-memory single buffers.
  - **Chunk-stream reader** for `unidata-nexrad-level2-chunks` —
    decode a volume from a list of byte chunks without first
    re-assembling the file.
  - MSG_2/MSG_5 root and per-sweep attrs surfaced for engine-swap
    parity with xradar (`Dataset.attrs` keys match).
  - Structural shape matches xradar's `open_nexradlevel2_datatree`
    for drop-in compatibility.
- **IRIS / Sigmet RAW backend** (PPI + RHI):
  - Per-sweep + volume attrs (`SigmetVolumeAttrs`,
    `SigmetSweepAttrs`).
  - Bytes input.
  - Criterion + Python wall-clock benchmarks vs xradar.
  - Integration tests (Rust + Python).
- **FM301 scalar variables** — `sweep_mode`, `sweep_number`,
  `sweep_fixed_angle`, `prt_mode`, `follow_mode` emitted as scalar
  data variables on each sweep group.
- **Python bindings** (PyO3 0.22): `read_cfradial1`, `scan_cfradial1`,
  `read_nexrad`, `scan_nexrad`, `read_sigmet`, `scan_sigmet`,
  `open_datatree`. Move-on-read ownership model so a typical
  xarray-driven open avoids cloning per-moment payloads.
- **Unified entry point**: `radish.open_datatree(input, backend=...)`
  dispatches across paths, bytes, and lists of either.
- **xarray plugin**: `xarray.open_datatree(path, engine="radish")`
  routes to the right backend automatically.
- **Structural parity tests vs xradar** — pin the data-tree shape
  for sigmet today, NEXRAD-ready scaffolding in place.
- **Long-form documentation** — `ARCHITECTURE.md`,
  `GETTING_STARTED.md`, `PROJECT_SUMMARY.md` (covering the Phase 0/1/2
  plan), all later moved under `docs/` in `[Unreleased]`.

### Known limitations

- Linux aarch64 + Windows wheels are not yet shipped (cross-compile
  and vcpkg static-lib hdf5 issues respectively); the sdist is the
  fallback for those platforms.
- CfRadial2 native reader and ODIM H5 backend are planned for
  Phase 2 (see `docs/PROJECT_SUMMARY.md`).

[Unreleased]: https://github.com/aladinor/radish/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/aladinor/radish/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/aladinor/radish/releases/tag/v0.1.0

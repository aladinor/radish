# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `docs/` folder consolidating long-form documentation (`ARCHITECTURE.md`,
  `GETTING_STARTED.md`, `PROJECT_SUMMARY.md`).
- `docs/CHANGELOG.md` (this file) and `docs/README.md` index, kicking off
  formal release-note tracking.

### Changed

- Cross-references in `CLAUDE.md`, `docs/GETTING_STARTED.md`, and
  `docs/PROJECT_SUMMARY.md` updated to point at the new `docs/` paths.

## [0.1.0] - 2026-05-03

First public release on PyPI as
[`radish-rs`](https://pypi.org/project/radish-rs/0.1.0/).

### Added

- **Core data model**: `VolumeData`, `SweepData`, `MomentData`,
  `Coordinates`, `VolumeMetadata`, `SweepMetadata` — normalized to
  CfRadial2 / FM301.
- **Backend trait system**: `RadarBackend` trait with `scan_file`,
  `read_sweep`, `read_volume`, plus `auto_backend()` /
  `auto_backend_for_bytes()` dispatchers.
- **CfRadial1 backend** (NetCDF input).
- **NEXRAD Level 2 backend** (Archive II input, MSG_2 + MSG_5 + MSG_31
  decoding, parallel LDM bzip2 decompression via the upstream `nexrad`
  crate's `parallel` feature).
- **IRIS / Sigmet RAW backend** (PPI + RHI, per-sweep + volume attrs,
  bytes input).
- **Python bindings** (PyO3 0.22): `read_cfradial1`, `scan_cfradial1`,
  `read_nexrad`, `scan_nexrad`, `read_sigmet`, `scan_sigmet`,
  `open_datatree`. Move-on-read ownership model so a typical
  xarray-driven open avoids cloning per-moment payloads.
- **xarray plugin**: `xarray.open_datatree(path, engine="radish")`
  routes to the right backend automatically.
- **Per-sweep + volume parity attrs**: `nexrad_attrs` and `sigmet_attrs`
  on `VolumeMetadata` / `SweepMetadata` reproduce xradar's
  `Dataset.attrs` keys verbatim for engine-swap compatibility.
- **Release pipeline**: GitHub Actions trusted publishing (OIDC) to
  PyPI, manylinux_2_28 wheels for Linux x86_64 + macOS x86_64 +
  macOS arm64.
- **Documentation**: `docs/ARCHITECTURE.md`, `docs/GETTING_STARTED.md`,
  `docs/PROJECT_SUMMARY.md`, `docs/RELEASING.md`, plus crate-level
  rustdoc and PyO3 docstrings.

### Known limitations

- Linux aarch64 + Windows wheels are not yet shipped (cross-compile and
  vcpkg static-lib hdf5 issues respectively); the sdist is the
  fallback for those platforms.
- CfRadial2 native reader and ODIM H5 backend are planned for
  Phase 2 (see `docs/PROJECT_SUMMARY.md`).

[Unreleased]: https://github.com/aladinor/radish/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/aladinor/radish/releases/tag/v0.1.0

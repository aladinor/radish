# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **NEXRAD: end-to-end `decode_volume(bytes) -> Scan` + parity
  harness against `danielway/nexrad`** — Phase 5+6 of plan 0003.
  New `decode/model.rs` lands the radish-internal `Scan` / `Sweep`
  / `Radial` / `Site` types with owned gate-byte buffers
  (`OwnedMoment` / `OwnedCfp`) so the returned tree is
  self-contained — matches the existing
  `nexrad_model::data::Radial` ownership shape that radish's
  adapter consumes today. `decode_volume` ties LDM split + bzip2
  + typed message decode + sweep grouping in one call. Sweep
  grouping uses the **ICD §3.2.4.17 radial_status start/end
  markers** (audit-required: SAILS / MRLE supplemental cuts that
  re-use a previous `elevation_number` form their own short
  sweep instead of merging into the parent — the divergence the
  earlier `danielway/nexrad` audit flagged).
  `radish/tests/test_nexrad_internal_parity.rs` adds two gated
  tests: KLOT structural parity + KILX phantom-radial divergence
  (pins `danielway/nexrad`'s known-wrong 6840-rays / 360-in-sweep_10
  output as a canary). Live KLOT fixture validates: 12 sweeps,
  KLOT lat/lon ≈ 41.6°N / -88.1°W, every sweep has REF moment.
  Not yet wired into the runtime path — Phase 7 swaps the call
  site. (#16)
- **NEXRAD: typed MSG_2 (RDA Status) + MSG_5 (Volume Coverage
  Pattern) parsers** at
  `radish/src/backends/nexrad/decode/messages/{msg2,msg5}.rs` —
  Phase 4 of plan 0003. MSG_2 is a flat 60-halfword
  fixed-frame parser (ICD §3.2.4.6 Table IV) covering all 30+
  status/calibration fields including the bit-packed
  `rda_scan_and_data_flags` (HW 14) that radish's existing
  `attrs.rs` consumes for the AVSET/EBC parity attrs. MSG_5
  decodes the 11-halfword header + N×23-halfword elevation cuts
  (ICD §3.2.4.12 Table XI), including ICD Table III-A binary-
  angle decoding for commanded elevation angles. The fixed-frame
  branch in `decode_messages` now dispatches MSG_2/MSG_5 to typed
  parsers in both single-segment and multi-segment (reassembled)
  paths via new `parse_fixed_frame_payload` /
  `parse_reassembled_payload` helpers; everything else stays
  `Raw` / `Reassembled`. Live KLOT fixture validation: typed MSG_2
  decodes plausible bounds (rda_build_number 19xx-24xx,
  vcp_magnitude in ICD range 1..767), typed MSG_5 advertises the
  same VCP as MSG_2 with first cut elevation ≈ 0.5°. (#15)
- **NEXRAD: typed MSG_31 (Digital Radar Data Generic Format)
  parser** at `radish/src/backends/nexrad/decode/messages/msg31/`
  — Phase 3 of plan 0003. Decodes the 72-byte per-radial data
  header (ICD §3.2.4.17.1 Table XVII-A: ICAO + collection time +
  azimuth/elevation + 10 data block pointers), the VOL/ELV/RAD
  info blocks (Tables XVII-E/F/H, with legacy 16-byte and modern
  24-byte RAD layouts auto-detected via `lrtup`; legacy 40-byte
  and modern 48-byte VOL likewise), the generic moment block
  shared by REF/VEL/SW/ZDR/PHI/RHO (Table XVII-B descriptor with
  ICD Table XVII-I gate decoding: `raw=0 → BelowThreshold`,
  `raw=1 → RangeFolded`, else `(raw - offset) / scale`), and the
  CFP block (Table XVII-Q clutter-status / power overlay). The
  message-iteration loop now dispatches MSG_31 to the typed
  parser via `MessagePayload::Msg31(Box<msg31::Msg31<'a>>)`;
  Skip / fixed-frame messages keep their `Raw` payload until
  Phase 4. Live KLOT fixture validates: 7200 typed MSG_31s
  parsed, first radial's VOL block carries KLOT's published
  lat/lon (~41.6°N, -88.1°W), modified Julian date matches
  2025-12-10 (20433). (#14)
- **NEXRAD: internal byte-level decoder infrastructure** at
  `radish/src/backends/nexrad/decode/` — first installment toward
  replacing the runtime dependency on `danielway/nexrad`.
  Lands typed `NexradDecodeError`, `SliceReader` with the
  load-bearing `try_skip_to(target)` resync that fixes the upstream
  phantom-radial bug, LDM record splitter + bzip2 (parallel via
  rayon), optional 24-byte Volume Header parser, `MessageHeader`
  per ICD §3.1.3 + §3.2.4.1 (28-byte: 12 TCM + 16 Table II logical),
  `MessageType` enum with explicit `Skip(u8)` for forward-compat,
  and a `decode_messages` iteration loop with the boundary fix from
  day one. Handles ICD Note 7's 0xFFFF variable-length sentinel and
  walks past LDM bzip2 trailing zero-padded frames silently. Not yet
  wired to the production read path — `read_nexrad` / `scan_nexrad`
  still go through the upstream `nexrad-data` decoder. Phase 7 of
  plan 0003 will swap the call site once Phase 3-6 fill in the
  per-message parsers and side-by-side parity tests. (#13)
- **NEXRAD test-corpus infrastructure** — new
  `RADISH_NEXRAD_FIXTURE_DIR` env-var convention (legacy single-file
  `RADISH_NEXRAD_FIXTURE` still honoured); both Rust
  (`radish/tests/test_nexrad.rs`) and Python
  (`python/tests/conftest.py`) test harnesses resolve fixtures
  from the directory with consistent fallback ordering. New
  `radish/tests/fixtures/CORPUS.md` documents the canonical KLOT +
  KILX corpus with SHA-256 sums, S3 URLs, `curl` / `fsspec`
  download recipes, and a deferred-fixture roster. New
  `corpus_sha256s_match_documentation` test pins file contents
  against documented sums so a maintainer who replaces a fixture
  with a slightly different S3 version gets a loud failure pointing
  at CORPUS.md before any parity test runs against drift-data. New
  `nexrad_kilx_fixture` Python fixture + `kilx_fixture()` Rust
  helper queued for the upcoming Phase 2 regression test. (#12)

### Changed

- **CI: Python matrix trimmed to 3.12 + 3.13** (was 3.9, 3.10, 3.11,
  3.12). Drivers: Python 3.9 reached EOL on 2025-10-31; the new
  internal decoder uses PEP 604 (`Path | None`) syntax requiring
  3.10+; consolidating now avoids re-trimming as the decoder grows.
  `python/pyproject.toml` `requires-python = ">=3.12"`. Wheel matrix
  drops from 12 to 6 (3 targets × 2 versions). Lint and type-check
  jobs bumped 3.11 → 3.13. (#12)
- **`docs/RELEASING.md`** — release matrix now exports
  `RADISH_NEXRAD_FIXTURE_DIR=$HOME/.cache/radish/fixtures/nexrad`
  and runs `cargo test -- --ignored` for the parity suite that
  lands in a later phase. Wheel-count claim updated to 6. (#12, #13)

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

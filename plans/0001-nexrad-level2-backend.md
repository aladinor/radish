# NEXRAD Level 2 Backend for Radish

## Context

Radish (`/home/alfonso-ladino/python/radish`) is a Rust-core radar library with a Python/xarray frontend. It currently ships a CfRadial1 backend; the goal is to add a **NEXRAD Level 2 (Archive II / AR2V)** backend that:

1. Decodes faster than the Python `xradar.io.open_nexradlevel2_datatree` reference (`/home/alfonso-ladino/python/xradar/xradar/io/backends/nexrad_level2.py`).
2. Plugs into radish's existing `RadarBackend` trait and `VolumeData` model (no parallel API).
3. Is consumable as `xr.open_datatree(path, engine="radish")` so xradar users can switch with one parameter change.
4. Handles **MSG_31** (modern, Build 19+) **and MSG_1** (pre-2008 legacy) in the MVP.

User chose a **spike-then-decide** strategy: a half-day Phase 0 evaluation of the upstream Rust `nexrad-decode` crate decides whether we ship a thin adapter (Path A, ~1 week) or write a from-scratch parser (Path B, ~3–4 weeks). The radish-side work (data-model wiring, error variants, PyO3 binding, xarray dispatch, tests) is identical in both paths — only the `nexrad/` internals differ.

User has a local NEXRAD fixture; tests/benches read from `RADISH_NEXRAD_FIXTURE`.

Perf target: ≥ 3× faster than xradar on a representative ~70 MB Archive II file; minimum acceptable 1.5×.

---

## Phase 0 — Spike & Decision Gate ✅ DONE (2026-05-01)

Goal: verify whether `nexrad-decode` + `nexrad-model` (MIT/Apache-2.0, `danielway/nexrad`) decodes faster than xradar **and** surfaces every field radish needs.

- [x] Created scratch crate `/tmp/nexrad-spike/` with `nexrad = "1.0.0-rc.4"`, `nexrad-model = "1.0.0-rc.2"`, `nexrad-decode = "1.0.0-rc.3"`, `anyhow = "1"`
- [x] Wrote `src/main.rs` calling `nexrad::load_file(...)`; prints decode time, VCP, sweep count, ray-0 metadata, and SweepField extraction per product
- [x] `cargo run --release`: decode median **115 ms** (3 runs: 111/114/115)
- [x] `xradar.io.open_nexradlevel2_datatree(path)`: median **2.92 s** (3 runs: 2.92/2.85/3.00) on KLOT 8 MB / 19 sweeps
- [x] Inspected upstream API surface — all needed fields present:
  - `scan.site()` → ICAO ("KLOT"), latitude (41.604), longitude (-88.084), height_meters (202), tower_height_meters (29)
  - `scan.coverage_pattern_number()` → VCP enum (e.g. `PrecipitationSz2_212`)
  - `scan.time_range()` → `(DateTime<Utc>, DateTime<Utc>)`
  - `Sweep::elevation_angle_degrees()`, `elevation_number()`, `radials()`
  - `Radial::azimuth_angle_degrees()`, `elevation_angle_degrees()`, `radial_status()`, `collection_time()`
  - All six moments + CFP via `Radial::reflectivity()`/`velocity()`/etc. → `Option<&MomentData>`
  - `nexrad::extract_field(sweep, Product) -> Option<SweepField>` returns flat `values() -> &[f32]` (already physical), `statuses() -> &[GateStatus]`, `azimuths() -> &[f32]`, `gate_count()`, `first_gate_range_km()`, `gate_interval_km()`, `label()`, `unit()` — perfect for `Array2::from_shape_vec`
  - **Gaps**: per-radial `nyquist_velocity`/`unambig_range` (RAD block) not on `Radial`; may need `nexrad-decode` low-level. **MSG_1** API path not yet validated against a legacy fixture (modern fixture only); runtime check in Phase 1.
- [x] **Decision: Path A (adapter)**. Total Rust ~224 ms (decode + extract) vs xradar 2.92 s = **~13× speedup**, well past the ≥ 3× target.
- [x] Edge case confirmed on the user's fixture: split-resolution sweep[0] — REF at 1832 gates / 0.25 km but ZDR/PHIDP/RHOHV/CCORH at 1192 gates in the same sweep. Plan's max-gates NaN-padding applies.
- [ ] Clean up `/tmp/nexrad-spike/` after Phase 1 lands

---

## Phase 1 — MVP Correctness (Path A) ✅ DONE (2026-05-01)

Goal: `read_volume` works end-to-end on MSG_31 files; Python wired; xarray dispatch works; numerical parity with xradar.

> **Path B (from-scratch) skipped** — Phase 0 picked Path A. The Path B section below remains for reference if a future regression forces a fallback.
>
> **MSG_1 legacy:** the user's fixture is modern NEXRAD (Build 19+, MSG_31 only). MSG_1 path is unverified end-to-end; deferred to Phase 3 when a legacy fixture is available. `nexrad-decode` documents MSG_1 support; the adapter is format-agnostic above the upstream API, so this is expected to "just work" when tested.

### 1a. Rust scaffolding ✅

- [x] Created `/home/alfonso-ladino/python/radish/radish/src/backends/nexrad/`
- [x] `mod.rs` — `pub struct NexradBackend; impl RadarBackend`
- [x] `mapping.rs` — moment name table (REF→DBZH, VEL→VRADH, SW→WRADH, ZDR→ZDR, PHI→PHIDP, RHO→RHOHV, CFP→CCORH) with units, standard_name, long_name; backed by `nexrad_model::data::Product`
- [x] `sniff.rs` — extension match, `AR2V` magic-bytes check, canonical `KXXXyyyymmdd_hhmmss` filename regex
- [x] `RadarBackend::can_read` overridden to OR all three signals
- [x] Added `Bzip2(String)`, `MalformedRecord { offset, msg }`, `Decode(String)` to `radish/src/error.rs`
- [x] `radish/src/backends/mod.rs` — `pub mod nexrad`, `pub use NexradBackend`, registered in `available_backends()`

### 1b. Path A — adapter implementation ✅

- [x] Workspace `Cargo.toml`: `nexrad = "1.0.0-rc.4"` (features: model + decode + data + chrono), `nexrad-model = "1.0.0-rc.2"` (chrono feature)
- [x] `nexrad/adapter.rs` — `convert_scan(scan, path) -> Result<VolumeData>`, `convert_sweep(sweep, idx) -> Result<SweepData>`, `build_volume_metadata(scan, source) -> Result<VolumeMetadata>`, `build_moment_array(field, nrays, max_gates) -> Array2<f32>`
- [x] Per-moment buffer pre-allocated with `f32::NAN`, populated from `SweepField`'s flat values+statuses (Valid → physical f32; BelowThreshold/RangeFolded/NoData → NaN), then `Array2::from_shape_vec` (zero copy)
- [x] **Bug fixed during testing**: `SweepField::from_radials` sorts radials by azimuth internally; the adapter now applies the same azimuth-sort permutation to azimuth/elevation/time coords so they line up ray-for-ray with the moment data. Without this, every ray was misaligned.
- [x] Field mapping: ICAO from `scan.site()`; lat/lon/altitude/altitude_agl from VOL block via `Site`; `time_coverage_start/end` from `scan.time_range()` (chrono feature); `sweep_fixed_angles` from `Sweep::elevation_angle_degrees`; `attributes["scan_name"] = "VCP-{number}"`; `platform_type = Fixed`; `institution = "NOAA/NWS"`.
- [x] `RadarBackend::read_volume` → `nexrad::load_file` → `adapter::convert_scan`
- [x] `RadarBackend::scan_file` and `read_sweep` implemented (eager full decode in Phase 1)
- [ ] **MSG_1 verified on legacy fixture** — deferred to Phase 3 (no legacy fixture available)

### 1d. Python wiring ✅

- [x] `python/src/lib.rs` — added `read_nexrad`/`scan_nexrad` pyfunctions, registered in `_radish` pymodule
- [x] `python/radish/__init__.py` — re-exported `read_nexrad`/`scan_nexrad` and added to `__all__`
- [x] `python/radish/backends/xarray_backend.py` — `_detect_format(p)` helper covering `.nc/.nc4/.netcdf`, `.ar2/.ar2v`, AR2V magic bytes, and the `KXXX########_######` filename pattern
- [x] `guess_can_open` returns `_detect_format(...) is not None`
- [x] `open_dataset` and `open_datatree` branch on format and call `read_nexrad` vs `read_cfradial1`
- [x] **Fix uncovered during testing**: xarray's plugin discovery rejects `**kwargs` in `open_dataset`; rewrote signature with explicit `drop_variables`, `group` and exposed `open_dataset_parameters` tuple. Pre-existing CfRadial1 path likely never worked through `engine="radish"` because of this.
- [x] NEXRAD root attrs include `Conventions = "ODIM_H5/V2_2"`, `source = "NEXRAD Level 2 Archive"`, `scan_name`, `instrument_name`, `institution` (matching xradar's surface)
- [x] **DataTree availability**: handle both legacy `datatree` package and modern `xarray.DataTree` (xarray ≥ 2024.10)

### 1e. Build & smoke test ✅

- [x] `cargo build --release -p radish` succeeds
- [x] `maturin develop --release` (run from `python/`) succeeds
- [x] `radish.read_nexrad(...)` round-trips on real KLOT fixture (decoded `instrument_name=KLOT`, 19 sweeps)

### 1f. Tests ✅

- [x] Unit tests in `mapping.rs` — moment mapping table (4 tests)
- [x] Unit tests in `sniff.rs` — magic bytes + filename pattern (3 tests)
- [x] Unit tests in `adapter.rs` — ICAO extraction from filename (2 tests)
- [x] Unit tests in `nexrad/mod.rs` — backend metadata + can_read (4 tests)
- [x] Rust integration test at `radish/tests/test_nexrad.rs`, gated on `RADISH_NEXRAD_FIXTURE`: asserts ICAO length 4, ≥ 5 sweeps, every sweep has DBZH or VRADH, ray-shape consistency across moments
- [x] Python parity test at `python/tests/test_nexrad.py`: opens same file via radish + xradar, aligns rays by azimuth bin (0.25° tol), compares 6 moments per sweep with plausibility windows. Result: **94 (sweep, moment) pairs match within atol=1e-3; only 3 of ~13,680 rays unmatched (sweep_4 AVSET truncation in xradar)**.
- [x] Python regression test — `engine="radish"` still detects `.nc/.nc4/.netcdf` as cfradial1
- [x] `python/tests/conftest.py` provides `nexrad_fixture` from env var with auto-skip

### 1g. Acceptance gate ✅

- [x] All Rust unit tests pass (14/14)
- [x] Rust integration test passes (2/2)
- [x] Python tests pass (5/5) including ray-for-ray parity vs xradar
- [x] **Wall-clock benchmark on 8 MB KLOT file (5 runs each)**: xradar median 2.88 s → radish median **1.07 s** = **2.69× faster**. Above the ≥1.5× minimum; below the 3× target — Phase 2 perf headroom analysis below.

### 1h. Pre-existing issues fixed during Phase 1

These were discovered while building and were necessary to make the workspace compile/test on the user's machine. Documented here so future readers know why these dep bumps happened.

- [x] Workspace `netcdf` 0.9 → 0.12 (old `hdf5-sys 0.8.1` rejects HDF5 1.14.5 on Ubuntu 25.04)
- [x] Workspace `hdf5` 0.8 → `hdf5-metno 0.12` (the maintained fork)
- [x] Migrated `cfradial1.rs` and `io/netcdf_utils.rs` to the netcdf 0.12 API (`Numeric → NcTypeDescriptor + Copy`, `AttrValue → AttributeValue`, `var.get(...) → var.get_value/get_values(...)`)
- [x] Fixed `if let Ok(var) = file.variable(...)` → `Some(var)` in cfradial1.rs (variable returns Option)
- [x] Fixed `python/pyproject.toml` `python-source = "python"` → `"."` (relative path was wrong from `python/`)

---

## Phase 2 — Performance ✅ DONE (2026-05-02)

Goal: ≥ 3× faster than xradar. **Achieved 16.96× on the user's KLOT fixture.**

### Final benchmark (8 MB KLOT, 19 sweeps, 5 runs)

| | xradar | radish | Speedup |
|---|---|---|---|
| Wall clock (`xr.open_datatree`) | 2.92 s | **0.172 s** | **16.96×** |
| Pure-Rust `read_volume` (criterion) | n/a | 150 ms | |
| `scan_file` (criterion) | n/a | 117 ms | |

### Where the wins came from

| Round | Change | Wall clock | Speedup |
|---|---|---|---|
| Phase 1 baseline | First working adapter | 1.07 s | 2.69× |
| /simplify cleanup | Coord zero-copy via `PyArray1::from_slice_bound` | 1.07 s | 2.73× |
| Zero-copy moment | `PyArray2::from_owned_array_bound`; `get_sweep`/`get_moment` move-out | 0.97 s | 3.00× |
| Sort-once adapter | One azimuth sort per sweep, decode `Product::moment_data` directly into the padded `Array2` (skip `SweepField`) | 0.90 s | 3.18× |
| `parallel` feature + drop double-copy | Enable `nexrad/parallel` (rayon-based LDM bzip2 decompress); `nexrad::data::volume::File::new(data)` direct path skips one 8 MB clone | 0.31 s | 9.30× |
| Parallel sweep adapter (rayon) | `convert_scan` uses `par_iter` over sweeps | 0.17 s | 16.78× |
| xradar root-attr parity | `scan_name = "VCP-212"` via `VCPNumber::number()`; surface `vcp` and `vcp_description` | 0.17 s | **16.96×** |

### 2a. Lazy paths

- [x] `read_sweep` — single-elevation reads currently still go through full `decode_scan` (~131 ms criterion). True lazy decode would need our own MSG_31 parser bypassing `File::scan()` (Phase 0 rejected the from-scratch path; not worth it now).
- [x] `scan_file` — currently 117 ms (full decode minus moment Array2 build). Can't go cheaper without the same from-scratch parser. **Decision: defer "header-only fast path" until a user need surfaces.**

### 2b. Throughput

- [x] Wall-clock harness committed (`python/examples/bench_nexrad_vs_xradar.py`)
- [x] Zero-copy `PyArray2::from_owned_array_bound` for moment hand-off
- [x] LDM chunk decompression parallelised via the upstream's `parallel` feature
- [x] Per-sweep adapter parallelised via rayon
- [ ] Profile with `cargo flamegraph` to find further wins. **Diminishing returns — the remaining 30 ms in the adapter is below memory-bandwidth floor for the ~108M f32 writes; the upstream's bzip2 decompress is the new ceiling.**

### 2c. Bench

- [x] Criterion dev-dep + `[[bench]] name = "nexrad" harness = false` in `radish/Cargo.toml`
- [x] `radish/benches/nexrad.rs` with `read_volume`, `scan_file`, `read_sweep[0]` benches reading `RADISH_NEXRAD_FIXTURE`
- [x] `python/examples/bench_nexrad_vs_xradar.py` runs and prints speedup
- [x] `radish/examples/time_decode.rs` — small fixed-loop harness for ad-hoc profiling
- [x] `RADISH_NEXRAD_FIXTURE=<path> cargo bench -p radish --bench nexrad` produces criterion report

### 2d. Acceptance ✅

- [x] Median radish ≥ 1.5× faster than xradar (achieved 16.96×)
- [x] Target: ≥ 3× faster than xradar (achieved 16.96×, **5.6× past target**)
- [ ] `scan_file` < 50 ms on the fixture (currently 117 ms; deferred per 2a)

### 2e. Critical gotcha discovered along the way

The single biggest win came from **enabling the `parallel` feature on `nexrad`**. The Phase 0 spike used the upstream's default features (which transitively enable `parallel` via `full`). When the radish workspace pinned `default-features = false, features = ["model", "decode", "data", "chrono"]`, that silently dropped `parallel` and decode time went from ~115 ms to ~700 ms — a 6× regression that wasn't visible in any test. Always lock features explicitly and benchmark.

The minimal correct feature set is:

```toml
nexrad = {
    version = "1.0.0-rc.4",
    default-features = false,
    features = ["model", "decode", "data", "chrono", "parallel"],
}
```

This avoids pulling `image` (render), `reqwest`+`tokio` (aws), and `nexrad-process` while keeping the parallel decompression path.

---

## Phase 3 — Edge Cases & xradar Attribute Parity (~3–5 days)

Goal: handle real-world NEXRAD quirks; root attrs match xradar's output for drop-in compatibility.

- [ ] Handle AVSET (cuts dropped mid-volume): `expected_sweeps` from MSG_5 may exceed actual; trust actual sweep count
- [ ] Handle super-res variable gate counts within a sweep (already covered by max-gates padding in Phase 1; verify with super-res fixture)
- [ ] Handle incomplete trailing sweep (chunk files / S-files) — seal in `finalize()` with `complete = false` flag, optionally drop in `xarray_backend.py`
- [ ] Handle missing MSG_5 gracefully — fall back to per-sweep elevation median for `sweep_fixed_angles` (already in Phase 1; verify on a fixture without MSG_5)
- [ ] Handle multi-resolution sweeps (REF at 1 km, VEL at 250 m): document Phase 3 option to split into `/sweep_0_z` and `/sweep_0_v` matching xradar; default keeps unified range axis
- [ ] Match xradar's full root attr set in `_create_root_dataset` for NEXRAD: `Conventions`, `version`, `title`, `source`, `history`, `references`, `comment`, `instrument_name`, `scan_name`
- [ ] Match xradar's per-sweep attrs: `sweep_mode = "azimuth_surveillance"`, `prt_mode`, `follow_mode`, `sweep_fixed_angle`
- [ ] Test against an AWS noaa-nexrad-level2 sample suite of 10 mixed VCPs

---

## Verification (final run, 2026-05-02) ✅

- [x] `cargo test --release -p radish` — 21 unit + 2 integration tests pass
- [x] `RADISH_NEXRAD_FIXTURE=<path> cargo test --release -p radish` — Rust integration tests pass on real fixture
- [x] `cd python && maturin develop --release && pytest tests/ -v` — 5/5 Python tests pass (NEXRAD parity + CfRadial1 regression)
- [x] `RADISH_NEXRAD_FIXTURE=<path> python python/examples/bench_nexrad_vs_xradar.py` — **16.96×** faster than xradar
- [x] `xr.open_datatree(<file>.nc, engine="radish")` still works (CfRadial1 regression)
- [x] `xr.open_datatree(<file>.ar2v, engine="radish")` produces a DataTree with `/sweep_0`, ... groups; root attrs include `scan_name = "VCP-212"`, `Conventions`, `source = "NEXRAD Level 2 Archive"`, `instrument_name = "KLOT"`, `vcp = "212"`, `vcp_description = "Precipitation, SZ-2"`, etc. (xradar-compatible shape)
- [x] `RADISH_NEXRAD_FIXTURE=<path> cargo bench -p radish --bench nexrad` — `read_volume` 150 ms / `scan_file` 117 ms / `read_sweep[0]` 131 ms; throughput 50–65 MiB/s

---

## Reference: data-model field sources (NEXRAD → radish)

| Radish field | NEXRAD source |
|---|---|
| `instrument_name` | volume header `icao` (4 ASCII bytes); fallback to filename prefix |
| `latitude`, `longitude`, `altitude` | first MSG_31's `VOL` block (`lat`, `lon`, `site_height + feedhorn_height`) |
| `altitude_agl` | VOL `feedhorn_height` |
| `time_coverage_start`/`end` | first/last ray: `(collect_date - 1) * 86400 + collect_ms / 1000` seconds since epoch |
| `volume_number` | MSG_2 if present, else 0 |
| `sweep_group_names` | `["sweep_0", "sweep_1", ...]` |
| `sweep_fixed_angles` | MSG_5 cut[i] elevation if present, else median of MSG_31 `elevation_angle` per sweep |
| `frequency` | `None` (not in ICD) |
| `platform_type` | `Some(PlatformType::Fixed)` |
| `attributes["scan_name"]` | `format!("VCP-{}", vcp_pattern_number)` |
| Per-sweep `nyquist_velocity` | RAD block `nyquist_vel` (cm/s ÷ 100) |
| Per-sweep `unambiguous_range` | RAD block `unambig_range` (1/10 km × 100) |
| `sweep_mode` | `SweepMode::Azimuth` (PPI surveillance) |
| Moment `scale_factor`/`add_offset` | `1.0` / `0.0` (already physical f32); `fill_value = NaN` |

Below-threshold and range-folded raw codes (0, 1) → `f32::NAN`.

---

## Critical files

- `/home/alfonso-ladino/python/radish/radish/src/backends/nexrad/mod.rs` *(new — `RadarBackend` impl)*
- `/home/alfonso-ladino/python/radish/radish/src/backends/nexrad/adapter.rs` *(new, Path A)* **or** `decode.rs` + `sweep_builder.rs` + `ldm.rs` + `msg31.rs` + `msg1.rs` + `msg5.rs` + `msg2.rs` + `blocks.rs` + `message.rs` *(new, Path B)*
- `/home/alfonso-ladino/python/radish/radish/src/backends/nexrad/mapping.rs` *(new — moment name/units table)*
- `/home/alfonso-ladino/python/radish/radish/src/backends/nexrad/sniff.rs` *(new — magic-byte + filename detection)*
- `/home/alfonso-ladino/python/radish/radish/src/backends/mod.rs` *(register NexradBackend)*
- `/home/alfonso-ladino/python/radish/radish/src/error.rs` *(add `Bzip2`, `MalformedRecord`, `Decode` variants)*
- `/home/alfonso-ladino/python/radish/radish/Cargo.toml` *(deps)*
- `/home/alfonso-ladino/python/radish/python/src/lib.rs` *(`read_nexrad`/`scan_nexrad` pyfunctions; Phase 2 zero-copy `from_owned_array_bound`)*
- `/home/alfonso-ladino/python/radish/python/radish/__init__.py` *(re-exports)*
- `/home/alfonso-ladino/python/radish/python/radish/backends/xarray_backend.py` *(format detect + dispatch)*
- `/home/alfonso-ladino/python/radish/python/tests/test_nexrad.py` *(parity test, acceptance gate)*
- `/home/alfonso-ladino/python/radish/python/tests/conftest.py` *(nexrad_fixture)*
- `/home/alfonso-ladino/python/radish/tests/test_nexrad.rs` *(Rust integration test)*
- `/home/alfonso-ladino/python/radish/radish/benches/nexrad.rs` *(criterion, Phase 2)*
- `/home/alfonso-ladino/python/radish/python/examples/bench_nexrad_vs_xradar.py` *(wall-clock harness)*

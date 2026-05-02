# NEXRAD MSG_2 / MSG_5 Attribute Parity (Phase B)

## Context

PR #1 shipped a NEXRAD Level 2 backend with structural xarray parity vs. xradar — same dims, coords, dtypes, variable set, per-DataArray CF metadata. Phase A explicitly deferred xradar's **MSG_2 / MSG_5 root and per-sweep attributes**. This plan closes that gap.

xradar emits **16 root-level attrs** (10 from MSG_5, 5 from MSG_2, 1 computed) and **9 per-sweep attrs** (all from MSG_5 elevation cuts). Most of MSG_5 is already accessible through `nexrad-data`'s `Scan::coverage_pattern()`; MSG_2 is silently discarded by `Scan::scan()` and requires a separate one-pass walk over `File::records()`.

---

## Approach

**Keep the existing fast `File::scan()` path.** It already decodes MSG_5 into a normalized `VolumeCoveragePattern` model that exposes everything we need for the **9 per-sweep attrs** and **10 of the 11 MSG_5-derived root attrs** via typed methods (no bitfield work in our code).

**Add a single MSG_2 walk** alongside the existing scan call: iterate `File::records()` once, pluck the first `MessageContents::RDAStatusData`, decode the 5 MSG_2 root attrs. MSG_2 lives in the first uncompressed record (~120 bytes), so the cost is negligible.

**Don't pollute the generic data model.** Format-specific attrs go in `Option<NexradVolumeAttrs>` and `Option<NexradSweepAttrs>` nested structs on the existing `VolumeMetadata` / `SweepMetadata`. CfRadial1 leaves them `None`.

---

## Reference: Field map

### Root attrs (16) — match xradar verbatim

| Attr name | Source | Upstream method |
|---|---|---|
| `scan_name` | MSG_5 | already wired (`f"VCP-{n}"`) |
| `dynamic_scan_type` | MSG_5 | derive from `sails_enabled` + `sails_cuts` + `mrle_enabled` + `mrle_cuts` → `"SAILS x N"` / `"MRLE x N"` / `"standard"` |
| `mpda_vcp` | MSG_5 | `vcp.mpda_enabled()` |
| `base_tilt_vcp` | MSG_5 | `vcp.base_tilt_enabled()` |
| `num_base_tilts` | MSG_5 | `vcp.base_tilt_count()` |
| `vcp_truncated` | MSG_5 | `vcp.truncated()` |
| `vcp_sequence_active` | MSG_5 | `vcp.sequence_active()` |
| `number_elevation_cuts` | MSG_5 | `vcp.number_of_elevation_cuts()` |
| `doppler_velocity_resolution` | MSG_5 | `vcp.doppler_velocity_resolution()` (returns f32: 0.5 or 1.0) |
| `vcp_pulse_width` | MSG_5 | `vcp.pulse_width()` → `"short"` / `"long"` |
| `avset_enabled` | MSG_2 | `msg2.rda_scan_and_data_flags().avset_enabled()` |
| `ebc_enabled` | MSG_2 | `msg2.rda_scan_and_data_flags().ebc_enabled()` |
| `super_res_status` | MSG_2 | `msg2.raw_super_resolution_status()` (int code) |
| `rda_build_number` | MSG_2 | `msg2.raw_rda_build_number()` (int) |
| `operational_mode` | MSG_2 | `msg2.raw_operational_mode()` (int code) |
| `actual_elevation_cuts` | computed | `volume.num_sweeps()` |

### Per-sweep attrs (9) — all from MSG_5 elevation cuts

| Attr name | Source | Upstream method |
|---|---|---|
| `waveform_type` | MSG_5_ELEV | xradar-style string from `cut.waveform_type()` (1→`contiguous_surveillance`, 2→`contiguous_doppler`, 3→`batch`, 4→`staggered_pulse_pair`) |
| `channel_config` | MSG_5_ELEV | xradar-style string from `cut.channel_configuration()` (0→`constant_phase`, 1→`random_phase`, 2→`sz2_phase_coding`) |
| `super_resolution` | MSG_5_ELEV | int reconstructed from 4 super-res bools (bit0=half_deg_az, bit1=quarter_km_refl, bit2=dop_300, bit3=dualpol_300) |
| `sails_cut` | MSG_5_ELEV | `cut.is_sails_cut()` |
| `sails_sequence_number` | MSG_5_ELEV | `cut.sails_sequence_number()` |
| `mrle_cut` | MSG_5_ELEV | `cut.is_mrle_cut()` |
| `mrle_sequence_number` | MSG_5_ELEV | `cut.mrle_sequence_number()` |
| `mpda_cut` | MSG_5_ELEV | `cut.is_mpda_cut()` |
| `base_tilt_cut` | MSG_5_ELEV | `cut.is_base_tilt_cut()` |

⚠️ xradar's `_WAVEFORM_TYPES` only defines codes 0-4; upstream Rust enum has codes 1-5 (`CS`/`CDW`/`CDWO`/`B`/`SPP`). xradar collapses the two contiguous-doppler waveforms (ICD codes 2 and 3) under one string. Our adapter does the same to match xradar exactly: `CS`→`contiguous_surveillance`, `CDW`/`CDWO`→`contiguous_doppler`, `B`→`batch`, `SPP`→`staggered_pulse_pair`, `Unknown`→`not_applicable`.

---

## Phase B1 — Rust data model

- [ ] Add `radish/src/model/nexrad_attrs.rs` defining `NexradVolumeAttrs { dynamic_scan_type: String, mpda_vcp: bool, base_tilt_vcp: bool, num_base_tilts: u8, vcp_truncated: bool, vcp_sequence_active: bool, number_elevation_cuts: u32, doppler_velocity_resolution: f32, vcp_pulse_width: String, avset_enabled: bool, ebc_enabled: bool, super_res_status: u16, rda_build_number: u16, operational_mode: u16, actual_elevation_cuts: u32 }` and `NexradSweepAttrs { waveform_type: String, channel_config: String, super_resolution: u8, sails_cut: bool, sails_sequence_number: u8, mrle_cut: bool, mrle_sequence_number: u8, mpda_cut: bool, base_tilt_cut: bool }`
- [ ] Add `pub nexrad: Option<NexradVolumeAttrs>` to `VolumeMetadata` (default `None`)
- [ ] Add `pub nexrad: Option<NexradSweepAttrs>` to `SweepMetadata` (default `None`)
- [ ] Re-export from `radish/src/model/mod.rs`
- [ ] Update `VolumeMetadata::new()` and `SweepMetadata::new()` to default `nexrad: None`

## Phase B2 — Rust adapter wiring

- [ ] In `radish/src/backends/nexrad/mod.rs`, after `File::decompress()` but before throwing it away, walk `file.records()` once and decode the first MSG_2 (`MessageContents::RDAStatusData`); return both the `Scan` and the optional `RdaStatusMessage` to the adapter
- [ ] In `radish/src/backends/nexrad/adapter.rs::convert_scan`, build `NexradVolumeAttrs` from `scan.coverage_pattern()` + the MSG_2 wrapper (use `into_owned()` to satisfy the lifetime); set `metadata.nexrad = Some(...)`
- [ ] In `convert_sweep`, build `NexradSweepAttrs` from the corresponding `ElevationCut` (looked up by sweep index in `coverage_pattern.elevation_cuts()`)
- [ ] Add a small helper `nexrad/attrs.rs` with `pub(crate) fn waveform_type_str(WaveformType) -> &'static str` and `pub(crate) fn channel_config_str(ChannelConfiguration) -> &'static str` matching xradar's mapping
- [ ] Add a `pub(crate) fn dynamic_scan_type(vcp: &VolumeCoveragePattern) -> String` that mirrors xradar's `_get_dynamic_scan_type` (SAILS / MRLE / standard)
- [ ] Handle the missing-MSG_2 case: leave the 5 MSG_2-derived fields as zero/false (matches xradar's `.get(..., default)` behavior)

## Phase B3 — PyO3 bindings

- [ ] In `python/src/lib.rs`, add `PyNexradVolumeAttrs` and `PyNexradSweepAttrs` `#[pyclass]` wrappers with one `#[getter]` per field
- [ ] Add `nexrad_attrs(&self) -> Option<PyNexradVolumeAttrs>` getter on `PyVolumeMetadata`
- [ ] Add `nexrad_attrs(&self) -> Option<PyNexradSweepAttrs>` getter on `PySweepData` (or `PySweepMetadata`)
- [ ] Re-export the two new classes in the `_radish` pymodule
- [ ] Re-export from `python/radish/__init__.py`

## Phase B4 — xarray backend

- [ ] In `python/radish/backends/xarray_backend.py::_create_root_dataset`, when format is NEXRAD pull `nexrad_attrs` from the volume and merge all 16 attrs into the root Dataset's `attrs` dict
- [ ] In `_sweep_to_dataset`, when format is NEXRAD pull per-sweep `nexrad_attrs` and merge all 9 attrs into the sweep Dataset's `attrs` dict
- [ ] Make sure attr dtypes are Python primitives (`bool`, `int`, `float`, `str`) — no numpy scalars — so `xr.DataTree.equals` against xradar can match

## Phase B5 — Tests

- [ ] Unit test in `radish/src/backends/nexrad/attrs.rs`: waveform_type_str / channel_config_str / dynamic_scan_type table-driven cases (incl. CDW vs CDWO collapsing to contiguous_doppler, SAILS+0 → "SAILS", SAILS+1 → "SAILS x 1")
- [ ] Unit test on `NexradVolumeAttrs::Default` round-trip via serde
- [ ] Python test extending `python/tests/test_nexrad.py`: assert all 16 root attrs are present on the radish DataTree root with the expected types and values matching the xradar fixture
- [ ] Python test asserting all 9 per-sweep attrs are on each `/sweep_N` ds.attrs
- [ ] Update `test_radish_xradar_structural_parity` to include attr-set comparison: `set(rd.ds.attrs) >= set(xd.ds.attrs)` for the documented attr list

## Phase B6 — Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test --all-features`
- [ ] `cd python && maturin develop --release && pytest tests/ -v`
- [ ] Re-run `python/examples/bench_nexrad_vs_xradar.py` — confirm speedup is unchanged (extra MSG_2 walk should add < 1ms)
- [ ] Open the smoke-test notebook and confirm `xr.DataTree.equals(rd, xd)` is closer (or, if attr values diverge in non-trivial ways, document the diff)

## Phase B7 — Open PR

- [ ] Push branch `feat/nexrad-msg2-msg5-attrs`
- [ ] Open PR titled `feat(nexrad): surface MSG_2 + MSG_5 root and per-sweep attrs`
- [ ] PR body: motivation (xradar parity), scope (16 root + 9 per-sweep), how (typed nested struct, no string-coercion), test plan, deferred items (MSG_1 legacy fixture, more VCPs)

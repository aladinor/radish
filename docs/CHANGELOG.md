# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Low-level NEXRAD per-moment decoders** —
  `radish.decode_nexrad_record_moment`,
  `radish.decode_nexrad_sweep_moment`,
  `radish.nexrad_record_moment_encoding`,
  `radish.nexrad_sweep_moment_encoding`, and the
  `radish.MomentEncodingError` exception. These pull **one moment** out of
  **one LDM record** (or one sweep-sized byte span) as the raw NEXRAD words,
  so chunked/lazy consumers — zarr codecs, virtual/byte-range reference
  stores, partial-volume reads — can decode exactly the bytes they need
  instead of a whole volume. A 120-radial × 1832-gate reflectivity block
  decodes in ~0.06 ms; the sweep variant decompresses records in parallel
  via rayon (~5× on 8 cores). Verified bit-identical to
  `xradar.io.open_nexradlevel2_datatree` on the first cut of every fixture
  in the corpus. (#32)

  The names are format-qualified (matching `read_nexrad` / `scan_nexrad`)
  so a future Sigmet/ODIM equivalent has room to exist. The unqualified
  spellings issue #32 introduced — `decode_record_moment`,
  `decode_sweep_moment`, `record_moment_encoding`, `sweep_moment_encoding`
  — are kept as first-class aliases referring to the same objects, so that
  issue's `hasattr(radish, "decode_record_moment")` check and any early
  code keep working.

  Output arrays are native-endian; a non-native dtype (`">u2"`) is
  **rejected** rather than silently satisfied, because an array that
  compares equal element-wise but whose `.tobytes()` is byte-swapped is
  exactly the corruption a zarr/reference-store caller would not notice.
  An implausible `out_shape` is rejected too — the allocator would
  otherwise `abort()`, which cannot be turned back into a Python
  exception and would take a long-lived worker down with it.

  The decoders read each Message 31 data block's own
  `word_size`/`scale`/`offset` rather than assuming a fixed encoding —
  NEXRAD moment encodings change across RDA builds (KVNX flipped ZDR from
  `8-bit, scale=16, offset=128` to `16-bit, scale=32, offset=418` on
  2020-06-02, so a decoder that assumes one encoding returns physically
  wrong values for the other era). Pass `scale=`/`offset=` to remap onto a
  common target grid; the remap is applied only when exactly representable
  and `MomentEncodingError` is raised otherwise. An undersized `out_shape`
  is likewise an error — radish never silently truncates gates or drops
  radials.

  Because one output array carries exactly one
  `scale_factor`/`add_offset`, blocks that disagree on
  `(word_size, scale, offset)` are refused unless a target grid is given
  — including blocks of the same width whose `scale`/`offset` differ, and
  including disagreements between separate LDM records in one sweep span.
  `sort_by_azimuth=True` reproduces `np.argsort(azimuth, kind="stable")`
  exactly, signed zero and NaN included, so callers can reorder their
  coordinate arrays to match.

- **KVNX cross-RDA-build fixtures** added to the test corpus
  (`radish/tests/fixtures/CORPUS.md`): `KVNX20200602_123502_V06` and
  `KVNX20200602_201830_V06`, the 8-bit and 16-bit ZDR eras either side of
  the 2020-06-02 upgrade outage. The earlier volume also pins a divergence
  where xradar's first cut has 719 rays with a 1.0° azimuth hole at
  ~90.75°, while radish returns all 720 at uniform 0.5° spacing — confirmed
  against a hand-rolled `bz2`/`struct` walk of the Message 31 headers and
  against radish's own independent volume reader. (#32)

- **Rust API (`radish::backends::nexrad::demux`): the public structs are
  `#[non_exhaustive]` with constructors**, so radish can add fields later
  without a breaking change. Build `DemuxOptions` with
  `DemuxOptions::new(moment, out_shape, word)` — `out_shape` is a `(rays,
  gates)` pair so the two dimensions can't be silently transposed — then
  set the `pub` `fill_value` / `target` fields directly; build
  `TargetEncoding` with `TargetEncoding::new(scale, offset)`. The returned
  `MomentEncoding` and `RecordInventory` are `#[non_exhaustive]` too. The
  enums (`MomentSelector`, `OutputWord`, `RawMoment`) stay exhaustive on
  purpose — their variants are closed domains fixed by the wire format.
  The Python API is unaffected. (#32)

### Fixed

- **KILX corpus documentation was inverted.** `CORPUS.md` and
  `python/tests/conftest.py` described `sweep_10` of
  `KILX20230629_154426_V06` as carrying 358 MSG_31 records with 360 being
  an upstream bug. The file carries **360** — a full 1° circle — and 358
  is what xradar reports. `radish/tests/test_nexrad_internal_parity.rs`
  has always asserted the correct 360; only the prose was wrong.
  Confirmed against radish's own reader, a hand-rolled `bz2`/`struct`
  walk, and Py-ART. (#32)

### Security

- **All outstanding `cargo audit` advisories resolved; the Security Audit
  CI job is green again with no `--ignore` entries.** It had been failing
  on every branch — `main`'s last green run predates the advisories.

  | Advisory | Crate | Resolution |
  | --- | --- | --- |
  | RUSTSEC-2026-0177 | pyo3 0.22.6 | pyo3 0.22 → 0.29 |
  | RUSTSEC-2025-0020 | pyo3 0.22.6 | pyo3 0.22 → 0.29 (was previously ignored in CI) |
  | RUSTSEC-2026-0204 | crossbeam-epoch 0.9.18 | `cargo update` → 0.9.20 |
  | RUSTSEC-2026-0185 | quinn-proto 0.11.14 | `cargo update` → 0.11.16 (high, 7.5) |

  The pyo3 bump also required `numpy` 0.22 → 0.29. The migration was
  small: `PyArray2::from_owned_array_bound` → `from_owned_array`,
  `PyArray1::from_slice_bound` → `from_slice`, and an explicit
  `from_py_object` opt-in on the five `#[pyclass]` types that derive
  `Clone` (pyo3 0.29 makes that derive opt-in; opting in preserves
  today's behaviour exactly). No API or behaviour change for Python
  callers.

  The old ignore was justified on the grounds that "the upstream
  `nexrad` crate ecosystem hasn't moved yet". That was stale —
  `cargo tree -i pyo3` shows pyo3 is pulled only by `numpy` and by
  radish itself, and `nexrad` is a dev-dependency that doesn't depend on
  pyo3 at all.

## [0.2.5] - 2026-05-05

The "every NEXRAD timestamp was +1 day" fix-only release. ICD 2620002R Table III §3.2.4.17 specifies the per-radial `modified_julian_date` field as 1-indexed days since 1970-01-01, but radish 0.2.2 through 0.2.4 computed `days * 86_400 + secs` (no `-1`), shifting every emitted timestamp by exactly +86,400,000 ms — every sweep, every ray, every file. xradar's `nexrad_level2.py:open_sweeps_as_dict` and danielway/nexrad's `volume/record.rs` both subtract 1; only radish disagreed. Filed by the raw2zarr maintainer, who currently mitigates with an in-process `-86400` shim that 0.2.5 lets them remove. (#26) Plus CI maintenance: GitHub Actions bumped to Node 24-compatible versions before the deprecation deadline (#25).

### Fixed

- **NEXRAD: every ray timestamp was off by exactly +1 day** — every
  sweep, every ray, every NEXRAD Level 2 file across radish-rs
  versions **0.2.2, 0.2.3, and 0.2.4**. ICD 2620002R Table III
  §3.2.4.17 specifies the MSG_31 / MSG_1 ray-header
  `modified_julian_date` field as **1-indexed days since
  1970-01-01** (day 1 = 1970-01-01), but radish computed
  `days * 86_400 + secs` (no `-1`), placing day 1 of the epoch at
  1970-01-02 instead of 1970-01-01. Every decoded timestamp was
  exactly +86_400_000 ms ahead of truth. Moment data
  (DBZH/VRADH/ZDR/PHIDP/RHOHV) was unaffected — only the time axis.

  **Affected APIs:** `radish.scan_nexrad`, `radish.scan`,
  `radish.scan_nexrad_chunks`, `radish.open_datatree`,
  `radish.open_dataset`, `radish.read_nexrad`,
  `radish.read_nexrad_chunks` — all share the same date decoder, so
  all of them were wrong. The bug surfaced in
  `metadata.time_coverage_start` / `time_coverage_end`,
  `nexrad_attrs.sweep_time_ranges[i]`, and per-sweep `time` xarray
  coordinates.

  **Severity:** critical for any consumer using the time axis.
  Time-series Zarr / icechunk stores keyed on `vcp_time` filed
  every record under the wrong date; joins with NWP / RAP / HRRR
  model output landed in the wrong analysis cycle; nowcasting
  pipelines mistimed adjacent sites.

  **Fix:** insert `-1` in both date-conversion call sites
  (`decode/model.rs::msg31_collection_time` and
  `decode/messages/msg1.rs::Msg1::collection_time`) so
  `unix_secs = (days - 1) * 86_400 + collection_time_ms / 1000`,
  matching xradar's `nexrad_level2.py:open_sweeps_as_dict` and
  danielway/nexrad's `volume/record.rs` byte-for-byte.

  **Verified:** the bug-report's reproducer
  (`s3://unidata-nexrad-level2/2025/12/13/KLOT/KLOT20251213_180112_V06`)
  now decodes to `time_coverage_start = 2025-12-13T18:01:12Z`
  matching the V06 filename truth. Plus 4 new regression tests:
  three unit tests pinning the day-1 boundary + the KLOT
  filename-truth fixture value, plus tightened integration tests
  on the KLOT 2025-12-10 + KVNX 2011 fixtures asserting the
  decoded date matches the filename-encoded one. Filed by the
  raw2zarr maintainer at
  `https://github.com/aladinor/raw2zarr` — they currently
  mitigate with an in-process `-86400` shim that this fix lets
  them remove. Thanks to the filer for the precise reproducer
  + cross-implementation comparison against xradar and
  danielway/nexrad.

## [0.2.4] - 2026-05-04

The "metadata-fast-path on bytes / streams" release. Closes the input-shape asymmetry between `read_nexrad` (path/bytes/file-like/chunks) and `scan_nexrad` (path-only) that 0.2.3 left in place. After 0.2.3, `radish.open_datatree(blob)` worked on pre-Build-12 NEXRAD via raw Archive II + Build-11 MSG_31 support, but the **metadata-only** fast path still required a temp-file workaround for S3 / fsspec / obstore inputs. 0.2.4 closes that gap with a new format-agnostic `radish.scan(filename_or_obj, backend=None)` dispatcher and the underlying `scan_nexrad_bytes` / `scan_nexrad_chunks` PyO3 functions. End-to-end on a modern KLOT V06 (5.8 MB): `radish.scan(blob)` ≈ 80 ms, vs `radish.open_datatree(blob)` ≈ 200 ms — the 2.5× speedup is now reachable on bytes input, matching what was already available on path input. (#21)

### Added

- **NEXRAD: `radish.scan` accepts bytes / file-like / chunk streams**,
  closing the input-shape asymmetry between `read_nexrad`
  (path/bytes/file-like/chunks) and `scan_nexrad` (path-only). New
  format-agnostic `radish.scan(filename_or_obj, backend=None)`
  dispatcher mirrors `radish.open_datatree` — same input-shape
  detection, returns `VolumeMetadata` instead of `xr.DataTree`.
  Two new PyO3 friend functions exposed: `scan_nexrad_bytes(data)`
  and `scan_nexrad_chunks(chunks)`. **Compression-agnostic**: caller
  passes already-decompressed AR2V bytes; for `.gz` archives use
  fsspec's `compression="gzip"` filter, `gzip.decompress(raw)`, or
  obstore registered as an fsspec backend
  (`from obstore.fsspec import register`). Closes the
  fail-fallback-to-xradar pattern in raw2zarr v0.18.0 PR #244 — the
  ~10× metadata-extraction speedup is now reachable on S3 input
  through a single `fsspec.open(uri, 'rb').read() →
  radish.scan(blob)` hop.

### Changed

- **`PyNexradVolumeAttrs` and `PyNexradSweepAttrs` now implement
  `__eq__`** (via PyO3's `pyclass(eq)` derived from the underlying
  Rust `PartialEq`). Lets users compare metadata across input
  shapes (e.g. `radish.scan(path).nexrad_attrs ==
  radish.scan(bytes).nexrad_attrs`) without walking every field —
  useful for the parity checks bulk-ingest workflows do per file.

## [0.2.3] - 2026-05-04

The "pre-Build-12 NEXRAD" release. Adds full support for NEXRAD Level 2 / Archive II files predating Build 12 (March 2012) — the format used by the entire 1991-2012 public archive on AWS / Unidata. `radish.scan(blob)` and `radish.open_datatree(blob)` now succeed on these files where they previously raised `unexpected EOF at offset 36`. Modern Build-12+ LDM files are unchanged (verified against KLOT, KILX). End-to-end smoke on `KVNX20110520_000442_V06.gz` (45.6 MB raw AR2): 17 sweeps × 720 az × 1832 range with full dual-pol moments (DBZH/ZDR/PHIDP/RHOHV) in ~780 ms. (#22)

### Added

- **NEXRAD: pre-Build-12 raw Archive II support, including
  Build-11.x MSG_31 layout.** Before this change, files predating
  Build 12 (March 2012) — e.g.
  `s3://unidata-nexrad-level2/2011/05/20/KVNX/...` — raised
  `unexpected EOF at offset 36` because the decoder assumed every
  file was wrapped in LDM-bzip2 records. radish now:

  1. Detects raw Archive II via the zero-valued `u32_be` at byte
     offset 24 (matches xradar's `nexrad_level2.py:309-319` and
     `danielway/nexrad`'s `volume/record.rs:139-156`) and walks
     the message stream directly without bzip2 decompression.
  2. Includes a new `messages::msg1` parser for the legacy MSG_1
     (Digital Radar Data, ICD §3.2.4.2 Table III) format used by
     1991-2008 files.
  3. **Detects the Build-11 MSG_31 layout (9 pointer slots, 68-byte
     header) vs Build-12+ (10 pointer slots, 72-byte header).**
     The CFP block was added in Build 12, so older MSG_31 messages
     reserve only 9 pointer slots in their data header. Detection
     uses the smallest non-zero pointer value (which always equals
     the on-wire header size by construction). Pointer arithmetic
     in `msg31::parse` was also corrected from
     `message_start_offset + ptr` to the canonical
     `header_offset + ptr` (= `start_position + ptr`,
     matching `danielway/nexrad`'s `digital_radar_data::Message::parse`
     and xradar's `block_pointer + 12 + LEN_MSG_HEADER`).
  4. Synthesizes a minimal MSG_5 (VCP) fallback when the source
     file lacks one — common on legacy raw files.

  **Verified:** `KVNX20110520_000442_V06.gz` (45.6 MB raw AR2)
  decodes through `radish.open_datatree` to 17 sweeps × 720 az ×
  1832 range with full dual-pol moments (DBZH/ZDR/PHIDP/RHOHV) in
  ~1.2 s end-to-end. Modern KLOT/KILX LDM files unchanged.

## [0.2.2] - 2026-05-04

The "internal NEXRAD decoder" release. radish now ships a from-scratch ICD-2620002AA-compliant Level 2 / Archive II decoder at `radish::backends::nexrad::decode`, replacing the runtime dependency on `danielway/nexrad`. The public Python and Rust surfaces are unchanged; output values are byte-identical to 0.2.0 except where 0.2.0 had bugs (KLOT VCP-32 surveillance sweeps now omit spurious `VRADH`/`WRADH` moments). Decode performance matches `danielway/nexrad` (1.01× ratio on KLOT and KILX) and is **7.78× faster than xradar** end-to-end through the xarray engine.

### Performance

- **NEXRAD: fused decompress + typed decode into one rayon
  par_iter step** in `decode_volume`. Each rayon worker now
  decompresses one LDM record AND walks its typed messages in
  the same task, so the typed parse + gate-byte copies run in
  parallel with bzip2 decompression instead of sequentially
  after it. Mirrors `nexrad-data-1.0.0-rc.7`'s `File::scan` shape.
  KLOT (5.8 MB): radish::decode_volume 143 → 125.5 ms (-12%),
  matching `danielway/nexrad`'s 127 ms (1.01× ratio).
  KILX (10.4 MB): 147 → 140.8 ms, danielway 142.3 ms.
  Python end-to-end vs xradar: KLOT 6.7× → 7.78×.

### Fixed

- **NEXRAD: MSG_31 data-block routing now goes by
  `DataBlockId.name`, not by ICD-slot index.** Real files pack
  the `data_block_count` valid blocks contiguously into the
  pointer slots in arrival order; the slot index doesn't
  determine the block type. Pre-fix, KLOT VCP-32 surveillance
  sweeps surfaced spurious `VRADH` / `WRADH` moments because
  pointer slot 4 (ICD's PTR_VEL) actually carried a `DZDR`
  block — its gate bytes got mislabeled as velocity.
  Post-fix matches xradar and `danielway/nexrad`'s name-based
  routing. Two new regression tests pin the behavior. (#17)

### Changed

- **NEXRAD: replaced `nexrad` / `nexrad-decode` / `nexrad-data` /
  `nexrad-model` runtime dependencies with the in-tree decoder
  at `radish::backends::nexrad::decode`** — Phase 7 of plan 0003.
  `NexradBackend::{read_volume, scan_file, read_sweep,
  read_bytes_volume, read_chunks_volume}` now route through
  `decode::decode_volume`. The upstream `nexrad` crate stays as
  a `[dev-dependencies]` reference for
  `tests/test_nexrad_internal_parity.rs` only; `cargo tree -p
  radish --edges normal` shows zero `nexrad-*` runtime deps.
  No public API change. The bundled bug-fix benefit is on
  `KILX20230629_154426_V06` where xradar reports 358 rays in
  sweep_10 vs the on-wire-correct 360 — the in-tree decoder
  matches `danielway/nexrad`'s 360 (xradar's stride bug
  documented at
  `xradar/io/backends/nexrad_level2.py:397`).

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
  tests: KLOT and KILX structural parity vs `danielway/nexrad`.
  ICD §3.2.4.17 field-by-field analysis of the previously suspect
  `KILX20230629_154426_V06` confirmed all 6840 MSG_31 records are
  on-wire valid (monotonic timestamps, sequential azimuth_numbers,
  `radial_status=1`, `spot_blank=0`); both our decoder and
  danielway correctly read all 6840. The retracted xradar issue
  #376 stands retracted — the off-by-2 was xradar's, traced to
  its `(recnum - 134) // 120` stride in
  `xradar/io/backends/nexrad_level2.py:397` hard-coding 120
  messages per LDM record (LDM 49 of KILX has 122 = 120 MSG_31 +
  2 MSG_2). Live KLOT fixture: 12 sweeps, KLOT lat/lon ≈
  41.6°N / -88.1°W, every sweep has REF moment. Not yet wired
  into the runtime path — Phase 7 swaps the call site. (#16)
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
  Lands typed `NexradDecodeError`, `SliceReader` with a
  `try_skip_to(target)` boundary-resync helper for defensive
  recovery from any future under-read, LDM record splitter + bzip2
  (parallel via
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

[Unreleased]: https://github.com/aladinor/radish/compare/v0.2.2...HEAD
[0.2.2]: https://github.com/aladinor/radish/compare/v0.2.0...v0.2.2
[0.2.0]: https://github.com/aladinor/radish/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/aladinor/radish/releases/tag/v0.1.0

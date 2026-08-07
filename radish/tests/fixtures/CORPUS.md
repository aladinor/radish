# NEXRAD Test Fixture Corpus

The fixtures themselves are **not committed** (large public files
available from NOAA's `unidata-nexrad-level2` S3 bucket). Tests resolve
their on-disk paths from the **`RADISH_NEXRAD_FIXTURE_DIR`**
environment variable. Set it to a directory containing the files
listed below.

## Default location

`~/.cache/radish/fixtures/nexrad/` is the recommended location; the
fixtures are reusable across radish, raw2zarr, and other downstream
tools that decode NEXRAD Level 2.

```bash
export RADISH_NEXRAD_FIXTURE_DIR="$HOME/.cache/radish/fixtures/nexrad"
```

## Required files

| Filename | Size | SHA-256 | Purpose |
| --- | ---: | --- | --- |
| `KLOT20251210_102338_V06` | 5,821,705 | `a5ed05d7dceaaceeb5adfb08601f10276a77a161ffdae7f302c49626e16cca81` | Modern happy-path baseline (Lincoln IL → reachable, light precip) |
| `KILX20230629_154426_V06` | 10,398,582 | `715c3c18691f6efe87a27127d631add8d90fd92c66a019a17965b624757180da` | Missing-radial divergence file — `sweep_10` carries **360** MSG_31 records on the wire (a full 1° circle) and radish must produce all 360; xradar reports 358. Independently confirmed by `danielway/nexrad`. See "The xradar LDM stride divergence" below. Pinned by `radish/tests/test_nexrad_internal_parity.rs`. |
| `KVNX20200602_123502_V06` | 1,920,466 | `fde3fda1ca80e7fc3d2d859cc591ee7c4da7a80b17c2166a19f6f7047950bd1c` | **8-bit-era** half of the cross-RDA-build pair (see below). ZDR is `word_size=8, scale=16.0, offset=128.0`; no CFP block. Also the missing-radial divergence file: its first cut has 720 MSG_31 radials at uniform ~0.5° spacing, but xradar reports 719 with a 1.0° azimuth hole at ~90.75°. radish must produce 720. |
| `KVNX20200602_201830_V06` | 4,063,422 | `cea716258763881b28f57483b65b144526e554bfe773aaa1df942c4a3024b855` | **16-bit-era** half of the pair. ZDR is `word_size=16, scale=32.0, offset=418.0`; CFP present. |

### The xradar LDM stride divergence

Three of the four fixtures expose an upstream xradar bug, so their
documented ray counts are **radish's**, not xradar's.

`xradar/io/backends/nexrad_level2.py:NEXRADRecordFile.init_record`
hard-codes a 120-message stride between LDM-compressed records
(`(recnum - 134) // 120`). NEXRAD ICD 2620010J §7.3.4 mandates a
*variable* count — 120 MSG_31 plus **zero or more** MSG_2 — so any LDM
record that interleaves MSG_2 overruns the budget and xradar drops the
trailing MSG_31s at the record boundary.

Observed drops (radish / `danielway/nexrad` -> xradar):

| Fixture | Cut | Wire | xradar |
| --- | --- | ---: | ---: |
| `KILX20230629_154426_V06` | `sweep_10` | 360 | 358 |
| `KVNX20200602_123502_V06` | `sweep_0` | 720 | 719 |
| `KVNX20200602_123502_V06` | `sweep_1` | 720 | 719 |
| `KVNX20200602_123502_V06` | `sweep_4` | 360 | 356 |
| `KVNX20200602_201830_V06` | `sweep_3` | 720 | 719 |
| `KVNX20200602_201830_V06` | `sweep_4` | 360 | 358 |
| `KLOT20251210_102338_V06` | — | — | no drops |

Every dropped radial sits at the tail of an LDM record, carries
`radial_status=1`, and has a sequential `azimuth_number` — i.e. they are
ordinary radials, not truncation artefacts.

Filed upstream as [openradar/xradar#376][x376] with a fix in
[openradar/xradar#377][x377]. Both were open at the time of writing; the
fix is **not** in xradar 0.12.0 and **not** on their `main`. When #377
merges, the xradar-parity tests in `python/tests/test_nexrad_demux.py`
that assert the divergence should start failing — that is the signal to
update them.

[x376]: https://github.com/openradar/xradar/issues/376
[x377]: https://github.com/openradar/xradar/pull/377

### The KVNX cross-RDA-build pair

The two `KVNX20200602_*` volumes straddle a ~7.7 h RDA upgrade outage on
2020-06-02 and encode ZDR differently on the wire:

| | ZDR raw | scale / offset | CFP |
| --- | --- | --- | --- |
| ≤ 2020-06-02 12:35 UTC | `uint8` | 16.0 / 128.0 | absent |
| ≥ 2020-06-02 20:18 UTC | `uint16` | 32.0 / 418.0 | present |

They are the regression gate for the per-moment decoders' remap logic
(issue #32): a decoder that assumes a fixed encoding silently returns
physically wrong values for the earlier volume. The `8 → 16` map is
`raw16 = 2 * raw8 + 162`, exact in physical units.

Tests that need them resolve `RADISH_NEXRAD_KVNX_DIR` first, then fall
back to `RADISH_NEXRAD_FIXTURE_DIR`.

## Acquiring the corpus

All four files are publicly accessible via anonymous S3:

```bash
mkdir -p ~/.cache/radish/fixtures/nexrad
cd ~/.cache/radish/fixtures/nexrad

curl -fsSLO "https://unidata-nexrad-level2.s3.amazonaws.com/2025/12/10/KLOT/KLOT20251210_102338_V06"
curl -fsSLO "https://unidata-nexrad-level2.s3.amazonaws.com/2023/06/29/KILX/KILX20230629_154426_V06"
curl -fsSLO "https://unidata-nexrad-level2.s3.amazonaws.com/2020/06/02/KVNX/KVNX20200602_123502_V06"
curl -fsSLO "https://unidata-nexrad-level2.s3.amazonaws.com/2020/06/02/KVNX/KVNX20200602_201830_V06"

sha256sum -c <<EOF
a5ed05d7dceaaceeb5adfb08601f10276a77a161ffdae7f302c49626e16cca81  KLOT20251210_102338_V06
715c3c18691f6efe87a27127d631add8d90fd92c66a019a17965b624757180da  KILX20230629_154426_V06
fde3fda1ca80e7fc3d2d859cc591ee7c4da7a80b17c2166a19f6f7047950bd1c  KVNX20200602_123502_V06
cea716258763881b28f57483b65b144526e554bfe773aaa1df942c4a3024b855  KVNX20200602_201830_V06
EOF
```

Or via Python `fsspec`:

```python
import fsspec
import shutil

paths = [
    ("s3://unidata-nexrad-level2/2025/12/10/KLOT/KLOT20251210_102338_V06",
     "KLOT20251210_102338_V06"),
    ("s3://unidata-nexrad-level2/2023/06/29/KILX/KILX20230629_154426_V06",
     "KILX20230629_154426_V06"),
    ("s3://unidata-nexrad-level2/2020/06/02/KVNX/KVNX20200602_123502_V06",
     "KVNX20200602_123502_V06"),
    ("s3://unidata-nexrad-level2/2020/06/02/KVNX/KVNX20200602_201830_V06",
     "KVNX20200602_201830_V06"),
]
for url, name in paths:
    with fsspec.open(url, mode="rb", anon=True) as src:
        with open(f"~/.cache/radish/fixtures/nexrad/{name}", "wb") as dst:
            shutil.copyfileobj(src, dst)
```

## Real-time chunk fixture (plan 0009)

One volume from the live `unidata-nexrad-level2-chunks` S3 feed,
captured as the 55 raw chunk objects (1 `S` + 53 `I` + 1 `E`) of KLOT
volume 2026-03-28 20:14:57 UTC. This is the only fixture that exercises
real S/I/E chunk framing — the volume header + LDM control word in the
`S` chunk, bare control-word-prefixed bzip2 records in `I` chunks, and
the negative (last-record) control word in the `E` chunk. Byte-splitting
a full archive file can never cover those.

Tests resolve it from **`RADISH_NEXRAD_CHUNKS_DIR`**:

```bash
export RADISH_NEXRAD_CHUNKS_DIR="$HOME/.cache/radish/fixtures/nexrad_chunks_KLOT"
```

| Fixture | Files | Tarball SHA-256 | Purpose |
| --- | ---: | --- | --- |
| `nexrad_chunks_KLOT/` (from `nexrad_level2_chunks_KLOT.tar.gz`) | 55 | `630b275011eb9e41d91aba24a54c97f744c5d3e61d0555941f1c42d7b336f5a2` | Real chunk framing; incomplete-sweep detection and drop/pad policy (plan 0009). Full volume decodes to 12 sweeps (720×6 split cuts, 360×6). `S + 10 I` chunks truncate mid-sweep: sweep 1 has 480/720 rays. |

Acquire via [open-radar-data](https://github.com/openradar/open-radar-data)
(the tarball is in its pooch registry) and extract:

```bash
python -c "from open_radar_data import DATASETS; print(DATASETS.fetch('nexrad_level2_chunks_KLOT.tar.gz'))"
mkdir -p ~/.cache/radish/fixtures
tar xzf ~/.cache/open-radar-data/nexrad_level2_chunks_KLOT.tar.gz -C ~/.cache/radish/fixtures/
```

Chunk object names follow the bucket convention
`YYYYMMDD-HHMMSS-NNN-[SIE]` (volume start time, 1-based sequence
number, chunk type). Lexicographic sort of the directory = scan order.

## Deferred fixtures

Add these to the corpus only if a parity-audit regression surfaces
during decoder Phase 6:

- **KAMX** (south-Florida, marine VCP) —
  `s3://unidata-nexrad-level2/<recent-date>/KAMX/KAMX...V06`
- **KFTG** (Denver, mountain backdrop) — same pattern, station `KFTG`
- **KMUX** (San Jose, west-coast precip) — same pattern, station `KMUX`
- **MSG_1 legacy file** (pre-2008) — pick a 2007 file from the
  `unidata-nexrad-level2/2007/...` prefix. Required when MSG_1
  legacy decoding lands (deferred to plan 0004).

## Test gating

- **Rust integration tests** that need a fixture skip cleanly when the
  env var is unset. See `radish/tests/test_nexrad.rs::fixture()` (and
  the new `kilx_fixture()` helper added by plan 0003 Phase 2).
- **Python tests** use the `nexrad_fixture` and `nexrad_kilx_fixture`
  fixtures in `python/tests/conftest.py`; they `pytest.skip()` on a
  missing env var.
- **Parity tests** (`radish/tests/test_nexrad_internal_parity.rs`) are
  marked `#[ignore]` so they don't slow `cargo test`. Run with
  `cargo test -- --ignored` once the corpus is in place.

## NEXRAD Level 3 (NIDS) corpus

Unmodified NIDS products pulled from `s3://unidata-nexrad-level3` (a flat
bucket — object keys ARE the filenames below, no date-prefixed path).
Not committed, same policy as the Level 2 corpus above. Tests resolve
their on-disk path from **`RADISH_NEXRAD_LEVEL3_FIXTURE_DIR`**.

```bash
export RADISH_NEXRAD_LEVEL3_FIXTURE_DIR="$HOME/.cache/radish/fixtures/nexrad_level3"
```

### Required files

| file | bytes | sha256 | product | why this one |
| --- | ---: | --- | --- | --- |
| `LOT_N0B_2026_07_31_13_06_53` | 224559 | `e4f0dd21d74dd5415bb5eb95e32d1126d0674f82662b9969cd7129dc2c54510b` | N0B | lowest tilt, packet 16, `LinearInteger` scaling — the primary reflectivity product |
| `LOT_N3B_2026_07_31_13_02_14` | 18644 | `fa66cfb820b08d37a401fe8f00afe6f58c30aec19daf745b8fdabcac6b319689` | N3B | half the azimuth resolution (360 vs 720 radials) — a genuinely different grid |
| `LOT_N0X_2020_03_30_00_02_07` | 54368 | `e25169aa65e0f191ca1e7022cb51d12d9c05490ffc0c0dcf48b9ff3bdc2b2fb4` | N0X | ZDR, the float32 scale/offset form |
| `LOT_N0C_2020_03_31_00_05_24` | 57703 | `dc705bb0a49482420944044c4cfcaf5bdf28a99af51811ab7e273de037652838` | N0C | RHOHV, float32 form |
| `LOT_N0K_2020_03_31_00_05_24` | 5109 | `8338080a732bd6920caad48734de327cb76a376d8fdbb9d66759a58d27ab45bc` | N0K | KDP, float32 form |
| `LOT_N0G_2026_08_04_00_09_57` | 98434 | `a9779831847031f2739fb8fa6e8ad38cc55af102d3b755d46fc877019479ca2f` | N0G | velocity, message code **154** (lower tilts) |
| `LOT_N2U_2026_08_04_00_09_57` | 18091 | `32c016b9fa19615fc1f4641ed0e68ee159c3fb620440e4a1de377d38fc8692d9` | N2U | velocity, message code **99** (upper tilts) — velocity is split by tilt (`G` on N0/N1/NA, `U` on N2/N3/NB), so N0G alone can't cover message code 99 |

All seven are KLOT, VCP 215. Reflectivity spans the six super-res tilts
(`N0B NAB N1B NBB N2B N3B` at 0.5/0.9/1.3/1.8/2.4/3.1°); N3B above is
tilt 5.

```bash
mkdir -p ~/.cache/radish/fixtures/nexrad_level3
cd ~/.cache/radish/fixtures/nexrad_level3

for f in LOT_N0B_2026_07_31_13_06_53 LOT_N3B_2026_07_31_13_02_14 \
         LOT_N0X_2020_03_30_00_02_07 LOT_N0C_2020_03_31_00_05_24 \
         LOT_N0K_2020_03_31_00_05_24 LOT_N0G_2026_08_04_00_09_57 \
         LOT_N2U_2026_08_04_00_09_57; do
  aws s3 cp --no-sign-request "s3://unidata-nexrad-level3/$f" .
done

sha256sum -c <<EOF
e4f0dd21d74dd5415bb5eb95e32d1126d0674f82662b9969cd7129dc2c54510b  LOT_N0B_2026_07_31_13_06_53
fa66cfb820b08d37a401fe8f00afe6f58c30aec19daf745b8fdabcac6b319689  LOT_N3B_2026_07_31_13_02_14
e25169aa65e0f191ca1e7022cb51d12d9c05490ffc0c0dcf48b9ff3bdc2b2fb4  LOT_N0X_2020_03_30_00_02_07
dc705bb0a49482420944044c4cfcaf5bdf28a99af51811ab7e273de037652838  LOT_N0C_2020_03_31_00_05_24
8338080a732bd6920caad48734de327cb76a376d8fdbb9d66759a58d27ab45bc  LOT_N0K_2020_03_31_00_05_24
a9779831847031f2739fb8fa6e8ad38cc55af102d3b755d46fc877019479ca2f  LOT_N0G_2026_08_04_00_09_57
32c016b9fa19615fc1f4641ed0e68ee159c3fb620440e4a1de377d38fc8692d9  LOT_N2U_2026_08_04_00_09_57
EOF
```

Or via Python `fsspec` (`anon=True`, same public bucket):

```python
import fsspec, shutil

names = [
    "LOT_N0B_2026_07_31_13_06_53", "LOT_N3B_2026_07_31_13_02_14",
    "LOT_N0X_2020_03_30_00_02_07", "LOT_N0C_2020_03_31_00_05_24",
    "LOT_N0K_2020_03_31_00_05_24", "LOT_N0G_2026_08_04_00_09_57",
    "LOT_N2U_2026_08_04_00_09_57",
]
for name in names:
    with fsspec.open(f"s3://unidata-nexrad-level3/{name}", mode="rb", anon=True) as src:
        with open(f"~/.cache/radish/fixtures/nexrad_level3/{name}", "wb") as dst:
            shutil.copyfileobj(src, dst)
```

### Expected-output sidecars (Tier 1 byte-parity)

`radish/tests/fixtures/nexrad_level3/expected/<fixture>.json` — **committed**
(small JSON, no raw arrays). Generated once, offline, from an independent
Python NIDS decoder used as the byte-level oracle, by
`radish/tests/fixtures/nexrad_level3/generate_expected.py`. Each sidecar
carries every scalar the decoder produces (site/product/moment/tilt/
message_code/vcp/elevation/scan_time/lat/lon/height/geometry/scale triple),
the full azimuth array, and `codes_sha256` — a SHA-256 of the oracle's
row-major `codes` array bytes, giving full-array byte-exactness without
committing the array itself (same reasoning as the Level 2 corpus's
uncommitted-fixtures policy above, applied to expected output instead of
input). Re-run the generator only when the fixture list changes; it needs
Python, numpy, and access to that oracle decoder — none of which the Rust
test (`test_nexrad_level3_parity.rs`) needs at run time.

## Test gating (NIDS)

- `test_nexrad_level3_parity.rs`'s fixture-parity cases are `#[ignore]`d
  and skip (rather than fail) when `RADISH_NEXRAD_LEVEL3_FIXTURE_DIR` is
  unset or a listed file is missing — same discipline as the Level 2
  parity tests above. Run with
  `RADISH_NEXRAD_LEVEL3_FIXTURE_DIR=... cargo test -p radish --test
  test_nexrad_level3_parity -- --ignored`.
- The sabotage-verify test in the same file is **not** `#[ignore]`d — it
  runs on every `cargo test` and needs no fixtures, since it only
  perturbs an in-memory byte array and checks the comparison catches it.

## Velocity dealiasing golden corpus

Reuses the NEXRAD Level 2 corpus above — no separate fixture download.
`radish/tests/test_dealias_parity.rs` decodes real velocity (`VRADH`)
sweeps from `KLOT20251210_102338_V06` with radish's own NEXRAD backend,
runs `radish::transforms::dealias_region_based` on them, and checks the
resulting fold-count array against a golden sidecar generated from a
**real Py-ART install** — see `radish/tests/fixtures/dealias/generate_expected.py`'s
module doc for exactly how (it deliberately uses radish's own decoded
array as the shared input to both sides, so this gate isolates
dealiasing-only parity from decode parity, which
`test_nexrad_internal_parity.rs` already covers separately).

**Py-ART version pinned: `2.2.0`** — any Py-ART install of the same
version will reproduce identical sidecars, since the golden output only
depends on Py-ART's own deterministic algorithm, not anything
environment-specific. Re-run `generate_expected.py` and update this pin
if upgrading the reference Py-ART version — a real maintenance point, not
a one-time setup step.

### Expected-output sidecars

`radish/tests/fixtures/dealias/expected/*.json` — **committed** (small
JSON, no raw arrays, same "hash the array, don't commit it" convention as
the NIDS corpus above): per case, sweep index, moment, nyquist,
`rays_wrap_around`, shape, valid-gate count, and `folds_sha256` — a
SHA-256 of the expected fold-count array's row-major `int32` bytes.

| sidecar | sweep | shape | valid gates | nonzero folds | folds_sha256 (prefix) |
| --- | ---: | --- | ---: | ---: | --- |
| `KLOT20251210_102338_V06_sweep1` | 1 (0.48°) | 720x1192 | 405,994 | 225,842 | `958d100460b5...` |
| `KLOT20251210_102338_V06_sweep9` | 9 (2.42°) | 360x1336 | — | — | `a067ddbc859b...` |

### Test gating (dealiasing)

- `test_dealias_parity.rs`'s fixture-parity cases are `#[ignore]`d and
  skip cleanly when `RADISH_NEXRAD_FIXTURE_DIR` is unset or the fixture
  is missing. Run with `RADISH_NEXRAD_FIXTURE_DIR=... cargo test -p
  radish --test test_dealias_parity -- --ignored`.
- The sabotage-verify test in the same file is **not** `#[ignore]`d.
- `radish/benches/dealias.rs` (criterion) also resolves
  `RADISH_NEXRAD_FIXTURE_DIR` and skips cleanly when unset. Run with
  `RADISH_NEXRAD_FIXTURE_DIR=... cargo bench --bench dealias`.

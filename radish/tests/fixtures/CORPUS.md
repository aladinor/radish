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

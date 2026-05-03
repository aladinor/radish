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
| `KILX20230629_154426_V06` | 10,398,582 | `715c3c18691f6efe87a27127d631add8d90fd92c66a019a17965b624757180da` | Phantom-radial divergence file — `sweep_10` has 358 MSG_31 records but the upstream `nexrad-decode 1.0.0-rc.3` parser produces 360 due to a byte-cursor desync. Our internal decoder must produce 358; the parity test pins this divergence as the correctness signal. |

## Acquiring the corpus

Both files are publicly accessible via anonymous S3:

```bash
mkdir -p ~/.cache/radish/fixtures/nexrad
cd ~/.cache/radish/fixtures/nexrad

curl -fsSLO "https://unidata-nexrad-level2.s3.amazonaws.com/2025/12/10/KLOT/KLOT20251210_102338_V06"
curl -fsSLO "https://unidata-nexrad-level2.s3.amazonaws.com/2023/06/29/KILX/KILX20230629_154426_V06"

sha256sum -c <<EOF
a5ed05d7dceaaceeb5adfb08601f10276a77a161ffdae7f302c49626e16cca81  KLOT20251210_102338_V06
715c3c18691f6efe87a27127d631add8d90fd92c66a019a17965b624757180da  KILX20230629_154426_V06
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

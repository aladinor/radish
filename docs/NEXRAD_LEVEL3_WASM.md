# NEXRAD Level 3 (NIDS) in radish, and radish in the browser

**Status:** design, 2026-08-06. Nothing here is built.

This document is written to be executed by someone (or some session) working
**inside the radish repo**, who has not seen the conversation that produced it.
It carries the full context deliberately, including absolute paths into a
*different* repository — [AtmoScale / radar-animation][atmoscale] — which is the
consumer that motivates the work.

> **Path convention.** radish's own docs use repo-relative paths
> (`docs/ARCHITECTURE.md`). Everything referring to radar-animation is written
> as an **absolute path**, because it is a separate checkout:
> `/home/alfonso-ladino/python/radar-animation/...`. Those files are the
> specification; read them before writing code.

---

## 1. Why this work exists

AtmoScale is a browser weather-radar product with two tiers:

| tier | data | path | who pays |
|---|---|---|---|
| **free / real-time** | NEXRAD **Level 3** (NIDS), last 6–12 frames | **browser reads public S3 directly** | nobody — NOAA pays egress |
| **paid / historical** | Level 2 archive, arbitrary depth | server + virtual-reference catalogue | the customer |

The free tier's decision — go serverless, no backend at all — was made on
2026-08-06 after the original reasoning was falsified and re-established on new
evidence. The decision record is here:

- `/home/alfonso-ladino/python/radar-animation/plans/realtime-first-wiring.md`
  — section **D0**, the decision itself and everything that refuted the previous
  answer.
- `/home/alfonso-ladino/python/radar-animation/docs/competitive-landscape.md`
  — the measurements behind it: four browser radar competitors, browser decode
  benchmarks, the GPU memory wall, and why loop depth is the paywall.

**The one thing that decision costs is a decoder.** Today a Python server
decodes NIDS and ships uint8 codes to the browser. With no server, the browser
must decode NIDS itself — bzip2 decompression plus the NEXRAD Level 3 product
format.

That decoder is what this document is about, and **radish is where it should
live.**

---

## 2. Why radish, and not a TypeScript port

The AtmoScale plan originally called for a TypeScript port of the Python
decoder, policed by a CI diff-test against the Python original. That plan is at
`/home/alfonso-ladino/python/radar-animation/plans/serverless-realtime.md`
(Phases 2–3) and **this document supersedes those two phases.**

The problem with a port is structural: two implementations of one binary format
that must agree byte-for-byte, forever. Level 2 and Level 3 share a single uint8
code space in AtmoScale, so a disagreement does not throw — it renders the same
storm differently in the free tier than in the paid one.

radish removes the problem instead of policing it:

```
radish (pure Rust core: decode, model, normalize)
  |
  +-- radish-python   (pyo3 / maturin, cdylib "_radish")  -> servers, notebooks, the paid tier
  +-- radish-wasm     (wasm-bindgen)                       -> the browser, the free tier
```

**One decoder, two bindings, nothing to diff.** The pyo3 binding already exists
(`python/Cargo.toml`, `crate-type = ["cdylib"]`, `pyo3` + `numpy`), which is the
condition that makes this pay off. Without a Python binding, a Rust decoder would
just be a *different* second implementation; with one, it is the only
implementation.

Secondary benefits, in honest order of importance:

1. radish gains a Level 3 backend it wants anyway — it currently reads
   CfRadial1, ODIM, IRIS/Sigmet and NEXRAD Level 2, and Level 3 is the obvious
   gap.
2. radish gains a `wasm32` target, which makes every backend it has
   browser-reachable, not only this one.
3. AtmoScale gets its decoder without owning a second codebase.

---

## 3. What already exists — three implementations, none of them this one

| where | language | scope | status | use it as |
|---|---|---|---|---|
| `/home/alfonso-ladino/python/radar-animation/src/server/nexrad_level3.py` | Python | **638 lines.** Packet 16 only. 6 products: 153, 154, 99, 159, 161, 163 | in production, 7 committed fixtures | **the ORACLE.** Byte parity with this is the acceptance gate |
| [openradar/xradar PR #392][xr392] | Python | **1,182 lines.** Packets 16, AF1F, 28. Codes 94, 99, 153, 154, 155, 159, 161, 163, 165 + precip products | open PR by `mgrover1` | **the BREADTH reference.** A second independent reading of the same ICD |
| `radish/src/backends/nexrad/` | **Rust** | Level 2 only — msg1, msg2, msg5, msg31 | shipped | **the SCAFFOLDING.** `decode/reader.rs`, `common/buffer.rs`, error types, the adapter -> model pipeline |

Two independent Python implementations of the same spec is a genuine asset. Where
they agree, the ICD reading is settled. **Where they disagree, stop and find out
why before encoding either into Rust** — that disagreement is information.

Note the difference in ambition. radar-animation's decoder is deliberately
narrow: it reads exactly what its dashboard serves and **refuses everything
else**. xradar's is a general reader. radish should land closer to xradar's
scope (it normalizes to CfRadial2/FM301, same as xradar) while satisfying
radar-animation's contract exactly on the products they share.

---

## 4. The contract — read this before writing any decode code

This section is the part most likely to be got wrong, because most of it is
counter-intuitive and all of it was learned by getting it wrong first. Sources:
the docstrings in `nexrad_level3.py` itself, which are unusually detailed and
explain *why* for each item.

### 4.1 The codes pass through untouched — that is the whole design

AtmoScale's wire format ships **raw uint8 codes**, not physical values. The
browser uploads them to an R8 texture and a shader maps them through a 256-entry
colour LUT. No float ever crosses the wire.

This works because AtmoScale's quantization table `QUANT`
(`/home/alfonso-ladino/python/radar-animation/src/server/frames.py:138`) is
**pinned to the NIDS products' own declared scales**. Code N means the same
physical number in both tiers, so a NIDS product's bytes are already on the
serving scale.

**Consequence for radish:** the decoder must expose the **raw codes plus the
declared scale**, not only normalized float values. radish's usual output is a
CfRadial2-style model with physical values — correct for its normal consumers,
and useless for this one. The API needs both:

```rust
sweep.values()   // f32, physical, NaN below the data floor  -- radish's normal model
sweep.codes()    // &[u8], verbatim, exactly as the file stored them
sweep.scaling()  // (value_min, value_increment, n_levels) as DECLARED in the PDB
```

### 4.2 Refuse, never silently rescale

If a product's declared scale is not the one the consumer serves that moment on,
the correct behaviour is a **hard error**, not a remap. See `_check_scale` at
`/home/alfonso-ladino/python/radar-animation/src/server/level3_source.py:225`.
Its reasoning: if the NWS ever re-scales a product, passing bytes through would
mis-colour every pixel by a constant, silently.

radish does not own that policy — the consumer does — but radish must **expose
the declared scale faithfully** so the consumer can enforce it. Do not normalize
it away.

### 4.3 Two scaling forms, and the bool that keeps them apart

The PDB (Product Description Block) encodes value scaling two different ways,
selected by message code:

| form | codes | where | meaning |
|---|---|---|---|
| **integer** | 153, 154, 99 | halfwords 22 / 23 / 24 | min x10, increment x10, level count |
| **float32** | 159, 161, 163 | halfwords 22–25 | a float32 scale and offset |

In the Python this is a `bool` field on `ProductSpec`, and the docstring records
why: it started as a free-text string, and a typo (`"Int"` instead of `"int"`)
routed silently to the float branch. A real `N0X` read that way reported "min
-716.8, increment 0.0". **In Rust, make this an enum.** The type system should
make the third state unrepresentable.

### 4.4 Facts that are NOT in the file

Three values look like they are in the file and are not:

- **Gate spacing is 250 m, and it does not come from the packet header.** The
  halfword that looks like a range scale is the **cosine of the elevation angle
  x1000** — it reads 999 on the 0.5-degree tilt and 998 on the 3.1-degree one.
  Dividing it by 1000 and calling it kilometres is meaningless. See the comment
  at `nexrad_level3.py:188`, which pins 250 m by two independent cross-checks.
- **First gate is 125 m** (`GATE_SPACING_M / 2`), the *centre* of bin 0. Verified
  against the ARCO Level 2 store for the same radar, which reports
  `first_gate_m = 2125.0` where NIDS data begins at bin 8.
- **The tilt and the moment come from the AWIPS id in the text header**, not from
  the message code. A message code identifies the *format*, not the tilt: all six
  reflectivity tilts report 153.

### 4.5 Azimuths are ray CENTRES, not the stored start angles

The file stores each radial's **start** angle plus its angular width. The
consumer's protocol wants the **centre** — `start + delta/2`. Publishing raw
start angles rotates every sweep by half a beamwidth counter-clockwise: 436 m of
displacement at 100 km on a 720-ray sweep, growing to ~2 km at the sweep edge.

### 4.6 Velocity is two products, split by tilt

The NWS splits velocity across two message codes. A one-letter-per-moment map
cannot express this, and that shape is what made velocity look absent from a
bucket that had been carrying it all along:

| letter | code | tilts | elevations |
|---|---|---|---|
| `G` | 154 | 0, 1, 2 | 0.5, 0.9, 1.3 |
| `U` | 99 | 3, 4, 5 | 1.8, 2.4, 3.1 |

Both declare min -63.5, increment 0.5, 254 levels.

### 4.7 The decompression bomb guard is not optional

`MAX_DECOMPRESSED_BYTES = 8 << 20` (`nexrad_level3.py:177`). The Python
decompresses **incrementally with a hard cap**, because 1.3 KB of crafted input
expands without bound otherwise. The largest real product is ~1.3 MB.

**This matters more in a browser than on a server.** A one-shot `decompress()`
in wasm reintroduces a hole that is already closed, in a context where the
attacker controls the input (any S3 key the page can be pointed at) and the
victim is the user's own tab.

### 4.8 Code 0 and code 1 are not data

`DATA_FLOOR_CODE = 2`. In every digital radial product, 0 is "below threshold"
and 1 is "range folded". Physical value is `value_min + (code - 2) *
value_increment`; codes below 2 are NaN / transparent.

---

## 5. Phase A — make radish compile to `wasm32-unknown-unknown`

**Do this first, before writing any Level 3 code.** It is the phase that can
fail in ways that change the plan, and it is independent of the decoder.

radish cannot target wasm today. From `Cargo.toml` (workspace) and
`radish/Cargo.toml`, which has **no `[features]` section at all** — every
dependency is unconditional:

| dependency | pin | problem on wasm32 | fix |
|---|---|---|---|
| `hdf5` (`hdf5-metno`) | 0.12 | C library, links libhdf5 | feature-gate behind `native` |
| `netcdf` | 0.12 | C library, pulls `hdf5-sys` | feature-gate behind `native` |
| `bzip2` | 0.6 | defaults to the C `libbz2` binding | switch to the pure-Rust backend (`libbz2-rs-sys`), or `bzip2-rs` |
| `rayon` | 1.10 | needs `wasm-bindgen-rayon` + cross-origin isolation | feature-gate; single-threaded on wasm |
| `ndarray`, `chrono`, `serde`, `serde_json`, `byteorder`, `bytemuck`, `thiserror`, `anyhow` | — | none expected | — |

Steps:

- [ ] `rustup target add wasm32-unknown-unknown`, then
      `cargo build -p radish --target wasm32-unknown-unknown` **before changing
      anything**. Record the actual error list. The table above is read from
      manifests, not from a build — **the first real build turns it from a
      prediction into a measurement**, and it may well find transitive blockers
      not listed here.
- [ ] Add `[features]` to `radish/Cargo.toml`. Suggested shape:
      `default = ["native"]`, `native = ["hdf5", "netcdf", "rayon"]`, with
      `cfradial1` and any netCDF/HDF5-backed backend gated on `native`.
- [ ] Make the `bzip2` backend pure Rust. **This benefits Level 2 too** — it is
      the same dependency Archive II LDM records use — so it is not
      wasm-only work.
- [ ] Gate `rayon` usage behind `#[cfg(feature = "rayon")]` with a serial
      fallback. Do not reach for `wasm-bindgen-rayon`: it requires COOP/COEP
      headers on the serving page, which is a real deployment constraint to
      impose on a static-hosted free tier.
- [ ] CI job: `cargo check -p radish --no-default-features --target
      wasm32-unknown-unknown`. Without this, the first `native`-only import
      merged after Phase A silently un-does it.

**Exit:** `radish` core compiles for wasm32 with `--no-default-features`, and CI
fails if that stops being true.

---

## 6. Phase B — the Level 3 backend

New module `radish/src/backends/nexrad_level3/`, following the existing backend
pattern (`docs/ARCHITECTURE.md`, "Backend Implementation Pattern"). Reuse
`backends/common/` — `buffer.rs`, `coords.rs`, `geometry.rs`, `sniff.rs` — rather
than writing new byte-reading helpers.

- [ ] **Sniff.** NIDS files begin with a WMO/AWIPS text header, separator
      `\r\r\n`, and the AWIPS id is a 6-char alphanumeric token (`N0BLOT`). Do
      **not** commit to the first 6-char token found — a NOAAPORT-style prefix
      line puts an unrelated one ahead of the AWIPS id. Keep scanning.
- [ ] **Message header + PDB.** Locate the message header by **validating**, not
      by pattern-searching: at the right offset the message code is a known one
      *and* the product description block is self-consistent.
- [ ] **Value scaling**, both forms, dispatched on an enum (see 4.3).
- [ ] **Symbology block** -> layer -> **digital radial data array packet
      (code 16)**: radial count, azimuth start/delta per radial, range step,
      run-length-encoded gates. Anything that is not packet 16 is a different
      geometry and must be **rejected**, not read as if it were this one.
- [ ] **Optional, matching xradar #392's breadth:** packet `AF1F` (RLE radials)
      and packet 28 (generic data, DPR/HHC). Not required by AtmoScale. Decide
      deliberately — see "Open questions".
- [ ] **bzip2** on the symbology block, incremental, with the hard cap from 4.7.
- [ ] **Product table.** At minimum the six AtmoScale products (153, 154, 99,
      159, 161, 163). Prefer xradar #392's fuller table if the extra codes are
      cheap — but every added code needs a fixture, or it is untested surface.
- [ ] **Model output:** the CfRadial2/FM301 normalization radish always produces,
      **plus** the raw-code accessors from 4.1. Both, not either.

**Exit:** one real product decodes to codes + georeference in Rust.

---

## 7. Phase C — the parity gate

This is the phase that makes the whole thing trustworthy, and it is not
optional. radar-animation's decoder is the oracle; radish must agree with it
byte for byte on the products they share.

**Seven fixtures already exist**, unmodified NIDS pulled from
`s3://unidata-nexrad-level3`, with a README explaining why each one was chosen.
Copy them into radish rather than re-downloading, so both repos test the same
bytes:

```
/home/alfonso-ladino/python/radar-animation/tests/server/fixtures/level3/
    README.md                          <- read this; it documents each fixture
    LOT_N0B_2026_07_31_13_06_53        N0B  tilt 0  720 x 1840  DBZH, int-scaled (153)
    LOT_N3B_2026_07_31_13_02_14        N3B  tilt 5  360 x 1161  HALF azimuth resolution
    LOT_N0G_2026_08_04_00_09_57        N0G  tilt 0  720 x 1200  velocity, code 154
    LOT_N2U_2026_08_04_00_09_57        N2U  tilt 4  360 x 1200  velocity, code 99
    LOT_N0X_2020_03_30_00_02_07        N0X  tilt 0  360 x 1200  ZDR,   float32-scaled (159)
    LOT_N0C_2020_03_31_00_05_24        N0C  tilt 0  360 x 1200  RHOHV, float32-scaled (161)
    LOT_N0K_2020_03_31_00_05_24        N0K  tilt 0  360 x 1200  KDP,   float32-scaled (163)
```

They are chosen to cover exactly the axes that break: both scaling forms, both
velocity message codes, and two different azimuth resolutions. `N3B` at
360 x 1161 is called out in that README as "the case most likely to break".

- [ ] Copy the seven fixtures into `radish/tests/fixtures/nexrad_level3/`, with
      SHA-256 sums — radish already does this for its Level 2 corpus
      (`radish/tests/fixtures/CORPUS.md`).
- [ ] Generate the expected output **from the Python oracle**, not by hand:
      codes array, azimuths, elevation, scaling triple, site lon/lat/height,
      scan time, first gate, gate spacing.
- [ ] Assert **byte-for-byte** equality on the code arrays. Not "close" — equal.
- [ ] Assert georeference equality within a **stated and justified** tolerance.
- [ ] **Sabotage-verify the gate.** Perturb one code in the Rust output and
      confirm the test goes red. A parity test that has never failed has not
      been shown to work. This is a standing rule in the consuming repo, and it
      has caught vacuous tests there more than once.
- [ ] Diff against **xradar #392** as well where scope overlaps. Three-way
      agreement is much stronger evidence than two-way.

**Exit:** CI cannot go green with a decoder that disagrees with the oracle.

---

## 8. Phase D — the wasm binding

New crate `wasm/` (sibling of `python/`), mirroring how the pyo3 binding is
structured.

- [ ] `crate-type = ["cdylib"]`, `wasm-bindgen`, and `radish` with
      `default-features = false`.
- [ ] Export the raw-code path as **zero-copy**. The consumer's next call is
      `gl.texImage2D` with the code array; a copy at the wasm/JS boundary is pure
      waste. `bytemuck::cast_slice` into a `Uint8Array` view over wasm linear
      memory. **Document the lifetime**: that view is invalidated when wasm
      memory grows.
- [ ] Keep it a **library, not an application**. No fetch, no S3, no worker
      logic inside the crate — bytes in, decoded sweep out. The consumer owns
      networking. This keeps the crate testable natively and keeps radish from
      growing a browser dependency.
- [ ] **Measure and record**: `wasm-opt -O3` binary size (gzipped), and decode
      milliseconds on the real 224 KB `LOT_N0B` fixture — not a synthetic buffer.

**Budget, for calibration.** A comparable Rust->WASM radar module (Mora &
Perdomo Charry, benchmarked in
`/home/alfonso-ladino/python/radar-animation/docs/competitive-landscape.md` §4)
ships at **54 kB** after `wasm-opt -O3` — though it does geometry, not decoding,
so it is a floor rather than a target. AtmoScale's stated reject threshold is
**> ~100 KB gzipped** for the decompressor alone. Record the real number either
way; the budget should be re-argued from a measurement, not from feel.

**Exit:** a browser decodes one real NIDS product with no server in the path.

---

## 9. What this deliberately does NOT do

- **No networking, no S3 listing, no caching.** The consumer's plan
  (`/home/alfonso-ladino/python/radar-animation/plans/serverless-realtime.md`,
  Phases 4–5) owns all of that. radish takes bytes.
- **No rendering, no geometry-for-GPU.** AtmoScale's renderer
  (`/home/alfonso-ladino/python/radar-animation/dashboard/lib/maplibre/RadarFrameLayer.ts`)
  projects analytically in the shader from ~6.3 KB of unit geometry and uploads
  1 byte per gate. Building vertex buffers in wasm would be a large regression —
  the best mesh representation benchmarked in the paper cited above costs 28 B
  per gate. **Do not add a `to_vertices` API.**
- **No replacement of the Python decoder in radar-animation.** It stays as the
  oracle and as the paid tier's server path, at least until Phase C has been
  green for a while.
- **No opinion on how many frames the free tier loops.** That is an open product
  decision in D0.

---

## 10. Reference map

### In radish (repo-relative)

| path | what |
|---|---|
| `docs/ARCHITECTURE.md` | backend trait, data model, the pattern a new backend follows |
| `radish/src/backends/nexrad/` | the Level 2 decoder — closest existing analogue |
| `radish/src/backends/common/` | shared byte-reading, coords, geometry, sniffing |
| `radish/Cargo.toml` | no `[features]` section yet — Phase A adds it |
| `Cargo.toml` (workspace) | dependency pins quoted in Phase A |
| `python/Cargo.toml` | the pyo3 binding this work mirrors |
| `plans/0001-nexrad-level2-backend.md` … | how previous backends were planned here |

### In radar-animation (absolute — separate checkout)

| path | what |
|---|---|
| `/home/alfonso-ladino/python/radar-animation/src/server/nexrad_level3.py` | **the oracle.** 638 lines. Docstrings explain the *why* for every item in section 4 |
| `/home/alfonso-ladino/python/radar-animation/src/server/level3_source.py` | `_check_scale` at :225 — the refuse-don't-rescale policy |
| `/home/alfonso-ladino/python/radar-animation/src/server/frames.py` | `QUANT` at :138 — the pinned quantization that makes codes verbatim |
| `/home/alfonso-ladino/python/radar-animation/tests/server/fixtures/level3/` | the seven fixtures + a README explaining each choice |
| `/home/alfonso-ladino/python/radar-animation/docs/level3.md` | what the products contain, and how the decoder is organized |
| `/home/alfonso-ladino/python/radar-animation/docs/frames-protocol.md` | the uint8 wire format the codes end up in |
| `/home/alfonso-ladino/python/radar-animation/plans/serverless-realtime.md` | the consuming plan. **Phases 2–3 are superseded by this document** |
| `/home/alfonso-ladino/python/radar-animation/plans/realtime-first-wiring.md` | section D0 — the tier decision and its evidence |
| `/home/alfonso-ladino/python/radar-animation/docs/competitive-landscape.md` | why serverless, the memory wall, the WASM size benchmark |
| `/home/alfonso-ladino/python/radar-animation/docs/product-architecture.md` | section 5.1 — the tier split of record |
| `/home/alfonso-ladino/python/radar-animation/dashboard/lib/maplibre/RadarFrameLayer.ts` | the renderer that consumes the codes — read it before proposing a geometry API |

### External

- [openradar/xradar PR #392][xr392] — the Python NIDS reader to diff against.
- NEXRAD Level 3 ICD (NOAA 2620001) — the format specification. Note that
  section 4.4 lists three places where the ICD's field names mislead.

---

## 11. Open questions — decide these before Phase B, not during

1. **Scope: AtmoScale's six products, or xradar #392's fuller set?** The narrow
   set is what has fixtures and an oracle. The fuller set is what makes radish a
   general Level 3 reader. Extra codes without fixtures are untested surface —
   whatever is chosen, the fixture count should track the product count.
2. **Packets AF1F and 28?** Not needed by the consumer. They are most of the gap
   between 638 and 1,182 lines.
3. **Does the wasm crate live in radish, or in AtmoScale?** In radish, it is one
   more binding of one library. In AtmoScale, radish stays a pure library and the
   browser glue lives with its consumer. Recommendation: **radish**, because the
   zero-copy boundary in Phase D is decode-side knowledge, not app-side.
4. **Does `radish-python` replace radar-animation's Python decoder eventually?**
   That would make it genuinely one implementation everywhere. It also puts a
   compiled dependency on the server's critical path. Not urgent; revisit after
   Phase C has been green for a while.

---

[atmoscale]: https://github.com/aladinor/radar-dashboard
[xr392]: https://github.com/openradar/xradar/pull/392

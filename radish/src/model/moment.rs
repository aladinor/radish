//! Moment (radar variable) data structures.

use ndarray::Array2;

/// A product's declared value scaling, verbatim from its source encoding —
/// e.g. a NEXRAD Level 3 (NIDS) product description block.
///
/// Exists because some consumers need the **codes**, not the physical
/// values: NEXRAD Level 3 defines its wire format directly in terms of a
/// product's own declared scale (see `docs/NEXRAD_LEVEL3_WASM.md` §4.1) —
/// re-deriving a scale from [`MomentData::data`] and re-quantizing would
/// both waste work and risk a rounding disagreement with the source file.
/// [`MomentData::raw_codes`] carries the verbatim `u8` codes; this struct is
/// what a caller needs to turn a code back into the same physical value the
/// source file declares, without assuming any particular quantization.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeclaredScale {
    /// Physical value at [`data_floor_code`](Self::data_floor_code).
    pub value_min: f32,
    /// Physical value change per code above the floor.
    pub value_increment: f32,
    /// Number of codes that carry data (from the floor code through 255,
    /// inclusive) — NEXRAD Level 3's digital radial products declare or
    /// imply 254.
    pub n_levels: u16,
    /// The lowest code that carries data. Codes below this are sentinels
    /// (e.g. NEXRAD Level 3: 0 = below threshold, 1 = range folded) —
    /// never `value_min + (code - data_floor_code) * value_increment`.
    pub data_floor_code: u8,
}

impl DeclaredScale {
    /// Physical value for one code, or `None` below [`data_floor_code`](Self::data_floor_code)
    /// (a sentinel, not data).
    pub fn decode(&self, code: u8) -> Option<f32> {
        if code < self.data_floor_code {
            return None;
        }
        Some(self.value_min + (code - self.data_floor_code) as f32 * self.value_increment)
    }
}

/// Radar moment data (e.g., reflectivity, velocity)
#[derive(Debug, Clone)]
pub struct MomentData {
    /// Variable name (e.g., "DBZH", "VRADH")
    pub name: String,

    /// CF standard name
    pub standard_name: Option<String>,

    /// Long descriptive name
    pub long_name: Option<String>,

    /// Units
    pub units: String,

    /// 2D data array [rays × gates]
    pub data: Array2<f32>,

    /// Fill value (missing data indicator)
    pub fill_value: Option<f32>,

    /// Scale factor
    pub scale_factor: Option<f32>,

    /// Add offset
    pub add_offset: Option<f32>,

    /// Valid minimum
    pub valid_min: Option<f32>,

    /// Valid maximum
    pub valid_max: Option<f32>,

    /// Coordinates this variable depends on
    pub coordinates: Option<String>,

    /// Additional attributes
    pub attributes: std::collections::HashMap<String, String>,

    /// Verbatim on-wire codes, when the source format defines its wire
    /// format in terms of codes rather than physical values (currently:
    /// NEXRAD Level 3 / NIDS digital radial products). `None` for every
    /// other backend — this is deliberately *additional* to
    /// [`data`](Self::data), not a replacement: `data` stays the physical
    /// values every other radish consumer expects, and `raw_codes` +
    /// [`declared_scale`](Self::declared_scale) are there for a consumer
    /// that needs the source's own quantization untouched (see
    /// `docs/NEXRAD_LEVEL3_WASM.md` §4.1 — "the codes pass through
    /// untouched, that is the whole design"). Same shape as `data`
    /// (`[rays × gates]`) when present.
    pub raw_codes: Option<Array2<u8>>,

    /// The scale [`raw_codes`](Self::raw_codes) is declared on, verbatim
    /// from the source file. `None` whenever `raw_codes` is `None`, and
    /// also `None` for a categorical product (e.g. hydrometeor
    /// classification) whose codes index a fixed label table rather than a
    /// linear physical scale.
    pub declared_scale: Option<DeclaredScale>,
}

impl MomentData {
    /// Create a new MomentData
    pub fn new(name: String, units: String, data: Array2<f32>) -> Self {
        Self {
            name,
            standard_name: None,
            long_name: None,
            units,
            data,
            fill_value: None,
            scale_factor: None,
            add_offset: None,
            valid_min: None,
            valid_max: None,
            coordinates: None,
            attributes: std::collections::HashMap::new(),
            raw_codes: None,
            declared_scale: None,
        }
    }

    /// Get the shape of the data array
    pub fn shape(&self) -> (usize, usize) {
        let shape = self.data.shape();
        (shape[0], shape[1])
    }

    /// Apply scale and offset to get physical values
    pub fn apply_scale_offset(&mut self) {
        if let (Some(scale), Some(offset)) = (self.scale_factor, self.add_offset) {
            self.data.mapv_inplace(|v| {
                if let Some(fill) = self.fill_value {
                    if v == fill {
                        return v;
                    }
                }
                v * scale + offset
            });
            self.scale_factor = None;
            self.add_offset = None;
        }
    }

    /// Mask invalid values
    pub fn mask_invalid(&mut self, mask_value: f32) {
        if let Some(fill) = self.fill_value {
            self.data
                .mapv_inplace(|v| if v == fill { mask_value } else { v });
        }

        if let (Some(min), Some(max)) = (self.valid_min, self.valid_max) {
            self.data
                .mapv_inplace(|v| if v < min || v > max { mask_value } else { v });
        }
    }
}

// `MomentMetadata` and its `from_name` table used to live here. They were
// never wired into any backend and are now superseded by the per-backend
// metadata sources of truth (e.g. `radish::backends::nexrad::mapping`, which
// uses the `radish_types::moments` constants directly). Re-introduce only
// when a generalised name→metadata lookup actually has callers.

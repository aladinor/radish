//! Moment (radar variable) data structures.

use ndarray::Array2;

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

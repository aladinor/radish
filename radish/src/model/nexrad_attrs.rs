//! NEXRAD-specific volume and sweep attributes derived from MSG_2 and MSG_5.
//!
//! These mirror what xradar's `open_nexradlevel2_datatree` puts on the root
//! Dataset and on each sweep Dataset, so we can produce a `DataTree` whose
//! `.attrs` match for a drop-in user experience.

use serde::{Deserialize, Serialize};

/// Volume-level NEXRAD attrs (MSG_2 + MSG_5 + computed).
///
/// All 16 fields here correspond directly to keys xradar emits on the root
/// `Dataset.attrs`. Defaults (zero / false / empty string) are deliberate:
/// xradar's reader uses `.get(name, default)` on the parsed dicts, so a missing
/// MSG_2 in the volume yields the same zero/False values it would there.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NexradVolumeAttrs {
    pub dynamic_scan_type: String,
    pub mpda_vcp: bool,
    pub base_tilt_vcp: bool,
    pub num_base_tilts: u8,
    pub vcp_truncated: bool,
    pub vcp_sequence_active: bool,
    pub number_elevation_cuts: u32,
    pub doppler_velocity_resolution: f32,
    pub vcp_pulse_width: String,
    pub avset_enabled: bool,
    pub ebc_enabled: bool,
    pub super_res_status: u16,
    pub rda_build_number: u16,
    pub operational_mode: u16,
    pub actual_elevation_cuts: u32,
}

/// Per-sweep NEXRAD attrs (all from MSG_5 elevation cuts).
///
/// Index-aligned with the volume's sweep list — xradar relies on the same
/// alignment in `_assign_sweep_attrs`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NexradSweepAttrs {
    pub waveform_type: String,
    pub channel_config: String,
    pub super_resolution: u8,
    pub sails_cut: bool,
    pub sails_sequence_number: u8,
    pub mrle_cut: bool,
    pub mrle_sequence_number: u8,
    pub mpda_cut: bool,
    pub base_tilt_cut: bool,
}

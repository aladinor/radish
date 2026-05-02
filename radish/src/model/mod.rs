//! Data model for radar volumes, sweeps, and moments.
//!
//! This module defines the core data structures that represent weather radar
//! data in a format-agnostic way, following the CfRadial2/FM301 standard.

mod coordinates;
mod moment;
mod nexrad_attrs;
mod sweep;
mod volume;

pub use coordinates::Coordinates;
pub use moment::MomentData;
pub use nexrad_attrs::{NexradSweepAttrs, NexradVolumeAttrs};
pub use sweep::{SweepData, SweepMetadata};
pub use volume::{VolumeData, VolumeMetadata};

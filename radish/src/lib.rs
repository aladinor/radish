//! Radish: High-performance weather radar data library.
//!
//! This library provides fast, memory-efficient reading of multiple weather
//! radar formats with a unified interface, normalizing to the CfRadial2/FM301
//! standard.

pub mod backends;
pub mod error;
pub mod model;
pub mod transforms;

// Re-export commonly used types
pub use backends::RadarBackend;
pub use error::{RadishError, Result};
pub use model::{
    Coordinates, MomentData, NexradSweepAttrs, NexradVolumeAttrs, SweepData, SweepMetadata,
    VolumeData, VolumeMetadata,
};

#[cfg(test)]
mod tests {
    #[test]
    fn test_basic() {
        // Basic smoke test
        assert_eq!(2 + 2, 4);
    }
}

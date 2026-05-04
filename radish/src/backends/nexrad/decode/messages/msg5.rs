//! MSG_5 (Volume Coverage Pattern) parser — ICD §3.2.4.12 Table XI.
//!
//! Layout:
//!
//! * 11 halfwords header (HW 1-11): message size, pattern type,
//!   pattern number, elevation cut count, version, clutter map
//!   group, doppler velocity resolution + pulse width (packed in
//!   HW 6), 2 reserved halfwords, VCP sequencing, VCP supplemental
//!   data, 1 reserved halfword.
//! * Then `number_of_elevation_cuts × 23 halfwords` per ICD
//!   footnote 18: `EX = (12 + (X-1)) + ((Cut-1) * 23)`.
//!
//! Each elevation cut (46 bytes) carries the commanded angle, scan
//! parameters, SAILS/MRLE/MPDA/BASE-TILT classification flags, and
//! per-sector metadata.

use crate::backends::nexrad::decode::error::{NexradDecodeError, Result};
use crate::backends::nexrad::decode::reader::SliceReader;

/// Halfwords per elevation cut block (E1-E23 per ICD Note 18).
pub(crate) const ELEVATION_CUT_HALFWORDS: usize = 23;

/// One elevation cut from an MSG_5 VCP definition (ICD Table XI
/// E1-E23). Field types follow ICD; we preserve raw values for
/// fields that are bit-packed (channel_configuration,
/// waveform_type, super_resolution_control, supplemental_data) so
/// downstream consumers can apply their own interpretations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ElevationCut {
    /// E1 — commanded elevation angle as a Code*2 binary angle
    /// (Table III-A). Stored raw; convert via
    /// `binary_angle_degrees(self.elevation_angle_raw)`.
    pub(crate) elevation_angle_raw: u16,
    /// E2 upper byte — channel configuration code.
    pub(crate) channel_configuration: u8,
    /// E2 lower byte — waveform type code.
    pub(crate) waveform_type: u8,
    /// E3 upper byte — super-resolution control bits.
    pub(crate) super_resolution_control: u8,
    /// E3 lower byte — surveillance PRF number (1-8, 0 = N/A).
    pub(crate) surveillance_prf_number: u8,
    /// E4 — surveillance pulse count per radial.
    pub(crate) surveillance_pulse_count: u16,
    /// E5 — azimuth rate (Table XI-D BA velocity).
    pub(crate) azimuth_rate_raw: u16,
    /// E6-E11 — SNR thresholds (scaled signed integer, /8 → dB).
    pub(crate) reflectivity_threshold_raw: i16,
    pub(crate) velocity_threshold_raw: i16,
    pub(crate) spectrum_width_threshold_raw: i16,
    pub(crate) differential_reflectivity_threshold_raw: i16,
    pub(crate) differential_phase_threshold_raw: i16,
    pub(crate) correlation_coefficient_threshold_raw: i16,
    /// E12-E14 — Sector 1 (edge angle, doppler PRF #, pulse count).
    pub(crate) sector1_edge_angle_raw: u16,
    pub(crate) sector1_doppler_prf_number: u16,
    pub(crate) sector1_doppler_pulse_count: u16,
    /// E15 — supplemental data (SAILS / MRLE / MPDA / BASE-TILT bits).
    pub(crate) supplemental_data: u16,
    /// E16-E18 — Sector 2.
    pub(crate) sector2_edge_angle_raw: u16,
    pub(crate) sector2_doppler_prf_number: u16,
    pub(crate) sector2_doppler_pulse_count: u16,
    /// E19 — EBC angle correction (binary angle).
    pub(crate) ebc_angle_raw: u16,
    /// E20-E22 — Sector 3.
    pub(crate) sector3_edge_angle_raw: u16,
    pub(crate) sector3_doppler_prf_number: u16,
    pub(crate) sector3_doppler_pulse_count: u16,
    /// E23 — reserved halfword.
    pub(crate) reserved: u16,
}

/// Decoded MSG_5 VCP definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Msg5 {
    pub(crate) message_size_halfwords: u16,
    pub(crate) pattern_type: u16,
    pub(crate) pattern_number: u16,
    pub(crate) number_of_elevation_cuts: u16,
    pub(crate) vcp_version: u8,
    pub(crate) clutter_map_group_number: u8,
    /// HW 6 upper byte — doppler velocity resolution code
    /// (2 = 0.5 m/s, 4 = 1.0 m/s).
    pub(crate) doppler_velocity_resolution: u8,
    /// HW 6 lower byte — pulse width code (2 = short, 4 = long).
    pub(crate) pulse_width: u8,
    /// HW 9 — VCP sequencing flags (per ICD Note 15).
    pub(crate) vcp_sequencing: u16,
    /// HW 10 — VCP supplemental data (SAILS/MRLE/MPDA/BASE-TILT
    /// flags, per ICD Note 16).
    pub(crate) vcp_supplemental: u16,
    pub(crate) elevation_cuts: Vec<ElevationCut>,
}

impl Msg5 {
    pub(crate) fn read(reader: &mut SliceReader<'_>) -> Result<Self> {
        let message_size_halfwords = reader.read_u16_be()?;
        let pattern_type = reader.read_u16_be()?;
        let pattern_number = reader.read_u16_be()?;
        let number_of_elevation_cuts = reader.read_u16_be()?;
        let vcp_version = reader.read_u8()?;
        let clutter_map_group_number = reader.read_u8()?;
        let doppler_velocity_resolution = reader.read_u8()?;
        let pulse_width = reader.read_u8()?;
        let _reserved_hw7 = reader.read_u16_be()?;
        let _reserved_hw8 = reader.read_u16_be()?;
        let vcp_sequencing = reader.read_u16_be()?;
        let vcp_supplemental = reader.read_u16_be()?;
        let _reserved_hw11 = reader.read_u16_be()?;

        // Sanity-check elevation count to keep `Vec::with_capacity`
        // honest. ICD Table XI: 1..32.
        if number_of_elevation_cuts == 0 || number_of_elevation_cuts > 32 {
            return Err(NexradDecodeError::MalformedHeader {
                offset: 0,
                reason: "MSG_5 number_of_elevation_cuts outside ICD 1..32",
            });
        }

        let mut elevation_cuts = Vec::with_capacity(number_of_elevation_cuts as usize);
        for _ in 0..number_of_elevation_cuts {
            elevation_cuts.push(ElevationCut::read(reader)?);
        }

        Ok(Self {
            message_size_halfwords,
            pattern_type,
            pattern_number,
            number_of_elevation_cuts,
            vcp_version,
            clutter_map_group_number,
            doppler_velocity_resolution,
            pulse_width,
            vcp_sequencing,
            vcp_supplemental,
            elevation_cuts,
        })
    }
}

impl ElevationCut {
    fn read(reader: &mut SliceReader<'_>) -> Result<Self> {
        let elevation_angle_raw = reader.read_u16_be()?;
        let channel_configuration = reader.read_u8()?;
        let waveform_type = reader.read_u8()?;
        let super_resolution_control = reader.read_u8()?;
        let surveillance_prf_number = reader.read_u8()?;
        let surveillance_pulse_count = reader.read_u16_be()?;
        let azimuth_rate_raw = reader.read_u16_be()?;
        let reflectivity_threshold_raw = reader.read_i16_be()?;
        let velocity_threshold_raw = reader.read_i16_be()?;
        let spectrum_width_threshold_raw = reader.read_i16_be()?;
        let differential_reflectivity_threshold_raw = reader.read_i16_be()?;
        let differential_phase_threshold_raw = reader.read_i16_be()?;
        let correlation_coefficient_threshold_raw = reader.read_i16_be()?;
        let sector1_edge_angle_raw = reader.read_u16_be()?;
        let sector1_doppler_prf_number = reader.read_u16_be()?;
        let sector1_doppler_pulse_count = reader.read_u16_be()?;
        let supplemental_data = reader.read_u16_be()?;
        let sector2_edge_angle_raw = reader.read_u16_be()?;
        let sector2_doppler_prf_number = reader.read_u16_be()?;
        let sector2_doppler_pulse_count = reader.read_u16_be()?;
        let ebc_angle_raw = reader.read_u16_be()?;
        let sector3_edge_angle_raw = reader.read_u16_be()?;
        let sector3_doppler_prf_number = reader.read_u16_be()?;
        let sector3_doppler_pulse_count = reader.read_u16_be()?;
        let reserved = reader.read_u16_be()?;
        Ok(Self {
            elevation_angle_raw,
            channel_configuration,
            waveform_type,
            super_resolution_control,
            surveillance_prf_number,
            surveillance_pulse_count,
            azimuth_rate_raw,
            reflectivity_threshold_raw,
            velocity_threshold_raw,
            spectrum_width_threshold_raw,
            differential_reflectivity_threshold_raw,
            differential_phase_threshold_raw,
            correlation_coefficient_threshold_raw,
            sector1_edge_angle_raw,
            sector1_doppler_prf_number,
            sector1_doppler_pulse_count,
            supplemental_data,
            sector2_edge_angle_raw,
            sector2_doppler_prf_number,
            sector2_doppler_pulse_count,
            ebc_angle_raw,
            sector3_edge_angle_raw,
            sector3_doppler_prf_number,
            sector3_doppler_pulse_count,
            reserved,
        })
    }

    /// Convert the `elevation_angle_raw` (Code*2 binary angle per
    /// Table III-A) to degrees. Bits 3-15 carry the angle, weighted
    /// `180 * 2^(bit_index - 15)`. The upstream `nexrad-decode`
    /// uses this same formula.
    pub(crate) fn elevation_angle_degrees(&self) -> f32 {
        binary_angle_degrees(self.elevation_angle_raw)
    }
}

/// Convert an ICD Table III-A binary angle (Code*2) to degrees.
/// Bits 0-2 are unused; bits 3-15 carry the angle, with weight
/// `180 * 2^(bit_index - 15)`.
pub(crate) fn binary_angle_degrees(raw: u16) -> f32 {
    let mut angle = 0.0_f32;
    for i in 3i32..16 {
        if raw & (1u16 << i) != 0 {
            angle += 180.0_f32 * (2.0_f32).powi(i - 15);
        }
    }
    angle
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an N-elevation-cut MSG_5 with `n_cuts` zeroed cuts +
    /// distinct pattern_number / vcp_version values to verify the
    /// header round-trip.
    fn synth_msg5(n_cuts: u16) -> Vec<u8> {
        let mut buf = Vec::new();
        // HW 1..11 — header.
        let total_halfwords = 11 + n_cuts * ELEVATION_CUT_HALFWORDS as u16;
        buf.extend_from_slice(&total_halfwords.to_be_bytes()); // HW 1
        buf.extend_from_slice(&2u16.to_be_bytes()); // HW 2 pattern_type
        buf.extend_from_slice(&212u16.to_be_bytes()); // HW 3 pattern_number
        buf.extend_from_slice(&n_cuts.to_be_bytes()); // HW 4 n_cuts
        buf.push(1); // HW 5 upper: vcp_version
        buf.push(1); // HW 5 lower: clutter_map_group
        buf.push(2); // HW 6 upper: dop_vel_res = 0.5 m/s
        buf.push(2); // HW 6 lower: pulse_width = short
        buf.extend_from_slice(&0u16.to_be_bytes()); // HW 7 reserved
        buf.extend_from_slice(&0u16.to_be_bytes()); // HW 8 reserved
        buf.extend_from_slice(&(n_cuts | (3u16 << 5)).to_be_bytes()); // HW 9 vcp_sequencing
        buf.extend_from_slice(&(1u16 | (1u16 << 1)).to_be_bytes()); // HW 10 supplemental: SAILS + 1 SAILS cut
        buf.extend_from_slice(&0u16.to_be_bytes()); // HW 11 reserved
                                                    // Elevation cuts — distinct per-cut elevation_angle_raw so
                                                    // we can verify ordering.
        for i in 0..n_cuts {
            // E1 — angle; use a non-trivial bit pattern per cut.
            buf.extend_from_slice(&((i + 1) << 8).to_be_bytes()); // E1
            buf.push(0); // E2 channel_configuration
            buf.push(1); // E2 waveform_type (= CS)
            buf.push(0); // E3 super_resolution_control
            buf.push(1); // E3 surveillance_prf_number
            buf.extend_from_slice(&17u16.to_be_bytes()); // E4 pulse_count
            buf.extend_from_slice(&0u16.to_be_bytes()); // E5 azimuth_rate
                                                        // E6..E11 — six SNR thresholds, all 0
            for _ in 0..6 {
                buf.extend_from_slice(&0_i16.to_be_bytes());
            }
            // E12..E14 — sector 1
            buf.extend_from_slice(&0u16.to_be_bytes()); // edge_angle
            buf.extend_from_slice(&0u16.to_be_bytes()); // prf_number
            buf.extend_from_slice(&0u16.to_be_bytes()); // pulse_count
                                                        // E15 supplemental_data — set sails_cut bit on first
                                                        // cut to verify per-cut decoding.
            let supp = if i == 0 { 1 } else { 0 };
            buf.extend_from_slice(&(supp as u16).to_be_bytes());
            // E16..E18 — sector 2
            buf.extend_from_slice(&0u16.to_be_bytes());
            buf.extend_from_slice(&0u16.to_be_bytes());
            buf.extend_from_slice(&0u16.to_be_bytes());
            // E19 — EBC angle
            buf.extend_from_slice(&0u16.to_be_bytes());
            // E20..E22 — sector 3
            buf.extend_from_slice(&0u16.to_be_bytes());
            buf.extend_from_slice(&0u16.to_be_bytes());
            buf.extend_from_slice(&0u16.to_be_bytes());
            // E23 — reserved
            buf.extend_from_slice(&0u16.to_be_bytes());
        }
        debug_assert_eq!(
            buf.len(),
            (11 + (n_cuts as usize) * ELEVATION_CUT_HALFWORDS) * 2
        );
        buf
    }

    #[test]
    fn read_consumes_full_header_and_cuts() {
        let bytes = synth_msg5(13);
        let mut r = SliceReader::new(&bytes);
        let msg = Msg5::read(&mut r).unwrap();
        assert_eq!(msg.pattern_number, 212);
        assert_eq!(msg.number_of_elevation_cuts, 13);
        assert_eq!(msg.vcp_version, 1);
        assert_eq!(msg.doppler_velocity_resolution, 2);
        assert_eq!(msg.pulse_width, 2);
        assert_eq!(msg.elevation_cuts.len(), 13);
        // First cut has supplemental_data = 1 (SAILS) per the synth.
        assert_eq!(msg.elevation_cuts[0].supplemental_data, 1);
        // Subsequent cuts have supplemental_data = 0.
        assert!(msg.elevation_cuts[1..]
            .iter()
            .all(|c| c.supplemental_data == 0));
    }

    #[test]
    fn read_rejects_zero_or_oversized_elevation_count() {
        let mut bytes = synth_msg5(3);
        // HW 4 (number_of_elevation_cuts) → bytes 6..8.
        bytes[6..8].copy_from_slice(&0u16.to_be_bytes());
        let mut r = SliceReader::new(&bytes);
        assert!(Msg5::read(&mut r).is_err());

        let mut bytes = synth_msg5(3);
        bytes[6..8].copy_from_slice(&33u16.to_be_bytes());
        let mut r = SliceReader::new(&bytes);
        assert!(Msg5::read(&mut r).is_err());
    }

    #[test]
    fn read_errors_on_truncated_cut() {
        // Header says 5 cuts but only 2 cuts of bytes follow.
        let mut bytes = synth_msg5(5);
        bytes.truncate(11 * 2 + 2 * ELEVATION_CUT_HALFWORDS * 2);
        let mut r = SliceReader::new(&bytes);
        assert!(Msg5::read(&mut r).is_err());
    }

    #[test]
    fn binary_angle_decodes_known_values() {
        // Bit 15 = 180 * 2^0 = 180°.
        assert!((binary_angle_degrees(0x8000) - 180.0).abs() < 1e-3);
        // Bit 14 = 180 * 2^-1 = 90°.
        assert!((binary_angle_degrees(0x4000) - 90.0).abs() < 1e-3);
        // 0x0100 = bit 8 = 180 * 2^-7 ≈ 1.40625°. Used by the
        // synth_msg5 helper so the cuts have distinguishable angles.
        let v = binary_angle_degrees(0x0100);
        assert!((v - 1.40625).abs() < 1e-3, "got {v}");
    }
}

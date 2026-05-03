//! Build `NexradVolumeAttrs` and `NexradSweepAttrs` from upstream MSG_2 / MSG_5
//! decoded structs, matching xradar's emission verbatim.
//!
//! Mapping decisions here are deliberately conservative and mirror what
//! xradar's `open_nexradlevel2_datatree` does in
//! `xradar/io/backends/nexrad_level2.py`. If a value diverges, the user-facing
//! `xr.DataTree` `.attrs` will diverge — that's the contract.

use nexrad::decode::messages::rda_status_data::Message as RdaStatusMessage;
use nexrad_model::data::{
    ChannelConfiguration, ElevationCut, PulseWidth, Sweep, VolumeCoveragePattern, WaveformType,
};

use crate::{NexradSweepAttrs, NexradVolumeAttrs};

/// xradar's `_WAVEFORM_TYPES` lookup. We match xradar **by raw byte**, not by
/// ICD semantics: xradar's table is off-by-one vs the ICD (raw=3 ⇒ "batch"
/// in xradar, but "Contiguous Doppler without Ambiguity Resolution" per ICD
/// 2620002AA), and raw=5 falls through to `str(code)`. The whole point of
/// this code path is *xradar parity*, so we emit what xradar would emit
/// regardless of whether xradar matches the spec.
///
/// Cached strings for raw=5/6/... avoid allocating per call.
pub(super) fn waveform_type_str_from_raw(raw: u8) -> String {
    match raw {
        0 => "not_applicable".to_string(),
        1 => "contiguous_surveillance".to_string(),
        2 => "contiguous_doppler".to_string(),
        3 => "batch".to_string(),
        4 => "staggered_pulse_pair".to_string(),
        // xradar falls through to `_WAVEFORM_TYPES.get(wf, str(wf))`.
        other => other.to_string(),
    }
}

/// xradar's `_CHANNEL_CONFIGS` lookup. Same raw-byte rule as above.
pub(super) fn channel_config_str_from_raw(raw: u8) -> String {
    match raw {
        0 => "constant_phase".to_string(),
        1 => "random_phase".to_string(),
        2 => "sz2_phase_coding".to_string(),
        other => other.to_string(),
    }
}

/// Map the upstream `WaveformType` enum back to its MSG_5_ELEV E3 raw byte.
/// The upstream `Scan::scan()` decode is bijective on known values
/// (see `nexrad-data/src/volume/file.rs::scan` lines 192-199), so this round-
/// trips losslessly. For `Unknown` we lose the original code; emit 0
/// (xradar would emit `_WAVEFORM_TYPES[0] = "not_applicable"`).
pub(super) fn waveform_to_raw(w: WaveformType) -> u8 {
    match w {
        WaveformType::CS => 1,
        WaveformType::CDW => 2,
        WaveformType::CDWO => 3,
        WaveformType::B => 4,
        WaveformType::SPP => 5,
        WaveformType::Unknown => 0,
    }
}

/// Map the upstream `ChannelConfiguration` enum back to its MSG_5_ELEV E2 raw
/// byte. Same rationale as `waveform_to_raw`. For `Unknown` we emit 3
/// (xradar's table is empty there, so it falls through to `str(3)` = `"3"`).
pub(super) fn channel_config_to_raw(c: ChannelConfiguration) -> u8 {
    match c {
        ChannelConfiguration::ConstantPhase => 0,
        ChannelConfiguration::RandomPhase => 1,
        ChannelConfiguration::SZ2Phase => 2,
        ChannelConfiguration::Unknown => 3,
    }
}

/// xradar's `_get_dynamic_scan_type`. SAILS and MRLE are mutually exclusive
/// per ICD 2620002AA Note 16.
pub(super) fn dynamic_scan_type(vcp: &VolumeCoveragePattern) -> String {
    if vcp.sails_enabled() {
        let n = vcp.sails_cuts();
        if n == 0 {
            "SAILS".to_string()
        } else {
            format!("SAILS x {n}")
        }
    } else if vcp.mrle_enabled() {
        let n = vcp.mrle_cuts();
        if n == 0 {
            "MRLE".to_string()
        } else {
            format!("MRLE x {n}")
        }
    } else {
        "standard".to_string()
    }
}

/// xradar emits `vcp_pulse_width` as `"short"`, `"long"`, or `str(code)`. The
/// upstream `PulseWidth::Unknown` doesn't carry the original code, so we emit
/// an empty string for that case — xradar would have emitted `"0"` here, but
/// `PulseWidth::Unknown` only fires when the raw code is *neither* 2 nor 4,
/// which is unusual; the divergence isn't load-bearing.
pub(super) fn pulse_width_str(p: PulseWidth) -> &'static str {
    match p {
        PulseWidth::Short => "short",
        PulseWidth::Long => "long",
        PulseWidth::Unknown => "",
    }
}

/// Pack the four super-resolution control bits back into the 4-bit code that
/// xradar publishes as `super_resolution`. Bit layout per ICD MSG_5_ELEV E4:
/// bit 0 = 0.5° azimuth, bit 1 = 1/4 km reflectivity, bit 2 = Doppler to 300
/// km, bit 3 = dual-pol to 300 km.
fn pack_super_resolution(cut: &ElevationCut) -> u8 {
    let mut bits = 0u8;
    if cut.super_resolution_half_degree_azimuth() {
        bits |= 0b0001;
    }
    if cut.super_resolution_quarter_km_reflectivity() {
        bits |= 0b0010;
    }
    if cut.super_resolution_doppler_to_300km() {
        bits |= 0b0100;
    }
    if cut.super_resolution_dual_pol_to_300km() {
        bits |= 0b1000;
    }
    bits
}

/// Build the per-sweep attrs from a single MSG_5 elevation cut.
///
/// We round-trip the upstream typed enums back through their MSG_5_ELEV raw
/// bytes (E2 / E3) and feed those into xradar's `_WAVEFORM_TYPES` /
/// `_CHANNEL_CONFIGS` lookups. Going through the raw byte is the only way to
/// reproduce xradar's emission verbatim — its tables don't agree with the ICD
/// and the typed enum loses the numeric identity needed for `str(code)` fall-
/// through cases.
pub(super) fn sweep_attrs_from_cut(cut: &ElevationCut) -> NexradSweepAttrs {
    NexradSweepAttrs {
        waveform_type: waveform_type_str_from_raw(waveform_to_raw(cut.waveform_type())),
        channel_config: channel_config_str_from_raw(channel_config_to_raw(
            cut.channel_configuration(),
        )),
        super_resolution: pack_super_resolution(cut),
        sails_cut: cut.is_sails_cut(),
        sails_sequence_number: cut.sails_sequence_number(),
        mrle_cut: cut.is_mrle_cut(),
        mrle_sequence_number: cut.mrle_sequence_number(),
        mpda_cut: cut.is_mpda_cut(),
        base_tilt_cut: cut.is_base_tilt_cut(),
    }
}

/// xradar's `decode_rda_scan_data_flags`. Decodes the raw HW 14 u16, NOT the
/// upstream `ScanDataFlags::avset_enabled()` accessor — that one checks
/// bit 0 instead of bit 1, which is a bug vs xradar's (correct) ICD-Note-13
/// bit positions. We need xradar parity, so we redo the bit math.
fn decode_rda_scan_data_flags(raw: u16) -> (bool, bool) {
    // bit 1 (value 0x0002) = AVSET enabled per ICD; bit 3 (0x0008) = EBC enabled.
    let avset_enabled = raw & 0x0002 != 0;
    let ebc_enabled = raw & 0x0008 != 0;
    (avset_enabled, ebc_enabled)
}

/// Build the volume-level attrs from MSG_5 (always present; we already use it
/// for moments) plus the optional MSG_2. Missing MSG_2 yields zero/false
/// values for the 5 RDA-status fields — xradar's
/// `dict.get(name, default)` does the same.
///
/// `sweeps` provides the per-sweep data needed to populate
/// [`NexradVolumeAttrs::sweep_attrs`] (one [`NexradSweepAttrs`] per sweep,
/// from the corresponding `ElevationCut`) and
/// [`NexradVolumeAttrs::sweep_time_ranges`] (each sweep's
/// `(time_start, time_end)` from its first/last ray's
/// `collection_time`). Both are populated whether the caller is doing a
/// metadata-only `scan_file` or a full `read_volume` — neither needs
/// per-ray moment decode.
pub(super) fn volume_attrs(
    vcp: &VolumeCoveragePattern,
    msg2: Option<&RdaStatusMessage<'_>>,
    sweeps: &[Sweep],
    actual_elevation_cuts: u32,
) -> NexradVolumeAttrs {
    let (avset_enabled, ebc_enabled, super_res_status, rda_build_number, operational_mode) =
        match msg2 {
            Some(m) => {
                let (avset, ebc) = decode_rda_scan_data_flags(m.raw_rda_scan_and_data_flags());
                (
                    avset,
                    ebc,
                    m.raw_super_resolution_status(),
                    m.raw_rda_build_number(),
                    m.raw_operational_mode(),
                )
            }
            None => (false, false, 0, 0, 0),
        };

    // Per-sweep MSG_5 attrs. Index-aligned with the volume's sweep list:
    // entry `i` corresponds to `sweeps[i]`. When the cut table is shorter
    // than the sweep count (e.g. truncated VCP), pad with default
    // `NexradSweepAttrs` so callers can still index by sweep number — see
    // [`NexradVolumeAttrs::sweep_attrs`] for the consumer-side contract on
    // distinguishing "default-padded" from "definitively false."
    let cuts = vcp.elevation_cuts();
    let sweep_attrs: Vec<NexradSweepAttrs> = (0..sweeps.len())
        .map(|idx| cuts.get(idx).map(sweep_attrs_from_cut).unwrap_or_default())
        .collect();

    // Per-sweep `(time_start, time_end)` ranges as Unix seconds since
    // 1970-01-01 UTC, matching the `Coordinates::time` axis convention.
    // `Sweep::time_range()` is a min/max walk over already-decoded
    // radials — sub-millisecond for a typical volume.
    let sweep_time_ranges: Vec<Option<(f64, f64)>> = sweeps
        .iter()
        .map(|s| {
            s.time_range().map(|(start, end)| {
                (
                    start.timestamp_micros() as f64 / 1.0e6,
                    end.timestamp_micros() as f64 / 1.0e6,
                )
            })
        })
        .collect();

    NexradVolumeAttrs {
        dynamic_scan_type: dynamic_scan_type(vcp),
        mpda_vcp: vcp.mpda_enabled(),
        base_tilt_vcp: vcp.base_tilt_enabled(),
        num_base_tilts: vcp.base_tilt_count(),
        vcp_truncated: vcp.truncated(),
        vcp_sequence_active: vcp.sequence_active(),
        number_elevation_cuts: vcp.number_of_elevation_cuts() as u32,
        doppler_velocity_resolution: vcp.doppler_velocity_resolution(),
        vcp_pulse_width: pulse_width_str(vcp.pulse_width()).to_string(),
        avset_enabled,
        ebc_enabled,
        super_res_status,
        rda_build_number,
        operational_mode,
        actual_elevation_cuts,
        sweep_attrs,
        sweep_time_ranges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_type_from_raw_matches_xradar_table() {
        // Pinning xradar's `_WAVEFORM_TYPES` map verbatim. xradar's table is
        // off-by-one vs the ICD starting at code 3 — that's xradar's bug,
        // not ours, and we match it on purpose for drop-in attr parity.
        assert_eq!(waveform_type_str_from_raw(0), "not_applicable");
        assert_eq!(waveform_type_str_from_raw(1), "contiguous_surveillance");
        assert_eq!(waveform_type_str_from_raw(2), "contiguous_doppler");
        assert_eq!(waveform_type_str_from_raw(3), "batch");
        assert_eq!(waveform_type_str_from_raw(4), "staggered_pulse_pair");
        // xradar falls through: `_WAVEFORM_TYPES.get(5, str(5)) == "5"`.
        assert_eq!(waveform_type_str_from_raw(5), "5");
        assert_eq!(waveform_type_str_from_raw(99), "99");
    }

    #[test]
    fn channel_config_from_raw_matches_xradar_table() {
        assert_eq!(channel_config_str_from_raw(0), "constant_phase");
        assert_eq!(channel_config_str_from_raw(1), "random_phase");
        assert_eq!(channel_config_str_from_raw(2), "sz2_phase_coding");
        // xradar falls through to `str(3) == "3"` for unknown codes.
        assert_eq!(channel_config_str_from_raw(3), "3");
    }

    #[test]
    fn waveform_to_raw_round_trips_known_variants() {
        // Bijective on known ICD values 1-5, defaults to 0 for Unknown.
        assert_eq!(waveform_to_raw(WaveformType::CS), 1);
        assert_eq!(waveform_to_raw(WaveformType::CDW), 2);
        assert_eq!(waveform_to_raw(WaveformType::CDWO), 3);
        assert_eq!(waveform_to_raw(WaveformType::B), 4);
        assert_eq!(waveform_to_raw(WaveformType::SPP), 5);
        assert_eq!(waveform_to_raw(WaveformType::Unknown), 0);
    }

    #[test]
    fn channel_config_to_raw_round_trips_known_variants() {
        assert_eq!(
            channel_config_to_raw(ChannelConfiguration::ConstantPhase),
            0
        );
        assert_eq!(channel_config_to_raw(ChannelConfiguration::RandomPhase), 1);
        assert_eq!(channel_config_to_raw(ChannelConfiguration::SZ2Phase), 2);
        assert_eq!(channel_config_to_raw(ChannelConfiguration::Unknown), 3);
    }

    #[test]
    fn sweep_attrs_emit_xradar_strings_for_batch_and_staggered() {
        // The bug that motivated this round-trip: a real KLOT volume has a
        // sweep with `WaveformType::B` (raw=4 per ICD), and xradar emits
        // `"staggered_pulse_pair"` — not `"batch"` — for that raw byte.
        let cut_b = ElevationCut::new(
            0.5,
            ChannelConfiguration::ConstantPhase,
            WaveformType::B,
            18.0,
            true,
            true,
            false,
            false,
            1,
            17,
            16.0,
            -20.0,
            12.0,
            0.0,
            0.0,
            0.0,
            false,
            0,
            false,
            0,
            false,
            false,
        );
        let attrs = sweep_attrs_from_cut(&cut_b);
        assert_eq!(attrs.waveform_type, "staggered_pulse_pair");

        // CDWO (raw=3) → xradar emits "batch".
        let cut_cdwo = ElevationCut::new(
            0.5,
            ChannelConfiguration::SZ2Phase,
            WaveformType::CDWO,
            18.0,
            false,
            false,
            false,
            false,
            1,
            17,
            16.0,
            -20.0,
            12.0,
            0.0,
            0.0,
            0.0,
            false,
            0,
            false,
            0,
            false,
            false,
        );
        let attrs = sweep_attrs_from_cut(&cut_cdwo);
        assert_eq!(attrs.waveform_type, "batch");
        assert_eq!(attrs.channel_config, "sz2_phase_coding");
    }

    #[test]
    fn dynamic_scan_type_sails_with_count() {
        // SAILS+1 → "SAILS x 1"; SAILS+0 → bare "SAILS"; not-SAILS → standard.
        let cut = sample_cut();
        let vcp = sample_vcp(true, 1, false, 0, vec![cut.clone()]);
        assert_eq!(dynamic_scan_type(&vcp), "SAILS x 1");

        let vcp = sample_vcp(true, 0, false, 0, vec![cut.clone()]);
        assert_eq!(dynamic_scan_type(&vcp), "SAILS");

        let vcp = sample_vcp(false, 0, true, 2, vec![cut.clone()]);
        assert_eq!(dynamic_scan_type(&vcp), "MRLE x 2");

        let vcp = sample_vcp(false, 0, false, 0, vec![cut]);
        assert_eq!(dynamic_scan_type(&vcp), "standard");
    }

    #[test]
    fn pack_super_resolution_round_trip() {
        // Half-deg azimuth + dual-pol 300 km enabled, others off → 0b1001 = 9.
        let cut = ElevationCut::new(
            0.5,
            ChannelConfiguration::ConstantPhase,
            WaveformType::CS,
            18.0,
            true,  // half_degree_azimuth
            false, // quarter_km_reflectivity
            false, // doppler_to_300km
            true,  // dual_pol_to_300km
            1,
            17,
            16.0,
            -20.0,
            12.0,
            0.0,
            0.0,
            0.0,
            false,
            0,
            false,
            0,
            false,
            false,
        );
        assert_eq!(pack_super_resolution(&cut), 0b1001);
    }

    #[test]
    fn pulse_width_str_table() {
        assert_eq!(pulse_width_str(PulseWidth::Short), "short");
        assert_eq!(pulse_width_str(PulseWidth::Long), "long");
        assert_eq!(pulse_width_str(PulseWidth::Unknown), "");
    }

    #[test]
    fn rda_scan_data_flags_xradar_bits() {
        // Pinning xradar's `decode_rda_scan_data_flags` verbatim: AVSET at bit
        // 1 (0x0002), EBC at bit 3 (0x0008). The upstream Rust accessor
        // misnumbers these, so we don't use it.
        assert_eq!(decode_rda_scan_data_flags(0x0000), (false, false));
        assert_eq!(decode_rda_scan_data_flags(0x0002), (true, false));
        assert_eq!(decode_rda_scan_data_flags(0x0008), (false, true));
        assert_eq!(decode_rda_scan_data_flags(0x000A), (true, true));
        // Bit 0 (0x0001) is spare per ICD; should NOT toggle AVSET.
        assert_eq!(decode_rda_scan_data_flags(0x0001), (false, false));
    }

    #[test]
    fn volume_attrs_without_msg2_zeroes_rda_fields() {
        let vcp = sample_vcp(false, 0, false, 0, vec![sample_cut()]);
        let attrs = volume_attrs(&vcp, None, &[], 5);
        assert_eq!(attrs.dynamic_scan_type, "standard");
        assert!(!attrs.avset_enabled);
        assert!(!attrs.ebc_enabled);
        assert_eq!(attrs.super_res_status, 0);
        assert_eq!(attrs.rda_build_number, 0);
        assert_eq!(attrs.operational_mode, 0);
        assert_eq!(attrs.actual_elevation_cuts, 5);
        assert_eq!(attrs.number_elevation_cuts, 1);
        // Empty `sweeps` slice → empty per-sweep arrays even when the
        // VCP carries cuts. Pins "we size by sweeps, not by cuts."
        assert!(attrs.sweep_attrs.is_empty());
        assert!(attrs.sweep_time_ranges.is_empty());
    }

    /// HIGH-priority: pin that `volume_attrs` populates one
    /// `NexradSweepAttrs` per sweep, sourced from the corresponding
    /// `ElevationCut`. A regression that drops the sweeps loop
    /// (or sources from `sweeps[0]` for every entry) is caught here.
    #[test]
    fn volume_attrs_populates_one_sweep_attr_per_sweep_from_matching_cut() {
        // Three cuts with distinct, observable signatures: vary the
        // commanded angle and the SAILS bit so each cut is uniquely
        // identifiable from the resulting `NexradSweepAttrs` (via
        // `sails_cut`).
        let cut_a = cut_with(0.5, false);
        let cut_b = cut_with(0.9, true);
        let cut_c = cut_with(1.5, false);
        let vcp = sample_vcp(true, 1, false, 0, vec![cut_a, cut_b, cut_c]);
        let sweeps = vec![empty_sweep(1), empty_sweep(2), empty_sweep(3)];

        let attrs = volume_attrs(&vcp, None, &sweeps, 3);

        assert_eq!(attrs.sweep_attrs.len(), 3, "one entry per sweep");
        assert!(!attrs.sweep_attrs[0].sails_cut);
        assert!(attrs.sweep_attrs[1].sails_cut, "cut_b has sails_cut=true");
        assert!(!attrs.sweep_attrs[2].sails_cut);
    }

    /// HIGH-priority: pin the truncated-VCP padding contract. When
    /// `cuts.len() < sweeps.len()`, trailing entries fall back to
    /// `NexradSweepAttrs::default()` rather than panicking or
    /// truncating the output.
    #[test]
    fn volume_attrs_pads_with_default_when_cuts_shorter_than_sweeps() {
        // 1 cut, 4 sweeps. Entries 1..4 must be defaults.
        let vcp = sample_vcp(false, 0, false, 0, vec![sample_cut()]);
        let sweeps = vec![
            empty_sweep(1),
            empty_sweep(2),
            empty_sweep(3),
            empty_sweep(4),
        ];

        let attrs = volume_attrs(&vcp, None, &sweeps, 4);

        assert_eq!(attrs.sweep_attrs.len(), 4);
        // Entry 0 is sourced from the real cut (waveform_type non-empty).
        assert_eq!(
            attrs.sweep_attrs[0].waveform_type,
            "contiguous_surveillance"
        );
        // Entries 1..4 are defaults — empty waveform_type is the
        // sentinel that tells consumers "we don't know."
        for sa in &attrs.sweep_attrs[1..] {
            assert_eq!(sa, &NexradSweepAttrs::default());
        }
    }

    /// HIGH-priority: pin both branches of `Sweep::time_range`.
    /// `None` for empty sweeps (no radials), `Some((start, end))` with
    /// `start <= end` for sweeps with timestamped radials. We use
    /// `f64` Unix seconds to match the public axis convention.
    #[test]
    fn volume_attrs_sweep_time_ranges_some_and_none_branches() {
        let vcp = sample_vcp(false, 0, false, 0, vec![sample_cut(), sample_cut()]);
        let sweeps = vec![
            empty_sweep(1), // → None
            sweep_with_timestamps(
                2,
                &[
                    1_700_000_000_000_i64, // 2023-11-14T22:13:20Z
                    1_700_000_005_500_i64, // +5.5 s
                    1_700_000_002_000_i64,
                ],
            ),
        ];

        let attrs = volume_attrs(&vcp, None, &sweeps, 2);

        assert_eq!(attrs.sweep_time_ranges.len(), 2);
        assert!(attrs.sweep_time_ranges[0].is_none(), "no radials → None");

        let (start, end) = attrs.sweep_time_ranges[1].expect("Some");
        // 1700000000.000 → 1700000005.500. Allow microsecond noise from
        // the `timestamp_micros() / 1e6` round-trip.
        assert!((start - 1_700_000_000.0).abs() < 1e-3, "start = {start}");
        assert!((end - 1_700_000_005.5).abs() < 1e-3, "end = {end}");
        assert!(start <= end, "time_range invariant: start <= end");
    }

    fn cut_with(angle_deg: f64, sails: bool) -> ElevationCut {
        ElevationCut::new(
            angle_deg,
            ChannelConfiguration::ConstantPhase,
            WaveformType::CS,
            18.0,
            true,
            true,
            false,
            false,
            1,
            17,
            16.0,
            -20.0,
            12.0,
            0.0,
            0.0,
            0.0,
            sails,
            if sails { 1 } else { 0 },
            false,
            0,
            false,
            false,
        )
    }

    fn empty_sweep(elevation_number: u8) -> Sweep {
        Sweep::new(elevation_number, Vec::new())
    }

    /// Build a sweep whose radials carry the given collection
    /// timestamps (milliseconds since Unix epoch, matching upstream
    /// `Radial::collection_timestamp`). Other radial fields are
    /// arbitrary defaults — only the timestamps drive
    /// [`Sweep::time_range`].
    fn sweep_with_timestamps(elevation_number: u8, timestamps_ms: &[i64]) -> Sweep {
        use nexrad_model::data::{MomentData, Radial, RadialStatus};
        let radials: Vec<Radial> = timestamps_ms
            .iter()
            .enumerate()
            .map(|(i, ts)| {
                let reflectivity =
                    MomentData::from_fixed_point(1, 2_000, 250, 8, 2.0, 66.0, vec![10]);
                Radial::new(
                    *ts,
                    i as u16,
                    (i as f32) * 1.0,
                    0.5,
                    RadialStatus::ScanStart,
                    elevation_number,
                    0.5,
                    Some(reflectivity),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
            })
            .collect();
        Sweep::new(elevation_number, radials)
    }

    fn sample_cut() -> ElevationCut {
        ElevationCut::new(
            0.5,
            ChannelConfiguration::ConstantPhase,
            WaveformType::CS,
            18.0,
            true,
            true,
            false,
            false,
            1,
            17,
            16.0,
            -20.0,
            12.0,
            0.0,
            0.0,
            0.0,
            false,
            0,
            false,
            0,
            false,
            false,
        )
    }

    fn sample_vcp(
        sails: bool,
        sails_cuts: u8,
        mrle: bool,
        mrle_cuts: u8,
        cuts: Vec<ElevationCut>,
    ) -> VolumeCoveragePattern {
        VolumeCoveragePattern::new(
            212,
            1,
            0.5,
            PulseWidth::Short,
            sails,
            sails_cuts,
            mrle,
            mrle_cuts,
            false,
            false,
            0,
            false,
            false,
            cuts,
        )
    }
}

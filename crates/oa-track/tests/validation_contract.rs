//! Public-API validation scenarios across input admission and track initiation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::{frame0, measure, Manoeuvre, Rng};
use oa_track::fusion::{
    fusion_adsb, fusion_advance, fusion_config, fusion_finish, fusion_new, fusion_stats,
    fusion_tracker_config, fusion_tracker_stats, Fusion as TrackerFusion,
};
use oa_track::radar::RadarSite;
use oa_track::{
    output_new, tracker_flush, tracker_live, tracker_new, tracker_stats, tracker_update,
    Measurement, TrackOutput, TrackerConfig, DEFAULT_CONFIG, REF_TIME, TIME_LIMIT_NS,
};

const S: i64 = 1_000_000_000;

const SITE: RadarSite = RadarSite {
    lat_deg: 41.976_940_27,
    lon_deg: -87.908_149_72,
    height_m: 207.0,
    sigma_range_m: 70.0,
    sigma_az_deg: 0.08,
    range_quantum_m: 0.0,
    alt_quantum_ft: 100,
    period_ns: 4_800_000_000,
    epoch_ns: 0,
    sensor: 17,
};

fn feed_and_finish(fusion: &mut TrackerFusion, out: &mut TrackOutput) {
    for second in 1..=3 {
        fusion_adsb(fusion, &sample(f64::from(second)));
    }
    fusion_finish(fusion, 4 * S, out);
}

fn output_digest(out: &TrackOutput) -> String {
    let mut hash = oa_sha256::Sha256::new();
    oa_track::output_hash_into(&mut hash, out);
    oa_sha256::to_hex(&hash.finalize())
}

fn sample(t_s: f64) -> Measurement {
    let mut rng = Rng(0x17);
    let truth = Manoeuvre::straight(190.0, 75.0).at(t_s, 0.0, 0.0);
    measure(
        &frame0(),
        0xA10001,
        t_s,
        &truth,
        6.0,
        Some(1.0),
        false,
        &mut rng,
    )
}

// REQ: OA-TRK-001, OA-TRK-002
#[test]
fn refused_time_and_measurement_leave_later_valid_fusion_evidence_usable() {
    let mut cfg = fusion_config(fusion_tracker_config(
        &TrackerConfig {
            max_tracks: 16,
            out_capacity: 1 << 16,
            log_capacity: 256,
            ..DEFAULT_CONFIG
        },
        &[SITE],
    ));
    cfg.registration = false;
    let mut fusion = fusion_new(&cfg, &[SITE]).unwrap();
    let mut out = output_new(cfg.tracker.out_capacity);

    // One report has a time outside the accepted range; a separate report has invalid
    // covariance. Neither failure may create a track or change later reports' fate.
    let mut bad = sample(0.0);
    bad.toa_ns = i64::MIN;
    fusion_adsb(&mut fusion, &bad);
    let mut malformed = sample(0.0);
    malformed.pos_cov_m2 = [f32::NAN, 0.0, 1.0];
    fusion_adsb(&mut fusion, &malformed);
    fusion_advance(&mut fusion, 0, &mut out);

    // Three valid, geometrically compatible reports follow one straight path after the
    // refusals. They must still initiate exactly one track.
    fusion_adsb(&mut fusion, &sample(0.0));
    fusion_adsb(&mut fusion, &sample(1.0));
    fusion_adsb(&mut fusion, &sample(2.0));
    fusion_finish(&mut fusion, 3 * S, &mut out);

    let stats = fusion_tracker_stats(&fusion);
    assert_eq!(fusion_stats(&fusion).adsb_in, 5);
    assert_eq!(stats.refused.iter().sum::<u64>(), 2);
    assert_eq!(stats.applied, 3);
    assert_eq!(stats.started, 1);
    assert!(out.addr.contains(&0xA10001));
    assert!(out.t_ns.iter().all(|&t| t >= 0));
    assert_eq!(
        stats.offered,
        stats.applied + stats.refused.iter().sum::<u64>()
    );
}

// REQ: OA-TRK-088
#[test]
fn rejected_impossible_advance_does_not_close_future_input_buckets() {
    let make_fusion = || {
        let cfg = fusion_config(fusion_tracker_config(
            &TrackerConfig {
                max_tracks: 16,
                out_capacity: 1 << 16,
                log_capacity: 256,
                ..DEFAULT_CONFIG
            },
            &[SITE],
        ));
        fusion_new(&cfg, &[SITE]).unwrap()
    };
    let mut control = make_fusion();
    let mut control_out = output_new(1 << 16);
    feed_and_finish(&mut control, &mut control_out);

    let mut probe = make_fusion();
    let mut probe_out = output_new(1 << 16);
    // TIME_LIMIT_NS is i64::MAX / 2, so this advance is refused by the tracker.
    fusion_advance(&mut probe, i64::MAX, &mut probe_out);
    feed_and_finish(&mut probe, &mut probe_out);

    assert!(control_out.addr.contains(&0xA10001));
    assert_eq!(output_digest(&probe_out), output_digest(&control_out));
    assert_eq!(fusion_stats(&probe).late, 0);
    assert_eq!(fusion_tracker_stats(&probe).refused[oa_track::REF_TIME], 1);
}

// REQ: OA-TRK-069
#[test]
fn invalid_tracker_flush_times_do_not_end_live_tracks_or_emit_rows() {
    let cfg = TrackerConfig {
        max_tracks: 16,
        out_capacity: 1 << 16,
        log_capacity: 256,
        ..DEFAULT_CONFIG
    };
    let mut tracker = tracker_new(&cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    for second in 0..=2 {
        tracker_update(&mut tracker, &sample(f64::from(second)), &mut out);
    }
    let live = tracker_live(&tracker);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].0, 0xA10001);
    let digest = output_digest(&out);
    let refusal_count = tracker_stats(&tracker).refused[REF_TIME];

    for (i, invalid) in [TIME_LIMIT_NS + 1, -TIME_LIMIT_NS - 1, i64::MAX, i64::MIN]
        .into_iter()
        .enumerate()
    {
        tracker_flush(&mut tracker, invalid, &mut out);
        assert_eq!(tracker_live(&tracker), live);
        assert_eq!(output_digest(&out), digest);
        assert_eq!(
            tracker_stats(&tracker).refused[REF_TIME],
            refusal_count + i as u64 + 1
        );
    }

    tracker_flush(&mut tracker, 3 * S, &mut out);
    assert!(tracker_live(&tracker).is_empty());
}

// REQ: OA-TRK-069
#[test]
fn invalid_fusion_finish_times_preserve_pending_inputs_and_live_tracks() {
    let mut cfg = fusion_config(fusion_tracker_config(
        &TrackerConfig {
            max_tracks: 16,
            out_capacity: 1 << 16,
            log_capacity: 256,
            ..DEFAULT_CONFIG
        },
        &[SITE],
    ));
    cfg.registration = false;
    let mut fusion = fusion_new(&cfg, &[SITE]).unwrap();
    let mut out = output_new(cfg.tracker.out_capacity);
    for second in 0..=2 {
        fusion_adsb(&mut fusion, &sample(f64::from(second)));
    }
    fusion_advance(&mut fusion, 3 * S, &mut out);
    let live = oa_track::fusion::fusion_tracker(&fusion);
    let live = tracker_live(live);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].0, 0xA10001);

    fusion_adsb(&mut fusion, &sample(4.0));
    let digest = output_digest(&out);
    let applied = fusion_tracker_stats(&fusion).applied;
    let refusal_count = fusion_tracker_stats(&fusion).refused[REF_TIME];
    for (i, invalid) in [TIME_LIMIT_NS + 1, -TIME_LIMIT_NS - 1, i64::MAX, i64::MIN]
        .into_iter()
        .enumerate()
    {
        fusion_finish(&mut fusion, invalid, &mut out);
        assert_eq!(
            tracker_live(oa_track::fusion::fusion_tracker(&fusion)),
            live
        );
        assert_eq!(fusion_tracker_stats(&fusion).applied, applied);
        assert_eq!(output_digest(&out), digest);
        assert_eq!(
            fusion_tracker_stats(&fusion).refused[REF_TIME],
            refusal_count + i as u64 + 1
        );
    }

    fusion_finish(&mut fusion, 5 * S, &mut out);
    let tracker = oa_track::fusion::fusion_tracker(&fusion);
    assert_eq!(fusion_tracker_stats(&fusion).applied, applied + 1);
    assert!(tracker_live(tracker).is_empty());
    assert!(out
        .t_ns
        .iter()
        .zip(&out.addr)
        .any(|(&time, &addr)| time == 4 * S && addr == 0xA10001));
    assert_eq!(fusion_stats(&fusion).late, 0);
}

// REQ: OA-TRK-069
#[test]
fn valid_fusion_advance_times_at_both_limits_are_not_refused_after_bucket_rounding() {
    for now_ns in [
        -TIME_LIMIT_NS,
        -TIME_LIMIT_NS + 1,
        TIME_LIMIT_NS - 1,
        TIME_LIMIT_NS,
    ] {
        let mut cfg = fusion_config(fusion_tracker_config(
            &TrackerConfig {
                max_tracks: 16,
                out_capacity: 1 << 16,
                log_capacity: 256,
                ..DEFAULT_CONFIG
            },
            &[SITE],
        ));
        cfg.registration = false;
        let mut fusion = fusion_new(&cfg, &[SITE]).unwrap();
        let mut out = output_new(cfg.tracker.out_capacity);

        fusion_advance(&mut fusion, now_ns, &mut out);

        assert_eq!(
            fusion_tracker_stats(&fusion).refused[REF_TIME],
            0,
            "valid advance {now_ns} was refused with bucket {}",
            cfg.bucket_ns
        );
    }
}

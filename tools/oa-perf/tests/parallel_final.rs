//! Shell integration check: synthetic sensor measurements pass through the public tracker and
//! the tracker output, rather than truth points, feeds the parallel-final monitor.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use oa_runway::final_monitor::{
    prepare_centerline_pair, update, FinalAlertBuffer, FinalMonitorConfig, FinalMonitorState,
    FinalSeverity, PredictedTracks, RunwayFinalSelection, UpdateError,
    DEFAULT_CENTERLINE_EXTENSION_M, DEFAULT_CONFIG, DEGRADED_AFTER_NS,
};
use oa_runway::plane::{plane_new, to_local, Geo};
use oa_runway::runway::{build_runway, DeclaredFt, EndInput, Runway, RunwayInput};
use oa_sensorsim::plots::{plots_len, plots_row};
use oa_sensorsim::profile::{RadarProfile, EARTH_K_STANDARD, MODE_S_ATC302};
use oa_sensorsim::radar::{simulate_radar, RadarSite as SimRadarSite};
use oa_track::radar::{plot_convert, radar_geom, RadarPlot, RadarSite, NO_BIAS};
use oa_track::synth_truth::{build_synthetic_truth, Seg, Traj};
use oa_track::{
    output_clear, output_len, output_new, tracker_new, tracker_update, TrackerConfig,
    DEFAULT_CONFIG as TRACKER,
};

const EPOCH_NS: i64 = 1_700_000_000_000_000_000;
const DEFAULT_SENSOR_PERIOD_NS: i64 = 1_200_000_000;
const TERMINAL_SENSOR_PERIOD_NS: i64 = 4_800_000_000;

fn end(name: &'static str, alignment: f64, lat: f64, lon: f64) -> EndInput<'static> {
    EndInput {
        name,
        true_alignment_deg: Some(alignment),
        position: Some(Geo {
            lat_deg: lat,
            lon_deg: lon,
        }),
        displaced_position: None,
        displaced_len_ft: None,
        elev_ft: None,
        displaced_elev_ft: None,
        declared: DeclaredFt::default(),
        cifp_threshold: None,
    }
}

fn kord_pair(
    config: FinalMonitorConfig,
) -> (
    oa_runway::plane::Plane,
    Runway,
    Runway,
    oa_runway::final_monitor::FinalPair,
) {
    let plane = plane_new(41.97694027, -87.90814972).expect("pinned KORD reference point");
    // Public NASR APT_RWY_END fixture values, cycle 2026-10-29; these are source facts, not a
    // procedure fix. The monitor's outer endpoints are explicitly derived from the centerlines.
    let r27r = build_runway(
        &plane,
        &RunwayInput {
            length_ft: Some(7_500.0),
            width_ft: Some(150.0),
            a: end("27R", 270.0, 42.0028308, -87.89908325),
            b: end("09L", 90.0, 42.00283205, -87.92667608),
        },
    )
    .expect("KORD public 09L/27R runway");
    let r27l = build_runway(
        &plane,
        &RunwayInput {
            length_ft: Some(11_260.0),
            width_ft: Some(150.0),
            a: end("27L", 270.0, 41.98390033, -87.89015488),
            b: end("09R", 90.0, 41.98389366, -87.93156908),
        },
    )
    .expect("KORD public 09R/27L runway");
    let pair = prepare_centerline_pair(
        RunwayFinalSelection {
            runway_id: 27_001,
            runway: &r27r,
            end_name: *b"27R",
        },
        RunwayFinalSelection {
            runway_id: 27_002,
            runway: &r27l,
            end_name: *b"27L",
        },
        DEFAULT_CENTERLINE_EXTENSION_M,
        config,
    )
    .expect("pinned parallel centerlines");
    (plane, r27r, r27l, pair)
}

fn synthetic_truth(pair: oa_runway::final_monitor::FinalPair) -> oa_truth::Truth {
    let reference = oa_geo::aeqd::frame_new(41.97694027, -87.90814972);
    let along = -3_500.0;
    let cross = -(pair.ntz_half_width_m + 500.0);
    let east = pair.origin.east_m + pair.axis_east * along + pair.normal_east * cross;
    let north = pair.origin.north_m + pair.axis_north * along + pair.normal_north * cross;
    let (lat, lon) = oa_geo::aeqd::unproject(&reference, east, north).expect("synthetic start");
    let heading = (pair.axis_east * 0.866_025_403_784_438_6 + pair.normal_east * 0.5)
        .atan2(pair.axis_north * 0.866_025_403_784_438_6 + pair.normal_north * 0.5)
        .to_degrees()
        .rem_euclid(360.0);
    build_synthetic_truth(
        lat,
        lon,
        EPOCH_NS,
        &[Traj {
            addr: 0xA00001,
            t0_s: 0.0,
            x0: 0.0,
            y0: 0.0,
            heading_deg: heading,
            speed_mps: 130.0,
            alt_ft: 3_000.0,
            segs: vec![Seg {
                dur_s: 60.0,
                turn_dps: 0.0,
                vrate_fpm: 0.0,
            }],
        }],
    )
    .expect("synthetic truth")
}

// Shared implementation exercised by both named sensor-cadence tests below.
// HAZARD: HZ-CA-001, HZ-NET-STATE-001
fn run_blunder_pipeline(sensor_period_ns: i64, monitor_update_ns: u64, minimum_reports: usize) {
    let config = FinalMonitorConfig {
        update_ns: monitor_update_ns,
        ..DEFAULT_CONFIG
    };
    let (plane, _r27r, _r27l, pair) = kord_pair(config);
    let truth = synthetic_truth(pair);
    // Keep this deterministic scenario focused on pipeline timing, not detection-probability
    // luck. Geometry/noise remain the named Mode S profile; the explicit synthetic run sets
    // detection to 1 and false/split rates to zero so the test has no silently lost target work.
    let profile = RadarProfile {
        pd: 1.0,
        // The tracker cannot convert a first report without Mode C before association; keep
        // altitude present so this synthetic pipeline test measures end-to-end detector behavior.
        pd_alt: 1.0,
        false_rate: 0.0,
        split_rate: 0.0,
        ..MODE_S_ATC302
    };
    let simulated_site = SimRadarSite {
        lat_deg: 41.97694027,
        lon_deg: -87.90814972,
        height_m: 207.264,
        rotation_period_s: sensor_period_ns as f64 / 1_000_000_000.0,
        initial_az_deg: 0.0,
        epoch_ns: EPOCH_NS,
        coverage_range_m: 60.0 * 1_852.0,
        earth_k: EARTH_K_STANDARD,
        profile,
        bias_range_m: 0.0,
        bias_az_deg: 0.0,
        split_dr_m: 0.0,
        split_daz_deg: 0.0,
        sac: 1,
        sic: 1,
    };
    let (simulated, sim_stats) = simulate_radar(
        &truth,
        &simulated_site,
        0xA2_2026,
        EPOCH_NS,
        EPOCH_NS + 60_000_000_000,
    )
    .expect("bounded synthetic radar scans");
    assert_eq!(sim_stats.aircraft, 1);
    assert!(sim_stats.missed == 0 && sim_stats.unsolved == 0 && sim_stats.no_truth == 0);
    let sensor_site = RadarSite {
        lat_deg: simulated_site.lat_deg,
        lon_deg: simulated_site.lon_deg,
        height_m: simulated_site.height_m,
        sigma_range_m: profile.range_sigma_m,
        sigma_az_deg: profile.az_sigma_deg,
        range_quantum_m: profile.range_quantum_m,
        alt_quantum_ft: profile.alt_quantum_ft as u16,
        period_ns: sensor_period_ns,
        epoch_ns: EPOCH_NS,
        sensor: 1,
    };
    let sensor_geom = radar_geom(&sensor_site);

    // This fixture is an explicitly declared sensor-cadence profile. The tracker defaults are
    // calibrated for 1 s reports; initiation timing follows this site's declared scan period,
    // while the configured 2-of-3 confirmation policy remains unchanged.
    let tracker_config = TrackerConfig {
        expected_interval_ns: sensor_period_ns,
        init_interval_ns: sensor_period_ns,
        ..TRACKER
    };
    assert_eq!(tracker_config.expected_interval_ns, sensor_site.period_ns);
    assert_eq!(tracker_config.init_interval_ns, sensor_site.period_ns);
    assert_eq!(tracker_config.init_m, 2);
    assert_eq!(tracker_config.init_n, 3);
    let mut tracker = tracker_new(&tracker_config).expect("sensor-cadence tracker config");
    let mut output = output_new(4096);
    let mut report_times = Vec::with_capacity(plots_len(&simulated));
    let mut monitor = FinalMonitorState::new();
    let mut alert_addresses = [0_u32; 1];
    let mut pair_ids = [pair.id; 1];
    let mut severities = [FinalSeverity::Caution; 1];
    let mut lat_e7 = [0_i32; 1];
    let mut lon_e7 = [0_i32; 1];
    let mut seconds_to_entry_milli = [0_u32; 1];
    let mut caution_at = None;
    let mut warning_at = None;
    let mut last_report = None;

    let mut true_plots = 0usize;
    let mut unconfirmed_first_report_rows = 0u64;
    let mut empty_confirmed_tracker_outputs = 0u64;
    let mut malformed_plot_rows = 0u64;
    let mut conversion_refusals = 0u64;
    let mut missing_tracker_addresses = 0u64;
    let mut unprojectable_tracker_rows = 0u64;
    let mut monitor_calls = 0u64;
    let mut monitor_due_updates = 0u64;
    let mut monitor_not_due_updates = 0u64;
    let mut monitor_examined_rows = 0u64;
    let mut monitor_alert_rows = 0u64;
    let mut monitor_bad_input_refusals = 0u64;
    let mut monitor_capacity_refusals = 0u64;
    let mut monitor_clock_refusals = 0u64;
    let mut monitor_degraded_updates = 0u64;
    for plot_ix in 0..plots_len(&simulated) {
        let Some(plot) = plots_row(&simulated, plot_ix) else {
            malformed_plot_rows += 1;
            continue;
        };
        if plot.addr != 0xA00001 {
            continue;
        }
        true_plots += 1;
        if let Some(previous) = last_report {
            report_times.push(plot.time_ns - previous);
        }
        last_report = Some(plot.time_ns);
        let sensor_plot = RadarPlot {
            time_ns: plot.time_ns,
            range_m: plot.range_m,
            az_deg: plot.az_deg,
            alt_ft: plot.alt_ft,
            addr: plot.addr,
            // The simulator's truth label is carried only as the tracker's non-decision label.
            label: plot.truth_addr,
            mode_a: plot.mode_a,
        };
        let measurement = match plot_convert(
            &sensor_geom,
            &sensor_site,
            &sensor_plot,
            &NO_BIAS,
            None,
            sensor_plot.addr,
        ) {
            Ok(converted) => converted.m,
            Err(_) => {
                conversion_refusals += 1;
                continue;
            }
        };
        output_clear(&mut output);
        tracker_update(&mut tracker, &measurement, &mut output);
        let rows = output_len(&output);
        if rows == 0 {
            // The tracker deliberately does not expose a one-hit tentative track. This first
            // report is still processed; it is not a lost sensor sample or a monitor refusal.
            if true_plots == 1 {
                unconfirmed_first_report_rows += 1;
            } else {
                empty_confirmed_tracker_outputs += 1;
            }
            continue;
        }
        let Some(i) = (0..rows).find(|&row| {
            output
                .addr
                .get(row)
                .is_some_and(|address| *address == measurement.addr)
        }) else {
            missing_tracker_addresses += 1;
            continue;
        };
        let row_addr = output.addr[i];
        let Some(local) = to_local(
            &plane,
            Geo {
                lat_deg: f64::from(output.lat_e7[i]) / 10_000_000.0,
                lon_deg: f64::from(output.lon_e7[i]) / 10_000_000.0,
            },
        ) else {
            unprojectable_tracker_rows += 1;
            continue;
        };
        let east = [local.east_m];
        let north = [local.north_m];
        let vx = [f64::from(output.vel_east_mps[i])];
        let vy = [f64::from(output.vel_north_mps[i])];
        let r95 = [oa_track::r95_m_of(&output.pos_cov_m2[i])];
        let addresses = [row_addr];
        let lats = [output.lat_e7[i]];
        let lons = [output.lon_e7[i]];
        let report_ns = u64::try_from(measurement.toa_ns).expect("nonnegative sensor time");
        let result = update(
            pair,
            &mut monitor,
            report_ns,
            report_ns,
            PredictedTracks {
                addresses: &addresses,
                east_m: &east,
                north_m: &north,
                velocity_east_mps: &vx,
                velocity_north_mps: &vy,
                r95_m: &r95,
                lat_e7: &lats,
                lon_e7: &lons,
            },
            FinalAlertBuffer {
                addresses: &mut alert_addresses,
                pair_ids: &mut pair_ids,
                severity: &mut severities,
                lat_e7: &mut lat_e7,
                lon_e7: &mut lon_e7,
                seconds_to_entry_milli: &mut seconds_to_entry_milli,
            },
        );
        let result = match result {
            Ok(result) => result,
            Err(UpdateError::BadInput) => {
                monitor_bad_input_refusals += 1;
                continue;
            }
            Err(UpdateError::Capacity) => {
                monitor_capacity_refusals += 1;
                continue;
            }
            Err(UpdateError::ClockRegression) => {
                monitor_clock_refusals += 1;
                continue;
            }
        };
        monitor_calls += 1;
        if result.due {
            monitor_due_updates += 1;
            monitor_examined_rows += u64::from(result.examined);
        } else {
            monitor_not_due_updates += 1;
        }
        monitor_alert_rows += u64::from(result.alerts);
        if result.degraded {
            monitor_degraded_updates += 1;
        }
        assert!(
            result.alerts <= 1,
            "one predicted track fits the alert buffer"
        );
        if result.alerts > 0 {
            match severities[0] {
                FinalSeverity::Caution => caution_at.get_or_insert(measurement.toa_ns),
                FinalSeverity::Warning => warning_at.get_or_insert(measurement.toa_ns),
            };
        }
    }
    assert!(
        true_plots >= minimum_reports,
        "simulator omitted too many target scans for the configured sensor cadence"
    );
    assert_eq!(malformed_plot_rows, 0);
    assert_eq!(sim_stats.missed, 0);
    assert_eq!(sim_stats.unsolved, 0);
    assert_eq!(sim_stats.no_truth, 0);
    assert_eq!(sim_stats.out_of_range, 0);
    assert_eq!(sim_stats.below_horizon, 0);
    assert_eq!(sim_stats.non_finite, 0);
    assert_eq!(sim_stats.false_plots, 0);
    assert_eq!(sim_stats.split_plots, 0);
    assert_eq!(
        sim_stats.detected,
        u64::try_from(true_plots).expect("bounded plot count")
    );
    assert_eq!(sim_stats.in_coverage, sim_stats.detected);
    assert_eq!(conversion_refusals, 0);
    assert_eq!(unconfirmed_first_report_rows, 1);
    assert_eq!(empty_confirmed_tracker_outputs, 0);
    assert_eq!(missing_tracker_addresses, 0);
    assert_eq!(unprojectable_tracker_rows, 0);
    assert_eq!(monitor_bad_input_refusals, 0);
    assert_eq!(monitor_capacity_refusals, 0);
    assert_eq!(monitor_clock_refusals, 0);
    assert_eq!(monitor_degraded_updates, 0);
    assert_eq!(monitor_calls, monitor_due_updates + monitor_not_due_updates);
    assert!(monitor_due_updates > 0);
    assert_eq!(monitor_examined_rows, monitor_due_updates);
    assert!(monitor_alert_rows > 0);
    assert!(!report_times.is_empty());
    let cadence_tolerance_ns = 200_000_000;
    assert!(
        report_times.iter().all(|&dt| {
            (sensor_period_ns - cadence_tolerance_ns..=sensor_period_ns + cadence_tolerance_ns)
                .contains(&dt)
        }),
        "beam-crossing timestamps, not tracker ticks, define report cadence"
    );
    let tracker_stats = oa_track::tracker_stats(&tracker);
    assert_eq!(tracker_stats.refused.iter().sum::<u64>(), 0);
    assert_eq!(tracker_stats.capacity_refused, 0);
    assert_eq!(tracker_stats.rows_unrepresentable, 0);
    assert!(
        caution_at.is_some(),
        "the real tracker output should predict NTZ entry"
    );
    assert!(
        warning_at.is_some(),
        "the real tracker output should later enter the NTZ"
    );
    assert!(
        caution_at < warning_at,
        "the track should progress from caution to warning"
    );
}

// REQ: OA-RWY-015, OA-RWY-016
#[test]
fn synthetic_1_2s_reports_track_a_blunder_from_caution_to_warning() {
    run_blunder_pipeline(
        DEFAULT_SENSOR_PERIOD_NS,
        DEFAULT_SENSOR_PERIOD_NS as u64,
        30,
    );
}

// REQ: OA-RWY-015, OA-RWY-016, OA-RWY-017
#[test]
fn synthetic_4_8s_reports_track_a_blunder_without_false_degraded_status() {
    assert_eq!(DEGRADED_AFTER_NS, TERMINAL_SENSOR_PERIOD_NS as u64);
    run_blunder_pipeline(
        TERMINAL_SENSOR_PERIOD_NS,
        TERMINAL_SENSOR_PERIOD_NS as u64,
        10,
    );
}

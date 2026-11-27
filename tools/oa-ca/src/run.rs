//! The run: clip, tracker, conflict alert, files. Shell (B3).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use oa_bench::input::{file_opener, load_adsb_counted, Opener};
use oa_ca::{
    alert_log_clear, alert_log_new, ca_check, ca_cycle_with_inhibits, ca_finish, ca_new,
    ca_note_lost_input, ca_totals, event_log_clear, event_log_new, Alert, AlertLog, AltLost,
    CaState, CaTrack, CycleStats, EventLog, F_COAST, F_IDENTITY_UNCONFIRMED, F_RADAR_ONLY,
    F_UNCERTAIN,
};
use oa_encounter_run::run::{ground_at, ground_table, GroundTable};
use oa_safety::inhibit::{inhibits_new, InhibitTable};
use oa_safety::net::{
    net_status, state_report, state_table_new, Area, NetId, NetReport, Reason, StateTable,
};
use oa_sealed::Sealed;
use oa_sha256::{sha256, to_hex};
use oa_track::adsb::{adsb_measurements, check_adsb_config, AdsbConfig, AdsbStats};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_flush, tracker_new,
    tracker_stats, tracker_update, Measurement, TrackOutput, TrackerConfig, DEFAULT_CONFIG,
    O_COAST, O_HAS_ALT, O_IDENTITY_UNCONFIRMED, O_TICK, O_UNCERTAIN,
};
use oa_track_tools::score::{interval_stats, scoring_config, split_reports, IntervalStats};

use crate::args::Args;
use crate::config::Settings;
use crate::guard::{open_guarded, refuse_sealed, OpenFail};
use crate::report::{alerts_csv, events_csv, state_text, summary_text, SummaryInput};

/// The chi-square quantile of 2 degrees of freedom at 95 %: the 95 % radius of a circular
/// Gaussian is `sqrt(CHI2_2_95 * variance)`; for the covariance of a track the larger eigenvalue
/// takes the variance, so the radius holds in every direction.
pub const CHI2_2_95: f64 = 5.991_464_547_107_98;
const SPEED_DIAGNOSTIC_HOURS: usize = 24;
const HOUR_NS: i64 = 3_600_000_000_000;
const DAY_NS: i64 = 24 * HOUR_NS;

/// The 95 % horizontal radius, metres, of a position covariance `[ee, en, nn]` (m^2); NaN if the
/// covariance is not usable (the core then holds it to the cap and counts it).
pub fn r95_of(cov: [f32; 3]) -> f32 {
    let (ee, en, nn) = (f64::from(cov[0]), f64::from(cov[1]), f64::from(cov[2]));
    if !(ee.is_finite() && en.is_finite() && nn.is_finite()) || ee < 0.0 || nn < 0.0 {
        return f32::NAN;
    }
    // Allow ordinary f32 rounding of the variances and cross-term near the rank-one boundary.
    // The component products fit in f64, so compare squared magnitudes directly.
    let cross_sq = en * en;
    let rounding_factor = 1.0 + 4.0 * f64::from(f32::EPSILON);
    if cross_sq > ee * nn * (rounding_factor * rounding_factor) {
        return f32::NAN;
    }
    let half = (ee - nn) / 2.0;
    let lam = (ee + nn) / 2.0 + (half * half + cross_sq).sqrt();
    (CHI2_2_95 * lam).sqrt() as f32
}

/// What a run of the tracker and conflict alert produced. A7.2: cold struct, size guard waived.
pub struct Record {
    pub alerts: Vec<Alert>,
    pub events: Vec<AltLost>,
    pub totals: CycleStats,
    /// Cycles by the text of the net state after each.
    pub net_cycles: BTreeMap<String, u64>,
    pub log_overflow: u64,
    pub event_overflow: u64,
    pub tracker_overflow: u64,
    /// Valid measurements refused because the track table had no free slot.
    pub tracker_capacity_refused: u64,
    pub tick_rows: u64,
    /// Rows of the tracker's output left out for a missing column (also in `totals`).
    pub rows_skipped: u64,
    /// Cycles that left work undone and also had input missing.
    pub cycles_both: u64,
    /// The net state after the last report, the end of the run included.
    pub final_net: String,
    /// Present only when `--speed-diagnostics` was explicitly requested.
    pub speed_diagnostics: Option<SpeedDiagnostics>,
}

/// Address-free count-only view of CA speed rejections. Input speed/category and the filter
/// update path are unavailable from the current tracker output contract, so each rejected row is
/// explicitly counted as context-unavailable rather than joined heuristically.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpeedDiagnosticHour {
    /// Finite velocity vectors above the configured limit; this matches CA `rows_bad_speed`.
    pub bad_speed_rows: u64,
    /// Non-finite velocity components, the separate CA reason counter.
    pub nonfinite_component_rows: u64,
    pub coast_flagged_rows: u64,
    pub not_marked_coast_rows: u64,
    pub last_input_speed_unknown_rows: u64,
    pub aircraft_class_unknown_rows: u64,
    pub update_path_unknown_rows: u64,
}

/// A fixed 24-hour UTC-day table plus an explicit out-of-anchor-day bucket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeedDiagnostics {
    pub day_start_ns: Option<i64>,
    pub hours: [SpeedDiagnosticHour; SPEED_DIAGNOSTIC_HOURS],
    pub out_of_anchor_day: SpeedDiagnosticHour,
    /// Run-global adapter counts, not attributed to individual output rows.
    pub adapter: Option<AdsbStats>,
    /// Run-global tracker gate count, not attributed to individual output rows.
    pub tracker_velocity_gated: Option<u64>,
}

impl SpeedDiagnostics {
    fn new() -> Self {
        Self {
            day_start_ns: None,
            hours: [SpeedDiagnosticHour::default(); SPEED_DIAGNOSTIC_HOURS],
            out_of_anchor_day: SpeedDiagnosticHour::default(),
            adapter: None,
            tracker_velocity_gated: None,
        }
    }

    fn add_cycle(&mut self, t_ns: i64, rows: &[CaTrack], max_speed_mps: f64) {
        let day_start = t_ns.div_euclid(DAY_NS) * DAY_NS;
        let anchor = *self.day_start_ns.get_or_insert(day_start);
        let bucket = if day_start != anchor {
            &mut self.out_of_anchor_day
        } else {
            let hour = ((t_ns - anchor) / HOUR_NS) as usize;
            match self.hours.get_mut(hour) {
                Some(bucket) => bucket,
                None => return,
            }
        };
        for row in rows {
            let east = f64::from(row.vel_east_mps);
            let north = f64::from(row.vel_north_mps);
            let components_finite = east.is_finite() && north.is_finite();
            if !components_finite {
                bucket.nonfinite_component_rows = bucket.nonfinite_component_rows.saturating_add(1);
                continue;
            }
            if east * east + north * north <= max_speed_mps * max_speed_mps {
                continue;
            }
            bucket.bad_speed_rows = bucket.bad_speed_rows.saturating_add(1);
            bucket.last_input_speed_unknown_rows =
                bucket.last_input_speed_unknown_rows.saturating_add(1);
            bucket.aircraft_class_unknown_rows =
                bucket.aircraft_class_unknown_rows.saturating_add(1);
            bucket.update_path_unknown_rows = bucket.update_path_unknown_rows.saturating_add(1);
            if row.flags & F_COAST != 0 {
                bucket.coast_flagged_rows = bucket.coast_flagged_rows.saturating_add(1);
            } else {
                bucket.not_marked_coast_rows = bucket.not_marked_coast_rows.saturating_add(1);
            }
        }
    }
}

/// The optional diagnostic is separate from the alert and summary products; it contains no
/// identifiers and never participates in the decision path.
pub fn speed_diagnostics_csv(diagnostics: &SpeedDiagnostics) -> String {
    let mut out = String::from(
        "bucket,hour_start_ns,rows_bad_speed,rows_bad_velocity_component,coast_flagged_speed_rows,not_marked_coast_speed_rows,last_input_speed_unknown_rows,aircraft_class_unknown_rows,update_path_unknown_rows,adapter_rows_in,adapter_passed,adapter_removed_total,adapter_with_velocity,adapter_velocity_default_nacv,adapter_velocity_refused,adapter_with_altitude,adapter_with_vrate,tracker_velocity_gated\n",
    );
    for (hour, counts) in diagnostics.hours.iter().enumerate() {
        let start = diagnostics
            .day_start_ns
            .map(|day| day + hour as i64 * HOUR_NS)
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "hour,{start},{},{},{},{},{},{},{},,,,,,,,,",
            counts.bad_speed_rows,
            counts.nonfinite_component_rows,
            counts.coast_flagged_rows,
            counts.not_marked_coast_rows,
            counts.last_input_speed_unknown_rows,
            counts.aircraft_class_unknown_rows,
            counts.update_path_unknown_rows
        );
    }
    let counts = diagnostics.out_of_anchor_day;
    let _ = writeln!(
        out,
        "out_of_anchor_day,,{},{},{},{},{},{},{},,,,,,,,,",
        counts.bad_speed_rows,
        counts.nonfinite_component_rows,
        counts.coast_flagged_rows,
        counts.not_marked_coast_rows,
        counts.last_input_speed_unknown_rows,
        counts.aircraft_class_unknown_rows,
        counts.update_path_unknown_rows
    );
    let adapter = diagnostics.adapter.unwrap_or_default();
    let removed_total = adapter.removed.iter().copied().sum::<u64>();
    let velocity_gated = diagnostics
        .tracker_velocity_gated
        .map_or(String::new(), |n| n.to_string());
    let _ = writeln!(
        out,
        "run_total,,,,,,,,,{},{},{},{},{},{},{},{},{}",
        adapter.rows_in,
        adapter.passed,
        removed_total,
        adapter.with_velocity,
        adapter.velocity_default_nacv,
        adapter.velocity_refused,
        adapter.with_altitude,
        adapter.with_vrate,
        velocity_gated
    );
    out
}

#[cfg(test)]
mod speed_diagnostic_tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use oa_ca::{ca_cycle_with_inhibits, Area as CaArea, Volume, MAX_VOLUMES};

    fn track(addr: u32, t: f32, n: f32, flags: u8) -> CaTrack {
        CaTrack {
            lat_deg: 0.0,
            lon_deg: 0.0,
            addr,
            vel_east_mps: t,
            vel_north_mps: n,
            r95_m: 0.0,
            alt_ft: f32::NAN,
            vrate_fpm: f32::NAN,
            flags,
            ground: 0,
        }
    }

    // REQ: OA-CA-015
    #[test]
    fn speed_diagnostic_conserves_rejected_rows_and_never_emits_track_identity() {
        let mut d = SpeedDiagnostics::new();
        let day_ns = 19_000 * DAY_NS;
        let rows = [
            track(0x1234_abcd, 701.0, 0.0, F_COAST),
            track(0x5678_abcd, 0.0, 800.0, 0),
            track(0x9abc_def0, f32::INFINITY, 0.0, 0),
            track(0xfedc_ba98, 300.0, 0.0, 0),
        ];
        let volume = Volume {
            area: CaArea::Box {
                lat_min_e7: -90_000_000,
                lat_max_e7: 90_000_000,
                lon_min_e7: -180_000_000,
                lon_max_e7: 180_000_000,
            },
            alt_lo_ft: -100_000,
            alt_hi_ft: 100_000,
            h_m: 5_000.0,
            v_ft: 300.0,
        };
        let config = oa_ca::CaConfig {
            max_tracks: 8,
            max_open: 8,
            max_candidates: 32,
            look_ahead_ns: 10 * 1_000_000_000,
            hysteresis_ns: 1_000_000_000,
            work_budget: 1_000,
            eval_budget: 100,
            max_speed_mps: 700.0,
            r95_cap_m: 20_000.0,
            grid_band_m: 10_000.0,
            default_volume: volume,
            volumes: [volume; MAX_VOLUMES],
            n_volumes: 0,
        };
        let mut state = ca_new(&config).expect("synthetic CA state");
        let mut alerts = alert_log_new(8).expect("synthetic alert log");
        let mut events = event_log_new(8).expect("synthetic event log");
        let mut net =
            state_table_new(30_000_000_000, 5_000_000_000).expect("synthetic network state");
        let mut inhibits = inhibits_new();
        let t = day_ns + 23 * HOUR_NS;
        let out = ca_cycle_with_inhibits(
            &mut state,
            t,
            &rows,
            &mut alerts,
            &mut events,
            &mut net,
            &mut inhibits,
        );
        d.add_cycle(t, &rows, config.max_speed_mps);
        assert_eq!(d.hours[23].bad_speed_rows, out.stats.rows_bad_speed);
        assert_eq!(
            d.hours[23].nonfinite_component_rows,
            out.stats.rows_bad_velocity_component
        );
        assert_eq!(
            (
                out.stats.rows_bad_speed,
                out.stats.rows_bad_velocity_component
            ),
            (2, 1)
        );

        let next_day_rows = [track(0x7654_3210, 900.0, 0.0, F_COAST)];
        let next_t = day_ns + DAY_NS;
        let next = ca_cycle_with_inhibits(
            &mut state,
            next_t,
            &next_day_rows,
            &mut alerts,
            &mut events,
            &mut net,
            &mut inhibits,
        );
        d.add_cycle(next_t, &next_day_rows, config.max_speed_mps);
        assert_eq!(
            d.out_of_anchor_day.bad_speed_rows,
            next.stats.rows_bad_speed
        );

        let hour = d.hours[23];
        assert_eq!(hour.bad_speed_rows, out.stats.rows_bad_speed);
        assert_eq!(hour.nonfinite_component_rows, 1);
        assert_eq!(hour.coast_flagged_rows, 1);
        assert_eq!(hour.not_marked_coast_rows, 1);
        assert_eq!(hour.last_input_speed_unknown_rows, hour.bad_speed_rows);
        assert_eq!(hour.aircraft_class_unknown_rows, hour.bad_speed_rows);
        assert_eq!(hour.update_path_unknown_rows, hour.bad_speed_rows);
        assert_eq!(d.out_of_anchor_day.bad_speed_rows, 1);
        assert_eq!(d.out_of_anchor_day.last_input_speed_unknown_rows, 1);
        assert_eq!(d.out_of_anchor_day.aircraft_class_unknown_rows, 1);
        assert_eq!(d.out_of_anchor_day.update_path_unknown_rows, 1);
        let adapter = AdsbStats {
            rows_in: 5,
            passed: 4,
            removed: [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            velocity_refused: 2,
            ..AdsbStats::default()
        };
        d.adapter = Some(adapter);
        d.tracker_velocity_gated = Some(7);

        let csv = speed_diagnostics_csv(&d);
        assert_eq!(csv.lines().count(), SPEED_DIAGNOSTIC_HOURS + 3);
        let header_columns = csv
            .lines()
            .next()
            .expect("diagnostic header")
            .split(',')
            .count();
        assert!(csv
            .lines()
            .skip(1)
            .all(|line| line.split(',').count() == header_columns));
        assert!(!csv.contains("1234ABCD"));
        assert!(!csv.contains("5678ABCD"));
        assert!(csv.contains("out_of_anchor_day,,1,0,1,0,1,1,1,"));
        assert!(csv.contains("run_total,,,,,,,,,5,4,1,0,0,2,0,0,7\n"));
    }
}

struct Feed<'a> {
    st: CaState,
    alerts: AlertLog,
    events: EventLog,
    net: StateTable,
    inh: Box<InhibitTable>,
    batch: Vec<CaTrack>,
    cur: Option<i64>,
    ground: &'a GroundTable,
    all: Vec<Alert>,
    all_events: Vec<AltLost>,
    net_cycles: BTreeMap<String, u64>,
    tick_rows: u64,
    rows_skipped: u64,
    cycles_both: u64,
    last_overflow: u64,
    tracker_capacity_refused: u64,
    capacity_refused_seen: u64,
    capacity_refused_pending: u64,
    capacity_refused_pending_at_ns: Option<i64>,
    last_t: i64,
    speed_diagnostics: Option<SpeedDiagnostics>,
    max_speed_mps: f64,
}

fn flush(f: &mut Feed) {
    if let Some(t) = f.cur.take() {
        if f.capacity_refused_pending_at_ns.is_some_and(|at| at <= t) {
            let count = std::mem::take(&mut f.capacity_refused_pending);
            f.capacity_refused_pending_at_ns = None;
            // Capacity refusal is detected by the tracker call, not reported per held input.
            // Charge it to the first cycle at or after that detection time.
            ca_note_lost_input(&mut f.st, count);
        }
        if let Some(diagnostics) = &mut f.speed_diagnostics {
            diagnostics.add_cycle(t, &f.batch, f.max_speed_mps);
        }
        let out = ca_cycle_with_inhibits(
            &mut f.st,
            t,
            &f.batch,
            &mut f.alerts,
            &mut f.events,
            &mut f.net,
            &mut f.inh,
        );
        f.last_t = t;
        if out.work_short > 0 && out.input_short > 0 {
            f.cycles_both += 1;
        }
        f.batch.clear();
        f.all.extend_from_slice(&f.alerts.items);
        alert_log_clear(&mut f.alerts);
        f.all_events.extend_from_slice(&f.events.items);
        event_log_clear(&mut f.events);
        let s = net_status(&f.net, &f.inh, NetId::ConflictAlert, t);
        *f.net_cycles.entry(state_text(&s.state)).or_default() += 1;
    }
}

/// The core's flag byte of the tracker's. The tracker's one identity flag is the radar-handle
/// key of a track with no ADS-B address, so it sets both radar-only and identity-unconfirmed.
fn flags_of(o: u8) -> u8 {
    let mut f = 0;
    if o & O_COAST != 0 {
        f |= F_COAST;
    }
    if o & O_UNCERTAIN != 0 {
        f |= F_UNCERTAIN;
    }
    if o & O_IDENTITY_UNCONFIRMED != 0 {
        f |= F_RADAR_ONLY | F_IDENTITY_UNCONFIRMED;
    }
    f
}

/// Takes the one-second rows (every live track at a tick, one time after another) of a drained
/// table; a change of time ends the cycle.
fn consume(f: &mut Feed, o: &TrackOutput) {
    // Rows the tracker's output table dropped for want of room since the last look were never
    // seen by conflict alert: the next cycle must not read as running.
    ca_note_lost_input(&mut f.st, o.overflow.saturating_sub(f.last_overflow));
    f.last_overflow = o.overflow;
    for k in 0..output_len(o) {
        let (
            Some(&fl),
            Some(&t),
            Some(&addr),
            Some(&la),
            Some(&lo),
            Some(&alt),
            Some(&vr),
            Some(&ve),
            Some(&vn),
            Some(&cov),
        ) = (
            o.flags.get(k),
            o.t_ns.get(k),
            o.addr.get(k),
            o.lat_e7.get(k),
            o.lon_e7.get(k),
            o.alt_ft.get(k),
            o.vrate_fpm.get(k),
            o.vel_east_mps.get(k),
            o.vel_north_mps.get(k),
            o.pos_cov_m2.get(k),
        )
        else {
            // A row with a column missing cannot be read: counted, and told to the core.
            f.rows_skipped += 1;
            ca_note_lost_input(&mut f.st, 1);
            continue;
        };
        if fl & O_TICK == 0 {
            continue;
        }
        if f.cur != Some(t) {
            flush(f);
            f.cur = Some(t);
        }
        f.tick_rows += 1;
        f.batch.push(CaTrack {
            lat_deg: f64::from(la) / 1e7,
            lon_deg: f64::from(lo) / 1e7,
            addr,
            vel_east_mps: ve,
            vel_north_mps: vn,
            r95_m: r95_of(cov),
            alt_ft: if fl & O_HAS_ALT != 0 { alt } else { f32::NAN },
            vrate_fpm: if fl & O_HAS_ALT != 0 { vr } else { f32::NAN },
            flags: flags_of(fl),
            ground: ground_at(f.ground, addr, t),
        });
    }
}

/// Runs the tracker over `meas` (in time order) as the scoring run does and gives its one-second
/// rows to conflict alert, one cycle per tick.
pub fn track_and_alert(
    meas: &[Measurement],
    cfg: &TrackerConfig,
    ground: &GroundTable,
    set: &Settings,
) -> Result<Record, String> {
    track_and_alert_with_diagnostics(meas, cfg, ground, set, false, None)
}

fn track_and_alert_with_diagnostics(
    meas: &[Measurement],
    cfg: &TrackerConfig,
    ground: &GroundTable,
    set: &Settings,
    enable_speed_diagnostics: bool,
    adapter_stats: Option<AdsbStats>,
) -> Result<Record, String> {
    ca_check(&set.ca).map_err(|e| format!("conflict alert configuration: {e}"))?;
    let mut t = tracker_new(cfg).map_err(|e| e.to_string())?;
    let mut out = output_new(cfg.out_capacity);
    let mut f = Feed {
        st: ca_new(&set.ca)?,
        alerts: alert_log_new(set.alert_log_capacity)?,
        events: event_log_new(set.event_log_capacity)?,
        net: state_table_new(set.net_stale_limit_ns, set.net_future_horizon_ns)
            .map_err(|e| format!("net state table: {e:?}"))?,
        inh: Box::new(inhibits_new()),
        batch: Vec::new(),
        cur: None,
        ground,
        all: Vec::new(),
        all_events: Vec::new(),
        net_cycles: BTreeMap::new(),
        tick_rows: 0,
        rows_skipped: 0,
        cycles_both: 0,
        last_overflow: 0,
        tracker_capacity_refused: 0,
        capacity_refused_seen: 0,
        capacity_refused_pending: 0,
        capacity_refused_pending_at_ns: None,
        last_t: 0,
        speed_diagnostics: enable_speed_diagnostics.then(SpeedDiagnostics::new),
        max_speed_mps: set.ca.max_speed_mps,
    };
    let drain = cfg.out_capacity.min(1 << 16) / 2;
    for m in meas {
        tracker_update(&mut t, m, &mut out);
        let refused = tracker_stats(&t).capacity_refused;
        let newly_refused = refused.saturating_sub(f.capacity_refused_seen);
        if newly_refused > 0 {
            // A sorted offline update can apply held earlier measurements before this call's
            // current measurement. The tracker exposes only a cumulative refusal count, so this
            // call time is the earliest defensible detection time. Drain and close earlier
            // cycles before adding this loss; those cycles were already known and stay unchanged.
            consume(&mut f, &out);
            output_clear(&mut out);
            flush(&mut f);
            f.capacity_refused_seen = refused;
            f.tracker_capacity_refused = f.tracker_capacity_refused.saturating_add(newly_refused);
            f.capacity_refused_pending = f.capacity_refused_pending.saturating_add(newly_refused);
            f.capacity_refused_pending_at_ns = Some(
                f.capacity_refused_pending_at_ns
                    .map_or(m.toa_ns, |at| at.max(m.toa_ns)),
            );
        }
        if output_len(&out) > drain {
            consume(&mut f, &out);
            output_clear(&mut out);
        }
    }
    let end = meas.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut t, end, &mut out);
    tracker_flush(&mut t, end, &mut out);
    consume(&mut f, &out);
    flush(&mut f);
    ca_finish(&mut f.st, &mut f.alerts, &mut f.net);
    f.all.extend_from_slice(&f.alerts.items);
    let mut final_t = f.last_t;
    if let Some(last_fault_ns) = f.capacity_refused_pending_at_ns {
        // No tracker tick followed the final refusal. Report the run-level shortfall at its
        // detection time so the final status does not remain RUNNING.
        final_t = final_t.max(last_fault_ns);
        let _ = state_report(
            &mut f.net,
            NetId::ConflictAlert,
            NetReport::Degraded {
                reason: Reason::InputMissing,
                area: Area::Whole,
                count: u32::try_from(f.capacity_refused_pending).unwrap_or(u32::MAX),
            },
            last_fault_ns,
            last_fault_ns,
        );
    }
    let final_net = state_text(&net_status(&f.net, &f.inh, NetId::ConflictAlert, final_t).state);
    let mut speed_diagnostics = f.speed_diagnostics;
    if let Some(diagnostics) = &mut speed_diagnostics {
        diagnostics.adapter = adapter_stats;
        diagnostics.tracker_velocity_gated = Some(tracker_stats(&t).velocity_gated);
    }
    Ok(Record {
        alerts: f.all,
        events: f.all_events,
        totals: ca_totals(&f.st),
        net_cycles: f.net_cycles,
        log_overflow: f.alerts.overflow,
        event_overflow: f.events.overflow,
        tracker_overflow: out.overflow,
        tracker_capacity_refused: f.tracker_capacity_refused,
        tick_rows: f.tick_rows,
        rows_skipped: f.rows_skipped,
        cycles_both: f.cycles_both,
        final_net,
        speed_diagnostics,
    })
}

/// The whole run: reads the clip and the configuration, writes `alerts.csv`,
/// `altitude_lost.csv` and `summary.txt` under `args.out`, and returns the summary text.
pub fn run(args: &Args) -> Result<String, String> {
    run_counted(args, &mut 0)
}

/// [`run`], also giving `sealed` the count of clip rows left out for a sealed window (set as soon
/// as the frames are read, so an error after that, or in reading the rest, still carries it).
pub fn run_counted(args: &Args, sealed: &mut u64) -> Result<String, String> {
    run_counted_with(args, &file_opener(), sealed)
}

/// Applies an independently pinned issued preset before `ca_check` and detector setup.
fn load_adaptation_preset(
    set: &mut Settings,
    root: &Path,
    pin: oa_sha256::Digest,
) -> Result<(), String> {
    refuse_sealed(Sealed::production(), root)?;
    let package = oa_adapt::manifest::verify_current(root, pin)
        .map_err(|e| format!("adaptation package refused: {e}"))?;
    let body = package
        .ca_preset_bytes()
        .ok_or("adaptation package has no v3 CA preset component")?;
    let facility = package.identity().facility.as_str();
    let preset =
        oa_ca::ca_preset_parse(body, facility).map_err(|e| format!("CA preset refused: {e}"))?;
    if oa_ca::ca_preset_facility(&preset) != facility {
        return Err("CA preset facility identity mismatch".into());
    }
    oa_ca::ca_preset_apply(&mut set.ca, &preset);
    set.max_alert_suppression_ns = preset.max_alert_suppression_ns;
    set.allow_military_group_suppression = preset.allow_military_group_suppression;
    set.adaptation_manifest_sha256 = Some(to_hex(&package.identity().manifest_sha256));
    set.adaptation_version = package.identity().adaptation_version;
    Ok(())
}

/// The names of the output files, the summary last: it is the mark of a complete run.
const OUTPUTS: [&str; 3] = ["alerts.csv", "altitude_lost.csv", "summary.txt"];
const DIAGNOSTIC_OUTPUT: &str = "speed_diagnostics.csv";

/// Removes the output files of an earlier run, each path checked by the guard first, so that a
/// run that fails never leaves files that look like its own result.
fn remove_stale_outputs(out: &Path) -> Result<(), String> {
    for name in OUTPUTS.into_iter().chain([DIAGNOSTIC_OUTPUT]) {
        let p = out.join(name);
        refuse_sealed(Sealed::production(), &p)?;
        match fs::remove_file(&p) {
            Ok(()) => {}
            // No such file, or no such directory (the run then fails on creating it, with its
            // own message, after the clip is read).
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(e) => return Err(format!("{}: {e}", p.display())),
        }
    }
    Ok(())
}

/// Writes `text` to `dir/name` whole or not at all: to a temporary name in the same directory,
/// flushed to disk, then renamed over the final name.
fn write_atomic(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    let tmp = dir.join(format!(".{name}.tmp"));
    let fin = dir.join(name);
    let written = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, &fin)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(format!("{}: {e}", fin.display()));
    }
    Ok(())
}

/// [`run_counted`] with the clip opened by `open` (a test hands it a clip that fails).
pub fn run_counted_with(args: &Args, open: &Opener, sealed: &mut u64) -> Result<String, String> {
    // Every path is checked before anything is opened or created, for a caller of the library too.
    for p in [&args.adsb, &args.config, &args.out] {
        refuse_sealed(Sealed::production(), p)?;
    }
    remove_stale_outputs(&args.out)?;
    let mut cfg_bytes = Vec::new();
    open_guarded(Sealed::production(), &args.config)
        .map_err(|e| match e {
            OpenFail::Refused(m) | OpenFail::Io(m) => m,
        })?
        .read_to_end(&mut cfg_bytes)
        .map_err(|e| format!("{}: {e}", args.config.display()))?;
    let cfg_text = String::from_utf8(cfg_bytes.clone())
        .map_err(|_| format!("{}: not UTF-8 text", args.config.display()))?;
    let mut set = crate::config::parse_config_for_preset(&cfg_text)
        .map_err(|e| format!("{}: {e}", args.config.display()))?;
    if let (Some(root), Some(pin)) = (&args.adaptation_package, args.adaptation_pin) {
        load_adaptation_preset(&mut set, root, pin)?;
    }
    ca_check(&set.ca).map_err(|e| format!("configuration refused by conflict alert: {e}"))?;
    // The count goes straight into `sealed`, so a clip that fails after rows were dropped still
    // reports them.
    let clip = load_adsb_counted(&args.adsb, open, sealed).map_err(|e| e.to_string())?;
    let clip_sha = clip.hash.sha256.clone();
    let (reports, scope) = if args.scorer_half {
        let (a, b, _) = split_reports(&clip.reports);
        oa_bench::split_check::check_split(&a, &b)
            .map_err(|v| format!("the split of the clip is not valid, run refused: {v:?}"))?;
        (
            a,
            "the scorer's half A of the clip (as oa-track-run)".to_string(),
        )
    } else {
        (clip.reports, "every report of the clip".to_string())
    };
    let adsb_cfg = AdsbConfig::default();
    check_adsb_config(&adsb_cfg).map_err(|e| format!("adapter configuration: {e}"))?;
    let ground = ground_table(&reports);
    let (meas, adapter_stats) = adsb_measurements(&reports, &adsb_cfg);
    drop(reports);
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    let rec = track_and_alert_with_diagnostics(
        &meas,
        &cfg,
        &ground,
        &set,
        args.speed_diagnostics,
        args.speed_diagnostics.then_some(adapter_stats),
    )?;
    fs::create_dir_all(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    let csv = alerts_csv(&rec.alerts);
    let csv_sha = to_hex(&sha256(csv.as_bytes()));
    let name_of = |p: &std::path::Path| {
        p.file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    };
    let summary = summary_text(&SummaryInput {
        cfg: &set.ca,
        alerts: &rec.alerts,
        events: &rec.events,
        totals: rec.totals,
        net_cycles: &rec.net_cycles,
        log_overflow: rec.log_overflow,
        event_overflow: rec.event_overflow,
        tracker_overflow: rec.tracker_overflow,
        tracker_capacity_refused: rec.tracker_capacity_refused,
        measurements: meas.len() as u64,
        tick_rows: rec.tick_rows,
        rows_skipped: rec.rows_skipped,
        cycles_both: rec.cycles_both,
        final_net: &rec.final_net,
        clip_name: &name_of(&args.adsb),
        clip_sha256: &clip_sha,
        sealed_window_dropped: *sealed,
        config_name: &name_of(&args.config),
        config_sha256: &to_hex(&sha256(&cfg_bytes)),
        scope: &scope,
        alerts_sha256: &csv_sha,
        adaptation_manifest_sha256: set.adaptation_manifest_sha256.as_deref(),
        adaptation_version: set.adaptation_version,
    });
    let events = events_csv(&rec.events);
    for (file, text) in OUTPUTS.into_iter().zip([&csv, &events, &summary]) {
        write_atomic(&args.out, file, text)?;
    }
    if let Some(diagnostics) = &rec.speed_diagnostics {
        write_atomic(
            &args.out,
            DIAGNOSTIC_OUTPUT,
            &speed_diagnostics_csv(diagnostics),
        )?;
    }
    // The renames reach the disk with the directory.
    // seal-allow: opens the output directory, which this run wrote, to sync the renames
    fs::File::open(&args.out)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("{}: {e}", args.out.display()))?;
    Ok(summary)
}

#[cfg(test)]
mod adaptation_preset_tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use oa_airspace::model::{empty_model, Meta};
    use oa_airspace::snapshot::encode;
    use oa_ca::{
        alert_log_new, ca_check, ca_cycle_with_inhibits, ca_new, ca_open_len, event_log_new,
        AlertLog, CaTrack, EventLog, AIRBORNE,
    };
    use oa_safety::{inhibit::inhibits_new, net::state_table_new};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn temp_root(label: &str) -> std::path::PathBuf {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("oa-ca-a3-{label}-{}-{serial}", std::process::id()))
    }

    fn open_count(cfg: &oa_ca::CaConfig, midpoint_lat_deg: f64) -> usize {
        let mut state = ca_new(cfg).expect("synthetic CA state");
        let mut alerts: AlertLog = alert_log_new(8).expect("synthetic alert log");
        let mut events: EventLog = event_log_new(8).expect("synthetic event log");
        let mut net = state_table_new(30_000_000_000, 5_000_000_000).expect("synthetic net table");
        let mut inhibits = inhibits_new();
        let half_separation_deg = 0.0315;
        let rows = [
            CaTrack {
                lat_deg: midpoint_lat_deg - half_separation_deg,
                lon_deg: -88.0,
                addr: 0x00ab_0101,
                vel_east_mps: 0.0,
                vel_north_mps: 0.0,
                r95_m: 0.0,
                alt_ft: 10_000.0,
                vrate_fpm: 0.0,
                flags: 0,
                ground: AIRBORNE,
            },
            CaTrack {
                lat_deg: midpoint_lat_deg + half_separation_deg,
                lon_deg: -88.0,
                addr: 0x00ab_0102,
                vel_east_mps: 0.0,
                vel_north_mps: 0.0,
                r95_m: 0.0,
                alt_ft: 10_000.0,
                vrate_fpm: 0.0,
                flags: 0,
                ground: AIRBORNE,
            },
        ];
        ca_cycle_with_inhibits(
            &mut state,
            0,
            &rows,
            &mut alerts,
            &mut events,
            &mut net,
            &mut inhibits,
        );
        ca_open_len(&state)
    }

    // REQ: OA-CA-036
    #[test]
    fn verified_issued_preset_loads_into_runtime_settings_before_ca_cycle() {
        // Encode the synthetic baseline with the current snapshot writer so package readback
        // exercises the published version-5 shape instead of a stale hand-pinned version-3 file.
        let snapshot = encode(&empty_model(Meta {
            nasr_effective: 20_261_029,
            cifp_cycle: 2611,
            artcc: "ZAU".to_owned(),
            buffer_m: 92_600,
            nasr_sha256: [0; 32],
            cifp_sha256: [0; 32],
        }))
        .expect("synthetic schema snapshot encodes");
        let preset = b"OACA-PRESET-1\nfacility=ZAU\ndefault=box,-900000000,900000000,-1800000000,1800000000,-1000,60000,9260,1000\nmax_alert_suppression_ns=60000000000\nallow_military_group_suppression=true\nvolume=box,414000000,416000000,-881000000,-879000000,0,40000,5556,1000\n";
        let previous_root = temp_root("previous");
        let package_root = temp_root("issued");
        let previous_pin = oa_adapt::manifest::write_new_package(
            &previous_root,
            &snapshot,
            "synthetic-base",
            "ZAU",
            "A3-FIXTURE",
        )
        .expect("synthetic schema snapshot validates as baseline");
        let (_, pin) = oa_adapt::manifest::issue_new_package_with_ca_preset(
            &package_root,
            &snapshot,
            preset,
            "synthetic-v3",
            "ZAU",
            "A3-PRESET",
            &previous_root,
            previous_pin,
            oa_adapt::manifest::ChangeClass::Initial,
            "issuer-a",
            "approver-b",
            "synthetic preset loader regression",
        )
        .expect("v3 fixture package issues and readback verifies");

        let cfg_text = include_str!("../testdata/dev.cfg");
        let mut settings = crate::config::parse_config_for_preset(cfg_text)
            .expect("development runtime settings parse before preset load");
        load_adaptation_preset(&mut settings, &package_root, pin)
            .expect("runtime verifies the package and loads exact preset bytes");
        ca_check(&settings.ca).expect("preset minima are checked before detector setup");
        assert_eq!(settings.adaptation_version, Some(1));
        let expected_pin = oa_sha256::to_hex(&pin);
        assert_eq!(
            settings.adaptation_manifest_sha256.as_deref(),
            Some(expected_pin.as_str())
        );
        assert_eq!(settings.ca.n_volumes, 1);
        assert_eq!(settings.ca.volumes[0].h_m, 5556.0);
        assert_eq!(settings.max_alert_suppression_ns, Some(60_000_000_000));
        assert!(settings.allow_military_group_suppression);

        // The same 7km pair is outside the issued 3NM local volume but inside the 5NM default.
        // At latitude 41.5 the local preset must leave the detector quiet; outside its area the
        // configured default must produce the alert through the normal cycle API.
        assert_eq!(open_count(&settings.ca, 41.5), 0);
        assert_eq!(open_count(&settings.ca, 42.0), 1);
        let _ = std::fs::remove_dir_all(previous_root);
        let _ = std::fs::remove_dir_all(package_root);
    }
}

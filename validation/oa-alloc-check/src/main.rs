#![deny(unsafe_code)]
//! Standalone allocation validation. Shell (B3): fixtures, execution and reports.

mod allocator;
mod display_window;
mod nam_window;
mod pending_windows;
use display_window::{display_window, DisplayEvidence};
use nam_window::{nam_session_window, NamEvidence};
use pending_windows::{adsb_pending_window, plot_pending_window, PendingWindowEvidence};

use allocator::{control_alloc, control_realloc, control_zeroed, Counts};
use oa_track::fusion::{
    fusion_adsb, fusion_advance, fusion_config, fusion_finish, fusion_new, fusion_plots,
    fusion_registration, fusion_stats, fusion_tracker_config, fusion_tracker_stats,
};
use oa_track::fusion_params::MAX_BUCKET_PLOTS;
use oa_track::params::RING_DEPTH;
use oa_track::radar::{plot_convert, radar_geom, RadarPlot, RadarSite, NONE_CODE, NO_BIAS};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_counts, tracker_flush,
    tracker_log, tracker_new, tracker_predict, tracker_stats, tracker_update, Measurement, OutRow,
    TrackOutput, Tracker, TrackerConfig, DEFAULT_CONFIG, M_HAS_VEL, NONE_I32, NO_ADDR, NO_IDENT,
    O_COAST, RADAR_KEY_BASE,
};

const S: i64 = 1_000_000_000;
const ADDRESS: u32 = 0xA10001;
const REFERENCE_COUNT: usize = 4;
// Three prior ADS-B updates and five eligible residuals require eight cold scans.
const REFERENCE_SCANS: usize = 8;
const REFERENCE_PLOTS: usize = REFERENCE_COUNT * REFERENCE_SCANS;
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
const REFERENCE_AZIMUTHS: [f64; REFERENCE_COUNT] = [10.0, 100.0, 190.0, 280.0];

// A7.2: cold fixture report, one per executable run.
struct TrackerEvidence {
    counts: Counts,
    applied: u64,
    refiltered: u64,
    refused: u64,
    capacity_refused: u64,
    full_capacity: usize,
    recovery_applied: u64,
    rows: usize,
    predictions: u64,
    advance_rows: usize,
    flush_rows: usize,
    ended_total: u64,
    injected: bool,
}

// A7.2: cold fixture report, one per executable run.
struct FusionEvidence {
    counts: Counts,
    plots: u64,
    hard_applied: u64,
    candidates: u64,
    refused: u64,
    rows: usize,
    local_rows: usize,
    quiet_advance_rows: usize,
    coast_rows: usize,
    radar_tracks: usize,
    registration_used: u64,
    registration_history: usize,
    initiation_visits: u64,
    initiation_tests: u64,
    anon_applied: u64,
    injected: bool,
}

struct DefaultCapacityEvidence {
    counts: Counts,
    occupied: usize,
    confirmed: usize,
    refused: u64,
    recovered: u64,
    history_refiltered: u64,
    history_replayed: u64,
    late_ring: u64,
    injected: bool,
}

struct OutputLogEvidence {
    counts: Counts,
    output_overflow: u64,
    output_rows_after_reuse: usize,
    output_aligned: bool,
    output_preserved_addr: bool,
    output_payload_unchanged: bool,
    log_overflow: u64,
    log_rows: usize,
    log_aligned: bool,
    injected: bool,
}

struct ClockEvidence {
    counts: Counts,
    held: u64,
    held_applied: u64,
    step_forward: u64,
    late: u64,
    step_back: u64,
    future_dropped: u64,
    recovered: u64,
    injected: bool,
}

struct BucketCapacityEvidence {
    counts: Counts,
    plots_offered: u64,
    bucket_plot_overflow: u64,
    known_hard_applied: u64,
    unmatched_hard_no_track: u64,
    recovery_applied: u64,
    injected: bool,
}

// A7.2: cold aggregate used only after every measurement window has disarmed.
struct AllocationEvidence<'a> {
    controls: Counts,
    tracker: &'a TrackerEvidence,
    default_capacity: &'a DefaultCapacityEvidence,
    output_log: &'a OutputLogEvidence,
    clock_gate: &'a ClockEvidence,
    bucket_capacity: &'a BucketCapacityEvidence,
    adsb_pending: &'a PendingWindowEvidence,
    plot_pending: &'a PendingWindowEvidence,
    fusion: &'a FusionEvidence,
    nam_session: &'a NamEvidence,
    display_frame: &'a DisplayEvidence,
}

fn emit_evidence(
    status: &'static str,
    injection_requested: bool,
    evidence: &AllocationEvidence<'_>,
) {
    let AllocationEvidence {
        controls,
        tracker,
        default_capacity,
        output_log,
        clock_gate,
        bucket_capacity,
        adsb_pending,
        plot_pending,
        fusion,
        nam_session,
        display_frame,
    } = evidence;
    println!(
        "{{\"status\":\"{}\",\"injection_requested\":{},\"fixture_ids\":[\"fusion_adsb_hard_assoc_registration\",\"fusion_adsb_pending_queue_capacity_recovery\",\"fusion_anonymous_init_and_assoc\",\"fusion_bucket_plot_cap_refusal_recovery\",\"fusion_plot_pending_queue_capacity_recovery\",\"display_rows_traffic_frame_refusal\",\"nam_session_activate_traffic_retry_full_deactivate\",\"tracker_clock_hold_stepback_recovery\",\"tracker_default_4096_capacity_recovery\",\"tracker_output_log_overflow_reuse\",\"tracker_update_ooo_refusal_capacity_advance_flush\"],\"route_ids\":[\"display_altitude_marker\",\"display_callsign_conflict\",\"display_frame_chain\",\"display_frame_refusal\",\"display_malformed_target\",\"display_map_scene\",\"display_rows_to_targets\",\"display_set_refusal\",\"display_track_end\",\"display_traffic_draw\",\"display_uncertain_radius\",\"fusion_adsb_ingest\",\"fusion_adsb_pending_drain\",\"fusion_adsb_pending_overflow\",\"fusion_adsb_pending_recovery\",\"fusion_advance\",\"fusion_anonymous_initiation\",\"fusion_bucket_capacity_recovery\",\"fusion_bucket_known_match\",\"fusion_bucket_plot_cap\",\"fusion_bucket_unknown_identity\",\"fusion_finish\",\"fusion_local_association\",\"fusion_plot_ingest\",\"fusion_plot_pending_bucket_cap\",\"fusion_plot_pending_overflow\",\"fusion_plot_pending_recovery\",\"fusion_registration\",\"nam_session_abandon\",\"nam_session_activation\",\"nam_session_deactivation\",\"nam_session_deliver\",\"nam_session_discard\",\"nam_session_keepalive\",\"nam_session_pending_full\",\"nam_session_peer_silent\",\"nam_session_resend\",\"nam_session_response\",\"nam_session_timeout\",\"nam_session_traffic\",\"tracker_advance\",\"tracker_capacity_recovery\",\"tracker_capacity_refusal\",\"tracker_clock_backstep\",\"tracker_clock_forward_recovery\",\"tracker_clock_hold\",\"tracker_clock_recovery\",\"tracker_default_capacity\",\"tracker_default_confirmed_capacity\",\"tracker_flush\",\"tracker_history_retention_refusal\",\"tracker_history_wrap_refilter\",\"tracker_history_wrap_replay\",\"tracker_invalid_input_refusal\",\"tracker_log_overflow\",\"tracker_out_of_order_refilter\",\"tracker_output_overflow\",\"tracker_output_payload_stability\",\"tracker_output_reuse\",\"tracker_prediction\",\"tracker_update\"],\"controls\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{}}},\"tracker\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{},\"applied\":{},\"refiltered\":{},\"refused\":{},\"capacity_refused\":{},\"full_capacity\":{},\"recovery_applied\":{},\"rows\":{},\"predictions\":{},\"advance_rows\":{},\"flush_rows\":{},\"ended\":{}}},\"default_capacity\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{},\"occupied\":{},\"confirmed\":{},\"refused\":{},\"recovered\":{},\"history_refiltered\":{},\"history_replayed\":{},\"late_ring\":{}}},\"output_log\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{},\"output_overflow\":{},\"output_rows_after_reuse\":{},\"output_aligned\":{},\"output_preserved_addr\":{},\"output_payload_unchanged\":{},\"log_overflow\":{},\"log_rows\":{},\"log_aligned\":{}}},\"clock_gate\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{},\"held\":{},\"held_applied\":{},\"step_forward\":{},\"late\":{},\"step_back\":{},\"future_dropped\":{},\"recovered\":{}}},\"bucket_capacity\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{},\"plots_offered\":{},\"bucket_plot_overflow\":{},\"known_hard_applied\":{},\"unmatched_hard_no_track\":{},\"recovery_applied\":{}}},\"adsb_pending\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{},\"offered\":{},\"pending_overflow\":{},\"drained\":{},\"bucket_plot_overflow\":{},\"recovery_applied\":{},\"tracker_applied\":{},\"tracker_duplicates\":{},\"tracker_refused\":{},\"tracker_capacity_refused\":{},\"tracker_late_window\":{},\"tracker_late_ring\":{},\"tracker_jump_rejected\":{},\"tracker_numerical_refused\":{},\"hard_extra\":{},\"hard_mismatch\":{}}},\"plot_pending\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{},\"offered\":{},\"pending_overflow\":{},\"drained\":{},\"bucket_plot_overflow\":{},\"recovery_applied\":{},\"tracker_applied\":{},\"tracker_duplicates\":{},\"tracker_refused\":{},\"tracker_capacity_refused\":{},\"tracker_late_window\":{},\"tracker_late_ring\":{},\"tracker_jump_rejected\":{},\"tracker_numerical_refused\":{},\"hard_extra\":{},\"hard_mismatch\":{}}},\"fusion\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{},\"plots\":{},\"hard_applied\":{},\"anon_applied\":{},\"candidates_confirmed\":{},\"init_visits\":{},\"init_tests\":{},\"refused\":{},\"rows\":{},\"local_rows\":{},\"radar_tracks\":{},\"registration_used\":{},\"registration_history\":{},\"quiet_advance_rows\":{},\"coast_rows\":{}}},\"nam_session\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{},\"irq_sent\":{},\"atm_sent\":{},\"resent\":{},\"timed_out\":{},\"delivered\":{},\"responses\":{},\"asm_sent\":{},\"peer_silent\":{},\"lrm_sent\":{},\"trq_sent\":{},\"abandoned\":{},\"peak_pending\":{},\"pending_full\":{},\"discards\":{},\"dropped\":{},\"script_errors\":{},\"final_inactive\":{}}},\"display_frame\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{},\"convert_added\":{},\"convert_ended\":{},\"traffic_drawn\":{},\"uncertain\":{},\"cs_conflict\":{},\"r95_unknown\":{},\"refused\":{},\"alt_clamped\":{},\"malformed\":{},\"frame_drawn\":{},\"scene_items\":{},\"refusing_refused\":{}}}}}",
        status,
        injection_requested,
        controls.overflow,
        controls.alloc,
        controls.zeroed,
        controls.realloc,
        tracker.counts.overflow,
        tracker.counts.alloc,
        tracker.counts.zeroed,
        tracker.counts.realloc,
        tracker.applied,
        tracker.refiltered,
        tracker.refused,
        tracker.capacity_refused,
        tracker.full_capacity,
        tracker.recovery_applied,
        tracker.rows,
        tracker.predictions,
        tracker.advance_rows,
        tracker.flush_rows,
        tracker.ended_total,
        default_capacity.counts.overflow,
        default_capacity.counts.alloc,
        default_capacity.counts.zeroed,
        default_capacity.counts.realloc,
        default_capacity.occupied,
        default_capacity.confirmed,
        default_capacity.refused,
        default_capacity.recovered,
        default_capacity.history_refiltered,
        default_capacity.history_replayed,
        default_capacity.late_ring,
        output_log.counts.overflow,
        output_log.counts.alloc,
        output_log.counts.zeroed,
        output_log.counts.realloc,
        output_log.output_overflow,
        output_log.output_rows_after_reuse,
        output_log.output_aligned,
        output_log.output_preserved_addr,
        output_log.output_payload_unchanged,
        output_log.log_overflow,
        output_log.log_rows,
        output_log.log_aligned,
        clock_gate.counts.overflow,
        clock_gate.counts.alloc,
        clock_gate.counts.zeroed,
        clock_gate.counts.realloc,
        clock_gate.held,
        clock_gate.held_applied,
        clock_gate.step_forward,
        clock_gate.late,
        clock_gate.step_back,
        clock_gate.future_dropped,
        clock_gate.recovered,
        bucket_capacity.counts.overflow,
        bucket_capacity.counts.alloc,
        bucket_capacity.counts.zeroed,
        bucket_capacity.counts.realloc,
        bucket_capacity.plots_offered,
        bucket_capacity.bucket_plot_overflow,
        bucket_capacity.known_hard_applied,
        bucket_capacity.unmatched_hard_no_track,
        bucket_capacity.recovery_applied,
        adsb_pending.counts.overflow,
        adsb_pending.counts.alloc,
        adsb_pending.counts.zeroed,
        adsb_pending.counts.realloc,
        adsb_pending.offered,
        adsb_pending.pending_overflow,
        adsb_pending.drained,
        adsb_pending.bucket_plot_overflow,
        adsb_pending.recovery_applied,
        adsb_pending.tracker_applied,
        adsb_pending.tracker_duplicates,
        adsb_pending.tracker_refused,
        adsb_pending.tracker_capacity_refused,
        adsb_pending.tracker_late_window,
        adsb_pending.tracker_late_ring,
        adsb_pending.tracker_jump_rejected,
        adsb_pending.tracker_numerical_refused,
        adsb_pending.hard_extra,
        adsb_pending.hard_mismatch,
        plot_pending.counts.overflow,
        plot_pending.counts.alloc,
        plot_pending.counts.zeroed,
        plot_pending.counts.realloc,
        plot_pending.offered,
        plot_pending.pending_overflow,
        plot_pending.drained,
        plot_pending.bucket_plot_overflow,
        plot_pending.recovery_applied,
        plot_pending.tracker_applied,
        plot_pending.tracker_duplicates,
        plot_pending.tracker_refused,
        plot_pending.tracker_capacity_refused,
        plot_pending.tracker_late_window,
        plot_pending.tracker_late_ring,
        plot_pending.tracker_jump_rejected,
        plot_pending.tracker_numerical_refused,
        plot_pending.hard_extra,
        plot_pending.hard_mismatch,
        fusion.counts.overflow,
        fusion.counts.alloc,
        fusion.counts.zeroed,
        fusion.counts.realloc,
        fusion.plots,
        fusion.hard_applied,
        fusion.anon_applied,
        fusion.candidates,
        fusion.initiation_visits,
        fusion.initiation_tests,
        fusion.refused,
        fusion.rows,
        fusion.local_rows,
        fusion.radar_tracks,
        fusion.registration_used,
        fusion.registration_history,
        fusion.quiet_advance_rows,
        fusion.coast_rows,
        nam_session.counts.overflow,
        nam_session.counts.alloc,
        nam_session.counts.zeroed,
        nam_session.counts.realloc,
        nam_session.irq_sent,
        nam_session.atm_sent,
        nam_session.resent,
        nam_session.timed_out,
        nam_session.delivered,
        nam_session.responses,
        nam_session.asm_sent,
        nam_session.peer_silent,
        nam_session.lrm_sent,
        nam_session.trq_sent,
        nam_session.abandoned,
        nam_session.peak_pending,
        nam_session.pending_full,
        nam_session.discards,
        nam_session.dropped,
        nam_session.script_errors,
        nam_session.final_inactive,
        display_frame.counts.overflow,
        display_frame.counts.alloc,
        display_frame.counts.zeroed,
        display_frame.counts.realloc,
        display_frame.convert_added,
        display_frame.convert_ended,
        display_frame.traffic_drawn,
        display_frame.uncertain,
        display_frame.cs_conflict,
        display_frame.r95_unknown,
        display_frame.refused,
        display_frame.alt_clamped,
        display_frame.malformed,
        display_frame.frame_drawn,
        display_frame.scene_items,
        display_frame.refusing_refused,
    );
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

fn fail(message: &'static str) -> ! {
    eprintln!("FAIL: {message}");
    std::process::exit(2)
}

fn tracker_config() -> TrackerConfig {
    TrackerConfig {
        max_tracks: 8,
        out_capacity: 32_768,
        log_capacity: 64,
        ..DEFAULT_CONFIG
    }
}

fn measurement(addr: u32, t_ns: i64, lat_e7: i32, lon_e7: i32) -> Measurement {
    Measurement {
        toa_ns: t_ns,
        pos_cov_m2: [25.0, 0.0, 25.0],
        vel_cov_m2s2: [1.0, 0.0, 1.0],
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        lat_e7,
        lon_e7,
        alt_ft: 20_000,
        vrate_fpm: NONE_I32,
        addr,
        alt_quant_ft: 25,
        sensor: 1,
        ident: NO_IDENT,
        flags: M_HAS_VEL,
        category: 0,
    }
}

fn counts_zero(c: Counts) -> bool {
    !c.overflow && c.alloc == 0 && c.zeroed == 0 && c.realloc == 0
}

fn controls_ok(c: Counts) -> bool {
    // Two alloc calls: one is the realloc control's initial global allocation.
    !c.overflow && c.alloc == 2 && c.zeroed == 1 && c.realloc == 1
}

fn run_controls() -> Option<Counts> {
    if !allocator::start() {
        return None;
    }
    let alloc_ok = control_alloc();
    let zeroed_ok = control_zeroed();
    let realloc_ok = control_realloc();
    let counts = allocator::stop()?;
    if alloc_ok && zeroed_ok && realloc_ok && controls_ok(counts) {
        Some(counts)
    } else {
        None
    }
}

fn tracker_window(inject: bool) -> Result<TrackerEvidence, &'static str> {
    let cfg = tracker_config();
    let mut tracker: Tracker = tracker_new(&cfg).map_err(|_| "tracker configuration refused")?;
    let mut out: TrackOutput = output_new(cfg.out_capacity);
    let first = measurement(ADDRESS, 0, 419_000_000, -879_000_000);
    let second = measurement(ADDRESS, S, 419_000_010, -879_000_000);
    let third = measurement(ADDRESS, 2 * S, 419_000_020, -879_000_000);
    let late = measurement(ADDRESS, S + S / 2, 419_000_015, -879_000_000);
    let invalid = Measurement {
        pos_cov_m2: [f32::NAN, 0.0, 1.0],
        toa_ns: 3 * S,
        ..third
    };
    let capacity = measurement(ADDRESS + 1, 3 * S, 419_000_030, -879_000_000);

    if !allocator::start() {
        return Err("tracker allocation window did not arm cleanly");
    }
    tracker_update(&mut tracker, &first, &mut out);
    tracker_update(&mut tracker, &second, &mut out);
    tracker_update(&mut tracker, &third, &mut out);
    tracker_update(&mut tracker, &late, &mut out);
    tracker_update(&mut tracker, &invalid, &mut out);
    for offset in 0..7_u32 {
        let key = ADDRESS + 10 + offset;
        let lat = 420_000_000 + offset as i32 * 10;
        let first = measurement(key, 3 * S, lat, -879_000_000);
        let second = measurement(key, 4 * S, lat + 1, -879_000_000);
        tracker_update(&mut tracker, &first, &mut out);
        tracker_update(&mut tracker, &second, &mut out);
    }
    let (tentative_at_capacity, confirmed_at_capacity) = tracker_counts(&tracker);
    let live_at_capacity = tentative_at_capacity + confirmed_at_capacity;
    tracker_update(&mut tracker, &capacity, &mut out);
    let applied_before_recovery = tracker_stats(&tracker).applied;
    let recovery = measurement(ADDRESS, 5 * S, 419_000_030, -879_000_000);
    tracker_update(&mut tracker, &recovery, &mut out);
    let recovery_applied = tracker_stats(&tracker)
        .applied
        .saturating_sub(applied_before_recovery);
    let predictions = if tracker_predict(&tracker, ADDRESS, 6 * S).is_some() {
        1
    } else {
        0
    };
    let rows_before_advance = output_len(&out);
    tracker_advance(&mut tracker, 6 * S, &mut out);
    let rows_after_advance = output_len(&out);
    let advance_rows = rows_after_advance.saturating_sub(rows_before_advance);
    tracker_flush(&mut tracker, 7 * S, &mut out);
    let rows_after_flush = output_len(&out);
    let flush_rows = rows_after_flush.saturating_sub(rows_after_advance);
    let injected = !inject || allocator::inject_core_allocation();
    let Some(counts) = allocator::stop() else {
        return Err("tracker allocation window did not disarm cleanly");
    };

    let stats = tracker_stats(&tracker);
    let applied = stats.applied;
    let refiltered = stats.refiltered;
    let refused = stats.refused.iter().copied().sum();
    let capacity_refused = stats.capacity_refused;
    let ended_total = stats.ended.iter().copied().sum();
    let rows = output_len(&out);
    Ok(TrackerEvidence {
        counts,
        applied,
        refiltered,
        refused,
        capacity_refused,
        full_capacity: live_at_capacity,
        recovery_applied,
        rows,
        predictions,
        advance_rows,
        flush_rows,
        ended_total,
        injected,
    })
}

fn default_capacity_window(inject: bool) -> Result<DefaultCapacityEvidence, &'static str> {
    let cfg = TrackerConfig {
        max_tracks: DEFAULT_CONFIG.max_tracks,
        out_capacity: 1 << 16,
        log_capacity: DEFAULT_CONFIG.max_tracks,
        ..DEFAULT_CONFIG
    };
    let mut tracker =
        tracker_new(&cfg).map_err(|_| "default-capacity tracker configuration refused")?;
    let mut out = output_new(cfg.out_capacity);
    let mut reports = Vec::with_capacity(DEFAULT_CONFIG.max_tracks * 2);
    for slot in 0..DEFAULT_CONFIG.max_tracks {
        let key = 0xB00000_u32 + slot as u32;
        let lat = 400_000_000 + slot as i32 * 10;
        reports.push(measurement(key, 0, lat, -870_000_000));
        reports.push(measurement(key, S, lat + 1, -870_000_000));
    }
    let overflow = measurement(0xB10000, S, 410_000_000, -870_000_000);
    let recovery = measurement(0xB00000, 2 * S, 400_000_002, -870_000_000);
    let mut history = Vec::with_capacity(RING_DEPTH + 2);
    for ix in 0..RING_DEPTH + 2 {
        let mut report = measurement(
            0xB00000,
            3 * S + ix as i64 * (S / 100),
            400_000_002,
            -870_000_000,
        );
        report.sensor = 1;
        history.push(report);
    }
    let newest_ns = 3 * S + (RING_DEPTH as i64 + 1) * (S / 100);
    let late_between = measurement(0xB00000, newest_ns - S / 200, 400_000_002, -870_000_000);
    let older_than_ring = measurement(0xB00000, 3 * S + S / 100, 400_000_002, -870_000_000);
    if !allocator::start() {
        return Err("default-capacity tracker window did not arm cleanly");
    }
    for report in &reports {
        tracker_update(&mut tracker, report, &mut out);
    }
    let (tentative, confirmed) = tracker_counts(&tracker);
    let occupied = tentative + confirmed;
    tracker_update(&mut tracker, &overflow, &mut out);
    let before = tracker_stats(&tracker).applied;
    tracker_update(&mut tracker, &recovery, &mut out);
    let recovered = tracker_stats(&tracker).applied.saturating_sub(before);
    let before_history = *tracker_stats(&tracker);
    for report in &history {
        tracker_update(&mut tracker, report, &mut out);
    }
    tracker_update(&mut tracker, &late_between, &mut out);
    let after_refilter = tracker_stats(&tracker);
    let history_refiltered = after_refilter
        .refiltered
        .saturating_sub(before_history.refiltered);
    let history_replayed = after_refilter
        .replayed
        .saturating_sub(before_history.replayed);
    let before_ring_refusal = after_refilter.late_ring;
    tracker_update(&mut tracker, &older_than_ring, &mut out);
    let late_ring = tracker_stats(&tracker)
        .late_ring
        .saturating_sub(before_ring_refusal);
    let refused = tracker_stats(&tracker).capacity_refused;
    let injected = !inject || allocator::inject_core_allocation();
    let Some(counts) = allocator::stop() else {
        return Err("default-capacity tracker window did not disarm cleanly");
    };
    Ok(DefaultCapacityEvidence {
        counts,
        occupied,
        confirmed,
        refused,
        recovered,
        history_refiltered,
        history_replayed,
        late_ring,
        injected,
    })
}

fn output_is_aligned(out: &TrackOutput) -> bool {
    let n = output_len(out);
    out.t_ns.len() == n
        && out.addr.len() == n
        && out.lat_e7.len() == n
        && out.lon_e7.len() == n
        && out.vel_east_mps.len() == n
        && out.vel_north_mps.len() == n
        && out.pos_cov_m2.len() == n
        && out.vel_cov_m2s2.len() == n
        && out.turn_rate_rps.len() == n
        && out.alt_ft.len() == n
        && out.alt_var_ft2.len() == n
        && out.vrate_fpm.len() == n
        && out.mu_h.len() == n
        && out.mu_v.len() == n
        && out.flags.len() == n
}

fn first_output(out: &TrackOutput) -> Option<OutRow> {
    Some(OutRow {
        t_ns: *out.t_ns.first()?,
        addr: *out.addr.first()?,
        lat_e7: *out.lat_e7.first()?,
        lon_e7: *out.lon_e7.first()?,
        vel_east_mps: *out.vel_east_mps.first()?,
        vel_north_mps: *out.vel_north_mps.first()?,
        pos_cov_m2: *out.pos_cov_m2.first()?,
        vel_cov_m2s2: *out.vel_cov_m2s2.first()?,
        turn_rate_rps: *out.turn_rate_rps.first()?,
        alt_ft: *out.alt_ft.first()?,
        alt_var_ft2: *out.alt_var_ft2.first()?,
        vrate_fpm: *out.vrate_fpm.first()?,
        mu_h: *out.mu_h.first()?,
        mu_v: *out.mu_v.first()?,
        flags: *out.flags.first()?,
    })
}

fn output_log_window(inject: bool) -> Result<OutputLogEvidence, &'static str> {
    let cfg = TrackerConfig {
        max_tracks: 2,
        out_capacity: 1,
        log_capacity: 1,
        ..DEFAULT_CONFIG
    };
    let mut tracker = tracker_new(&cfg).map_err(|_| "small-table tracker configuration refused")?;
    let mut out = output_new(cfg.out_capacity);
    let reports = [
        measurement(0xC00001, 0, 410_000_000, -870_000_000),
        measurement(0xC00001, S, 410_000_001, -870_000_000),
        measurement(0xC00001, 2 * S, 410_000_002, -870_000_000),
        measurement(0xC00002, 0, 411_000_000, -870_000_000),
        measurement(0xC00002, S, 411_000_001, -870_000_000),
        measurement(0xC00002, 2 * S, 411_000_002, -870_000_000),
        measurement(0xC00001, 3 * S, 410_000_003, -870_000_000),
    ];
    if !allocator::start() {
        return Err("output/log tracker window did not arm cleanly");
    }
    for report in reports.iter().take(3) {
        tracker_update(&mut tracker, report, &mut out);
    }
    let original_payload = first_output(&out).ok_or("first track did not emit an output row")?;
    for report in reports.iter().skip(3).take(3) {
        tracker_update(&mut tracker, report, &mut out);
    }
    let initial_overflow = out.overflow;
    let preserved_addr = out.addr.first().copied() == Some(0xC00001);
    let preserved_aligned = output_is_aligned(&out);
    let output_payload_unchanged = first_output(&out) == Some(original_payload);
    output_clear(&mut out);
    if let Some(reuse) = reports.get(6) {
        tracker_update(&mut tracker, reuse, &mut out);
    }
    let output_rows_after_reuse = output_len(&out);
    let output_aligned = output_is_aligned(&out);
    tracker_flush(&mut tracker, 4 * S, &mut out);
    let log = tracker_log(&tracker);
    let log_rows = log.addr.len();
    let log_aligned = log.first_ns.len() == log_rows
        && log.confirmed_ns.len() == log_rows
        && log.last_ns.len() == log_rows
        && log.updates.len() == log_rows
        && log.reason.len() == log_rows;
    let log_overflow = log.overflow;
    let injected = !inject || allocator::inject_core_allocation();
    let Some(counts) = allocator::stop() else {
        return Err("output/log tracker window did not disarm cleanly");
    };
    Ok(OutputLogEvidence {
        counts,
        output_overflow: initial_overflow,
        output_rows_after_reuse,
        output_aligned: preserved_aligned && output_aligned,
        output_preserved_addr: preserved_addr,
        output_payload_unchanged,
        log_overflow,
        log_rows,
        log_aligned,
        injected,
    })
}

fn clock_gate_window(inject: bool) -> Result<ClockEvidence, &'static str> {
    let cfg = TrackerConfig {
        max_tracks: 8,
        out_capacity: 256,
        log_capacity: 8,
        tick_ns: S,
        expected_interval_ns: S,
        init_interval_ns: S,
        drop_after_ns: 100 * S,
        ooo_window_ns: 2 * S,
        clock_step_limit_ns: 3 * S,
        ..DEFAULT_CONFIG
    };
    let mut tracker = tracker_new(&cfg).map_err(|_| "clock-gate tracker configuration refused")?;
    let mut out = output_new(cfg.out_capacity);
    let mut reports = Vec::with_capacity(3 * 3 + 3 + 6 + 1);
    for (ix, key) in [0xD00001_u32, 0xD00002, 0xD00003].into_iter().enumerate() {
        let sensor = 10 + ix as u16;
        for (step, time) in [0, S, 2 * S].into_iter().enumerate() {
            let mut report = measurement(
                key,
                time,
                415_000_000 + ix as i32 * 100 + step as i32,
                -870_000_000,
            );
            report.sensor = sensor;
            reports.push(report);
        }
    }
    for (ix, key) in [0xD00001_u32, 0xD00002, 0xD00003].into_iter().enumerate() {
        let mut report = measurement(
            key,
            (20 + ix as i64) * S,
            415_000_010 + ix as i32 * 100,
            -870_000_000,
        );
        report.sensor = 20 + ix as u16;
        reports.push(report);
    }
    for ix in 0..6_usize {
        let time = 8 * S + (ix as i64) * (S / 2);
        let key = 0xD10000 + (ix % 3) as u32;
        let mut report = measurement(key, time, 416_000_000 + ix as i32, -870_000_000);
        report.sensor = 10 + (ix % 3) as u16;
        reports.push(report);
    }
    let recovery = measurement(0xD10002, 11 * S, 416_000_100, -870_000_000);
    if !allocator::start() {
        return Err("clock-gate tracker window did not arm cleanly");
    }
    for report in &reports {
        tracker_update(&mut tracker, report, &mut out);
    }
    let before_recovery = tracker_stats(&tracker).applied;
    tracker_update(&mut tracker, &recovery, &mut out);
    let stats = tracker_stats(&tracker);
    let recovered = stats.applied.saturating_sub(before_recovery);
    let held = stats.time_held;
    let held_applied = stats.time_held_applied;
    let step_forward = stats.time_steps;
    let late = stats.late_window;
    let step_back = stats.time_steps_back;
    let future_dropped = stats.time_future_dropped;
    let injected = !inject || allocator::inject_core_allocation();
    let Some(counts) = allocator::stop() else {
        return Err("clock-gate tracker window did not disarm cleanly");
    };
    Ok(ClockEvidence {
        counts,
        held,
        held_applied,
        step_forward,
        late,
        step_back,
        future_dropped,
        recovered,
        injected,
    })
}

fn addressed_plot(t_ns: i64, addr: u32, az_deg: f64) -> RadarPlot {
    RadarPlot {
        time_ns: t_ns,
        range_m: 20_000.0,
        az_deg,
        alt_ft: 20_000,
        addr,
        label: addr,
        mode_a: NONE_CODE,
    }
}

fn anonymous_plot(
    t_ns: i64,
    range_m: f64,
    az_deg: f64,
    label: u32,
    geom: &oa_track::radar::RadarGeom,
) -> Result<RadarPlot, &'static str> {
    let plot = RadarPlot {
        time_ns: t_ns,
        range_m,
        az_deg,
        alt_ft: 20_000,
        addr: NO_ADDR,
        label,
        mode_a: NONE_CODE,
    };
    let converted = plot_convert(geom, &SITE, &plot, &NO_BIAS, None, NO_ADDR)
        .map_err(|_| "anonymous radar fixture failed geometry conversion")?;
    if converted.m.addr != NO_ADDR {
        return Err("anonymous fixture did not preserve its unaddressed key");
    }
    Ok(plot)
}

fn fusion_bucket_capacity_window(inject: bool) -> Result<BucketCapacityEvidence, &'static str> {
    let base = TrackerConfig {
        max_tracks: 4,
        out_capacity: 256,
        log_capacity: 8,
        ..DEFAULT_CONFIG
    };
    let cfg = fusion_config(fusion_tracker_config(&base, &[SITE]));
    let mut fusion =
        fusion_new(&cfg, &[SITE]).map_err(|_| "bucket-capacity fusion configuration refused")?;
    let mut out = output_new(cfg.tracker.out_capacity);
    let seed_plot = addressed_plot(0, 0xE00000, 10.0);
    let geom = radar_geom(&SITE);
    let converted = plot_convert(&geom, &SITE, &seed_plot, &NO_BIAS, None, 0xE00000)
        .map_err(|_| "bucket-capacity seed geometry conversion failed")?;
    let mut seed_first = measurement(0xE00000, 0, converted.m.lat_e7, converted.m.lon_e7);
    seed_first.sensor = 0;
    let mut seed_second = measurement(0xE00000, S, converted.m.lat_e7, converted.m.lon_e7);
    seed_second.sensor = 0;
    fusion_adsb(&mut fusion, &seed_first);
    fusion_advance(&mut fusion, S, &mut out);
    fusion_adsb(&mut fusion, &seed_second);
    fusion_advance(&mut fusion, 2 * S, &mut out);

    let bucket_time = SITE.period_ns;
    let mut plots = Vec::with_capacity(MAX_BUCKET_PLOTS + 1);
    plots.push(addressed_plot(bucket_time, 0xE00000, 10.0));
    for offset in 0..MAX_BUCKET_PLOTS {
        let addr = 0xE10000_u32 + offset as u32;
        let azimuth = (offset % 360) as f64;
        plots.push(addressed_plot(bucket_time, addr, azimuth));
    }
    let recovery = addressed_plot(2 * SITE.period_ns, 0xE00000, 10.0);
    if !allocator::start() {
        return Err("bucket-capacity fusion window did not arm cleanly");
    }
    fusion_plots(&mut fusion, 0, &plots);
    fusion_advance(&mut fusion, bucket_time + S, &mut out);
    let first_bucket = *fusion_stats(&fusion);
    let bucket_plot_overflow = first_bucket.bucket_plot_overflow;
    let before_recovery = first_bucket.applied_hard;
    fusion_plots(&mut fusion, 0, std::slice::from_ref(&recovery));
    fusion_advance(&mut fusion, 2 * SITE.period_ns + S, &mut out);
    let after = fusion_stats(&fusion);
    let recovery_applied = after.applied_hard.saturating_sub(before_recovery);
    let plots_offered = after.plots_in;
    let known_hard_applied = before_recovery;
    let unmatched_hard_no_track = first_bucket.hard_no_track;
    let injected = !inject || allocator::inject_core_allocation();
    let Some(counts) = allocator::stop() else {
        return Err("bucket-capacity fusion window did not disarm cleanly");
    };
    Ok(BucketCapacityEvidence {
        counts,
        plots_offered,
        bucket_plot_overflow,
        known_hard_applied,
        unmatched_hard_no_track,
        recovery_applied,
        injected,
    })
}

fn fusion_window(inject: bool) -> Result<FusionEvidence, &'static str> {
    let base = TrackerConfig {
        max_tracks: 8,
        out_capacity: 32_768,
        log_capacity: 512,
        ..DEFAULT_CONFIG
    };
    let cfg = fusion_config(fusion_tracker_config(&base, &[SITE]));
    let mut fusion = fusion_new(&cfg, &[SITE]).map_err(|_| "fusion configuration refused")?;
    let mut out = output_new(cfg.tracker.out_capacity);
    let geom = radar_geom(&SITE);
    let mut adsb = [measurement(ADDRESS, 0, 0, 0); REFERENCE_PLOTS];
    let first_azimuth = REFERENCE_AZIMUTHS
        .first()
        .copied()
        .ok_or("reference azimuth fixture is empty")?;
    let mut plots = [addressed_plot(0, ADDRESS, first_azimuth); REFERENCE_PLOTS];
    for scan in 0..REFERENCE_SCANS {
        let time_ns = (scan as i64) * SITE.period_ns;
        for (reference, &azimuth) in REFERENCE_AZIMUTHS.iter().enumerate() {
            let ix = scan * REFERENCE_COUNT + reference;
            let addr = ADDRESS + reference as u32;
            let plot = addressed_plot(time_ns, addr, azimuth);
            let converted = plot_convert(&geom, &SITE, &plot, &NO_BIAS, None, addr)
                .map_err(|_| "reference fixture failed production plot conversion")?;
            let mut report = measurement(addr, time_ns, converted.m.lat_e7, converted.m.lon_e7);
            report.sensor = 0;
            *adsb.get_mut(ix).ok_or("reference ADS-B fixture index")? = report;
            *plots.get_mut(ix).ok_or("reference plot fixture index")? = plot;
        }
    }
    let mut anonymous = [[anonymous_plot(0, 90_000.0, 40.0, 100, &geom)?; 2]; 4];
    for (scan, [first, second]) in anonymous.iter_mut().enumerate() {
        let time_ns = (scan as i64 + REFERENCE_SCANS as i64) * SITE.period_ns;
        *first = anonymous_plot(time_ns, 90_000.0, 40.0, 100, &geom)?;
        *second = anonymous_plot(time_ns, 130_000.0, 230.0, 101, &geom)?;
    }
    let first = adsb.first().ok_or("reference ADS-B fixture is empty")?;
    let invalid = measurement(ADDRESS + 8, i64::MIN, first.lat_e7, first.lon_e7);

    if !allocator::start() {
        return Err("fusion allocation window did not arm cleanly");
    }
    fusion_adsb(&mut fusion, &invalid);
    for (ix, (report, plot)) in adsb.iter().zip(&plots).enumerate() {
        fusion_adsb(&mut fusion, report);
        fusion_plots(&mut fusion, 0, std::slice::from_ref(plot));
        if ix % REFERENCE_COUNT == REFERENCE_COUNT - 1 {
            let until = plot.time_ns.saturating_add(S);
            fusion_advance(&mut fusion, until, &mut out);
        }
    }
    for pair in &anonymous {
        fusion_plots(&mut fusion, 0, pair);
        let until = pair
            .first()
            .map_or(0, |plot| plot.time_ns)
            .saturating_add(S);
        fusion_advance(&mut fusion, until, &mut out);
    }
    let rows_before_quiet = output_len(&out);
    fusion_advance(
        &mut fusion,
        (REFERENCE_SCANS as i64 + 7) * SITE.period_ns,
        &mut out,
    );
    let quiet_advance_rows = output_len(&out).saturating_sub(rows_before_quiet);
    fusion_finish(
        &mut fusion,
        (REFERENCE_SCANS as i64 + 7) * SITE.period_ns + S,
        &mut out,
    );
    let injected = !inject || allocator::inject_core_allocation();
    let Some(counts) = allocator::stop() else {
        return Err("fusion allocation window did not disarm cleanly");
    };

    let stats = fusion_stats(&fusion);
    let plots_in = stats.plots_in;
    let hard_applied = stats.applied_hard;
    let confirmed_candidates = stats.cand_confirmed;
    let refused = fusion_tracker_stats(&fusion).refused.iter().copied().sum();
    let anon_applied = stats.applied_assoc;
    let initiation_visits = stats.init_visits;
    let initiation_tests = stats.init_tests;
    let registration =
        fusion_registration(&fusion, 0).ok_or("one radar registration state was not available")?;
    let registration_used = registration.0.used;
    let registration_history = registration.1.len();
    let rows = output_len(&out);
    let local_rows = out
        .addr
        .iter()
        .filter(|&&key| key >= RADAR_KEY_BASE)
        .count();
    let coast_rows = out
        .flags
        .iter()
        .filter(|&&flags| flags & O_COAST != 0)
        .count();
    let radar_tracks = oa_track::fusion::fusion_radar_tracks(&fusion).len();
    Ok(FusionEvidence {
        counts,
        plots: plots_in,
        hard_applied,
        candidates: confirmed_candidates,
        refused,
        rows,
        local_rows,
        quiet_advance_rows,
        coast_rows,
        radar_tracks,
        registration_used,
        registration_history,
        initiation_visits,
        initiation_tests,
        anon_applied,
        injected,
    })
}

fn main() {
    let mut controls_only_mode = false;
    let mut inject = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--controls-only" if !controls_only_mode && !inject => controls_only_mode = true,
            "--inject-allocation" if !inject && !controls_only_mode => inject = true,
            _ => fail("usage: oa-alloc-check [--controls-only | --inject-allocation]"),
        }
    }

    let Some(control_counts) = run_controls() else {
        fail("positive control did not observe alloc, alloc_zeroed, and realloc through the standard global API");
    };
    if controls_only_mode {
        println!(
            "{{\"status\":\"PASS_CONTROLS\",\"overflow\":false,\"alloc\":{},\"zeroed\":{},\"realloc\":{}}}",
            control_counts.alloc, control_counts.zeroed, control_counts.realloc
        );
        return;
    }

    let tracker = match tracker_window(inject) {
        Ok(evidence) => evidence,
        Err(message) => fail(message),
    };
    let default_capacity = match default_capacity_window(inject) {
        Ok(evidence) => evidence,
        Err(message) => fail(message),
    };
    let output_log = match output_log_window(inject) {
        Ok(evidence) => evidence,
        Err(message) => fail(message),
    };
    let clock_gate = match clock_gate_window(inject) {
        Ok(evidence) => evidence,
        Err(message) => fail(message),
    };
    let bucket_capacity = match fusion_bucket_capacity_window(inject) {
        Ok(evidence) => evidence,
        Err(message) => fail(message),
    };
    let adsb_pending = match adsb_pending_window(inject) {
        Ok(evidence) => evidence,
        Err(message) => fail(message),
    };
    let plot_pending = match plot_pending_window(inject) {
        Ok(evidence) => evidence,
        Err(message) => fail(message),
    };
    let fusion = match fusion_window(inject) {
        Ok(evidence) => evidence,
        Err(message) => fail(message),
    };

    let nam_session = match nam_session_window(inject) {
        Ok(evidence) => evidence,
        Err(message) => fail(message),
    };

    let display_frame = match display_window(inject) {
        Ok(evidence) => evidence,
        Err(message) => fail(message),
    };

    let evidence = AllocationEvidence {
        controls: control_counts,
        tracker: &tracker,
        default_capacity: &default_capacity,
        output_log: &output_log,
        clock_gate: &clock_gate,
        bucket_capacity: &bucket_capacity,
        adsb_pending: &adsb_pending,
        plot_pending: &plot_pending,
        fusion: &fusion,
        nam_session: &nam_session,
        display_frame: &display_frame,
    };

    if tracker.applied < 4
        || tracker.refiltered == 0
        || tracker.refused == 0
        || tracker.capacity_refused == 0
        || tracker.full_capacity != 8
        || tracker.recovery_applied == 0
        || tracker.rows == 0
        || tracker.predictions == 0
        || tracker.advance_rows == 0
        || tracker.flush_rows == 0
        || tracker.ended_total == 0
        || default_capacity.occupied != DEFAULT_CONFIG.max_tracks
        || default_capacity.confirmed != DEFAULT_CONFIG.max_tracks
        || default_capacity.refused != 1
        || default_capacity.recovered == 0
        || output_log.output_overflow == 0
        || output_log.output_rows_after_reuse == 0
        || !output_log.output_aligned
        || !output_log.output_preserved_addr
        || !output_log.output_payload_unchanged
        || output_log.log_overflow != 1
        || output_log.log_rows != 1
        || !output_log.log_aligned
        || clock_gate.held != 2
        || clock_gate.held_applied != 2
        || clock_gate.step_forward != 1
        || clock_gate.late != 5
        || clock_gate.step_back != 1
        || clock_gate.future_dropped != 3
        || clock_gate.recovered != 1
        || bucket_capacity.plots_offered != (MAX_BUCKET_PLOTS + 2) as u64
        || bucket_capacity.bucket_plot_overflow != 1
        || bucket_capacity.known_hard_applied != 1
        || bucket_capacity.unmatched_hard_no_track != 16_383
        || bucket_capacity.recovery_applied != 1
        || adsb_pending.offered != 1_048_577
        || adsb_pending.pending_overflow != 1
        || adsb_pending.drained != 1_048_576
        || adsb_pending.recovery_applied != 2
        || adsb_pending.tracker_applied != 1
        || adsb_pending.tracker_duplicates != 1_048_575
        || adsb_pending.tracker_refused != 0
        || adsb_pending.tracker_capacity_refused != 0
        || adsb_pending.tracker_late_window != 0
        || adsb_pending.tracker_late_ring != 0
        || adsb_pending.tracker_jump_rejected != 0
        || adsb_pending.tracker_numerical_refused != 0
        || adsb_pending.hard_extra != 0
        || adsb_pending.hard_mismatch != 0
        || plot_pending.offered != 1_048_577
        || plot_pending.pending_overflow != 1
        || plot_pending.drained != 1_048_576
        || plot_pending.bucket_plot_overflow != 1_032_192
        || plot_pending.recovery_applied != 1
        || plot_pending.tracker_applied != 1
        || plot_pending.tracker_duplicates != 0
        || plot_pending.tracker_refused != 0
        || plot_pending.tracker_capacity_refused != 0
        || plot_pending.tracker_late_window != 0
        || plot_pending.tracker_late_ring != 0
        || plot_pending.tracker_jump_rejected != 0
        || plot_pending.tracker_numerical_refused != 0
        || plot_pending.hard_extra != 16_383
        || plot_pending.hard_mismatch != 0
    {
        emit_evidence("FAIL_M3_COVERAGE", inject, &evidence);
        fail("tracker fixture did not prove all declared runtime paths");
    }
    if fusion.plots < REFERENCE_PLOTS as u64 + 8
        || fusion.hard_applied == 0
        || fusion.candidates < 2
        || fusion.refused == 0
        || fusion.registration_used == 0
        || fusion.registration_history == 0
        || fusion.initiation_visits == 0
        || fusion.initiation_tests == 0
        || fusion.anon_applied < 2
        || fusion.rows == 0
        || fusion.local_rows < 2
        || fusion.radar_tracks < 2
        || fusion.quiet_advance_rows == 0
        || fusion.coast_rows == 0
    {
        emit_evidence("FAIL_M3_COVERAGE", inject, &evidence);
        fail("fusion fixture did not prove association, registration, initiation, refusal, and output");
    }
    if nam_session.irq_sent < 3
        || nam_session.atm_sent == 0
        || nam_session.resent == 0
        || nam_session.timed_out == 0
        || nam_session.delivered == 0
        || nam_session.responses != 2
        || nam_session.asm_sent == 0
        || nam_session.peer_silent != 1
        || nam_session.lrm_sent != 1
        || nam_session.trq_sent == 0
        || nam_session.abandoned != oa_nam::session::MAX_PENDING as u64
        || nam_session.peak_pending != oa_nam::session::MAX_PENDING
        || nam_session.pending_full != 1
        || nam_session.discards == 0
        || nam_session.dropped != 0
        || nam_session.script_errors != 0
        || !nam_session.final_inactive
    {
        emit_evidence("FAIL_M3_COVERAGE", inject, &evidence);
        fail("NAM session fixture did not prove activation, traffic, retries, a full table and deactivation");
    }
    if display_frame.convert_added < 50
        || display_frame.convert_ended == 0
        || display_frame.traffic_drawn < 30
        || display_frame.uncertain == 0
        || display_frame.cs_conflict == 0
        || display_frame.r95_unknown == 0
        || display_frame.refused == 0
        || display_frame.alt_clamped == 0
        || display_frame.malformed != 1
        || display_frame.frame_drawn < 50
        || display_frame.scene_items == 0
        || display_frame.refusing_refused == 0
    {
        emit_evidence("FAIL_M3_COVERAGE", inject, &evidence);
        fail("display fixture did not draw rows, every target state, the map and the refusals");
    }
    if !tracker.injected
        || !default_capacity.injected
        || !output_log.injected
        || !clock_gate.injected
        || !bucket_capacity.injected
        || !adsb_pending.injected
        || !plot_pending.injected
        || !fusion.injected
        || !nam_session.injected
        || !display_frame.injected
    {
        emit_evidence("FAIL_M3_INJECTION_NOT_OBSERVED", inject, &evidence);
        std::process::exit(1);
    }
    if !counts_zero(tracker.counts)
        || !counts_zero(default_capacity.counts)
        || !counts_zero(output_log.counts)
        || !counts_zero(clock_gate.counts)
        || !counts_zero(bucket_capacity.counts)
        || !counts_zero(adsb_pending.counts)
        || !counts_zero(plot_pending.counts)
        || !counts_zero(fusion.counts)
        || !counts_zero(nam_session.counts)
        || !counts_zero(display_frame.counts)
    {
        emit_evidence("FAIL_M3_ALLOCATION", inject, &evidence);
        std::process::exit(1);
    }
    emit_evidence("PASS_M3_EXERCISED", false, &evidence);
}

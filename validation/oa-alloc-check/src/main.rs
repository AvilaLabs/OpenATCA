#![deny(unsafe_code)]
//! Standalone allocation validation. Shell (B3): fixtures, execution and reports.

mod allocator;

use allocator::{control_alloc, control_realloc, control_zeroed, Counts};
use oa_track::fusion::{
    fusion_adsb, fusion_advance, fusion_config, fusion_finish, fusion_new, fusion_plots,
    fusion_registration, fusion_stats, fusion_tracker_config, fusion_tracker_stats,
};
use oa_track::radar::{plot_convert, radar_geom, RadarPlot, RadarSite, NONE_CODE, NO_BIAS};
use oa_track::{
    output_len, output_new, tracker_advance, tracker_flush, tracker_new, tracker_predict,
    tracker_counts, tracker_stats, tracker_update, Measurement, TrackOutput, Tracker, TrackerConfig,
    DEFAULT_CONFIG, M_HAS_VEL, NONE_I32, NO_ADDR, NO_IDENT, O_COAST, RADAR_KEY_BASE,
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

fn emit_evidence(
    status: &'static str,
    injection_requested: bool,
    controls: Counts,
    tracker: &TrackerEvidence,
    fusion: &FusionEvidence,
) {
    println!(
        "{{\"status\":\"{}\",\"injection_requested\":{},\"fixture_ids\":[\"tracker_update_ooo_refusal_capacity_advance_flush\",\"fusion_adsb_hard_assoc_registration\",\"fusion_anonymous_init_and_assoc\"],\"route_ids\":[\"tracker_update\",\"tracker_out_of_order_refilter\",\"tracker_invalid_input_refusal\",\"tracker_capacity_refusal\",\"tracker_capacity_recovery\",\"tracker_prediction\",\"tracker_advance\",\"tracker_flush\",\"fusion_adsb_ingest\",\"fusion_plot_ingest\",\"fusion_advance\",\"fusion_finish\",\"fusion_registration\",\"fusion_anonymous_initiation\",\"fusion_local_association\"],\"controls\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{}}},\"tracker\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{},\"applied\":{},\"refiltered\":{},\"refused\":{},\"capacity_refused\":{},\"full_capacity\":{},\"recovery_applied\":{},\"rows\":{},\"predictions\":{},\"advance_rows\":{},\"flush_rows\":{},\"ended\":{}}},\"fusion\":{{\"overflow\":{},\"alloc\":{},\"zeroed\":{},\"realloc\":{},\"plots\":{},\"hard_applied\":{},\"anon_applied\":{},\"candidates_confirmed\":{},\"init_visits\":{},\"init_tests\":{},\"refused\":{},\"rows\":{},\"local_rows\":{},\"radar_tracks\":{},\"registration_used\":{},\"registration_history\":{},\"quiet_advance_rows\":{},\"coast_rows\":{}}}}}",
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
    let recovery_applied = tracker_stats(&tracker).applied.saturating_sub(applied_before_recovery);
    let predictions = if tracker_predict(&tracker, ADDRESS, 4 * S).is_some() {
        1
    } else {
        0
    };
    let rows_before_advance = output_len(&out);
    tracker_advance(&mut tracker, 4 * S, &mut out);
    let rows_after_advance = output_len(&out);
    let advance_rows = rows_after_advance.saturating_sub(rows_before_advance);
    tracker_flush(&mut tracker, 5 * S, &mut out);
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
    let fusion = match fusion_window(inject) {
        Ok(evidence) => evidence,
        Err(message) => fail(message),
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
    {
        emit_evidence(
            "FAIL_M3_COVERAGE",
            inject,
            control_counts,
            &tracker,
            &fusion,
        );
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
        emit_evidence(
            "FAIL_M3_COVERAGE",
            inject,
            control_counts,
            &tracker,
            &fusion,
        );
        fail("fusion fixture did not prove association, registration, initiation, refusal, and output");
    }
    if !tracker.injected || !fusion.injected {
        emit_evidence(
            "FAIL_M3_INJECTION_NOT_OBSERVED",
            inject,
            control_counts,
            &tracker,
            &fusion,
        );
        std::process::exit(1);
    }
    if !counts_zero(tracker.counts) || !counts_zero(fusion.counts) {
        emit_evidence(
            "FAIL_M3_ALLOCATION",
            inject,
            control_counts,
            &tracker,
            &fusion,
        );
        std::process::exit(1);
    }
    emit_evidence(
        "PASS_M3_EXERCISED",
        false,
        control_counts,
        &tracker,
        &fusion,
    );
}

//! Full production pending-queue capacity windows. Shell (B3): bounded fixtures and evidence.
#![forbid(unsafe_code)]
use super::*;
use oa_track::fusion_params::{MAX_BUCKET_PLOTS, MAX_PENDING};

/// A7.2: cold evidence for one measured pending-queue window.
pub(super) struct PendingWindowEvidence {
    pub counts: Counts,
    pub offered: u64,
    pub pending_overflow: u64,
    pub drained: u64,
    pub bucket_plot_overflow: u64,
    pub recovery_applied: u64,
    pub tracker_applied: u64,
    pub tracker_duplicates: u64,
    pub tracker_refused: u64,
    pub tracker_capacity_refused: u64,
    pub tracker_late_window: u64,
    pub tracker_late_ring: u64,
    pub tracker_jump_rejected: u64,
    pub tracker_numerical_refused: u64,
    pub hard_extra: u64,
    pub hard_mismatch: u64,
    pub injected: bool,
}

/// Fill the actual ADS-B pending queue with one valid repeated addressed report, overflow once,
/// drain it, then prove later addressed reports still apply. Repeated identity avoids pair search.
pub(super) fn adsb_pending_window(inject: bool) -> Result<PendingWindowEvidence, &'static str> {
    let base = TrackerConfig {
        max_tracks: 8,
        out_capacity: 32_768,
        log_capacity: 64,
        ..DEFAULT_CONFIG
    };
    let cfg = fusion_config(fusion_tracker_config(&base, &[SITE]));
    let mut fusion =
        fusion_new(&cfg, &[SITE]).map_err(|_| "pending ADS-B fusion configuration refused")?;
    let mut out = output_new(cfg.tracker.out_capacity);
    let report = measurement(ADDRESS, 0, 419_000_000, -879_000_000);
    if !allocator::start() {
        return Err("pending ADS-B window did not arm cleanly");
    }
    for _ in 0..MAX_PENDING {
        fusion_adsb(&mut fusion, &report);
    }
    fusion_adsb(&mut fusion, &report);
    let pending_overflow = fusion_stats(&fusion).pending_overflow;
    fusion_advance(&mut fusion, S, &mut out);
    let queue_stats = fusion_stats(&fusion);
    let drained = queue_stats.processed;
    let tracker_queue_stats = fusion_tracker_stats(&fusion);
    let tracker_applied = tracker_queue_stats.applied;
    let tracker_duplicates = tracker_queue_stats.duplicates;
    let tracker_refused = tracker_queue_stats.refused.iter().copied().sum();
    let tracker_capacity_refused = tracker_queue_stats.capacity_refused;
    let tracker_late_window = tracker_queue_stats.late_window;
    let tracker_late_ring = tracker_queue_stats.late_ring;
    let tracker_jump_rejected = tracker_queue_stats.jump_rejected;
    let tracker_numerical_refused = tracker_queue_stats.numerical_refused;
    let before_recovery = fusion_tracker_stats(&fusion).applied;
    let later_first = measurement(ADDRESS, 2 * S, 419_000_001, -879_000_000);
    let later_second = measurement(ADDRESS, 3 * S, 419_000_002, -879_000_000);
    fusion_adsb(&mut fusion, &later_first);
    fusion_advance(&mut fusion, 3 * S, &mut out);
    fusion_adsb(&mut fusion, &later_second);
    fusion_advance(&mut fusion, 4 * S, &mut out);
    let recovery_applied = fusion_tracker_stats(&fusion)
        .applied
        .saturating_sub(before_recovery);
    let injected = !inject || allocator::inject_core_allocation();
    let counts = allocator::stop().ok_or("pending ADS-B window did not disarm")?;
    Ok(PendingWindowEvidence {
        counts,
        offered: (MAX_PENDING as u64) + 1,
        pending_overflow,
        drained,
        bucket_plot_overflow: 0,
        recovery_applied,
        tracker_applied,
        tracker_duplicates,
        tracker_refused,
        tracker_capacity_refused,
        tracker_late_window,
        tracker_late_ring,
        tracker_jump_rejected,
        tracker_numerical_refused,
        hard_extra: 0,
        hard_mismatch: 0,
        injected,
    })
}

/// Fill the actual plot pending queue, count its single overflow and the production bucket's
/// intentional cap loss, then confirm a next-scan addressed plot remains applicable.
pub(super) fn plot_pending_window(inject: bool) -> Result<PendingWindowEvidence, &'static str> {
    let base = TrackerConfig {
        max_tracks: 8,
        out_capacity: 32_768,
        log_capacity: 64,
        ..DEFAULT_CONFIG
    };
    let cfg = fusion_config(fusion_tracker_config(&base, &[SITE]));
    let mut fusion =
        fusion_new(&cfg, &[SITE]).map_err(|_| "pending plot fusion configuration refused")?;
    let mut out = output_new(cfg.tracker.out_capacity);
    let seed_plot = addressed_plot(0, ADDRESS, 10.0);
    let geom = radar_geom(&SITE);
    let converted = plot_convert(&geom, &SITE, &seed_plot, &NO_BIAS, None, ADDRESS)
        .map_err(|_| "pending plot seed conversion failed")?;
    for second in 0..3_i64 {
        let mut report = measurement(ADDRESS, second * S, converted.m.lat_e7, converted.m.lon_e7);
        report.sensor = 0;
        fusion_adsb(&mut fusion, &report);
        fusion_advance(&mut fusion, (second + 1) * S, &mut out);
    }
    let tracker_seed_stats = *fusion_tracker_stats(&fusion);
    let plot = addressed_plot(SITE.period_ns, ADDRESS, 10.0);
    if !allocator::start() {
        return Err("pending plot window did not arm cleanly");
    }
    for _ in 0..MAX_PENDING {
        fusion_plots(&mut fusion, 0, std::slice::from_ref(&plot));
    }
    fusion_plots(&mut fusion, 0, std::slice::from_ref(&plot));
    let pending_overflow = fusion_stats(&fusion).pending_overflow;
    let processed_before_drain = fusion_stats(&fusion).processed;
    fusion_advance(&mut fusion, 5 * S, &mut out);
    let after_drain = *fusion_stats(&fusion);
    let bucket_plot_overflow = after_drain.bucket_plot_overflow;
    let drained = after_drain.processed.saturating_sub(processed_before_drain);
    let tracker_queue_stats = fusion_tracker_stats(&fusion);
    let tracker_applied = tracker_queue_stats
        .applied
        .saturating_sub(tracker_seed_stats.applied);
    let tracker_duplicates = tracker_queue_stats
        .duplicates
        .saturating_sub(tracker_seed_stats.duplicates);
    let tracker_refused = tracker_queue_stats
        .refused
        .iter()
        .copied()
        .sum::<u64>()
        .saturating_sub(tracker_seed_stats.refused.iter().copied().sum());
    let tracker_capacity_refused = tracker_queue_stats
        .capacity_refused
        .saturating_sub(tracker_seed_stats.capacity_refused);
    let tracker_late_window = tracker_queue_stats
        .late_window
        .saturating_sub(tracker_seed_stats.late_window);
    let tracker_late_ring = tracker_queue_stats
        .late_ring
        .saturating_sub(tracker_seed_stats.late_ring);
    let tracker_jump_rejected = tracker_queue_stats
        .jump_rejected
        .saturating_sub(tracker_seed_stats.jump_rejected);
    let tracker_numerical_refused = tracker_queue_stats
        .numerical_refused
        .saturating_sub(tracker_seed_stats.numerical_refused);
    let before_recovery = after_drain.applied_hard;
    let recovery = addressed_plot(2 * SITE.period_ns, ADDRESS, 10.0);
    fusion_plots(&mut fusion, 0, std::slice::from_ref(&recovery));
    fusion_advance(&mut fusion, 10 * S, &mut out);
    let recovery_applied = fusion_stats(&fusion)
        .applied_hard
        .saturating_sub(before_recovery);
    let injected = !inject || allocator::inject_core_allocation();
    let counts = allocator::stop().ok_or("pending plot window did not disarm")?;
    Ok(PendingWindowEvidence {
        counts,
        offered: (MAX_PENDING as u64) + 1,
        pending_overflow,
        drained,
        bucket_plot_overflow,
        recovery_applied,
        tracker_applied,
        tracker_duplicates,
        tracker_refused,
        tracker_capacity_refused,
        tracker_late_window,
        tracker_late_ring,
        tracker_jump_rejected,
        tracker_numerical_refused,
        hard_extra: after_drain.hard_extra,
        hard_mismatch: after_drain.hard_mismatch,
        injected,
    })
}

const _: () = assert!(MAX_PENDING == 1_048_576);
const _: () = assert!(MAX_BUCKET_PLOTS == 16_384);

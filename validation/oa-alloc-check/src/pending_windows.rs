//! Full production pending-queue capacity windows. Shell (B3): bounded fixtures and evidence.
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
    pub injected: bool,
}

/// Fill the actual ADS-B pending queue with one valid repeated addressed report, overflow once,
/// drain it, then prove later addressed reports still apply. Repeated identity avoids pair search.
pub(super) fn adsb_pending_window(inject: bool) -> Result<PendingWindowEvidence, &'static str> {
    let base = TrackerConfig { max_tracks: 8, out_capacity: 32_768, log_capacity: 64, ..DEFAULT_CONFIG };
    let cfg = fusion_config(fusion_tracker_config(&base, &[SITE]));
    let mut fusion = fusion_new(&cfg, &[SITE]).map_err(|_| "pending ADS-B fusion configuration refused")?;
    let mut out = output_new(cfg.tracker.out_capacity);
    let report = measurement(ADDRESS, 0, 419_000_000, -879_000_000);
    if !allocator::start() { return Err("pending ADS-B window did not arm cleanly"); }
    for _ in 0..MAX_PENDING { fusion_adsb(&mut fusion, &report); }
    fusion_adsb(&mut fusion, &report);
    let pending_overflow = fusion_stats(&fusion).pending_overflow;
    fusion_advance(&mut fusion, S, &mut out);
    let drained = fusion_stats(&fusion).processed;
    let before_recovery = fusion_tracker_stats(&fusion).applied;
    let later_first = measurement(ADDRESS, 2 * S, 419_000_001, -879_000_000);
    let later_second = measurement(ADDRESS, 3 * S, 419_000_002, -879_000_000);
    fusion_adsb(&mut fusion, &later_first);
    fusion_advance(&mut fusion, 3 * S, &mut out);
    fusion_adsb(&mut fusion, &later_second);
    fusion_advance(&mut fusion, 4 * S, &mut out);
    let recovery_applied = fusion_tracker_stats(&fusion).applied.saturating_sub(before_recovery);
    let injected = !inject || allocator::inject_core_allocation();
    let counts = allocator::stop().ok_or("pending ADS-B window did not disarm")?;
    Ok(PendingWindowEvidence { counts, offered: (MAX_PENDING as u64) + 1, pending_overflow, drained, bucket_plot_overflow: 0, recovery_applied, injected })
}

/// Fill the actual plot pending queue, count its single overflow and the production bucket's
/// intentional cap loss, then confirm a next-scan addressed plot remains applicable.
pub(super) fn plot_pending_window(inject: bool) -> Result<PendingWindowEvidence, &'static str> {
    let base = TrackerConfig { max_tracks: 8, out_capacity: 32_768, log_capacity: 64, ..DEFAULT_CONFIG };
    let cfg = fusion_config(fusion_tracker_config(&base, &[SITE]));
    let mut fusion = fusion_new(&cfg, &[SITE]).map_err(|_| "pending plot fusion configuration refused")?;
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
    let plot = addressed_plot(SITE.period_ns, ADDRESS, 10.0);
    if !allocator::start() { return Err("pending plot window did not arm cleanly"); }
    for _ in 0..MAX_PENDING { fusion_plots(&mut fusion, 0, std::slice::from_ref(&plot)); }
    fusion_plots(&mut fusion, 0, std::slice::from_ref(&plot));
    let pending_overflow = fusion_stats(&fusion).pending_overflow;
    let processed_before_drain = fusion_stats(&fusion).processed;
    fusion_advance(&mut fusion, 5 * S, &mut out);
    let after_drain = fusion_stats(&fusion);
    let bucket_plot_overflow = after_drain.bucket_plot_overflow;
    let drained = after_drain.processed.saturating_sub(processed_before_drain);
    let before_recovery = after_drain.applied_hard;
    let recovery = addressed_plot(2 * SITE.period_ns, ADDRESS, 10.0);
    fusion_plots(&mut fusion, 0, std::slice::from_ref(&recovery));
    fusion_advance(&mut fusion, 10 * S, &mut out);
    let recovery_applied = fusion_stats(&fusion).applied_hard.saturating_sub(before_recovery);
    let injected = !inject || allocator::inject_core_allocation();
    let counts = allocator::stop().ok_or("pending plot window did not disarm")?;
    Ok(PendingWindowEvidence { counts, offered: (MAX_PENDING as u64) + 1, pending_overflow, drained, bucket_plot_overflow, recovery_applied, injected })
}

const _: () = assert!(MAX_PENDING == 1_048_576);
const _: () = assert!(MAX_BUCKET_PLOTS == 16_384);

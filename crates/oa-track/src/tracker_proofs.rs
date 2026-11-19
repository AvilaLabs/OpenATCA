//! Narrow Kani proofs of the actual in-order dispatch and its transaction rollback.
//!
//! `decide` is stubbed to isolate the dispatch outcomes from nonlinear filter calculations;
//! `filter_output` is stubbed only where row representability is the input under proof. The
//! production `update_in_order`, transaction helpers, confirmation gate, `emit`, and
//! `output_push` remain in the proof path.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;
use crate::hfilter::{HFilter, HModel};
use crate::vfilter::{VFilter, VModel};

const PROOF_ADDR: u32 = 0x00ab_c123;
const PROOF_SLOT: usize = 0;
const PROOF_TIME: i64 = 1_700_000_000_000_000_000;

fn proof_tracker(state: u8) -> Tracker {
    // No rollback pool: the proofs that are not about a large move do not need its memory.
    proof_tracker_with_pool(state, 0)
}

fn proof_tracker_with_pool(state: u8, pool: usize) -> Tracker {
    let cfg = TrackerConfig {
        max_tracks: 1,
        out_capacity: 1,
        log_capacity: 1,
        confirm_pool: pool,
        ..crate::types::DEFAULT_CONFIG
    };
    let mut t = match tracker_new(&cfg) {
        Ok(t) => t,
        Err(e) => panic!("proof tracker configuration unexpectedly refused: {e:?}"),
    };
    t.high = 1;
    t.active = 1;
    t.key[PROOF_SLOT] = PROOF_ADDR;
    t.state[PROOF_SLOT] = state;
    t.last_ns[PROOF_SLOT] = PROOF_TIME - 1;
    t.confirmed_ns[PROOF_SLOT] = PROOF_TIME - 10;
    t.cur[PROOF_SLOT] = blank_filter();
    t.anchor[PROOF_SLOT] = blank_filter();
    t
}

fn proof_measurement() -> Measurement {
    let mut m = blank_measurement();
    m.toa_ns = kani::any();
    kani::assume(m.toa_ns > PROOF_TIME && m.toa_ns < PROOF_TIME + 1_000);
    m.addr = PROOF_ADDR;
    m.flags = kani::any();
    m
}

fn fixed_proof_measurement() -> Measurement {
    Measurement {
        toa_ns: PROOF_TIME,
        addr: PROOF_ADDR,
        ..blank_measurement()
    }
}

const LATE_BASE_NS: i64 = PROOF_TIME - 20;
const LATE_INSERT_NS: i64 = PROOF_TIME - 10;
const LATE_LATER_NS: i64 = PROOF_TIME;

fn late_proof_tracker() -> Tracker {
    let mut t = proof_tracker(CONFIRMED);
    let base = Measurement {
        toa_ns: LATE_BASE_NS,
        addr: PROOF_ADDR,
        sensor: 1,
        ..blank_measurement()
    };
    let later = Measurement {
        toa_ns: LATE_LATER_NS,
        addr: PROOF_ADDR,
        sensor: 2,
        ..blank_measurement()
    };
    t.last_ns[PROOF_SLOT] = LATE_LATER_NS;
    t.confirmed_ns[PROOF_SLOT] = LATE_BASE_NS;
    t.updates[PROOF_SLOT] = 2;
    t.hits[PROOF_SLOT] = 2;
    t.ring_len[PROOF_SLOT] = 2;
    t.ring_head[PROOF_SLOT] = 0;
    t.anchor[PROOF_SLOT].t_ns = LATE_BASE_NS;
    t.cur[PROOF_SLOT].t_ns = LATE_LATER_NS;
    t.ring_m[PROOF_SLOT * RING_DEPTH] = base;
    t.ring_m[PROOF_SLOT * RING_DEPTH + 1] = later;
    t.newest_update_ns = LATE_LATER_NS;
    t
}

fn late_proof_measurement() -> Measurement {
    Measurement {
        toa_ns: LATE_INSERT_NS,
        addr: PROOF_ADDR,
        sensor: 3,
        ..blank_measurement()
    }
}

fn output_row(at_ns: i64) -> OutRow {
    OutRow {
        t_ns: at_ns,
        addr: 0,
        lat_e7: 0,
        lon_e7: 0,
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        pos_cov_m2: [0.0; 3],
        vel_cov_m2s2: [0.0; 3],
        turn_rate_rps: 0.0,
        alt_ft: 0.0,
        alt_var_ft2: 0.0,
        vrate_fpm: 0.0,
        mu_h: [0.0; 2],
        mu_v: [0.0; 2],
        flags: 0,
    }
}

fn arbitrary_h_model() -> HModel {
    let mut x = [0.0; 5];
    let mut p = [[0.0; 5]; 5];
    for i in 0..5 {
        x[i] = kani::any();
        for j in 0..5 {
            p[i][j] = kani::any();
        }
    }
    HModel { x, p }
}

fn arbitrary_v_model() -> VModel {
    VModel {
        x: [kani::any(), kani::any()],
        p: [kani::any(), kani::any(), kani::any()],
    }
}

fn arbitrary_filter() -> Filter {
    Filter {
        h: HFilter {
            m: [arbitrary_h_model(), arbitrary_h_model()],
            mu: [kani::any(), kani::any()],
        },
        v: VFilter {
            m: [arbitrary_v_model(), arbitrary_v_model()],
            mu: [kani::any(), kani::any()],
        },
        t_ns: kani::any(),
        lat_e7: kani::any(),
        lon_e7: kani::any(),
        trig: [kani::any(), kani::any()],
        rej_t_ns: kani::any(),
        rej_lat_e7: kani::any(),
        rej_lon_e7: kani::any(),
        rej_h: kani::any(),
        rej_v: kani::any(),
        has_alt: kani::any(),
        h_mode: kani::any(),
        rej_c: kani::any(),
        reacq: kani::any(),
    }
}

fn arbitrary_measurement() -> Measurement {
    Measurement {
        toa_ns: kani::any(),
        pos_cov_m2: [kani::any(), kani::any(), kani::any()],
        vel_cov_m2s2: [kani::any(), kani::any(), kani::any()],
        vel_east_mps: kani::any(),
        vel_north_mps: kani::any(),
        lat_e7: kani::any(),
        lon_e7: kani::any(),
        alt_ft: kani::any(),
        vrate_fpm: kani::any(),
        addr: kani::any(),
        alt_quant_ft: kani::any(),
        sensor: kani::any(),
        ident: kani::any(),
        flags: kani::any(),
        category: kani::any(),
    }
}

fn arbitrary_category_state() -> CategoryState {
    CategoryState {
        trusted: kani::any(),
        cand: kani::any(),
        cand_n: kani::any(),
    }
}

fn arbitrary_stats() -> TrackerStats {
    TrackerStats {
        offered: kani::any(),
        applied: kani::any(),
        refused: [
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
        ],
        duplicates: kani::any(),
        late_window: kani::any(),
        late_ring: kani::any(),
        refiltered: kani::any(),
        replayed: kani::any(),
        jump_rejected: kani::any(),
        numerical_refused: kani::any(),
        resyncs: kani::any(),
        alt_rejected: kani::any(),
        alt_geometric_skipped: kani::any(),
        capacity_refused: kani::any(),
        started: kani::any(),
        confirmed: kani::any(),
        ended: [kani::any(), kani::any(), kani::any()],
        init_implausible: kani::any(),
        init_window_expired: kani::any(),
        ticks: kani::any(),
        ticks_skipped: kani::any(),
        rows_computed: kani::any(),
        rows_unrepresentable: kani::any(),
        rows_uncertain: kani::any(),
        nan_covariance: kani::any(),
        velocity_gated: kani::any(),
        time_held: kani::any(),
        time_steps: kani::any(),
        time_held_applied: kani::any(),
        time_steps_back: kani::any(),
        time_future_dropped: kani::any(),
        time_sensor_evicted: kani::any(),
        moves_unconfirmed: kani::any(),
        moves_confirmed: kani::any(),
        moves_rolled_back: kani::any(),
        moves_undecided: kani::any(),
        moves_decision_rejected: kani::any(),
        moves_late: kani::any(),
        moves_rollback_lost_late: kani::any(),
        moves_abandoned_ended: kani::any(),
        moves_abandoned_restarted: kani::any(),
        moves_abandoned_resync: kani::any(),
        moves_abandoned_demoted: kani::any(),
        moves_abandoned_numerical: kani::any(),
        move_no_snapshot: kani::any(),
        move_widen_failed: kani::any(),
        moves_internal_refused: kani::any(),
        moves_peak_active: kani::any(),
        nis_sum: kani::any(),
        nis_n: kani::any(),
    }
}

fn return_row(_: &Filter, at_ns: i64) -> Option<OutRow> {
    Some(output_row(at_ns))
}

fn return_current_row(_: &Filter, at_ns: i64) -> Option<OutRow> {
    assert_eq!(at_ns, PROOF_TIME);
    Some(output_row(at_ns))
}

fn return_current_unrepresentable(_: &Filter, at_ns: i64) -> Option<OutRow> {
    assert_eq!(at_ns, PROOF_TIME);
    None
}

fn return_late_row(_: &Filter, at_ns: i64) -> Option<OutRow> {
    assert_eq!(at_ns, LATE_INSERT_NS);
    Some(output_row(at_ns))
}

fn filter_replay_must_be_unreachable(
    _: &mut Filter,
    _: &Measurement,
    _: &mut TrackerStats,
    _: &TrackerConfig,
) -> Applied {
    panic!("filter_replay unexpectedly reached by bounded proof fixture")
}

fn implausible_must_be_unreachable(_: &Tracker, _: usize, _: &Measurement) -> bool {
    panic!("tentative plausibility path unexpectedly reached by bounded proof fixture")
}

fn track_begin_must_be_unreachable(_: &mut Tracker, _: usize, _: &Measurement) -> bool {
    panic!("track restart path unexpectedly reached by bounded proof fixture")
}

fn filter_apply_insert_then_refuse(
    f: &mut Filter,
    m: &Measurement,
    stats: &mut TrackerStats,
    _: &TrackerConfig,
) -> Applied {
    if m.toa_ns == LATE_INSERT_NS {
        f.t_ns = m.toa_ns;
        f.h.m[0].x[0] = 123.0;
        stats.applied = 7;
        Applied::Yes
    } else if m.toa_ns == LATE_LATER_NS {
        f.t_ns = m.toa_ns;
        *stats = arbitrary_stats();
        #[cfg(kani_late_replay_covers)]
        kani::cover!(true, "late replay reaches later numerical refusal");
        Applied::NumericalRefused
    } else {
        panic!("unexpected replay measurement in proof fixture")
    }
}

fn filter_apply_symbolic_yes_or_resync(
    f: &mut Filter,
    m: &Measurement,
    stats: &mut TrackerStats,
    _: &TrackerConfig,
) -> Applied {
    f.t_ns = m.toa_ns;
    f.h.m[0].x[0] = 321.0;
    let is_resync: bool = kani::any();
    if is_resync {
        stats.resyncs += 1;
        Applied::Resync
    } else {
        Applied::Yes
    }
}

fn decide_applied_or_resynced(t: &mut Tracker, slot: usize, m: &Measurement) -> Step {
    t.last_ns[slot] = m.toa_ns;
    let is_resync: bool = kani::any();
    if is_resync {
        t.stats.resyncs += 1;
        Step::Resynced
    } else {
        Step::Applied
    }
}

fn decide_rejected(_: &mut Tracker, _: usize, _: &Measurement) -> Step {
    Step::Rejected
}

fn decide_begun(t: &mut Tracker, slot: usize, m: &Measurement) -> Step {
    t.last_ns[slot] = m.toa_ns;
    Step::Begun
}

fn decide_numerical_refusal(t: &mut Tracker, slot: usize, _: &Measurement) -> Step {
    t.state[slot] = kani::any();
    t.first_ns[slot] = kani::any();
    t.confirmed_ns[slot] = kani::any();
    t.last_ns[slot] = kani::any();
    t.disagree_until_ns[slot] = kani::any();
    t.expected_ns[slot] = kani::any();
    t.updates[slot] = kani::any();
    t.hits[slot] = kani::any();
    t.cur[slot] = arbitrary_filter();
    t.ring_len[slot] = kani::any();
    t.ring_head[slot] = kani::any();
    t.cat[slot] = arbitrary_category_state();
    t.anchor[slot] = arbitrary_filter();
    let start = slot * RING_DEPTH;
    for k in 0..RING_DEPTH {
        t.ring_m[start + k] = arbitrary_measurement();
    }
    t.newest_update_ns = kani::any();
    t.stats = arbitrary_stats();
    Step::NumericalRefused
}

fn assert_same_track_txn(a: &TrackTxn, b: &TrackTxn) {
    assert_filter_bits(&a.cur, &b.cur);
    assert_filter_bits(&a.anchor, &b.anchor);
    for k in 0..RING_DEPTH {
        assert_measurement_bits(&a.ring[k], &b.ring[k]);
    }
    assert_eq!(a.first_ns, b.first_ns);
    assert_eq!(a.confirmed_ns, b.confirmed_ns);
    assert_eq!(a.last_ns, b.last_ns);
    assert_eq!(a.disagree_until_ns, b.disagree_until_ns);
    assert_eq!(a.expected_ns, b.expected_ns);
    assert_eq!(a.newest_update_ns, b.newest_update_ns);
    assert_eq!(a.cat.trusted, b.cat.trusted);
    assert_eq!(a.cat.cand, b.cat.cand);
    assert_eq!(a.cat.cand_n, b.cat.cand_n);
    assert_eq!(a.updates, b.updates);
    assert_eq!(a.state, b.state);
    assert_eq!(a.hits, b.hits);
    assert_eq!(a.ring_len, b.ring_len);
    assert_eq!(a.ring_head, b.ring_head);
}

fn assert_stats_eq(a: &TrackerStats, b: &TrackerStats) {
    assert_eq!(a.offered, b.offered);
    assert_eq!(a.applied, b.applied);
    for i in 0..crate::types::REFUSE_KINDS {
        assert_eq!(a.refused[i], b.refused[i]);
    }
    assert_eq!(a.duplicates, b.duplicates);
    assert_eq!(a.late_window, b.late_window);
    assert_eq!(a.late_ring, b.late_ring);
    assert_eq!(a.refiltered, b.refiltered);
    assert_eq!(a.replayed, b.replayed);
    assert_eq!(a.jump_rejected, b.jump_rejected);
    assert_eq!(a.numerical_refused, b.numerical_refused);
    assert_eq!(a.resyncs, b.resyncs);
    assert_eq!(a.alt_rejected, b.alt_rejected);
    assert_eq!(a.alt_geometric_skipped, b.alt_geometric_skipped);
    assert_eq!(a.capacity_refused, b.capacity_refused);
    assert_eq!(a.started, b.started);
    assert_eq!(a.confirmed, b.confirmed);
    for i in 0..crate::types::END_KINDS {
        assert_eq!(a.ended[i], b.ended[i]);
    }
    assert_eq!(a.init_implausible, b.init_implausible);
    assert_eq!(a.init_window_expired, b.init_window_expired);
    assert_eq!(a.ticks, b.ticks);
    assert_eq!(a.ticks_skipped, b.ticks_skipped);
    assert_eq!(a.rows_computed, b.rows_computed);
    assert_eq!(a.rows_unrepresentable, b.rows_unrepresentable);
    assert_eq!(a.rows_uncertain, b.rows_uncertain);
    assert_eq!(a.nan_covariance, b.nan_covariance);
    assert_eq!(a.velocity_gated, b.velocity_gated);
    assert_eq!(a.time_held, b.time_held);
    assert_eq!(a.time_steps, b.time_steps);
    assert_eq!(a.time_held_applied, b.time_held_applied);
    assert_eq!(a.time_steps_back, b.time_steps_back);
    assert_eq!(a.time_future_dropped, b.time_future_dropped);
    assert_eq!(a.time_sensor_evicted, b.time_sensor_evicted);
    assert_eq!(a.moves_unconfirmed, b.moves_unconfirmed);
    assert_eq!(a.moves_confirmed, b.moves_confirmed);
    assert_eq!(a.moves_rolled_back, b.moves_rolled_back);
    assert_eq!(a.moves_undecided, b.moves_undecided);
    assert_eq!(a.moves_decision_rejected, b.moves_decision_rejected);
    assert_eq!(a.moves_late, b.moves_late);
    assert_eq!(a.moves_rollback_lost_late, b.moves_rollback_lost_late);
    assert_eq!(a.moves_abandoned_ended, b.moves_abandoned_ended);
    assert_eq!(a.moves_abandoned_restarted, b.moves_abandoned_restarted);
    assert_eq!(a.moves_abandoned_resync, b.moves_abandoned_resync);
    assert_eq!(a.moves_abandoned_demoted, b.moves_abandoned_demoted);
    assert_eq!(a.moves_abandoned_numerical, b.moves_abandoned_numerical);
    assert_eq!(a.move_no_snapshot, b.move_no_snapshot);
    assert_eq!(a.move_widen_failed, b.move_widen_failed);
    assert_eq!(a.moves_internal_refused, b.moves_internal_refused);
    assert_eq!(a.moves_peak_active, b.moves_peak_active);
    assert_f64_bits(a.nis_sum, b.nis_sum);
    assert_eq!(a.nis_n, b.nis_n);
}

fn assert_f32_bits(a: f32, b: f32) {
    assert_eq!(a.to_bits(), b.to_bits());
}

fn assert_f64_bits(a: f64, b: f64) {
    assert_eq!(a.to_bits(), b.to_bits());
}

fn assert_filter_bits(a: &Filter, b: &Filter) {
    for model in 0..2 {
        for axis in 0..5 {
            assert_f64_bits(a.h.m[model].x[axis], b.h.m[model].x[axis]);
            for other in 0..5 {
                assert_f64_bits(a.h.m[model].p[axis][other], b.h.m[model].p[axis][other]);
            }
        }
        assert_f64_bits(a.h.mu[model], b.h.mu[model]);
        for axis in 0..2 {
            assert_f64_bits(a.v.m[model].x[axis], b.v.m[model].x[axis]);
        }
        for covariance in 0..3 {
            assert_f64_bits(a.v.m[model].p[covariance], b.v.m[model].p[covariance]);
        }
        assert_f64_bits(a.v.mu[model], b.v.mu[model]);
    }
    for axis in 0..2 {
        assert_f64_bits(a.trig[axis], b.trig[axis]);
    }
    assert_eq!(a.t_ns, b.t_ns);
    assert_eq!(a.lat_e7, b.lat_e7);
    assert_eq!(a.lon_e7, b.lon_e7);
    assert_eq!(a.rej_t_ns, b.rej_t_ns);
    assert_eq!(a.rej_lat_e7, b.rej_lat_e7);
    assert_eq!(a.rej_lon_e7, b.rej_lon_e7);
    assert_eq!(a.rej_h, b.rej_h);
    assert_eq!(a.rej_c, b.rej_c);
    assert_eq!(a.reacq, b.reacq);
    assert_eq!(a.rej_v, b.rej_v);
    assert_eq!(a.has_alt, b.has_alt);
    assert_eq!(a.h_mode, b.h_mode);
}

fn assert_measurement_bits(a: &Measurement, b: &Measurement) {
    for i in 0..3 {
        assert_f32_bits(a.pos_cov_m2[i], b.pos_cov_m2[i]);
        assert_f32_bits(a.vel_cov_m2s2[i], b.vel_cov_m2s2[i]);
    }
    assert_f32_bits(a.vel_east_mps, b.vel_east_mps);
    assert_f32_bits(a.vel_north_mps, b.vel_north_mps);
    assert_eq!(a.toa_ns, b.toa_ns);
    assert_eq!(a.lat_e7, b.lat_e7);
    assert_eq!(a.lon_e7, b.lon_e7);
    assert_eq!(a.alt_ft, b.alt_ft);
    assert_eq!(a.vrate_fpm, b.vrate_fpm);
    assert_eq!(a.addr, b.addr);
    assert_eq!(a.alt_quant_ft, b.alt_quant_ft);
    assert_eq!(a.sensor, b.sensor);
    assert_eq!(a.ident, b.ident);
    assert_eq!(a.flags, b.flags);
    assert_eq!(a.category, b.category);
}

fn assert_output_row(out: &TrackOutput, i: usize, row: &OutRow) {
    assert_eq!(out.t_ns[i], row.t_ns);
    assert_eq!(out.addr[i], row.addr);
    assert_eq!(out.lat_e7[i], row.lat_e7);
    assert_eq!(out.lon_e7[i], row.lon_e7);
    assert_f32_bits(out.vel_east_mps[i], row.vel_east_mps);
    assert_f32_bits(out.vel_north_mps[i], row.vel_north_mps);
    for j in 0..3 {
        assert_f32_bits(out.pos_cov_m2[i][j], row.pos_cov_m2[j]);
        assert_f32_bits(out.vel_cov_m2s2[i][j], row.vel_cov_m2s2[j]);
    }
    assert_f32_bits(out.turn_rate_rps[i], row.turn_rate_rps);
    assert_f32_bits(out.alt_ft[i], row.alt_ft);
    assert_f32_bits(out.alt_var_ft2[i], row.alt_var_ft2);
    assert_f32_bits(out.vrate_fpm[i], row.vrate_fpm);
    for j in 0..2 {
        assert_f32_bits(out.mu_h[i][j], row.mu_h[j]);
        assert_f32_bits(out.mu_v[i][j], row.mu_v[j]);
    }
    assert_eq!(out.flags[i], row.flags);
}

fn assert_output_txn_eq(a: &OutputTxn, b: &OutputTxn) {
    assert_eq!(a.overflow, b.overflow);
    for i in 0..15 {
        assert_eq!(a.lens[i], b.lens[i]);
    }
}

fn assert_output_len(out: &TrackOutput, n: usize) {
    assert_eq!(out.t_ns.len(), n);
    assert_eq!(out.addr.len(), n);
    assert_eq!(out.lat_e7.len(), n);
    assert_eq!(out.lon_e7.len(), n);
    assert_eq!(out.vel_east_mps.len(), n);
    assert_eq!(out.vel_north_mps.len(), n);
    assert_eq!(out.pos_cov_m2.len(), n);
    assert_eq!(out.vel_cov_m2s2.len(), n);
    assert_eq!(out.turn_rate_rps.len(), n);
    assert_eq!(out.alt_ft.len(), n);
    assert_eq!(out.alt_var_ft2.len(), n);
    assert_eq!(out.vrate_fpm.len(), n);
    assert_eq!(out.mu_h.len(), n);
    assert_eq!(out.mu_v.len(), n);
    assert_eq!(out.flags.len(), n);
}

fn assert_track_restore_stats(before: &TrackerStats, after: &TrackerStats) {
    let mut expected = *before;
    expected.numerical_refused = expected.numerical_refused.saturating_add(1);
    assert_stats_eq(after, &expected);
}

// REQ: OA-TRK-334
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_applied_or_resynced)]
#[kani::stub(filter_output, return_row)]
fn confirmed_applied_or_resynced_attempts_one_current_report_row() {
    let mut t = proof_tracker(CONFIRMED);
    let mut out = crate::types::output_new(1);
    let m = proof_measurement();
    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);

    assert_eq!(t.stats.applied, 1);
    assert_eq!(t.stats.rows_computed, 1);
    assert_eq!(out.t_ns.len(), 1);
    assert_output_len(&out, 1);
    assert_eq!(out.t_ns[0], m.toa_ns);
    assert_eq!(out.addr[0], PROOF_ADDR);
    assert_eq!(out.flags[0] & O_TICK, 0);
    kani::cover!(out.overflow == 0, "report row stored");
    kani::cover!(t.stats.resyncs == 0, "applied dispatch");
    kani::cover!(t.stats.resyncs == 1, "resynced dispatch");
}

// REQ: OA-TRK-334
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_applied_or_resynced)]
#[kani::stub(filter_output, return_current_unrepresentable)]
fn confirmed_applied_or_resynced_counts_unrepresentable_current_row() {
    let mut t = proof_tracker(CONFIRMED);
    let mut out = crate::types::output_new(1);
    let m = fixed_proof_measurement();
    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);

    assert_eq!(t.stats.applied, 1);
    assert_eq!(t.stats.rows_computed, 1);
    assert_eq!(t.stats.rows_unrepresentable, 1);
    assert_output_len(&out, 0);
    assert_eq!(out.overflow, 0);
    kani::cover!(t.stats.rows_unrepresentable == 1, "unrepresentable output");
    kani::cover!(t.stats.resyncs == 0, "applied unrepresentable output");
    kani::cover!(t.stats.resyncs == 1, "resynced unrepresentable output");
}

// REQ: OA-TRK-334
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_applied_or_resynced)]
#[kani::stub(filter_output, return_current_row)]
fn confirmed_applied_or_resynced_counts_output_overflow() {
    let mut t = proof_tracker(CONFIRMED);
    let mut out = crate::types::output_new(1);
    output_push(&mut out, &output_row(PROOF_TIME - 1));
    let before_output = output_txn(&out);
    let sentinel = output_row(PROOF_TIME - 1);
    let m = fixed_proof_measurement();
    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);

    assert_eq!(t.stats.rows_computed, 1);
    let mut expected_output = before_output;
    expected_output.overflow = expected_output.overflow.saturating_add(1);
    assert_output_txn_eq(&expected_output, &output_txn(&out));
    assert_eq!(out.overflow, 1);
    assert_output_row(&out, 0, &sentinel);
    kani::cover!(out.overflow == 1, "full output table");
    kani::cover!(t.stats.resyncs == 0, "applied overflow output");
    kani::cover!(t.stats.resyncs == 1, "resynced overflow output");
}

// REQ: OA-TRK-334
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_rejected)]
#[kani::stub(filter_output, return_row)]
fn confirmed_rejected_update_does_not_attempt_report_output() {
    let mut t = proof_tracker(CONFIRMED);
    let mut out = crate::types::output_new(1);
    let m = proof_measurement();
    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);
    assert_eq!(t.stats.rows_computed, 0);
    assert_eq!(t.stats.applied, 0);
    assert_output_len(&out, 0);
}

// REQ: OA-TRK-334
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_applied_or_resynced)]
#[kani::stub(filter_output, return_row)]
fn unconfirmed_applied_or_resynced_update_does_not_attempt_report_output() {
    let mut t = proof_tracker(TENTATIVE);
    let mut out = crate::types::output_new(1);
    let m = proof_measurement();
    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);
    assert_eq!(t.stats.rows_computed, 0);
    assert_eq!(t.stats.applied, 1);
    assert_output_len(&out, 0);
}

// REQ: OA-TRK-334
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_begun)]
#[kani::stub(filter_output, return_row)]
fn unconfirmed_begun_update_does_not_attempt_report_output() {
    let mut t = proof_tracker(TENTATIVE);
    let mut out = crate::types::output_new(1);
    let m = proof_measurement();
    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);
    assert_eq!(t.stats.rows_computed, 0);
    assert_eq!(t.stats.applied, 0);
    assert_output_len(&out, 0);
}

// REQ: OA-TRK-334
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_begun)]
#[kani::stub(filter_output, return_row)]
fn confirmed_begun_update_attempts_one_current_report_row() {
    let mut t = proof_tracker(CONFIRMED);
    let mut out = crate::types::output_new(1);
    let m = fixed_proof_measurement();
    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);
    assert_eq!(t.stats.rows_computed, 1);
    assert_eq!(t.stats.applied, 0);
    assert_eq!(t.last_ns[PROOF_SLOT], m.toa_ns);
    assert_output_len(&out, 1);
    assert_eq!(out.t_ns[0], m.toa_ns);
    assert_eq!(out.addr[0], PROOF_ADDR);
}

// REQ: OA-TRK-334
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_symbolic_yes_or_resync)]
#[kani::stub(filter_output, return_current_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn actual_decide_commits_applied_or_resynced_report_and_emits_current_time() {
    let mut t = proof_tracker(CONFIRMED);
    let mut out = crate::types::output_new(1);
    let m = fixed_proof_measurement();
    assert_eq!(ring_len(&t, PROOF_SLOT), 0);

    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);

    assert_eq!(t.last_ns[PROOF_SLOT], m.toa_ns);
    assert_eq!(t.ring_len[PROOF_SLOT], 1);
    assert_eq!(t.ring_head[PROOF_SLOT], 0);
    assert_eq!(t.updates[PROOF_SLOT], 1);
    assert_eq!(t.hits[PROOF_SLOT], 1);
    assert_eq!(t.cur[PROOF_SLOT].t_ns, m.toa_ns);
    assert_eq!(t.anchor[PROOF_SLOT].t_ns, m.toa_ns);
    assert_f64_bits(t.cur[PROOF_SLOT].h.m[0].x[0], 321.0);
    assert_f64_bits(t.anchor[PROOF_SLOT].h.m[0].x[0], 321.0);
    assert_measurement_bits(&t.ring_m[ring_ix(&t, PROOF_SLOT, 0)], &m);
    assert_eq!(t.stats.applied, 1);
    assert!(t.stats.resyncs <= 1);
    assert_eq!(out.t_ns.len(), 1);
    assert_output_len(&out, 1);
    assert_eq!(out.t_ns[0], m.toa_ns);
    assert_eq!(out.addr[0], PROOF_ADDR);
    kani::cover!(t.stats.resyncs == 0, "actual Applied decision commits");
    kani::cover!(t.stats.resyncs == 1, "actual Resynced decision commits");
}

fn decide_numerical_refusal_without_mutation(_: &mut Tracker, _: usize, _: &Measurement) -> Step {
    Step::NumericalRefused
}

struct LateReplayCase {
    tracker: Tracker,
    output: TrackOutput,
    before_track: TrackTxn,
    after_track: TrackTxn,
    before_output: OutputTxn,
    sentinel: OutRow,
    capacity: usize,
}

// Independent proof observation: read each owned column directly, without production
// track_txn, so capture defects cannot be shared by the expected and observed snapshots.
fn observe_track_fields_directly(t: &Tracker, slot: usize) -> TrackTxn {
    let start = slot * RING_DEPTH;
    let mut ring = [blank_measurement(); RING_DEPTH];
    for k in 0..RING_DEPTH {
        ring[k] = t.ring_m[start + k];
    }
    TrackTxn {
        cur: t.cur[slot],
        anchor: t.anchor[slot],
        ring,
        stats: t.stats,
        first_ns: t.first_ns[slot],
        confirmed_ns: t.confirmed_ns[slot],
        last_ns: t.last_ns[slot],
        disagree_until_ns: t.disagree_until_ns[slot],
        expected_ns: t.expected_ns[slot],
        newest_update_ns: t.newest_update_ns,
        cat: t.cat[slot],
        updates: t.updates[slot],
        state: t.state[slot],
        hits: t.hits[slot],
        ring_len: t.ring_len[slot],
        ring_head: t.ring_head[slot],
    }
}

// Shared actual-caller scenario for functional and reachability obligations.
#[inline(always)]
fn run_late_replay_case(capacity: usize) -> LateReplayCase {
    let mut t = late_proof_tracker();
    let mut out = crate::types::output_new(capacity);
    let sentinel = output_row(PROOF_TIME - 30);
    output_push(&mut out, &sentinel);
    let before_output = output_txn(&out);
    let before_track = observe_track_fields_directly(&t, PROOF_SLOT);
    let m = late_proof_measurement();

    update_late(&mut t, PROOF_SLOT, &m, &mut out);
    let actual_capacity = out.capacity;
    let after_track = observe_track_fields_directly(&t, PROOF_SLOT);

    LateReplayCase {
        tracker: t,
        output: out,
        before_track,
        after_track,
        before_output,
        sentinel,
        capacity: actual_capacity,
    }
}

fn assert_late_current_filter_projection(before: &TrackTxn, after: &TrackTxn) {
    assert_filter_bits(&before.cur, &after.cur);
}

fn assert_late_anchor_filter_projection(before: &TrackTxn, after: &TrackTxn) {
    assert_filter_bits(&before.anchor, &after.anchor);
}

fn assert_late_ring_projection(before: &TrackTxn, after: &TrackTxn) {
    for k in 0..RING_DEPTH {
        assert_measurement_bits(&before.ring[k], &after.ring[k]);
    }
    assert_eq!(before.ring_len, after.ring_len);
    assert_eq!(before.ring_head, after.ring_head);
}

fn assert_late_metadata_projection(before: &TrackTxn, after: &TrackTxn) {
    assert_eq!(before.first_ns, after.first_ns);
    assert_eq!(before.confirmed_ns, after.confirmed_ns);
    assert_eq!(before.last_ns, after.last_ns);
    assert_eq!(before.disagree_until_ns, after.disagree_until_ns);
    assert_eq!(before.expected_ns, after.expected_ns);
    assert_eq!(before.newest_update_ns, after.newest_update_ns);
    assert_eq!(before.cat.trusted, after.cat.trusted);
    assert_eq!(before.cat.cand, after.cat.cand);
    assert_eq!(before.cat.cand_n, after.cat.cand_n);
    assert_eq!(before.updates, after.updates);
    assert_eq!(before.state, after.state);
    assert_eq!(before.hits, after.hits);
}

fn assert_late_stats_projection(before: &TrackTxn, after: &TrackTxn) {
    let mut expected = before.stats;
    expected.numerical_refused = expected.numerical_refused.saturating_add(1);
    assert_stats_eq(&after.stats, &expected);
}

fn assert_late_output_projection(case: &LateReplayCase) {
    assert_output_txn_eq(&case.before_output, &output_txn(&case.output));
    assert_eq!(case.output.capacity, case.capacity);
    assert_output_len(&case.output, 1);
    assert_output_row(&case.output, 0, &case.sentinel);
}

// Full snapshot postcondition is the conjunction of the six named projections above.
#[inline(always)]
fn prove_late_replay_rollback_for_capacity(capacity: usize) -> usize {
    let case = run_late_replay_case(capacity);
    assert_late_current_filter_projection(&case.before_track, &case.after_track);
    assert_late_anchor_filter_projection(&case.before_track, &case.after_track);
    assert_late_ring_projection(&case.before_track, &case.after_track);
    assert_late_metadata_projection(&case.before_track, &case.after_track);
    assert_late_stats_projection(&case.before_track, &case.after_track);
    assert_late_output_projection(&case);
    case.capacity
}

// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_numerical_refusal_without_mutation)]
fn numerical_refusal_dispatch_routes_and_accounts_once() {
    let mut t = proof_tracker(CONFIRMED);
    t.stats.numerical_refused = kani::any();
    let before = t.stats.numerical_refused;
    let mut out = crate::types::output_new(1);
    let m = proof_measurement();

    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);

    assert_eq!(t.stats.numerical_refused, before.saturating_add(1));
    assert_eq!(t.stats.rows_computed, 0);
    assert_output_len(&out, 0);
    kani::cover!(before == 0, "first numerical refusal counted");
    kani::cover!(before == u64::MAX, "numerical refusal counter saturates");
}

// Component restoration evidence; this is not the combined rollback theorem.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
fn restore_track_transaction_restores_current_filter() {
    let mut t = proof_tracker(CONFIRMED);
    t.cur[PROOF_SLOT] = arbitrary_filter();
    let expected = t.cur[PROOF_SLOT];
    let snapshot = track_txn(&t, PROOF_SLOT);
    let dirty = arbitrary_filter();
    t.cur[PROOF_SLOT] = dirty;
    kani::cover!(
        t.cur[PROOF_SLOT].h.m[0].x[0].to_bits() != expected.h.m[0].x[0].to_bits(),
        "current filter differs before restoration"
    );

    restore_track_txn(&mut t, PROOF_SLOT, snapshot);

    assert_filter_bits(&t.cur[PROOF_SLOT], &expected);
}

// Component restoration evidence; this is not the combined rollback theorem.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
fn restore_track_transaction_restores_anchor_filter() {
    let mut t = proof_tracker(CONFIRMED);
    t.anchor[PROOF_SLOT] = arbitrary_filter();
    let expected = t.anchor[PROOF_SLOT];
    let snapshot = track_txn(&t, PROOF_SLOT);
    let dirty = arbitrary_filter();
    t.anchor[PROOF_SLOT] = dirty;
    kani::cover!(
        t.anchor[PROOF_SLOT].h.m[0].x[0].to_bits() != expected.h.m[0].x[0].to_bits(),
        "anchor filter differs before restoration"
    );

    restore_track_txn(&mut t, PROOF_SLOT, snapshot);

    assert_filter_bits(&t.anchor[PROOF_SLOT], &expected);
}

// Component restoration evidence; this is not the combined rollback theorem.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
fn restore_track_transaction_restores_ring_history() {
    let mut t = proof_tracker(CONFIRMED);
    let start = PROOF_SLOT * RING_DEPTH;
    for k in 0..RING_DEPTH {
        t.ring_m[start + k] = arbitrary_measurement();
    }
    let mut expected = [blank_measurement(); RING_DEPTH];
    for k in 0..RING_DEPTH {
        expected[k] = t.ring_m[start + k];
    }
    let snapshot = track_txn(&t, PROOF_SLOT);
    let mut dirty = [blank_measurement(); RING_DEPTH];
    for item in dirty.iter_mut() {
        *item = arbitrary_measurement();
    }
    for (k, item) in dirty.into_iter().enumerate() {
        t.ring_m[start + k] = item;
    }
    kani::cover!(
        t.ring_m[start].vel_east_mps.to_bits() != expected[0].vel_east_mps.to_bits(),
        "ring history differs before restoration"
    );
    t.ring_len[PROOF_SLOT] = kani::any();
    t.ring_head[PROOF_SLOT] = kani::any();

    restore_track_txn(&mut t, PROOF_SLOT, snapshot);

    for k in 0..RING_DEPTH {
        assert_measurement_bits(&t.ring_m[start + k], &expected[k]);
    }
}

// Component restoration evidence; this is not the combined rollback theorem.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
fn restore_track_transaction_restores_metadata_and_category() {
    let mut t = proof_tracker(CONFIRMED);
    t.first_ns[PROOF_SLOT] = kani::any();
    t.confirmed_ns[PROOF_SLOT] = kani::any();
    t.last_ns[PROOF_SLOT] = kani::any();
    t.disagree_until_ns[PROOF_SLOT] = kani::any();
    t.expected_ns[PROOF_SLOT] = kani::any();
    t.updates[PROOF_SLOT] = kani::any();
    t.state[PROOF_SLOT] = kani::any();
    t.hits[PROOF_SLOT] = kani::any();
    t.ring_len[PROOF_SLOT] = kani::any();
    t.ring_head[PROOF_SLOT] = kani::any();
    t.cat[PROOF_SLOT] = arbitrary_category_state();
    t.newest_update_ns = kani::any();
    let expected_first_ns = t.first_ns[PROOF_SLOT];
    let expected_confirmed_ns = t.confirmed_ns[PROOF_SLOT];
    let expected_last_ns = t.last_ns[PROOF_SLOT];
    let expected_disagree_until_ns = t.disagree_until_ns[PROOF_SLOT];
    let expected_expected_ns = t.expected_ns[PROOF_SLOT];
    let expected_newest_update_ns = t.newest_update_ns;
    let expected_updates = t.updates[PROOF_SLOT];
    let expected_state = t.state[PROOF_SLOT];
    let expected_hits = t.hits[PROOF_SLOT];
    let expected_ring_len = t.ring_len[PROOF_SLOT];
    let expected_ring_head = t.ring_head[PROOF_SLOT];
    let expected_cat = t.cat[PROOF_SLOT];
    let snapshot = track_txn(&t, PROOF_SLOT);
    t.first_ns[PROOF_SLOT] = kani::any();
    t.confirmed_ns[PROOF_SLOT] = kani::any();
    t.last_ns[PROOF_SLOT] = kani::any();
    t.disagree_until_ns[PROOF_SLOT] = kani::any();
    t.expected_ns[PROOF_SLOT] = kani::any();
    t.updates[PROOF_SLOT] = kani::any();
    t.state[PROOF_SLOT] = kani::any();
    t.hits[PROOF_SLOT] = kani::any();
    t.ring_len[PROOF_SLOT] = kani::any();
    t.ring_head[PROOF_SLOT] = kani::any();
    t.cat[PROOF_SLOT] = arbitrary_category_state();
    t.newest_update_ns = kani::any();
    kani::cover!(
        t.first_ns[PROOF_SLOT] != expected_first_ns,
        "metadata differs before restoration"
    );

    restore_track_txn(&mut t, PROOF_SLOT, snapshot);

    let after = track_txn(&t, PROOF_SLOT);
    assert_eq!(after.first_ns, expected_first_ns);
    assert_eq!(after.confirmed_ns, expected_confirmed_ns);
    assert_eq!(after.last_ns, expected_last_ns);
    assert_eq!(after.disagree_until_ns, expected_disagree_until_ns);
    assert_eq!(after.expected_ns, expected_expected_ns);
    assert_eq!(after.newest_update_ns, expected_newest_update_ns);
    assert_eq!(after.updates, expected_updates);
    assert_eq!(after.state, expected_state);
    assert_eq!(after.hits, expected_hits);
    assert_eq!(after.ring_len, expected_ring_len);
    assert_eq!(after.ring_head, expected_ring_head);
    assert_eq!(after.cat.trusted, expected_cat.trusted);
    assert_eq!(after.cat.cand, expected_cat.cand);
    assert_eq!(after.cat.cand_n, expected_cat.cand_n);
}

// Component restoration evidence; this is not the combined rollback theorem.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
fn restore_track_transaction_restores_all_statistics() {
    let mut t = proof_tracker(CONFIRMED);
    t.stats = arbitrary_stats();
    let expected_stats = t.stats;
    let snapshot = track_txn(&t, PROOF_SLOT);
    let dirty = arbitrary_stats();
    t.stats = dirty;
    kani::cover!(
        t.stats.offered != expected_stats.offered,
        "statistics differ before restoration"
    );

    restore_track_txn(&mut t, PROOF_SLOT, snapshot);

    assert_track_restore_stats(&expected_stats, &t.stats);
}

// Component restoration evidence; this is not the combined rollback theorem.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
fn restore_output_transaction_restores_appended_row_and_columns() {
    let mut out = crate::types::output_new(2);
    let sentinel = output_row(PROOF_TIME - 1);
    let appended = output_row(PROOF_TIME);
    output_push(&mut out, &sentinel);
    let snapshot = output_txn(&out);
    output_push(&mut out, &appended);
    assert_eq!(out.t_ns.len(), snapshot.lens[0] + 1);
    assert_eq!(out.t_ns[1], appended.t_ns);

    restore_output_txn(&mut out, snapshot);

    assert_output_txn_eq(&snapshot, &output_txn(&out));
    assert_output_len(&out, 1);
    assert_output_row(&out, 0, &sentinel);
}

// Component restoration evidence; this is not the combined rollback theorem.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
fn restore_output_transaction_restores_overflow_counter() {
    let mut out = crate::types::output_new(1);
    let sentinel = output_row(PROOF_TIME - 1);
    output_push(&mut out, &sentinel);
    out.overflow = kani::any();
    let snapshot = output_txn(&out);
    output_push(&mut out, &output_row(PROOF_TIME));
    assert_eq!(out.overflow, snapshot.overflow.saturating_add(1));

    restore_output_txn(&mut out, snapshot);

    assert_output_txn_eq(&snapshot, &output_txn(&out));
    assert_output_len(&out, 1);
    assert_output_row(&out, 0, &sentinel);
    kani::cover!(snapshot.overflow == 0, "zero output overflow restored");
    kani::cover!(
        snapshot.overflow == u64::MAX,
        "saturated output overflow restored"
    );
}

// All-state rollback remains an open resource-limited obligation.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_numerical_refusal)]
fn numerical_refusal_restores_track_and_output_transaction_state() {
    let mut t = proof_tracker(CONFIRMED);
    t.stats.numerical_refused = kani::any();
    let m = proof_measurement();
    let before_track = track_txn(&t, PROOF_SLOT);

    let mut out = crate::types::output_new(2);
    let sentinel = output_row(PROOF_TIME - 1);
    output_push(&mut out, &sentinel);
    let output_before_append = output_txn(&out);
    output_push(&mut out, &output_row(PROOF_TIME));
    output_push(&mut out, &output_row(PROOF_TIME + 1));
    assert_eq!(out.overflow, output_before_append.overflow + 1);
    restore_output_txn(&mut out, output_before_append);
    assert_output_txn_eq(&output_before_append, &output_txn(&out));
    assert_output_row(&out, 0, &sentinel);

    let before_output = output_txn(&out);
    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);

    let after_track = track_txn(&t, PROOF_SLOT);
    assert_same_track_txn(&before_track, &after_track);
    let mut expected_stats = before_track.stats;
    expected_stats.numerical_refused = expected_stats.numerical_refused.saturating_add(1);
    assert_stats_eq(&t.stats, &expected_stats);

    let after_output = output_txn(&out);
    assert_output_txn_eq(&before_output, &after_output);
    assert_eq!(out.capacity, 2);
    assert_output_row(&out, 0, &sentinel);
}

// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_replay_restores_transaction_after_current_row_then_later_refusal() {
    #[cfg(kani_late_replay_covers)]
    kani::cover!(
        prove_late_replay_rollback_for_capacity(1) == 1,
        "late replay overflow rollback case"
    );
    #[cfg(not(kani_late_replay_covers))]
    assert_eq!(prove_late_replay_rollback_for_capacity(1), 1);
    #[cfg(kani_late_replay_covers)]
    kani::cover!(
        prove_late_replay_rollback_for_capacity(2) == 2,
        "late replay append rollback case"
    );
    #[cfg(not(kani_late_replay_covers))]
    assert_eq!(prove_late_replay_rollback_for_capacity(2), 2);
}

#[path = "tracker_rollback_paths.rs"]
mod rollback_paths;

const OLD_T: i64 = PROOF_TIME - 1_000;
const NEW_T: i64 = PROOF_TIME;

/// A row for the new filter; for the snapshot's (old) filter a row at another position, or none
/// (its position cannot be represented), either way.
fn move_row_or_none(f: &Filter, at_ns: i64) -> Option<OutRow> {
    let mut row = output_row(at_ns);
    if f.t_ns == OLD_T {
        if kani::any() {
            return None;
        }
        row.lat_e7 = 1;
    }
    Some(row)
}

fn decide_moved(t: &mut Tracker, slot: usize, m: &Measurement) -> Step {
    t.cur[slot].t_ns = NEW_T;
    t.last_ns[slot] = m.toa_ns;
    Step::Moved
}

fn any_distance(_: &OutRow, _: &OutRow) -> f64 {
    // A distance is not negative; it can be not a number if a position cannot be placed.
    let d: f64 = kani::any();
    kani::assume(d.is_nan() || d >= 0.0);
    d
}

fn any_radius(_: &[f32; 3]) -> f64 {
    let r: f64 = kani::any();
    kani::assume(r >= 0.0 && !r.is_nan());
    r
}

// What this proves: the production `update_in_order`, given a decision `Moved`, starts the
// track's unconfirmed move (the mark is set by production, not by the harness), and the
// production `emit` marks the row it makes, whether or not the old path's row exists and
// whatever the distance and radii are (stubbed to any value): a row with an old path that
// differs from the new one is widened or left as it is, a row whose widening cannot be computed
// gets the widest covariance and is counted. What it does not prove: the value of the widened
// radius (the sqrt and the scaling; the integration tests check it against the old path).
// REQ: OA-TRK-347
#[kani::proof]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_moved)]
#[kani::stub(filter_output, move_row_or_none)]
#[kani::stub(filter_row_distance_m, any_distance)]
#[kani::stub(r95_m, any_radius)]
fn a_move_made_by_production_marks_the_row_it_emits_and_widens_or_fails_closed() {
    let mut t = proof_tracker_with_pool(CONFIRMED, 1);
    t.cur[PROOF_SLOT].t_ns = OLD_T;
    let mut out = crate::types::output_new(1);
    let m = proof_measurement();
    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);
    assert!(move_active(&t, PROOF_SLOT));
    assert_eq!(t.stats.moves_unconfirmed, 1);
    assert_eq!(out.t_ns.len(), 1);
    assert!(out.flags[0] & O_MOVE_UNCONFIRMED != 0);
    if t.stats.move_widen_failed == 1 {
        assert_eq!(out.pos_cov_m2[0], WIDEST_COV);
    }
    kani::cover!(
        t.stats.move_widen_failed == 1,
        "the widening cannot be computed"
    );
    kani::cover!(
        t.stats.move_widen_failed == 0 && out.pos_cov_m2[0] != [0.0; 3],
        "the row is widened over an old path that differs"
    );
    kani::cover!(
        t.stats.move_widen_failed == 0 && out.pos_cov_m2[0] == [0.0; 3],
        "the row is already wide enough"
    );
}

// What this proves (pure): a report under 0.5 s after the move decides nothing; `Confirm` or
// `Rollback` only for a report at least 0.5 s after the move, within the window, whose distance
// from the moved track (confirm) or the old path (rollback) is within the agreement limit,
// which never passes `confirm_move_m`; Confirm against Rollback: a report nearer the old path
// than the moved track (both within the limit) rolls back, a tie or an unknown old path
// confirms, a report that does not agree with the moved track and is within the limit of the
// old path rolls back, and a report within the limit of neither is undecided.
// REQ: OA-TRK-348
#[kani::proof]
#[kani::solver(cadical)]
fn a_single_far_report_marks_every_row_until_a_second_report_decides() {
    // Any configuration the checker accepts, any times, any distances (so any gap and any
    // covariance behind them: the decision does not see them), any window from the floor up.
    let cfg = TrackerConfig {
        confirm_move_m: kani::any(),
        confirm_agree_m: kani::any(),
        confirm_window_ns: kani::any(),
        gate_manoeuvre_mps2: kani::any(),
        ..crate::types::DEFAULT_CONFIG
    };
    kani::assume(crate::types::check_config(&cfg).is_ok());
    let start_ns: i64 = kani::any();
    let now_ns: i64 = kani::any();
    let window_ns: i64 = kani::any();
    kani::assume(window_ns >= cfg.confirm_window_ns);
    let cur_m: f64 = kani::any();
    let old_m: f64 = kani::any();
    let v = crate::confirm::resolve_decide(start_ns, now_ns, cur_m, old_m, window_ns, &cfg);
    let sep = now_ns.saturating_sub(start_ns);
    // Nothing but a second report, apart from the move and from the window, decides it.
    if v == crate::confirm::Resolve::Wait {
        assert!(sep < crate::params::CONFIRM_MIN_SEP_NS);
    } else {
        assert!(sep >= crate::params::CONFIRM_MIN_SEP_NS);
    }
    if v == crate::confirm::Resolve::Confirm || v == crate::confirm::Resolve::Rollback {
        assert!(sep <= window_ns);
        let near = if v == crate::confirm::Resolve::Confirm {
            cur_m
        } else {
            old_m
        };
        // However long the wait, the agreement never loosens past the fixed move distance.
        assert!(near <= cfg.confirm_move_m);
    }
    // Ordering: Confirm needs the moved track no farther than the old path (or the old path
    // unknown); Rollback needs the old path strictly nearer or the moved track out of reach.
    if v == crate::confirm::Resolve::Confirm {
        assert!(old_m.is_nan() || old_m >= cur_m);
    }
    if v == crate::confirm::Resolve::Rollback {
        assert!(old_m < cur_m || cur_m.is_nan() || cur_m > cfg.confirm_move_m);
    }
    if cur_m == old_m
        && v != crate::confirm::Resolve::Wait
        && v != crate::confirm::Resolve::Undecided
    {
        assert!(v == crate::confirm::Resolve::Confirm);
    }
    kani::cover!(v == crate::confirm::Resolve::Confirm, "confirmed");
    kani::cover!(v == crate::confirm::Resolve::Rollback, "rolled back");
    kani::cover!(v == crate::confirm::Resolve::Undecided, "undecided");
    kani::cover!(v == crate::confirm::Resolve::Wait, "too soon");
    kani::cover!(
        cur_m == old_m && v == crate::confirm::Resolve::Confirm,
        "a tie confirms"
    );
}

fn any_miss(_: &Filter, _: &Measurement) -> f64 {
    kani::any()
}

// What this proves: the production `update_in_order_held` with the decision stubbed to "the
// report is not applied" (rejected) and any distances (so any verdict): the unconfirmed move is
// still there, no outcome is counted (confirmed, rolled back, undecided), and the track's
// columns are exactly what they were before the call (a verdict of rollback is undone). What
// rests on the integration tests: that a report applied by the real filter decides.
// REQ: OA-TRK-348
#[kani::proof]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_rejected)]
#[kani::stub(filter_miss_m, any_miss)]
fn a_report_that_is_not_applied_decides_nothing_whatever_the_verdict() {
    let mut t = proof_tracker_with_pool(CONFIRMED, 1);
    t.cur[PROOF_SLOT].t_ns = OLD_T;
    // An unconfirmed move holding a snapshot, the track since moved.
    t.mv_on[PROOF_SLOT] = 1;
    t.mv_live = 1;
    t.mv_start_ns[PROOF_SLOT] = kani::any();
    t.snaps[0] = snap_take(&t, PROOF_SLOT);
    t.snap_free.clear();
    t.mv_snap[PROOF_SLOT] = 0;
    t.cur[PROOF_SLOT].t_ns = NEW_T;
    t.last_ns[PROOF_SLOT] = kani::any();
    t.updates[PROOF_SLOT] = kani::any();
    let before = snap_take(&t, PROOF_SLOT);
    let mut out = crate::types::output_new(1);
    let m = proof_measurement();
    update_in_order_held(&mut t, PROOF_SLOT, &m, &mut out);
    let after = snap_take(&t, PROOF_SLOT);
    assert!(move_active(&t, PROOF_SLOT));
    assert_eq!(t.mv_snap[PROOF_SLOT], 0);
    assert_eq!(t.stats.moves_confirmed, 0);
    assert_eq!(t.stats.moves_rolled_back, 0);
    assert_eq!(t.stats.moves_undecided, 0);
    assert!(
        after.cur.t_ns == before.cur.t_ns
            && after.last_ns == before.last_ns
            && after.updates == before.updates
            && after.state == before.state
            && after.hits == before.hits
    );
    assert_eq!(out.t_ns.len(), 0);
    kani::cover!(
        t.stats.moves_decision_rejected == 1,
        "a deciding report was refused"
    );
    kani::cover!(t.stats.moves_decision_rejected == 0, "too soon to decide");
}

// What this proves: the rollback of an unconfirmed move (`snap_restore` from the move's
// snapshot, taken from the production capture `track_txn`) puts back the scalar columns, the
// category, and the current filter's time and first state value as they were before the move,
// leaves the columns written from outside the update path (`disagree_until_ns`, `expected_ns`)
// as they are after the move, and leaves the tracker's counters and its newest-update bound
// alone. The whole filters and the ring are restored by the same function and covered by the
// OA-TRK-335 component proofs on `restore_track_txn` (a proof with every float symbolic at once
// does not fit 6 GiB).
// REQ: OA-TRK-349
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
fn rolling_back_restores_measurement_state_and_leaves_the_outside_columns() {
    let mut t = proof_tracker_with_pool(CONFIRMED, 1);
    t.state[PROOF_SLOT] = kani::any();
    t.first_ns[PROOF_SLOT] = kani::any();
    t.confirmed_ns[PROOF_SLOT] = kani::any();
    t.last_ns[PROOF_SLOT] = kani::any();
    t.updates[PROOF_SLOT] = kani::any();
    t.hits[PROOF_SLOT] = kani::any();
    t.cur[PROOF_SLOT].t_ns = kani::any();
    t.cur[PROOF_SLOT].h.m[0].x[0] = kani::any();
    t.ring_len[PROOF_SLOT] = kani::any();
    t.ring_head[PROOF_SLOT] = kani::any();
    t.cat[PROOF_SLOT] = arbitrary_category_state();
    let before = observe_track_fields_directly(&t, PROOF_SLOT);
    let snap = snap_of_txn(&track_txn(&t, PROOF_SLOT));
    t.mv_on[PROOF_SLOT] = 1;
    t.mv_live = 1;
    t.snaps[0] = snap;
    t.snap_free.clear();
    t.mv_snap[PROOF_SLOT] = 0;
    // The move changes every column of the track; the fusion layer writes its two columns.
    t.state[PROOF_SLOT] = kani::any();
    t.first_ns[PROOF_SLOT] = kani::any();
    t.confirmed_ns[PROOF_SLOT] = kani::any();
    t.last_ns[PROOF_SLOT] = kani::any();
    t.updates[PROOF_SLOT] = kani::any();
    t.hits[PROOF_SLOT] = kani::any();
    t.cur[PROOF_SLOT].t_ns = kani::any();
    t.cur[PROOF_SLOT].h.m[0].x[0] = kani::any();
    t.ring_len[PROOF_SLOT] = kani::any();
    t.ring_head[PROOF_SLOT] = kani::any();
    t.cat[PROOF_SLOT] = arbitrary_category_state();
    t.disagree_until_ns[PROOF_SLOT] = kani::any();
    t.expected_ns[PROOF_SLOT] = kani::any();
    let outside = (t.disagree_until_ns[PROOF_SLOT], t.expected_ns[PROOF_SLOT]);
    let newest = t.newest_update_ns;

    // The rollback of `update_in_order_held`, before the report is applied to the restored track.
    let old = t.snaps[0];
    snap_restore(&mut t, PROOF_SLOT, &old);
    end_move(&mut t, PROOF_SLOT, MoveEnd::RolledBack);

    let after = observe_track_fields_directly(&t, PROOF_SLOT);
    // One assertion, not one per field: a proof with a property for every float of the filters
    // and the ring does not fit 6 GiB.
    let same = after.cur.t_ns == before.cur.t_ns
        && after.cur.h.m[0].x[0].to_bits() == before.cur.h.m[0].x[0].to_bits()
        && after.first_ns == before.first_ns
        && after.confirmed_ns == before.confirmed_ns
        && after.last_ns == before.last_ns
        && after.cat.trusted == before.cat.trusted
        && after.cat.cand == before.cat.cand
        && after.cat.cand_n == before.cat.cand_n
        && after.updates == before.updates
        && after.state == before.state
        && after.hits == before.hits
        && after.ring_len == before.ring_len
        && after.ring_head == before.ring_head;
    assert!(same);
    assert_eq!(
        (t.disagree_until_ns[PROOF_SLOT], t.expected_ns[PROOF_SLOT]),
        outside
    );
    assert_eq!(t.newest_update_ns, newest);
    assert!(!move_active(&t, PROOF_SLOT));
    assert_eq!(t.stats.moves_rolled_back, 1);
    // The snapshot is back in the pool.
    assert_eq!(t.snap_free.len(), 1);
}

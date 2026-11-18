//! Projected proofs that the production in-order caller reaches transaction rollback.
//!
//! The decision seam models a refusal after deliberately changing the selected projection.
//! `update_in_order`, transaction capture/restore, and refusal accounting remain production.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;

fn decide_dirty_filter_then_refuse(t: &mut Tracker, slot: usize, _: &Measurement) -> Step {
    let prior_t = t.cur[slot].t_ns;
    t.cur[slot] = arbitrary_filter();
    kani::cover!(
        prior_t != t.cur[slot].t_ns,
        "filter projection dirtied before refusal"
    );
    Step::NumericalRefused
}

fn decide_dirty_metadata_then_refuse(t: &mut Tracker, slot: usize, _: &Measurement) -> Step {
    let prior_last = t.last_ns[slot];
    let prior_state = t.state[slot];
    let prior_cat = t.cat[slot];
    let prior_newest = t.newest_update_ns;
    let prior_ring_len = t.ring_len[slot];
    let prior_ring_head = t.ring_head[slot];
    t.first_ns[slot] = kani::any();
    t.confirmed_ns[slot] = kani::any();
    t.last_ns[slot] = kani::any();
    t.disagree_until_ns[slot] = kani::any();
    t.expected_ns[slot] = kani::any();
    t.updates[slot] = kani::any();
    t.hits[slot] = kani::any();
    t.state[slot] = kani::any();
    t.cat[slot] = arbitrary_category_state();
    t.newest_update_ns = kani::any();
    t.ring_len[slot] = kani::any();
    kani::assume(usize::from(t.ring_len[slot]) <= RING_DEPTH);
    t.ring_head[slot] = kani::any();
    kani::assume(usize::from(t.ring_head[slot]) < RING_DEPTH);
    let cat = t.cat[slot];
    kani::cover!(
        prior_last != t.last_ns[slot]
            || prior_state != t.state[slot]
            || prior_cat.trusted != cat.trusted
            || prior_cat.cand != cat.cand
            || prior_cat.cand_n != cat.cand_n
            || prior_newest != t.newest_update_ns
            || prior_ring_len != t.ring_len[slot]
            || prior_ring_head != t.ring_head[slot],
        "metadata projection dirtied before refusal"
    );
    Step::NumericalRefused
}

fn decide_dirty_anchor_then_refuse(t: &mut Tracker, slot: usize, _: &Measurement) -> Step {
    let prior_t = t.anchor[slot].t_ns;
    t.anchor[slot] = arbitrary_filter();
    kani::cover!(
        prior_t != t.anchor[slot].t_ns,
        "anchor projection dirtied before refusal"
    );
    Step::NumericalRefused
}

fn decide_dirty_ring_then_refuse(t: &mut Tracker, slot: usize, _: &Measurement) -> Step {
    let start = slot * RING_DEPTH;
    let mut changed = false;
    for k in 0..RING_DEPTH {
        let prior = t.ring_m[start + k];
        let dirty = arbitrary_measurement();
        t.ring_m[start + k] = dirty;
        changed |= prior.toa_ns != dirty.toa_ns || prior.sensor != dirty.sensor;
    }
    kani::cover!(changed, "ring history projection dirtied before refusal");
    Step::NumericalRefused
}

fn decide_dirty_stats_then_refuse(t: &mut Tracker, _: usize, _: &Measurement) -> Step {
    let prior_applied = t.stats.applied;
    t.stats = arbitrary_stats();
    kani::cover!(
        prior_applied != t.stats.applied,
        "stats projection dirtied before refusal"
    );
    Step::NumericalRefused
}

// The decision model changes the current filter before refusal.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_dirty_filter_then_refuse)]
fn in_order_refusal_restores_current_filter_projection() {
    let mut t = proof_tracker(CONFIRMED);
    t.cur[PROOF_SLOT] = arbitrary_filter();
    let original = t.cur[PROOF_SLOT];
    let mut out = crate::types::output_new(1);
    let m = fixed_proof_measurement();

    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);

    assert_filter_bits(&t.cur[PROOF_SLOT], &original);
    assert_eq!(t.stats.numerical_refused, 1);
}

// This checks all transaction metadata through the production caller. Storage is the valid
// initialized tracker allocation; ring length/head remain within their declared bounds.
// The modeled refusal is the only dispatch premise; this is not a whole-tracker validity claim.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_dirty_metadata_then_refuse)]
fn in_order_refusal_restores_track_metadata_projection() {
    let mut t = proof_tracker(CONFIRMED);
    t.state[PROOF_SLOT] = kani::any();
    t.first_ns[PROOF_SLOT] = kani::any();
    t.confirmed_ns[PROOF_SLOT] = kani::any();
    t.last_ns[PROOF_SLOT] = kani::any();
    t.disagree_until_ns[PROOF_SLOT] = kani::any();
    t.expected_ns[PROOF_SLOT] = kani::any();
    t.updates[PROOF_SLOT] = kani::any();
    t.hits[PROOF_SLOT] = kani::any();
    t.cat[PROOF_SLOT] = arbitrary_category_state();
    t.newest_update_ns = kani::any();
    t.ring_len[PROOF_SLOT] = kani::any();
    kani::assume(usize::from(t.ring_len[PROOF_SLOT]) <= RING_DEPTH);
    t.ring_head[PROOF_SLOT] = kani::any();
    kani::assume(usize::from(t.ring_head[PROOF_SLOT]) < RING_DEPTH);
    let first = t.first_ns[PROOF_SLOT];
    let confirmed = t.confirmed_ns[PROOF_SLOT];
    let last = t.last_ns[PROOF_SLOT];
    let disagree = t.disagree_until_ns[PROOF_SLOT];
    let expected = t.expected_ns[PROOF_SLOT];
    let updates = t.updates[PROOF_SLOT];
    let hits = t.hits[PROOF_SLOT];
    let state = t.state[PROOF_SLOT];
    let cat = t.cat[PROOF_SLOT];
    let newest = t.newest_update_ns;
    let ring_len = t.ring_len[PROOF_SLOT];
    let ring_head = t.ring_head[PROOF_SLOT];
    let mut out = crate::types::output_new(1);
    let m = fixed_proof_measurement();

    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);

    assert_eq!(t.first_ns[PROOF_SLOT], first);
    assert_eq!(t.confirmed_ns[PROOF_SLOT], confirmed);
    assert_eq!(t.last_ns[PROOF_SLOT], last);
    assert_eq!(t.disagree_until_ns[PROOF_SLOT], disagree);
    assert_eq!(t.expected_ns[PROOF_SLOT], expected);
    assert_eq!(t.updates[PROOF_SLOT], updates);
    assert_eq!(t.hits[PROOF_SLOT], hits);
    assert_eq!(t.state[PROOF_SLOT], state);
    assert_eq!(t.cat[PROOF_SLOT].trusted, cat.trusted);
    assert_eq!(t.cat[PROOF_SLOT].cand, cat.cand);
    assert_eq!(t.cat[PROOF_SLOT].cand_n, cat.cand_n);
    assert_eq!(t.newest_update_ns, newest);
    assert_eq!(t.ring_len[PROOF_SLOT], ring_len);
    assert_eq!(t.ring_head[PROOF_SLOT], ring_head);
}

// This checks the anchor filter as an independent transaction projection.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_dirty_anchor_then_refuse)]
fn in_order_refusal_restores_anchor_filter_projection() {
    let mut t = proof_tracker(CONFIRMED);
    t.anchor[PROOF_SLOT] = arbitrary_filter();
    let original = t.anchor[PROOF_SLOT];
    let mut out = crate::types::output_new(1);
    let m = fixed_proof_measurement();

    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);

    assert_filter_bits(&t.anchor[PROOF_SLOT], &original);
    assert_eq!(t.stats.numerical_refused, 1);
}

// All twelve fixed ring entries are independently symbolic in the retained snapshot.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_dirty_ring_then_refuse)]
fn in_order_refusal_restores_all_ring_entry_projection() {
    let mut t = proof_tracker(CONFIRMED);
    let start = PROOF_SLOT * RING_DEPTH;
    let mut original = [blank_measurement(); RING_DEPTH];
    for k in 0..RING_DEPTH {
        t.ring_m[start + k] = arbitrary_measurement();
        original[k] = t.ring_m[start + k];
    }
    let mut out = crate::types::output_new(1);
    let m = fixed_proof_measurement();

    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);

    for k in 0..RING_DEPTH {
        assert_measurement_bits(&t.ring_m[start + k], &original[k]);
    }
    assert_eq!(t.stats.numerical_refused, 1);
}

// Stats are independently symbolic; only the production refusal counter increment is allowed.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(decide, decide_dirty_stats_then_refuse)]
fn in_order_refusal_restores_stats_projection_except_refusal_count() {
    let mut t = proof_tracker(CONFIRMED);
    t.stats = arbitrary_stats();
    let original = t.stats;
    let mut expected = original;
    expected.numerical_refused = expected.numerical_refused.saturating_add(1);
    let mut out = crate::types::output_new(1);
    let m = fixed_proof_measurement();

    update_in_order(&mut t, PROOF_SLOT, &m, &mut out);

    assert_stats_eq(&t.stats, &expected);
}

// Each output capacity gets a separate bounded replay execution.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_replay_restores_transaction_after_overflow_before_refusal() {
    let capacity = prove_late_replay_rollback_for_capacity(1);
    assert_eq!(capacity, 1);
}

// This separately covers append followed by rollback.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_replay_restores_transaction_after_append_before_refusal() {
    let capacity = prove_late_replay_rollback_for_capacity(2);
    assert_eq!(capacity, 2);
}

fn assert_late_refusal_count(case: &LateReplayCase) {
    assert_eq!(
        case.tracker.stats.numerical_refused,
        case.before_track.stats.numerical_refused.saturating_add(1)
    );
}

// Reachability-only obligations share the exact production replay scenario and stubs.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_replay_overflow_reachability() {
    let case = run_late_replay_case(1);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 1);
    kani::cover!(case.capacity == 1, "late replay overflow rollback case");
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
fn late_replay_append_reachability() {
    let case = run_late_replay_case(2);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 2);
    kani::cover!(case.capacity == 2, "late replay append rollback case");
}

// Preserves the shared late-replay scenario and checks only the current postcondition.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_overflow_current_filter_projection() {
    let case = run_late_replay_case(1);
    let after = &case.after_track;
    assert_late_current_filter_projection(&case.before_track, after);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 1);
}

// Preserves the shared late-replay scenario and checks only the anchor postcondition.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_overflow_anchor_filter_projection() {
    let case = run_late_replay_case(1);
    let after = &case.after_track;
    assert_late_anchor_filter_projection(&case.before_track, after);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 1);
}

// Preserves the shared late-replay scenario and checks only the ring postcondition.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_overflow_ring_projection() {
    let case = run_late_replay_case(1);
    let after = &case.after_track;
    assert_late_ring_projection(&case.before_track, after);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 1);
}

// Preserves the shared late-replay scenario and checks only the metadata postcondition.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_overflow_metadata_projection() {
    let case = run_late_replay_case(1);
    let after = &case.after_track;
    assert_late_metadata_projection(&case.before_track, after);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 1);
}

// Preserves the shared late-replay scenario and checks only the stats postcondition.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_overflow_stats_projection() {
    let case = run_late_replay_case(1);
    let after = &case.after_track;
    assert_late_stats_projection(&case.before_track, after);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 1);
}

// Preserves the shared late-replay scenario and checks only the current postcondition.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_append_current_filter_projection() {
    let case = run_late_replay_case(2);
    let after = &case.after_track;
    assert_late_current_filter_projection(&case.before_track, after);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 2);
}

// Preserves the shared late-replay scenario and checks only the anchor postcondition.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_append_anchor_filter_projection() {
    let case = run_late_replay_case(2);
    let after = &case.after_track;
    assert_late_anchor_filter_projection(&case.before_track, after);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 2);
}

// Preserves the shared late-replay scenario and checks only the ring postcondition.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_append_ring_projection() {
    let case = run_late_replay_case(2);
    let after = &case.after_track;
    assert_late_ring_projection(&case.before_track, after);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 2);
}

// Preserves the shared late-replay scenario and checks only the metadata postcondition.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_append_metadata_projection() {
    let case = run_late_replay_case(2);
    let after = &case.after_track;
    assert_late_metadata_projection(&case.before_track, after);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 2);
}

// Preserves the shared late-replay scenario and checks only the stats postcondition.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_append_stats_projection() {
    let case = run_late_replay_case(2);
    let after = &case.after_track;
    assert_late_stats_projection(&case.before_track, after);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 2);
}

// Preserves the shared late-replay scenario and checks only the output postcondition.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_overflow_output_projection() {
    let case = run_late_replay_case(1);
    assert_late_output_projection(&case);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 1);
}

// Preserves the shared late-replay scenario and checks only the output postcondition.
// REQ: OA-TRK-335
#[kani::proof]
#[kani::unwind(16)]
#[kani::solver(cadical)]
#[kani::stub(filter_apply, filter_apply_insert_then_refuse)]
#[kani::stub(filter_output, return_late_row)]
#[kani::stub(filter_replay, filter_replay_must_be_unreachable)]
#[kani::stub(implausible, implausible_must_be_unreachable)]
#[kani::stub(track_begin, track_begin_must_be_unreachable)]
fn late_append_output_projection() {
    let case = run_late_replay_case(2);
    assert_late_output_projection(&case);
    assert_late_refusal_count(&case);
    assert_eq!(case.capacity, 2);
}

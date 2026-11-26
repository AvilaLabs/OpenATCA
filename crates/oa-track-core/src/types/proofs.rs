//! Bounded Kani contracts for the real output-table append and clear operations.
//!
//! These harnesses cover one- and two-row tables and sequences of at most two pushes. They do
//! not claim an unbounded proof over arbitrary table capacities or push sequences.

use super::{output_clear, output_len, output_new, output_push, OutRow, TrackOutput};

fn any_row() -> OutRow {
    // `kani::any` leaves each float as an unconstrained f32 bit-vector, including NaN payloads.
    OutRow {
        t_ns: kani::any(),
        addr: kani::any(),
        lat_e7: kani::any(),
        lon_e7: kani::any(),
        vel_east_mps: kani::any(),
        vel_north_mps: kani::any(),
        pos_cov_m2: kani::any(),
        vel_cov_m2s2: kani::any(),
        turn_rate_rps: kani::any(),
        alt_ft: kani::any(),
        alt_var_ft2: kani::any(),
        vrate_fpm: kani::any(),
        mu_h: kani::any(),
        mu_v: kani::any(),
        flags: kani::any(),
    }
}

fn assert_row_at(o: &TrackOutput, i: usize, r: &OutRow) {
    assert_eq!(o.t_ns[i], r.t_ns);
    assert_eq!(o.addr[i], r.addr);
    assert_eq!(o.lat_e7[i], r.lat_e7);
    assert_eq!(o.lon_e7[i], r.lon_e7);
    assert_eq!(o.vel_east_mps[i].to_bits(), r.vel_east_mps.to_bits());
    assert_eq!(o.vel_north_mps[i].to_bits(), r.vel_north_mps.to_bits());
    for j in 0..3 {
        assert_eq!(o.pos_cov_m2[i][j].to_bits(), r.pos_cov_m2[j].to_bits());
        assert_eq!(o.vel_cov_m2s2[i][j].to_bits(), r.vel_cov_m2s2[j].to_bits());
    }
    assert_eq!(o.turn_rate_rps[i].to_bits(), r.turn_rate_rps.to_bits());
    assert_eq!(o.alt_ft[i].to_bits(), r.alt_ft.to_bits());
    assert_eq!(o.alt_var_ft2[i].to_bits(), r.alt_var_ft2.to_bits());
    assert_eq!(o.vrate_fpm[i].to_bits(), r.vrate_fpm.to_bits());
    for j in 0..2 {
        assert_eq!(o.mu_h[i][j].to_bits(), r.mu_h[j].to_bits());
        assert_eq!(o.mu_v[i][j].to_bits(), r.mu_v[j].to_bits());
    }
    assert_eq!(o.flags[i], r.flags);
}

fn assert_aligned(o: &TrackOutput, expected: usize) {
    assert_eq!(output_len(o), expected);
    assert_eq!(o.t_ns.len(), expected);
    assert_eq!(o.addr.len(), expected);
    assert_eq!(o.lat_e7.len(), expected);
    assert_eq!(o.lon_e7.len(), expected);
    assert_eq!(o.vel_east_mps.len(), expected);
    assert_eq!(o.vel_north_mps.len(), expected);
    assert_eq!(o.pos_cov_m2.len(), expected);
    assert_eq!(o.vel_cov_m2s2.len(), expected);
    assert_eq!(o.turn_rate_rps.len(), expected);
    assert_eq!(o.alt_ft.len(), expected);
    assert_eq!(o.alt_var_ft2.len(), expected);
    assert_eq!(o.vrate_fpm.len(), expected);
    assert_eq!(o.mu_h.len(), expected);
    assert_eq!(o.mu_v.len(), expected);
    assert_eq!(o.flags.len(), expected);
}

fn storage_capacities(o: &TrackOutput) -> [usize; 15] {
    [
        o.t_ns.capacity(),
        o.addr.capacity(),
        o.lat_e7.capacity(),
        o.lon_e7.capacity(),
        o.vel_east_mps.capacity(),
        o.vel_north_mps.capacity(),
        o.pos_cov_m2.capacity(),
        o.vel_cov_m2s2.capacity(),
        o.turn_rate_rps.capacity(),
        o.alt_ft.capacity(),
        o.alt_var_ft2.capacity(),
        o.vrate_fpm.capacity(),
        o.mu_h.capacity(),
        o.mu_v.capacity(),
        o.flags.capacity(),
    ]
}

fn assert_storage_capacities(o: &TrackOutput, expected: &[usize; 15]) {
    let actual = storage_capacities(o);
    // Elementwise checks avoid lowering array equality into a 120-byte memcmp loop.
    for i in 0..15 {
        assert_eq!(actual[i], expected[i]);
    }
}

// REQ: OA-TRK-333
#[kani::proof]
#[kani::unwind(17)]
fn output_push_appends_every_column_bit_exactly_and_preserves_prefix() {
    // Capacity 2 and a two-row sequence exercise a nonempty prefix and the final available slot.
    let mut output = output_new(2);
    let prefix = any_row();
    let appended = any_row();

    output_push(&mut output, &prefix);
    assert_aligned(&output, 1);
    assert_row_at(&output, 0, &prefix);

    output_push(&mut output, &appended);
    assert_aligned(&output, 2);
    assert_row_at(&output, 0, &prefix);
    assert_row_at(&output, 1, &appended);
    assert_eq!(output.overflow, 0);
}

// REQ: OA-TRK-333
#[kani::proof]
#[kani::unwind(17)]
fn output_push_at_capacity_changes_only_saturating_overflow() {
    // Capacity 1 gives an existing payload row to preserve. Any initial overflow value includes
    // the saturation boundary; the rejected row must not alter any stored payload or allocation.
    let mut output = output_new(1);
    let existing = any_row();
    let rejected = any_row();
    output_push(&mut output, &existing);
    output.overflow = kani::any();
    let old_overflow = output.overflow;
    let old_storage_capacities = storage_capacities(&output);

    output_push(&mut output, &rejected);

    assert_eq!(output.capacity, 1);
    assert_eq!(output.overflow, old_overflow.saturating_add(1));
    assert_aligned(&output, 1);
    assert_row_at(&output, 0, &existing);
    assert_storage_capacities(&output, &old_storage_capacities);
}

// REQ: OA-TRK-333
#[kani::proof]
#[kani::unwind(17)]
fn output_clear_empties_columns_and_preserves_capacity_and_storage() {
    // One pushed row is enough to exercise nonempty clear; the chosen storage capacity is 2.
    let mut output = output_new(2);
    let row = any_row();
    output_push(&mut output, &row);
    output.overflow = kani::any();
    let configured_capacity = output.capacity;
    let overflow = output.overflow;
    let storage_before = storage_capacities(&output);

    output_clear(&mut output);

    assert_aligned(&output, 0);
    assert_eq!(output.capacity, configured_capacity);
    assert_eq!(output.overflow, overflow);
    assert_storage_capacities(&output, &storage_before);
}

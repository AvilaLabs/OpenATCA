//! Narrow Kani contracts for the production-sized sensor table.
//! Compiled only with `kani_full_capacity`; these do not prove clock-policy behavior.
use super::*;

// Fail closed if the opt-in configuration ever stops matching product capacities.
const _: () = assert!(SENSOR_TABLE == 64);
const _: () = assert!(RUN_BUF == 8);

const MAX_DROP_NS: i64 = 86_400 * 1_000_000_000;

/// Construct a valid full table with unique IDs and primitive symbolic times, avoiding an
/// arbitrary complex state.
fn arbitrary_full_table() -> GateState {
    let mut g = gate_new();
    for i in 0..SENSOR_TABLE {
        g.sensor_used[i] = true;
        g.sensor_id[i] = i as u16;
        g.sensor_ns[i] = kani::any();
    }
    g
}

/// The live count is monotone nonincreasing when the clock advances with a fixed nonnegative
/// drop interval. The full 64 symbolic stored timestamps include both signed extremes.
// REQ: OA-TRK-336
#[kani::proof]
#[kani::unwind(70)]
#[kani::solver(cadical)]
fn full_sensor_table_live_count_is_monotone() {
    let g = arbitrary_full_table();
    let early_clock: i64 = kani::any();
    let late_clock: i64 = kani::any();
    kani::assume(early_clock <= late_clock);
    let drop_ns: i64 = kani::any();
    kani::assume(drop_ns >= 0 && drop_ns <= MAX_DROP_NS);

    let early = gate_live_sensors(&g, early_clock, drop_ns);
    let late = gate_live_sensors(&g, late_clock, drop_ns);
    assert!(early <= SENSOR_TABLE);
    assert!(late <= SENSOR_TABLE);
    assert!(early >= late);

    kani::cover!(
        early_clock == i64::MIN && late_clock == i64::MAX,
        "signed clock extremes"
    );
    kani::cover!(
        g.sensor_ns[0] == i64::MIN,
        "minimum stored sensor timestamp"
    );
    kani::cover!(
        g.sensor_ns[1] == i64::MAX,
        "maximum stored sensor timestamp"
    );
}

/// A single note operation on a valid full table either updates an existing unique sensor's
/// maximum timestamp without eviction, or evicts the first least-recent slot exactly once for a
/// new sensor. The table stays full and unique in both branches.
// REQ: OA-TRK-336
#[kani::proof]
#[kani::unwind(70)]
#[kani::solver(cadical)]
fn full_sensor_table_note_preserves_or_evicts_by_contract() {
    // Valid fixture scope: every slot is used, IDs are exactly 0..63, and the
    // operation targets one of those IDs or the distinct new ID 64.
    let mut g = gate_new();
    for i in 0..SENSOR_TABLE {
        g.sensor_used[i] = true;
        g.sensor_id[i] = i as u16;
        g.sensor_ns[i] = kani::any();
    }
    g.evicted = kani::any();
    let old_times = g.sensor_ns;
    let before_evicts = g.evicted;
    let is_new: bool = kani::any();
    let target: usize = kani::any();
    kani::assume(target < SENSOR_TABLE);
    let sensor = if is_new {
        SENSOR_TABLE as u16
    } else {
        target as u16
    };
    let toa: i64 = kani::any();

    let mut oldest = 0usize;
    let mut oldest_ns = i64::MAX;
    for i in 0..SENSOR_TABLE {
        if old_times[i] < oldest_ns {
            oldest_ns = old_times[i];
            oldest = i;
        }
    }
    // Independent argmin/tie contract: the selected slot has a global minimum
    // timestamp, and no earlier slot ties it (the implementation keeps the first tie).
    for i in 0..SENSOR_TABLE {
        assert!(old_times[oldest] <= old_times[i]);
        if i < oldest {
            assert!(old_times[oldest] < old_times[i]);
        }
    }
    let expected_slot = if is_new { oldest } else { target };

    gate_note_applied(&mut g, sensor, toa);

    let expected_evictions = if is_new { 1u64 } else { 0u64 };
    assert_eq!(g.evicted, before_evicts.saturating_add(expected_evictions));
    for i in 0..SENSOR_TABLE {
        assert!(g.sensor_used[i]);
        if i == expected_slot {
            assert_eq!(g.sensor_id[i], sensor);
            let old = if is_new { i64::MIN } else { old_times[i] };
            assert_eq!(g.sensor_ns[i], old.max(toa));
        } else {
            assert_eq!(g.sensor_id[i], i as u16);
            assert_eq!(g.sensor_ns[i], old_times[i]);
        }
    }
    let mut sensor_occurrences = 0usize;
    for &id in &g.sensor_id {
        if id == sensor {
            sensor_occurrences += 1;
        }
    }
    assert_eq!(sensor_occurrences, 1);
    kani::cover!(is_new, "new sensor reaches full-table eviction");
    kani::cover!(!is_new, "existing sensor update does not evict");
    kani::cover!(
        is_new && before_evicts == u64::MAX,
        "eviction counter saturates"
    );
    kani::cover!(toa == i64::MIN, "minimum report-time input");
    kani::cover!(toa == i64::MAX, "maximum report-time input");
}

/// Updating an existing ID preserves the full table and does not evict.
// REQ: OA-TRK-336
#[kani::proof]
#[kani::unwind(70)]
#[kani::solver(cadical)]
fn full_sensor_table_note_existing_id_updates_max_without_eviction() {
    let mut g = gate_new();
    for i in 0..SENSOR_TABLE {
        g.sensor_used[i] = true;
        g.sensor_id[i] = i as u16;
        g.sensor_ns[i] = kani::any();
    }
    g.evicted = kani::any();
    let old_times = g.sensor_ns;
    let before_evicts = g.evicted;
    let target: usize = kani::any();
    kani::assume(target < SENSOR_TABLE);
    let toa: i64 = kani::any();

    gate_note_applied(&mut g, target as u16, toa);

    assert_eq!(g.evicted, before_evicts);
    for i in 0..SENSOR_TABLE {
        assert!(g.sensor_used[i]);
        if i == target {
            assert_eq!(g.sensor_id[i], target as u16);
            assert_eq!(g.sensor_ns[i], old_times[i].max(toa));
        } else {
            assert_eq!(g.sensor_id[i], i as u16);
            assert_eq!(g.sensor_ns[i], old_times[i]);
        }
    }
    kani::cover!(target == 0, "existing ID at first slot");
    kani::cover!(target == SENSOR_TABLE - 1, "existing ID at last slot");
    kani::cover!(toa == i64::MIN, "minimum report-time input");
    kani::cover!(toa == i64::MAX, "maximum report-time input");
    kani::cover!(
        before_evicts == u64::MAX,
        "unchanged saturated eviction counter"
    );
}

/// Inserting a new ID into a full table evicts the first least-recent slot.
// REQ: OA-TRK-336
#[kani::proof]
#[kani::unwind(70)]
#[kani::solver(cadical)]
fn full_sensor_table_note_new_id_evicts_first_argmin() {
    let mut g = gate_new();
    for i in 0..SENSOR_TABLE {
        g.sensor_used[i] = true;
        g.sensor_id[i] = i as u16;
        g.sensor_ns[i] = kani::any();
    }
    g.evicted = kani::any();
    let old_times = g.sensor_ns;
    let before_evicts = g.evicted;
    let toa: i64 = kani::any();

    // This proof-local witness does not constrain the input timestamp array: every finite
    // 64-element array has a first minimum. The proof establishes the postcondition for any
    // witness satisfying that exact first-argmin contract.
    let expected_slot: usize = kani::any();
    kani::assume(expected_slot < SENSOR_TABLE);
    for i in 0..SENSOR_TABLE {
        kani::assume(old_times[expected_slot] <= old_times[i]);
        if i < expected_slot {
            kani::assume(old_times[expected_slot] < old_times[i]);
        }
    }

    gate_note_applied(&mut g, SENSOR_TABLE as u16, toa);

    assert_eq!(g.evicted, before_evicts.saturating_add(1));
    for i in 0..SENSOR_TABLE {
        assert!(g.sensor_used[i]);
        if i == expected_slot {
            assert_eq!(g.sensor_id[i], SENSOR_TABLE as u16);
            assert_eq!(g.sensor_ns[i], i64::MIN.max(toa));
        } else {
            assert_eq!(g.sensor_id[i], i as u16);
            assert_eq!(g.sensor_ns[i], old_times[i]);
        }
    }
    kani::cover!(before_evicts == u64::MAX, "eviction counter saturates");
    kani::cover!(toa == i64::MIN, "minimum report-time input");
    kani::cover!(toa == i64::MAX, "maximum report-time input");
    kani::cover!(
        expected_slot == 0 && old_times[1] == old_times[0] && old_times[0] <= old_times[2],
        "tied minimum keeps earliest slot"
    );
}

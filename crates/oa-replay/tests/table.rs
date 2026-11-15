//! Tests of the flight table and the apply rules, over documents built from made-up data and
//! decoded by the real oa-fixm and oa-tbfm decoders.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_replay::table::{FEED_SFDPS, FEED_TBFM, FROZEN, HAS_ALT, HAS_POS, NS_PER_S, TBFM_ORIGIN};
use oa_replay::{
    advance_time, apply_sfdps, apply_tbfm, new_table, slot_of_gufi, state_hash, sweep, Applied,
    ConfigError, FlightTable, TableConfig,
};
use oa_tbfm::NO_TIME;

fn table(cap: usize) -> FlightTable {
    new_table(TableConfig {
        capacity: cap,
        ..TableConfig::default()
    })
    .unwrap()
}

fn sfa(t: &mut FlightTable, m: Sf, now: i64) -> Applied {
    let ev = events(&sf_doc(&[m]));
    apply_sfdps(t, &ev, 0, now)
}

fn tba(t: &mut FlightTable, msg_ms: i64, centre: &str, a: Tb, now: i64) -> Vec<Applied> {
    let r = rows(&tb_doc(msg_ms, centre, &[a]));
    (0..r.section.len())
        .map(|i| apply_tbfm(t, &r, i, now))
        .collect()
}

fn slot(t: &FlightTable, n: u32) -> usize {
    slot_of_gufi(t, gufi(n)).expect("flight present")
}

fn name(b: &[u8]) -> &str {
    oa_fixm::code_str(b)
}

// REQ: OA-RPL-001
#[test]
fn table_is_allocated_once_with_fixed_capacity_and_validated_settings() {
    let t = table(100);
    assert_eq!(t.capacity, 100);
    assert_eq!(t.gufi.len(), 100);
    assert_eq!(t.free.len(), 100);
    assert_eq!(t.live, 0);
    // The index is at least twice the capacity, a power of two: load never above 1/2.
    assert!(t.by_gufi.cells.len() >= 200 && t.by_gufi.cells.len().is_power_of_two());
    assert_eq!(oa_replay::DEFAULT_CAPACITY, 32_768);
    assert_eq!(oa_replay::DEFAULT_MAX_AGE_NS, 30 * 60 * NS_PER_S);
    let bad = |cfg| new_table(cfg).err().unwrap();
    let ok = TableConfig::default();
    assert_eq!(
        bad(TableConfig { capacity: 0, ..ok }),
        ConfigError::Capacity(0)
    );
    assert_eq!(
        bad(TableConfig {
            max_age_ns: 0,
            ..ok
        }),
        ConfigError::MaxAge(0)
    );
    assert_eq!(
        bad(TableConfig { sweep_ns: -1, ..ok }),
        ConfigError::Sweep(-1)
    );
}

// REQ: OA-RPL-002
#[test]
fn sfdps_row_fills_identity_position_and_source() {
    let mut t = table(16);
    let m = sf(1, 1000)
        .pos(32.5, -84.25, 900)
        .alt(36000)
        .call("TST123")
        .plan("KA000001AB")
        .arr("KTST")
        .dep("KDEP")
        .ctype("B38M")
        .cid("104")
        .unit("ZXX", "08");
    assert_eq!(sfa(&mut t, m, 5 * NS), Applied::Applied);
    let s = slot(&t, 1);
    assert_eq!(name(&t.callsign[s]), "TST123");
    assert_eq!(name(&t.arrival[s]), "KTST");
    assert_eq!(name(&t.departure[s]), "KDEP");
    assert_eq!(name(&t.aircraft_type[s]), "B38M");
    assert_eq!(name(&t.computer_id[s]), "104");
    assert_eq!(name(&t.plan_id[s]), "KA000001AB");
    assert_eq!((t.lat_e7[s], t.lon_e7[s]), (325_000_000, -842_500_000));
    assert_eq!(t.alt_ft[s], 36000);
    assert_eq!(t.pos_time_ms[s], DAY + 900);
    assert_ne!(t.flags[s] & HAS_POS, 0);
    assert_ne!(t.flags[s] & HAS_ALT, 0);
    assert_eq!(t.last_update_ns[s], 5 * NS);
    assert_eq!((t.last_feed[s], &t.last_source[s]), (FEED_SFDPS, b"TH"));
    assert_eq!(&t.ctrl[s][..3], b"ZXX");
    assert_eq!((t.updates[s], t.out_of_order[s], t.conflicts[s]), (1, 0, 0));
    assert_eq!(t.live, 1);
}

// REQ: OA-RPL-003
#[test]
fn an_older_position_never_overwrites_a_newer_one_and_is_counted() {
    let mut t = table(16);
    assert_eq!(
        sfa(&mut t, sf(1, 1000).pos(10.0, 20.0, 5000), NS),
        Applied::Applied
    );
    // Older position time: dropped, counted; the row's other fields still apply.
    let stale = sf(1, 2000).pos(11.0, 21.0, 4000).arr("KTST");
    assert_eq!(sfa(&mut t, stale, 2 * NS), Applied::AppliedStalePosition);
    let s = slot(&t, 1);
    assert_eq!((t.lat_e7[s], t.lon_e7[s]), (100_000_000, 200_000_000));
    assert_eq!(t.pos_time_ms[s], DAY + 5000);
    assert_eq!(name(&t.arrival[s]), "KTST");
    assert_eq!((t.out_of_order[s], t.updates[s]), (1, 2));
    assert_eq!(t.stats.out_of_order, 1);
    // The same time applies (a redelivery); a newer one applies.
    assert_eq!(
        sfa(&mut t, sf(1, 3000).pos(12.0, 22.0, 5000), 3 * NS),
        Applied::Applied
    );
    assert_eq!(t.lat_e7[s], 120_000_000);
    assert_eq!(
        sfa(&mut t, sf(1, 4000).pos(13.0, 23.0, 6000), 4 * NS),
        Applied::Applied
    );
    assert_eq!((t.lat_e7[s], t.pos_time_ms[s]), (130_000_000, DAY + 6000));
    assert_eq!(t.out_of_order[s], 1);
    // A row without a position is never out of order.
    assert_eq!(
        sfa(&mut t, sf(1, 100).call("TST1"), 5 * NS),
        Applied::Applied
    );
    assert_eq!(t.stats.out_of_order, 1);
}

// REQ: OA-RPL-004
#[test]
fn identity_fills_then_a_different_value_is_a_counted_conflict_and_kept_out() {
    let mut t = table(16);
    sfa(
        &mut t,
        sf(1, 1000).call("TST123").arr("KTST").cid("104"),
        NS,
    );
    let s = slot(&t, 1);
    // Same value confirms; different callsign and arrival conflict; the computer id changes
    // as the flight crosses centres and is not a conflict.
    sfa(
        &mut t,
        sf(1, 2000).call("TST123").arr("KOTH").cid("205"),
        2 * NS,
    );
    sfa(&mut t, sf(1, 3000).call("OTH999"), 3 * NS);
    assert_eq!(name(&t.callsign[s]), "TST123");
    assert_eq!(name(&t.arrival[s]), "KTST");
    assert_eq!(name(&t.computer_id[s]), "205");
    assert_eq!(t.conflicts[s], 2);
    assert_eq!(t.stats.conflicts, 2);
}

// REQ: OA-RPL-005
#[test]
fn tbfm_links_to_an_sfdps_flight_by_flight_plan_id_and_otherwise_creates_one() {
    let mut t = table(16);
    sfa(&mut t, sf(1, 1000).plan("KA000001AB").call("TST123"), NS);
    // The TBFM gufi is the SFDPS flight-plan identifier.
    let r = tba(
        &mut t,
        2000,
        "ZTT",
        tb(2000, Some("KA000001AB"), "T00001").flt("B38M/L"),
        2 * NS,
    );
    assert_eq!(r, vec![Applied::Applied]);
    assert_eq!(t.live, 1, "linked, no new flight");
    assert_eq!(t.stats.tbfm_linked_gufi, 1);
    let s = slot(&t, 1);
    assert_eq!(name(&t.aircraft_type[s]), "B38M", "the /L suffix is cut");
    assert_eq!(t.last_feed[s], FEED_TBFM);
    assert_eq!(&t.tma_id[s][..6], b"T00001");
    // An unknown TBFM gufi creates a TBFM-first flight.
    tba(
        &mut t,
        3000,
        "ZTT",
        tb(3000, Some("KA000002CD"), "T00002").flt("A320/L"),
        3 * NS,
    );
    assert_eq!(t.live, 2);
    assert_eq!((t.stats.tbfm_created, t.stats.tbfm_linked_gufi), (1, 1));
    let s2 = (0..16).find(|&i| t.flags[i] & TBFM_ORIGIN != 0).unwrap();
    assert_eq!(name(&t.plan_id[s2]), "KA000002CD");
    // When SFDPS reports that flight plan it gives the flight its real GUFI: still one flight,
    // metering state kept.
    sfa(
        &mut t,
        sf(7, 4000).plan("KA000002CD").call("NEW777"),
        4 * NS,
    );
    assert_eq!(t.live, 2);
    assert_eq!(t.stats.promoted, 1);
    let s7 = slot(&t, 7);
    assert_eq!(s7, s2);
    assert_eq!(t.flags[s7] & TBFM_ORIGIN, 0);
    assert_eq!(name(&t.aircraft_type[s7]), "A320");
    assert_eq!(
        name(&t.callsign[s7]),
        "TST123",
        "held identity is not overwritten"
    );
    assert_eq!(t.conflicts[s7], 1);
}

// REQ: OA-RPL-006
#[test]
fn tbfm_without_gufi_links_by_centre_and_tma_id_and_counts_unlinked() {
    let mut t = table(16);
    // A row with no gufi and no known tmaId is unlinked and applies nothing.
    let r = tba(
        &mut t,
        1000,
        "ZTT",
        tb(1000, None, "T00009").eta("MFXA1", 5000),
        NS,
    );
    assert_eq!(r, vec![Applied::Unlinked]);
    assert_eq!((t.live, t.stats.tbfm_unlinked), (0, 1));
    // A row with a gufi teaches the (centre, tmaId) pair.
    tba(
        &mut t,
        2000,
        "ZTT",
        tb(2000, Some("KA000001AB"), "T00001").mrp("MFXA1", "09L", "NEW"),
        2 * NS,
    );
    // Now a row without gufi links through it, for the same centre.
    let r = tba(
        &mut t,
        3000,
        "ZTT",
        tb(3000, None, "T00001").eta("MFXA1", 7000),
        3 * NS,
    );
    assert_eq!(r, vec![Applied::Applied]);
    assert_eq!(t.stats.tbfm_linked_tma, 1);
    let s = (0..16).find(|&i| t.eta_ms[i] != NO_TIME).unwrap();
    assert_eq!(t.eta_ms[s], DAY + 7000);
    // The same tmaId from another centre is another key.
    let r = tba(
        &mut t,
        4000,
        "ZYY",
        tb(4000, None, "T00001").eta("MFXA1", 8000),
        4 * NS,
    );
    assert_eq!(r, vec![Applied::Unlinked]);
    assert_eq!(t.stats.tbfm_unlinked, 2);
}

// REQ: OA-RPL-007
#[test]
fn tbfm_metering_columns_and_frozen_flag() {
    let mut t = table(16);
    let a = tb(1000, Some("KA000001AB"), "T00001")
        .mrp("MFXA1", "09L", "NEW")
        .eta("MFXA1", 60_000)
        .sta("MFXA1", 90_000)
        .sch("MFXA1", "YES");
    let r = tba(&mut t, 1000, "ZTT", a, NS);
    assert!(r.iter().all(|&x| x == Applied::Applied));
    let s = (0..16).find(|&i| t.meter_fix[i] != [0; 8]).unwrap();
    assert_eq!(name(&t.meter_fix[s]), "MFXA1");
    assert_eq!(name(&t.runway[s]), "09L");
    assert_eq!(t.eta_ms[s], DAY + 60_000);
    // The feed has no sta_mfx; the scheduling-fix time stands in.
    assert_eq!(t.sta_ms[s], DAY + 90_000);
    assert_ne!(t.flags[s] & FROZEN, 0);
    assert_eq!(t.tbfm_msg_ms[s], DAY + 1000);
    // sfz NO unfreezes.
    tba(
        &mut t,
        2000,
        "ZTT",
        tb(2000, Some("KA000001AB"), "T00001").sch("MFXA1", "NO"),
        2 * NS,
    );
    assert_eq!(t.flags[s] & FROZEN, 0);
    // An older message is refused whole and counted.
    let r = tba(
        &mut t,
        1500,
        "ZTT",
        tb(1500, Some("KA000001AB"), "T00001").eta("MFXA1", 1),
        3 * NS,
    );
    assert_eq!(r, vec![Applied::OutOfOrder]);
    assert_eq!(t.eta_ms[s], DAY + 60_000);
    assert_eq!((t.out_of_order[s], t.stats.out_of_order), (1, 1));
}

// REQ: OA-RPL-008
#[test]
fn tbfm_del_clears_metering_but_keeps_the_flight() {
    let mut t = table(16);
    let a = tb(1000, Some("KA000001AB"), "T00001")
        .mrp("MFXA1", "09L", "NEW")
        .eta("MFXA1", 60_000)
        .sta("MFXA1", 90_000)
        .sch("MFXA1", "YES");
    tba(&mut t, 1000, "ZTT", a, NS);
    let s = (0..16).find(|&i| t.meter_fix[i] != [0; 8]).unwrap();
    // DEL on the air element.
    tba(
        &mut t,
        2000,
        "ZTT",
        tb(2000, Some("KA000001AB"), "T00001")
            .air_type("DEL")
            .flt("B38M/L"),
        2 * NS,
    );
    assert_eq!(t.live, 1, "the flight stays");
    assert_eq!(t.meter_fix[s], [0; 8]);
    assert_eq!(t.runway[s], [0; 4]);
    assert_eq!((t.sta_ms[s], t.eta_ms[s]), (NO_TIME, NO_TIME));
    assert_eq!(t.flags[s] & FROZEN, 0);
    // Metering can start again.
    tba(
        &mut t,
        3000,
        "ZTT",
        tb(3000, Some("KA000001AB"), "T00001").eta("MFXB2", 70_000),
        3 * NS,
    );
    assert_eq!(
        (name(&t.meter_fix[s]), t.eta_ms[s]),
        ("MFXB2", DAY + 70_000)
    );
    // DEL of the meter route (mrpType) clears too.
    tba(
        &mut t,
        4000,
        "ZTT",
        tb(4000, Some("KA000001AB"), "T00001").mrp("MFXB2", "27R", "DEL"),
        4 * NS,
    );
    assert_eq!(t.meter_fix[s], [0; 8]);
    assert_eq!(t.eta_ms[s], NO_TIME);
    assert_eq!(t.live, 1);
}

// REQ: OA-RPL-009
#[test]
fn eviction_is_by_age_at_the_sweep_cadence_in_slot_order() {
    let mut t = new_table(TableConfig {
        capacity: 8,
        max_age_ns: 100 * NS,
        sweep_ns: 10 * NS,
        ..TableConfig::default()
    })
    .unwrap();
    for n in 1..=4u32 {
        sfa(&mut t, sf(n, 1000).call("TST1"), 0);
    }
    // Flight 3 is refreshed at t=60 s.
    sfa(&mut t, sf(3, 2000).call("TST1"), 60 * NS);
    // The first call only schedules the first sweep, on a multiple of the cadence.
    assert_eq!(advance_time(&mut t, 5 * NS), 0);
    assert_eq!(t.next_sweep_ns, 10 * NS);
    // Not due yet (the first sweep is at 10 s): nothing happens.
    assert_eq!(advance_time(&mut t, 9 * NS), 0);
    assert_eq!(t.stats.sweeps, 0);
    // Exactly the age limit is not older than it: at t=100 s the flights from t=0 stay.
    assert_eq!(advance_time(&mut t, 100 * NS), 0);
    assert_eq!(t.stats.sweeps, 1);
    assert_eq!(t.next_sweep_ns, 110 * NS);
    // At 105 s no sweep is due (next is 110 s); the old flights outlive the limit until then.
    assert_eq!(advance_time(&mut t, 105 * NS), 0);
    assert_eq!(t.live, 4);
    // At 110 s flights 1, 2, 4 are 110 s old (> 100) and go; flight 3 (50 s old) stays.
    assert_eq!(advance_time(&mut t, 110 * NS), 3);
    assert_eq!(t.live, 1);
    assert!(slot_of_gufi(&t, gufi(3)).is_some());
    assert_eq!(t.stats.evicted, 3);
    // Freed slots are reused lowest-slot-first deterministically: the free stack was pushed in
    // slot order, so the last evicted slot is on top.
    let evicted_slots = [0usize, 1, 3];
    sfa(&mut t, sf(9, 3000).call("NEW1"), 111 * NS);
    assert_eq!(slot(&t, 9), evicted_slots[2]);
    // Eviction removes the flight from every index: its keys can be reused.
    sfa(
        &mut t,
        sf(1, 4000).call("TST1").plan("KA000001AB"),
        112 * NS,
    );
    assert!(slot_of_gufi(&t, gufi(1)).is_some());
}

// REQ: OA-RPL-010
#[test]
fn sweep_visits_slots_in_order_and_age_is_configurable() {
    let mut t = new_table(TableConfig {
        capacity: 4,
        max_age_ns: 10 * NS,
        sweep_ns: NS,
        ..TableConfig::default()
    })
    .unwrap();
    for n in [5u32, 3, 9] {
        sfa(
            &mut t,
            sf(n, 1000).call("TST1").plan(&format!("KA00000{n}AA")),
            0,
        );
    }
    // Slots 0, 1, 2 hold flights 5, 3, 9.
    assert_eq!(slot(&t, 5), 0);
    assert_eq!(slot(&t, 9), 2);
    assert_eq!(sweep(&mut t, 11 * NS), 3);
    // Slot order: 0, 1, 2 evicted, so the free stack top is slot 2 and the next new flight
    // lands in slot 2, then 1, then 0.
    for (n, want) in [(21u32, 2usize), (22, 1), (23, 0)] {
        sfa(&mut t, sf(n, 1000).call("TST1"), 12 * NS);
        assert_eq!(slot(&t, n), want);
    }
    // The plan index forgot the evicted flights.
    assert!(t.by_plan.used == 0 && t.by_tma.used == 0);
}

// REQ: OA-RPL-011
#[test]
fn a_full_table_refuses_new_flights_and_never_evicts_early() {
    let mut t = table(3);
    for n in 1..=3u32 {
        assert_eq!(sfa(&mut t, sf(n, 1000).call("TST1"), NS), Applied::Applied);
    }
    // A fourth flight, even with every flight long idle, is refused and counted.
    let late = 10_000 * NS;
    assert_eq!(
        sfa(&mut t, sf(4, 2000).call("NEW1"), late),
        Applied::TableFull
    );
    assert_eq!(t.stats.refused_full, 1);
    assert_eq!(t.live, 3);
    assert!(slot_of_gufi(&t, gufi(4)).is_none());
    assert_eq!(t.stats.evicted, 0);
    // TBFM creating a flight is refused the same way; linking an existing one still works.
    let r = tba(
        &mut t,
        3000,
        "ZTT",
        tb(3000, Some("KA000009ZZ"), "T00009").flt("B38M/L"),
        late,
    );
    assert_eq!(r, vec![Applied::TableFull]);
    assert_eq!(t.stats.refused_full, 2);
    // Existing flights keep being updated.
    assert_eq!(
        sfa(&mut t, sf(2, 4000).call("TST1").arr("KTST"), late),
        Applied::Applied
    );
    // Only the age sweep frees room; then the refused flight can be admitted.
    // Flights 1 and 3 are idle for hours; flight 2 was updated at `late`.
    let after = late + 29 * 60 * NS;
    assert_eq!(sweep(&mut t, after), 2);
    assert_eq!(
        sfa(&mut t, sf(4, 5000).call("NEW1"), after),
        Applied::Applied
    );
}

// REQ: OA-RPL-012
#[test]
fn state_hash_does_not_depend_on_insertion_order() {
    // Updates that commute: several per flight, each touching its own columns, across flights.
    let mut updates: Vec<(u32, Sf)> = Vec::new();
    for n in 1..=40u32 {
        let id = n * 7;
        updates.push((
            id,
            sf(id, 1000)
                .call(&format!("TST{n}"))
                .plan(&format!("KA{n:06}AB")),
        ));
        updates.push((id, sf(id, 1000).arr("KTST").dep("KDEP")));
        updates.push((id, sf(id, 1000).unit("ZXX", "08")));
        updates.push((
            id,
            sf(id, 1000).pos(30.0 + f64::from(n) / 10.0, -80.0, 5000),
        ));
    }
    let tbfm: Vec<Tb> = (1..=40u32)
        .flat_map(|n| {
            let g = format!("KB{n:06}CD");
            let t = format!("T{n:05}");
            vec![
                tb(9000, Some(&g), &t).mrp("MFXA1", "09L", "NEW"),
                tb(9000, Some(&g), &t).eta("MFXA1", 60_000 + i64::from(n)),
                tb(9000, Some(&g), &t).sta("MFXA1", 90_000),
                tb(9000, Some(&g), &t).sch("MFXA1", "YES"),
            ]
        })
        .collect();

    let run = |seed: u64| -> ([u8; 32], usize) {
        let mut items: Vec<(bool, usize)> = (0..updates.len())
            .map(|i| (true, i))
            .chain((0..tbfm.len()).map(|i| (false, i)))
            .collect();
        let mut rng = Rng(seed);
        if seed != 0 {
            rng.shuffle(&mut items);
        }
        let mut t = table(256);
        for (is_sf, i) in items {
            if is_sf {
                let (_, m) = &updates[i];
                assert_eq!(sfa(&mut t, m.clone(), 5 * NS), Applied::Applied);
            } else {
                let r = rows(&tb_doc(9000, "ZTT", &[tbfm[i].clone()]));
                assert_eq!(apply_tbfm(&mut t, &r, 0, 7 * NS), Applied::Applied);
            }
        }
        assert_eq!(t.live, 80);
        (state_hash(&t), t.live)
    };
    let base = run(0);
    for seed in 1..=8u64 {
        assert_eq!(run(seed * 0x9e37_79b9), base, "seed {seed}");
    }
}

// REQ: OA-RPL-013
#[test]
fn state_hash_reflects_state_not_slots_and_changes_with_any_column() {
    let mut a = table(8);
    let mut b = table(8);
    sfa(&mut a, sf(1, 1000).call("TST1"), NS);
    sfa(&mut a, sf(2, 1000).call("TST2"), NS);
    // b reaches the same two flights through a different history: flight 1 is evicted and
    // re-added, so the slots differ from a's.
    sfa(&mut b, sf(2, 1000).call("TST2"), NS);
    sfa(&mut b, sf(5, 1000).call("OLD5"), 0);
    sfa(&mut b, sf(1, 1000).call("TST1"), NS);
    sweep(&mut b, 4000 * NS);
    assert_eq!(b.live, 0, "everything idle was evicted");
    // Rebuild b's flights after the sweep so only the history differs.
    let mut c = table(8);
    sfa(&mut c, sf(2, 1000).call("TST2"), NS);
    sfa(&mut c, sf(1, 1000).call("TST1"), NS);
    assert_ne!(slot(&a, 1), slot(&c, 1));
    assert_eq!(state_hash(&a), state_hash(&c));
    // Any column change shows in the hash.
    let h = state_hash(&a);
    sfa(&mut a, sf(1, 2000).unit("ZXX", "01"), 2 * NS);
    assert_ne!(state_hash(&a), h);
    let h2 = state_hash(&a);
    sfa(&mut a, sf(2, 2000).pos(1.0, 2.0, 100), 2 * NS);
    assert_ne!(state_hash(&a), h2);
    // An empty table has a stable hash.
    assert_eq!(state_hash(&table(8)), state_hash(&table(16)));
}

// REQ: OA-RPL-026
#[test]
fn canonical_serialisation_has_a_pinned_golden_hash() {
    // A fixed state hashes to a fixed value on every machine; a change to the serialisation (or
    // to what the state holds) must show here and be a deliberate edit of HASH_TAG's version.
    let mut t = table(8);
    sfa(
        &mut t,
        sf(1, 1000)
            .pos(32.5, -84.25, 900)
            .alt(36000)
            .call("TST123")
            .plan("KA000001AB")
            .arr("KTST")
            .unit("ZXX", "08"),
        5 * NS,
    );
    tba(
        &mut t,
        2000,
        "ZTT",
        tb(2000, Some("KA000001AB"), "T00001")
            .mrp("MFXA1", "09L", "NEW")
            .eta("MFXA1", 60_000)
            .sch("MFXA1", "YES"),
        6 * NS,
    );
    let hex = oa_sha256::to_hex(&state_hash(&t));
    assert_eq!(oa_replay::HASH_TAG, b"OATS1");
    // Re-pinned when each SFDPS flight began to hold the timestamp of its newest message, which
    // is hashed in a block after the flights (OA-RPL-201); the serialisation of the columns is
    // unchanged, so HASH_TAG keeps its version.
    assert_eq!(
        hex,
        "5ba94b361871858c42cfc9c6bb86b2c63db90dd192c7d834517fd44ba0eff530"
    );
}

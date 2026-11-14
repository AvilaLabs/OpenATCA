//! The flight table does not allocate after initialisation (RULESET C1, C5). A counting allocator
//! needs an `unsafe` impl, which RULESET A0 forbids in this crate, so (as in OA-ZIP-007) the
//! check is on the buffers themselves: every column and index keeps its address and capacity
//! through applying, refusing, sweeping and evicting, which is what a reallocation would change.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_replay::{apply_sfdps, apply_tbfm, new_table, sweep, FlightTable, TableConfig};

/// Address and capacity of every buffer the table owns.
fn fingerprint(t: &FlightTable) -> Vec<(usize, usize)> {
    fn f<T>(v: &Vec<T>) -> (usize, usize) {
        (v.as_ptr() as usize, v.capacity())
    }
    vec![
        f(&t.occupied),
        f(&t.free),
        f(&t.by_gufi.cells),
        f(&t.by_plan.cells),
        f(&t.by_tma.cells),
        f(&t.gufi),
        f(&t.callsign),
        f(&t.computer_id),
        f(&t.departure),
        f(&t.arrival),
        f(&t.aircraft_type),
        f(&t.plan_id),
        f(&t.lat_e7),
        f(&t.lon_e7),
        f(&t.alt_ft),
        f(&t.gs_dkt),
        f(&t.vx_dkt),
        f(&t.vy_dkt),
        f(&t.pos_time_ms),
        f(&t.last_update_ns),
        f(&t.last_source),
        f(&t.last_feed),
        f(&t.ctrl),
        f(&t.tma_center),
        f(&t.tma_id),
        f(&t.meter_fix),
        f(&t.sta_ms),
        f(&t.eta_ms),
        f(&t.runway),
        f(&t.tbfm_msg_ms),
        f(&t.flags),
        f(&t.updates),
        f(&t.out_of_order),
        f(&t.conflicts),
    ]
}

// REQ: OA-RPL-015
#[test]
fn table_buffers_never_move_or_grow_after_init() {
    let mut t = new_table(TableConfig {
        capacity: 64,
        ..TableConfig::default()
    })
    .unwrap();
    let start = fingerprint(&t);
    let msgs: Vec<Sf> = (1..=80u32)
        .map(|n| {
            sf(n, 1000)
                .call("TST1")
                .plan(&format!("KA{n:06}AB"))
                .pos(30.0, -80.0, 500)
                .arr("KTST")
        })
        .collect();
    let ev = events(&sf_doc(&msgs));
    let airs: Vec<Tb> = (1..=80u32)
        .map(|n| {
            tb(2000, Some(&format!("KB{n:06}AB")), &format!("T{n:05}"))
                .mrp("MFXA1", "09L", "NEW")
                .eta("MFXA1", 60_000)
        })
        .collect();
    let r = rows(&tb_doc(2000, "ZTT", &airs));
    // 80 flights into 64 slots: the table fills and refuses.
    for round in 0..3i64 {
        for i in 0..ev.len {
            apply_sfdps(&mut t, &ev, i, round * 1_000_000_000);
        }
        for i in 0..r.section.len() {
            apply_tbfm(&mut t, &r, i, round * 1_000_000_000 + 500);
        }
        assert_eq!(fingerprint(&t), start);
        sweep(&mut t, (round + 1) * 4_000_000_000_000);
        assert_eq!(fingerprint(&t), start);
    }
    assert!(t.stats.refused_full > 0 && t.stats.evicted > 0);
}

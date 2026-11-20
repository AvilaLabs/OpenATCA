//! The split of the climbing and descending encounters near 1,000 ft by what the flight record
//! held, over made-up encounters and answers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_encounter::Encounter;
use oa_encounter_run::args::parse;
use oa_encounter_run::join::{
    assigned_of, effective_of, is_near_climb, join_text, Answer, Clearance, JoinStats,
};
use oa_flight::{Alt, ALT_AT_FIX, ALT_NIL, ALT_SIMPLE, ALT_VFR};

fn enc(v: f32, vr: f32, class: u8) -> Encounter {
    Encounter {
        addr_a: 1,
        addr_b: 2,
        start_ns: 0,
        end_ns: 5_000_000_000,
        closest_ns: 2_000_000_000,
        interp_ns: 2_000_000_000,
        min_h_m: 500.0,
        min_h_interp_m: 500.0,
        min_v_ft: v,
        vrate_a_fpm: vr,
        vrate_b_fpm: 0.0,
        max_abs_vrate_fpm: vr.abs(),
        v_at_closest_ft: v,
        min_scaled_m: 900.0,
        alt_ft: 12_000.0,
        lat_e7: 411_234_567,
        lon_e7: -881_234_567,
        ticks_inside: 6,
        ticks_alt_unknown: 0,
        ticks_unevaluated: 0,
        flags_a: 0,
        flags_b: 0,
        ground_either: 0,
        class,
    }
}

fn simple(ft: i32) -> Alt {
    Alt {
        form: ALT_SIMPLE,
        a_ft: ft,
        b_ft: 0,
    }
}

fn ans(assigned: Alt, interim: Alt) -> Answer {
    Answer {
        flight: true,
        ended: false,
        assigned,
        interim,
    }
}

const NONE: Alt = Alt {
    form: 0,
    a_ft: 0,
    b_ft: 0,
};

fn stats() -> JoinStats {
    JoinStats {
        frames: 10,
        ops: 20,
        refused: 1,
        conflicts: 0,
        flights_held: 3,
    }
}

fn count(text: &str, section: &str, label: &str) -> u64 {
    let s = text.split(section).nth(1).unwrap();
    let line = s.lines().find(|l| l.contains(label)).unwrap();
    line.rsplit(": ").next().unwrap().trim().parse().unwrap()
}

// REQ: OA-FLT-020
#[test]
fn the_group_is_airborne_climbing_encounters_from_900_ft_up_to_the_volume() {
    let ap = [];
    assert!(is_near_climb(&enc(950.0, 800.0, 2), &ap, 1000.0));
    assert!(is_near_climb(&enc(900.0, -800.0, 2), &ap, 1000.0));
    assert!(
        !is_near_climb(&enc(899.0, 800.0, 2), &ap, 1000.0),
        "under the band"
    );
    assert!(
        !is_near_climb(&enc(950.0, 100.0, 2), &ap, 1000.0),
        "level flight"
    );
    assert!(
        !is_near_climb(&enc(1000.0, 800.0, 2), &ap, 1000.0),
        "not under the volume"
    );
}

// REQ: OA-FLT-020
#[test]
fn the_interim_altitude_wins_where_it_is_held_and_a_clear_leaves_the_assigned_one() {
    let a = ans(simple(30_000), simple(24_000));
    assert_eq!(assigned_of(&a), Clearance::Feet(30_000));
    assert_eq!(effective_of(&a), Clearance::Feet(24_000));
    let cleared = ans(
        simple(30_000),
        Alt {
            form: ALT_NIL,
            a_ft: 0,
            b_ft: 0,
        },
    );
    assert_eq!(effective_of(&cleared), Clearance::Feet(30_000));
    let vfr = ans(
        Alt {
            form: ALT_VFR,
            a_ft: 0,
            b_ft: 0,
        },
        NONE,
    );
    assert_eq!(
        assigned_of(&vfr),
        Clearance::NotSimple,
        "VFR is never a number"
    );
    let fix = ans(
        Alt {
            form: ALT_AT_FIX,
            a_ft: 22_000,
            b_ft: 7_000,
        },
        NONE,
    );
    assert_eq!(assigned_of(&fix), Clearance::NotSimple);
    assert_eq!(assigned_of(&ans(NONE, NONE)), Clearance::Missing);
}

// REQ: OA-FLT-020
#[test]
fn each_encounter_of_the_group_is_split_once_and_the_unresolved_are_counted_apart() {
    // Encounters 0 to 5 are in the group; 6 is level flight (not in it).
    let es = [
        enc(950.0, 800.0, 2),
        enc(950.0, 800.0, 2),
        enc(950.0, 800.0, 2),
        enc(950.0, 800.0, 2),
        enc(950.0, 800.0, 3),
        enc(950.0, 800.0, 3),
        enc(990.0, 0.0, 2),
    ];
    let none = Answer::default();
    let mut answers = Vec::new();
    // 0: cleared 1,000 ft apart (level-off). 1: cleared 500 ft apart (does not stop).
    answers.extend([ans(simple(30_000), NONE), ans(simple(31_000), NONE)]);
    answers.extend([ans(simple(30_000), NONE), ans(simple(30_500), NONE)]);
    // 2: no record for the second. 3: a record with no altitude.
    answers.extend([ans(simple(30_000), NONE), none]);
    answers.extend([ans(simple(30_000), NONE), ans(NONE, NONE)]);
    // 4: assigned the same, the interim 1,000 ft apart. 5: VFR.
    answers.extend([
        ans(simple(30_000), simple(24_000)),
        ans(simple(30_000), simple(25_000)),
    ]);
    answers.extend([
        ans(
            Alt {
                form: ALT_VFR,
                a_ft: 0,
                b_ft: 0,
            },
            NONE,
        ),
        ans(simple(30_000), NONE),
    ]);
    answers.extend([none, none]);
    let t = join_text(&es, &answers, &[], 1000.0, &stats(), "test");
    assert!(t.contains("before (one group): 6 climbing"));
    let a = t.split("judged on the assigned altitude").nth(1).unwrap();
    let a = a.split("judged on the interim").next().unwrap();
    let all = |label: &str| count(a, "all classes:", label);
    assert_eq!(all("level-off"), 1);
    assert_eq!(all("does not stop"), 2, "encounters 1 and 4");
    assert_eq!(all("no flight record"), 1);
    assert_eq!(all("a flight record with no altitude"), 1);
    assert_eq!(all("that is not one level"), 1);
    let sum: u64 = [
        "level-off",
        "does not stop",
        "no flight record",
        "a flight record with no altitude",
        "that is not one level",
    ]
    .iter()
    .map(|l| all(l))
    .sum();
    assert_eq!(sum, 6, "every encounter of the group lands in exactly one");
    let e = t
        .split("judged on the interim altitude over assigned")
        .nth(1)
        .unwrap();
    assert_eq!(
        count(e, "all classes:", "level-off"),
        2,
        "encounters 0 and 4 stop 1,000 ft apart"
    );
    assert!(t.contains("flight record found: both aircraft 5, one 1, neither 0"));
}

// REQ: OA-FLT-021
#[test]
fn the_arguments_take_a_window_and_the_sfdps_input() {
    let a = |v: &[&str]| parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let base = ["--adsb", "c.oaf", "--out", "o"];
    let ok = a(&[
        base[0],
        base[1],
        base[2],
        base[3],
        "--from",
        "100",
        "--to",
        "200",
        "--sfdps",
        "d",
        "--lag-floor-s",
        "60",
    ])
    .unwrap();
    assert_eq!(ok.window, Some((100, 200)));
    assert_eq!(ok.lag_floor_s, 60);
    assert!(ok.sfdps.is_some());
    let d = a(&base).unwrap();
    assert_eq!(
        (d.window, d.lag_floor_s, d.sfdps, d.hours),
        (None, 58, None, false)
    );
    assert!(a(&[base[0], base[1], base[2], base[3], "--from", "100"]).is_err());
    assert!(a(&[base[0], base[1], base[2], base[3], "--from", "200", "--to", "100"]).is_err());
    assert!(a(&[base[0], base[1], base[2], base[3], "--from", "x", "--to", "1"]).is_err());
}

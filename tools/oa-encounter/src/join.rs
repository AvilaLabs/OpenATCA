//! The join of the record of close encounters with the flight record (M4): for each aircraft of
//! each encounter, the altitudes SFDPS had made available for it at the closest approach. Shell
//! (B3): it reads the recordings through `oa-flight-sfdps`.
//!
//! "As of" is the table's own time: an operation counts only from `max(received, stamped + lag
//! floor)`, so a question at time T is answered before the first operation available after T is
//! applied and sees nothing that arrived or aged in later.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use oa_encounter::Encounter;
use oa_flight::{
    flight_by_address, flight_is_live, Alt, FlightConfig, FlightTable, ALT_AT_FIX, ALT_BLOCK,
    ALT_NIL, ALT_SIMPLE, ALT_VFR, ALT_VFR_ON_TOP_PLUS,
};
use oa_flight_sfdps::{recordings_in, replay_sfdps_counted, ReplayConfig};

use crate::report::{at_vertical_minimum, class_name, context_of, Airport, CLIMB_FPM, LEVEL_FT};

/// Vertical separations from this up to the record volume's 1,000 ft are "near 1,000 ft" for the
/// split (the 25 ft altitude quantum puts two levels 1,000 ft apart at 975 to 1,000 ft, and a
/// climb stopping at a level leaves the pair a little under that while it settles).
pub const NEAR_FROM_FT: f32 = 900.0;
/// Two clearances at least this far apart are levels 1,000 ft apart: a level-off.
pub const LEVEL_OFF_FT: i32 = 1000;

/// What the flight record held for one aircraft at one time. A7.2: cold struct, size guard waived.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    /// A live flight claims the aircraft address.
    pub flight: bool,
    /// A flight claims it but every plan of it had ended.
    pub ended: bool,
    pub assigned: Alt,
    pub interim: Alt,
}

/// The sfdps input of a run.
// A7.2: cold struct, size guard waived
pub struct Sfdps {
    pub dir: PathBuf,
    pub lag_floor_ms: i64,
}

/// What the replay behind the answers saw (counts only).
// A7.2: cold struct, size guard waived
pub struct JoinStats {
    pub frames: u64,
    /// SFDPS frames and rows left out because their time is in a sealed window (OA-SEAL-014).
    pub sealed_window_dropped: u64,
    pub ops: u64,
    pub refused: u64,
    pub conflicts: u64,
    pub flights_held: usize,
}

/// The flight-record configuration of the tool: the national day holds about 45,000 flights, so
/// 120,000 slots leave a margin of more than two.
fn flight_config(lag_floor_ms: i64) -> FlightConfig {
    FlightConfig {
        capacity: 120_000,
        lag_floor_ms,
        max_future_ms: 5_000,
        refusal_log: 64,
        conflict_log: 64,
        removal_log: 4096,
    }
}

fn lookup(t: &FlightTable, addr: u32) -> Answer {
    match flight_by_address(t, addr) {
        None => Answer::default(),
        Some(f) if flight_is_live(&f) => Answer {
            flight: true,
            ended: false,
            assigned: f.assigned,
            interim: f.interim,
        },
        Some(_) => Answer {
            ended: true,
            ..Answer::default()
        },
    }
}

/// The answers to `queries` (aircraft address, Unix ns), in the order given.
pub fn answer_queries(
    s: &Sfdps,
    queries: &[(u32, i64)],
) -> Result<(Vec<Answer>, JoinStats), String> {
    answer_queries_counted(s, queries, &mut 0)
}

/// [`answer_queries`], with the frames and messages the replay dropped in a sealed window put in
/// `sealed` on every exit.
pub fn answer_queries_counted(
    s: &Sfdps,
    queries: &[(u32, i64)],
    sealed: &mut u64,
) -> Result<(Vec<Answer>, JoinStats), String> {
    let mut order: Vec<usize> = (0..queries.len()).collect();
    order.sort_by_key(|&k| queries.get(k).map_or(0, |q| q.1));
    let mut answers = vec![Answer::default(); queries.len()];
    let end_ns = queries.iter().map(|q| q.1).max().unwrap_or(i64::MIN);
    let paths = recordings_in(&s.dir).map_err(|e| e.to_string())?;
    let cfg = ReplayConfig {
        flight: flight_config(s.lag_floor_ms),
        from_ns: i64::MIN,
        to_ns: end_ns,
        sweep_every_ms: 60_000,
        retain_ms: 3_600_000,
        silence_ms: 6 * 3_600_000,
    };
    let mut next = 0usize;
    let rep = replay_sfdps_counted(
        paths,
        &cfg,
        &mut |t, avail_ms| {
            while let Some(&k) = order.get(next) {
                let Some(&(addr, t_ns)) = queries.get(k) else {
                    break;
                };
                if t_ns.div_euclid(1_000_000) >= avail_ms {
                    break;
                }
                if let Some(a) = answers.get_mut(k) {
                    *a = lookup(t, addr);
                }
                next += 1;
            }
        },
        sealed,
    )
    .map_err(|e| e.to_string())?;
    let st = &rep.table.stats;
    let stats = JoinStats {
        frames: rep.frames,
        sealed_window_dropped: rep.sealed_window_dropped,
        ops: st.ops,
        refused: st.refused.iter().sum(),
        conflicts: st.conflicts.iter().sum(),
        flights_held: oa_flight::flights_len(&rep.table),
    };
    Ok((answers, stats))
}

/// The clearance a pair is judged on: `Some(feet)` for a simple altitude, else why not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clearance {
    Feet(i32),
    /// VFR, VFR on top, a block, an altitude at a fix, or none: no single level.
    NotSimple,
    /// No altitude held.
    Missing,
}

fn simple(a: &Alt) -> Clearance {
    match a.form {
        ALT_SIMPLE => Clearance::Feet(a.a_ft),
        ALT_VFR | ALT_VFR_ON_TOP_PLUS | ALT_BLOCK | ALT_AT_FIX => Clearance::NotSimple,
        _ => Clearance::Missing,
    }
}

/// The assigned altitude only.
pub fn assigned_of(a: &Answer) -> Clearance {
    simple(&a.assigned)
}

/// The interim altitude where one is held (a temporary level wins), else the assigned one; an
/// interim stated as none (cleared) leaves the assigned one.
pub fn effective_of(a: &Answer) -> Clearance {
    match a.interim.form {
        ALT_SIMPLE => Clearance::Feet(a.interim.a_ft),
        ALT_NIL => simple(&a.assigned),
        _ => match simple(&a.interim) {
            Clearance::Missing => simple(&a.assigned),
            c => c,
        },
    }
}

/// How one climbing or descending encounter near 1,000 ft resolves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Cleared to altitudes 1,000 ft or more apart: the aircraft stops 1,000 ft apart.
    LevelOff,
    /// Cleared to altitudes less than 1,000 ft apart (including the same): the climb or descent
    /// does not stop here.
    ClimbThrough,
    NoFlightRecord,
    NoAssigned,
    NotSimple,
}

pub const VERDICT_NAMES: [&str; 5] = [
    "level-off (cleared 1,000 ft or more apart)",
    "climb or descent that does not stop (cleared less than 1,000 ft apart)",
    "no flight record for one or both aircraft",
    "a flight record with no altitude for one or both",
    "an altitude that is not one level (VFR, block, at a fix) for one or both",
];

fn verdict(a: &Answer, b: &Answer, of: fn(&Answer) -> Clearance) -> Verdict {
    if !a.flight || !b.flight {
        return Verdict::NoFlightRecord;
    }
    match (of(a), of(b)) {
        (Clearance::Feet(x), Clearance::Feet(y)) => {
            if (x - y).abs() >= LEVEL_OFF_FT {
                Verdict::LevelOff
            } else {
                Verdict::ClimbThrough
            }
        }
        (Clearance::NotSimple, _) | (_, Clearance::NotSimple) => Verdict::NotSimple,
        _ => Verdict::NoAssigned,
    }
}

fn vindex(v: Verdict) -> usize {
    match v {
        Verdict::LevelOff => 0,
        Verdict::ClimbThrough => 1,
        Verdict::NoFlightRecord => 2,
        Verdict::NoAssigned => 3,
        Verdict::NotSimple => 4,
    }
}

/// Whether an encounter is one of the climbing or descending encounters near 1,000 ft: airborne
/// context, not the at-vertical-minimum subgroup, a vertical separation at the closest approach
/// from [`NEAR_FROM_FT`] up to the volume's 1,000 ft, and either aircraft at [`CLIMB_FPM`] or more.
pub fn is_near_climb(e: &Encounter, airports: &[Airport], volume_v_ft: f32) -> bool {
    context_of(e, airports) == 0
        && !at_vertical_minimum(e, volume_v_ft)
        && e.v_at_closest_ft >= NEAR_FROM_FT
        && e.v_at_closest_ft < volume_v_ft
        && e.max_abs_vrate_fpm >= CLIMB_FPM
}

/// The split of the encounters near 1,000 ft, counts only, as text for the summary.
pub fn join_text(
    encounters: &[Encounter],
    answers: &[Answer],
    airports: &[Airport],
    volume_v_ft: f32,
    stats: &JoinStats,
    window: &str,
) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "\nFLIGHT RECORD JOIN\n");
    let _ = writeln!(o, "  window: {window}");
    let _ = writeln!(
        o,
        "  flight record: {} SFDPS frames replayed, {} operations, {} refused, {} conflicts listed, {} flights held at the end",
        stats.frames, stats.ops, stats.refused, stats.conflicts, stats.flights_held
    );
    let _ = writeln!(
        o,
        "  the flight record is read as of each encounter's closest approach: an operation counts from the later of its receipt and its timestamp plus the lag floor"
    );
    let _ = writeln!(
        o,
        "  the group: AIRBORNE, not AT_VERTICAL_MINIMUM, vertical separation at the closest approach {NEAR_FROM_FT:.0} ft up to {volume_v_ft:.0} ft, either aircraft at {CLIMB_FPM:.0} fpm or more; LEVEL_FT is {LEVEL_FT:.0}"
    );
    let ans = |i: usize| answers.get(i).copied().unwrap_or_default();
    // Encounter k asked 2k (address a) and 2k + 1 (address b).
    let mut before: BTreeMap<u8, u64> = BTreeMap::new();
    let mut by_class: BTreeMap<u8, [[u64; 5]; 2]> = BTreeMap::new();
    let (mut rec_both, mut rec_one, mut rec_none, mut ended_any) = (0u64, 0u64, 0u64, 0u64);
    let (mut asg_both, mut asg_one, mut asg_none) = (0u64, 0u64, 0u64);
    let mut n = 0u64;
    for (k, e) in encounters.iter().enumerate() {
        if !is_near_climb(e, airports, volume_v_ft) {
            continue;
        }
        n += 1;
        *before.entry(e.class).or_default() += 1;
        let (a, b) = (ans(2 * k), ans(2 * k + 1));
        match u8::from(a.flight) + u8::from(b.flight) {
            2 => rec_both += 1,
            1 => rec_one += 1,
            _ => rec_none += 1,
        }
        if a.ended || b.ended {
            ended_any += 1;
        }
        match u8::from(matches!(assigned_of(&a), Clearance::Feet(_)))
            + u8::from(matches!(assigned_of(&b), Clearance::Feet(_)))
        {
            2 => asg_both += 1,
            1 => asg_one += 1,
            _ => asg_none += 1,
        }
        for (v, of) in [assigned_of as fn(&Answer) -> Clearance, effective_of]
            .into_iter()
            .enumerate()
        {
            let r = verdict(&a, &b, of);
            if let Some(c) = by_class
                .entry(e.class)
                .or_default()
                .get_mut(v)
                .and_then(|t| t.get_mut(vindex(r)))
            {
                *c += 1;
            }
        }
    }
    let _ = writeln!(
        o,
        "\n  before (one group): {n} climbing or descending encounters near 1,000 ft"
    );
    for (c, m) in &before {
        let _ = writeln!(o, "    {}: {m}", class_name(*c));
    }
    let _ = writeln!(o, "  flight record found: both aircraft {rec_both}, one {rec_one}, neither {rec_none}; a flight whose plans had all ended for either: {ended_any}");
    let _ = writeln!(o, "  assigned altitude held as one level: both aircraft {asg_both}, one {asg_one}, neither {asg_none}");
    for (v, name) in ["assigned altitude", "interim altitude over assigned"]
        .iter()
        .enumerate()
    {
        let _ = writeln!(o, "\n  after, judged on the {name}, by class:");
        for (c, t) in &by_class {
            let _ = writeln!(o, "    {}:", class_name(*c));
            for (k, label) in VERDICT_NAMES.iter().enumerate() {
                let n = t.get(v).and_then(|t| t.get(k)).copied().unwrap_or(0);
                let _ = writeln!(o, "      {label}: {n}");
            }
        }
        let _ = writeln!(o, "    all classes:");
        for (k, label) in VERDICT_NAMES.iter().enumerate() {
            let n: u64 = by_class
                .values()
                .map(|t| t.get(v).and_then(|t| t.get(k)).copied().unwrap_or(0))
                .sum();
            let _ = writeln!(o, "      {label}: {n}");
        }
    }
    o
}

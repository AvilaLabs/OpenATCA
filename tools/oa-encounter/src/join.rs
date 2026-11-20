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
    flight_by_address, flight_is_live, flights_flying_by_callsign, Alt, Flight, FlightConfig,
    FlightKey, FlightTable, ALT_AT_FIX, ALT_BLOCK, ALT_NIL, ALT_SIMPLE, ALT_VFR,
    ALT_VFR_ON_TOP_PLUS, KEY_GUFI,
};
use oa_flight_sfdps::{norm_callsign, recordings_in, replay_sfdps, Positions, ReplayConfig};

use crate::report::{at_vertical_minimum, class_name, context_of, Airport, CLIMB_FPM, LEVEL_FT};

/// Vertical separations from this up to the record volume's 1,000 ft are "near 1,000 ft" for the
/// split (the 25 ft altitude quantum puts two levels 1,000 ft apart at 975 to 1,000 ft, and a
/// climb stopping at a level leaves the pair a little under that while it settles).
pub const NEAR_FROM_FT: f32 = 900.0;
/// Two clearances at least this far apart are levels 1,000 ft apart: a level-off.
pub const LEVEL_OFF_FT: i32 = 1000;

/// How the join found the flight of an aircraft.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Key {
    /// No flight.
    #[default]
    None,
    /// A live flight claims the aircraft address.
    Address,
    /// The callsign names one flying flight and its own position agrees with the ADS-B one.
    CallsignChecked,
    /// The callsign names one flying flight and it has no position within the window.
    CallsignUnchecked,
}

/// Why the callsign join found nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsReject {
    /// The ADS-B reports of the aircraft in the previous 60 s carry no callsign.
    NoCallsign,
    /// They carry more than one.
    Unstable,
    /// No flight that is flying states the callsign.
    NoLiveFlight,
    /// Several do.
    SeveralLive,
    /// The one flight's own position within 120 s is more than 10 NM from the ADS-B one.
    PositionDisagrees,
}

pub const CS_REJECT_NAMES: [&str; 5] = [
    "no callsign",
    "unstable callsign",
    "no live flight with the callsign",
    "several live flights",
    "position disagrees",
];

fn reject_index(r: CsReject) -> usize {
    match r {
        CsReject::NoCallsign => 0,
        CsReject::Unstable => 1,
        CsReject::NoLiveFlight => 2,
        CsReject::SeveralLive => 3,
        CsReject::PositionDisagrees => 4,
    }
}

/// The result of the callsign join for one aircraft.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CsResult {
    /// Not tried (the aircraft is not in a context the callsign join is asked for).
    #[default]
    NotTried,
    Accepted {
        checked: bool,
    },
    Rejected(CsReject),
}

/// What the flight record held for one aircraft at one time. A7.2: cold struct, size guard
/// waived.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    /// The flight the join chose (the address one, else the accepted callsign one) is live.
    pub flight: bool,
    /// A flight claims the address but every plan of it had ended.
    pub ended: bool,
    pub assigned: Alt,
    pub interim: Alt,
    /// How the chosen flight was found.
    pub key: Key,
    /// The flight the address names, and the flight the accepted callsign names: both kept, so
    /// the two joins can be compared where both are possible.
    pub addr_flight: Option<FlightKey>,
    pub cs_flight: Option<FlightKey>,
    pub cs: CsResult,
}

/// The callsign an aircraft's ADS-B reports carry over the 60 s before a time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CallsignObs {
    /// All the callsigns of the window are this one (upper case, trimmed).
    Stable([u8; 8]),
    Unstable,
    #[default]
    Absent,
}

/// One question: the flight of an aircraft at a time.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default)]
pub struct Query {
    pub addr: u32,
    /// Unix ns.
    pub t_ns: i64,
    pub callsign: CallsignObs,
    /// The ADS-B position at the time, 1e-7 degree.
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// Whether the callsign join is asked for as well as the address one.
    pub try_callsign: bool,
}

/// The longest a flight's own position may be from the query time and still check it, ms.
pub const POSITION_AGE_MS: i64 = 120_000;
/// The farthest a flight's own position may be from the ADS-B one, metres (10 NM).
pub const POSITION_DIST_M: f64 = 10.0 * 1852.0;
/// The window over which the ADS-B callsign must be stable, ns.
pub const STABLE_NS: i64 = 60 * 1_000_000_000;

/// The ADS-B reports of the aircraft of interest: time, position and callsign, by address, in
/// time order.
pub type Timeline = BTreeMap<u32, Vec<(i64, i32, i32, [u8; 8])>>;

/// The timeline of the reports from `from_ns` up to `to_ns` of the addresses in `want`.
pub fn timeline(
    r: &oa_adsb::Reports,
    want: &std::collections::BTreeSet<u32>,
    from_ns: i64,
    to_ns: i64,
) -> Timeline {
    let mut t: Timeline = BTreeMap::new();
    for i in 0..oa_adsb::reports_len(r) {
        let (Some(&a), Some(&ns), Some(&la), Some(&lo), Some(&c)) = (
            r.icao.get(i),
            r.time_ns.get(i),
            r.lat_e7.get(i),
            r.lon_e7.get(i),
            r.callsign.get(i),
        ) else {
            continue;
        };
        if ns >= from_ns && ns <= to_ns && want.contains(&a) {
            t.entry(a).or_default().push((ns, la, lo, norm_callsign(c)));
        }
    }
    for v in t.values_mut() {
        v.sort_by_key(|e| e.0);
    }
    t
}

/// How long before a time the latest ADS-B report may be and still give the aircraft's position, ns.
pub const ADSB_POSITION_AGE_NS: i64 = 60 * 1_000_000_000;

/// The callsign state of `addr` over the 60 s up to `t_ns`, and its latest position not after it
/// and within [`ADSB_POSITION_AGE_NS`].
pub fn observe(tl: &Timeline, addr: u32, t_ns: i64) -> (CallsignObs, Option<(i32, i32)>) {
    let Some(v) = tl.get(&addr) else {
        return (CallsignObs::Absent, None);
    };
    let hi = v.partition_point(|e| e.0 <= t_ns);
    let lo = v.partition_point(|e| e.0 <= t_ns - STABLE_NS);
    let window = v.get(lo..hi).unwrap_or(&[]);
    let mut seen: Option<[u8; 8]> = None;
    let mut obs = CallsignObs::Absent;
    for e in window.iter().filter(|e| e.3 != [0; 8]) {
        match seen {
            None => {
                seen = Some(e.3);
                obs = CallsignObs::Stable(e.3);
            }
            Some(c) if c == e.3 => {}
            Some(_) => {
                obs = CallsignObs::Unstable;
                break;
            }
        }
    }
    let pos = hi
        .checked_sub(1)
        .and_then(|k| v.get(k))
        .filter(|e| t_ns - e.0 <= ADSB_POSITION_AGE_NS)
        .map(|e| (e.1, e.2));
    (obs, pos)
}

/// Great-circle distance between two positions of 1e-7 degree, metres.
pub fn distance_m(a: (i32, i32), b: (i32, i32)) -> f64 {
    let (la1, lo1) = (f64::from(a.0) / 1e7, f64::from(a.1) / 1e7);
    let (la2, lo2) = (f64::from(b.0) / 1e7, f64::from(b.1) / 1e7);
    let (p1, p2) = (la1.to_radians(), la2.to_radians());
    let dl = (lo2 - lo1).to_radians();
    let h = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * h.sqrt().min(1.0).asin()
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
    pub ops: u64,
    pub refused: u64,
    pub conflicts: u64,
    pub flights_held: usize,
    /// Flights that reported a position, and positions the bounded table could not take.
    pub positions: usize,
    pub positions_refused: u64,
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

fn cs_join(t: &FlightTable, pos: &Positions, q: &Query) -> (CsResult, Option<Flight>) {
    let c = match q.callsign {
        CallsignObs::Absent => return (CsResult::Rejected(CsReject::NoCallsign), None),
        CallsignObs::Unstable => return (CsResult::Rejected(CsReject::Unstable), None),
        CallsignObs::Stable(c) => c,
    };
    let (n, first) = flights_flying_by_callsign(t, &c);
    let Some(f) = first else {
        return (CsResult::Rejected(CsReject::NoLiveFlight), None);
    };
    if n > 1 {
        return (CsResult::Rejected(CsReject::SeveralLive), None);
    }
    let t_ms = q.t_ns.div_euclid(1_000_000);
    let own = (f.key.kind == KEY_GUFI)
        .then(|| pos.get(f.key.gufi))
        .flatten()
        .filter(|p| (t_ms - p.ts_ms).abs() <= POSITION_AGE_MS);
    match own {
        Some(p) if distance_m((p.lat_e7, p.lon_e7), (q.lat_e7, q.lon_e7)) > POSITION_DIST_M => {
            (CsResult::Rejected(CsReject::PositionDisagrees), None)
        }
        Some(_) => (CsResult::Accepted { checked: true }, Some(f)),
        None => (CsResult::Accepted { checked: false }, Some(f)),
    }
}

fn lookup(t: &FlightTable, pos: &Positions, q: &Query) -> Answer {
    let mut a = Answer::default();
    let mut chosen = None;
    match flight_by_address(t, q.addr) {
        Some(f) if flight_is_live(&f) => {
            a.addr_flight = Some(f.key);
            a.key = Key::Address;
            chosen = Some(f);
        }
        Some(_) => a.ended = true,
        None => {}
    }
    if q.try_callsign {
        let (r, f) = cs_join(t, pos, q);
        a.cs = r;
        a.cs_flight = f.map(|f| f.key);
        if chosen.is_none() {
            if let (CsResult::Accepted { checked }, Some(f)) = (r, f) {
                a.key = if checked {
                    Key::CallsignChecked
                } else {
                    Key::CallsignUnchecked
                };
                chosen = Some(f);
            }
        }
    }
    if let Some(f) = chosen {
        a.flight = true;
        a.assigned = f.assigned;
        a.interim = f.interim;
    }
    a
}

/// The answers to `queries`, in the order given. A query at time T is answered before the first
/// operation that is available after T is applied, and sees the positions of the same moment: for
/// the address join and the callsign join alike.
pub fn answer_queries(s: &Sfdps, queries: &[Query]) -> Result<(Vec<Answer>, JoinStats), String> {
    let mut order: Vec<usize> = (0..queries.len()).collect();
    order.sort_by_key(|&k| queries.get(k).map_or(0, |q| q.t_ns));
    let mut answers = vec![Answer::default(); queries.len()];
    let end_ns = queries.iter().map(|q| q.t_ns).max().unwrap_or(i64::MIN);
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
    let rep = replay_sfdps(paths, &cfg, &mut |t, pos, avail_ms| {
        while let Some(&k) = order.get(next) {
            let Some(q) = queries.get(k) else {
                break;
            };
            if q.t_ns.div_euclid(1_000_000) >= avail_ms {
                break;
            }
            if let Some(a) = answers.get_mut(k) {
                *a = lookup(t, pos, q);
            }
            next += 1;
        }
    })
    .map_err(|e| e.to_string())?;
    let st = &rep.table.stats;
    let stats = JoinStats {
        frames: rep.frames,
        ops: st.ops,
        refused: st.refused.iter().sum(),
        conflicts: st.conflicts.iter().sum(),
        flights_held: oa_flight::flights_len(&rep.table),
        positions: rep.positions.len(),
        positions_refused: rep.positions.refused_full,
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
    o.push_str(&key_text(encounters, answers, airports, volume_v_ft));
    o
}

/// The two joins compared where both name a flight.
#[derive(Clone, Copy, Default)]
struct Cross {
    both: u64,
    same: u64,
    diff: u64,
    checked: u64,
    diff_checked: u64,
    diff_unchecked: u64,
}

impl Cross {
    fn add(&mut self, same: bool, checked: bool) {
        self.both += 1;
        self.checked += u64::from(checked);
        if same {
            self.same += 1;
        } else {
            self.diff += 1;
            if checked {
                self.diff_checked += 1;
            } else {
                self.diff_unchecked += 1;
            }
        }
    }
}

fn key_rank(k: Key) -> u8 {
    match k {
        Key::Address => 0,
        Key::CallsignChecked => 1,
        Key::CallsignUnchecked => 2,
        Key::None => 3,
    }
}

const KEY_NAMES: [&str; 4] = [
    "address",
    "callsign, position checked",
    "callsign, no position check",
    "none",
];

/// The join split by the key that found the flight, the callsign rejections, and the two keys
/// compared where both are possible. Counts only.
fn key_text(
    encounters: &[Encounter],
    answers: &[Answer],
    airports: &[Airport],
    volume_v_ft: f32,
) -> String {
    let mut o = String::new();
    let ans = |i: usize| answers.get(i).copied().unwrap_or_default();
    let _ = writeln!(
        o,
        "\n  join by key (the callsign is used only where the address names no live flight)"
    );
    let _ = writeln!(
        o,
        "  callsign rule: one flying flight with the callsign (not ended, not dropped); its own SFDPS position within {} s, if it has one, within 10 NM of the ADS-B one; the ADS-B callsign stable over the previous 60 s",
        POSITION_AGE_MS / 1000
    );
    let mut per_key = [0u64; 4];
    let mut rejects = [0u64; 5];
    let (mut aircraft, mut not_tried) = (0u64, 0u64);
    let (mut addr_both, mut addr_one, mut addr_none) = (0u64, 0u64, 0u64);
    let (mut both, mut one, mut none) = (0u64, 0u64, 0u64);
    let mut weakest = [0u64; 3];
    let mut by_weakest = [[0u64; 5]; 3];
    let mut n = 0u64;
    // [group, all tried]: both possible, same flight, different flight, checked-different, unchecked-different.
    let mut cross = [Cross::default(); 2];
    for (k, e) in encounters.iter().enumerate() {
        let (a, b) = (ans(2 * k), ans(2 * k + 1));
        let group = is_near_climb(e, airports, volume_v_ft);
        for x in [&a, &b] {
            if x.cs == CsResult::NotTried {
                continue;
            }
            if let (Some(af), Some(cf), CsResult::Accepted { checked }) =
                (x.addr_flight, x.cs_flight, x.cs)
            {
                for (c, on) in cross.iter_mut().zip([group, true]) {
                    if on {
                        c.add(af == cf, checked);
                    }
                }
            }
        }
        if !group {
            continue;
        }
        n += 1;
        let v = verdict(&a, &b, assigned_of);
        for x in [&a, &b] {
            aircraft += 1;
            if let Some(c) = per_key.get_mut(usize::from(key_rank(x.key))) {
                *c += 1;
            }
            if x.cs == CsResult::NotTried {
                not_tried += 1;
            }
            if x.addr_flight.is_none() {
                if let CsResult::Rejected(r) = x.cs {
                    if let Some(c) = rejects.get_mut(reject_index(r)) {
                        *c += 1;
                    }
                }
            }
        }
        match u8::from(a.addr_flight.is_some()) + u8::from(b.addr_flight.is_some()) {
            2 => addr_both += 1,
            1 => addr_one += 1,
            _ => addr_none += 1,
        }
        match u8::from(a.flight) + u8::from(b.flight) {
            2 => {
                both += 1;
                let w = key_rank(a.key).max(key_rank(b.key)).min(2);
                if let Some(c) = weakest.get_mut(usize::from(w)) {
                    *c += 1;
                }
                if let Some(c) = by_weakest
                    .get_mut(usize::from(w))
                    .and_then(|r| r.get_mut(vindex(v)))
                {
                    *c += 1;
                }
            }
            1 => one += 1,
            _ => none += 1,
        }
    }
    let _ = writeln!(
        o,
        "  the group: {n} encounters, {aircraft} aircraft ({not_tried} without the callsign join)"
    );
    let _ = writeln!(o, "  aircraft by the key that found their flight:");
    for (name, c) in KEY_NAMES.iter().zip(per_key) {
        let _ = writeln!(o, "    {name}: {c}");
    }
    let _ = writeln!(
        o,
        "  callsign rejections, aircraft whose address named no live flight:"
    );
    for (name, c) in CS_REJECT_NAMES.iter().zip(rejects) {
        let _ = writeln!(o, "    {name}: {c}");
    }
    let _ = writeln!(o, "  flight record for both aircraft, address only: {addr_both} of {n}; one {addr_one}, neither {addr_none}");
    let _ = writeln!(o, "  flight record for both aircraft, address then callsign: {both} of {n}; one {one}, neither {none}");
    let _ = writeln!(
        o,
        "  encounters with a record for both, by the weakest key of the two:"
    );
    for (k, name) in KEY_NAMES.iter().take(3).enumerate() {
        let r = by_weakest.get(k).copied().unwrap_or([0; 5]);
        let _ = writeln!(
            o,
            "    {name}: {} (level-off {}, does not stop {}, no altitude {}, not one level {})",
            weakest.get(k).copied().unwrap_or(0),
            r[0],
            r[1],
            r[3],
            r[4]
        );
    }
    let _ = writeln!(
        o,
        "  address and callsign compared where both name a flight (the callsign error):"
    );
    for (label, c) in [
        ("group aircraft", cross[0]),
        ("every aircraft the callsign join was tried for", cross[1]),
    ] {
        let _ = writeln!(
            o,
            "    {label}: both possible {}, the same flight {}, different flights {} (callsign checked {} of the both-possible, different with check {}, different without {})",
            c.both, c.same, c.diff, c.checked, c.diff_checked, c.diff_unchecked
        );
    }
    o
}

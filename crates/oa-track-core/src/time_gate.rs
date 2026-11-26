//! The tracker's clock decision as a pure function, so it can be proved (docs/VERIFICATION.md).
//!
//! [`gate_decide`] is given the tracker clock and one measurement's time, address and sensor, and
//! says what becomes of the measurement: it is applied ([`Verdict::Accept`]), refused as too old
//! ([`Verdict::Late`]), counted and not applied while a step of the clock is unconfirmed
//! ([`Verdict::Held`]), or it moves the clock ([`Verdict::StepForward`], [`Verdict::StepBack`]).
//! It does no I/O and no allocation: its state is a fixed-size record. The tracker owns the clock
//! and acts on the verdict; the gate only keeps the runs of far-ahead and of late measurements
//! and the table of sensors that recently delivered.
//!
//! No single measurement may move the clock by as much as the drop age (the forward limit is
//! below it): a measurement that did would expire every live track, and the real measurements
//! after it would all be late. Farther than the limit, a step needs a run of consecutive
//! far-ahead measurements from enough distinct sensors and addresses that one faulty or forged
//! source cannot make it; when only the measurement's own track and sensor are live there is
//! nothing to protect and it steps at once. The way back from a clock that is wrong-ahead is the
//! same idea mirrored: a run of late refusals that spans time, is long, and is not one sensor's or
//! one aircraft's; sensors live only because of data ahead of that run's real time do not count
//! towards the sensors it needs. The tracker keeps the held measurements of a run and applies
//! them when it confirms: confirmation delays data, it does not lose it.

use crate::params::CLOCK_STEP_CONFIRM;

/// Capacity of the buffer of (sensor, address) pairs of a run: the last this many are kept. The
/// default Kani model uses the smallest value that holds the late run; `kani_full_capacity` uses
/// the production capacity.
#[cfg(any(not(kani), kani_full_capacity))]
pub const RUN_BUF: usize = 8;
#[cfg(all(kani, not(kani_full_capacity)))]
pub const RUN_BUF: usize = 6;
/// Capacity of the table of sensors that recently delivered (sensor id to its newest accepted
/// time). Default Kani proofs use a smaller table; `kani_full_capacity` uses this production size.
#[cfg(any(not(kani), kani_full_capacity))]
pub const SENSOR_TABLE: usize = 64;
#[cfg(all(kani, not(kani_full_capacity)))]
pub const SENSOR_TABLE: usize = 4;
/// Late refusals in a row (with no accepted measurement between) that can step the clock back.
pub const BACK_CONFIRM: u32 = 2 * CLOCK_STEP_CONFIRM;
/// Distinct sensors wanted by a step, when that many are live.
const WANT_SENSORS: usize = 2;
/// Distinct addresses wanted by a step, when that many tracks are live.
const WANT_ADDRS: usize = 3;

/// The times the gate compares with, ns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateCfg {
    /// Farthest a measurement may be ahead of the clock and still be applied without a
    /// confirmed step.
    pub limit_ns: i64,
    /// Out-of-order window: a measurement older than the clock by more is late.
    pub ooo_ns: i64,
    /// A sensor is live when it delivered within this long of the clock.
    pub drop_ns: i64,
}

/// What becomes of a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Apply it: the clock stays or moves forward by at most the limit.
    Accept,
    /// Older than the clock by more than the window: counted and dropped.
    Late,
    /// Far ahead and not confirmed: counted and not applied.
    Held,
    /// Far ahead and confirmed: the clock takes its time.
    StepForward,
    /// The clock was wrong-ahead: it takes `to_ns`, the newest time of the late run, and the
    /// caller drops what was built on the wrong clock. The measurement is then applied.
    StepBack { to_ns: i64 },
}

/// A run of measurements: how many, the time span, and who they came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Run {
    n: u32,
    /// The latest time added (forward run: the one the next must not precede by the window).
    last_ns: i64,
    first_ns: i64,
    newest_ns: i64,
    toa: [i64; RUN_BUF],
    sensor: [u16; RUN_BUF],
    addr: [u32; RUN_BUF],
    head: usize,
    /// Which slots of the two buffers hold a pair (all, once the run is `RUN_BUF` long).
    used: [bool; RUN_BUF],
}

const EMPTY_RUNS: Runs = Runs {
    fwd: EMPTY_RUN,
    back: EMPTY_RUN,
    restarted: false,
};

const EMPTY_RUN: Run = Run {
    n: 0,
    last_ns: 0,
    first_ns: 0,
    newest_ns: 0,
    toa: [0; RUN_BUF],
    sensor: [0; RUN_BUF],
    addr: [0; RUN_BUF],
    head: 0,
    used: [false; RUN_BUF],
};

impl Run {
    fn push(&mut self, toa: i64, addr: u32, sensor: u16) {
        if self.n == 0 {
            self.first_ns = toa;
            self.newest_ns = toa;
        }
        self.newest_ns = self.newest_ns.max(toa);
        self.last_ns = toa;
        self.n = self.n.saturating_add(1);
        // Written through a loop over fixed indices, not a computed one: cheaper to prove.
        for i in 0..RUN_BUF {
            if i == self.head {
                if let Some(x) = self.toa.get_mut(i) {
                    *x = toa;
                }
                if let Some(s) = self.sensor.get_mut(i) {
                    *s = sensor;
                }
                if let Some(a) = self.addr.get_mut(i) {
                    *a = addr;
                }
                if let Some(u) = self.used.get_mut(i) {
                    *u = true;
                }
            }
        }
        self.head = (self.head + 1) % RUN_BUF;
    }

    /// The run already holds this exact measurement (time, address and sensor): a repeat of it
    /// is not another member.
    fn has(&self, toa: i64, addr: u32, sensor: u16) -> bool {
        let mut found = false;
        for i in 0..RUN_BUF {
            if self.used.get(i) == Some(&true)
                && self.toa.get(i) == Some(&toa)
                && self.addr.get(i) == Some(&addr)
                && self.sensor.get(i) == Some(&sensor)
            {
                found = true;
            }
        }
        found
    }

    fn distinct_sensors(&self) -> usize {
        distinct(&self.sensor, &self.used)
    }

    fn distinct_addrs(&self) -> usize {
        distinct(&self.addr, &self.used)
    }

    /// The run comes from at least `min(2, live_sensors)` sensors and `min(3, live_tracks)`
    /// addresses.
    fn spread_ok(&self, live_sensors: usize, live_tracks: usize) -> bool {
        self.distinct_sensors() >= WANT_SENSORS.min(live_sensors)
            && self.distinct_addrs() >= WANT_ADDRS.min(live_tracks)
    }
}

/// The number of distinct values among the used slots of `v`. Plain loops over fixed indices,
/// not iterator chains or computed bounds: the proof (below) is much cheaper over them.
fn distinct<T: Copy + PartialEq>(v: &[T; RUN_BUF], used: &[bool; RUN_BUF]) -> usize {
    let mut n = 0;
    for i in 0..RUN_BUF {
        let mut seen = false;
        for j in 0..i {
            if used.get(j) == Some(&true) && v.get(i) == v.get(j) {
                seen = true;
            }
        }
        if used.get(i) == Some(&true) && !seen {
            n += 1;
        }
    }
    n
}

/// What the decision is told of the live tracks and sensors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Live {
    /// Live tracks.
    tracks: usize,
    /// Live sensors (accepted within the drop age of the clock).
    sensors: usize,
    /// Live sensors that count towards a step back: those whose newest accepted time is no later
    /// than the late run's newest time plus the window. A sensor live only because of data ahead
    /// of the real time is the suspect, and must not raise the bar for the real ones.
    sensors_back: usize,
    /// Every live track is the measurement's own address (or there is none) and every live
    /// sensor is its own sensor (or there is none): only its own track is at stake.
    alone: bool,
}

/// The two runs the decision keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Runs {
    /// Consecutive far-ahead measurements, none earlier than the one before by more than the
    /// window.
    fwd: Run,
    /// Late refusals since the last accepted measurement.
    back: Run,
    /// The last far-ahead measurement started a new run (the run before it, if any, was refuted).
    restarted: bool,
}

/// The gate's state: the two runs and the sensor table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateState {
    runs: Runs,
    sensor_id: [u16; SENSOR_TABLE],
    sensor_ns: [i64; SENSOR_TABLE],
    sensor_used: [bool; SENSOR_TABLE],
    /// Table entries reused while the table was full (the least recently accepted sensor lost
    /// its place).
    pub evicted: u64,
}

/// A gate with nothing seen.
pub fn gate_new() -> GateState {
    GateState {
        runs: EMPTY_RUNS,
        sensor_id: [0; SENSOR_TABLE],
        sensor_ns: [i64::MIN; SENSOR_TABLE],
        sensor_used: [false; SENSOR_TABLE],
        evicted: 0,
    }
}

/// Sensors with a measurement accepted within `drop_ns` of `clock_ns`.
pub fn gate_live_sensors(g: &GateState, clock_ns: i64, drop_ns: i64) -> usize {
    let floor = clock_ns.saturating_sub(drop_ns);
    let mut n = 0;
    for i in 0..SENSOR_TABLE {
        if g.sensor_used.get(i) == Some(&true) && g.sensor_ns.get(i).is_some_and(|&ns| ns >= floor)
        {
            n += 1;
        }
    }
    n
}

/// Live sensors as [`gate_live_sensors`], counting only those whose newest accepted time is at
/// most `cap_ns`.
fn live_sensors_upto(g: &GateState, clock_ns: i64, drop_ns: i64, cap_ns: i64) -> usize {
    let floor = clock_ns.saturating_sub(drop_ns);
    let mut n = 0;
    for i in 0..SENSOR_TABLE {
        if g.sensor_used.get(i) == Some(&true)
            && g.sensor_ns
                .get(i)
                .is_some_and(|&ns| ns >= floor && ns <= cap_ns)
        {
            n += 1;
        }
    }
    n
}

/// Whether `sensor` is one of the live sensors.
fn sensor_is_live(g: &GateState, clock_ns: i64, drop_ns: i64, sensor: u16) -> bool {
    let floor = clock_ns.saturating_sub(drop_ns);
    let mut live = false;
    for i in 0..SENSOR_TABLE {
        if g.sensor_used.get(i) == Some(&true)
            && g.sensor_id.get(i) == Some(&sensor)
            && g.sensor_ns.get(i).is_some_and(|&ns| ns >= floor)
        {
            live = true;
        }
    }
    live
}

/// Notes that `sensor` delivered a measurement at `toa` that the caller applied without asking
/// the gate (a held one, after its run confirmed).
pub fn gate_note_applied(g: &mut GateState, sensor: u16, toa: i64) {
    sensor_note(g, sensor, toa);
}

/// Whether the last decision started a new run of far-ahead measurements.
pub fn gate_run_restarted(g: &GateState) -> bool {
    g.runs.restarted
}

/// Notes that `sensor` delivered an accepted measurement at `toa`.
fn sensor_note(g: &mut GateState, sensor: u16, toa: i64) {
    let mut found = None;
    let mut free = None;
    let mut oldest = 0;
    let mut oldest_ns = i64::MAX;
    for i in 0..SENSOR_TABLE {
        if g.sensor_used.get(i) == Some(&true) {
            if g.sensor_id.get(i) == Some(&sensor) {
                found = Some(i);
            }
            if let Some(&ns) = g.sensor_ns.get(i) {
                if ns < oldest_ns {
                    oldest_ns = ns;
                    oldest = i;
                }
            }
        } else if free.is_none() {
            free = Some(i);
        }
    }
    let slot = match (found, free) {
        (Some(i), _) | (None, Some(i)) => i,
        // Full: reuse the least recently accepted sensor's place.
        (None, None) => {
            g.evicted = g.evicted.saturating_add(1);
            oldest
        }
    };
    for i in 0..SENSOR_TABLE {
        if i == slot {
            if let Some(u) = g.sensor_used.get_mut(i) {
                *u = true;
            }
            if let Some(id) = g.sensor_id.get_mut(i) {
                *id = sensor;
            }
            if let Some(ns) = g.sensor_ns.get_mut(i) {
                let keep = if found.is_some() { *ns } else { i64::MIN };
                *ns = keep.max(toa);
            }
        }
    }
}

/// Decides what becomes of a measurement at `toa` from `sensor` for `addr`, given the tracker
/// clock (`i64::MIN` before the first measurement), the number of live tracks and whether every
/// live track is the measurement's own address. Records what it needs of the measurement; the
/// caller moves the clock for every verdict but `Late` and `Held`.
#[allow(clippy::too_many_arguments)]
pub fn gate_decide(
    g: &mut GateState,
    cfg: &GateCfg,
    clock_ns: i64,
    toa: i64,
    addr: u32,
    sensor: u16,
    live_tracks: usize,
    own_addr_only: bool,
) -> Verdict {
    let live_sensors = gate_live_sensors(g, clock_ns, cfg.drop_ns);
    let own_sensor_only = live_sensors == 0
        || (live_sensors == 1 && sensor_is_live(g, clock_ns, cfg.drop_ns, sensor));
    let back_newest = if g.runs.back.n > 0 {
        g.runs.back.newest_ns.max(toa)
    } else {
        toa
    };
    let live = Live {
        tracks: live_tracks,
        sensors: live_sensors,
        sensors_back: live_sensors_upto(
            g,
            clock_ns,
            cfg.drop_ns,
            back_newest.saturating_add(cfg.ooo_ns),
        ),
        alone: own_addr_only && own_sensor_only,
    };
    let v = runs_decide(&mut g.runs, cfg, clock_ns, toa, addr, sensor, &live);
    match v {
        Verdict::Late | Verdict::Held => {}
        Verdict::Accept | Verdict::StepForward => sensor_note(g, sensor, toa),
        Verdict::StepBack { to_ns } => {
            // What the forged or faulty clock's sensors delivered is in the future of the new one.
            for i in 0..SENSOR_TABLE {
                if g.sensor_ns.get(i).is_some_and(|&ns| ns > to_ns) {
                    if let Some(u) = g.sensor_used.get_mut(i) {
                        *u = false;
                    }
                }
            }
            sensor_note(g, sensor, toa);
        }
    }
    v
}

/// The decision itself, given what is live: the runs and nothing else, so the proofs of the step
/// rules do not carry the sensor table.
fn runs_decide(
    r: &mut Runs,
    cfg: &GateCfg,
    clock_ns: i64,
    toa: i64,
    addr: u32,
    sensor: u16,
    live: &Live,
) -> Verdict {
    r.restarted = false;
    if clock_ns == i64::MIN {
        *r = EMPTY_RUNS;
        return Verdict::Accept;
    }
    if toa < clock_ns.saturating_sub(cfg.ooo_ns) {
        if !r.back.has(toa, addr, sensor) {
            r.back.push(toa, addr, sensor);
        }
        let span = r.back.newest_ns.saturating_sub(r.back.first_ns);
        if r.back.n >= BACK_CONFIRM
            && span >= cfg.ooo_ns
            && r.back.spread_ok(live.sensors_back, live.tracks)
        {
            let to_ns = r.back.newest_ns;
            *r = EMPTY_RUNS;
            return Verdict::StepBack { to_ns };
        }
        return Verdict::Late;
    }
    if toa <= clock_ns.saturating_add(cfg.limit_ns) {
        *r = EMPTY_RUNS;
        return Verdict::Accept;
    }
    // Far ahead.
    let continues = r.fwd.n > 0 && toa >= r.fwd.last_ns.saturating_sub(cfg.ooo_ns);
    if !continues {
        r.fwd = EMPTY_RUN;
        r.restarted = true;
    }
    // An exact repeat of a member adds nothing: a run is of distinct measurements.
    if !r.fwd.has(toa, addr, sensor) {
        r.fwd.push(toa, addr, sensor);
    }
    // With no live track nothing can be expired by it; with only its own track (and its own
    // sensor) live, only that aircraft is affected: nothing to protect, no run needed.
    if live.tracks == 0
        || live.alone
        || (r.fwd.n >= CLOCK_STEP_CONFIRM && r.fwd.spread_ok(live.sensors, live.tracks))
    {
        let restarted = r.restarted;
        *r = EMPTY_RUNS;
        r.restarted = restarted;
        return Verdict::StepForward;
    }
    Verdict::Held
}

#[cfg(all(kani, not(kani_full_capacity)))]
mod proofs {
    //! Kani proof harnesses (docs/VERIFICATION.md). Compiled only under `cargo kani`.
    use super::*;

    fn any_cfg() -> GateCfg {
        let c = GateCfg {
            limit_ns: kani::any(),
            ooo_ns: kani::any(),
            drop_ns: kani::any(),
        };
        // What the configuration check enforces.
        kani::assume(c.ooo_ns >= 0 && c.limit_ns >= c.ooo_ns && c.drop_ns > c.limit_ns);
        // And its time bound: no time over one day.
        kani::assume(c.drop_ns <= 86_400 * 1_000_000_000);
        c
    }

    /// Times within a quarter of the tracker's range (`TIME_LIMIT_NS` is half of `i64::MAX`), so
    /// that differences of two of them do not overflow.
    const BOUND: u64 = 1 << 61;

    /// The clock the tracker holds after a verdict.
    fn next_clock(clock: i64, toa: i64, v: Verdict) -> i64 {
        match v {
            Verdict::Accept => clock.max(toa),
            Verdict::StepForward => toa,
            Verdict::StepBack { to_ns } => to_ns,
            Verdict::Late | Verdict::Held => clock,
        }
    }

    /// The number of distinct values among the first `n` of `v` (independent of the gate's).
    fn count_distinct(v: &[u32], n: usize) -> usize {
        let mut c = 0;
        for i in 0..n {
            let mut seen = false;
            for j in 0..i {
                if v[j] == v[i] {
                    seen = true;
                }
            }
            if !seen {
                c += 1;
            }
        }
        c
    }

    // REQ: OA-TRK-193
    #[kani::proof]
    #[kani::unwind(10)]
    #[kani::solver(cadical)]
    fn gate_decide_is_total() {
        let cfg = any_cfg();
        let mut g = gate_new();
        let mut clock: i64 = kani::any();
        // Any time, address, sensor and track count, the clock included (`i64::MIN` too): no
        // panic, overflow or out-of-bounds access. Six calls fill the four-place sensor table and
        // reuse a place.
        for _ in 0..6 {
            let toa: i64 = kani::any();
            let live: usize = kani::any();
            let v = gate_decide(
                &mut g,
                &cfg,
                clock,
                toa,
                kani::any(),
                kani::any(),
                live,
                kani::any(),
            );
            clock = next_clock(clock, toa, v);
        }
    }

    // REQ: OA-TRK-198
    #[kani::proof]
    #[kani::unwind(10)]
    #[kani::solver(cadical)]
    fn gate_accept_is_within_window_and_limit() {
        // The verdict Accept depends on the clock and the time alone, not on the runs: one call.
        let cfg = any_cfg();
        let mut g = gate_new();
        let clock: i64 = kani::any();
        kani::assume(clock == i64::MIN || clock.unsigned_abs() < BOUND);
        let toa: i64 = kani::any();
        kani::assume(toa.unsigned_abs() < BOUND);
        let live: usize = kani::any();
        let v = gate_decide(
            &mut g,
            &cfg,
            clock,
            toa,
            kani::any(),
            kani::any(),
            live,
            kani::any(),
        );
        if let Verdict::Accept = v {
            if clock != i64::MIN {
                assert!(toa - clock <= cfg.limit_ns);
                assert!(clock - toa <= cfg.ooo_ns);
            }
        }
    }

    // REQ: OA-TRK-194
    #[kani::proof]
    #[kani::unwind(10)]
    #[kani::solver(cadical)]
    fn gate_step_forward_needs_a_confirmed_run() {
        let cfg = any_cfg();
        let mut r = EMPTY_RUNS;
        let clock: i64 = kani::any();
        kani::assume(clock.unsigned_abs() < BOUND);
        let live = Live {
            tracks: kani::any(),
            sensors: kani::any(),
            sensors_back: kani::any(),
            alone: kani::any(),
        };
        // With no live track a far-ahead measurement steps at once (nothing can be expired).
        kani::assume(live.tracks > 0);
        let mut addrs = [0u32; 3];
        let mut sensors = [0u32; 3];
        let mut prev = i64::MIN;
        for k in 0..3 {
            let toa: i64 = kani::any();
            let a: u32 = kani::any();
            let s: u16 = kani::any();
            // One unbroken run of far-ahead measurements: nondecreasing, past the limit.
            kani::assume(toa >= prev && toa.unsigned_abs() < BOUND);
            kani::assume(toa > clock + cfg.limit_ns);
            prev = toa;
            addrs[k] = a;
            sensors[k] = u32::from(s);
            let v = runs_decide(&mut r, &cfg, clock, toa, a, s, &live);
            // Far ahead: held or stepped, nothing else.
            assert!(matches!(v, Verdict::Held | Verdict::StepForward));
            if live.alone {
                // Only the measurement's own track and sensor are live: it steps at once.
                assert!(v == Verdict::StepForward);
                kani::cover!(k == 0, "a lone step is reachable");
            } else if k < 2 {
                // Fewer than three in a row never steps.
                assert!(v == Verdict::Held);
            } else if v == Verdict::StepForward {
                assert!(count_distinct(&addrs, 3) >= WANT_ADDRS.min(live.tracks));
                assert!(count_distinct(&sensors, 3) >= WANT_SENSORS.min(live.sensors));
                kani::cover!(true, "a confirmed step is reachable");
            }
        }
    }

    // REQ: OA-TRK-195
    #[kani::proof]
    #[kani::unwind(10)]
    #[kani::solver(cadical)]
    fn gate_step_back_needs_span_count_and_spread() {
        let cfg = any_cfg();
        let mut r = EMPTY_RUNS;
        let clock: i64 = kani::any();
        kani::assume(clock.unsigned_abs() < BOUND);
        let live = Live {
            tracks: kani::any(),
            sensors: kani::any(),
            sensors_back: kani::any(),
            alone: kani::any(),
        };
        let mut addrs = [0u32; 6];
        let mut sensors = [0u32; 6];
        let (mut first, mut newest) = (0i64, i64::MIN);
        for k in 0..6 {
            let toa: i64 = kani::any();
            let a: u32 = kani::any();
            let s: u16 = kani::any();
            // Every one is late.
            kani::assume(toa.unsigned_abs() < BOUND && toa < clock - cfg.ooo_ns);
            addrs[k] = a;
            sensors[k] = u32::from(s);
            if k == 0 {
                first = toa;
            }
            newest = newest.max(toa);
            let v = runs_decide(&mut r, &cfg, clock, toa, a, s, &live);
            if k < 5 {
                // Fewer than six refusals never step back.
                assert!(v == Verdict::Late);
            } else {
                match v {
                    Verdict::StepBack { to_ns } => {
                        assert_eq!(to_ns, newest);
                        assert!(newest - first >= cfg.ooo_ns);
                        assert!(count_distinct(&addrs, 6) >= WANT_ADDRS.min(live.tracks));
                        assert!(count_distinct(&sensors, 6) >= WANT_SENSORS.min(live.sensors_back));
                        kani::cover!(true, "a step back is reachable");
                    }
                    Verdict::Late => {}
                    _ => panic!("a late measurement got another verdict"),
                }
            }
        }
    }
}

#[cfg(all(kani, kani_full_capacity))]
mod proofs_full;

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// A full table: sensor `i` last accepted at `times[i]`.
    fn full(times: &[i64; SENSOR_TABLE]) -> GateState {
        let mut g = gate_new();
        for (i, &t) in times.iter().enumerate() {
            gate_note_applied(&mut g, i as u16, t);
        }
        assert_eq!(g.evicted, 0);
        g
    }

    // REQ: OA-TRK-336
    #[test]
    fn the_live_count_of_a_full_table_is_the_number_of_sensors_inside_the_drop_interval() {
        let mut times = [0i64; SENSOR_TABLE];
        for (i, t) in times.iter_mut().enumerate() {
            *t = 1_000 + 10 * i as i64;
        }
        let g = full(&times);
        let drop_ns = 100;
        for clock in [
            0, 900, 1_000, 1_100, 1_345, 1_639, 1_640, 1_739, 1_740, 5_000,
        ] {
            // Counted by hand from the list of times: those at or after `clock - drop`.
            let want = times.iter().filter(|&&t| t >= clock - drop_ns).count();
            assert_eq!(gate_live_sensors(&g, clock, drop_ns), want, "clock {clock}");
        }
        assert_eq!(gate_live_sensors(&g, 1_000, 0), SENSOR_TABLE);
        assert_eq!(gate_live_sensors(&g, 5_000, 0), 0);
    }

    // REQ: OA-TRK-336
    #[test]
    fn the_live_count_of_a_full_table_holds_at_the_signed_extremes_and_never_grows_with_the_clock()
    {
        let mut times = [0i64; SENSOR_TABLE];
        for (i, t) in times.iter_mut().enumerate() {
            *t = match i % 4 {
                0 => i64::MIN,
                1 => i64::MAX,
                2 => 0,
                _ => -1_000 * i as i64,
            };
        }
        let g = full(&times);
        for drop_ns in [0, 1, 86_400 * 1_000_000_000] {
            let mut last = SENSOR_TABLE;
            for clock in [i64::MIN, -5_000, -1, 0, 1, 5_000, i64::MAX - 1, i64::MAX] {
                let n = gate_live_sensors(&g, clock, drop_ns);
                let floor = clock.saturating_sub(drop_ns);
                assert_eq!(n, times.iter().filter(|&&t| t >= floor).count());
                assert!(
                    n <= SENSOR_TABLE && n <= last,
                    "clock {clock} drop {drop_ns}"
                );
                last = n;
            }
        }
    }

    // REQ: OA-TRK-336
    #[test]
    fn the_live_count_of_a_full_table_matches_an_i128_reference_for_random_times_clocks_and_drops()
    {
        const MAX_DROP_NS: u64 = 86_400 * 1_000_000_000;
        // splitmix64: the same sweep on every run, no outside crate.
        struct Sweep(u64);
        impl Sweep {
            fn next(&mut self) -> u64 {
                self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
                let mut z = self.0;
                z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
                z ^ (z >> 31)
            }
            /// Any i64, with the extremes and zero drawn often.
            fn time(&mut self) -> i64 {
                match self.next() % 32 {
                    0 => i64::MIN,
                    1 => i64::MAX,
                    2 => 0,
                    _ => self.next() as i64,
                }
            }
        }
        let mut sw = Sweep(0x7a11_9a7e);
        let (mut none_live, mut all_live) = (0u32, 0u32);
        for case in 0..20_000u32 {
            let mut times = [0i64; SENSOR_TABLE];
            for t in &mut times {
                *t = sw.time();
            }
            // Every table holds the signed minimum; the maximum is there half of the time.
            times[0] = i64::MIN;
            if case % 2 == 0 {
                times[1] = i64::MAX;
            }
            let g = full(&times);
            let clock = sw.time();
            let drop_ns = (sw.next() % (MAX_DROP_NS + 1)) as i64;
            // The reference in 128 bits, with no saturation to reason about.
            let floor = (i128::from(clock) - i128::from(drop_ns)).max(i128::from(i64::MIN));
            let want = times.iter().filter(|&&t| i128::from(t) >= floor).count();
            let got = gate_live_sensors(&g, clock, drop_ns);
            assert_eq!(got, want, "case {case}");
            none_live += u32::from(got == 0);
            all_live += u32::from(got == SENSOR_TABLE);
            // The clock moving forward by the same drop never adds a live sensor.
            let later = clock.saturating_add((sw.next() % 1_000_000_000_000) as i64);
            assert!(gate_live_sensors(&g, later, drop_ns) <= got, "case {case}");
        }
        assert!(none_live > 0 && all_live > 100, "{none_live} {all_live}");
    }
}

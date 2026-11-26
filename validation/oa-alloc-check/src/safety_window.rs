//! Allocation window for the safety-net core (`oa-safety`) and its display indications
//! (`oa-display::safety`). Shell (B3): a bounded fixture and its evidence. C5: after every table is
//! built, the net state reports, the inhibition create, query, suppress, remove and expire routes
//! (with their refusals, a full table and a full suppressed log) and the safety build with every
//! banner kind must not call the global allocator.
#![forbid(unsafe_code)]
use super::allocator::{self, Counts};
use oa_display::list::{list_clear, list_new};
use oa_display::project::projection_new;
use oa_display::safety::{
    build_safety, safety_caps, FeedState, FeedStatus, SafetyScene, FEED_NAME_BYTES,
};
use oa_display::scene::LEGEND_STRIP_PX;
use oa_display::traffic::targets_new;
use oa_safety::geom::Vertex;
use oa_safety::inhibit::{
    events_take, inhibit_config_none, inhibit_create, inhibit_expire, inhibit_remove,
    inhibit_suppress, inhibits_new, inhibits_refused_full, inhibits_refused_invalid,
    inhibits_slice, suppressed_log, suppressed_overflow, Event, EventKind, InhibitConfig,
    InhibitError, InhibitRequest, AUTHOR_BYTES, INHIBIT_CAP, SUPPRESSED_CAP,
};
use oa_safety::net::{
    net_status, state_backdated_applied, state_future_rejected, state_report, state_table_new,
    Area, NetId, NetReport, NetState, Reason, NETS, NET_COUNT,
};

const S: i64 = 1_000_000_000;
const NOW: i64 = 1_794_772_800 * S;
const ORD: (i32, i32) = (419_786_000, -879_048_000);
/// A valid position the projection cannot place (opposite the centre).
const UNPLACED: (i32, i32) = (-422_586_000, 918_952_000);
const FRAMES: i64 = 20;
const SUPPRESS_EXTRA: usize = 3;

/// A7.2: cold evidence for the safety windows, summed over the window.
pub(super) struct SafetyEvidence {
    pub counts: Counts,
    pub state_reports: u64,
    pub stale_unavailable: u64,
    pub created: u64,
    pub refused: u64,
    pub refused_full: u64,
    pub suppressed_logged: u64,
    pub suppressed_overflow: u64,
    pub removed: u64,
    pub expired: u64,
    pub events: u64,
    pub banner_lines: u64,
    pub inhibitions_drawn: u64,
    pub inhibitions_unplaced: u64,
    pub state_backdated: u64,
    pub state_future_refused: u64,
    pub banner_hidden: u64,
    pub error_builds: u64,
    pub injected: bool,
}

fn square(lat: i32, lon: i32, half: i32) -> [Vertex; 4] {
    [
        (lat - half, lon - half),
        (lat + half, lon - half),
        (lat + half, lon + half),
        (lat - half, lon + half),
    ]
}

fn config() -> InhibitConfig {
    InhibitConfig {
        max_duration_ns: [Some(3600 * S); NET_COUNT],
        max_start_ahead_ns: Some(3600 * S),
        max_extent_e7: Some(100_000_000),
        max_alert_suppression_ns: None,
        allow_military_group_suppression: false,
    }
}

fn request(net: NetId, vertices: &[Vertex], start: i64, end: i64) -> InhibitRequest<'_> {
    InhibitRequest {
        net,
        vertices,
        alt_low_ft: 0,
        alt_high_ft: 5000,
        start_ns: start,
        end_ns: end,
        reason: "RANGE 3 TEST FIRING",
        author: "SUP J SMITH",
    }
}

/// Runs `work` once inside one armed window; with `inject`, one allocation is forced inside it
/// (the negative control), and a failure to force it is an error.
fn measure(inject: bool, frames: i64, mut work: impl FnMut(i64)) -> Result<Counts, &'static str> {
    if !allocator::start() {
        return Err("allocation counters were not clean before a safety window");
    }
    for k in 0..frames {
        work(k);
    }
    if inject && !allocator::inject_core_allocation() {
        return Err("injected allocation failed");
    }
    allocator::stop().ok_or("safety window did not disarm cleanly")
}

fn add_counts(a: &mut Counts, b: Counts) {
    a.alloc = a.alloc.saturating_add(b.alloc);
    a.zeroed = a.zeroed.saturating_add(b.zeroed);
    a.realloc = a.realloc.saturating_add(b.realloc);
    a.overflow |= b.overflow;
}

pub(super) fn safety_window(inject: bool) -> Result<SafetyEvidence, &'static str> {
    // Everything up to the first `measure` is initialisation (C1): not counted.
    let mut counts = Counts {
        alloc: 0,
        zeroed: 0,
        realloc: 0,
        overflow: false,
    };
    let cfg = config();
    let none = inhibit_config_none();
    let area = square(ORD.0, ORD.1, 1_000_000);
    let bad_area: [Vertex; 3] = [(0, 0), (10, 10), (20, 20)];
    let targets = targets_new(1).map_err(|_| "targets_new")?;
    let mut inh = Box::new(inhibits_new());
    let mut st = state_table_new(10 * S, 5 * S).map_err(|_| "state_table_new")?;
    let mut ev = [Event {
        time_ns: 0,
        id: 0,
        kind: EventKind::Created,
        net: NetId::ConflictAlert,
        by: [0; AUTHOR_BYTES],
    }; 8];

    // Window 1: state reports, status, the inhibition routes and the logs, one scripted pass.
    let (mut reports, mut stale) = (0u64, 0u64);
    let (mut backdated, mut future) = (0u64, 0u64);
    let (mut created, mut refused, mut removed, mut expired, mut events) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut first_id = 0u32;
    let mut script_ok = true;
    let c = measure(inject, 1, |_| {
        for n in NETS {
            let rep = match n {
                NetId::ConflictAlert => NetReport::Degraded {
                    reason: Reason::BudgetExceeded,
                    area: Area::Polygon(7),
                    count: 12,
                },
                NetId::Msaw => NetReport::Unavailable {
                    reason: Reason::AltimeterRefused,
                },
                _ => NetReport::Running,
            };
            script_ok &= state_report(&mut st, n, rep, NOW, NOW).is_ok();
            reports += 1;
        }
        // A backwards report is refused; a late query finds every net stale.
        script_ok &= state_report(&mut st, NetId::Msaw, NetReport::Running, NOW - S, NOW).is_err();
        // A report dated far ahead is refused; a worse backdated one replaces the state.
        script_ok &= state_report(
            &mut st,
            NetId::AreaProximity,
            NetReport::Running,
            i64::MAX,
            NOW,
        )
        .is_err();
        script_ok &=
            state_report(&mut st, NetId::AreaProximity, NetReport::Running, NOW, NOW).is_ok();
        script_ok &= state_report(
            &mut st,
            NetId::AreaProximity,
            NetReport::Unavailable {
                reason: Reason::Maintenance,
            },
            NOW - S,
            NOW,
        )
        .is_ok();
        backdated = u64::from(state_backdated_applied(&st));
        future = u64::from(state_future_rejected(&st));
        for n in NETS {
            let s = net_status(&st, &inh, n, NOW + 60 * S);
            if s.state
                == (NetState::Unavailable {
                    reason: Reason::StateStale,
                })
            {
                stale += 1;
            }
        }
        // Valid requests, then the refusals: no maximum, too long, bad polygon.
        for net in [NetId::Msaw, NetId::ConflictAlert, NetId::Msaw] {
            match inhibit_create(
                &mut inh,
                &cfg,
                &request(net, &area, NOW, NOW + 1800 * S),
                NOW,
            ) {
                Ok(id) => {
                    created += 1;
                    if first_id == 0 {
                        first_id = id;
                    }
                }
                Err(_) => script_ok = false,
            }
        }
        let refusals = [
            (&none, &area[..], NOW + S, InhibitError::NoMaxConfigured),
            (&cfg, &area[..], NOW + 7200 * S, InhibitError::TooLong),
            (
                &cfg,
                &bad_area[..],
                NOW + S,
                InhibitError::BadPolygon("zero area"),
            ),
        ];
        for (c, v, end, want) in refusals {
            match inhibit_create(&mut inh, c, &request(NetId::Msaw, v, NOW, end), NOW) {
                Err(e)
                    if matches!(
                        (&e, &want),
                        (InhibitError::NoMaxConfigured, InhibitError::NoMaxConfigured)
                            | (InhibitError::TooLong, InhibitError::TooLong)
                            | (InhibitError::BadPolygon(_), InhibitError::BadPolygon(_))
                    ) =>
                {
                    refused += 1
                }
                _ => script_ok = false,
            }
        }
        // Fill the table; the next request is refused as full.
        while inhibits_slice(&inh).len() < INHIBIT_CAP {
            script_ok &= inhibit_create(
                &mut inh,
                &cfg,
                &request(NetId::Msaw, &area, NOW, NOW + 1800 * S),
                NOW,
            )
            .is_ok();
        }
        script_ok &= matches!(
            inhibit_create(
                &mut inh,
                &cfg,
                &request(NetId::Msaw, &area, NOW, NOW + 1800 * S),
                NOW
            ),
            Err(InhibitError::TableFull)
        );
        // Suppress past the log's capacity.
        for k in 0..(SUPPRESSED_CAP + SUPPRESS_EXTRA) {
            let s = inhibit_suppress(
                &mut inh,
                NetId::ConflictAlert,
                ORD.0,
                ORD.1,
                Some(2000),
                NOW + k as i64,
            );
            script_ok &= s.count == 1;
        }
        // Drain the events (the log is full of creations), remove one early, expire the rest.
        events += drain(&mut inh, &mut ev);
        script_ok &= inhibit_remove(&mut inh, first_id, "SUP B JONES", NOW + S).is_ok();
        removed += 1;
        expired += u64::from(inhibit_expire(&mut inh, NOW + 7200 * S));
        events += drain(&mut inh, &mut ev);
        loop {
            let n = inhibit_expire(&mut inh, NOW + 7200 * S);
            if n == 0 {
                break;
            }
            expired += u64::from(n);
            events += drain(&mut inh, &mut ev);
        }
    })?;
    add_counts(&mut counts, c);
    if !script_ok {
        return Err("a measured safety state or inhibition step failed");
    }
    let suppressed_logged = suppressed_log(&inh).len() as u64;
    let overflow = suppressed_overflow(&inh);
    let refused_full = u64::from(inhibits_refused_full(&inh));
    refused_total_check(&inh, refused)?;

    // Window 2: the safety build with every banner kind, over a fresh set of tables.
    let mut inh2 = Box::new(inhibits_new());
    let mut st2 = state_table_new(10 * S, 5 * S).map_err(|_| "state_table_new")?;
    st2_reports(&mut st2)?;
    let mut ids_ok = true;
    let unplaced_area = square(UNPLACED.0, UNPLACED.1, 1_000_000);
    for (net, v) in [
        (NetId::Msaw, &area[..]),
        (NetId::ApproachPath, &unplaced_area[..]),
    ] {
        ids_ok &= inhibit_create(
            &mut inh2,
            &cfg,
            &request(net, v, NOW - 60 * S, NOW + 1800 * S),
            NOW - 60 * S,
        )
        .is_ok();
    }
    if !ids_ok {
        return Err("safety display inhibitions were refused");
    }
    let feeds = [
        FeedStatus {
            name: name8("ADSB"),
            state: FeedState::Slow { age_ms: 3200 },
        },
        FeedStatus {
            name: name8("ASR9"),
            state: FeedState::Lost { age_ms: 31_050 },
        },
        FeedStatus {
            name: name8("MODES"),
            state: FeedState::Live,
        },
    ];
    let proj = projection_new(ORD.0, ORD.1, 60.0, 1920, 1080, LEGEND_STRIP_PX)
        .map_err(|_| "projection_new")?;
    let mut list = list_new(safety_caps()).map_err(|_| "list_new")?;
    let (mut lines, mut drawn, mut unplaced) = (0u64, 0u64, 0u64);
    let mut build_ok = true;
    let c = measure(inject, FRAMES, |_| {
        list_clear(&mut list);
        let sc = SafetyScene {
            states: &st2,
            inhibits: &inh2,
            now_ns: NOW,
            feeds: &feeds,
            // Older than the limit: the picture is frozen.
            newest_track_ns: Some(NOW - 30 * S),
            frozen_limit_ns: 5 * S,
            targets: &targets,
        };
        match build_safety(&mut list, &proj, &sc) {
            Ok(s) => {
                lines += u64::from(s.banner_lines);
                drawn += u64::from(s.inhibitions_drawn);
                unplaced += u64::from(s.inhibitions_unplaced);
            }
            Err(_) => build_ok = false,
        }
    })?;
    add_counts(&mut counts, c);
    if !build_ok {
        return Err("a measured safety build failed");
    }

    // Window 3: a frame too small for the banner, and a build that fails (the alarm path).
    let small = projection_new(ORD.0, ORD.1, 60.0, 800, 200, LEGEND_STRIP_PX)
        .map_err(|_| "projection_new small")?;
    let (mut hidden, mut error_builds) = (0u64, 0u64);
    let c = measure(inject, FRAMES, |_| {
        list_clear(&mut list);
        let mut sc = SafetyScene {
            states: &st2,
            inhibits: &inh2,
            now_ns: NOW,
            feeds: &feeds,
            newest_track_ns: Some(NOW - 30 * S),
            frozen_limit_ns: 5 * S,
            targets: &targets,
        };
        if let Ok(s) = build_safety(&mut list, &small, &sc) {
            hidden += u64::from(s.banner_hidden);
        }
        list_clear(&mut list);
        sc.frozen_limit_ns = 0;
        if build_safety(&mut list, &proj, &sc).is_err() {
            error_builds += 1;
        }
    })?;
    add_counts(&mut counts, c);

    Ok(SafetyEvidence {
        counts,
        state_reports: reports,
        stale_unavailable: stale,
        created,
        refused,
        refused_full,
        suppressed_logged,
        suppressed_overflow: overflow,
        removed,
        expired,
        events,
        banner_lines: lines,
        inhibitions_drawn: drawn,
        inhibitions_unplaced: unplaced,
        state_backdated: backdated,
        state_future_refused: future,
        banner_hidden: hidden,
        error_builds,
        injected: true,
    })
}

/// The invalid-request counter agrees with the refusals the script made (outside the window).
fn refused_total_check(
    inh: &oa_safety::inhibit::InhibitTable,
    refused: u64,
) -> Result<(), &'static str> {
    if u64::from(inhibits_refused_invalid(inh)) == refused {
        Ok(())
    } else {
        Err("the refusal counter disagrees with the script")
    }
}

fn st2_reports(st: &mut oa_safety::net::StateTable) -> Result<(), &'static str> {
    for n in NETS {
        let rep = match n {
            NetId::ConflictAlert => NetReport::Degraded {
                reason: Reason::BudgetExceeded,
                area: Area::Polygon(7),
                count: 12,
            },
            NetId::Msaw => NetReport::Running,
            NetId::AreaProximity => NetReport::Unavailable {
                reason: Reason::InputMissing,
            },
            NetId::ApproachPath => NetReport::Running,
        };
        // The approach net reported long ago: it is the stale one.
        let at = if n == NetId::ApproachPath {
            NOW - 30 * S
        } else {
            NOW
        };
        state_report(st, n, rep, at, NOW).map_err(|_| "state_report")?;
    }
    Ok(())
}

/// Takes every event out of the log and returns how many there were.
fn drain(inh: &mut oa_safety::inhibit::InhibitTable, ev: &mut [Event]) -> u64 {
    let mut total = 0u64;
    loop {
        let n = events_take(inh, ev);
        if n == 0 {
            return total;
        }
        total += n as u64;
    }
}

fn name8(s: &str) -> [u8; FEED_NAME_BYTES] {
    let mut out = [0u8; FEED_NAME_BYTES];
    for (d, b) in out.iter_mut().zip(s.bytes()) {
        *d = b;
    }
    out
}

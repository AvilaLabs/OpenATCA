//! The model's own checks: validation catches every kind of inconsistency, lookups answer
//! absent keys with `None`, and the bitset and string table behave at their edges.

mod common;

use common::*;
use oa_airspace::model::*;

fn base() -> Model {
    fixture_build().model
}

fn rejects(name: &str, f: impl FnOnce(&mut Model)) {
    let mut m = base();
    f(&mut m);
    assert!(validate_model(&m).is_err(), "{name} was not caught");
}

// REQ: OA-ASP-001
#[test]
fn validation_accepts_the_fixture_and_the_empty_model() {
    validate_model(&base()).expect("fixture");
    let m = empty_model(base().meta);
    validate_model(&m).expect("empty");
    assert_eq!(str_count(&m.strings), 0);
}

// REQ: OA-ASP-001
#[test]
fn validation_catches_every_kind_of_inconsistency() {
    rejects("short column", |m| {
        m.points.elev_ft.pop();
    });
    rejects("long column", |m| m.legs.alt1_ft.push(0));
    rejects("string index", |m| m.points.name[0] = StrIx(u32::MAX - 1));
    rejects("point index", |m| m.legs.fix[0] = PointIx(u32::MAX - 1));
    rejects("airway point", |m| {
        m.airway_legs.from[0] = PointIx(m.points.kind.len() as u32)
    });
    rejects("point code", |m| m.points.kind[0] = 0);
    rejects("point code high", |m| {
        m.points.kind[0] = point_kind::MAX + 1
    });
    rejects("boundary source", |m| m.boundaries.source[0] = 0);
    rejects("boundary stratum", |m| {
        m.boundaries.stratum[0] = stratum::MAX + 1
    });
    rejects("path terminator", |m| {
        m.legs.path_term[0] = path_term::MAX + 1
    });
    rejects("procedure kind", |m| m.procs.kind[0] = 0);
    rejects("segment via", |m| m.segments.via[0] = 0);
    rejects("unresolved role", |m| m.unresolved.role[0] = 0);
    rejects("key order", |m| {
        let last = m.points.ident.len() - 1;
        m.points.ident.swap(0, last);
    });
    rejects("duplicate key", |m| {
        m.points.ident[1] = m.points.ident[0];
        m.points.region[1] = m.points.region[0];
        m.points.scope[1] = m.points.scope[0];
        m.points.kind[1] = m.points.kind[0];
    });
    rejects("span gap", |m| m.procs.routes[0].start = RouteIx(1));
    rejects("span overrun", |m| {
        let n = m.procs.routes.len();
        m.procs.routes[n - 1].len += 1;
    });
    rejects("span overlap", |m| m.airways.legs[1].start = AirwayLegIx(0));
    rejects("vertex span", |m| m.boundaries.vertices[0].len += 1);
    rejects("bitset length", |m| {
        m.points.in_area.len -= 1;
    });
    rejects("bitset tail", |m| {
        let n = m.points.cls_b.len as usize;
        if !n.is_multiple_of(64) {
            *m.points.cls_b.words.last_mut().expect("words") |= 1u64 << 63;
        } else {
            m.points.cls_b.words.pop();
        }
    });
    rejects("string offsets", |m| {
        let n = m.strings.offsets.len();
        m.strings.offsets[n - 1] += 1;
    });
    rejects("string offset order", |m| m.strings.offsets.swap(1, 2));
    rejects("position range", |m| m.points.lat[0] = 900_000_001);
    rejects("mora order", |m| {
        m.mora.lat.swap(0, 5);
        m.mora.lon.swap(0, 5);
    });
}

// REQ: OA-ASP-001
#[test]
fn lookups_find_present_keys_and_answer_none_for_absent_ones() {
    let m = base();
    assert!(find_proc(&m, "KORD", proc_kind::STAR, "BENKY6").is_some());
    assert!(find_proc(&m, "KORD", proc_kind::STAR, "NOPE1").is_none());
    assert!(find_proc(&m, "KJFK", proc_kind::STAR, "BENKY6").is_none());
    assert!(find_airway(&m, "V156", b'C').is_some());
    assert!(find_airway(&m, "V156", b'A').is_none());
    assert!(find_airway(&m, "", b'C').is_none());
    assert!(find_boundary(&m, boundary_source::NASR_ARB, stratum::ARTCC_HIGH, "ZAU", 0).is_some());
    assert!(find_boundary(&m, boundary_source::NASR_ARB, stratum::ARTCC_HIGH, "ZZZ", 0).is_none());
    // Out-of-range handles never panic.
    assert!(point_key(&m, PointIx::NONE).is_none());
    assert!(point_key(&m, PointIx(m.points.kind.len() as u32)).is_none());
    assert_eq!(at(&m.points.lat, u32::MAX), None);
    assert_eq!(str_at(&m.strings, StrIx(u32::MAX - 1)), None);
    assert_eq!(str_or_empty(&m.strings, StrIx::NONE), "");
    // Binary search agrees with a linear scan for every point.
    for i in 0..m.points.kind.len() {
        let ix = PointIx(i as u32);
        let k = point_key(&m, ix).expect("key");
        assert_eq!(find_point(&m, k), Some(ix));
    }
}

// REQ: OA-ASP-001
#[test]
fn bitsets_hold_exactly_their_length() {
    let mut b = bit_zeros(130);
    assert_eq!((b.len, b.words.len()), (130, 3));
    assert_eq!(bit_count(&b), 0);
    for i in [0usize, 63, 64, 129] {
        bit_set(&mut b, i, true);
    }
    assert_eq!(bit_count(&b), 4);
    assert!(bit_get(&b, 63) && bit_get(&b, 64) && bit_get(&b, 129) && !bit_get(&b, 65));
    // Out of range reads are false and writes are ignored.
    assert!(!bit_get(&b, 130) && !bit_get(&b, usize::MAX));
    bit_set(&mut b, 130, true);
    bit_set(&mut b, usize::MAX, true);
    assert_eq!(bit_count(&b), 4);
    bit_set(&mut b, 63, false);
    assert_eq!(bit_count(&b), 3);
    let mut p = bit_zeros(0);
    for i in 0..200 {
        bit_push(&mut p, i % 3 == 0);
    }
    assert_eq!((p.len, p.words.len()), (200, bit_words(200)));
    assert_eq!(bit_count(&p), 67);
    assert_eq!(bit_words(0), 0);
    assert_eq!(bit_words(64), 1);
    assert_eq!(bit_words(65), 2);
}

// REQ: OA-ASP-001
#[test]
fn code_names_cover_every_code_and_flag_unknown_ones() {
    for k in 1..=point_kind::MAX {
        assert_ne!(point_kind::name(k), "unknown kind");
    }
    assert_eq!(point_kind::name(0), "unknown kind");
    for s in 1..=stratum::MAX {
        assert_ne!(stratum::name(s), "unknown stratum");
    }
    for r in 1..=unresolved_role::MAX {
        assert_ne!(unresolved_role::name(r), "unknown role");
    }
    for k in 1..=proc_kind::MAX {
        assert_ne!(proc_kind::name(k), "unknown procedure kind");
    }
    assert_eq!(path_term::name(0), "--");
    assert_eq!(path_term::name(1), "AF");
    assert_eq!(path_term::name(path_term::MAX), "VR");
    assert_eq!(path_term::name(path_term::MAX + 1), "--");
    assert_eq!(path_term::NAMES.len(), usize::from(path_term::MAX));
}

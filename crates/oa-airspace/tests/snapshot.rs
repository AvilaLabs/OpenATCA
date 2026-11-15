//! The snapshot reader and writer: limits, rejection of damaged input, and mutation testing.
//! The reader is bounded and no input may make it panic.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::*;
use oa_airspace::model::*;
use oa_airspace::snapshot::*;
use oa_sha256::sha256;

fn fixture_bytes() -> Vec<u8> {
    encode(&fixture_build().model).expect("encode")
}

/// Deterministic pseudo-random sequence (no `rand`: the crate has no dependencies).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

fn tiny_model() -> Model {
    empty_model(Meta {
        nasr_effective: 20_261_029,
        cifp_cycle: 2611,
        artcc: "ZAU".to_string(),
        buffer_m: 92_600,
        nasr_sha256: [1; 32],
        cifp_sha256: [2; 32],
    })
}

// REQ: OA-ASP-006
#[test]
fn an_empty_model_round_trips_and_has_a_fixed_layout() {
    let m = tiny_model();
    let bytes = encode(&m).expect("encode");
    assert_eq!(&bytes[..11], b"OATCA-ADAPT");
    assert_eq!(
        u32::from_le_bytes(bytes[11..15].try_into().expect("4 bytes")),
        FORMAT_VERSION
    );
    assert_eq!(
        u32::from_le_bytes(bytes[15..19].try_into().expect("4 bytes")),
        20_261_029,
        "little-endian"
    );
    assert_eq!(decode(&bytes).expect("decode"), m);
    assert_eq!(
        encode(&decode(&bytes).expect("decode")).expect("encode"),
        bytes
    );
    // Thirteen tables, each introduced by its tag, in the documented order.
    let tags = [
        "STRS", "PNTS", "VERT", "BNDS", "SEGS", "AWYS", "AWLG", "PROC", "RTES", "LEGS", "MORA",
        "UNRS", "DISA",
    ];
    let mut at = 0;
    for tag in tags {
        let found = bytes[at..]
            .windows(4)
            .position(|w| w == tag.as_bytes())
            .unwrap_or_else(|| panic!("{tag} missing"));
        at += found + 4;
    }
}

// REQ: OA-ASP-006
#[test]
fn sentinels_and_all_value_ranges_survive_a_round_trip() {
    let mut m = tiny_model();
    // One of everything a column can hold at its extremes.
    let mut st = StrTable::default();
    for text in ["", "K5", "ZAU", "naïve"] {
        st.text.push_str(text);
        st.offsets.push(st.text.len() as u32);
    }
    m.strings = st;
    m.vertices = vec![PosE7 {
        lat: i32::MIN,
        lon: i32::MAX,
    }];
    m.boundaries = Boundaries {
        source: vec![boundary_source::NASR_ARB],
        stratum: vec![stratum::ARTCC_HIGH],
        edge_interp: vec![edge_interp::UNKNOWN],
        multiple: vec![0],
        facility: vec![StrIx::new(2)],
        name: vec![StrIx::NONE],
        floor_ft: vec![ALT_NONE],
        ceiling_ft: vec![ALT_UNLIMITED],
        vertices: vec![Span {
            start: VertexIx::new(0),
            len: 1,
        }],
        segs: vec![Span {
            start: SegIx::new(0),
            len: 0,
        }],
        floor_agl: bit_zeros(1),
        ceiling_agl: bit_zeros(1),
        time_code: vec![time_code::UNKNOWN],
        level: vec![level::UNKNOWN],
    };
    m.mora = Mora {
        lat: vec![-90, 89],
        lon: vec![-180, 179],
        hundreds_ft: vec![MORA_UNKNOWN, 1_000],
    };
    // A vertex off the globe is refused at write time.
    assert!(matches!(encode(&m), Err(SnapshotError::Invalid(_))));
    m.vertices = vec![PosE7 {
        lat: 900_000_000,
        lon: -1_800_000_000,
    }];
    let bytes = encode(&m).expect("encode");
    let back = decode(&bytes).expect("decode");
    assert_eq!(back, m);
    assert_eq!(
        str_at(&back.strings, StrIx::new(3)),
        Some("naïve"),
        "UTF-8 strings survive"
    );
    assert_eq!(str_at(&back.strings, StrIx::NONE), None);
}

// REQ: OA-ASP-012
#[test]
fn the_writer_enforces_the_limits_the_reader_enforces() {
    let mut m = fixture_build().model;
    m.meta.artcc = "A".repeat(MAX_ARTCC_BYTES);
    let bytes = encode(&m).expect("a name at the limit is written");
    assert_eq!(
        decode(&bytes).expect("and read back").meta.artcc.len(),
        MAX_ARTCC_BYTES
    );
    m.meta.artcc = "A".repeat(MAX_ARTCC_BYTES + 1);
    assert_eq!(encode(&m), Err(SnapshotError::BadArtcc));
    // The reader refuses what the writer refuses.
    let mut raw = bytes.clone();
    let at = MAGIC.len() + 4 * 4;
    raw[at..at + 4].copy_from_slice(&((MAX_ARTCC_BYTES + 1) as u32).to_le_bytes());
    assert_eq!(decode(&raw), Err(SnapshotError::BadArtcc));
}

// REQ: OA-ASP-006
#[test]
fn a_dropped_or_duplicated_row_changes_the_snapshot_identity_or_is_refused() {
    let m = fixture_build().model;
    let original = encode(&m).expect("encode");
    let id = sha256(&original);
    let rows = m.mora.lat.len();
    assert!(rows >= 3);
    for victim in [0, rows / 2, rows - 1] {
        // Drop one MORA cell from every column of the table.
        let mut dropped = m.clone();
        dropped.mora.lat.remove(victim);
        dropped.mora.lon.remove(victim);
        dropped.mora.hundreds_ft.remove(victim);
        let bytes = encode(&dropped).expect("a table with one cell fewer is still consistent");
        assert_ne!(bytes, original, "drop {victim}");
        assert_ne!(sha256(&bytes), id, "drop {victim}");
        assert_eq!(decode(&bytes).expect("decode").mora.lat.len(), rows - 1);
        // Repeat one: the cell key is no longer unique, so the model is refused.
        let mut twice = m.clone();
        twice.mora.lat.insert(victim, m.mora.lat[victim]);
        twice.mora.lon.insert(victim, m.mora.lon[victim]);
        twice
            .mora
            .hundreds_ft
            .insert(victim, m.mora.hundreds_ft[victim]);
        assert!(
            matches!(encode(&twice), Err(SnapshotError::Invalid(_))),
            "dup {victim}"
        );
    }
    // Dropping the last row of a table that has no key order (unresolved references) is
    // accepted by validation, and the identity still changes.
    let mut fewer = m.clone();
    let u = &mut fewer.unresolved;
    u.role.pop();
    u.owner_kind.pop();
    u.owner.pop();
    u.scope.pop();
    u.ordinal.pop();
    u.name.pop();
    u.region.pop();
    u.wanted.pop();
    assert_ne!(sha256(&encode(&fewer).expect("encode")), id);
}

// REQ: OA-ASP-006
#[test]
fn an_inconsistent_model_is_not_written() {
    let mut m = fixture_build().model;
    m.points.lat.pop();
    assert!(matches!(encode(&m), Err(SnapshotError::Invalid(_))));
    let mut m = fixture_build().model;
    m.procs.routes[0].len += 1;
    assert!(matches!(encode(&m), Err(SnapshotError::Invalid(_))));
}

// REQ: OA-GEN-003
#[test]
fn the_reader_rejects_input_over_its_stated_limits() {
    // The file size limit is checked before anything else.
    let big = vec![0u8; MAX_SNAPSHOT_BYTES + 1];
    assert_eq!(
        decode(&big),
        Err(SnapshotError::FileTooLarge {
            len: MAX_SNAPSHOT_BYTES + 1
        })
    );
    // The ARTCC length limit.
    let mut bytes = encode(&tiny_model()).expect("encode");
    let at = 11 + 4 + 12; // after magic, version, three u32 fields: the artcc length
    bytes[at..at + 4].copy_from_slice(&((MAX_ARTCC_BYTES as u32) + 1).to_le_bytes());
    assert!(decode(&bytes).is_err());
    bytes[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode(&bytes).is_err());
    // A row count larger than the file could hold is refused before any allocation.
    let good = fixture_bytes();
    for tag in [
        "PNTS", "VERT", "BNDS", "SEGS", "AWYS", "AWLG", "PROC", "RTES", "LEGS", "MORA", "UNRS",
        "DISA",
    ] {
        let at = good
            .windows(4)
            .position(|w| w == tag.as_bytes())
            .expect("tag")
            + 4;
        let mut b = good.clone();
        b[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        match decode(&b) {
            Err(SnapshotError::CountExceedsFile { table, .. }) => assert_eq!(table, tag, "{tag}"),
            other => panic!("{tag}: {other:?}"),
        }
    }
}

// REQ: OA-ASP-006
#[test]
fn damaged_headers_trailing_bytes_and_truncation_are_errors() {
    let good = fixture_bytes();
    assert_eq!(decode(&[]), Err(SnapshotError::Truncated("magic")));
    let mut bad = good.clone();
    bad[0] ^= 1;
    assert_eq!(decode(&bad), Err(SnapshotError::BadMagic));
    let mut bad = good.clone();
    bad[11] = 3;
    assert_eq!(decode(&bad), Err(SnapshotError::BadVersion(3)));
    // The first format (no special-use time code or level) is refused, not misread.
    bad[11] = 1;
    assert_eq!(decode(&bad), Err(SnapshotError::BadVersion(1)));
    let mut bad = good.clone();
    bad.push(0);
    assert_eq!(decode(&bad), Err(SnapshotError::TrailingBytes { extra: 1 }));
    // Every strict prefix is rejected (sampled: every length near the ends, then a stride).
    let step = (good.len() / 700).max(1);
    let lens: Vec<usize> = (0..good.len())
        .filter(|n| *n < 300 || good.len() - n < 300 || n % step == 0)
        .collect();
    for n in lens {
        assert!(decode(&good[..n]).is_err(), "prefix of {n} bytes accepted");
    }
}

// REQ: OA-GEN-004
#[test]
fn no_mutation_of_a_snapshot_makes_the_reader_panic() {
    let good = fixture_bytes();
    let mut accepted = 0;
    let mut rejected = 0;
    let mut check = |bytes: &[u8]| match decode(bytes) {
        Ok(m) => {
            // Whatever the reader accepts is consistent and can be written back.
            validate_model(&m).expect("decode validates");
            encode(&m).expect("an accepted model re-encodes");
            accepted += 1;
        }
        Err(e) => {
            assert!(!e.to_string().is_empty());
            rejected += 1;
        }
    };
    // Every byte of the header region, four patterns each.
    for at in 0..600.min(good.len()) {
        for pat in [0x01u8, 0xff, 0x00, 0x80] {
            let mut b = good.clone();
            b[at] = if pat == 0x01 { b[at] ^ 1 } else { pat };
            check(&b);
        }
    }
    // Pseudo-random positions across the whole file, and runs of garbage.
    let mut rng = Lcg(7);
    for _ in 0..1_500 {
        let mut b = good.clone();
        let at = (rng.next() as usize) % b.len();
        b[at] = rng.next() as u8;
        check(&b);
    }
    for _ in 0..60 {
        let mut b = good.clone();
        let at = (rng.next() as usize) % b.len();
        let run = 1 + (rng.next() as usize) % 64;
        for k in at..(at + run).min(b.len()) {
            b[k] = rng.next() as u8;
        }
        check(&b);
    }
    // Purely random input and random input after a valid header.
    for n in [0usize, 1, 11, 19, 100, 5_000] {
        let mut b: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        check(&b);
        if n >= 19 {
            b[..11].copy_from_slice(b"OATCA-ADAPT");
            b[11..15].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
            check(&b);
        }
    }
    assert!(rejected > 1_000, "{rejected}");
    // Some single-byte changes land in values the format cannot check (a latitude digit): those
    // are accepted, and that is fine because the snapshot hash is the integrity check.
    assert!(accepted > 0, "{accepted}");
}

// REQ: OA-GEN-004
#[test]
fn every_table_column_mutation_is_caught_or_consistent() {
    // Rewrite each u32 count in the first 19 bytes of every table with a nearby value.
    let good = fixture_bytes();
    for tag in [
        "STRS", "PNTS", "VERT", "BNDS", "SEGS", "AWYS", "AWLG", "PROC", "RTES", "LEGS", "MORA",
        "UNRS", "DISA",
    ] {
        let at = good
            .windows(4)
            .position(|w| w == tag.as_bytes())
            .expect("tag")
            + 4;
        let rows = u32::from_le_bytes(good[at..at + 4].try_into().expect("4 bytes"));
        for delta in [-1i64, 1, 2, 1_000] {
            let mut b = good.clone();
            let v = (i64::from(rows) + delta).clamp(0, i64::from(u32::MAX)) as u32;
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
            if v == rows {
                continue; // clamped at zero: not a change
            }
            assert!(
                decode(&b).is_err(),
                "{tag}: {rows} rows rewritten as {v} was accepted"
            );
        }
    }
}

// REQ: OA-GEN-005
#[test]
fn size_guards_hold_for_array_element_structs() {
    use std::mem::size_of;
    assert_eq!(size_of::<PosE7>(), 8);
    assert_eq!(size_of::<Span<VertexIx>>(), 8);
    assert_eq!(size_of::<Span<SegIx>>(), 8);
    assert_eq!(size_of::<Span<AirwayLegIx>>(), 8);
    assert_eq!(size_of::<Span<RouteIx>>(), 8);
    assert_eq!(size_of::<Span<LegIx>>(), 8);
    for n in [
        size_of::<StrIx>(),
        size_of::<PointIx>(),
        size_of::<VertexIx>(),
        size_of::<SegIx>(),
        size_of::<BoundaryIx>(),
        size_of::<AirwayIx>(),
        size_of::<AirwayLegIx>(),
        size_of::<ProcIx>(),
        size_of::<RouteIx>(),
        size_of::<LegIx>(),
    ] {
        assert_eq!(n, 4);
    }
    // Every index type has the same NONE sentinel and it is never a valid row.
    assert_eq!(StrIx::NONE.get(), u32::MAX);
    assert_eq!(PointIx::NONE.get(), u32::MAX);
    assert_eq!(LegIx::NONE.get(), u32::MAX);
}

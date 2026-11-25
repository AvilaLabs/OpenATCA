//! The ERAM export (OA-REP-001 to OA-REP-007): the format round-trips, the reader refuses what the
//! writer does not write, a manifest names the file's digest, a sample in a sealed window is never
//! written or read, and a drop list removes whole flights. Every identifier is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::assoc::norm_callsign;
use oa_bench::eram_export::{
    check_manifest, drop_addresses, drop_note, file_sha256, from_legacy, manifest_line, parse,
    parse_drop_list, summary, to_legacy, write, Meta,
};
use oa_bench::legacy::{
    flight_ix, new_legacy, pad, push_th, set_addr, set_addr_hint, Legacy, NONE, NO_ALT, NO_TIME,
};
use oa_bench::thindex::{build_th_index, samples_of};

/// 2026-11-15 12:00:00Z, a development day, and 2026-11-21 12:00:00Z, inside the sealed window.
const DEV_MS: i64 = 1_794_744_000_000;
const SEALED_MS: i64 = 1_795_262_400_000;

fn meta() -> Meta {
    Meta {
        source_dir: "sfdps".into(),
        source_inputs: vec!["sfdps (sfdps) files=2 bytes=10 sha256=ab".into()],
        source_sha256: "ab".into(),
        replay_hash: "cd".into(),
        sealed_dropped: 3,
        notes: vec!["a note".into()],
        counts: vec!["documents offered: 5".into(), "drop reason: x: 2".into()],
    }
}

/// Four flights: one with an address and a hint-free callsign, one with only a hint, one with
/// only a callsign, one with nothing (but samples).
fn legacy() -> Legacy {
    let mut l = new_legacy();
    let f0 = flight_ix(&mut l, 100).unwrap();
    set_addr(&mut l, f0, 0xa1b2c3);
    l.flights.callsign[f0 as usize] = pad(b"abc123");
    let f1 = flight_ix(&mut l, 101).unwrap();
    set_addr_hint(&mut l, f1, 0x000001);
    let f2 = flight_ix(&mut l, 102).unwrap();
    l.flights.callsign[f2 as usize] = pad(b"  N12\x00");
    let f3 = flight_ix(&mut l, 103).unwrap();
    // Interleaved, as a feed delivers them; the second sample of f0 goes back in time.
    push_th(
        &mut l,
        f0,
        DEV_MS + 12_000,
        7,
        410_000_000,
        -870_000_000,
        35_000,
        Some((1500, -20)),
        false,
    );
    push_th(&mut l, f3, DEV_MS, 7, 1, 2, NO_ALT, None, true);
    push_th(
        &mut l,
        f1,
        DEV_MS + 1,
        7,
        400_000_000,
        -860_000_000,
        NO_ALT,
        None,
        true,
    );
    push_th(
        &mut l,
        f0,
        DEV_MS,
        7,
        410_100_000,
        -870_100_000,
        35_000,
        Some((-1500, 20)),
        true,
    );
    push_th(
        &mut l,
        f2,
        DEV_MS + 5,
        7,
        -10,
        20,
        5_000,
        Some((0, 0)),
        false,
    );
    push_th(&mut l, f3, DEV_MS + 6, 7, 3, 4, NO_ALT, None, false);
    l
}

// REQ: OA-REP-001
#[test]
fn an_export_round_trips_and_keeps_what_the_scorer_reads() {
    let l = legacy();
    let e = from_legacy(&l, meta()).unwrap();
    assert_eq!(e.flights.len(), 4);
    let text = write(&e).unwrap();
    assert!(text.starts_with("oa-eram-export 1\n"));
    let back = parse(text.as_bytes()).unwrap();
    assert_eq!(back, e);
    // The keyless flight is its sample count, no positions.
    assert!(oa_bench::eram_export::flight_keyless(&back.flights[3]));
    assert_eq!((back.flights[3].n_rows, back.flights[3].rows.len()), (2, 0));
    // The legacy tables made from it pair the same: keys, and per flight the samples in order.
    let r = to_legacy(&back).unwrap();
    assert_eq!(r.flights.gufi.len(), 4);
    assert_eq!(r.flights.addr, l.flights.addr);
    assert_eq!(r.flights.addr_hint, l.flights.addr_hint);
    for f in 0..4 {
        assert_eq!(
            norm_callsign(&r.flights.callsign[f]),
            norm_callsign(&l.flights.callsign[f])
        );
    }
    let (a, b) = (build_th_index(&l.th, 4), build_th_index(&r.th, 4));
    for f in 0..3u32 {
        let (sa, sb) = (samples_of(&a, f), samples_of(&b, f));
        assert_eq!(sa.len(), sb.len());
        for (&i, &j) in sa.iter().zip(sb) {
            let (i, j) = (i as usize, j as usize);
            assert_eq!(l.th.pos_ms[i], r.th.pos_ms[j]);
            assert_eq!(
                (l.th.lat_e7[i], l.th.lon_e7[i]),
                (r.th.lat_e7[j], r.th.lon_e7[j])
            );
            assert_eq!(l.th.alt_ft[i], r.th.alt_ft[j]);
            assert_eq!(
                (l.th.vx_dkt[i], l.th.vy_dkt[i]),
                (r.th.vx_dkt[j], r.th.vy_dkt[j])
            );
            assert_eq!(l.th.coast[i], r.th.coast[j]);
        }
    }
    assert_eq!(samples_of(&b, 3).len(), 2);
    assert!(samples_of(&b, 3)
        .iter()
        .all(|&i| r.th.pos_ms[i as usize] == NO_TIME));
    // What is not read is not written: no receive time, no plan, no GUFI value.
    assert!(!text.contains("gufi"));
    assert_eq!(r.th.recv_ms.iter().filter(|&&t| t != NO_TIME).count(), 0);
}

// REQ: OA-REP-001
#[test]
fn callsigns_are_written_normalised_and_odd_bytes_in_hex() {
    let mut l = new_legacy();
    for (i, cs) in [&b"abc123"[..], b"-", b"x:1", b"A\x01B", b" \x00"]
        .iter()
        .enumerate()
    {
        let f = flight_ix(&mut l, i as u128).unwrap();
        l.flights.callsign[f as usize] = pad(cs);
    }
    let e = from_legacy(&l, meta()).unwrap();
    let text = write(&e).unwrap();
    let lines: Vec<&str> = text.lines().filter(|l| l.starts_with("F ")).collect();
    assert_eq!(lines[0], "F - - ABC123 0");
    assert_eq!(lines[1], "F - - x:2d00000000000000 0");
    assert_eq!(lines[2], "F - - X:1 0");
    assert_eq!(lines[3], "F - - x:4101420000000000 0");
    assert_eq!(lines[4], "F - - - 0");
    assert_eq!(parse(text.as_bytes()).unwrap(), e);
}

// REQ: OA-REP-001, OA-REP-009
#[test]
fn legacy_export_refuses_oversized_or_malformed_source_tables_without_skipping_rows() {
    let mut too_many_flights = new_legacy();
    too_many_flights.flights.gufi.resize(100_001, 0);
    assert!(from_legacy(&too_many_flights, meta())
        .unwrap_err()
        .contains("flight count exceeds export limit"));

    let mut too_many_rows = new_legacy();
    too_many_rows.th.flight.resize(8_000_001, 0);
    assert!(from_legacy(&too_many_rows, meta())
        .unwrap_err()
        .contains("TH row count exceeds export limit"));

    let mut missing_column = legacy();
    missing_column.th.pos_ms.pop();
    assert!(from_legacy(&missing_column, meta())
        .unwrap_err()
        .contains("TH table columns have inconsistent lengths"));

    let mut bad_reference = legacy();
    if let Some(first) = bad_reference.th.flight.first_mut() {
        *first = u32::MAX;
    }
    assert!(from_legacy(&bad_reference, meta())
        .unwrap_err()
        .contains("references nonexistent flight"));
}

// REQ: OA-REP-002
#[test]
fn the_reader_refuses_a_changed_file_a_wrong_count_and_anything_it_does_not_write() {
    let text = write(&from_legacy(&legacy(), meta()).unwrap()).unwrap();
    // One digit of a sample changed: the file's own digest no longer matches.
    let bad = text.replacen("T 1794744012000 410000000", "T 1794744012000 410000001", 1);
    assert_ne!(bad, text);
    assert!(parse(bad.as_bytes()).unwrap_err().contains("digest"));
    // Truncation, no trailer, not UTF-8, not an export.
    assert!(parse(&text.as_bytes()[..text.len() - 10]).is_err());
    assert!(parse(b"").is_err());
    assert!(parse(b"\xff\xfe\n").is_err());
    assert!(parse(b"hello\nworld\n").is_err());
    // A file a hand rebuilt with a valid digest but a wrong record or count is still refused.
    let reseal = |body: &str| format!("{body}end 4 6 {}\n", file_sha256(body.as_bytes()));
    let body = &text[..text.rfind("end ").unwrap()];
    assert!(parse(reseal(body).as_bytes()).is_ok());
    for (from, to) in [
        ("flights 4\n", "flights 5\n"),
        ("rows 6\n", "rows 7\n"),
        (
            "T 1794744000001 400000000 -860000000 - - - 1\n",
            "T 1794744000001 0400000000 -860000000 - - - 1\n",
        ),
        (
            "T 1794744000001 400000000 -860000000 - - - 1\n",
            "T 1794744000001 400000000 -860000000 - - - 2\n",
        ),
        (
            "T 1794744000001 400000000 -860000000 - - - 1\n",
            "T 1794744000001 400000000 -860000000 - -\n",
        ),
        ("F a1b2c3 - ABC123 2\n", "F A1B2C3 - ABC123 2\n"),
        ("F a1b2c3 - ABC123 2\n", "F a1b2c3 - abc123 2\n"),
        ("sealed-dropped 3\n", "sealed-dropped 03\n"),
        ("F - - - 2\n", "F - - - 2\nX 1\n"),
        ("F - 000001 - 1\n", "F - 00001 - 1\n"),
    ] {
        let t = body.replacen(from, to, 1);
        // The 15 Nov rows above have other digits; only a changed text is a test.
        if t != body {
            assert!(parse(reseal(&t).as_bytes()).is_err(), "{from:?} -> {to:?}");
        }
    }
    // Rows that do not match their flight's count.
    let short = body.replacen("F a1b2c3 - ABC123 2\n", "F a1b2c3 - ABC123 3\n", 1);
    assert!(parse(reseal(&short).as_bytes()).is_err());
    // A header text with a control character is not written.
    let mut m = meta();
    m.notes.push("two\nlines".into());
    assert!(write(&from_legacy(&legacy(), m).unwrap()).is_err());
    let mut m = meta();
    m.source_dir = " padded".into();
    assert!(write(&from_legacy(&legacy(), m).unwrap()).is_err());
}

// REQ: OA-REP-003
#[test]
fn the_manifest_names_the_digest_and_a_wrong_file_is_refused() {
    let text = write(&from_legacy(&legacy(), meta()).unwrap()).unwrap();
    let line = manifest_line("eram.txt", text.as_bytes()).unwrap();
    assert_eq!(line.len(), 64 + 2 + "eram.txt\n".len());
    assert!(check_manifest(&line, "eram.txt", text.as_bytes()).is_ok());
    let other = format!("{text}x");
    assert!(check_manifest(&line, "eram.txt", other.as_bytes())
        .unwrap_err()
        .contains("does not match"));
    assert!(check_manifest(&line, "other.txt", text.as_bytes())
        .unwrap_err()
        .contains("no line"));
    assert!(
        check_manifest(&format!("{line}{line}"), "eram.txt", text.as_bytes())
            .unwrap_err()
            .contains("twice")
    );
    assert!(check_manifest("abc  eram.txt\n", "eram.txt", b"").is_err());
    assert!(check_manifest("no-separator\n", "eram.txt", b"").is_err());
    assert!(manifest_line("dir/x", b"").is_err());
    assert!(manifest_line("a b", b"").is_err());
}

// REQ: OA-REP-004
#[test]
fn a_sample_in_a_sealed_window_is_never_written_or_read() {
    let mut l = legacy();
    let f = flight_ix(&mut l, 104).unwrap();
    set_addr(&mut l, f, 0x123456);
    push_th(&mut l, f, SEALED_MS, 0, 1, 1, NO_ALT, None, false);
    let e = from_legacy(&l, meta()).unwrap();
    let err = write(&e).unwrap_err();
    assert!(err.contains("sealed"), "{err}");
    // The message names a count and says nothing of the row.
    assert!(!err.contains("1795262400000") && !err.contains("123456"));
    // A hand-made file with a valid digest and such a row is refused by the reader too.
    let mut ok = from_legacy(&legacy(), meta()).unwrap();
    let text = write(&ok).unwrap();
    let body = &text[..text.rfind("end ").unwrap()];
    let doctored = body.replacen(
        "T 1794744000000 410100000",
        &format!("T {SEALED_MS} 410100000"),
        1,
    );
    assert_ne!(doctored, body);
    let sealed_file = format!("{doctored}end 4 6 {}\n", file_sha256(doctored.as_bytes()));
    assert!(parse(sealed_file.as_bytes())
        .unwrap_err()
        .contains("sealed"));
    // The day before the window and its margin is fine; one inside the margin is not.
    ok.flights[0].rows[0].pos_ms = 1_795_131_840_000; // 2026-11-19 23:44:00Z
    assert!(write(&ok).is_ok());
    ok.flights[0].rows[0].pos_ms = 1_795_131_900_000; // 23:45:00Z, inside the 15 minute margin
    assert!(write(&ok).is_err());
}

// REQ: OA-REP-006
#[test]
fn a_drop_list_removes_whole_flights_by_address_or_hint() {
    let l = from_legacy(&legacy(), meta()).unwrap();
    assert_eq!(parse_drop_list("# none\n\n").unwrap().len(), 0);
    let list = parse_drop_list("# two\nA1B2C3\n000001\n\n  a1b2c3  \n").unwrap();
    assert_eq!(list.len(), 2);
    for bad in ["a1b2c", "a1b2c3d", "g1b2c3", "a1b2c3 000001", "-"] {
        assert!(parse_drop_list(bad).is_err(), "{bad}");
    }
    let mut e = l.clone();
    let none = drop_addresses(&mut e, &parse_drop_list("").unwrap());
    assert_eq!((none.flights, none.rows, none.listed), (0, 0, 0));
    assert_eq!(e, l);
    let st = drop_addresses(&mut e, &list);
    // The address flight (2 samples) and the hint flight (1) go; the callsign-only and the
    // keyless flight stay, in order.
    assert_eq!((st.flights, st.rows, st.listed), (2, 3, 2));
    assert_eq!(e.flights.len(), 2);
    assert_eq!(e.flights[0].addr, NONE);
    assert!(oa_bench::eram_export::flight_keyless(&e.flights[1]));
    assert!(drop_note(&st).contains("2 flights, 3 track samples"));
    // A list naming no one in the file changes nothing.
    let mut e2 = l.clone();
    assert_eq!(
        drop_addresses(&mut e2, &parse_drop_list("ffffff").unwrap()).flights,
        0
    );
    assert_eq!(e2, l);
}

// REQ: OA-REP-007
#[test]
fn the_summary_counts_flights_samples_and_distinct_addresses() {
    let e = from_legacy(&legacy(), meta()).unwrap();
    let s = summary(&e);
    assert_eq!(
        (
            s.flights,
            s.with_addr,
            s.hint_only,
            s.callsign_only,
            s.keyless
        ),
        (4, 1, 1, 1, 1)
    );
    assert_eq!(
        (s.rows_listed, s.rows_counted_only, s.rows_coast),
        (4, 2, 2)
    );
    assert_eq!(s.distinct_addresses, 2);
}

//! File-level behaviour: header and cycle rule (OA-424-001), limits (OA-GEN-003), no panics
//! (OA-GEN-004), no unsafe code (OA-GEN-001) and no dependencies (OA-GEN-002).

mod common;

use std::io::{BufRead, Read};

use common::{file_of, fixture, patch, records, with_header, Rng};
use oa_arinc424::{
    altitude_feet, cifp_cycle, cycle_is_after, find_airport, is_clean, mora_at, parse_bytes,
    parse_cycle, parse_lat, parse_lon, parse_path_terminator, parse_reader, parse_record,
    runways_at, Cycle, Error, ErrorKind, Record, MAX_AIRWAY_FIXES, MAX_BOUNDARY_POINTS,
    MAX_CONTINUATIONS_PER_PRIMARY, MAX_HEADER_RECORDS, MAX_LINE_LEN, MAX_RECORDS,
    MAX_RECORD_ERRORS, MAX_ROUTES_PER_PROCEDURE, MAX_ROUTE_LEGS, RECORD_LEN,
};

fn parse(text: &str) -> Result<oa_arinc424::Cifp, Error> {
    parse_bytes(text.as_bytes(), "test.cifp")
}

fn lines_of(name: &str) -> Vec<String> {
    fixture(name).lines().map(str::to_string).collect()
}

// ---------------------------------------------------------------- OA-424-001

// REQ: OA-424-001
#[test]
fn header_record_is_parsed() {
    let text = with_header(&fixture("airports.txt"));
    let c = parse(&text).expect("parses");
    let h = c.header.as_ref().expect("header");
    assert_eq!(h.file_name, "FAACIFP18");
    assert_eq!(h.version, "001");
    assert_eq!(h.production_flag, 'P');
    assert_eq!(h.record_length, 132);
    assert_eq!(h.record_count, 3); // replaced by the helper; the real file declares 397304
    assert_eq!(
        h.cycle,
        Cycle {
            year: 26,
            release: 11
        }
    );
    assert_eq!(h.creation_date, "07-OCT-2026");
    assert_eq!(h.creation_time, "15:55:03");
    assert_eq!(h.data_supplier, "U.S.A. DOT FAA");
    assert_eq!(h.crc, "9D60747B");
    assert_eq!(h.records.len(), 5);
    assert!(h.records[3]
        .contains("CODED INSTRUMENT FLIGHT PROCEDURES VOLUME 2611  EFFECTIVE 29 OCT 2026"));
    assert_eq!(
        cifp_cycle(&c),
        Some(Cycle {
            year: 26,
            release: 11
        })
    );
    // The real file declares 397,304 data records in cols 29-35.
    assert_eq!(&fixture("header.txt")[28..35], "0397304");
}

// REQ: OA-424-001
#[test]
fn records_keep_the_cycle_of_their_last_change_and_older_cycles_are_accepted() {
    // The fixtures carry cycles from 1703 to 2611 under a 2611 header: records keep the cycle
    // of their last change, so a current file holds many cycles.
    let c = file_of(&[
        "airports.txt",
        "runways.txt",
        "localizers.txt",
        "sid.txt",
        "airways.txt",
    ]);
    assert!(is_clean(&c), "{:?}", c.errors);
    assert!(c.cycle_counts.len() >= 5, "{:?}", c.cycle_counts);
    assert_eq!(
        c.cycle_counts.keys().next_back(),
        Some(&Cycle {
            year: 26,
            release: 11
        })
    );
    assert!(c.cycle_counts.keys().next().is_some_and(|k| k.year < 20));
}

// REQ: OA-424-001
#[test]
fn a_record_newer_than_the_header_cycle_rejects_the_file() {
    let mut lines = lines_of("airports.txt");
    lines[1] = patch(&lines[1], 129, "2701");
    let e = parse(&with_header(&(lines.join("\n") + "\n"))).expect_err("mixed cycles");
    assert_eq!(e.line, 7); // 5 header lines + second record
    assert_eq!(
        e.kind,
        ErrorKind::CycleAfterHeader {
            header: "2611".into(),
            record: "2701".into()
        }
    );
    assert!(e.to_string().contains("test.cifp:7"), "{e}");
    // The next release of the same year is also newer; the header's own cycle is fine.
    lines[1] = patch(&lines[1], 129, "2612");
    assert!(parse(&with_header(&(lines.join("\n") + "\n"))).is_err());
    lines[1] = patch(&lines[1], 129, "2611");
    assert!(parse(&with_header(&(lines.join("\n") + "\n"))).is_ok());
    // The year is two digits: against a 0001 header, 9912 is older, 0002 is newer.
    let mut text = with_header(&(lines.join("\n") + "\n"));
    text.replace_range(35..39, "0001");
    for l in lines.iter_mut() {
        *l = patch(l, 129, "9912");
    }
    let older =
        text.split('\n').take(5).collect::<Vec<_>>().join("\n") + "\n" + &(lines.join("\n") + "\n");
    assert!(parse(&older).is_ok());
    lines[2] = patch(&lines[2], 129, "0002");
    let newer =
        text.split('\n').take(5).collect::<Vec<_>>().join("\n") + "\n" + &(lines.join("\n") + "\n");
    assert!(parse(&newer).is_err());
}

// REQ: OA-424-001
#[test]
fn cycle_arithmetic_and_malformed_cycles() {
    let c = |y, r| Cycle {
        year: y,
        release: r,
    };
    assert!(cycle_is_after(c(26, 11), c(26, 10)));
    assert!(!cycle_is_after(c(26, 11), c(26, 11)));
    assert!(cycle_is_after(c(0, 1), c(99, 13)));
    assert!(!cycle_is_after(c(99, 13), c(0, 1)));
    assert_eq!(parse_cycle("2611"), Some(c(26, 11)));
    assert_eq!(parse_cycle("2614"), Some(c(26, 14))); // 14-cycle years exist in the data
    for bad in ["2600", "2615", "26", "26111", "ABCD", "2a11", "    ", ""] {
        assert_eq!(parse_cycle(bad), None, "{bad:?}");
    }
    // A record with a malformed cycle is an error for that record; the rest still parse.
    let mut lines = lines_of("airports.txt");
    lines[0] = patch(&lines[0], 129, "2615");
    lines[1] = patch(&lines[1], 129, "ABCD");
    let c = parse(&with_header(&(lines.join("\n") + "\n"))).expect("file parses");
    assert_eq!(c.airports.len(), 1);
    assert_eq!(c.errors.len(), 2);
    assert_eq!((c.errors[0].line, c.errors[1].line), (6, 7));
    assert!(c.errors[0].to_string().contains("cycle date"));
}

// REQ: OA-424-001
#[test]
fn header_problems_reject_the_file() {
    let body = fixture("airports.txt");
    // No header.
    let e = parse(&body).expect_err("no header");
    assert_eq!((e.line, e.kind), (1, ErrorKind::MissingHeader));
    let e = parse("").expect_err("empty");
    assert_eq!(e.kind, ErrorKind::MissingHeader);
    // The count in HDR01 must equal the number of data records (truncation check).
    let mut text = with_header(&body);
    text.replace_range(28..35, "0000009");
    let e = parse(&text).expect_err("count");
    assert_eq!(
        e.kind,
        ErrorKind::RecordCountMismatch {
            declared: 9,
            found: 3
        }
    );
    // Bad cycle, bad record length, non-numeric count.
    for (range, value) in [
        (35..39, "26AA"),
        (24..28, "0100"),
        (28..35, "00000AB"),
        (35..39, "2615"),
    ] {
        let mut text = with_header(&body);
        text.replace_range(range, value);
        let e = parse(&text).expect_err("bad header");
        assert!(matches!(e.kind, ErrorKind::BadHeader(_)), "{value}: {e}");
        assert_eq!(e.line, 1);
    }
    // A header record after data is reported, and counted as a record.
    let mut text = with_header("");
    let hdr = text.lines().next().expect("hdr").to_string();
    text.push_str(&body);
    text.push_str(&hdr);
    text.push('\n');
    text.replace_range(28..35, "0000004");
    let c = parse(&text).expect("parses");
    assert_eq!(c.errors.len(), 1);
    assert_eq!(c.errors[0].kind, ErrorKind::HeaderAfterData);
    assert_eq!(c.airports.len(), 3);
}

// ---------------------------------------------------------------- whole fixtures as one file

// REQ: OA-424-001, OA-424-002, OA-424-003, OA-424-004, OA-424-005, OA-424-006
#[test]
fn every_fixture_parses_as_one_file_with_counts_and_lookups() {
    let names = [
        "airports.txt",
        "runways.txt",
        "localizers.txt",
        "vhf.txt",
        "ndb.txt",
        "enroute_waypoints.txt",
        "terminal_waypoints.txt",
        "airways.txt",
        "sid.txt",
        "star.txt",
        "approach.txt",
        "uc.txt",
        "ur.txt",
        "mora.txt",
    ];
    let c = file_of(&names);
    assert!(is_clean(&c), "{:?}", c.errors);
    assert_eq!(c.airports.len(), 3);
    assert_eq!(c.runways.len(), 5);
    assert_eq!(c.localizers.len(), 4);
    assert_eq!(c.vhf_navaids.len(), 4);
    assert_eq!(c.ndbs.len(), 4);
    assert_eq!(c.enroute_waypoints.len(), 3);
    assert_eq!(c.terminal_waypoints.len(), 3);
    assert_eq!(c.airways.len(), 3);
    assert_eq!(c.procedures.len(), 4); // BITEZ1, BENKY6, I04R, R28C... (two SID routes share BITEZ1)
    assert_eq!(c.controlled_airspace.len(), 2);
    assert_eq!(c.special_use_airspace.len(), 3);
    assert_eq!(c.mora.len(), 4);
    assert_eq!(c.counts.get("PD"), Some(&6));
    assert_eq!(c.counts.get("PF"), Some(&20));
    assert_eq!(c.counts.get("D"), Some(&4));
    assert_eq!(c.counts.get("UR"), Some(&10));
    assert_eq!(
        c.counts.values().sum::<usize>(),
        c.header.as_ref().map(|h| h.record_count).unwrap_or(0)
    );
    // Lookups.
    assert_eq!(
        find_airport(&c, "KORD").map(|a| a.iata.clone()),
        Some(Some("ORD".into()))
    );
    assert!(find_airport(&c, "KXXX").is_none());
    assert_eq!(runways_at(&c, "KORD").count(), 3);
    assert_eq!(runways_at(&c, "KMDW").count(), 2);
    assert_eq!(mora_at(&c, 41.0, -88.0), Some(3500));
    assert_eq!(mora_at(&c, 36.578, -118.292), Some(16_800));
    assert_eq!(mora_at(&c, 10.0, 10.0), None);
}

// REQ: OA-424-004
#[test]
fn orphan_continuations_and_broken_sequences_are_reported_not_attached() {
    let ap = lines_of("approach.txt");
    let cont = ap
        .iter()
        .position(|l| &l[38..39] == "2")
        .expect("a continuation");
    // Continuation without its primary: remove the primary line.
    let mut lines = ap.clone();
    lines.remove(cont - 1);
    let c = parse(&with_header(&(lines.join("\n") + "\n"))).expect("parses");
    assert_eq!(c.errors.len(), 1);
    assert_eq!(c.errors[0].kind, ErrorKind::OrphanContinuation);
    assert_eq!(c.errors[0].line, 5 + cont); // the continuation's line (primary removed)
                                            // Continuation whose key differs from the preceding primary.
    let mut lines = ap.clone();
    lines[cont] = patch(&lines[cont], 30, "ZZZZZ");
    let c = parse(&with_header(&(lines.join("\n") + "\n"))).expect("parses");
    assert_eq!(c.errors.len(), 1);
    assert_eq!(c.errors[0].kind, ErrorKind::OrphanContinuation);
    // A repeated sequence number in a route is an error and the leg is not added.
    let mut lines = ap.clone();
    lines.insert(1, ap[0].clone());
    let c = parse(&with_header(&(lines.join("\n") + "\n"))).expect("parses");
    assert_eq!(c.errors.len(), 1);
    assert_eq!(
        c.errors[0].kind,
        ErrorKind::SequenceNotIncreasing {
            previous: 10,
            found: 10
        }
    );
    // Same for airways and boundaries.
    let mut lines = lines_of("airways.txt");
    lines.insert(1, lines[0].clone());
    let c = parse(&with_header(&(lines.join("\n") + "\n"))).expect("parses");
    assert_eq!(c.errors.len(), 1);
    assert_eq!(c.airways[0].fixes.len(), 6);
    let mut lines = lines_of("uc.txt");
    lines.insert(1, lines[0].clone());
    let c = parse(&with_header(&(lines.join("\n") + "\n"))).expect("parses");
    assert_eq!(c.errors.len(), 1);
    assert_eq!(c.controlled_airspace[0].boundary.len(), 14);
}

// ---------------------------------------------------------------- coordinates

// REQ: OA-424-002, OA-424-007, OA-GEN-004
#[test]
fn coordinates_parse_to_signed_degrees_and_reject_bad_input() {
    // 41 58 36.99 N, 87 54 29.34 W (ORD airport reference point).
    assert!(
        (parse_lat("N41583699").expect("lat") - (41.0 + 58.0 / 60.0 + 36.99 / 3600.0)).abs()
            < 1e-12
    );
    assert!(
        (parse_lon("W087542934").expect("lon") + (87.0 + 54.0 / 60.0 + 29.34 / 3600.0)).abs()
            < 1e-12
    );
    assert_eq!(parse_lat("S00000000"), Some(-0.0));
    assert_eq!(parse_lat("N90000000"), Some(90.0));
    assert_eq!(parse_lat("S90000000"), Some(-90.0));
    assert_eq!(parse_lon("E180000000"), Some(180.0));
    assert_eq!(parse_lon("W180000000"), Some(-180.0));
    assert_eq!(parse_lon("E000000001"), Some(1.0 / 360_000.0));
    for bad in [
        "N90000001",
        "N91000000",
        "N41600000",
        "N41596000",
        "N4158369",
        "N415836990",
        "X41583699",
        "n41583699",
        "N4158369é",
        "N41 83699",
        "N-1583699",
        "",
        "N",
        "N41583699 ",
    ] {
        assert_eq!(parse_lat(bad), None, "{bad:?}");
    }
    for bad in [
        "E180000001",
        "W181000000",
        "W087600000",
        "W087596000",
        "W08754293",
        "W0875429340",
        "N087542934",
        "W08é542934",
        "",
        "W",
    ] {
        assert_eq!(parse_lon(bad), None, "{bad:?}");
    }
}

// REQ: OA-424-002, OA-424-003, OA-424-004
#[test]
fn altitude_and_path_terminator_forms() {
    let airways = lines_of("airways.txt");
    let alt = |text: &str| -> Result<Option<oa_arinc424::Altitude>, ErrorKind> {
        match parse_record(1, &patch(&airways[0], 84, text))? {
            Record::AirwayFix(f) => Ok(f.min_altitude),
            o => panic!("{o:?}"),
        }
    };
    use oa_arinc424::Altitude as A;
    assert_eq!(alt("02000"), Ok(Some(A::Feet(2000))));
    assert_eq!(alt("FL450"), Ok(Some(A::FlightLevel(450))));
    assert_eq!(alt("-0128"), Ok(Some(A::Feet(-128))));
    assert_eq!(alt("GND  "), Ok(Some(A::Ground)));
    assert_eq!(alt("UNLTD"), Ok(Some(A::Unlimited)));
    assert_eq!(alt("UNKNN"), Ok(Some(A::Unknown)));
    assert_eq!(alt("     "), Ok(None));
    assert_eq!(altitude_feet(A::FlightLevel(450)), Some(45_000));
    assert_eq!(altitude_feet(A::Ground), None);
    for bad in ["12A45", "FLABC", "FL   ", "1 000", "--128", "999999"] {
        assert!(matches!(alt(bad), Err(ErrorKind::Field { .. })), "{bad:?}");
    }
    for t in [
        "AF", "CA", "CD", "CF", "CI", "CR", "DF", "FA", "FC", "FD", "FM", "HA", "HF", "HM", "IF",
        "PI", "RF", "TF", "VA", "VD", "VI", "VM", "VR",
    ] {
        assert!(parse_path_terminator(t).is_some(), "{t}");
    }
    for t in ["", "XX", "tf", "T", "TFF", "  "] {
        assert!(parse_path_terminator(t).is_none(), "{t:?}");
    }
    // An unknown path terminator on a leg is a record error that names the columns.
    let star = lines_of("star.txt");
    let e = parse_record(1, &patch(&star[0], 48, "ZZ")).expect_err("terminator");
    assert!(
        e.to_string().contains("path terminator (columns 48-49)"),
        "{e}"
    );
}

// ---------------------------------------------------------------- OA-GEN-003

fn err_of(text: &str) -> Error {
    parse(text).expect_err("limit error")
}

/// A reader that serves `chunk` forever and counts the bytes consumed.
struct Endless {
    chunk: Vec<u8>,
    pos: usize,
    consumed: usize,
}

impl Read for Endless {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(self.chunk.len() - self.pos);
        buf[..n].copy_from_slice(&self.chunk[self.pos..self.pos + n]);
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for Endless {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        Ok(&self.chunk[self.pos..])
    }
    fn consume(&mut self, n: usize) {
        self.consumed += n;
        self.pos = (self.pos + n) % self.chunk.len();
    }
}

// REQ: OA-GEN-003
#[test]
fn an_overlong_line_is_rejected_with_line_and_limit() {
    let mut text = with_header(&fixture("airports.txt"));
    text.push_str(&"A".repeat(MAX_LINE_LEN + 10));
    text.push('\n');
    let e = err_of(&text);
    assert_eq!(e.line, 9);
    assert_eq!(
        e.kind,
        ErrorKind::LineTooLong {
            limit: MAX_LINE_LEN
        }
    );
    let shown = e.to_string();
    assert!(
        shown.contains("test.cifp:9") && shown.contains(&MAX_LINE_LEN.to_string()),
        "{shown}"
    );
    // A line of exactly the limit (record plus CRLF slack) is fine as far as length goes.
    let ok = "A".repeat(MAX_LINE_LEN - 1);
    let c = parse(&with_header(&format!("{ok}\n"))).expect("short enough");
    assert_eq!(c.errors.len(), 1); // the line is a record error (wrong length), not a limit
    assert_eq!(
        c.errors[0].kind,
        ErrorKind::WrongLength {
            len: MAX_LINE_LEN - 1,
            expected: RECORD_LEN
        }
    );
}

// REQ: OA-GEN-003, OA-GEN-004
#[test]
fn a_stream_without_line_breaks_is_not_buffered() {
    let mut r = Endless {
        chunk: vec![b'A'; 8192],
        pos: 0,
        consumed: 0,
    };
    let e = parse_reader(&mut r, "endless").expect_err("limit");
    assert_eq!(
        e.kind,
        ErrorKind::LineTooLong {
            limit: MAX_LINE_LEN
        }
    );
    assert_eq!(e.line, 1);
    assert_eq!(r.consumed, 0, "nothing is consumed once the bound is hit"); // the bound is checked before consuming
}

// REQ: OA-GEN-003
#[test]
fn too_many_records_is_rejected() {
    // A header followed by an endless supply of one cheap, skipped record.
    let mut line = fixture("airports.txt")
        .lines()
        .next()
        .expect("line")
        .to_string();
    line = patch(&line, 13, "N"); // section P subsection N: terminal NDB, skipped
    let mut chunk = with_header("").into_bytes();
    let header_len = chunk.len();
    for _ in 0..1000 {
        chunk.extend_from_slice(line.as_bytes());
        chunk.push(b'\n');
    }
    // Serve the header once, then the 1000-line block over and over.
    struct Source {
        data: Vec<u8>,
        header_len: usize,
        pos: usize,
    }
    impl Read for Source {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.consume(n);
            Ok(n)
        }
    }
    impl BufRead for Source {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            Ok(&self.data[self.pos..])
        }
        fn consume(&mut self, n: usize) {
            self.pos += n;
            if self.pos >= self.data.len() {
                self.pos = self.header_len;
            }
        }
    }
    let mut src = Source {
        data: chunk,
        header_len,
        pos: 0,
    };
    let e = parse_reader(&mut src, "endless-records").expect_err("record limit");
    assert_eq!(e.kind, ErrorKind::TooManyRecords { limit: MAX_RECORDS });
    assert_eq!(e.line, MAX_RECORDS + 1);
    assert!(e.to_string().contains(&MAX_RECORDS.to_string()));
}

// REQ: OA-GEN-003
#[test]
fn too_many_continuations_for_one_primary_is_rejected() {
    let ap = lines_of("approach.txt");
    let cont = ap
        .iter()
        .position(|l| &l[38..39] == "2")
        .expect("continuation");
    let mut body = vec![ap[cont - 1].clone()];
    for _ in 0..=MAX_CONTINUATIONS_PER_PRIMARY {
        body.push(ap[cont].clone());
    }
    let e = err_of(&with_header(&(body.join("\n") + "\n")));
    assert_eq!(
        e.kind,
        ErrorKind::TooManyContinuations {
            limit: MAX_CONTINUATIONS_PER_PRIMARY
        }
    );
    assert_eq!(e.line, 5 + 1 + MAX_CONTINUATIONS_PER_PRIMARY + 1);
    // Exactly the limit is accepted.
    body.pop();
    let c = parse(&with_header(&(body.join("\n") + "\n"))).expect("at the limit");
    assert_eq!(
        c.procedures[0].routes[0].legs[0].continuations.len(),
        MAX_CONTINUATIONS_PER_PRIMARY
    );
}

// REQ: OA-GEN-003
#[test]
fn group_size_limits_name_the_limit() {
    // Airway fixes.
    let v9 = lines_of("airways.txt")[0].clone();
    let body: Vec<String> = (1..=MAX_AIRWAY_FIXES + 1)
        .map(|i| patch(&v9, 26, &format!("{i:04}")))
        .collect();
    let e = err_of(&with_header(&(body.join("\n") + "\n")));
    assert_eq!(
        e.kind,
        ErrorKind::TooManyInGroup {
            what: "airway fixes",
            limit: MAX_AIRWAY_FIXES
        }
    );
    assert_eq!(e.line, 5 + MAX_AIRWAY_FIXES + 1);
    assert!(parse(&with_header(&(body[..MAX_AIRWAY_FIXES].join("\n") + "\n"))).is_ok());
    // Legs in one route.
    let leg = lines_of("sid.txt")[0].clone();
    let body: Vec<String> = (1..=MAX_ROUTE_LEGS + 1)
        .map(|i| patch(&leg, 27, &format!("{i:03}")))
        .collect();
    let e = err_of(&with_header(&(body.join("\n") + "\n")));
    assert_eq!(
        e.kind,
        ErrorKind::TooManyInGroup {
            what: "legs in one route",
            limit: MAX_ROUTE_LEGS
        }
    );
    assert!(parse(&with_header(&(body[..MAX_ROUTE_LEGS].join("\n") + "\n"))).is_ok());
    // Routes in one procedure.
    let body: Vec<String> = (0..=MAX_ROUTES_PER_PROCEDURE)
        .map(|i| patch(&leg, 21, &format!("T{i:04}")))
        .collect();
    let e = err_of(&with_header(&(body.join("\n") + "\n")));
    assert_eq!(
        e.kind,
        ErrorKind::TooManyInGroup {
            what: "routes in one procedure",
            limit: MAX_ROUTES_PER_PROCEDURE
        }
    );
    // Boundary points in one airspace volume.
    let pt = lines_of("uc.txt")[1].clone();
    let body: Vec<String> = (1..=MAX_BOUNDARY_POINTS + 1)
        .map(|i| patch(&pt, 21, &format!("{i:04}")))
        .collect();
    let e = err_of(&with_header(&(body.join("\n") + "\n")));
    assert_eq!(
        e.kind,
        ErrorKind::TooManyInGroup {
            what: "boundary points",
            limit: MAX_BOUNDARY_POINTS
        }
    );
    assert!(parse(&with_header(
        &(body[..MAX_BOUNDARY_POINTS].join("\n") + "\n")
    ))
    .is_ok());
}

// REQ: OA-GEN-003
#[test]
fn header_record_and_error_list_limits() {
    let hdr = fixture("header.txt");
    let first = hdr.lines().next().expect("hdr");
    let mut text = String::from(first);
    text.push('\n');
    for i in 2..=MAX_HEADER_RECORDS + 1 {
        text.push_str(&format!("HDR{i:02}{}\n", " ".repeat(RECORD_LEN - 5)));
    }
    let e = err_of(&text);
    assert_eq!(
        e.kind,
        ErrorKind::TooManyHeaderRecords {
            limit: MAX_HEADER_RECORDS
        }
    );
    assert_eq!(e.line, MAX_HEADER_RECORDS + 1);
    // More record errors than the cap: the first MAX_RECORD_ERRORS are kept, the rest counted.
    let junk = "X".repeat(RECORD_LEN);
    let n = MAX_RECORD_ERRORS + 50;
    let body = vec![junk; n].join("\n") + "\n";
    let c = parse(&with_header(&body)).expect("parses with errors");
    assert_eq!(c.errors.len(), MAX_RECORD_ERRORS);
    assert_eq!(c.errors_dropped, 50);
    assert!(!is_clean(&c));
    assert_eq!(c.errors[0].kind, ErrorKind::BadRecordType { found: 'X' });
}

// ---------------------------------------------------------------- OA-GEN-004

fn all_fixture_lines() -> Vec<String> {
    [
        "airports.txt",
        "runways.txt",
        "localizers.txt",
        "vhf.txt",
        "ndb.txt",
        "enroute_waypoints.txt",
        "terminal_waypoints.txt",
        "airways.txt",
        "sid.txt",
        "star.txt",
        "approach.txt",
        "uc.txt",
        "ur.txt",
        "mora.txt",
    ]
    .iter()
    .flat_map(|n| lines_of(n))
    .collect()
}

/// A printable-biased random byte (mostly ASCII, sometimes anything).
fn rand_byte(rng: &mut Rng) -> u8 {
    match rng.below(10) {
        0 => (rng.next() & 0xff) as u8,
        1 => b' ',
        2 => b'0' + (rng.next() % 10) as u8,
        3 => b'A' + (rng.next() % 26) as u8,
        _ => 0x20 + (rng.next() % 0x5f) as u8,
    }
}

// REQ: OA-GEN-004
#[test]
fn mutated_real_records_never_panic() {
    let lines = all_fixture_lines();
    assert!(lines.len() > 100);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut ok, mut err) = (0usize, 0usize);
    for line in &lines {
        for _ in 0..400 {
            let mut bytes = line.clone().into_bytes();
            for _ in 0..=rng.below(4) {
                let i = rng.below(bytes.len());
                bytes[i] = rand_byte(&mut rng);
            }
            // As a single record (lossy text, so non-ASCII becomes a 3-byte character) ...
            match parse_record(1, &String::from_utf8_lossy(&bytes)) {
                Ok(_) => ok += 1,
                Err(_) => err += 1,
            }
            // ... and as a whole file with a valid header (arbitrary bytes).
            let mut file = with_header("").into_bytes();
            file.extend_from_slice(&bytes);
            file.push(b'\n');
            let _ = parse_bytes(&file, "mutated");
        }
    }
    // The mutations reach both the success and the failure paths.
    assert!(ok > 1000, "ok {ok}");
    assert!(err > 1000, "err {err}");
}

// REQ: OA-GEN-004
#[test]
fn truncated_garbage_and_odd_input_never_panics() {
    let lines = all_fixture_lines();
    for line in &lines {
        for n in 0..=RECORD_LEN {
            // Prefixes of every length: always an error except the full record.
            let r = parse_record(1, &line[..n]);
            assert_eq!(r.is_ok(), n == RECORD_LEN, "{n}");
        }
        // Multi-byte characters in every position.
        for at in [0usize, 4, 5, 12, 38, 40, 82, 100, 128, 131] {
            let mut s = line.clone();
            s.replace_range(at..at + 1, "é");
            let _ = parse_record(1, &s);
            let t = format!("{}é{}", &line[..at], line.get(at + 2..).unwrap_or(""));
            let _ = parse_record(1, &t);
        }
    }
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
    let hdr = with_header("");
    for _ in 0..3000 {
        let len = rng.below(400);
        let mut file = hdr.clone().into_bytes();
        for _ in 0..len {
            file.push(rand_byte(&mut rng));
            if rng.below(60) == 0 {
                file.push(b'\n');
            }
        }
        let _ = parse_bytes(&file, "garbage");
        let _ = parse_bytes(&file[hdr.len()..], "garbage-no-header");
    }
    // 132-character printable lines of random content, one record each.
    for _ in 0..20_000 {
        let s: String = (0..RECORD_LEN)
            .map(|_| (0x20 + (rng.next() % 0x5f) as u8) as char)
            .collect();
        let _ = parse_record(1, &s);
        let mut t = s.clone();
        t.replace_range(0..1, "S");
        t.replace_range(128..132, "2611");
        for sec in [
            "PA", "PG", "PI", "PC", "PD", "PE", "PF", "EA", "ER", "D ", "DB", "UC", "UR", "AS",
        ] {
            let mut u = t.clone();
            if matches!(sec, "PA" | "PG" | "PI" | "PC" | "PD" | "PE" | "PF") {
                u.replace_range(4..5, "P");
                u.replace_range(12..13, &sec[1..]);
            } else {
                u.replace_range(4..6, sec);
            }
            let _ = parse_record(1, &u);
        }
    }
    // Odd framing.
    for odd in [
        &b""[..],
        b"\n",
        b"\n\n\n",
        b"\r",
        b"\r\n",
        b"\0",
        b"HDR01",
        &[0xff; 300][..],
        &[b'\n'; 1000][..],
    ] {
        let _ = parse_bytes(odd, "odd");
        let mut f = hdr.clone().into_bytes();
        f.extend_from_slice(odd);
        let _ = parse_bytes(&f, "odd");
    }
}

// REQ: OA-GEN-004
#[test]
fn line_endings_and_non_ascii_bytes_are_handled() {
    let body = fixture("airports.txt");
    let lf = with_header(&body);
    let crlf = lf.replace('\n', "\r\n");
    let a = parse(&lf).expect("lf");
    let b = parse(&crlf).expect("crlf");
    assert_eq!(a.airports, b.airports);
    // No final newline is fine too.
    assert!(parse(lf.trim_end()).is_ok());
    // A non-ASCII byte in a record is a record error naming the column, not a crash.
    let mut bytes = lf.into_bytes();
    let at = bytes.len() - 100;
    bytes[at] = 0xC3;
    let c = parse_bytes(&bytes, "t").expect("parses");
    assert_eq!(c.errors.len(), 1);
    assert!(matches!(c.errors[0].kind, ErrorKind::BadByte { .. }));
    // A lone CR inside a record is a control character.
    let mut text = with_header(&body);
    let at = text.len() - 60;
    text.replace_range(at..at + 1, "\r");
    let c = parse(&text).expect("parses");
    assert_eq!(c.errors.len(), 1);
    // A short or long record is reported with its length.
    let mut lines = lines_of("airports.txt");
    lines[0].pop();
    lines[1].push(' ');
    let c = parse(&with_header(&(lines.join("\n") + "\n"))).expect("parses");
    assert_eq!(
        c.errors[0].kind,
        ErrorKind::WrongLength {
            len: 131,
            expected: 132
        }
    );
    assert_eq!(
        c.errors[1].kind,
        ErrorKind::WrongLength {
            len: 133,
            expected: 132
        }
    );
    assert_eq!(c.airports.len(), 1);
}

// ---------------------------------------------------------------- OA-GEN-001 / 002

fn src_files() -> Vec<(String, String)> {
    let dir = format!("{}/src", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_dir(&dir)
        .expect("src dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .map(|p| {
            (
                p.display().to_string(),
                std::fs::read_to_string(&p).expect("read"),
            )
        })
        .collect()
}

// REQ: OA-GEN-001
#[test]
fn the_crate_forbids_unsafe_code() {
    let files = src_files();
    let lib = files
        .iter()
        .find(|(n, _)| n.ends_with("lib.rs"))
        .expect("lib.rs");
    assert!(lib.1.starts_with("#![forbid(unsafe_code)]"));
    let keyword = ["un", "safe"].concat();
    for (name, text) in &files {
        for (i, l) in text.lines().enumerate() {
            let code = l.split("//").next().unwrap_or("");
            assert!(
                !code.contains(&keyword) || code.contains("forbid(unsafe_code)"),
                "{name}:{}",
                i + 1
            );
        }
    }
}

// REQ: OA-GEN-002
#[test]
fn the_crate_has_no_dependencies_of_any_kind() {
    let toml = std::fs::read_to_string(format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR")))
        .expect("Cargo.toml");
    let mut section = String::new();
    for l in toml.lines().map(str::trim) {
        if l.starts_with('[') {
            section = l.to_string();
            assert!(
                !section.contains("dev-dependencies")
                    && !section.contains("build-dependencies")
                    && !section.starts_with("[target"),
                "{section}"
            );
        } else if section == "[dependencies]" {
            assert!(l.is_empty() || l.starts_with('#'), "dependency: {l}");
        }
    }
    assert!(toml.contains("[dependencies]"));
}

#[allow(dead_code)]
fn _use(_: &[Record]) {
    let _ = records;
}

//! Tests for oa-csv.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_csv::{
    CsvError, CsvErrorKind, Limits, Reader, MAX_FIELDS, MAX_FIELD_BYTES, MAX_RECORDS,
    MAX_RECORD_BYTES,
};
use std::io::{self, Cursor, Read};

fn reader(text: &[u8]) -> Result<Reader<Cursor<Vec<u8>>>, CsvError> {
    Reader::new("test.csv", Cursor::new(text.to_vec()))
}

fn rows(text: &[u8]) -> Vec<Vec<String>> {
    let r = reader(text).expect("header");
    r.map(|rec| rec.expect("record").fields().map(str::to_string).collect())
        .collect()
}

fn first_error(text: &[u8]) -> CsvError {
    match reader(text) {
        Err(e) => e,
        Ok(r) => {
            for rec in r {
                if let Err(e) = rec {
                    return e;
                }
            }
            panic!("expected an error");
        }
    }
}

// REQ: OA-CSV-001
#[test]
fn plain_fields_lf_and_crlf() {
    assert_eq!(
        rows(b"A,B\n1,2\n3,4\n"),
        vec![vec!["1", "2"], vec!["3", "4"]]
    );
    assert_eq!(
        rows(b"A,B\r\n1,2\r\n3,4\r\n"),
        vec![vec!["1", "2"], vec!["3", "4"]]
    );
    // Mixed endings and no final newline.
    assert_eq!(
        rows(b"A,B\r\n1,2\n3,4"),
        vec![vec!["1", "2"], vec!["3", "4"]]
    );
}

// REQ: OA-CSV-001
#[test]
fn quoted_fields_with_commas_and_doubled_quotes() {
    let r = rows(b"A,B,C\n\"x,y\",\"say \"\"hi\"\"\",\"\"\n");
    assert_eq!(r, vec![vec!["x,y", "say \"hi\"", ""]]);
}

// REQ: OA-CSV-001
#[test]
fn embedded_newlines_in_quoted_fields() {
    let r = rows(b"A,B\n\"line1\nline2\",x\n\"a\r\nb\",y\r\n");
    assert_eq!(r, vec![vec!["line1\nline2", "x"], vec!["a\r\nb", "y"]]);
}

// REQ: OA-CSV-001
#[test]
fn empty_fields_and_trailing_comma_and_bom() {
    assert_eq!(
        rows(b"A,B,C\n,,\n1,,3\n"),
        vec![vec!["", "", ""], vec!["1", "", "3"]]
    );
    assert_eq!(rows(b"A,B\n1,\n"), vec![vec!["1", ""]]);
    assert_eq!(rows(b"\xEF\xBB\xBFA,B\n1,2\n"), vec![vec!["1", "2"]]);
    let r = reader(b"\xEF\xBB\xBFA,B\n").expect("header");
    assert_eq!(r.header().names(), ["A", "B"]);
}

// REQ: OA-CSV-001
#[test]
fn non_ascii_text_round_trips() {
    assert_eq!(
        rows("A\n\"caf\u{e9} \u{2014} \u{4e2d}\"\n".as_bytes()),
        vec![vec!["caf\u{e9} \u{2014} \u{4e2d}"]]
    );
}

// REQ: OA-CSV-001
#[test]
fn rfc_violations_are_errors() {
    assert_eq!(
        first_error(b"A\nab\"c\n").kind,
        CsvErrorKind::QuoteInUnquotedField
    );
    assert_eq!(
        first_error(b"A\n\"ab\"c\n").kind,
        CsvErrorKind::CharacterAfterClosingQuote
    );
    assert_eq!(
        first_error(b"A\n\"abc\n").kind,
        CsvErrorKind::UnterminatedQuote
    );
    assert_eq!(
        first_error(b"A\nab\rcd\n").kind,
        CsvErrorKind::BareCarriageReturn
    );
    assert_eq!(first_error(b"A\n\xFF\n").kind, CsvErrorKind::InvalidUtf8);
    // A multi-byte character split across two fields is not valid in either.
    assert_eq!(
        first_error(b"A,B\n\xC3,\xA9\n").kind,
        CsvErrorKind::InvalidUtf8
    );
}

// REQ: OA-CSV-002
#[test]
fn fields_are_read_by_header_name() {
    let mut r = reader(b"FIX_ID,ICAO\nABC,K5\n").expect("header");
    let rec = r.next_record().expect("ok").expect("record");
    assert_eq!(rec.get(r.header(), "ICAO").expect("col"), "K5");
    assert_eq!(rec.get(r.header(), "FIX_ID").expect("col"), "ABC");
    assert!(r.header().contains("ICAO"));
    assert!(r.next_record().expect("ok").is_none());
}

// REQ: OA-CSV-002
#[test]
fn missing_column_is_an_error_naming_it() {
    let mut r = reader(b"A,B\n1,2\n").expect("header");
    let rec = r.next_record().expect("ok").expect("record");
    let e = rec.get(r.header(), "NOPE").expect_err("missing column");
    assert_eq!(e.kind, CsvErrorKind::MissingColumn("NOPE".to_string()));
    assert!(e.to_string().contains("NOPE"));
    assert!(r.header().index_of("NOPE").is_err());
}

// REQ: OA-CSV-002
#[test]
fn bad_headers_are_rejected() {
    assert_eq!(first_error(b"").kind, CsvErrorKind::MissingHeader);
    assert_eq!(
        first_error(b"A,A\n1,2\n").kind,
        CsvErrorKind::DuplicateHeader("A".to_string())
    );
    assert_eq!(
        first_error(b"A,\n1,2\n").kind,
        CsvErrorKind::EmptyHeaderName { index: 1 }
    );
}

// REQ: OA-CSV-003
#[test]
fn error_line_is_the_physical_line() {
    // Line 1 header, line 2 ok, lines 3-4 one record with an embedded newline (ok),
    // line 5 has the wrong field count.
    let e = first_error(b"A,B\n1,2\n\"x\ny\",z\n1,2,3\n");
    assert_eq!(e.line, 5);
    assert_eq!(e.source, "test.csv");
    assert!(e.to_string().starts_with("test.csv:5:"));
}

// REQ: OA-CSV-003
#[test]
fn record_line_numbers_count_embedded_newlines() {
    let mut r = reader(b"A\n1\n\"x\ny\"\n3\n").expect("header");
    let lines: Vec<u64> = std::iter::from_fn(|| r.next_record().expect("ok"))
        .map(|rec| rec.line())
        .collect();
    assert_eq!(lines, vec![2, 3, 5]);
}

// REQ: OA-CSV-003
#[test]
fn syntax_error_reports_record_start_line() {
    let e = first_error(b"A\nok\n\"never closed\nmore\n");
    assert_eq!(e.line, 3);
    assert_eq!(e.kind, CsvErrorKind::UnterminatedQuote);
    let e = first_error(b"A\nok\nbad\"quote\n");
    assert_eq!(e.line, 3);
}

// REQ: OA-CSV-004
#[test]
fn field_count_mismatch_is_rejected() {
    let e = first_error(b"A,B,C\n1,2\n");
    assert_eq!(
        e.kind,
        CsvErrorKind::FieldCountMismatch {
            expected: 3,
            found: 2
        }
    );
    let e = first_error(b"A,B\n1,2,3\n");
    assert_eq!(
        e.kind,
        CsvErrorKind::FieldCountMismatch {
            expected: 2,
            found: 3
        }
    );
    // A blank line is a one-field record.
    let e = first_error(b"A,B\n1,2\n\n");
    assert_eq!(
        e.kind,
        CsvErrorKind::FieldCountMismatch {
            expected: 2,
            found: 1
        }
    );
}

// REQ: OA-CSV-004
#[test]
fn reader_stops_after_an_error() {
    let mut r = reader(b"A,B\n1\n3,4\n").expect("header");
    assert!(r.next_record().is_err());
    assert!(r.next_record().expect("stopped").is_none());
}

// REQ: OA-GEN-003
#[test]
fn field_length_limit_names_limit_and_line() {
    let mut text = b"A\nok\n".to_vec();
    text.extend(std::iter::repeat_n(b'x', MAX_FIELD_BYTES + 1));
    text.push(b'\n');
    let e = first_error(&text);
    assert_eq!(e.line, 3);
    assert_eq!(
        e.kind,
        CsvErrorKind::LimitExceeded {
            limit: "MAX_FIELD_BYTES",
            max: MAX_FIELD_BYTES
        }
    );
    assert!(e.to_string().contains("MAX_FIELD_BYTES"));
    // Exactly at the limit is accepted.
    let mut ok = b"A\n".to_vec();
    ok.extend(std::iter::repeat_n(b'x', MAX_FIELD_BYTES));
    ok.push(b'\n');
    assert_eq!(rows(&ok).len(), 1);
}

// REQ: OA-GEN-003
#[test]
fn record_length_limit() {
    let limits = Limits {
        max_record_bytes: 20,
        ..Limits::default()
    };
    let r = Reader::with_limits(
        "t",
        Cursor::new(b"A,B\n1,2\naaaaaaaaaaaa,bbbbbbbbbbbbbbbb\n".to_vec()),
        limits,
    )
    .expect("header");
    let e = r.filter_map(Result::err).next().expect("error");
    assert_eq!(e.line, 3);
    assert_eq!(
        e.kind,
        CsvErrorKind::LimitExceeded {
            limit: "MAX_RECORD_BYTES",
            max: 20
        }
    );
    assert_eq!(MAX_RECORD_BYTES, 1 << 20);
}

// REQ: OA-GEN-003
#[test]
fn field_count_limit() {
    let mut header = String::new();
    for i in 0..=MAX_FIELDS {
        header.push_str(&format!("C{i},"));
    }
    header.push_str("END\n");
    let e = first_error(header.as_bytes());
    assert_eq!(e.line, 1);
    assert_eq!(
        e.kind,
        CsvErrorKind::LimitExceeded {
            limit: "MAX_FIELDS",
            max: MAX_FIELDS
        }
    );
}

// REQ: OA-GEN-003
#[test]
fn record_count_limit() {
    let limits = Limits {
        max_records: 2,
        ..Limits::default()
    };
    let r =
        Reader::with_limits("t", Cursor::new(b"A\n1\n2\n3\n".to_vec()), limits).expect("header");
    let results: Vec<_> = r.collect();
    assert_eq!(results.len(), 3);
    let e = results[2].as_ref().expect_err("third record");
    assert_eq!(e.line, 4);
    assert_eq!(
        e.kind,
        CsvErrorKind::LimitExceeded {
            limit: "MAX_RECORDS",
            max: 2
        }
    );
    assert_eq!(MAX_RECORDS, 5_000_000);
}

// REQ: OA-GEN-003
#[test]
fn newline_free_input_is_bounded() {
    // A stream with no line ending must fail at the record limit instead of buffering it all.
    struct Endless(u64);
    impl Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0 += buf.len() as u64;
            buf.fill(b'a');
            Ok(buf.len())
        }
    }
    let r = Reader::new("endless", io::BufReader::new(Endless(0)));
    let e = r.err().expect("error");
    assert!(matches!(e.kind, CsvErrorKind::LimitExceeded { .. }));
}

// REQ: OA-GEN-003, OA-GEN-004
#[test]
fn io_errors_are_reported_not_panicked() {
    struct Failing;
    impl Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("boom"))
        }
    }
    let e = Reader::new("bad", io::BufReader::new(Failing))
        .err()
        .expect("error");
    assert!(matches!(e.kind, CsvErrorKind::Io(_)));
    assert!(e.to_string().contains("boom"));
    // Interrupted reads are retried.
    struct Flaky(u8);
    impl Read for Flaky {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0 += 1;
            match self.0 {
                1 => Err(io::Error::from(io::ErrorKind::Interrupted)),
                2 => {
                    let data = b"A\n1\n";
                    buf[..data.len()].copy_from_slice(data);
                    Ok(data.len())
                }
                _ => Ok(0),
            }
        }
    }
    let r = Reader::new("flaky", io::BufReader::new(Flaky(0))).expect("header");
    let recs: Vec<_> = r.collect();
    assert_eq!(recs.len(), 1);
    assert!(recs[0].is_ok());
}

struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

// REQ: OA-GEN-004
#[test]
fn fuzz_random_bytes_never_panic() {
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    // Alphabet weighted toward the characters that drive the state machine.
    let special = [
        b'"', b',', b'\r', b'\n', b'a', b'1', b' ', 0xC3, 0xA9, 0xFF, 0x00, 0xEF, 0xBB, 0xBF,
    ];
    for _ in 0..20_000 {
        let len = (rng.next() % 200) as usize;
        let mut bytes = Vec::with_capacity(len);
        for _ in 0..len {
            let r = rng.next();
            if r & 3 == 0 {
                bytes.push((r >> 8) as u8);
            } else {
                bytes.push(special[(r >> 8) as usize % special.len()]);
            }
        }
        let limits = Limits {
            max_record_bytes: 64,
            max_field_bytes: 16,
            max_fields: 8,
            max_records: 5,
        };
        for l in [Limits::default(), limits] {
            if let Ok(mut r) = Reader::with_limits("fuzz", Cursor::new(bytes.clone()), l) {
                let mut n = 0;
                while let Ok(Some(rec)) = r.next_record() {
                    for name in r.header().names() {
                        let _ = rec.get(r.header(), name);
                    }
                    let _ = rec.get(r.header(), "missing");
                    n += 1;
                    assert!(n <= 1000);
                }
            }
        }
    }
}

// REQ: OA-GEN-004
#[test]
fn fuzz_structured_csv_never_panics() {
    // Well-formed-ish documents with random mutations.
    let base = b"A,B,C\n1,\"x,y\",3\r\n\"q\"\"r\",,\n\"multi\nline\",b,c\n".to_vec();
    let mut rng = XorShift(0xDEAD_BEEF_CAFE_F00D);
    for _ in 0..20_000 {
        let mut b = base.clone();
        for _ in 0..(1 + rng.next() % 4) {
            let i = (rng.next() as usize) % b.len();
            match rng.next() % 3 {
                0 => b[i] = (rng.next() >> 16) as u8,
                1 => {
                    b.remove(i);
                }
                _ => b.insert(i, b"\",\n\r"[(rng.next() % 4) as usize]),
            }
        }
        if let Ok(r) = Reader::new("fuzz", Cursor::new(b)) {
            for rec in r {
                let _ = rec;
            }
        }
    }
}

// REQ: OA-GEN-001
#[test]
fn crate_forbids_unsafe_code() {
    let src = include_str!("../src/lib.rs");
    assert!(src.contains("#![forbid(unsafe_code)]"));
    assert!(!src.contains("unsafe {") && !src.contains("unsafe fn"));
}

// REQ: OA-GEN-002
#[test]
fn crate_has_no_dependencies() {
    let toml = include_str!("../Cargo.toml");
    let after = toml
        .split("[dependencies]")
        .nth(1)
        .expect("dependencies table");
    let section: Vec<&str> = after
        .lines()
        .skip(1)
        .take_while(|l| !l.trim_start().starts_with('['))
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert!(section.is_empty(), "unexpected dependencies: {section:?}");
    assert!(!toml.contains("[dev-dependencies]") && !toml.contains("[build-dependencies]"));
}

/// Reads `text` through a `BufReader` of `cap` bytes: the header, the records and the first error.
fn through_bufreader(
    text: &[u8],
    cap: Option<usize>,
) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let src = match cap {
        Some(c) => io::BufReader::with_capacity(c, text),
        None => io::BufReader::new(text),
    };
    let r = Reader::new("test.csv", src).map_err(|e| e.to_string())?;
    let header: Vec<String> = r.header().names().to_vec();
    let mut recs = Vec::new();
    for rec in r {
        let rec = rec.map_err(|e| e.to_string())?;
        recs.push(rec.fields().map(str::to_string).collect());
    }
    Ok((header, recs))
}

// REQ: OA-CSV-005
#[test]
fn a_byte_order_mark_is_skipped_however_the_reader_splits_it() {
    let text = b"\xEF\xBB\xBFh\nv\n";
    let want = (vec!["h".to_string()], vec![vec!["v".to_string()]]);
    for cap in [None, Some(1), Some(2), Some(3), Some(4)] {
        assert_eq!(
            through_bufreader(text, cap),
            Ok(want.clone()),
            "cap {cap:?}"
        );
    }
    // A mark and nothing else, and a mark cut short by the end of the input.
    for cap in [None, Some(1), Some(2)] {
        assert!(through_bufreader(b"\xEF\xBB\xBF", cap).is_err());
        assert!(through_bufreader(b"\xEF\xBB", cap).is_err());
    }
}

// REQ: OA-CSV-005
#[test]
fn bytes_that_start_like_a_byte_order_mark_but_are_not_one_are_delivered_intact() {
    // EF BB BE is U+FEFE, a valid character that is not the mark: it stays in the header.
    let text = b"\xEF\xBB\xBEh\nv\n";
    let want = (vec!["\u{FEFE}h".to_string()], vec![vec!["v".to_string()]]);
    for cap in [None, Some(1), Some(2), Some(3), Some(4)] {
        assert_eq!(
            through_bufreader(text, cap),
            Ok(want.clone()),
            "cap {cap:?}"
        );
    }
    // EF BB then an ASCII byte, and EF then an ASCII byte, are invalid UTF-8 however they split.
    for text in [&b"\xEF\xBBXh\nv\n"[..], &b"\xEFXh\nv\n"[..]] {
        let first = through_bufreader(text, None);
        assert!(first.is_err());
        for cap in [Some(1), Some(2), Some(3)] {
            assert_eq!(through_bufreader(text, cap), first, "cap {cap:?}");
        }
    }
    // A mark-like prefix inside a quoted header is plain data too.
    let text = b"\xEF\xBB\xBE\"a,b\"\nv\n";
    let first = through_bufreader(text, None);
    for cap in [Some(1), Some(2)] {
        assert_eq!(through_bufreader(text, cap), first);
    }
}

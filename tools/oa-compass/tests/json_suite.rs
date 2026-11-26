//! JSONTestSuite (https://github.com/nst/JSONTestSuite, MIT) `test_parsing` cases. This is kept
//! in the `oa-compass` integration suite so its corpus reads use the `oa-sealed` guard; the
//! `oa-json` package retains its synthetic parser tests and stays free of a private dev-dependency.
//! Gated on
//! OPENATCA_DATA: the directory `raw/jsontestsuite/test_parsing` must exist there (see
//! DATA.md); the test prints a note and skips otherwise.
//!
//! `y_` cases must be accepted and `n_` cases refused. For the `i_` cases the standard lets a
//! parser choose; the table below is our choice, enforced case by case:
//!
//! * Numbers that are well formed but out of range (`i_number_*`): the text is accepted, as
//!   numbers are kept as text. `as_f64` gives a value when the result is finite (underflow gives
//!   0.0, and a huge integer is a finite `f64`) and `None` when it overflows to infinity.
//! * Lone or inverted surrogate escapes, and an unpaired high surrogate: refused (they cannot
//!   be written as UTF-8).
//! * Invalid UTF-8 anywhere (overlong forms, encoded surrogates, bytes above U+10FFFF, Latin-1,
//!   truncated sequences, UTF-16 text): refused.
//! * A UTF-8 byte-order mark: refused.
//! * Nesting beyond the depth limit (500 levels against a limit of 64): refused.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_json::{parse, NodeIx, DEFAULT_LIMITS};
use std::path::PathBuf;

#[derive(Debug, PartialEq)]
enum Choice {
    /// Accepted; the first array element converts with `as_f64` to this (None: not finite).
    Num(Option<f64>),
    Reject,
}

const I_CASES: &[(&str, Choice)] = &[
    ("i_number_double_huge_neg_exp", Choice::Num(Some(0.0))),
    ("i_number_huge_exp", Choice::Num(None)),
    ("i_number_neg_int_huge_exp", Choice::Num(None)),
    ("i_number_pos_double_huge_exp", Choice::Num(None)),
    ("i_number_real_neg_overflow", Choice::Num(None)),
    ("i_number_real_pos_overflow", Choice::Num(None)),
    ("i_number_real_underflow", Choice::Num(Some(0.0))),
    (
        "i_number_too_big_neg_int",
        Choice::Num(Some(-1.2312312312312312e29)),
    ),
    ("i_number_too_big_pos_int", Choice::Num(Some(1e20))),
    (
        "i_number_very_big_negative_int",
        Choice::Num(Some(-2.374623746732769e47)),
    ),
    ("i_object_key_lone_2nd_surrogate", Choice::Reject),
    ("i_string_1st_surrogate_but_2nd_missing", Choice::Reject),
    ("i_string_1st_valid_surrogate_2nd_invalid", Choice::Reject),
    ("i_string_UTF-16LE_with_BOM", Choice::Reject),
    ("i_string_UTF-8_invalid_sequence", Choice::Reject),
    ("i_string_UTF8_surrogate_U+D800", Choice::Reject),
    (
        "i_string_incomplete_surrogate_and_escape_valid",
        Choice::Reject,
    ),
    ("i_string_incomplete_surrogate_pair", Choice::Reject),
    (
        "i_string_incomplete_surrogates_escape_valid",
        Choice::Reject,
    ),
    ("i_string_invalid_lonely_surrogate", Choice::Reject),
    ("i_string_invalid_surrogate", Choice::Reject),
    ("i_string_invalid_utf-8", Choice::Reject),
    ("i_string_inverted_surrogates_U+1D11E", Choice::Reject),
    ("i_string_iso_latin_1", Choice::Reject),
    ("i_string_lone_second_surrogate", Choice::Reject),
    ("i_string_lone_utf8_continuation_byte", Choice::Reject),
    ("i_string_not_in_unicode_range", Choice::Reject),
    ("i_string_overlong_sequence_2_bytes", Choice::Reject),
    ("i_string_overlong_sequence_6_bytes", Choice::Reject),
    ("i_string_overlong_sequence_6_bytes_null", Choice::Reject),
    ("i_string_truncated-utf-8", Choice::Reject),
    ("i_string_utf16BE_no_BOM", Choice::Reject),
    ("i_string_utf16LE_no_BOM", Choice::Reject),
    ("i_structure_500_nested_arrays", Choice::Reject),
    ("i_structure_UTF-8_BOM_empty_object", Choice::Reject),
];

fn suite_dir() -> Option<PathBuf> {
    let root = std::env::var_os("OPENATCA_DATA")?;
    let dir = PathBuf::from(root).join("raw/jsontestsuite/test_parsing");
    dir.is_dir().then_some(dir)
}

// REQ: OA-JSON-001
#[test]
fn jsontestsuite_y_accepted_n_refused() {
    let Some(dir) = suite_dir() else {
        eprintln!("skip: OPENATCA_DATA/raw/jsontestsuite/test_parsing not present");
        return;
    };
    let (mut y, mut n) = (0, 0);
    let mut bad = Vec::new();
    let walk = oa_sealed::walk_unsealed(&dir).unwrap();
    eprintln!("sealed entries skipped: {}", walk.skipped);
    for p in &walk.files {
        let name = p.file_name().unwrap().to_str().unwrap().to_string();
        let data = oa_sealed::read(p).unwrap();
        let ok = parse(&data, &DEFAULT_LIMITS).is_ok();
        if name.starts_with("y_") {
            y += 1;
            if !ok {
                bad.push(format!("{name}: should be accepted"));
            }
        } else if name.starts_with("n_") {
            n += 1;
            if ok {
                bad.push(format!("{name}: should be refused"));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
    assert_eq!((y, n), (95, 188), "suite size changed");
    eprintln!("jsontestsuite: {y} y_ accepted, {n} n_ refused");
}

// REQ: OA-JSON-008
#[test]
fn jsontestsuite_i_cases_follow_the_documented_choices() {
    let Some(dir) = suite_dir() else {
        eprintln!("skip: OPENATCA_DATA/raw/jsontestsuite/test_parsing not present");
        return;
    };
    let on_disk = oa_sealed::walk_unsealed(&dir)
        .unwrap()
        .files
        .iter()
        .filter(|p| p.file_name().unwrap().to_str().unwrap().starts_with("i_"))
        .count();
    assert_eq!(
        on_disk,
        I_CASES.len(),
        "every i_ case needs a documented choice"
    );
    for (name, want) in I_CASES {
        let data = oa_sealed::read(dir.join(format!("{name}.json"))).unwrap();
        let got = match parse(&data, &DEFAULT_LIMITS) {
            Ok(doc) => {
                let first = doc.elements(NodeIx::ROOT).next().unwrap();
                Choice::Num(doc.as_f64(first))
            }
            Err(_) => Choice::Reject,
        };
        assert_eq!(&got, want, "{name}");
    }
}

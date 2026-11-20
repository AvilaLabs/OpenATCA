//! The TFMS actual-arrival table is a diagnostic: the report text and counts of the existing
//! tables do not change with it populated.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use oa_bench::legacy::{new_legacy, pad, push_tfms, push_tfms_actual, tfms_kind, TfmsIn};
use oa_bench::report::extraction_counts;

fn row() -> TfmsIn {
    TfmsIn {
        plan_id: pad(b"PLAN1"),
        acid: pad(b"TST1"),
        dep: pad(b"KMSP"),
        arr: pad(b"KORD"),
        kind: tfms_kind::ETA,
        time_ms: 1_700_000_000_000,
        issue_ms: 1_699_999_000_000,
    }
}

// REQ: OA-BENCH-136
#[test]
fn the_report_text_is_byte_identical_with_and_without_the_actual_arrival_table() {
    let mut a = new_legacy();
    push_tfms(&mut a, &row());
    let mut b = new_legacy();
    push_tfms(&mut b, &row());
    for k in 0..3 {
        push_tfms_actual(
            &mut b,
            &TfmsIn {
                time_ms: 1_700_000_100_000 + k,
                ..row()
            },
        );
    }
    assert_eq!(b.tfms_actual.kind.len(), 3);
    assert_eq!(a.tfms.kind.len(), b.tfms.kind.len());
    assert_eq!(
        format!("{:?}", extraction_counts(&a)),
        format!("{:?}", extraction_counts(&b))
    );
    // A full table counts in its own counter and in none the report shows.
    let mut c = new_legacy();
    c.limits.tfms = 1;
    push_tfms_actual(&mut c, &row());
    push_tfms_actual(&mut c, &row());
    assert_eq!(
        (c.counts.overflow_tfms_actual, c.counts.overflow_tfms),
        (1, 0)
    );
}

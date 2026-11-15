//! Kani proof harnesses for the generic core (docs/VERIFICATION.md). Compiled only under
//! `cargo kani`; a child of `wire` so that it can reach the private walkers.
#![allow(clippy::indexing_slicing)]

use super::*;
use crate::uap::{UAP_021, UAP_048, UAP_062};

fn any_uap() -> &'static Uap {
    let which: u8 = kani::any();
    kani::assume(which < 3);
    match which {
        0 => &UAP_021,
        1 => &UAP_048,
        _ => &UAP_062,
    }
}

/// Every item of the UAP `uap`, walked over any bytes of up to `N` octets from the start.
fn walk_all_slots<const N: usize>(uap: &'static Uap) {
    let explicit = Field {
        kind: Kind::Explicit,
        spare: &[],
    };
    let buf: [u8; N] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= N);
    let input = &buf[..len];
    let mut frn = 1u8;
    while usize::from(frn) <= uap.slots.len() {
        let field = match slot_of(uap, frn) {
            Some(Slot::Item { field, .. }) => Some(field),
            Some(Slot::Re | Slot::Sp) => Some(&explicit),
            _ => None,
        };
        if let Some(field) = field {
            match walk_field(input, 0, field) {
                Ok(end) => assert!(0 < end && end <= len),
                Err(f) => assert!(f.at <= len),
            }
        }
        frn += 1;
    }
}

// REQ: OA-AST-102
#[kani::proof]
#[kani::unwind(60)]
fn walk_every_item_cat021_24() {
    walk_all_slots::<24>(&UAP_021);
}

// REQ: OA-AST-102
#[kani::proof]
#[kani::unwind(60)]
fn walk_every_item_cat048_24() {
    walk_all_slots::<24>(&UAP_048);
}

// REQ: OA-AST-102
#[kani::proof]
#[kani::unwind(60)]
fn walk_every_item_cat062_24() {
    walk_all_slots::<24>(&UAP_062);
}

/// The FSPEC of category `uap` over any bytes of up to `N` octets. The FSPEC ends or is refused
/// within `uap.slots.len() / 7` octets, so `N` one above that covers every input.
fn fspec_total<const N: usize>(uap: &'static Uap) {
    let buf: [u8; N] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= N);
    let input = &buf[..len];
    let start = 0;
    match parse_fspec(input, start, uap) {
        Ok((frns, at)) => {
            assert!(start < at && at <= len);
            assert!(!frns.is_empty());
        }
        Err((_, at)) => assert!(at <= len),
    }
}

// REQ: OA-AST-101
#[kani::proof]
#[kani::unwind(10)]
fn fspec_cat021_3() {
    fspec_total::<3>(&UAP_021);
}

// REQ: OA-AST-101
#[kani::proof]
#[kani::unwind(10)]
fn fspec_cat048_5() {
    fspec_total::<5>(&UAP_048);
}

// REQ: OA-AST-101
#[kani::proof]
#[kani::unwind(10)]
fn fspec_cat062_3() {
    fspec_total::<3>(&UAP_062);
}

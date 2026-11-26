//! Which emitter category an aircraft is judged by. Core (pure).
//!
//! The category raises the hard cap of the jump rule ([`crate::jump_verdict`]): 2,000 kt for the
//! high-performance, no-information and unmanned classes and 4,088 kt for space vehicles. It
//! comes from the aircraft's own reports, one of which can be wrong or forged, so a single report
//! must not be able to raise the cap and let a large jump through. The rule:
//!
//! * A report with no category (0) changes nothing.
//! * A reserved code (`A8`, `B5`, set D, ...) is ignored: it says nothing, and never raises the
//!   cap above the ordinary one.
//! * An ordinary class (`A1` to `A5`, `A7`, `B1` to `B4`, `C1` to `C3`) is taken at once: it can
//!   only lower the cap and the ceiling.
//! * A class that raises the cap (`A0`, `A6`, `B0`, `B6`, `B7`) is taken only after
//!   [`CATEGORY_PERSIST`] consecutive reports that print a category all print it. The category is
//!   printed on a minority of rows, so the count is over the reports that print one. Until then the
//!   category already trusted (none at the start, which is judged as before: the cap of the
//!   no-information class) stays.
//!
//! Three is the smallest count that one forged report, or two, cannot reach and that a real
//! aircraft passes within its first few printed categories (readsb prints one on roughly every
//! fifth row). The caller feeds the state only reports it accepted, and a report it rejects as a
//! jump is not fed, so a forger cannot count its own rejected reports towards the three.

/// Consecutive category-bearing accepted reports that must agree before a cap-raising category is
/// trusted.
pub const CATEGORY_PERSIST: u32 = 3;

/// The category trusted for one aircraft and the candidate that may replace it. Plain record (A1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived (one per aircraft or track slot being walked)
pub struct CategoryState {
    /// The category the jump rule uses, 0 until one is trusted.
    pub trusted: u8,
    /// The cap-raising category being counted, 0 if none.
    pub cand: u8,
    /// Consecutive printed reports of `cand`.
    pub cand_n: u32,
}

/// True for the codes the standard assigns no class (`A8`, `B5`, `B8`, set D and above, and any
/// byte that is not a category).
pub fn category_is_reserved(c: u8) -> bool {
    !matches!(c, 0 | 0xa0..=0xa7 | 0xb0..=0xb4 | 0xb6 | 0xb7 | 0xc0..=0xc3)
}

/// Feeds one accepted report's category and returns the trusted category after it.
pub fn category_observe(s: &mut CategoryState, reported: u8) -> u8 {
    if reported == 0 || category_is_reserved(reported) {
        return s.trusted;
    }
    if matches!(reported, 0xa0 | 0xa6 | 0xb0 | 0xb6 | 0xb7) {
        if s.cand == reported {
            s.cand_n = s.cand_n.saturating_add(1);
        } else {
            s.cand = reported;
            s.cand_n = 1;
        }
        if s.cand_n >= CATEGORY_PERSIST {
            s.trusted = reported;
        }
    } else {
        s.trusted = reported;
        s.cand = 0;
        s.cand_n = 0;
    }
    s.trusted
}

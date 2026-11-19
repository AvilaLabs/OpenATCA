//! The feeds the replay knows. Core (pure, B1, B7): plain values. Shared by the replay clock
//! (core) and the pipeline (shell, which owns the decoders).

/// Which feed a frame came from; chosen by the caller, never guessed from the payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum FeedKind {
    /// SWIM SFDPS flight messages.
    Sfdps = 0,
    /// SWIM TBFM metering publication.
    Tbfm = 1,
    /// SWIM TFMS flight and flow data (`oa-tfms`).
    Tfms = 2,
    /// SWIM FNS notices (`oa-notam`).
    Fns = 3,
    /// ADS-B position reports (`docs/ADSB_PAYLOAD.md`). Sorts after every SWIM feed in the merge
    /// tie-break (OA-RPL-021): at an equal receive time, SWIM frames come first.
    Adsb = 4,
}

/// Number of [`FeedKind`] values.
pub const FEED_COUNT: usize = 5;

/// Display names, indexed by `FeedKind as usize`.
pub const FEED_NAMES: [&str; FEED_COUNT] = ["sfdps", "tbfm", "tfms", "fns", "adsb"];

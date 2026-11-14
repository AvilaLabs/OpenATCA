//! Error kinds the replay counts. Core (pure, B1). Nothing that fails is skipped without being
//! counted under one of these (INTERFACES IF-R5, IF-R6).

/// One kind of failure. The discriminant indexes [`Errors`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrKind {
    /// A frame did not start with `OAF1`.
    FrameBadMagic,
    /// A frame's kind byte was not 1, 2 or 3.
    FrameUnknownKind,
    /// A frame claimed a payload over 4 MiB.
    FrameOversize,
    /// A frame's CRC-32 did not match.
    FrameCrc,
    /// A file ended inside a frame.
    FrameTruncated,
    /// A gzip recording was damaged.
    GzipDamaged,
    /// A gzip recording ended inside a member.
    GzipTruncated,
    /// A frame's receive time was earlier than one already seen; replay time did not go back.
    TimeRegression,
    /// An SFDPS payload was not well-formed XML within the limits.
    SfdpsXml,
    /// An SFDPS document whose root is not a FAA NAS 3.0 `MessageCollection`.
    SfdpsUnsupportedRoot,
    /// An SFDPS document with more elements than the decoder's limit.
    SfdpsTooManyElements,
    /// An SFDPS document the event table could not hold.
    SfdpsRowCapacity,
    /// An SFDPS document whose route text the string table could not hold.
    SfdpsStringCapacity,
    /// An SFDPS decoder internal check failed.
    SfdpsInternal,
    /// An SFDPS message with no GUFI.
    SfdpsMissingGufi,
    /// An SFDPS message with a bad GUFI.
    SfdpsBadGufi,
    /// An SFDPS message with no timestamp.
    SfdpsMissingTimestamp,
    /// An SFDPS message with a bad timestamp.
    SfdpsBadTimestamp,
    /// An SFDPS message that is not a `FlightMessageType`.
    SfdpsNotFlight,
    /// An SFDPS message with two `flight` elements.
    SfdpsDuplicateFlight,
    /// A field of an accepted SFDPS row that was malformed (the row was kept without it).
    SfdpsField,
    /// A TBFM payload was not well-formed XML within the limits.
    TbfmXml,
    /// A TBFM document whose root is not a TBFM publication.
    TbfmNotTbfm,
    /// A TBFM document with a refused field (the whole message was refused).
    TbfmField,
    /// A TBFM document with more rows than the row table holds.
    TbfmCapacity,
    /// A row index the apply step could not resolve.
    BadRow,
    /// An ADS-B payload of the wrong length.
    AdsbLength,
    /// An ADS-B payload that does not start with `ADS1`.
    AdsbMagic,
    /// An ADS-B payload with an unsupported record length or a non-zero reserved field.
    AdsbLayout,
    /// An ADS-B payload with more records than a frame may carry.
    AdsbTooManyRows,
    /// An ADS-B payload with a record whose values are out of range.
    AdsbBadRecord,
    /// An ADS-B payload the decoder could not range-check (an internal inconsistency).
    AdsbInternal,
}

/// Number of [`ErrKind`] values.
pub const ERR_COUNT: usize = 32;

/// Display names, indexed by `ErrKind as usize`.
pub const ERR_NAMES: [&str; ERR_COUNT] = [
    "frame-bad-magic",
    "frame-unknown-kind",
    "frame-oversize",
    "frame-crc",
    "frame-truncated",
    "gzip-damaged",
    "gzip-truncated",
    "time-regression",
    "sfdps-xml",
    "sfdps-unsupported-root",
    "sfdps-too-many-elements",
    "sfdps-row-capacity",
    "sfdps-string-capacity",
    "sfdps-internal",
    "sfdps-missing-gufi",
    "sfdps-bad-gufi",
    "sfdps-missing-timestamp",
    "sfdps-bad-timestamp",
    "sfdps-not-flight",
    "sfdps-duplicate-flight",
    "sfdps-field",
    "tbfm-xml",
    "tbfm-not-tbfm",
    "tbfm-field",
    "tbfm-capacity",
    "bad-row",
    "adsb-length",
    "adsb-magic",
    "adsb-layout",
    "adsb-too-many-rows",
    "adsb-bad-record",
    "adsb-internal",
];

/// Counts by [`ErrKind`].
pub type Errors = [u64; ERR_COUNT];

/// Adds `n` to the count for `k`.
pub fn count_error(e: &mut Errors, k: ErrKind, n: u64) {
    if let Some(c) = e.get_mut(k as usize) {
        *c = c.saturating_add(n);
    }
}

/// The sum of all counts.
pub fn errors_total(e: &Errors) -> u64 {
    e.iter().fold(0u64, |a, &b| a.saturating_add(b))
}

/// The [`ErrKind`] of an SFDPS reject reason index (`oa_fixm::Reject as usize`).
pub fn sfdps_reject_kind(i: usize) -> ErrKind {
    match i {
        0 => ErrKind::SfdpsMissingGufi,
        1 => ErrKind::SfdpsBadGufi,
        2 => ErrKind::SfdpsMissingTimestamp,
        3 => ErrKind::SfdpsBadTimestamp,
        4 => ErrKind::SfdpsNotFlight,
        _ => ErrKind::SfdpsDuplicateFlight,
    }
}

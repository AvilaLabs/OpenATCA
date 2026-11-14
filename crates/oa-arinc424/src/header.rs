//! The file header (records `HDR01`..`HDR05`).

use crate::error::ErrorKind;
use crate::field::{parse_digits, Rec};
use crate::types::{parse_cycle, Cycle};

/// The header of a CIFP file. HDR01 carries the file attributes; the other header records
/// are free text and kept raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// HDR01 cols 6-20.
    pub file_name: String,
    /// HDR01 cols 21-23.
    pub version: String,
    /// HDR01 col 24: `P` production, `T` test.
    pub production_flag: char,
    /// HDR01 cols 25-28: declared record length (132).
    pub record_length: usize,
    /// HDR01 cols 29-35: declared number of data records.
    pub record_count: usize,
    /// HDR01 cols 36-39: the cycle of the file.
    pub cycle: Cycle,
    /// HDR01 cols 42-52 (`07-OCT-2026`).
    pub creation_date: String,
    /// HDR01 cols 53-60.
    pub creation_time: String,
    /// HDR01 cols 61-76, trimmed. The FAA readme notes col 62 is a blank, not the first letter.
    pub data_supplier: String,
    /// HDR01 cols 125-132: the file CRC as written (hex text). Not verified here: the FAA readme
    /// says it is a CRC-32 per ARINC Report 665 and does not say which bytes it covers.
    pub crc: String,
    /// All header records, raw, in order.
    pub records: Vec<String>,
}

pub(crate) fn parse_hdr01(text: &str) -> Result<Header, ErrorKind> {
    let r = Rec::new(text);
    let bad = |why: &'static str| ErrorKind::BadHeader(why);
    if r.raw(1, 5) != "HDR01" {
        return Err(ErrorKind::MissingHeader);
    }
    let record_length =
        parse_digits(r.raw(25, 28)).ok_or_else(|| bad("record length is not numeric"))?;
    let record_count =
        parse_digits(r.raw(29, 35)).ok_or_else(|| bad("record count is not numeric"))?;
    let cycle = parse_cycle(r.raw(36, 39)).ok_or_else(|| bad("cycle is not YYRR"))?;
    if record_length as usize != crate::RECORD_LEN {
        return Err(bad("declared record length is not 132"));
    }
    Ok(Header {
        file_name: r.text(6, 20),
        version: r.text(21, 23),
        production_flag: r.ch(24),
        record_length: record_length as usize,
        record_count: record_count as usize,
        cycle,
        creation_date: r.text(42, 52),
        creation_time: r.text(53, 60),
        data_supplier: r.text(61, 76),
        crc: r.text(125, 132),
        records: vec![text.to_string()],
    })
}

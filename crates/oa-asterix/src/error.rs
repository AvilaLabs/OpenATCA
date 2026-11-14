//! The one error type of the crate: decode and encode refusals name the category, the record,
//! the data item (and subitem) and the byte offset (RULESET E5).

/// `record` value of an error that belongs to the data block, not to a record.
pub const NO_RECORD: u32 = u32::MAX;
/// `item` value of an error that belongs to the block header or the FSPEC, not to an item.
pub const NO_ITEM: u16 = 0;
/// `offset` value of an encode error (there is no input byte to point at).
pub const NO_OFFSET: usize = usize::MAX;

/// Why an input or a value was refused. Plain data (A1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The input ends inside a header, an FSPEC or an item.
    Truncated,
    /// LEN is below the three octets of the block header.
    BlockLengthBelowHeader,
    /// LEN claims more octets than the input has.
    BlockLengthExceedsInput,
    /// The block holds no record.
    EmptyBlock,
    /// The block holds more than [`crate::MAX_RECORDS_PER_BLOCK`] records.
    TooManyRecords,
    /// The category is not 021, 048 or 062.
    UnsupportedCategory,
    /// A record whose FSPEC sets no bit.
    EmptyFspec,
    /// The FSPEC runs past the longest FSPEC the UAP can address.
    FspecTooLong,
    /// The FSPEC ends with an all-zero octet that is not the only one.
    FspecNotMinimal,
    /// An FSPEC bit is set for a UAP slot the category does not define (spare or not used).
    UnknownUapSlot { frn: u8 },
    /// An item every record must carry (the data source identifier) is absent.
    MissingMandatory { frn: u8 },
    /// An extended item whose FX chain does not end inside the record or within
    /// [`crate::MAX_EXTENSIONS`] octets.
    ExtensionUnterminated,
    /// A repetitive item with a repetition factor of zero.
    ZeroRepetition,
    /// An explicit-length item (SP, RE) whose length octet is below two or past the input.
    ExplicitLengthInvalid,
    /// A compound item with no subitem present.
    EmptyCompound,
    /// A compound primary subfield sets the bit of a subitem the category does not define.
    UnknownSubitem,
    /// A compound primary subfield ends with an all-zero octet that is not the only one.
    CompoundNotMinimal,
    /// A bit the specification fixes to zero (a spare bit) is set.
    SpareBitSet,
    /// A value the specification does not allow (an invalid character, a latitude past 90
    /// degrees).
    InvalidValue,
    /// Encode: a value does not fit the bits of its element.
    ValueOutOfRange,
    /// Encode: an item is given twice (typed and raw, or raw twice).
    DuplicateItem,
    /// Encode: the block would exceed [`crate::MAX_BLOCK_LEN`].
    BlockTooLong,
    /// Encode: a raw item names a UAP slot that does not exist or is not an item.
    UnknownFrn { frn: u8 },
}

/// A refusal with its place. Plain data (A1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct AsterixError {
    /// Category, 0 if the header was not read.
    pub cat: u8,
    /// Zero-based record index in the block, or [`NO_RECORD`].
    pub record: u32,
    /// Data item number (130 for I021/130), or [`NO_ITEM`].
    pub item: u16,
    /// Subitem number within a compound item, 1-based, 0 if none.
    pub sub: u8,
    /// Byte offset in the input given to the decoder (stream-absolute for the stream decoder),
    /// or [`NO_OFFSET`] for an encode error.
    pub offset: usize,
    pub kind: ErrorKind,
}

impl std::fmt::Display for AsterixError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CAT{:03}", self.cat)?;
        if self.record != NO_RECORD {
            write!(f, " record {}", self.record)?;
        }
        if self.item != NO_ITEM {
            write!(f, " item I{:03}/{:03}", self.cat, self.item)?;
            if self.sub != 0 {
                write!(f, " subitem {}", self.sub)?;
            }
        }
        if self.offset != NO_OFFSET {
            write!(f, " byte {}", self.offset)?;
        }
        write!(f, ": {:?}", self.kind)
    }
}

impl std::error::Error for AsterixError {}

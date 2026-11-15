#![forbid(unsafe_code)]
//! oa-asterix: encoder and decoder for EUROCONTROL ASTERIX categories 021 (ADS-B target
//! reports), 048 (monoradar target reports) and 062 (system track data), written from the
//! specifications: Part 1 Ed 3.1 (general structure), CAT021 Ed 2.7, CAT048 Ed 1.32, CAT062
//! Ed 1.21. All of it is core, pure functions of bytes and values (B1); there is no shell.
//!
//! # Layers
//!
//! * Generic core, [`wire`] and [`uap`]: data block framing, the FSPEC and the item kinds
//!   (fixed, extended, repetitive, explicit, compound, plus the SP and RE fields), driven by
//!   one constant table per category. Adding an item is adding a table row.
//! * Typed layer, [`cat021`], [`cat048`], [`cat062`]: the items the tracker needs decoded into
//!   plain records of raw LSB counts ([`units`] names each LSB and cites the section); every
//!   other item is kept as raw bytes and round-trips. [`block`] ties both layers together.
//!
//! # Refusal (E5)
//!
//! Input that breaks a rule is refused with an [`AsterixError`] naming category, record index,
//! data item, subitem and byte offset. Refused: a LEN that is below three or past the input, a
//! block without records, an FSPEC that runs past its record or the UAP, a trailing all-zero
//! FSPEC octet (and, in an item the typed layer decodes, a trailing all-zero compound primary
//! octet), an FSPEC bit for a slot the UAP leaves unused, a record without its data source
//! identifier, an item that runs past the block, an FX chain that never ends, a repetition
//! factor of zero, an explicit length below two, an empty compound item, a compound subitem the
//! category does not define, and a spare bit that the specification fixes to zero. Reserved
//! enumeration codes are not refused (see [`cat021`]). The decision for each is in [`wire`].
//!
//! # Bounds
//!
//! [`MAX_BLOCK_LEN`] (65 535, the two-octet LEN of Part 1 5.2.2), [`MAX_RECORDS_PER_BLOCK`],
//! [`MAX_EXTENSIONS`], [`MAX_REPETITIONS`], [`MAX_PRIMARY_OCTETS`].
//!
//! # Round trip
//!
//! `encode_block(decode_block(x))` equals `x` for every block the decoder accepts: the decoder
//! refuses what the encoder would write differently (non-minimal FSPEC and compound primary
//! subfields of decoded items, spare bits); items kept raw (including a compound primary with a
//! trailing zero octet, and six zero octets for an identification) go back as they came. `decode(encode(r))` equals `r` for records the encoder accepts, except
//! that the encoder writes raw items (and the untyped subitems of I062/380) in FRN order and the
//! decoder returns them in that order.

pub mod block;
pub mod cat021;
pub mod cat048;
pub mod cat062;
pub mod common;
pub mod error;
pub mod uap;
pub mod units;
pub mod wire;

pub use block::{decode_block, decode_block_at, decode_stream, encode_block, Block};
pub use common::{
    e7_to_raw, ms_to_time128, raw_to_e7, time128_to_ms, Ident, Mode3a, RawItem, RawPosition,
    SelectedAltitude, SourceId,
};
pub use error::{AsterixError, ErrorKind, NO_ITEM, NO_OFFSET, NO_RECORD};
pub use wire::{
    parse_block, MAX_BLOCK_LEN, MAX_EXTENSIONS, MAX_PRIMARY_OCTETS, MAX_RECORDS_PER_BLOCK,
    MAX_REPETITIONS,
};

#[cfg(kani)]
mod proofs;

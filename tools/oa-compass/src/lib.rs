#![forbid(unsafe_code)]
//! oa-compass: writes a recorded UTC day of OpenATCA tracker output and of ERAM tracks as raw
//! ASTERIX CAT062 Ed 1.21 data blocks, and the ADS-B reports offered to the tracker's adapter (a
//! superset of those it used) as CAT021 Ed 2.4, with the data-source file and the import notes an outside evaluation tool (OpenATS
//! COMPASS) needs. Class `split` (B7).
//!
//! * `core`: the pure conversion of one output row (an OpenATCA tracker tick row, or an ERAM
//!   track sample) into the plain values of one CAT062 record, and of one ADS-B report into the
//!   values of one CAT021 record, in the item LSBs of the specification, with the list of what
//!   the row did not have, the ERAM flight address check, and the interval statistics the README
//!   states. No I/O, no clock.
//! * `shell`: the encoder glue onto `oa-asterix` (which allocates per record, so it is shell),
//!   the files, the tracker run, the ERAM load, the read-back verification, the notes, and the
//!   `oa-compass` binary.
//!
//! Requirements: OA-CMP-001 and the following, in REQUIREMENTS.md.
//!
//! # A run
//!
//! `shell::job` is one export: the output directory must be new or empty; the data files, the
//! data-source file and the README are all written as `.partial` and renamed in one pass, and any
//! error removes everything the run created. The commit the README names comes from the
//! `oa-build-info` crate (read at build time). `--openatca-rows ticks|updates` chooses the
//! tracker's one-second tick rows (the default) or its update rows. With both `--adsb` and
//! `--sfdps-dir`, every ERAM flight with an aircraft address is compared with the ADS-B reports of
//! the same address (`core::addrcheck`): a flight whose track contradicts the address it carries
//! is listed in the README; by default its address is written as ERAM published it, and
//! `--eram-contradicted-address drop` writes it without I380 address instead.
//!
//! # CAT021 Ed 2.4 against Ed 2.7, for every item `adsb.ast` encodes
//!
//! `oa-asterix` carries both editions as one codec ([`oa_asterix::Cat021Edition`]); the editions
//! are compared here from the two Part 12 documents (EUROCONTROL-SPEC-0149-12, Released Edition
//! 2.4 and Released Edition 2.7). The UAP is the same in both: FRN 1 to 49 hold the same items
//! at the same lengths (section 5.3, Table 2 of Ed 2.4, Table 1 of Ed 2.7); the only change in
//! the table is the name of FRN 39 ("Mode S MB Data", then "BDS Register Data"), which is not
//! encoded. The item sections have the same numbers in both editions. "Same" below means the
//! structure drawing, the LSB and the range are the same; wording-only changes are not listed.
//!
//! | Item | FRN | Length | Section | Ed 2.4 | Ed 2.7 | Difference |
//! |---|---|---|---|---|---|---|
//! | I021/010 data source | 1 | 2 | 5.2.2 | SAC, SIC | SAC, SIC | none (the SAC list moved to the ASTERIX site) |
//! | I021/040 target report descriptor | 2 | 1+ | 5.2.6 | primary ATP 3, ARC 2, RC, RAB; first extension DCR, GBS, SIM, TST, SAA, CL 2; second extension spare, LLC, IPC, NOGO, CPR, LDPJ, RCF | the same, plus a third extension (TBC, total bits corrected) and a fourth (MBC) | two extensions in Ed 2.4 (its second extension's FX points to a third that the edition does not define); the export writes the primary and first extension only |
//! | I021/161 track number | 3 | 2 | 5.2.31 | 12 bits, bits 16/13 spare | same | none; not encoded (ADS-B reports carry no track number) |
//! | I021/071 time of applicability for position | 5 | 3 | 5.2.8 | 1/128 s since midnight UTC | same | none; not encoded (no avionics time in the source) |
//! | I021/073 time of message reception of position | 12 | 3 | 5.2.10 | 1/128 s since midnight UTC | same | none; encoded |
//! | I021/075 time of message reception of velocity | 14 | 3 | 5.2.12 | 1/128 s | same | none; not encoded (one time per report) |
//! | I021/130 position in WGS-84 | 6 | 6 | 5.2.18 | lat, lon 24 bits, LSB 180/2^23 degrees | same (LSB written as 2.14576721191406e-5 degrees) | none; not encoded (I131 is finer) |
//! | I021/131 position, high resolution | 7 | 8 | 5.2.19 | lat, lon 32 bits, LSB 180/2^30 degrees, lat +-90, lon -180 to <180 | same (LSB written as 1.6763806343078e-7 degrees) | none; encoded |
//! | I021/080 target address | 11 | 3 | 5.2.15 | 24 bits | same | none in the structure; Ed 2.7 adds a note that the address type is the ATP of I021/040 |
//! | I021/090 quality indicators | 17 | 1+ | 5.2.16 | primary NACv (bits 8/6) and NIC (5/2); ext 1 NICBARO, SIL (7/6), NACp (5/2); ext 2 spare 8/7, SILS (6), SDA (5/4), GVA (3/2); ext 3 PIC (8/5), bits 4/2 spare | the same, with ext 3 bit 4 = SRC (PIC source) and bits 3/2 spare; plus ext 4 (validation: VAL_STATE, VD, VQ) | SRC and the fourth extension exist only in Ed 2.7; the export writes the primary, first and second extension |
//! | I021/210 MOPS version | 18 | 1 | 5.2.35 | spare 8, VNS (7), VN (6/4) 0 to 2, LTT (3/1) | same, VN 3 added (ED-102B/DO-260C) | VN 3 does not exist in Ed 2.4: a report with version 3 gets no I021/210 |
//! | I021/145 flight level | 21 | 2 | 5.2.22 | 1/4 FL, -15 to 1500 FL | same | none |
//! | I021/140 geometric height | 16 | 2 | 5.2.21 | 6.25 ft, -1500 to 150 000 ft | same | none |
//! | I021/155 barometric vertical rate | 24 | 2 | 5.2.28 | RE (16), 6.25 ft/min in 15 bits | same | none |
//! | I021/157 geometric vertical rate | 25 | 2 | 5.2.29 | RE (16), 6.25 ft/min in 15 bits | same | none |
//! | I021/160 airborne ground vector | 26 | 4 | 5.2.30 | RE (32), speed 15 bits 2^-14 NM/s, track 16 bits 360/2^16 degrees | same | none |
//! | I021/170 target identification | 29 | 6 | 5.2.33 | eight six-bit characters | same | the reference to the coding table moved (Table 3-9 of Ref 6, then Table 3-8 of Ref 5); the characters are the same |
//! | I021/020 emitter category | 30 | 1 | 5.2.5 | codes 0 to 24 | same | Ed 2.7 adds a note that codes 4, 6, 13, 14 and 16 are for versions 0 to 2 only; they are encoded as the source gives them |
//!
//! What the codec does with the editions, in `oa-asterix`: the UAP differs only in the spare
//! bits of I021/090; the records are checked for what only Ed 2.7 defines (a third extension of
//! I021/040, a fourth of I021/090, VN 3), refused on encode and on decode when the edition is
//! Ed 2.4. The unchanged items share all of the code.
//!
//! # Time and sources in `adsb.ast`
//!
//! The time is I073, the time the receiver network stamped the report; I071 (an avionics time)
//! and I075 (a separate time for the velocity) are not in the source. The data source is
//! SAC 255, SIC 13 (a test pair, like the two trackers': 11 and 12; SIC 1 is left free because COMPASS uses 255/1 for its reference).

// The width of `usize` is fixed by construction: no 32-bit build of the core (B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("this crate supports 64-bit targets only");

pub mod core;
pub mod shell;

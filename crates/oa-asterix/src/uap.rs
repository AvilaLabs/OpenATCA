//! The User Application Profiles as constant data (Part 1 Ed 3.1 section 5.1 and 5.2.5): one
//! table per category, one [`Slot`] per Field Reference Number, each item described by its
//! [`Kind`] and the spare bits it must keep at zero. No code per item: [`crate::wire`] walks any
//! item from its description.
//!
//! Sources: CAT021 Ed 2.7 section 5.3 (Table 1) and Ed 2.4 section 5.3 (Table 2) and 5.2.x, CAT048 Ed 1.32 section 5.3.1
//! (Table 2) and 5.2.x, CAT062 Ed 1.21 section 5.3 (Table 1) and 5.2.x. Item numbers in doc
//! comments are those of the category (I021/130 is "130" in CAT021).
//!
//! Spare bits: Part 1 5.2.5.2.5 says "Spare bits in the Data Items shall be set to zero". The
//! tables list the spare bits of every item that the crate walks and the specification marks
//! "spare" or "set to 0" in a fixed or extended item, as `(octet index, mask)` with the octet
//! counted from the start of the item (an extent counts as the next octet). Spare bits of
//! subitems inside a compound item are listed only where the subitem is typed or shown in the
//! specification as a spare bit; slots of a compound primary subfield that the category does not
//! define are always checked.

/// A spare-bit rule: the bits of `mask` in octet number `octet` of the item must be zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: constant table row, size guard waived
pub struct Spare {
    pub octet: u8,
    pub mask: u8,
}

/// The structure of a data item (Part 1 5.2.5.1).
#[derive(Debug, Clone, Copy)]
// A7.2: constant table row, size guard waived
pub enum Kind {
    /// Fixed length, `n` octets.
    Fixed(u8),
    /// Extended length: extents of `chunk` octets, the FX bit being the LSB of the last octet
    /// of each extent (every item here has one-octet extents except I062/510 with three).
    Extended(u8),
    /// Repetitive: a one-octet repetition factor, then that many subitems of `size` octets.
    Repetitive { size: u8 },
    /// Explicit length: a one-octet length that counts itself (the SP and RE fields).
    Explicit,
    /// Compound: a primary subfield of one-octet parts with FX, then the subitems whose bit is
    /// set, in order. Subitem `i` (zero-based) is bit `7 - i % 7` of primary octet `i / 7`.
    Compound(&'static [Field]),
}

/// An item or subitem: its structure and its spare bits.
#[derive(Debug, Clone, Copy)]
// A7.2: constant table row, size guard waived
pub struct Field {
    pub kind: Kind,
    pub spare: &'static [Spare],
}

/// One Field Reference Number of a UAP.
#[derive(Debug, Clone, Copy)]
// A7.2: constant table row, size guard waived
pub enum Slot {
    /// Spare or "not used": a set FSPEC bit is refused.
    Unused,
    /// A data item with its number (130 for I021/130).
    Item { id: u16, field: Field },
    /// The Reserved Expansion Field (explicit length).
    Re,
    /// The Special Purpose Field (explicit length).
    Sp,
}

/// A category's UAP.
#[derive(Debug, Clone, Copy)]
// A7.2: constant table row, size guard waived
pub struct Uap {
    pub cat: u8,
    /// Slot `frn - 1` describes FRN `frn`; the length is a multiple of seven (the FX bits sit
    /// between).
    pub slots: &'static [Slot],
    /// FRNs every record must carry.
    pub mandatory: &'static [u8],
}

const NO_SPARE: &[Spare] = &[];

const fn fixed(n: u8) -> Field {
    Field {
        kind: Kind::Fixed(n),
        spare: NO_SPARE,
    }
}
const fn fixed_s(n: u8, spare: &'static [Spare]) -> Field {
    Field {
        kind: Kind::Fixed(n),
        spare,
    }
}
const fn ext(chunk: u8, spare: &'static [Spare]) -> Field {
    Field {
        kind: Kind::Extended(chunk),
        spare,
    }
}
const fn rep(size: u8) -> Field {
    Field {
        kind: Kind::Repetitive { size },
        spare: NO_SPARE,
    }
}
const fn compound(subs: &'static [Field]) -> Field {
    Field {
        kind: Kind::Compound(subs),
        spare: NO_SPARE,
    }
}
const fn item(id: u16, field: Field) -> Slot {
    Slot::Item { id, field }
}
const fn sp(octet: u8, mask: u8) -> Spare {
    Spare { octet, mask }
}

/// I0xx/010, the data source identifier, is FRN 1 of every UAP here and "shall be present in
/// every ASTERIX record" (021 5.2.2, 048 5.2.1, 062 5.2.1).
const MANDATORY: &[u8] = &[1];

/// The editions of CAT021 this crate carries (Part 12). The codec is one: the edition selects
/// the UAP table ([`uap_021`]) and the few checks that differ ([`crate::cat021::check_edition`]).
/// Ed 2.7 is the default of every function without an edition argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold enum, size guard waived
pub enum Cat021Edition {
    /// Ed 2.4 (Released Edition): I021/040 has two extensions, I021/090 three, MOPS version 0 to 2.
    Ed24,
    /// Ed 2.7 (Released Edition).
    Ed27,
}

/// I021/090 spare bits, Ed 2.7 (5.2.16): extension 2 bits 8/7, extension 3 bits 3/2 (bit 4 is
/// SRC), extension 4 bits 8/7.
const QUALITY_SPARE_ED27: &[Spare] = &[sp(2, 0xC0), sp(3, 0x06), sp(4, 0xC0)];
/// I021/090 spare bits, Ed 2.4 (5.2.16): extension 2 bits 8/7, extension 3 bits 4/2; there is
/// no fourth extension.
const QUALITY_SPARE_ED24: &[Spare] = &[sp(2, 0xC0), sp(3, 0x0E)];

/// The 49 slots of the CAT021 UAP, one macro for both editions (Table 1 of Ed 2.7, Table 2 of Ed 2.4). The two tables list
/// the same FRNs, items and lengths (only the name of FRN 39 differs: "Mode S MB Data" in Ed 2.4,
/// "BDS Register Data" in Ed 2.7); the spare bits of I021/090 differ.
macro_rules! slots_021 {
    ($quality_spare:expr) => {
        [
        item(10, fixed(2)),
        // 5.2.6: the second extension has spare bit 8; the others are fully assigned.
        item(40, ext(1, &[sp(2, 0x80)])),
        item(161, fixed_s(2, &[sp(0, 0xF0)])),
        item(15, fixed(1)),
        item(71, fixed(3)),
        item(130, fixed(6)),
        item(131, fixed(8)),
        item(72, fixed(3)),
        item(150, fixed(2)),
        item(151, fixed(2)),
        item(80, fixed(3)),
        item(73, fixed(3)),
        item(74, fixed(4)),
        item(75, fixed(3)),
        item(76, fixed(4)),
        item(140, fixed(2)),
        // 5.2.16: spare bits per edition, see QUALITY_SPARE_ED27 and QUALITY_SPARE_ED24.
        item(90, ext(1, $quality_spare)),
        item(210, fixed_s(1, &[sp(0, 0x80)])),
        item(70, fixed_s(2, &[sp(0, 0xF0)])),
        item(230, fixed(2)),
        item(145, fixed(2)),
        item(152, fixed(2)),
        item(200, fixed(1)),
        item(155, fixed(2)),
        item(157, fixed(2)),
        item(160, fixed(4)),
        item(165, fixed_s(2, &[sp(0, 0xFC)])),
        item(77, fixed(3)),
        item(170, fixed(6)),
        item(20, fixed(1)),
        // 5.2.36 Met Information: wind speed, wind direction, temperature (two octets each),
        // turbulence (one octet).
        item(220, compound(&[fixed(2), fixed(2), fixed(2), fixed(1)])),
        item(146, fixed(2)),
        item(148, fixed(2)),
        // 5.2.17 Trajectory Intent: status (extended), data (repetitive, 15-octet points).
        item(110, compound(&[ext(1, &[sp(0, 0x3E)]), rep(15)])),
        item(16, fixed(1)),
        item(8, fixed(1)),
        item(271, ext(1, &[sp(0, 0xC0), sp(1, 0x0E)])),
        item(132, fixed(1)),
        item(250, rep(8)),
        item(260, fixed(7)),
        item(400, fixed(1)),
        // 5.2.41 Data Ages: 23 one-octet ages.
        item(295, compound(&[fixed(1); 23])),
        Slot::Unused,
        Slot::Unused,
        Slot::Unused,
        Slot::Unused,
        Slot::Unused,
        Slot::Re,
        Slot::Sp,
        ]
    };
}

static SLOTS_021_ED27: [Slot; 49] = slots_021!(QUALITY_SPARE_ED27);
static SLOTS_021_ED24: [Slot; 49] = slots_021!(QUALITY_SPARE_ED24);

/// CAT021 Ed 2.7, Table 1 (49 slots).
pub static UAP_021: Uap = Uap {
    cat: 21,
    mandatory: MANDATORY,
    slots: &SLOTS_021_ED27,
};

/// CAT021 Ed 2.4, Table 2 (49 slots).
pub static UAP_021_ED24: Uap = Uap {
    cat: 21,
    mandatory: MANDATORY,
    slots: &SLOTS_021_ED24,
};

/// The CAT021 UAP of edition `ed`.
pub fn uap_021(ed: Cat021Edition) -> &'static Uap {
    match ed {
        Cat021Edition::Ed24 => &UAP_021_ED24,
        Cat021Edition::Ed27 => &UAP_021,
    }
}

/// CAT048 Ed 1.32, Table 2 (28 slots).
pub static UAP_048: Uap = Uap {
    cat: 48,
    mandatory: MANDATORY,
    slots: &[
        item(10, fixed(2)),
        item(140, fixed(3)),
        // 5.2.2: extension 2 spare bit 2, extension 4 spare bit 2, extension 5 spare bits 4/2.
        item(20, ext(1, &[sp(2, 0x02), sp(4, 0x02), sp(5, 0x0E)])),
        item(40, fixed(4)),
        item(70, fixed_s(2, &[sp(0, 0x10)])),
        item(90, fixed(2)),
        // 5.2.16: seven one-octet subitems.
        item(130, compound(&[fixed(1); 7])),
        item(220, fixed(3)),
        item(240, fixed(6)),
        item(250, rep(8)),
        item(161, fixed_s(2, &[sp(0, 0xF0)])),
        item(42, fixed(4)),
        item(200, fixed(4)),
        // 5.2.19: the first extent has spare bits 4/2.
        item(170, ext(1, &[sp(1, 0x0E)])),
        item(210, fixed(4)),
        item(30, ext(1, NO_SPARE)),
        item(80, fixed_s(2, &[sp(0, 0xF0)])),
        item(100, fixed_s(4, &[sp(0, 0x30), sp(2, 0xF0)])),
        item(110, fixed_s(2, &[sp(0, 0xC0)])),
        // 5.2.15: calculated Doppler speed (two octets, spare bits 15/11), raw Doppler speed
        // (repetitive, six-octet entries).
        item(120, compound(&[fixed_s(2, &[sp(0, 0x7C)]), rep(6)])),
        item(230, fixed_s(2, &[sp(0, 0x01)])),
        item(260, fixed(7)),
        item(55, fixed(1)),
        item(50, fixed_s(2, &[sp(0, 0x10)])),
        item(65, fixed_s(1, &[sp(0, 0xE0)])),
        item(60, fixed_s(2, &[sp(0, 0xF0)])),
        Slot::Sp,
        Slot::Re,
    ],
};

/// CAT062 Ed 1.21, Table 1 (35 slots).
pub static UAP_062: Uap = Uap {
    cat: 62,
    mandatory: MANDATORY,
    slots: &[
        item(10, fixed(2)),
        Slot::Unused,
        item(15, fixed(1)),
        item(70, fixed(3)),
        item(105, fixed(8)),
        item(100, fixed(6)),
        item(185, fixed(4)),
        item(210, fixed(2)),
        item(60, fixed_s(2, &[sp(0, 0x10)])),
        item(245, fixed_s(7, &[sp(0, 0x3F)])),
        item(380, compound(&SUBS_062_380)),
        item(40, fixed(2)),
        // 5.2.6: the sixth extent has spare bits 7/2.
        item(80, ext(1, &[sp(6, 0x7E)])),
        // 5.2.20: ten ages, one octet each except the ADS-C age (two).
        item(
            290,
            compound(&[
                fixed(1),
                fixed(1),
                fixed(1),
                fixed(1),
                fixed(2),
                fixed(1),
                fixed(1),
                fixed(1),
                fixed(1),
                fixed(1),
            ]),
        ),
        item(200, fixed_s(1, &[sp(0, 0x01)])),
        // 5.2.21: 31 one-octet ages.
        item(295, compound(&[fixed(1); 31])),
        item(136, fixed(2)),
        item(130, fixed(2)),
        item(135, fixed(2)),
        item(220, fixed(2)),
        item(390, compound(&SUBS_062_390)),
        item(270, ext(1, NO_SPARE)),
        item(300, fixed(1)),
        // 5.2.9 Mode 5 data: summary 1, PIN 4, position 6, GNSS altitude 2, extended Mode 1 2,
        // time offset 1, X pulse 1.
        item(
            110,
            compound(&[
                fixed(1),
                fixed(4),
                fixed(6),
                fixed(2),
                fixed(2),
                fixed(1),
                fixed(1),
            ]),
        ),
        item(120, fixed_s(2, &[sp(0, 0xF0)])),
        item(510, ext(3, NO_SPARE)),
        // 5.2.26 Estimated Accuracies: position 4, XY covariance 2, WGS-84 position 4,
        // geometric altitude 1, barometric altitude 1, velocity 2, acceleration 2, rate of
        // climb 1.
        item(
            500,
            compound(&[
                fixed(4),
                fixed(2),
                fixed(4),
                fixed(1),
                fixed(1),
                fixed(2),
                fixed(2),
                fixed(1),
            ]),
        ),
        // 5.2.23 Measured Information: sensor id 2, position 4, height 2, Mode C 2, Mode 3/A 2
        // (spare bit 13), report type 1 (spare bits 2/1).
        item(
            340,
            compound(&[
                fixed(2),
                fixed(4),
                fixed(2),
                fixed(2),
                fixed_s(2, &[sp(0, 0x10)]),
                fixed_s(1, &[sp(0, 0x03)]),
            ]),
        ),
        Slot::Unused,
        Slot::Unused,
        Slot::Unused,
        Slot::Unused,
        Slot::Unused,
        Slot::Re,
        Slot::Sp,
    ],
};

/// I062/380 Aircraft Derived Data, 28 subitems (5.2.24): target address 3, target
/// identification 6, magnetic heading 2, indicated airspeed 2, true airspeed 2, selected
/// altitude 2, final state selected altitude 2, trajectory intent status (extended),
/// trajectory intent data (repetitive, 15), communications/ACAS 2, status reported by ADS-B 2,
/// ACAS resolution advisory 7, barometric vertical rate 2, geometric vertical rate 2, roll
/// angle 2, track angle rate 2, track angle 2, ground speed 2, velocity uncertainty 1, met data
/// 8, emitter category 1, position 6, geometric altitude 2, position uncertainty 1 (spare bits
/// 8/5), BDS register data (repetitive, 8), indicated airspeed 2, Mach number 2, barometric
/// pressure setting 2.
static SUBS_062_380: [Field; 28] = [
    fixed(3),
    fixed(6),
    fixed(2),
    fixed(2),
    fixed(2),
    fixed(2),
    fixed(2),
    ext(1, &[sp(0, 0x3E)]),
    rep(15),
    fixed(2),
    fixed(2),
    fixed(7),
    fixed(2),
    fixed(2),
    fixed(2),
    fixed(2),
    fixed(2),
    fixed(2),
    fixed(1),
    fixed(8),
    fixed(1),
    fixed(6),
    fixed(2),
    fixed_s(1, &[sp(0, 0xF0)]),
    rep(8),
    fixed(2),
    fixed(2),
    fixed(2),
];

/// I062/390 Flight Plan Related Data, 18 subitems (5.2.25): FPPS identification tag 2,
/// callsign 7, IFPS flight id 4, flight category 1, type of aircraft 4, wake turbulence
/// category 1, departure airport 4, destination airport 4, runway designation 3, current
/// cleared flight level 2, current control position 2, time of departure/arrival (repetitive,
/// 4), aircraft stand 6, stand status 1, standard instrument departure 7, standard instrument
/// arrival 7, pre-emergency Mode 3/A 2, pre-emergency callsign 7.
static SUBS_062_390: [Field; 18] = [
    fixed(2),
    fixed(7),
    fixed(4),
    fixed(1),
    fixed(4),
    fixed(1),
    fixed(4),
    fixed(4),
    fixed(3),
    fixed(2),
    fixed(2),
    rep(4),
    fixed(6),
    fixed(1),
    fixed(7),
    fixed(7),
    fixed(2),
    fixed(7),
];

/// The UAP of a category, `None` for categories this crate does not carry.
pub fn uap_for(cat: u8) -> Option<&'static Uap> {
    uap_for_edition(cat, Cat021Edition::Ed27)
}

/// The UAP of a category with CAT021 read as edition `ed` (the other categories have one
/// edition here), `None` for categories this crate does not carry.
pub fn uap_for_edition(cat: u8, ed: Cat021Edition) -> Option<&'static Uap> {
    match cat {
        21 => Some(uap_021(ed)),
        48 => Some(&UAP_048),
        62 => Some(&UAP_062),
        _ => None,
    }
}

/// The slot of FRN `frn` (1-based), `None` past the end of the table.
pub fn slot_of(uap: &Uap, frn: u8) -> Option<&'static Slot> {
    let ix = usize::from(frn).checked_sub(1)?;
    uap.slots.get(ix)
}

/// The subitems of the compound item at FRN `frn`, empty if that slot is not a compound item.
pub fn compound_subs(uap: &Uap, frn: u8) -> &'static [Field] {
    match slot_of(uap, frn) {
        Some(Slot::Item {
            field:
                Field {
                    kind: Kind::Compound(subs),
                    ..
                },
            ..
        }) => subs,
        _ => &[],
    }
}

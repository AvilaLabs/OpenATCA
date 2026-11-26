//! The message set as data: for every message the fields it carries, in wire order, and which
//! elements are required, optional or not used (Part II Tables 3 to 27, Rev G, with the
//! Attachment 2 additions). The parser and the encoder both read this one table; adding or
//! correcting a message is a table row. Shell (input-format adapter, B1, C6): allocates per message, so not real-time core (C1).

use crate::model::{Kind, Profile};

/// An element's presence rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: constant definition table, read once per message
pub enum Req {
    /// Not used in this message: present is a refusal.
    No,
    Opt,
    Yes,
}

/// How a Field 18 is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: constant definition table, read once per message
pub enum F18Rule {
    /// `0` or indicator elements (FPL, CPL, CHG, CNL).
    Any,
    /// `0`, or indicator elements that include `RMK/` (TRQ, TRS).
    RemarkIfAny,
    /// `RMK/` and free text, not split (MIS).
    Remark,
    /// `RMK/code/field/text` (LRM).
    Lrm,
}

/// One field of a message, in wire order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: constant definition table, read once per message
pub enum Rule {
    /// `ssr` is 07(b) and (c); `functional` allows a functional address in 07(a).
    F07 {
        ssr: Req,
        functional: bool,
    },
    F08,
    F09,
    F10,
    /// `time` is 13(b).
    F13 {
        time: Req,
    },
    F14,
    F15,
    /// `eet` is 16(b), `alt` is 16(c).
    F16 {
        eet: Req,
        alt: Req,
    },
    /// `required` false: the whole field may be absent.
    F18 {
        rule: F18Rule,
        required: bool,
    },
    /// Field 22 amendments; `abi` selects the ABI form (fields 9 and 15 required).
    F22 {
        abi: bool,
    },
    /// `sector` is 31(b), `text` is 31(c).
    F31 {
        sector: Req,
        text: Req,
    },
    F32,
}

/// A message type's definition.
#[derive(Debug, Clone, Copy)]
// A7.2: cold struct, size guard waived: constant definition table, read once per message
pub struct Spec {
    pub kind: Kind,
    pub mnemonic: &'static str,
    /// Table section of the Rev G ICD.
    pub section: &'static str,
    /// Field 03(c), reference data.
    pub reference: Req,
    pub fields: &'static [Rule],
}

const F07_ACID: Rule = Rule::F07 {
    ssr: Req::Opt,
    functional: false,
};
const F07_NO_SSR: Rule = Rule::F07 {
    ssr: Req::No,
    functional: false,
};
const F07_SSR: Rule = Rule::F07 {
    ssr: Req::Yes,
    functional: false,
};
const F13_TIME: Rule = Rule::F13 { time: Req::Yes };
const F13_NO_TIME: Rule = Rule::F13 { time: Req::No };
const F16_A: Rule = Rule::F16 {
    eet: Req::No,
    alt: Req::No,
};
const F18_ANY: Rule = Rule::F18 {
    rule: F18Rule::Any,
    required: true,
};
const F18_RMK_OPT: Rule = Rule::F18 {
    rule: F18Rule::RemarkIfAny,
    required: false,
};

/// Every message of the NAM set and the ICAO DLA field message.
pub const SPECS: [Spec; 26] = [
    Spec {
        kind: Kind::Fpl,
        mnemonic: "FPL",
        section: "3.1.1 Table 3",
        reference: Req::No,
        fields: &[
            F07_ACID,
            Rule::F08,
            Rule::F09,
            Rule::F10,
            F13_TIME,
            Rule::F15,
            Rule::F16 {
                eet: Req::Yes,
                alt: Req::Opt,
            },
            F18_ANY,
        ],
    },
    Spec {
        kind: Kind::Dla,
        mnemonic: "DLA",
        section: "ICAO Doc 4444 Appendix 3 2.3.4",
        reference: Req::Opt,
        fields: &[F07_NO_SSR, F13_TIME, F16_A, F18_ANY],
    },
    Spec {
        kind: Kind::Chg,
        mnemonic: "CHG",
        section: "3.1.2 Table 4",
        reference: Req::Yes,
        // Table 4 lists 13(b) and Field 18 as required; the ICD's own ACID-change example
        // (3.1.2.3, Field 22 `07/` alone) omits both. They are optional in the table here and
        // required again, except for that one change, by `message::post_checks`. README.
        fields: &[
            F07_ACID,
            Rule::F13 { time: Req::Opt },
            F16_A,
            Rule::F18 {
                rule: F18Rule::Any,
                required: false,
            },
            Rule::F22 { abi: false },
        ],
    },
    Spec {
        kind: Kind::Est,
        mnemonic: "EST",
        section: "3.1.3 Table 5",
        reference: Req::Yes,
        fields: &[F07_ACID, F13_NO_TIME, Rule::F14, F16_A],
    },
    Spec {
        kind: Kind::Cnl,
        mnemonic: "CNL",
        section: "3.1.4 Table 6",
        reference: Req::Yes,
        fields: &[F07_NO_SSR, Rule::F13 { time: Req::Opt }, F16_A, F18_ANY],
    },
    Spec {
        kind: Kind::Cpl,
        mnemonic: "CPL",
        section: "3.2.1 Table 7",
        reference: Req::No,
        fields: &[
            F07_ACID,
            Rule::F08,
            Rule::F09,
            Rule::F10,
            F13_NO_TIME,
            Rule::F14,
            Rule::F15,
            F16_A,
            F18_ANY,
        ],
    },
    Spec {
        kind: Kind::Mod,
        mnemonic: "MOD",
        section: "3.2.2 Table 8",
        reference: Req::Yes,
        fields: &[F07_ACID, F13_NO_TIME, F16_A, Rule::F22 { abi: false }],
    },
    Spec {
        kind: Kind::Abi,
        mnemonic: "ABI",
        section: "3.2.3 Table 9",
        reference: Req::No,
        fields: &[
            F07_ACID,
            F13_NO_TIME,
            Rule::F14,
            F16_A,
            Rule::F22 { abi: true },
        ],
    },
    Spec {
        kind: Kind::Mis,
        mnemonic: "MIS",
        section: "3.3.1 Table 10",
        reference: Req::No,
        fields: &[
            Rule::F07 {
                ssr: Req::No,
                functional: true,
            },
            Rule::F18 {
                rule: F18Rule::Remark,
                required: true,
            },
        ],
    },
    Spec {
        kind: Kind::Irq,
        mnemonic: "IRQ",
        section: "3.4.1 Table 11",
        reference: Req::No,
        fields: &[],
    },
    Spec {
        kind: Kind::Irs,
        mnemonic: "IRS",
        section: "3.4.2 Table 12",
        reference: Req::Yes,
        fields: &[],
    },
    Spec {
        kind: Kind::Trq,
        mnemonic: "TRQ",
        section: "3.4.3 Table 13",
        reference: Req::No,
        fields: &[F18_RMK_OPT],
    },
    Spec {
        kind: Kind::Trs,
        mnemonic: "TRS",
        section: "3.4.4 Table 14",
        reference: Req::Yes,
        fields: &[F18_RMK_OPT],
    },
    Spec {
        kind: Kind::Asm,
        mnemonic: "ASM",
        section: "3.4.5 Table 15",
        reference: Req::No,
        fields: &[],
    },
    Spec {
        kind: Kind::Rti,
        mnemonic: "RTI",
        section: "3.6.1 Table 18",
        reference: Req::Yes,
        fields: &[
            F07_SSR,
            F13_NO_TIME,
            F16_A,
            Rule::F31 {
                sector: Req::Opt,
                text: Req::No,
            },
            Rule::F32,
        ],
    },
    Spec {
        kind: Kind::Rtu,
        mnemonic: "RTU",
        section: "3.6.3 Table 20",
        reference: Req::Yes,
        fields: &[F07_SSR, F13_NO_TIME, F16_A, Rule::F32],
    },
    Spec {
        kind: Kind::Rla,
        mnemonic: "RLA",
        section: "3.6.2 Table 19",
        reference: Req::Yes,
        fields: &[Rule::F31 {
            sector: Req::Yes,
            text: Req::No,
        }],
    },
    Spec {
        kind: Kind::Rta,
        mnemonic: "RTA",
        section: "3.6.4 Table 21",
        reference: Req::Yes,
        fields: &[
            F07_SSR,
            F13_NO_TIME,
            F16_A,
            // 31(c) is not in Table 21; Attachment 2 2.17 adds the CPDLC contact for ERAM and
            // NAV CANADA. `message` refuses it under `Profile::Core`.
            Rule::F31 {
                sector: Req::Yes,
                text: Req::Opt,
            },
        ],
    },
    Spec {
        kind: Kind::Poi,
        mnemonic: "POI",
        section: "3.7.1 Table 22",
        reference: Req::Opt,
        fields: &[
            F07_SSR,
            F13_NO_TIME,
            F16_A,
            Rule::F31 {
                sector: Req::Opt,
                text: Req::Opt,
            },
            Rule::F32,
        ],
    },
    Spec {
        kind: Kind::Pla,
        mnemonic: "PLA",
        section: "3.7.2 Table 23",
        reference: Req::Yes,
        fields: &[Rule::F31 {
            sector: Req::Yes,
            text: Req::Opt,
        }],
    },
    Spec {
        kind: Kind::Poa,
        mnemonic: "POA",
        section: "3.7.3 Table 24",
        reference: Req::Yes,
        // 07(b) and (c) are Optional: Rev G lists 07 a only, Rev F listed a, b. See the README.
        fields: &[
            F07_ACID,
            F13_NO_TIME,
            F16_A,
            Rule::F31 {
                sector: Req::Yes,
                text: Req::Opt,
            },
        ],
    },
    Spec {
        kind: Kind::Poj,
        mnemonic: "POJ",
        section: "3.7.4 Table 25",
        reference: Req::Yes,
        fields: &[
            F07_NO_SSR,
            F13_NO_TIME,
            F16_A,
            Rule::F31 {
                sector: Req::Yes,
                text: Req::Opt,
            },
        ],
    },
    Spec {
        kind: Kind::Toc,
        mnemonic: "TOC",
        section: "3.8.1 Table 26",
        reference: Req::Yes,
        fields: &[F07_ACID, F13_TIME, F16_A],
    },
    Spec {
        kind: Kind::Aoc,
        mnemonic: "AOC",
        section: "3.8.2 Table 27",
        reference: Req::Yes,
        fields: &[F07_ACID, F13_TIME, F16_A],
    },
    Spec {
        kind: Kind::Lam,
        mnemonic: "LAM",
        section: "3.5.1 Table 16",
        reference: Req::Yes,
        fields: &[],
    },
    Spec {
        kind: Kind::Lrm,
        mnemonic: "LRM",
        section: "3.5.2 Table 17",
        reference: Req::Yes,
        fields: &[Rule::F18 {
            rule: F18Rule::Lrm,
            required: true,
        }],
    },
];

/// The definition of a message type, or `None` if `SPECS` has none (E6: never another
/// message's definition). A test checks that every [`Kind`] has one.
pub fn spec(kind: Kind) -> Option<&'static Spec> {
    spec_in(&SPECS, kind)
}

/// The entry for `kind` in `table`, `None` when the table has none.
pub fn spec_in(table: &[Spec], kind: Kind) -> Option<&Spec> {
    table.iter().find(|s| s.kind == kind)
}

/// Rev E (NAS-IC-21009205, 15 April 2016) differs from Rev G in four messages: the ASM carries
/// 03(c) (Table 15), the POI does not (Table 22), the POA carries 07 a, b, c (Table 24) and the
/// POJ is `03 a, b, c` and a Field 18 giving the reason, `RMK/...` or `0` (Table 25).
const REV_E_SPECS: [Spec; 4] = [
    Spec {
        kind: Kind::Asm,
        mnemonic: "ASM",
        section: "Rev E 3.4.5 Table 15",
        reference: Req::Yes,
        fields: &[],
    },
    Spec {
        kind: Kind::Poi,
        mnemonic: "POI",
        section: "Rev E 3.7.1 Table 22",
        reference: Req::No,
        fields: &[
            F07_SSR,
            F13_NO_TIME,
            F16_A,
            Rule::F31 {
                sector: Req::Opt,
                text: Req::Opt,
            },
            Rule::F32,
        ],
    },
    Spec {
        kind: Kind::Poa,
        mnemonic: "POA",
        section: "Rev E 3.7.3 Table 24",
        reference: Req::Yes,
        fields: &[
            F07_SSR,
            F13_NO_TIME,
            F16_A,
            Rule::F31 {
                sector: Req::Yes,
                text: Req::Opt,
            },
        ],
    },
    Spec {
        kind: Kind::Poj,
        mnemonic: "POJ",
        section: "Rev E 3.7.4 Table 25",
        reference: Req::Yes,
        fields: &[Rule::F18 {
            rule: F18Rule::RemarkIfAny,
            required: true,
        }],
    },
];

/// The definition of a message type under a profile: [`spec`], except the four messages Rev E
/// defines differently under [`Profile::RevE`].
pub fn spec_for(kind: Kind, profile: Profile) -> Option<&'static Spec> {
    if profile == Profile::RevE {
        if let Some(s) = spec_in(&REV_E_SPECS, kind) {
            return Some(s);
        }
    }
    spec(kind)
}

/// The message type for a three-letter identifier.
pub fn kind_of(mnemonic: &[u8]) -> Option<Kind> {
    SPECS
        .iter()
        .find(|s| s.mnemonic.as_bytes() == mnemonic)
        .map(|s| s.kind)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::model::ALL_KINDS;

    // REQ: OA-NAM-032
    #[test]
    fn an_unknown_kind_has_no_spec_never_another_messages() {
        // A table that lacks the kind gives None, not the first entry (E6).
        let short = &SPECS[..1];
        assert_eq!(spec_in(short, Kind::Fpl).map(|s| s.kind), Some(Kind::Fpl));
        assert!(spec_in(short, Kind::Lrm).is_none());
        assert!(spec_in(&[], Kind::Fpl).is_none());
        for k in ALL_KINDS {
            assert_eq!(spec(k).unwrap().kind, k);
            assert_eq!(spec_for(k, Profile::RevE).unwrap().kind, k);
        }
    }
}

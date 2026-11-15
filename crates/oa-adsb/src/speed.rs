//! The position-jump rule: is a displacement between two reports of one aircraft possible?
//! Core (pure). Replaces a single fixed speed limit, which rejected real fast aircraft.
//!
//! # Where the numbers come from
//!
//! * The ADS-B airborne velocity message (DO-260B 2.2.3.2.6) has a 10-bit ground-speed field per
//!   axis: subtype 1 (subsonic) in 1 kt steps up to 1021.5 kt, subtype 2 (supersonic) in 4 kt
//!   steps up to 4086 kt per axis. So 1,000 kt is the most any subsonic emitter can even report,
//!   and no emitter reports above about 4,100 kt.
//! * Subsonic airliners and business jets cruise at Mach 0.80 to 0.935 (Citation X 0.935 is the
//!   fastest certificated); at 36,000 ft that is about 530 kt true airspeed. Jet-stream winds
//!   of 200 to 250 kt are rare extremes, which put the largest ground speeds of airliners near
//!   780 kt. Published record ground speeds of airliners (about 800 mph, 700 kt, in Atlantic
//!   jet-stream crossings in 2020) are below that. These figures are from the author's reading
//!   of public reports and handbooks and were not looked up online while writing; the
//!   limits below have a margin that does not depend on their exact values.
//! * Military and research aircraft: fighters and bombers fly Mach 1.5 to 2.5 (1,000 to 1,650 kt
//!   at altitude), the SR-71 Mach 3.3 (about 1,900 kt); Concorde (retired 2003) cruised at Mach
//!   2.04 (about 1,350 kt).
//! * The ADS-B emitter categories (DO-260B table 2-17): set A `A0` no information, `A1` light
//!   (under 15,500 lb), `A2` small (to 75,000 lb), `A3` large (to 300,000 lb), `A4` high-vortex
//!   large (B-757), `A5` heavy, `A6` high performance (over 5 g and over 400 kt), `A7` rotorcraft;
//!   set B `B1` glider, `B2` lighter than air, `B3` parachutist, `B4` ultralight, `B6` unmanned
//!   aircraft, `B7` space or trans-atmospheric vehicle; set C `C1` to `C3` surface vehicles and
//!   obstacles. Only `A6` (and `A0`, `B0`, `B6`, which say nothing) can be fast; `B7` is the
//!   only class past 2,000 kt.
//!
//! # The rule
//!
//! A pair of reports `dist_m` apart and `dt_s` apart is a jump unless all hold:
//!
//! 1. the implied speed (after `slack_m` of position noise) is under the **hard cap** of the
//!    category class: 1,000 kt for ordinary classes (the subsonic field maximum), 2,000 kt for
//!    `A6`, `A0`, `B0`, `B6` and unknown categories, 4,088 kt for `B7`;
//! 2. if either report gives its ground speed, the implied speed is consistent with it: at most
//!    1.2 times the larger reported speed plus 40 kt plus 2 kt for each second of the gap (a
//!    deceleration or acceleration allowance of about 0.1 g, in line with transport aircraft;
//!    a chord is never longer than the path). So a real 900 kt aircraft that says it flies 900 kt
//!    is never split, and a 450 kt aircraft that jumps 2,800 kt is caught even though 2,800
//!    kt is within the hard cap of some classes;
//! 3. if neither gives a ground speed, the implied speed is under the **ceiling** of the class
//!    (the speed the class can fly plus 250 kt of wind).

/// Hard cap for ordinary emitter classes, knots: the subsonic ground-speed field maximum.
pub const CAP_ORDINARY_KT: f64 = 1000.0;
/// Hard cap for high-performance, no-information, unmanned and unknown categories, knots.
pub const CAP_HIGH_KT: f64 = 2000.0;
/// Hard cap for space and trans-atmospheric vehicles, knots: the supersonic field maximum.
pub const CAP_SPACE_KT: f64 = 4088.0;
/// Multiplier on the reported ground speed in the consistency test.
pub const CONSISTENCY_FACTOR: f64 = 1.2;
/// Constant added in the consistency test, knots.
pub const CONSISTENCY_BASE_KT: f64 = 40.0;
/// Speed change allowed per second of the gap in the consistency test, knots per second
/// (about 0.1 g).
pub const CONSISTENCY_ACCEL_KT_S: f64 = 2.0;

const MPS_PER_KT: f64 = 0.514_444_444_444;

/// Why a displacement was judged impossible; [`JumpVerdict::Plausible`] if it was not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpVerdict {
    Plausible,
    /// Faster than the hard cap of the category class.
    OverCap,
    /// Not consistent with the reported ground speed and the time gap.
    InconsistentWithGroundSpeed,
    /// No ground speed reported and faster than the class ceiling.
    OverCeiling,
}

/// One pair of reports to judge. Plain record (A1).
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct JumpPair {
    /// Distance between the two positions, metres.
    pub dist_m: f64,
    /// Time from the first report to the second, seconds.
    pub dt_s: f64,
    /// Position noise allowance added to the distance, metres.
    pub slack_m: f64,
    /// Emitter category byte (`"A3"` is 0xA3), 0 if not known.
    pub category: u8,
    /// Ground speed of the first and second report, 0.1 kt, [`crate::NONE_U16`] if absent.
    pub gs_a_dkt: u16,
    pub gs_b_dkt: u16,
}

/// The cap of a category's class, knots, given the cap for ordinary classes (normally
/// [`CAP_ORDINARY_KT`]). The ordinary cap is never raised above the high caps.
pub fn category_hard_cap_kt(category: u8, ordinary_cap_kt: f64) -> f64 {
    match category {
        0xb7 => CAP_SPACE_KT,
        0xa6 | 0xa0 | 0xb0 | 0xb6 | 0 => CAP_HIGH_KT.max(ordinary_cap_kt),
        0xa1..=0xa5 | 0xa7 | 0xb1..=0xb4 | 0xc1..=0xc3 => ordinary_cap_kt,
        // Reserved codes (`A8`, `B5`, set D, ...) say nothing, like no information.
        _ => CAP_HIGH_KT.max(ordinary_cap_kt),
    }
}

/// The ground speed a category's class can reach with a strong tailwind, knots; used only when
/// no ground speed is reported. Transport classes: Mach 0.935 (about 535 kt) plus 250 kt wind.
pub fn category_ceiling_kt(category: u8) -> f64 {
    match category {
        // Light aircraft and rotorcraft.
        0xa1 | 0xa7 => 600.0,
        // Gliders, balloons, parachutists, ultralights.
        0xb1..=0xb4 => 450.0,
        // Small, large, high-vortex and heavy transports.
        0xa2..=0xa5 => 800.0,
        // Surface vehicles and obstacles.
        0xc1..=0xc3 => 200.0,
        // Unmanned aircraft: the fastest are jets.
        0xb6 => 800.0,
        0xb7 => CAP_SPACE_KT,
        // High performance, no information, unknown.
        _ => CAP_HIGH_KT,
    }
}

/// Judges one pair. `ordinary_cap_kt` is the hard cap of the ordinary classes
/// ([`CAP_ORDINARY_KT`] unless a caller configures it). A displacement within `slack_m` is
/// always plausible; a pair with no time between them (`dt_s <= 0`) is plausible only then.
pub fn jump_verdict(p: &JumpPair, ordinary_cap_kt: f64) -> JumpVerdict {
    let within = |kt: f64| p.dist_m <= kt * MPS_PER_KT * p.dt_s.max(0.0) + p.slack_m;
    if p.dist_m <= p.slack_m {
        return JumpVerdict::Plausible;
    }
    if p.dt_s.is_nan() || p.dt_s <= 0.0 || p.dist_m.is_nan() {
        return JumpVerdict::OverCap;
    }
    let cap = category_hard_cap_kt(p.category, ordinary_cap_kt);
    if !within(cap) {
        return JumpVerdict::OverCap;
    }
    let known = |g: u16| (g != crate::NONE_U16).then(|| f64::from(g) / 10.0);
    match (known(p.gs_a_dkt), known(p.gs_b_dkt)) {
        (None, None) => {
            if within(category_ceiling_kt(p.category)) {
                JumpVerdict::Plausible
            } else {
                JumpVerdict::OverCeiling
            }
        }
        (a, b) => {
            let g = a.unwrap_or(0.0).max(b.unwrap_or(0.0));
            let allowed =
                CONSISTENCY_FACTOR * g + CONSISTENCY_BASE_KT + CONSISTENCY_ACCEL_KT_S * p.dt_s;
            if within(allowed) {
                JumpVerdict::Plausible
            } else {
                JumpVerdict::InconsistentWithGroundSpeed
            }
        }
    }
}

/// True if the pair is a position jump (any verdict but [`JumpVerdict::Plausible`]).
pub fn is_jump(p: &JumpPair, ordinary_cap_kt: f64) -> bool {
    jump_verdict(p, ordinary_cap_kt) != JumpVerdict::Plausible
}

/// True for the addresses a misconfigured transponder sends: 0x000000 and 0xFFFFFF are not
/// assigned to any aircraft (all-zeros and all-ones are reserved), so many unrelated aircraft
/// share them. Takes the
/// address word of [`crate::Reports::icao`]: a non-ICAO address ([`crate::NON_ICAO`] set, a
/// readsb pseudo-address) is a trackfile identifier and is not reserved.
pub fn is_reserved_address(addr: u32) -> bool {
    addr == 0 || addr == 0x00ff_ffff
}

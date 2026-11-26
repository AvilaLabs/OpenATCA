#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! Fixed-width ADS-B category and speed rules shared by pure engine crates.
//! Wire decoding and archive access remain in the `oa-adsb` shell package.

#[cfg(not(any(
    target_pointer_width = "64",
    all(
        target_arch = "wasm32",
        target_pointer_width = "32",
        target_os = "unknown",
        target_vendor = "unknown"
    )
)))]
compile_error!("core supports native 64-bit targets and wasm32-unknown-unknown only");

mod category;
mod speed;

pub use category::{category_is_reserved, category_observe, CategoryState, CATEGORY_PERSIST};
pub use quality::{nacp_epu_m, nacv_epu_mps};
pub use speed::{
    category_ceiling_kt, category_hard_cap_kt, is_jump, is_reserved_address, jump_verdict,
    JumpPair, JumpVerdict, CAP_HIGH_KT, CAP_ORDINARY_KT, CAP_SPACE_KT, CONSISTENCY_ACCEL_KT_S,
    CONSISTENCY_BASE_KT, CONSISTENCY_FACTOR, SPEED_ENVELOPE_KT, SPEED_ENVELOPE_MPS,
};

/// Absent `u16` value used by the jump-rule speed pair.
pub const NONE_U16: u16 = u16::MAX;

mod quality;

/// Multiplier from one-axis standard deviation to the radius of a 95% circular normal.
pub const SIGMA95: f64 = 2.447_746_830_680_816;
/// Distance allowance for the conservative speed jump check, metres.
pub const JUMP_SLACK_M: f64 = 200.0;
/// Consecutive speed-rule rejections before resynchronising a leg.
pub const JUMP_RESYNC: u32 = 5;

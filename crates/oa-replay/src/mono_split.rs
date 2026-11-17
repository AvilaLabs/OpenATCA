//! Core (pure): checked split of elapsed wall time into monotonic time and recorder-clock step.
//!
//! This small pure helper is shared verbatim with the isolated Creusot assurance package.
//! Keep its decision order and checked operations aligned with `clock::classify`'s original
//! implementation (RULESET E3).

// F1: used only by the explicitly approved proof-only package, excluded from product builds.
#[cfg(creusot)]
use creusot_std::prelude::*;

/// Returns `(wall_ns, mono_ns, step_ns)` when the monotonic reference is comparable and all
/// checked integer operations are representable. `None` covers every ineligible or
/// unrepresentable case.
#[cfg_attr(
    creusot,
    ensures(|result| match (mono, result) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(m), None) => {
            !ref_ok
                || ref_stream != stream
                || m@ < ref_mono_ns@
                || m@ - ref_mono_ns@ > i64::MAX@
                || t@ - ref_wall_ns@ < i64::MIN@
                || t@ - ref_wall_ns@ > i64::MAX@
                || (t@ - ref_wall_ns@) - (m@ - ref_mono_ns@) < i64::MIN@
                || (t@ - ref_wall_ns@) - (m@ - ref_mono_ns@) > i64::MAX@
        }
        (Some(m), Some((wall, mono_delta, step))) => {
            ref_ok
                && ref_stream == stream
                && m@ >= ref_mono_ns@
                && m@ - ref_mono_ns@ <= i64::MAX@
                && i64::MIN@ <= t@ - ref_wall_ns@
                && t@ - ref_wall_ns@ <= i64::MAX@
                && i64::MIN@ <= (t@ - ref_wall_ns@) - (m@ - ref_mono_ns@)
                && (t@ - ref_wall_ns@) - (m@ - ref_mono_ns@) <= i64::MAX@
                && wall@ == t@ - ref_wall_ns@
                && mono_delta@ == m@ - ref_mono_ns@
                && step@ == (t@ - ref_wall_ns@) - (m@ - ref_mono_ns@)
        }
    })
)]
pub(crate) fn split(
    ref_ok: bool,
    ref_stream: u32,
    ref_wall_ns: i64,
    ref_mono_ns: u64,
    t: i64,
    stream: u32,
    mono: Option<u64>,
) -> Option<(i64, i64, i64)> {
    let m = mono?;
    if !ref_ok || ref_stream != stream || m < ref_mono_ns {
        return None;
    }
    let mono_delta = m - ref_mono_ns;
    // E8: establish the conversion bound before casting. The explicit primitive check also
    // lets the proof-only Creusot package verify the exact conversion without a TryFrom model.
    if mono_delta > i64::MAX as u64 {
        return None;
    }
    let mono_ns = mono_delta as i64;
    let wall_ns = t.checked_sub(ref_wall_ns)?;
    wall_ns
        .checked_sub(mono_ns)
        .map(|step_ns| (wall_ns, mono_ns, step_ns))
}

#[cfg(test)]
#[path = "mono_split_tests.rs"]
mod tests;

#[cfg(kani)]
mod proofs {
    use super::split;

    /// Independent mathematical reference using i128, whose range contains every intermediate
    /// difference of two i64/u64 values and the subsequent subtraction.
    fn i128_reference(
        ref_ok: bool,
        ref_stream: u32,
        ref_wall_ns: i64,
        ref_mono_ns: u64,
        t: i64,
        stream: u32,
        mono: Option<u64>,
    ) -> Option<(i64, i64, i64)> {
        let m = mono?;
        if !ref_ok || ref_stream != stream || m < ref_mono_ns {
            return None;
        }
        let mono_delta = i128::from(m) - i128::from(ref_mono_ns);
        let wall_delta = i128::from(t) - i128::from(ref_wall_ns);
        let step = wall_delta - mono_delta;
        let fits_i64 = |x: i128| i128::from(i64::MIN) <= x && x <= i128::from(i64::MAX);
        if !fits_i64(mono_delta) || !fits_i64(wall_delta) || !fits_i64(step) {
            return None;
        }
        Some((wall_delta as i64, mono_delta as i64, step as i64))
    }

    // REQ: OA-RPL-272
    #[kani::proof]
    fn split_matches_i128_for_the_full_typed_domain() {
        let ref_ok: bool = kani::any();
        let ref_stream: u32 = kani::any();
        let ref_wall_ns: i64 = kani::any();
        let ref_mono_ns: u64 = kani::any();
        let t: i64 = kani::any();
        let stream: u32 = kani::any();
        let has_mono: bool = kani::any();
        let mono_value: u64 = kani::any();
        let mono = if has_mono { Some(mono_value) } else { None };

        let actual = split(
            ref_ok,
            ref_stream,
            ref_wall_ns,
            ref_mono_ns,
            t,
            stream,
            mono,
        );
        let expected = i128_reference(
            ref_ok,
            ref_stream,
            ref_wall_ns,
            ref_mono_ns,
            t,
            stream,
            mono,
        );
        assert_eq!(actual, expected);

        let eligible = has_mono && ref_ok && ref_stream == stream;
        let mut mono_representable = false;
        let mut wall_representable = false;
        let mut monotonic_regression = false;
        let mut mono_delta_too_large = false;
        if has_mono && mono_value >= ref_mono_ns {
            let mono_delta = i128::from(mono_value) - i128::from(ref_mono_ns);
            monotonic_regression = false;
            mono_representable = mono_delta <= i128::from(i64::MAX);
            mono_delta_too_large = mono_delta > i128::from(i64::MAX);
            let wall_delta = i128::from(t) - i128::from(ref_wall_ns);
            wall_representable =
                i128::from(i64::MIN) <= wall_delta && wall_delta <= i128::from(i64::MAX);
        } else if has_mono {
            monotonic_regression = mono_value < ref_mono_ns;
        }
        let wall_underflow = eligible && mono_value >= ref_mono_ns && mono_representable && {
            let wall_delta = i128::from(t) - i128::from(ref_wall_ns);
            wall_delta < i128::from(i64::MIN)
        };
        let wall_overflow = eligible && mono_value >= ref_mono_ns && mono_representable && {
            let wall_delta = i128::from(t) - i128::from(ref_wall_ns);
            wall_delta > i128::from(i64::MAX)
        };
        let step_underflow =
            eligible && mono_value >= ref_mono_ns && wall_representable && mono_representable && {
                let wall_delta = i128::from(t) - i128::from(ref_wall_ns);
                let mono_delta = i128::from(mono_value) - i128::from(ref_mono_ns);
                wall_delta - mono_delta < i128::from(i64::MIN)
            };
        kani::cover!(!has_mono, "absent monotonic timestamp");
        kani::cover!(has_mono && !ref_ok, "invalid monotonic reference");
        kani::cover!(
            has_mono && ref_ok && ref_stream != stream,
            "different monotonic stream"
        );
        kani::cover!(
            has_mono && ref_ok && ref_stream == stream && monotonic_regression,
            "monotonic regression"
        );
        kani::cover!(
            eligible && mono_delta_too_large,
            "monotonic delta exceeds i64"
        );
        kani::cover!(eligible && wall_underflow, "wall delta underflow");
        kani::cover!(eligible && wall_overflow, "wall delta overflow");
        kani::cover!(eligible && step_underflow, "clock step underflow");
        kani::cover!(actual.is_some(), "representable timestamp split");
    }
}

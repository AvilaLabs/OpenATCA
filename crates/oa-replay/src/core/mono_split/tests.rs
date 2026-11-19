use super::split;

// The exact pre-extraction implementation, retained here only to detect extraction drift.
// Preserve the pre-extraction control flow for the parity reference.
#[allow(clippy::question_mark)]
fn old_inline(
    ref_ok: bool,
    ref_stream: u32,
    ref_wall_ns: i64,
    ref_mono_ns: u64,
    t: i64,
    stream: u32,
    mono: Option<u64>,
) -> Option<(i64, i64, i64)> {
    let Some(m) = mono else { return None };
    if !ref_ok || ref_stream != stream || m < ref_mono_ns {
        return None;
    }
    let Ok(mono_ns) = i64::try_from(m - ref_mono_ns) else {
        return None;
    };
    let Some(wall_ns) = t.checked_sub(ref_wall_ns) else {
        return None;
    };
    wall_ns
        .checked_sub(mono_ns)
        .map(|step_ns| (wall_ns, mono_ns, step_ns))
}

// Independent mathematical oracle: use i128 throughout and explicitly require every returned
// difference to fit the production field type.
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
#[test]
fn extracted_helper_matches_old_inline_and_i128_reference_at_boundaries() {
    let signed = [i64::MIN, i64::MIN + 1, -1, 0, 1, i64::MAX - 1, i64::MAX];
    let monotonic = [
        0,
        1,
        i64::MAX as u64,
        i64::MAX as u64 + 1,
        u64::MAX - 1,
        u64::MAX,
    ];
    let mut cases = 0usize;
    for &ref_ok in &[false, true] {
        for &ref_stream in &[0, 1] {
            for &stream in &[0, 1] {
                for &ref_wall in &signed {
                    for &t in &signed {
                        for &ref_mono in &monotonic {
                            for mono in [
                                None,
                                Some(0),
                                Some(1),
                                Some(i64::MAX as u64),
                                Some(i64::MAX as u64 + 1),
                                Some(u64::MAX),
                            ] {
                                let actual =
                                    split(ref_ok, ref_stream, ref_wall, ref_mono, t, stream, mono);
                                assert_eq!(
                                    actual,
                                    old_inline(
                                        ref_ok, ref_stream, ref_wall, ref_mono, t, stream, mono,
                                    )
                                );
                                assert_eq!(
                                    actual,
                                    i128_reference(
                                        ref_ok, ref_stream, ref_wall, ref_mono, t, stream, mono,
                                    )
                                );
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 2 * 2 * 2 * 7 * 7 * 6 * 6);
}

// REQ: OA-RPL-272
#[test]
fn exercises_each_refusal_class_and_signed_boundary_successes() {
    let base = (true, 7, 0_i64, 10_u64, 0_i64, 7_u32);
    let (ok, stream_ref, wall_ref, mono_ref, t, stream) = base;
    assert_eq!(
        split(ok, stream_ref, wall_ref, mono_ref, t, stream, None),
        None
    );
    assert_eq!(
        split(false, stream_ref, wall_ref, mono_ref, t, stream, Some(10)),
        None
    );
    assert_eq!(
        split(ok, stream_ref, wall_ref, mono_ref, t, 8, Some(10)),
        None
    );
    assert_eq!(
        split(ok, stream_ref, wall_ref, mono_ref, t, stream, Some(9)),
        None
    );
    assert_eq!(
        split(ok, stream_ref, 0, 0, 0, stream, Some(i64::MAX as u64 + 1)),
        None
    );
    assert_eq!(
        split(ok, stream_ref, i64::MIN, 0, i64::MAX, stream, Some(0)),
        None
    );
    assert_eq!(split(ok, stream_ref, 0, 0, i64::MIN, stream, Some(1)), None);
    assert_eq!(
        split(ok, stream_ref, 0, 0, i64::MIN, stream, Some(0)),
        Some((i64::MIN, 0, i64::MIN))
    );
    assert_eq!(
        split(ok, stream_ref, 0, 0, 0, stream, Some(i64::MAX as u64)),
        Some((0, i64::MAX, -i64::MAX))
    );
    assert_eq!(
        split(ok, stream_ref, 0, 0, i64::MAX, stream, Some(0)),
        Some((i64::MAX, 0, i64::MAX))
    );
}

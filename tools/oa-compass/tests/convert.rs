//! The core conversion of one row to the values of one record (OA-CMP-001 to OA-CMP-006).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_compass::core::convert::*;
use oa_compass::core::time::{civil_from_days, day_time_of_ns, days_from_civil, UNITS_PER_DAY};

/// 2026-11-15T12:00:00Z, Unix ns.
const T0_NS: i64 = 1_794_744_000_000_000_000;

fn row() -> TrackRow {
    TrackRow {
        t_ns: T0_NS,
        lat_e7: 411_234_567,
        lon_e7: -881_234_567,
        vel_east_mps: 100.0,
        vel_north_mps: -50.13,
        pos_cov_m2: [100.0, 0.0, 400.0],
        alt_ft: 35_012.0,
        has_alt: true,
        vrate_fpm: -1_500.0,
        coast: false,
        radar_disagrees: false,
        uncertain: false,
        reacquired: false,
        identity_unconfirmed: false,
        address: Some(0xA1B2C3),
        monosensor: true,
    }
}

fn eram() -> EramRow {
    EramRow {
        pos_ms: T0_NS / 1_000_000,
        lat_e7: 411_234_567,
        lon_e7: -881_234_567,
        alt_ft: Some(35_000),
        vel_dkt: Some((2_000, -1_000)),
        coast: false,
        address: Some(0xA1B2C3),
        address_withheld: false,
        callsign: Some(*b"UAL123\0\0"),
    }
}

fn has(c: &Converted, note: usize) -> bool {
    c.notes & note_bit(note) != 0
}

// REQ: OA-CMP-001
#[test]
fn a_track_row_becomes_counts_of_the_item_lsbs() {
    let c = track_record(&row()).unwrap();
    let v = c.values;
    // 2^25 counts per 180 degrees; 41.1234567 degrees.
    let want_lat = (411_234_567f64 * 33_554_432.0 / 1.8e9).round() as i32;
    let want_lon = (-881_234_567f64 * 33_554_432.0 / 1.8e9).round() as i32;
    assert_eq!((v.lat, v.lon), (want_lat, want_lon));
    assert_eq!(v.velocity, Some((400, -201))); // 100/0.25, -50.13/0.25 = -200.5 -> -201
    assert_eq!(v.baro_alt, Some(1400)); // 35012 / 25 = 1400.48
    assert_eq!(v.measured_fl, None);
    assert_eq!(v.climb, Some(-240)); // -1500 / 6.25
                                     // 12:00:00 UTC is 43200 s = 5 529 600 counts.
    assert_eq!(v.time, 43_200 * 128);
    assert_eq!(v.service, Some(1));
    assert_eq!(v.address, Some(0xA1B2C3));
}

// REQ: OA-CMP-001
#[test]
fn an_eram_sample_becomes_counts_with_the_altitude_as_a_measured_flight_level() {
    let c = eram_record(&eram()).unwrap();
    let v = c.values;
    // 200 kt = 102.8889 m/s -> 411.56 counts; -100 kt -> -205.78.
    assert_eq!(v.velocity, Some((412, -206)));
    assert_eq!(v.measured_fl, Some(1400));
    assert_eq!(v.baro_alt, None);
    assert_eq!(v.climb, None);
    assert_eq!(v.accuracy, None);
    assert_eq!(v.service, None);
    assert_eq!(v.ident, Some(*b"UAL123  "));
    assert!(!has(&c, NOTE_ALT_ROUNDED));
    let mut off = eram();
    off.alt_ft = Some(35_010);
    let c = eram_record(&off).unwrap();
    assert_eq!(c.values.measured_fl, Some(1400));
    assert!(has(&c, NOTE_ALT_ROUNDED));
}

// REQ: OA-CMP-002
#[test]
fn time_rounds_to_a_128th_and_the_day_is_that_of_the_rounded_time() {
    let day = days_from_civil(2026, 11, 15).unwrap();
    assert_eq!(day_time_of_ns(T0_NS).unwrap().day, day);
    // Half a count is 3.90625 ms: 3.9 ms rounds down, 3.91 ms rounds up.
    assert_eq!(
        day_time_of_ns(T0_NS + 3_900_000).unwrap().units,
        43_200 * 128
    );
    assert_eq!(
        day_time_of_ns(T0_NS + 3_910_000).unwrap().units,
        43_200 * 128 + 1
    );
    // Just before midnight rounds into the next day, at time of day 0.
    let midnight = T0_NS + 12 * 3_600 * 1_000_000_000;
    let t = day_time_of_ns(midnight - 3_000_000).unwrap();
    assert_eq!((t.day, t.units), (day + 1, 0));
    let t = day_time_of_ns(midnight - 4_000_000).unwrap();
    assert_eq!((t.day, t.units), (day, (UNITS_PER_DAY - 1) as u32));
    // Before 1970.
    let t = day_time_of_ns(-1_000_000_000).unwrap();
    assert_eq!((t.day, t.units), (-1, (UNITS_PER_DAY - 128) as u32));
    for r in [day_time_of_ns(i64::MAX), day_time_of_ns(i64::MIN)] {
        assert!(r.is_some());
    }
}

// REQ: OA-CMP-002
#[test]
fn dates_are_checked_against_the_calendar_and_round_trip() {
    assert_eq!(days_from_civil(1970, 1, 1), Some(0));
    assert_eq!(days_from_civil(2026, 11, 15), Some(20_772));
    assert!(days_from_civil(2024, 2, 29).is_some());
    assert_eq!(days_from_civil(2026, 2, 29), None);
    assert_eq!(days_from_civil(1900, 2, 29), None);
    assert!(days_from_civil(2000, 2, 29).is_some());
    assert_eq!(days_from_civil(2026, 4, 31), None);
    assert_eq!(days_from_civil(2026, 13, 1), None);
    assert_eq!(days_from_civil(2026, 0, 1), None);
    assert_eq!(days_from_civil(2026, 1, 0), None);
    assert_eq!(days_from_civil(0, 1, 1), None);
    for d in (-719_000..2_900_000).step_by(997) {
        let (y, m, dd) = civil_from_days(d).unwrap();
        assert_eq!(days_from_civil(y, m, dd), Some(d));
    }
    assert_eq!(civil_from_days(20_772), Some((2026, 11, 15)));
    assert_eq!(civil_from_days(i64::MAX), None);
}

// REQ: OA-CMP-003
#[test]
fn an_absent_or_unfit_value_leaves_its_item_out_and_is_noted_never_zeroed() {
    let mut r = row();
    r.vel_east_mps = f32::NAN;
    r.alt_ft = 1.0e9;
    r.vrate_fpm = f32::NAN;
    let c = track_record(&r).unwrap();
    assert_eq!(c.values.velocity, None);
    assert_eq!(c.values.baro_alt, None);
    assert_eq!(c.values.climb, None);
    assert!(has(&c, NOTE_VEL_ABSENT) && has(&c, NOTE_ALT_RANGE) && has(&c, NOTE_CLIMB_ABSENT));
    // No altitude flag: altitude and climb absent, not zero.
    let mut r = row();
    r.has_alt = false;
    r.alt_ft = f32::NAN;
    r.vrate_fpm = f32::NAN;
    let c = track_record(&r).unwrap();
    assert_eq!((c.values.baro_alt, c.values.climb), (None, None));
    assert!(has(&c, NOTE_ALT_ABSENT) && has(&c, NOTE_CLIMB_ABSENT));
    // Out of range velocity and climb.
    let mut r = row();
    r.vel_north_mps = 9000.0;
    r.vrate_fpm = 1.0e6;
    let c = track_record(&r).unwrap();
    assert!(has(&c, NOTE_VEL_RANGE) && has(&c, NOTE_CLIMB_RANGE));
    assert_eq!((c.values.velocity, c.values.climb), (None, None));
    // The limits of the altitude item: -15 FL and 1500 FL are in, a count beyond is out.
    for (ft, ok) in [
        (-1_500.0, true),
        (-1_526.0, false),
        (150_000.0, true),
        (150_026.0, false),
    ] {
        let mut r = row();
        r.alt_ft = ft;
        assert_eq!(
            track_record(&r).unwrap().values.baro_alt.is_some(),
            ok,
            "{ft}"
        );
    }
    // ERAM: no velocity, no altitude.
    let mut e = eram();
    e.vel_dkt = None;
    e.alt_ft = None;
    let c = eram_record(&e).unwrap();
    assert_eq!((c.values.velocity, c.values.measured_fl), (None, None));
    assert!(has(&c, NOTE_VEL_ABSENT) && has(&c, NOTE_ALT_ABSENT));
}

// REQ: OA-CMP-003
#[test]
fn a_row_with_an_impossible_position_is_refused() {
    for (la, lo) in [
        (900_000_001, 0),
        (-900_000_001, 0),
        (0, 1_800_000_001),
        (0, i32::MIN),
    ] {
        let mut r = row();
        r.lat_e7 = la;
        r.lon_e7 = lo;
        assert_eq!(track_record(&r), Err(Refusal::Position));
        let mut e = eram();
        e.lat_e7 = la;
        e.lon_e7 = lo;
        assert_eq!(eram_record(&e), Err(Refusal::Position));
    }
    let mut r = row();
    r.lat_e7 = 900_000_000;
    r.lon_e7 = -1_800_000_000;
    assert!(track_record(&r).is_ok());
    let mut e = eram();
    e.pos_ms = i64::MAX;
    assert_eq!(eram_record(&e), Err(Refusal::Time));
}

// REQ: OA-CMP-004
#[test]
fn the_apw_accuracy_follows_the_stated_conversion() {
    // At the equator: meridian radius a(1-e^2) = 6 335 439.3 m, prime vertical 6 378 137 m;
    // 1 count = pi / 2^25 rad.
    let mut r = row();
    r.lat_e7 = 0;
    r.pos_cov_m2 = [100.0, 3.0, 400.0]; // sigma east 10 m, north 20 m
    let c = track_record(&r).unwrap();
    let per_rad = 33_554_432.0 / std::f64::consts::PI;
    let want_lat = (20.0 / 6_335_439.3 * per_rad).round() as u16;
    let want_lon = (10.0 / 6_378_137.0 * per_rad).round() as u16;
    assert_eq!(c.values.accuracy, Some((want_lat, want_lon)));
    assert_eq!((want_lat, want_lon), (34, 17));
    // At 60 degrees north a metre of longitude is twice as many counts.
    r.lat_e7 = 600_000_000;
    r.pos_cov_m2 = [10_000.0, 0.0, 10_000.0];
    let (la, lo) = track_record(&r).unwrap().values.accuracy.unwrap();
    assert!(lo > la);
    let ratio = f64::from(lo) / f64::from(la);
    assert!((1.95..2.05).contains(&ratio), "{ratio}");
    // r95 is 2.4477 sigma for a circular error: the components are r95 / 2.4477 in metres.
    let r95 = (5.991_464_547 * 100.0f64).sqrt();
    assert!((r95 / 2.4477 - 10.0).abs() < 0.001);
}

// REQ: OA-CMP-004
#[test]
fn the_apw_components_are_raised_to_one_or_saturate_and_a_bad_covariance_leaves_the_item_out() {
    let mut r = row();
    r.lat_e7 = 0;
    r.pos_cov_m2 = [0.0, 0.0, 0.0];
    let c = track_record(&r).unwrap();
    assert_eq!(c.values.accuracy, Some((1, 1)));
    assert!(has(&c, NOTE_ACCURACY_MIN) && !has(&c, NOTE_ACCURACY_MAX));
    r.pos_cov_m2 = [1.0e12, 0.0, 1.0e12]; // sigma 1000 km
    let c = track_record(&r).unwrap();
    assert_eq!(c.values.accuracy, Some((u16::MAX, u16::MAX)));
    assert!(has(&c, NOTE_ACCURACY_MAX));
    // At a pole the longitude component has no meaning in metres: the maximum.
    r.lat_e7 = 900_000_000;
    r.pos_cov_m2 = [100.0, 0.0, 100.0];
    let (_, lo) = track_record(&r).unwrap().values.accuracy.unwrap();
    assert_eq!(lo, u16::MAX);
    for bad in [
        [-1.0, 0.0, 1.0],
        [1.0, 0.0, f32::NAN],
        [f32::INFINITY, 0.0, 1.0],
    ] {
        let mut r = row();
        r.pos_cov_m2 = bad;
        let c = track_record(&r).unwrap();
        assert_eq!(c.values.accuracy, None);
        assert!(has(&c, NOTE_ACCURACY_ABSENT));
    }
    // ERAM has none.
    let c = eram_record(&eram()).unwrap();
    assert_eq!(c.values.accuracy, None);
    assert!(has(&c, NOTE_ACCURACY_ABSENT));
}

// REQ: OA-CMP-005
#[test]
fn status_flags_and_the_flags_without_a_field_are_carried_or_counted() {
    let mut r = row();
    r.coast = true;
    r.radar_disagrees = true;
    r.uncertain = true;
    r.reacquired = true;
    let c = track_record(&r).unwrap();
    assert!(c.values.coasting && c.values.aff);
    assert!(has(&c, NOTE_UNCERTAIN) && has(&c, NOTE_REACQUIRED));
    let c = track_record(&row()).unwrap();
    assert!(!c.values.coasting && !c.values.aff);
    assert!(!has(&c, NOTE_UNCERTAIN) && !has(&c, NOTE_REACQUIRED));
    let mut e = eram();
    e.coast = true;
    assert!(eram_record(&e).unwrap().values.coasting);
}

// REQ: OA-CMP-006
#[test]
fn the_target_address_is_only_a_24_bit_mode_s_address() {
    for (a, ok) in [
        (Some(0xA1B2C3), true),
        (Some(0xFF_FFFF), true),
        (Some(0x100_0000 | 0x12_3456), false), // non-ICAO bit
        (Some(0x4000_0001), false),
        (None, false),
    ] {
        let mut r = row();
        r.address = a;
        let c = track_record(&r).unwrap();
        assert_eq!(c.values.address.is_some(), ok, "{a:?}");
        assert_eq!(has(&c, NOTE_ADDRESS), !ok);
    }
    // The tracker output has no callsign: never an identification.
    let c = track_record(&row()).unwrap();
    assert_eq!(c.values.ident, None);
    assert!(has(&c, NOTE_IDENT));
}

// REQ: OA-CMP-006
#[test]
fn an_eram_callsign_is_encoded_only_if_every_character_is_valid() {
    let mut e = eram();
    for (cs, want) in [
        (*b"N123AB\0\0", Some(*b"N123AB  ")),
        (*b"AB 12\0\0\0", Some(*b"AB 12   ")),
        (*b"ABCDEFGH", Some(*b"ABCDEFGH")),
        (*b"abc12\0\0\0", None),
        (*b"AB-12\0\0\0", None),
        (*b"\0\0\0\0\0\0\0\0", None),
        (*b"        ", None),
    ] {
        e.callsign = Some(cs);
        let c = eram_record(&e).unwrap();
        assert_eq!(c.values.ident, want, "{cs:?}");
        assert_eq!(has(&c, NOTE_IDENT), want.is_none());
    }
    e.callsign = None;
    assert!(has(&eram_record(&e).unwrap(), NOTE_IDENT));
    e.address = Some(0x100_0000);
    assert!(has(&eram_record(&e).unwrap(), NOTE_ADDRESS));
}

// REQ: OA-CMP-006
#[test]
fn mode_3a_ages_and_movement_are_omitted_on_every_row_and_counted() {
    let mut counts = [0u64; NOTES];
    for c in [track_record(&row()).unwrap(), eram_record(&eram()).unwrap()] {
        count_notes(&mut counts, c.notes);
    }
    for n in [NOTE_MODE3A, NOTE_AGES, NOTE_MOVEMENT] {
        assert_eq!(counts[n], 2, "{}", NOTE_NAMES[n]);
    }
    assert_eq!(counts[NOTE_SERVICE], 1);
    assert_eq!(NOTE_NAMES.len(), NOTES);
}

// REQ: OA-CMP-003
#[test]
fn a_longitude_of_plus_180_is_written_as_minus_180_and_noted() {
    for lon in [1_800_000_000, 1_799_999_990] {
        let mut r = row();
        r.lon_e7 = lon;
        let c = track_record(&r).unwrap();
        assert_eq!(c.values.lon, -(1 << 25), "{lon}");
        assert!(has(&c, NOTE_LON_WRAPPED));
        let mut e = eram();
        e.lon_e7 = lon;
        let c = eram_record(&e).unwrap();
        assert_eq!(c.values.lon, -(1 << 25));
        assert!(has(&c, NOTE_LON_WRAPPED));
    }
    let mut r = row();
    r.lon_e7 = -1_800_000_000;
    let c = track_record(&r).unwrap();
    assert_eq!(c.values.lon, -(1 << 25));
    assert!(!has(&c, NOTE_LON_WRAPPED));
    r.lon_e7 = 1_799_000_000;
    assert!(!has(&track_record(&r).unwrap(), NOTE_LON_WRAPPED));
    assert!(track_record(&r).unwrap().values.lon < (1 << 25));
}

// REQ: OA-CMP-003
#[test]
fn a_valid_rate_of_climb_is_exported_without_an_altitude_and_counted() {
    let mut r = row();
    r.has_alt = false;
    r.alt_ft = f32::NAN;
    r.vrate_fpm = 625.0;
    let c = track_record(&r).unwrap();
    assert_eq!(c.values.climb, Some(100));
    assert_eq!(c.values.baro_alt, None);
    assert!(has(&c, NOTE_CLIMB_NO_ALT) && has(&c, NOTE_ALT_ABSENT) && !has(&c, NOTE_CLIMB_ABSENT));
    r.vrate_fpm = f32::NAN;
    let c = track_record(&r).unwrap();
    assert!(has(&c, NOTE_CLIMB_ABSENT) && !has(&c, NOTE_CLIMB_NO_ALT));
    assert!(track_record(&row()).unwrap().values.monosensor);
}

// REQ: OA-CMP-001
#[test]
fn eram_altitude_is_a_flight_level_from_18000_ft_and_qnh_corrected_below() {
    for (ft, fl, qnh) in [
        (18_000, true, false),
        (35_000, true, false),
        (17_975, false, true),
        (5_000, false, true),
    ] {
        let mut e = eram();
        e.alt_ft = Some(ft);
        let c = eram_record(&e).unwrap();
        assert_eq!(c.values.measured_fl.is_some(), fl, "{ft}");
        assert_eq!(c.values.baro_alt.is_some(), qnh, "{ft}");
        assert_eq!(c.values.baro_qnh, qnh);
        assert_eq!(has(&c, NOTE_ALT_QNH), qnh);
        assert!(!c.values.monosensor);
    }
}

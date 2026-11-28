#![forbid(unsafe_code)]
//! Selected allocation windows for CA, SUA, final-monitor and ARV real core entry points.

use crate::allocator::{self, Counts};

fn count(value: bool) -> u64 {
    if value {
        1
    } else {
        0
    }
}

struct TrackColumns<'a> {
    addresses: &'a [u32],
    east_m: &'a [f64],
    north_m: &'a [f64],
    velocity_east_mps: &'a [f64],
    velocity_north_mps: &'a [f64],
    r95_m: &'a [f64],
    lat_e7: &'a [i32],
    lon_e7: &'a [i32],
}

fn predicted_tracks<'a>(input: TrackColumns<'a>) -> oa_runway::final_monitor::PredictedTracks<'a> {
    oa_runway::final_monitor::PredictedTracks {
        addresses: input.addresses,
        east_m: input.east_m,
        north_m: input.north_m,
        velocity_east_mps: input.velocity_east_mps,
        velocity_north_mps: input.velocity_north_mps,
        r95_m: input.r95_m,
        lat_e7: input.lat_e7,
        lon_e7: input.lon_e7,
    }
}

struct ArvFlightColumns<'a> {
    identity: oa_runway::arv::FlightIdentityColumns<'a>,
    assigned_path_id: &'a [u32],
    predicted_east_m: &'a [f64],
    predicted_north_m: &'a [f64],
    track_east: &'a [f64],
    track_north: &'a [f64],
    go_around_authenticated: &'a [u8],
    prior_status: &'a [u8],
}

fn arv_flights<'a>(input: ArvFlightColumns<'a>) -> oa_runway::arv::PredictedFlights<'a> {
    oa_runway::arv::PredictedFlights {
        identity: input.identity,
        assigned_path_id: input.assigned_path_id,
        predicted_east_m: input.predicted_east_m,
        predicted_north_m: input.predicted_north_m,
        track_east: input.track_east,
        track_north: input.track_north,
        go_around_authenticated: input.go_around_authenticated,
        prior_status: input.prior_status,
    }
}

fn arv_identity<'a>(
    gufi: &'a [[u8; 16]],
    aircraft_address: &'a [u32],
    centre: &'a [[u8; 4]],
    plan_id: &'a [[u8; 12]],
) -> oa_runway::arv::FlightIdentityColumns<'a> {
    oa_runway::arv::FlightIdentityColumns {
        gufi,
        aircraft_address,
        centre,
        plan_id,
    }
}

fn arv_paths<'a>(
    ids: &'a [u32],
    kinds: &'a [u8],
    threshold_east_m: &'a [f64],
    threshold_north_m: &'a [f64],
    inbound_east: &'a [f64],
    inbound_north: &'a [f64],
    width_m: &'a [f64],
) -> oa_runway::arv::ArvPaths<'a> {
    oa_runway::arv::ArvPaths {
        ids,
        kinds,
        threshold_east_m,
        threshold_north_m,
        inbound_east,
        inbound_north,
        width_m,
    }
}

fn arv_output<'a>(
    gufi: &'a mut [[u8; 16]],
    aircraft_address: &'a mut [u32],
    centre: &'a mut [[u8; 4]],
    plan_id: &'a mut [[u8; 12]],
    status: &'a mut [u8],
    matched_path_id: &'a mut [u32],
) -> oa_runway::arv::ArvOutput<'a> {
    oa_runway::arv::ArvOutput {
        gufi,
        aircraft_address,
        centre,
        plan_id,
        status,
        matched_path_id,
    }
}

#[derive(Clone, Copy)]
pub struct WindowEvidence {
    pub counts: Counts,
    pub normal: u64,
    pub degraded: u64,
    pub overload: u64,
    pub injected: bool,
}

/// Exercise CA cycle and shared-predictor probe. All vectors/configuration are built before arm.
pub fn ca_window(inject: bool) -> Result<WindowEvidence, &'static str> {
    use oa_ca::{
        alert_log_new, ca_cycle, ca_new, event_log_new, Area, CaConfig, CaTrack, Volume,
        MAX_VOLUMES,
    };
    use oa_safety::net::state_table_new;

    let volume = Volume {
        area: Area::Box {
            lat_min_e7: -900_000_000,
            lat_max_e7: 900_000_000,
            lon_min_e7: -1_800_000_000,
            lon_max_e7: 1_800_000_000,
        },
        alt_lo_ft: -100_000,
        alt_hi_ft: 100_000,
        h_m: 5_000.0,
        v_ft: 300.0,
    };
    let config = CaConfig {
        max_tracks: 4,
        max_open: 8,
        max_candidates: 8,
        look_ahead_ns: 120_000_000_000,
        hysteresis_ns: 2_000_000_000,
        work_budget: 10_000,
        eval_budget: 1,
        max_speed_mps: 300.0,
        r95_cap_m: 5_000.0,
        grid_band_m: 1_000.0,
        default_volume: volume,
        volumes: [volume; MAX_VOLUMES],
        n_volumes: 0,
    };
    let mut state = ca_new(&config)?;
    let mut alerts = alert_log_new(16)?;
    let mut events = event_log_new(16)?;
    let mut net = state_table_new(600_000_000_000, 1_000_000_000).map_err(|_| "CA state table")?;
    let rows = [
        CaTrack {
            lat_deg: 41.0,
            lon_deg: -88.0,
            addr: 0x10001,
            vel_east_mps: 100.0,
            vel_north_mps: 0.0,
            r95_m: 10.0,
            alt_ft: 10_000.0,
            vrate_fpm: 0.0,
            flags: 0,
            ground: oa_ca::AIRBORNE,
        },
        CaTrack {
            lat_deg: 41.0,
            lon_deg: -87.99,
            addr: 0x10002,
            vel_east_mps: -100.0,
            vel_north_mps: 0.0,
            r95_m: 10.0,
            alt_ft: 10_000.0,
            vrate_fpm: 0.0,
            flags: 0,
            ground: oa_ca::AIRBORNE,
        },
        CaTrack {
            lat_deg: 41.1,
            lon_deg: -88.0,
            addr: 0x10003,
            vel_east_mps: 100.0,
            vel_north_mps: 0.0,
            r95_m: 10.0,
            alt_ft: 10_000.0,
            vrate_fpm: 0.0,
            flags: 0,
            ground: oa_ca::AIRBORNE,
        },
        CaTrack {
            lat_deg: 41.1,
            lon_deg: -87.99,
            addr: 0x10004,
            vel_east_mps: -100.0,
            vel_north_mps: 0.0,
            r95_m: 10.0,
            alt_ft: 10_000.0,
            vrate_fpm: 0.0,
            flags: 0,
            ground: oa_ca::AIRBORNE,
        },
    ];
    let mut a = oa_ca::probe::ProbeTrajectory {
        gufi: [1; 16],
        aircraft_address: 0x10001,
        centre: *b"ORD1",
        plan_id: *b"PLAN00000001",
        reserved_key_align: [0; 4],
        state_ms: 0,
        sample_count: 41,
        reserved_sample_count: [0; 6],
        time_ms: [0; oa_ca::probe::PROBE_MAX_SAMPLES],
        lat_e7: [410_000_000; oa_ca::probe::PROBE_MAX_SAMPLES],
        lon_e7: [0; oa_ca::probe::PROBE_MAX_SAMPLES],
        altitude_ft: [10_000; oa_ca::probe::PROBE_MAX_SAMPLES],
    };
    let mut b = a;
    b.gufi = [2; 16];
    b.aircraft_address = 0x10002;
    for i in 0..41 {
        let offset = i64::try_from(i).map_err(|_| "probe index overflow")? * 30_000;
        if let Some(t) = a.time_ms.get_mut(i) {
            *t = offset;
        }
        if let Some(t) = b.time_ms.get_mut(i) {
            *t = offset;
        }
        if let Some(lon) = a.lon_e7.get_mut(i) {
            *lon = -880_000_000 + i as i32 * 500;
        }
        if let Some(lon) = b.lon_e7.get_mut(i) {
            *lon = -879_995_000 - i as i32 * 500;
        }
    }
    let warm = ca_cycle(&mut state, 0, &rows, &mut alerts, &mut events, &mut net);
    let probe_warm = oa_ca::probe::probe_pair(&a, &b);
    core::hint::black_box((warm, probe_warm));
    if !allocator::start() {
        return Err("CA allocation window did not arm");
    }
    let normal = ca_cycle(
        &mut state,
        1_000_000_000,
        &rows,
        &mut alerts,
        &mut events,
        &mut net,
    );
    let probe = oa_ca::probe::probe_pair(&a, &b);
    let Some(first_row) = rows.first().copied() else {
        return Err("CA rows missing");
    };
    let mut too_many = [first_row; 5];
    for (index, row) in too_many.iter_mut().enumerate() {
        if let Ok(offset) = u32::try_from(index) {
            row.addr = 0x20001_u32.saturating_add(offset);
        }
    }
    let overloaded = ca_cycle(
        &mut state,
        2_000_000_000,
        &too_many,
        &mut alerts,
        &mut events,
        &mut net,
    );
    let observed = count(normal.stats.cycles > 0) + count(probe.examined_segments > 0);
    let degraded = count(normal.stats.not_evaluated > 0);
    let overload = count(overloaded.stats.rows_over_capacity > 0);
    if inject && !allocator::inject_core_allocation() {
        return Err("CA injection allocation failed");
    }
    let counts = allocator::stop().ok_or("CA allocation window did not disarm")?;
    let observed = observed.saturating_add(degraded).saturating_add(overload);
    if observed < 3
        || normal.stats.evaluated == 0
        || degraded == 0
        || overload == 0
        || probe.refusal != oa_ca::probe::ProbeRefusal::None
        || probe.status != oa_ca::probe::ProbeStatus::Loss
    {
        eprintln!(
            "synthetic CA fixture: cycles={} candidates={} evaluated={} not_evaluated={} overloaded_rows={} probe_status={:?} probe_refusal={:?} probe_segments={} first_loss_ms={}",
            normal.stats.cycles,
            normal.stats.candidates,
            normal.stats.evaluated,
            normal.stats.not_evaluated,
            overloaded.stats.rows_over_capacity,
            probe.status,
            probe.refusal,
            probe.examined_segments,
            probe.first_loss_ms,
        );
        return Err(
            "CA cycle/probe fixture failed to reach normal, degraded and overload outcomes",
        );
    }
    Ok(WindowEvidence {
        counts,
        normal: 2,
        degraded,
        overload,
        injected: inject,
    })
}

/// Exercise the real active SUA query over a synthetic polygon and bounded-input refusal.
pub fn sua_window(inject: bool) -> Result<WindowEvidence, &'static str> {
    use oa_airspace::core::conflict_probe::{probe_sua, SuaStatus, SuaTrajectoryKnots, SUA_KNOTS};
    use oa_airspace::core::geom::{build_area, latlon_e7};
    let ring = vec![
        latlon_e7(419_000_000, -879_000_000).map_err(|_| "SUA point")?,
        latlon_e7(419_000_000, -878_000_000).map_err(|_| "SUA point")?,
        latlon_e7(420_000_000, -878_000_000).map_err(|_| "SUA point")?,
        latlon_e7(420_000_000, -879_000_000).map_err(|_| "SUA point")?,
    ];
    let area = build_area(vec![ring], 0.0).map_err(|_| "synthetic SUA geometry did not build")?;
    let mut trajectory = SuaTrajectoryKnots {
        state_ms: 0,
        sample_count: 41,
        reserved_sample_count: [0; 6],
        time_ms: [0; SUA_KNOTS],
        lat_e7: [419_500_000; SUA_KNOTS],
        lon_e7: [0; SUA_KNOTS],
        altitude_msl_ft: [15_000; SUA_KNOTS],
    };
    for i in 0..41 {
        if let Some(t) = trajectory.time_ms.get_mut(i) {
            *t = i as i64 * 30_000;
        }
        if let Some(lon) = trajectory.lon_e7.get_mut(i) {
            *lon = -880_000_000 + i as i32 * 1_000;
        }
    }
    let warm = probe_sua(&trajectory, &area, true, 0, 1_200_000, 10_000, 20_000);
    core::hint::black_box(warm);
    if !allocator::start() {
        return Err("SUA allocation window did not arm");
    }
    let normal = probe_sua(&trajectory, &area, true, 0, 1_200_000, 10_000, 20_000);
    let degraded = probe_sua(&trajectory, &area, false, 0, 1_200_000, 10_000, 20_000);
    trajectory.sample_count = (SUA_KNOTS + 1) as u16;
    let overload = probe_sua(&trajectory, &area, true, 0, 1_200_000, 10_000, 20_000);
    if inject && !allocator::inject_core_allocation() {
        return Err("SUA injection allocation failed");
    }
    let counts = allocator::stop().ok_or("SUA allocation window did not disarm")?;
    let normal_count =
        count(normal.status == SuaStatus::Entry || normal.status == SuaStatus::Clear);
    let degraded_count = count(degraded.status == SuaStatus::Unavailable);
    let overload_count = count(overload.status == SuaStatus::Unavailable);
    if normal_count == 0 || degraded_count == 0 || overload_count == 0 {
        return Err("SUA probe fixture missed an expected outcome");
    }
    Ok(WindowEvidence {
        counts,
        normal: normal_count,
        degraded: degraded_count,
        overload: overload_count,
        injected: inject,
    })
}

/// Exercise final-monitor and ARV production APIs with caller-owned buffers.
pub fn runway_window(inject: bool) -> Result<WindowEvidence, &'static str> {
    use oa_runway::arv::{self, AlertKind};
    use oa_runway::final_monitor::{self, FinalAlertBuffer, FinalCourseInput, FinalMonitorState};
    use oa_runway::plane::Local;
    let first = FinalCourseInput {
        runway_id: 1,
        end_name: *b"27L",
        outer_fix: Local {
            east_m: -3_000.0,
            north_m: -5_000.0,
        },
        threshold: Local {
            east_m: -3_000.0,
            north_m: 5_000.0,
        },
    };
    let second = FinalCourseInput {
        runway_id: 2,
        end_name: *b"27R",
        outer_fix: Local {
            east_m: 3_000.0,
            north_m: -5_000.0,
        },
        threshold: Local {
            east_m: 3_000.0,
            north_m: 5_000.0,
        },
    };
    let pair = final_monitor::prepare_pair(first, second, final_monitor::DEFAULT_CONFIG)
        .map_err(|_| "final monitor geometry refused")?;
    let addresses = [0x10001_u32];
    let east = [0.0];
    let north = [0.0];
    let ve = [100.0];
    let vn = [0.0];
    let radius = [10.0];
    let lat = [419_000_000];
    let lon = [-879_000_000];
    let mut oa = [0_u32; 2];
    let mut op = [pair.id; 2];
    let mut os = [final_monitor::FinalSeverity::Caution; 2];
    let mut olat = [0_i32; 2];
    let mut olon = [0_i32; 2];
    let mut otime = [0_u32; 2];
    let mut state = FinalMonitorState::new();
    let warm = final_monitor::update(
        pair,
        &mut state,
        1_200_000_000,
        1_200_000_000,
        predicted_tracks(TrackColumns {
            addresses: &addresses,
            east_m: &east,
            north_m: &north,
            velocity_east_mps: &ve,
            velocity_north_mps: &vn,
            r95_m: &radius,
            lat_e7: &lat,
            lon_e7: &lon,
        }),
        FinalAlertBuffer {
            addresses: &mut oa,
            pair_ids: &mut op,
            severity: &mut os,
            lat_e7: &mut olat,
            lon_e7: &mut olon,
            seconds_to_entry_milli: &mut otime,
        },
    )
    .map_err(|_| "final monitor warmup refused")?;
    core::hint::black_box(warm);
    let gufi = [[0xA5; 16]];
    let ac = [0x10001_u32];
    let centre = [*b"ORD1"];
    let plan = [*b"PLAN00000001"];
    let assigned = [10_u32];
    let pe = [300.0];
    let pn = [-500.0];
    let te = [0.0];
    let tn = [1.0];
    let auth = [0_u8];
    let prior = [AlertKind::Clear as u8];
    let ids = [10_u32, 20_u32];
    let kinds = [arv::PathKind::Runway as u8; 2];
    let eastp = [0.0, 300.0];
    let northp = [0.0, 0.0];
    let ax = [0.0, 0.0];
    let ay = [1.0, 1.0];
    let widths = [30.0, 30.0];
    let mut og = [[0_u8; 16]];
    let mut oac = [0_u32];
    let mut oc = [[0_u8; 4]];
    let mut oplan = [[0_u8; 12]];
    let mut ost = [0_u8];
    let mut om = [0_u32];
    let arv_warm = arv::evaluate(
        arv::DEFAULT_CONFIG,
        arv_flights(ArvFlightColumns {
            identity: arv_identity(&gufi, &ac, &centre, &plan),
            assigned_path_id: &assigned,
            predicted_east_m: &pe,
            predicted_north_m: &pn,
            track_east: &te,
            track_north: &tn,
            go_around_authenticated: &auth,
            prior_status: &prior,
        }),
        arv_paths(&ids, &kinds, &eastp, &northp, &ax, &ay, &widths),
        arv_output(&mut og, &mut oac, &mut oc, &mut oplan, &mut ost, &mut om),
    )
    .map_err(|_| "ARV warmup refused")?;
    core::hint::black_box(arv_warm);
    if !allocator::start() {
        return Err("runway allocation window did not arm");
    }
    let normal = final_monitor::update(
        pair,
        &mut state,
        2_400_000_000,
        2_400_000_000,
        predicted_tracks(TrackColumns {
            addresses: &addresses,
            east_m: &east,
            north_m: &north,
            velocity_east_mps: &ve,
            velocity_north_mps: &vn,
            r95_m: &radius,
            lat_e7: &lat,
            lon_e7: &lon,
        }),
        FinalAlertBuffer {
            addresses: &mut oa,
            pair_ids: &mut op,
            severity: &mut os,
            lat_e7: &mut olat,
            lon_e7: &mut olon,
            seconds_to_entry_milli: &mut otime,
        },
    );
    let degraded = final_monitor::update(
        pair,
        &mut state,
        7_200_000_001,
        2_400_000_000,
        predicted_tracks(TrackColumns {
            addresses: &addresses,
            east_m: &east,
            north_m: &north,
            velocity_east_mps: &ve,
            velocity_north_mps: &vn,
            r95_m: &radius,
            lat_e7: &lat,
            lon_e7: &lon,
        }),
        FinalAlertBuffer {
            addresses: &mut oa,
            pair_ids: &mut op,
            severity: &mut os,
            lat_e7: &mut olat,
            lon_e7: &mut olon,
            seconds_to_entry_milli: &mut otime,
        },
    );
    let overload = final_monitor::update(
        pair,
        &mut state,
        8_400_000_001,
        8_400_000_001,
        predicted_tracks(TrackColumns {
            addresses: &addresses,
            east_m: &east,
            north_m: &north,
            velocity_east_mps: &ve,
            velocity_north_mps: &vn,
            r95_m: &radius,
            lat_e7: &lat,
            lon_e7: &lon,
        }),
        FinalAlertBuffer {
            addresses: &mut [],
            pair_ids: &mut [],
            severity: &mut [],
            lat_e7: &mut [],
            lon_e7: &mut [],
            seconds_to_entry_milli: &mut [],
        },
    );
    let arv_result = arv::evaluate(
        arv::DEFAULT_CONFIG,
        arv_flights(ArvFlightColumns {
            identity: arv_identity(&gufi, &ac, &centre, &plan),
            assigned_path_id: &assigned,
            predicted_east_m: &pe,
            predicted_north_m: &pn,
            track_east: &te,
            track_north: &tn,
            go_around_authenticated: &auth,
            prior_status: &prior,
        }),
        arv_paths(&ids, &kinds, &eastp, &northp, &ax, &ay, &widths),
        arv_output(&mut og, &mut oac, &mut oc, &mut oplan, &mut ost, &mut om),
    );
    let arv_overload = arv::evaluate(
        arv::DEFAULT_CONFIG,
        arv_flights(ArvFlightColumns {
            identity: arv_identity(&gufi, &ac, &centre, &plan),
            assigned_path_id: &assigned,
            predicted_east_m: &pe,
            predicted_north_m: &pn,
            track_east: &te,
            track_north: &tn,
            go_around_authenticated: &auth,
            prior_status: &prior,
        }),
        arv_paths(&ids, &kinds, &eastp, &northp, &ax, &ay, &widths),
        oa_runway::arv::ArvOutput {
            gufi: &mut [],
            aircraft_address: &mut [],
            centre: &mut [],
            plan_id: &mut [],
            status: &mut [],
            matched_path_id: &mut [],
        },
    );
    if inject && !allocator::inject_core_allocation() {
        return Err("runway injection allocation failed");
    }
    let counts = allocator::stop().ok_or("runway allocation window did not disarm")?;
    let normal_count = count(normal.is_ok_and(|result| result.alerts > 0))
        + count(arv_result.is_ok_and(|result| result.alerts > 0));
    let degraded_count = count(degraded.is_ok_and(|result| result.degraded));
    let overload_count = count(overload == Err(final_monitor::UpdateError::Capacity))
        + count(arv_overload == Err(arv::ArvError::Capacity));
    if normal_count != 2 || degraded_count == 0 || overload_count != 2 {
        return Err("runway final-monitor/ARV fixture missed an expected outcome");
    }
    Ok(WindowEvidence {
        counts,
        normal: normal_count,
        degraded: degraded_count,
        overload: overload_count,
        injected: inject,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::expect_used)]
    // E3: assertions are confined to this synthetic allocation regression test.
    // REQ: OA-ASP-002
    #[test]
    fn prepared_area_contains_is_allocation_free_for_geometry_queries() {
        use oa_airspace::geom::{area_contains, build_area};
        use oa_geo::wgs84;
        use oa_geo::LatLon;

        let _guard = allocator::lock_test_measurements();
        let point = |lat, lon| LatLon::new(lat, lon).expect("synthetic coordinate");
        let ring = vec![
            point(41.0, -89.0),
            point(41.0, -87.0),
            point(43.0, -87.0),
            point(43.0, -89.0),
        ];
        let area = build_area(vec![ring], 1_000.0).expect("synthetic area");
        let ell = wgs84();
        let near_edge = ell
            // The top edge is a geodesic and bows north of its equal-latitude endpoints.
            // 1,200 m north is still within the 1,000 m edge buffer after that bow.
            .direct(point(43.0, -88.0), 0.0, 1_200.0)
            .expect("near edge")
            .position;
        let outside_buffer = ell
            // Leave ample clearance beyond both the curved edge and its buffer.
            .direct(point(43.0, -88.0), 0.0, 5_000.0)
            .expect("outside buffer")
            .position;
        let inside = point(42.0, -88.0);

        assert!(
            std::hint::black_box(area_contains(&area, near_edge)).expect("warm-up near-edge query")
        );
        assert!(!std::hint::black_box(area_contains(&area, outside_buffer))
            .expect("warm-up outside-buffer query"));
        assert!(std::hint::black_box(area_contains(&area, inside)).expect("warm-up interior query"));
        assert!(
            allocator::start(),
            "area query allocation window did not arm"
        );
        let near = std::hint::black_box(area_contains(&area, near_edge));
        let outside = std::hint::black_box(area_contains(&area, outside_buffer));
        let interior = std::hint::black_box(area_contains(&area, inside));
        let counts = allocator::stop().expect("area query window did not disarm");

        assert!(near.expect("measured near-edge query"));
        assert!(!outside.expect("measured outside-buffer query"));
        assert!(interior.expect("measured interior query"));
        assert_eq!(counts.alloc, 0);
        assert_eq!(counts.zeroed, 0);
        assert_eq!(counts.realloc, 0);
        assert!(!counts.overflow);
    }

    fn assert_zero_counts(evidence: WindowEvidence) {
        assert_eq!(evidence.counts.alloc, 0);
        assert_eq!(evidence.counts.zeroed, 0);
        assert_eq!(evidence.counts.realloc, 0);
        assert!(!evidence.counts.overflow);
        assert!(!evidence.injected);
    }

    // REQ: OA-TRK-327
    #[test]
    fn advanced_core_allocation_windows_cover_outcomes_without_allocating() {
        let _guard = allocator::lock_test_measurements();

        let ca = ca_window(false);
        assert!(ca.is_ok(), "CA fixture must complete its measured windows");
        if let Ok(evidence) = ca {
            assert!(evidence.normal > 0 && evidence.degraded > 0 && evidence.overload > 0);
            assert_zero_counts(evidence);
        }

        let sua = sua_window(false);
        assert!(
            sua.is_ok(),
            "SUA fixture must complete its measured windows"
        );
        if let Ok(evidence) = sua {
            assert!(evidence.normal > 0 && evidence.degraded > 0 && evidence.overload > 0);
            assert_zero_counts(evidence);
        }

        let runway = runway_window(false);
        assert!(
            runway.is_ok(),
            "runway fixture must complete its measured windows"
        );
        if let Ok(evidence) = runway {
            assert_eq!(evidence.normal, 2);
            assert!(evidence.degraded > 0);
            assert_eq!(evidence.overload, 2);
            assert_zero_counts(evidence);
        }
    }
}

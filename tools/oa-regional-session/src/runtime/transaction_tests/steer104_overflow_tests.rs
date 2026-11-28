use super::*;

fn overflow_profile(track_capacity: usize) -> RegionalRunProfile {
    let mut profile = DEFAULT_REGIONAL_RUN_PROFILE;
    profile.surveillance.max_tracks = track_capacity;
    profile.surveillance.max_candidate_pairs = track_capacity * (track_capacity - 1) / 2;
    profile.surveillance.max_open_alerts = track_capacity;
    profile.max_flights = track_capacity;
    profile
}

fn overflow_runtime(profile: RegionalRunProfile) -> RegionalRuntime {
    let model = synthetic_model().expect("public synthetic adaptation");
    let snapshot = oa_airspace::core::snapshot::encode(&model).expect("encode synthetic model");
    regional_new_with_profile(
        model,
        oa_sha256::sha256(&snapshot),
        *b"ZAU1",
        SurveillanceConfig {
            centre_lat_e7: 410_000_000,
            centre_lon_e7: -870_000_000,
            range_nm: 180.0,
        },
        profile,
    )
    .expect("bounded overflow fixture runtime")
}

fn overflow_runtime_with_dimensions(
    mut profile: RegionalRunProfile,
    monitor: RuntimeFinalMonitorConfig,
    centre: [u8; 4],
    frame_width: u32,
    frame_height: u32,
) -> RegionalRuntime {
    profile.surveillance.frame_width = frame_width;
    profile.surveillance.frame_height = frame_height;
    let model = synthetic_model().expect("public synthetic adaptation");
    let snapshot = oa_airspace::core::snapshot::encode(&model).expect("encode synthetic model");
    regional_new_with_runtime_final_monitor(
        model,
        oa_sha256::sha256(&snapshot),
        centre,
        SurveillanceConfig {
            centre_lat_e7: 410_000_000,
            centre_lon_e7: -870_000_000,
            range_nm: 90.0,
        },
        profile,
        monitor,
    )
    .expect("bounded composed overflow runtime")
}

fn overflow_count(rows: &[surveillance::SurveillanceTextRow], text: &str) -> usize {
    rows.iter().filter(|row| row.text == text).count()
}

fn assert_safety_row_text(rows: &[surveillance::SurveillanceTextRow], text: &str, index: usize) {
    let actual = rows
        .iter()
        .find(|row| row.text == text)
        .expect("expected safety text is present in the actual display list");
    let plan =
        oa_display::core::status_strip::regional_status_plan().expect("shared status-row geometry");
    let placement = plan
        .row(oa_display::core::status_strip::StatusStripRow::Safety(
            index,
        ))
        .expect("requested safety row exists");
    let planned = placement
        .bounds_for(text)
        .expect("safety text fits planned row");
    assert!(
        actual.bounds.x0 == planned.x0
            && actual.bounds.x1 == planned.x1
            && actual.bounds.y0 == planned.y0
            && actual.bounds.y1 <= planned.y1,
        "safety text bounds {:?} do not fit row {} bounds {:?}",
        actual.bounds,
        index,
        planned
    );
}

fn spatial_low_altitude_batch(receipt_ns: i64, first_address: u32, count: usize) -> ReceiptBatch {
    const LOW_ADDRESS_ORIGIN: u32 = 0x00ab_c200;
    let mut events = Vec::with_capacity(count);
    for index in 0..count {
        let address = first_address + u32::try_from(index).expect("bounded address count");
        let ordinal = usize::try_from(address - LOW_ADDRESS_ORIGIN).expect("fixture address base");
        let latitude_e7 = 410_000_000
            + i32::try_from(ordinal / 8).expect("bounded fixture latitude") * 700_000
            - 5_600_000;
        let longitude_e7 =
            -869_000_000 + i32::try_from(ordinal % 8).expect("bounded fixture longitude") * 900_000;
        let callsign = format!("N{ordinal:05}");
        let line = format!(
            "v1\t{receipt_ns}\t{receipt_ns}\t{}\tSURVEILLANCE\t00000000000000000000000000000000\t{address:06x}\tZAU1\tPOS\t{latitude_e7}\t{longitude_e7}\t100\t0\t0\t0\tCALLSIGN\t{callsign}",
            index + 1
        );
        events.push(parse_event_line(&line).expect("valid spatial synthetic position"));
    }
    ReceiptBatch {
        receipt_ns,
        source_clock_regressions: 0,
        events,
    }
}

fn composed_final_alerts(
    source: RegionalFinalMonitorAlert,
    count: usize,
) -> Vec<RegionalFinalMonitorAlert> {
    let mut alerts = Vec::with_capacity(count);
    for index in 0..count {
        let mut alert = source;
        let ordinal = index + 1;
        alert.address = 0x00ad_0000_u32
            .checked_add(u32::try_from(index).expect("bounded final alert index"))
            .expect("bounded final alert address");
        // Final markers occupy the western synthetic scene; MSAW targets use the eastern scene.
        // These coordinates are assigned before rendering, not moved to hide a collision.
        alert.lat_e7 = 410_000_000
            + i32::try_from(ordinal / 8).expect("bounded final latitude") * 700_000
            - 5_600_000;
        alert.lon_e7 =
            -877_000_000 + i32::try_from(ordinal % 8).expect("bounded final longitude") * 900_000;
        alert.severity = if index + 1 == count {
            final_monitor_core::FinalSeverity::Warning
        } else {
            final_monitor_core::FinalSeverity::Caution
        };
        alerts.push(alert);
    }
    alerts
}

// Mutation killed: derive MSAW omission only from retained rows, losing the new warning refused
// by the valid 128-entry open table. This exercises the ordinary provider and runtime presenter.
// REQ: OA-REG-011, OA-DSP-078
#[test]
fn valid_open_table_limit_discloses_one_refused_warning_and_keeps_notice_next_frame() {
    const TRACKS: usize = 129;
    const OPEN_LIMIT: usize = 128;

    let mut profile = overflow_profile(TRACKS);
    profile.surveillance.max_open_alerts = OPEN_LIMIT + MAX_FINAL_APPROACH_ASSIGNMENTS;
    let mut runtime = overflow_runtime(profile);
    let service = crate::msaw_bridge::regional_msaw_synthetic_with_reported_open_limit_for_test(
        TRACKS,
        TRACKS,
        OPEN_LIMIT,
        OPEN_LIMIT,
        profile.surveillance.max_frames,
    )
    .expect("synthetic regional service uses a 128-entry core and reported open table");
    assert_eq!(
        crate::msaw_bridge::regional_msaw_row_limit(&service),
        TRACKS
    );
    assert_eq!(
        crate::msaw_bridge::regional_msaw_alert_limit(&service),
        OPEN_LIMIT
    );
    regional_attach_msaw(&mut runtime, service).expect("valid open-table limit is admitted");

    // The ordinary synthetic service runs the real MSAW plan/cycle over its declared 500-ft
    // terrain and explicitly covered empty obstacle index; it does not inject a warning packet.
    for scan in 1_i64..=4 {
        regional_step(
            &mut runtime,
            &low_altitude_population_batch_with_address_range(
                scan * surveillance::SECOND_NS,
                0x00ab_c101,
                OPEN_LIMIT,
            ),
        )
        .expect("128 synthetic positions complete the normal MSAW frame");
    }
    let mut latest = None;
    for scan in 5_i64..=7 {
        latest = Some(
            regional_step(
                &mut runtime,
                &low_altitude_population_batch_with_address_range(
                    scan * surveillance::SECOND_NS,
                    0x00ab_c100,
                    TRACKS,
                ),
            )
            .expect("129 synthetic positions complete the normal MSAW frame"),
        );
    }

    let overflow = latest.expect("overflow scans produce a completed frame");
    let msaw = overflow
        .surveillance
        .msaw
        .as_ref()
        .expect("ordinary MSAW provider remains attached");
    assert!(
        msaw.synthetic,
        "the fixture provenance is explicitly synthetic"
    );
    assert!(msaw.failure.is_empty());
    assert!(msaw.evaluation_attempted);
    assert!(msaw.planned_points > 0);
    assert_eq!(msaw.completed_floor_samples, msaw.planned_points);
    assert_eq!(msaw.open.len(), OPEN_LIMIT);
    assert_eq!(msaw.open_table_refused, 1);
    assert_eq!(msaw.open_table_refused_caution, 0);
    assert_eq!(msaw.open_table_refused_warning, 1);
    let core_stats = msaw
        .stats
        .expect("completed MSAW cycle reports core counts");
    assert_eq!(core_stats.open_full, 1);
    assert_eq!(core_stats.open_full_caution, 0);
    assert_eq!(core_stats.open_full_warning, 1);
    assert_eq!(core_stats.alerts_dropped, 0);
    assert_eq!(msaw.closed.len(), 1, "the refused warning is logged once");
    assert_eq!(
        msaw.report,
        oa_safety::net::NetReport::Degraded {
            reason: oa_safety::net::Reason::BudgetExceeded,
            area: oa_safety::net::Area::Whole,
            count: 1,
        }
    );
    let marker_stats = overflow
        .surveillance
        .msaw_markers
        .expect("runtime consumer publishes marker accounting");
    assert_eq!(marker_stats.examined, OPEN_LIMIT as u64);
    assert_eq!(marker_stats.drawn, OPEN_LIMIT as u64);
    assert_eq!(marker_stats.omitted, 1);
    let rows = surveillance::surveillance_text_rows(&runtime.surveillance)
        .expect("actual runtime display-list text rows");
    assert_eq!(overflow_count(&rows, "MSAW +1 NOT SHOWN (1 WARNING)"), 1);
    assert!(overflow
        .surveillance
        .warnings
        .iter()
        .any(|warning| warning == "MSAW PARTIAL"));
    assert!(overflow.surveillance.raster_rendered);
    super::overflow_pixels::assert_notice_isolated(
        &runtime,
        "MSAW +1 NOT SHOWN (1 WARNING)",
        oa_display::core::status_strip::StatusStripRow::MsawOverflow,
    );

    let next = regional_step(
        &mut runtime,
        &low_altitude_population_batch_with_address_range(
            8 * surveillance::SECOND_NS,
            0x00ab_c100,
            TRACKS,
        ),
    )
    .expect("valid open-table overflow does not prevent the next frame");
    assert_eq!(next.surveillance.msaw_markers.unwrap().omitted, 1);
    let next_rows = surveillance::surveillance_text_rows(&runtime.surveillance)
        .expect("next actual display-list text rows");
    assert_eq!(
        overflow_count(&next_rows, "MSAW +1 NOT SHOWN (1 WARNING)"),
        1
    );
    assert!(next.surveillance.raster_rendered);
    super::overflow_pixels::assert_notice_isolated(
        &runtime,
        "MSAW +1 NOT SHOWN (1 WARNING)",
        oa_display::core::status_strip::StatusStripRow::MsawOverflow,
    );
}

// Mutation killed: flattening severity at the bridge makes a refused warning indistinguishable
// from a caution and omits the required warning-qualified consumer notice.
// REQ: OA-REG-011, OA-DSP-078
#[test]
fn mixed_grade_open_table_refusal_is_disclosed_by_runtime_notice() {
    const TRACKS: usize = 129;
    const OPEN_LIMIT: usize = 128;
    let mut profile = overflow_profile(TRACKS);
    profile.surveillance.max_open_alerts = OPEN_LIMIT + MAX_FINAL_APPROACH_ASSIGNMENTS;
    let mut runtime = overflow_runtime(profile);
    let service = crate::msaw_bridge::regional_msaw_synthetic_with_reported_open_limit_for_test(
        TRACKS,
        TRACKS,
        OPEN_LIMIT,
        OPEN_LIMIT,
        profile.surveillance.max_frames,
    )
    .expect("synthetic provider uses the configured retained-alert limit");
    regional_attach_msaw(&mut runtime, service).expect("valid MSAW capacity is admitted");

    let mixed_batch = |receipt_ns: i64, include_warning: bool| {
        let count = if include_warning { TRACKS } else { OPEN_LIMIT };
        let mut batch =
            low_altitude_population_batch_with_address_range(receipt_ns, 0x00ab_c101, count);
        for (index, event) in batch.events.iter_mut().enumerate() {
            if let crate::EventKind::Position {
                altitude_ft,
                vertical_rate_fpm,
                ..
            } = &mut event.kind
            {
                *altitude_ft = Some(if index < OPEN_LIMIT { 1500 } else { 1000 });
                *vertical_rate_fpm = Some(-600);
            }
        }
        batch
    };

    for scan in 1_i64..=4 {
        regional_step(
            &mut runtime,
            &mixed_batch(scan * surveillance::SECOND_NS, false),
        )
        .expect("128 caution tracks open normally");
    }
    let mut latest = None;
    for scan in 5_i64..=7 {
        latest = Some(
            regional_step(
                &mut runtime,
                &mixed_batch(scan * surveillance::SECOND_NS, true),
            )
            .expect("new warning is evaluated behind retained cautions"),
        );
    }
    let frame = latest.expect("the configured scans produce a runtime frame");
    let msaw = frame.surveillance.msaw.as_ref().expect("MSAW frame exists");
    assert_eq!(msaw.open.len(), OPEN_LIMIT);
    assert_eq!(msaw.open_table_refused_caution, 0);
    assert_eq!(msaw.open_table_refused_warning, 1);
    assert_eq!(msaw.open_table_refused, 1);
    let rows = surveillance::surveillance_text_rows(&runtime.surveillance)
        .expect("actual runtime status text is available");
    assert_eq!(overflow_count(&rows, "MSAW +1 NOT SHOWN (1 WARNING)"), 1);
    assert!(frame.surveillance.raster_rendered);

    let next = regional_step(
        &mut runtime,
        &mixed_batch(8 * surveillance::SECOND_NS, true),
    )
    .expect("the next evaluated frame retains the current-cycle refusal evidence");
    assert_eq!(next.surveillance.msaw_markers.unwrap().omitted, 1);
    let next_rows = surveillance::surveillance_text_rows(&runtime.surveillance)
        .expect("next-frame runtime status text is available");
    assert_eq!(
        overflow_count(&next_rows, "MSAW +1 NOT SHOWN (1 WARNING)"),
        1
    );
}

fn composed_overflow_runtime(frame_height: u32) -> RegionalRuntime {
    const TRACKS: usize = 130;
    const OPEN_LIMIT: usize = 128;
    const FIRST_LOW_ADDRESS: u32 = 0x00ab_c200;

    // The profile reserves both complete output families. The attached provider still uses its
    // real 128-entry open table and receives 129 live violating reports.
    let profile = overflow_profile(OPEN_LIMIT + MAX_FINAL_APPROACH_ASSIGNMENTS);
    let mut runtime = overflow_runtime_with_dimensions(
        profile,
        final_monitor_config(100.0),
        *b"ZAU1",
        2_400,
        frame_height,
    );
    let service = crate::msaw_bridge::regional_msaw_synthetic_with_reported_open_limit_for_test(
        TRACKS,
        TRACKS,
        OPEN_LIMIT,
        OPEN_LIMIT,
        profile.surveillance.max_frames,
    )
    .expect("synthetic service has a real 128-entry open table");
    assert_eq!(
        crate::msaw_bridge::regional_msaw_row_limit(&service),
        TRACKS
    );
    assert_eq!(
        crate::msaw_bridge::regional_msaw_alert_limit(&service),
        OPEN_LIMIT
    );
    regional_attach_msaw(&mut runtime, service).expect("valid open-table limit is admitted");

    let assigned = seed_confirmed_final_monitor_track(&mut runtime, 410_000_000, 0);
    assert_eq!(assigned.final_monitor.status, FinalMonitorStatus::Available);
    let source_alert = assigned
        .final_monitor
        .alerts
        .first()
        .copied()
        .expect("assigned flight produces an actual final-monitor alert");

    for scan in 5_i64..=8 {
        let batch =
            spatial_low_altitude_batch(scan * surveillance::SECOND_NS, FIRST_LOW_ADDRESS + 1, 128);
        regional_step(&mut runtime, &batch)
            .expect("128 synthetic positions complete the normal provider cycle");
    }
    let mut overflow = None;
    for scan in 9_i64..=11 {
        let batch = spatial_low_altitude_batch(
            scan * surveillance::SECOND_NS,
            FIRST_LOW_ADDRESS,
            TRACKS - 1,
        );
        overflow = Some(
            regional_step(&mut runtime, &batch)
                .expect("129 synthetic positions complete the normal provider cycle"),
        );
    }
    let overflow = overflow.expect("overflow scans produce a complete runtime frame");
    let msaw = overflow
        .surveillance
        .msaw
        .as_ref()
        .expect("actual attached MSAW provider output");
    assert!(msaw.synthetic);
    assert!(msaw.evaluation_attempted);
    assert_eq!(msaw.open.len(), OPEN_LIMIT);
    assert_eq!(msaw.open_table_refused, 1);
    assert_eq!(msaw.open_table_refused_caution, 0);
    assert_eq!(msaw.open_table_refused_warning, 1);
    assert_eq!(msaw.stats.expect("MSAW accounting").open_full, 1);
    assert_eq!(msaw.stats.expect("MSAW accounting").open_full_warning, 1);
    assert_eq!(msaw.stats.expect("MSAW accounting").open_full_caution, 0);
    assert_eq!(msaw.stats.expect("MSAW accounting").alerts_dropped, 0);
    assert!(matches!(
        msaw.report,
        oa_safety::net::NetReport::Degraded {
            reason: oa_safety::net::Reason::BudgetExceeded,
            area: oa_safety::net::Area::Whole,
            count: 1,
        }
    ));
    let safety = overflow
        .surveillance
        .safety_stats
        .expect("actual safety presenter accounting");
    assert_eq!(safety.nets_shown, 3);
    assert_eq!(safety.feeds_shown, 4);
    assert_eq!(safety.banner_lines, 2);
    assert_eq!(safety.banner_hidden, 6);
    // The first and only FLIGHT operation occurs at 1s; the tested frame is 11s. All later
    // receipts in the fixture contain surveillance positions, so the lost-feed age is 10s.
    let independent_flight_age_ns = 11 * surveillance::SECOND_NS - surveillance::SECOND_NS;
    assert_eq!(independent_flight_age_ns, 10 * surveillance::SECOND_NS);

    let input_count = MAX_FINAL_APPROACH_ASSIGNMENTS + 1;
    runtime.final_monitor_display.alerts = composed_final_alerts(source_alert, input_count);
    runtime.final_monitor_marker_budget = MAX_FINAL_APPROACH_ASSIGNMENTS;
    let console = "C".repeat(MAX_OPERATOR_CONSOLE_BYTES);
    assert_eq!(console.len(), MAX_OPERATOR_CONSOLE_BYTES);
    regional_console_overlay(&mut runtime, &console)
        .expect("both overflow notices and full console compose into a valid frame");

    let final_stats = runtime
        .final_monitor_display
        .display_stats
        .expect("final family renderer reports actual accounting");
    assert_eq!(final_stats.examined, input_count as u64);
    assert_eq!(final_stats.drawn, MAX_FINAL_APPROACH_ASSIGNMENTS as u64);
    assert_eq!(final_stats.omitted, 1);
    assert_eq!(final_stats.warning, 1);
    assert_eq!(runtime.final_monitor_display.alerts.len(), input_count);

    let rows = surveillance::surveillance_text_rows(&runtime.surveillance)
        .expect("whole composed display list text evidence");
    let console_line = "C".repeat(MAX_OPERATOR_CONSOLE_BYTES / 2);
    assert_eq!(overflow_count(&rows, &console_line), 2);
    assert_eq!(overflow_count(&rows, "MSAW +1 NOT SHOWN (1 WARNING)"), 1);
    assert_eq!(overflow_count(&rows, "FINAL +1 NOT SHOWN"), 1);
    assert!(rows.iter().any(|row| row.text == "MSAW PARTIAL"));
    assert_safety_row_text(&rows, "FEED FLIGHT LOST 10.0S", 0);
    assert_safety_row_text(&rows, "+6 MORE STATES NOT SHOWN", 1);
    super::overflow_pixels::assert_notice_isolated(
        &runtime,
        "MSAW +1 NOT SHOWN (1 WARNING)",
        oa_display::core::status_strip::StatusStripRow::MsawOverflow,
    );
    super::overflow_pixels::assert_notice_isolated(
        &runtime,
        "FINAL +1 NOT SHOWN",
        oa_display::core::status_strip::StatusStripRow::FinalOverflow,
    );
    runtime
}

// Mutation killed: drop an overflow row or allow it to collide with other status/map text.
// REQ: OA-REG-020, OA-DSP-078
#[test]
fn simultaneous_alert_overflows_have_disjoint_text_at_review_canvas() {
    let runtime = composed_overflow_runtime(1_600);
    super::overflow_pixels::assert_all_text_disjoint(&runtime);
}

// REQ: OA-REG-020, OA-DSP-078
#[test]
fn simultaneous_alert_overflows_keep_notice_pixels_at_compact_canvas() {
    let runtime = composed_overflow_runtime(1_000);
    super::overflow_pixels::assert_all_text_disjoint(&runtime);
}

// Mutation killed: leave traffic blocks under late warning text or silently omit exhausted labels.
// REQ: OA-REG-019, OA-DSP-078
#[test]
fn exhausted_compact_scene_keeps_warnings_clear_and_reports_unplaced_blocks() {
    let mut runtime = overflow_runtime_with_dimensions(
        overflow_profile(8),
        final_monitor_config(100.0),
        *b"ZAU\0",
        800,
        342,
    );
    let assigned = seed_confirmed_final_monitor_track(&mut runtime, 410_000_000, 0);
    assert_eq!(assigned.final_monitor.status, FinalMonitorStatus::Available);
    let target = *assigned
        .final_monitor
        .alerts
        .first()
        .expect("the correlated target has a real final-monitor alert");
    // The actual target is on the NTZ centreline and is itself a warning.
    assert_eq!(target.severity, final_monitor_core::FinalSeverity::Warning);
    let mut alerts = vec![target];
    // Same explicit input view as overflow_runtime_with_dimensions; no private state bypass.
    let projection = oa_display::core::project::projection_new(
        410_000_000,
        -870_000_000,
        90.0,
        800,
        342,
        oa_display::status_strip::regional_status_plan()
            .unwrap()
            .top_inset_px(),
    )
    .expect("known compact runtime input view is valid");
    let center = oa_display::core::project::project(&projection, target.lat_e7, target.lon_e7)
        .expect("correlated target projects into the compact frame");
    let initial_rows = surveillance::surveillance_text_rows(&runtime.surveillance)
        .expect("initial runtime exposes its actual traffic block");
    let callsign = initial_rows
        .iter()
        .find(|row| row.text == "N123AB")
        .expect("the correlated target initially has its full callsign block");
    let width = callsign.bounds.x1 - callsign.bounds.x0;
    let height = callsign.bounds.y1 - callsign.bounds.y0;
    let center_x = center.0 / 16;
    let center_y = center.1 / 16;
    for (distance_index, distance) in [60_i32, 160, 320].into_iter().enumerate() {
        for direction in [-1_i32, 1] {
            let candidate_center_x = if direction > 0 {
                center_x + distance + width / 2
            } else {
                center_x - distance - width / 2
            };
            // The warning glyph uses a 10px font and a five-pixel label offset. Align its
            // renderer-derived padded bounds with the callsign candidate's actual vertical band.
            let candidate_top_y = center_y - height / 2;
            let point_y = candidate_top_y + 7;
            let (latitude, longitude) = oa_display::core::project::unproject(
                &projection,
                candidate_center_x * 16,
                point_y * 16,
            )
            .expect("bounded compact warning candidate maps to geographic coordinates");
            let mut warning = target;
            warning.address = 0x00ad_0100
                + u32::try_from(distance_index * 2 + if direction > 0 { 1 } else { 0 })
                    .expect("six bounded warning addresses");
            warning.lat_e7 = latitude;
            warning.lon_e7 = longitude;
            warning.severity = final_monitor_core::FinalSeverity::Warning;
            alerts.push(warning);
        }
    }
    alerts.sort_unstable_by_key(|alert| alert.address);
    runtime.final_monitor_display.alerts = alerts;
    runtime.final_monitor_marker_budget = runtime.final_monitor_display.alerts.len();
    regional_console_overlay(&mut runtime, "")
        .expect("actual regional presenter composes bounded compact warnings");

    let spec = regional_frame_spec(&runtime).expect("compact frame geometry is available");
    assert_eq!((spec.width, spec.height), (800, 342));
    super::overflow_pixels::assert_all_text_disjoint(&runtime);

    let rows = surveillance::surveillance_text_rows(&runtime.surveillance)
        .expect("compact runtime exposes actual rendered text rows");
    let notice = rows
        .iter()
        .find(|row| row.text.starts_with("DATA BLOCK +") && row.text.ends_with(" NOT PLACED"))
        .expect("exhausted candidate layout reports omitted full blocks");
    assert_eq!(rows.iter().filter(|row| row.text == "W").count(), 7);
    assert!(!rows.iter().any(|row| row.text == "N123AB"));
    assert_eq!(
        rows.iter().filter(|row| row.text == notice.text).count(),
        1,
        "only one shared-plan omission line is rendered"
    );
    let warnings = surveillance::surveillance_current_warnings(&runtime.surveillance)
        .expect("current runtime warnings include rendered omissions");
    assert!(
        warnings.iter().any(|warning| warning == &notice.text),
        "data-block omission reaches the existing warning channel"
    );
    super::overflow_pixels::assert_notice_isolated(
        &runtime,
        &notice.text,
        oa_display::core::status_strip::StatusStripRow::DataBlockOmitted,
    );
}

#![allow(
    unsafe_code,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::indexing_slicing
)]

use openatca_core_ffi::{
    oa_cifp_parse, OaCifpAirport, OaCifpAirspace, OaCifpAirway, OaCifpAirwayFix, OaCifpBoundary,
    OaCifpBuffers, OaCifpContinuation, OaCifpCounts, OaCifpCycleCount, OaCifpError, OaCifpHeader,
    OaCifpHeaderText, OaCifpLeg, OaCifpLevelOfService, OaCifpLocalizer, OaCifpMora, OaCifpNdb,
    OaCifpProcedure, OaCifpRoute, OaCifpRunway, OaCifpSectionCount, OaCifpVhfNavaid,
    OaCifpWaypoint, OaErrorInfo,
};

const VECTORS: &str = include_str!("fixtures/cifp-v1.tsv");

fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let h = (pair[0] as char).to_digit(16).expect("hex high");
            let l = (pair[1] as char).to_digit(16).expect("hex low");
            ((h << 4) | l) as u8
        })
        .collect()
}

fn diagnostic_values(row: &[&str]) -> [u64; 11] {
    let values = row
        .get(2..)
        .expect("diagnostic columns")
        .iter()
        .map(|value| value.parse::<u64>().expect("numeric diagnostic"))
        .collect::<Vec<_>>();
    values.try_into().expect("eleven diagnostic columns")
}

fn assert_diagnostics(detail: OaCifpError, expected: [u64; 11]) {
    let actual = [
        u64::from(detail.status),
        u64::from(detail.kind),
        detail.line,
        u64::from(detail.first_column),
        u64::from(detail.last_column),
        detail.limit,
        detail.actual,
        detail.previous,
        detail.found,
        u64::from(detail.code),
        u64::from(detail.flags),
    ];
    assert_eq!(actual, expected);
}

#[test]
fn cifp_shared_synthetic_vectors_query_fill_and_refusal_are_transactional() {
    let mut lines = VECTORS.lines();
    assert_eq!(
        lines.next(),
        Some("case\thex\tstatus\tkind\tline\tfirst_column\tlast_column\tlimit\tactual\tprevious\tfound\tcode\tflags")
    );
    let valid = lines
        .next()
        .expect("valid vector")
        .split('\t')
        .collect::<Vec<_>>();
    assert_eq!(valid.first().copied(), Some("valid"));
    let valid_diagnostics = diagnostic_values(&valid);
    let bytes = unhex(valid.get(1).copied().expect("encoded input"));
    let source = b"synthetic.cifp";
    let mut counts = OaCifpCounts::default();
    let mut detail = OaCifpError::default();
    let mut generic = OaErrorInfo {
        code: 0,
        location_kind: 0,
        location: 0,
    };
    let query_buffers = OaCifpBuffers::default();
    // SAFETY: slices and stack outputs are valid; query has no output arrays.
    let status = unsafe {
        oa_cifp_parse(
            bytes.as_ptr(),
            bytes.len() as u64,
            source.as_ptr(),
            source.len() as u64,
            &query_buffers,
            &mut counts,
            &mut detail,
            &mut generic,
        )
    };
    assert_eq!(status, 6);
    assert_eq!(generic.code, 6);
    assert_diagnostics(detail, valid_diagnostics);
    assert_eq!(counts.airports, 1, "counts {counts:?}; detail {detail:?}");
    assert_eq!(counts.runways, 1);
    assert_eq!(counts.localizers, 1);
    assert_eq!(counts.vhf_navaids, 1);
    assert_eq!(counts.ndbs, 2);
    assert_eq!(counts.waypoints, 2);
    assert_eq!(counts.moras, 1);
    assert_eq!(counts.airspaces, 1);
    assert_eq!(counts.boundaries, 1);
    assert_eq!(counts.airways, 1);
    assert_eq!(counts.airway_fixes, 1);
    assert_eq!(counts.header_records, 1);
    assert_eq!(counts.sections, 14);
    assert_eq!(counts.cycles, 1);
    assert_eq!(counts.errors, 0);
    assert_eq!(counts.procedures, 3);
    assert_eq!(counts.routes, 3);
    assert_eq!(counts.legs, 3);
    assert_eq!(counts.continuations, 1);
    assert_eq!(counts.levels_of_service, 3);
    assert!(counts.strings_bytes > 0);

    let mut header = vec![OaCifpHeader::default(); 1];
    let mut header_text = vec![OaCifpHeaderText::default(); counts.header_records as usize];
    let mut airports = vec![OaCifpAirport::default(); counts.airports as usize];
    let mut runways = vec![OaCifpRunway::default(); counts.runways as usize];
    let mut localizers = vec![OaCifpLocalizer::default(); counts.localizers as usize];
    let mut vhf_navaids = vec![OaCifpVhfNavaid::default(); counts.vhf_navaids as usize];
    let mut ndbs = vec![OaCifpNdb::default(); counts.ndbs as usize];
    let mut waypoints = vec![OaCifpWaypoint::default(); counts.waypoints as usize];
    let mut moras = vec![OaCifpMora::default(); counts.moras as usize];
    let mut airspaces = vec![OaCifpAirspace::default(); counts.airspaces as usize];
    let mut boundaries = vec![OaCifpBoundary::default(); counts.boundaries as usize];
    let mut airways = vec![OaCifpAirway::default(); counts.airways as usize];
    let mut airway_fixes = vec![OaCifpAirwayFix::default(); counts.airway_fixes as usize];
    let mut sections = vec![OaCifpSectionCount::default(); counts.sections as usize];
    let mut cycles = vec![OaCifpCycleCount::default(); counts.cycles as usize];
    let mut procedures = vec![OaCifpProcedure::default(); counts.procedures as usize];
    let mut routes = vec![OaCifpRoute::default(); counts.routes as usize];
    let mut legs = vec![OaCifpLeg::default(); counts.legs as usize];
    let mut continuations = vec![OaCifpContinuation::default(); counts.continuations as usize];
    let mut levels = vec![OaCifpLevelOfService::default(); counts.levels_of_service as usize];
    let mut strings = vec![0u8; counts.strings_bytes as usize];
    let outputs = OaCifpBuffers {
        header: header.as_mut_ptr(),
        header_capacity: 1,
        header_text: header_text.as_mut_ptr(),
        header_text_capacity: header_text.len() as u64,
        airports: airports.as_mut_ptr(),
        airports_capacity: airports.len() as u64,
        runways: runways.as_mut_ptr(),
        runways_capacity: runways.len() as u64,
        localizers: localizers.as_mut_ptr(),
        localizers_capacity: localizers.len() as u64,
        vhf_navaids: vhf_navaids.as_mut_ptr(),
        vhf_navaids_capacity: vhf_navaids.len() as u64,
        ndbs: ndbs.as_mut_ptr(),
        ndbs_capacity: ndbs.len() as u64,
        waypoints: waypoints.as_mut_ptr(),
        waypoints_capacity: waypoints.len() as u64,
        moras: moras.as_mut_ptr(),
        moras_capacity: moras.len() as u64,
        airspaces: airspaces.as_mut_ptr(),
        airspaces_capacity: airspaces.len() as u64,
        boundaries: boundaries.as_mut_ptr(),
        boundaries_capacity: boundaries.len() as u64,
        airways: airways.as_mut_ptr(),
        airways_capacity: airways.len() as u64,
        airway_fixes: airway_fixes.as_mut_ptr(),
        airway_fixes_capacity: airway_fixes.len() as u64,
        sections: sections.as_mut_ptr(),
        sections_capacity: sections.len() as u64,
        cycles: cycles.as_mut_ptr(),
        cycles_capacity: cycles.len() as u64,
        procedures: procedures.as_mut_ptr(),
        procedures_capacity: procedures.len() as u64,
        routes: routes.as_mut_ptr(),
        routes_capacity: routes.len() as u64,
        legs: legs.as_mut_ptr(),
        legs_capacity: legs.len() as u64,
        continuations: continuations.as_mut_ptr(),
        continuations_capacity: continuations.len() as u64,
        levels_of_service: levels.as_mut_ptr(),
        levels_of_service_capacity: levels.len() as u64,
        strings: strings.as_mut_ptr(),
        strings_capacity: strings.len() as u64,
        ..OaCifpBuffers::default()
    };
    // SAFETY: every non-empty table has a correctly sized, distinct vector allocation.
    let status = unsafe {
        oa_cifp_parse(
            bytes.as_ptr(),
            bytes.len() as u64,
            source.as_ptr(),
            source.len() as u64,
            &outputs,
            &mut counts,
            &mut detail,
            &mut generic,
        )
    };
    assert_eq!(status, 0);
    assert_eq!(detail.status, 0);
    assert_eq!(header[0].record_length, 132);
    assert_eq!(header[0].cycle_year, 26);
    assert_eq!(airports[0].meta.line, 2);
    assert_eq!(airports[0].present & 1, 1);
    let ident = &strings[airports[0].ident.offset as usize
        ..(airports[0].ident.offset + airports[0].ident.length) as usize];
    assert_eq!(ident, b"KZZZ");
    assert_eq!(runways[0].length_ft, 100);
    assert_eq!(localizers[0].frequency_mhz, 111.75);
    assert_eq!(vhf_navaids[0].frequency_mhz, 113.0);
    assert_eq!(ndbs[0].frequency_khz, 365.0);
    assert_eq!(ndbs[1].terminal, 1);
    assert_eq!(waypoints[0].kind, 1);
    assert_eq!(waypoints[1].kind, 2);
    assert_eq!(moras[0].cells_ft[0], 10_000);
    assert_eq!(airspaces[0].kind, 1);
    assert_eq!(airspaces[0].boundary_count, 1);
    assert_eq!(airways[0].fix_count, 1);
    assert_eq!(airway_fixes[0].sequence, 1);
    assert_eq!(sections[0].count, 1);
    assert_eq!(cycles[0].year, 26);
    assert_eq!(procedures[0].kind, 1);
    assert_eq!(procedures[1].kind, 2);
    assert_eq!(procedures[2].kind, 3);
    assert_eq!(routes[0].leg_count, 1);
    assert_eq!(routes[1].leg_count, 1);
    assert_eq!(legs[0].sequence, 10);
    assert_eq!(continuations[0].application_type, b'W');
    assert_eq!(levels[0].authorized, 1);
    assert_eq!(levels[1].authorized, 0);

    let mut short_airport = vec![OaCifpAirport::default(); 1];
    let mut short_outputs = outputs;
    short_outputs.airports = short_airport.as_mut_ptr();
    short_outputs.airports_capacity = 0;
    let before = short_airport.clone();
    // SAFETY: all non-airport outputs remain valid; airport pointer is valid but declared short.
    let status = unsafe {
        oa_cifp_parse(
            bytes.as_ptr(),
            bytes.len() as u64,
            source.as_ptr(),
            source.len() as u64,
            &short_outputs,
            &mut counts,
            &mut detail,
            &mut generic,
        )
    };
    assert_eq!(status, 6);
    assert_eq!(short_airport, before);
    assert_eq!(counts.airports, 1);
    assert_diagnostics(detail, valid_diagnostics);

    let missing = lines
        .next()
        .expect("missing header vector")
        .split('\t')
        .collect::<Vec<_>>();
    let missing_diagnostics = diagnostic_values(&missing);
    let bad = unhex(missing.get(1).copied().expect("bad input"));
    let sentinel = OaCifpCounts {
        airports: 99,
        ..OaCifpCounts::default()
    };
    counts = sentinel;
    // SAFETY: output/control buffers remain valid; malformed input is the only changed argument.
    let status = unsafe {
        oa_cifp_parse(
            bad.as_ptr(),
            bad.len() as u64,
            source.as_ptr(),
            source.len() as u64,
            &query_buffers,
            &mut counts,
            &mut detail,
            &mut generic,
        )
    };
    assert_eq!(status, 12);
    assert_eq!(counts, sentinel);
    assert_eq!(detail.status, 12);
    assert_diagnostics(detail, missing_diagnostics);
}

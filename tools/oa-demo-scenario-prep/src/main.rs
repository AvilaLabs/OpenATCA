#![forbid(unsafe_code)]
//! Private preparation shell for the synthetic ORD demo scenario.
//!
//! Reads only the pinned public CIFP archive and a public 737 type-certificate PDF. Both reads
//! pass through `oa-sealed`; neither source is copied into the repository. The generated ADS3
//! payload and provenance note are written privately with create-new semantics. This tool is
//! not run by tests or builds and does not read recordings, SWIM data, or facility adaptation.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as FmtWrite;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use oa_adsb::{
    encode_payload, reports_len, reports_push, Reports, Row, F_NEW_LEG, MAX_ROWS_PER_FRAME,
    NONE_I16, NONE_I32, NONE_U16, NON_ICAO, SIL_PER_SAMPLE,
};
use oa_arinc424::{cifp_cycle, is_clean, parse_bytes, ProcedureKind as CifpProcedureKind};
use oa_asterix::SourceId;
use oa_demo_scenario::{
    generate, materialize_traffic, resolve_route_chain, validate_route_motion, MotionAssumptions,
    PerformanceProfileRef, ProcedureKind, ProcedureRouteRef, RouteResolveError, RouteSource,
    RouteVariant, ScenarioArtifact, ScenarioRequest, SyntheticReport, TrafficError,
    B738_NG_MMO_MILLI, B738_NG_VMO_KCAS, NORMAL_ORD_AIRCRAFT, NORMAL_ORD_DURATION_MS,
    NORMAL_ORD_SEED,
};
use oa_sensorsim::{
    plots::to_cat048,
    profile::{profile_by_name, SCAN_TERMINAL_S},
    radar::{simulate_radar, RadarSite},
};
use oa_truth::{build_truth, DEFAULT_CONFIG};

const CIFP_SHA256: &str = "e3ff2b31a4f6ad3eda1342b83ada1ae399aae7bd8c1e67f3cacd5b288b5e141d";
const CIFP_NAME: &str = "CIFP_261029.zip";
const CIFP_ENTRY: &str = "FAACIFP18";
const MAX_CIFP_ARCHIVE: u64 = 1024 * 1024 * 1024;
const MAX_CANDIDATE_CHECKS: usize = 100_000;
const MAX_LISTED_CANDIDATES: usize = 16_384;
const MAX_CANDIDATE_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_DIAGNOSTIC_ROUTES: usize = 512;
const MAX_DIAGNOSTIC_LEGS: usize = 2_000_000;
const MAX_DIAGNOSTIC_OUTPUT_BYTES: usize = 128 * 1024;
const MAX_SELECTOR_INDEX_ENTRIES: usize = 50_000;
const MAX_AIRWAY_IDENTIFIERS: usize = 100_000;
const MAX_AIRWAY_FIX_SCANS: usize = 2_000_000;
const MAX_ROUTE_LIST_DIAGNOSTIC_BUCKETS: usize = 64;
const MAX_PERFORMANCE_SPEC: u64 = 32 * 1024 * 1024;
const SYNTHETIC_EPOCH_NS: i64 = 1_800_000_000_000_000_000;
const OUTPUT_ROOT: &str = "/home/connoravila/Documents/Avila-Labs/openatca-private/demo-scenarios";
const PERF_SPEC_URL: &str = "https://asims.cab.mlit.go.jp/fsdb/a_katashikisyoumei.nsf/0/b98da61b29ae7a89492575850030a18c/%24FILE/19756776.pdf/JCAB_TCDS_No.38_Rev.12_737Series_September_24%2C2026.pdf";
const SCENARIO_LIB_SOURCE: &[u8] = include_bytes!("../../../crates/oa-demo-scenario/src/lib.rs");
const SCENARIO_RESOLVER_SOURCE: &[u8] =
    include_bytes!("../../../crates/oa-demo-scenario/src/shell/resolve.rs");
const SCENARIO_TRAFFIC_SOURCE: &[u8] =
    include_bytes!("../../../crates/oa-demo-scenario/src/core/traffic.rs");
const SENSOR_PROFILE_SOURCE: &[u8] = include_bytes!("../../../crates/oa-sensorsim/src/profile.rs");
const SENSOR_RADAR_SOURCE: &[u8] = include_bytes!("../../../crates/oa-sensorsim/src/radar.rs");
const CIFP_PARSER_LIB_SOURCE: &[u8] = include_bytes!("../../../crates/oa-arinc424/src/lib.rs");
const CIFP_PARSER_FILE_SOURCE: &[u8] = include_bytes!("../../../crates/oa-arinc424/src/file.rs");
const TRUTH_BUILDER_SOURCE: &[u8] = include_bytes!("../../../crates/oa-truth/src/shell/build.rs");

fn normal_motion_assumptions() -> MotionAssumptions {
    MotionAssumptions {
        nominal_ground_speed_kt: 220,
        nominal_ias_proxy_kt: 220,
        initial_altitude_ft: 18_000,
        max_vertical_rate_fpm: 1_800,
        sample_interval_ms: 5_000,
    }
}
// seal-allow: compile-time hash input for this tool's own checked source, not external data
const PREP_TOOL_SOURCE: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"));
const SOURCE_BUNDLE_FILES: &[(&str, &[u8], &str)] = &[
    (
        "crates/oa-demo-scenario/src/lib.rs",
        include_bytes!("../../../crates/oa-demo-scenario/src/lib.rs"),
        "generator API facade",
    ),
    (
        "crates/oa-demo-scenario/src/core/scenario.rs",
        include_bytes!("../../../crates/oa-demo-scenario/src/core/scenario.rs"),
        "generator",
    ),
    (
        "crates/oa-demo-scenario/src/core/traffic.rs",
        include_bytes!("../../../crates/oa-demo-scenario/src/core/traffic.rs"),
        "generator",
    ),
    (
        "crates/oa-demo-scenario/src/core/turn.rs",
        include_bytes!("../../../crates/oa-demo-scenario/src/core/turn.rs"),
        "not used by the current materializer",
    ),
    (
        "crates/oa-demo-scenario/src/shell/resolve.rs",
        include_bytes!("../../../crates/oa-demo-scenario/src/shell/resolve.rs"),
        "generator shell input resolver",
    ),
    (
        "crates/oa-demo-scenario/src/shell/artifact.rs",
        include_bytes!("../../../crates/oa-demo-scenario/src/shell/artifact.rs"),
        "validation only",
    ),
    (
        "tools/oa-demo-scenario-prep/src/main.rs",
        // seal-allow: compile-time snapshot of this tool's own source for provenance
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")),
        "generator",
    ),
    (
        "crates/oa-arinc424/src/lib.rs",
        include_bytes!("../../../crates/oa-arinc424/src/lib.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-arinc424/src/airport.rs",
        include_bytes!("../../../crates/oa-arinc424/src/airport.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-arinc424/src/airspace.rs",
        include_bytes!("../../../crates/oa-arinc424/src/airspace.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-arinc424/src/airway.rs",
        include_bytes!("../../../crates/oa-arinc424/src/airway.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-arinc424/src/error.rs",
        include_bytes!("../../../crates/oa-arinc424/src/error.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-arinc424/src/field.rs",
        include_bytes!("../../../crates/oa-arinc424/src/field.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-arinc424/src/file.rs",
        include_bytes!("../../../crates/oa-arinc424/src/file.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-arinc424/src/header.rs",
        include_bytes!("../../../crates/oa-arinc424/src/header.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-arinc424/src/mora.rs",
        include_bytes!("../../../crates/oa-arinc424/src/mora.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-arinc424/src/navaid.rs",
        include_bytes!("../../../crates/oa-arinc424/src/navaid.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-arinc424/src/procedure.rs",
        include_bytes!("../../../crates/oa-arinc424/src/procedure.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-arinc424/src/record.rs",
        include_bytes!("../../../crates/oa-arinc424/src/record.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-arinc424/src/types.rs",
        include_bytes!("../../../crates/oa-arinc424/src/types.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-arinc424/src/waypoint.rs",
        include_bytes!("../../../crates/oa-arinc424/src/waypoint.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-geo/src/lib.rs",
        include_bytes!("../../../crates/oa-geo/src/lib.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-geo/src/coef.rs",
        include_bytes!("../../../crates/oa-geo/src/coef.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-geo/src/error.rs",
        include_bytes!("../../../crates/oa-geo/src/error.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-geo/src/geodesic.rs",
        include_bytes!("../../../crates/oa-geo/src/geodesic.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-geo/src/latlon.rs",
        include_bytes!("../../../crates/oa-geo/src/latlon.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-geo/src/aeqd.rs",
        include_bytes!("../../../crates/oa-geo/src/aeqd.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-geo/src/polygon.rs",
        include_bytes!("../../../crates/oa-geo/src/polygon.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-geo/src/rhumb.rs",
        include_bytes!("../../../crates/oa-geo/src/rhumb.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-geo/src/small.rs",
        include_bytes!("../../../crates/oa-geo/src/small.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/lib.rs",
        include_bytes!("../../../crates/oa-math/src/lib.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/atan.rs",
        include_bytes!("../../../crates/oa-math/src/atan.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/consts.rs",
        include_bytes!("../../../crates/oa-math/src/consts.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/dd.rs",
        include_bytes!("../../../crates/oa-math/src/dd.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/exp.rs",
        include_bytes!("../../../crates/oa-math/src/exp.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/fast.rs",
        include_bytes!("../../../crates/oa-math/src/fast.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/log.rs",
        include_bytes!("../../../crates/oa-math/src/log.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/nan.rs",
        include_bytes!("../../../crates/oa-math/src/nan.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/order.rs",
        include_bytes!("../../../crates/oa-math/src/order.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/pow.rs",
        include_bytes!("../../../crates/oa-math/src/pow.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/rem.rs",
        include_bytes!("../../../crates/oa-math/src/rem.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/root.rs",
        include_bytes!("../../../crates/oa-math/src/root.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/trig.rs",
        include_bytes!("../../../crates/oa-math/src/trig.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-math/src/util.rs",
        include_bytes!("../../../crates/oa-math/src/util.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-ieee/src/lib.rs",
        include_bytes!("../../../crates/oa-ieee/src/lib.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-zip/src/lib.rs",
        include_bytes!("../../../crates/oa-zip/src/lib.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-zip/src/inflate.rs",
        include_bytes!("../../../crates/oa-zip/src/inflate.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-zip/src/crc32.rs",
        include_bytes!("../../../crates/oa-zip/src/crc32.rs"),
        "generator dependency",
    ),
    (
        "crates/oa-sealed/src/lib.rs",
        include_bytes!("../../../crates/oa-sealed/src/lib.rs"),
        "input safety dependency",
    ),
    (
        "crates/oa-sha256/src/lib.rs",
        include_bytes!("../../../crates/oa-sha256/src/lib.rs"),
        "digest implementation",
    ),
    (
        "crates/oa-sha256/src/core/mod.rs",
        include_bytes!("../../../crates/oa-sha256/src/core/mod.rs"),
        "digest implementation",
    ),
    (
        "crates/oa-sha256/src/core/sha256.rs",
        include_bytes!("../../../crates/oa-sha256/src/core/sha256.rs"),
        "digest implementation",
    ),
    (
        "crates/oa-sha256/src/core/hex.rs",
        include_bytes!("../../../crates/oa-sha256/src/core/hex.rs"),
        "digest implementation",
    ),
    (
        "crates/oa-sha256/src/shell/mod.rs",
        include_bytes!("../../../crates/oa-sha256/src/shell/mod.rs"),
        "digest implementation",
    ),
    (
        "crates/oa-sha256/src/shell/hex.rs",
        include_bytes!("../../../crates/oa-sha256/src/shell/hex.rs"),
        "digest implementation",
    ),
    (
        "Cargo.toml",
        include_bytes!("../../../Cargo.toml"),
        "build configuration",
    ),
    (
        "Cargo.lock",
        include_bytes!("../../../Cargo.lock"),
        "build configuration",
    ),
    (
        "rust-toolchain.toml",
        include_bytes!("../../../rust-toolchain.toml"),
        "build configuration",
    ),
    (
        "crates/oa-demo-scenario/Cargo.toml",
        include_bytes!("../../../crates/oa-demo-scenario/Cargo.toml"),
        "build configuration",
    ),
    (
        "crates/oa-arinc424/Cargo.toml",
        include_bytes!("../../../crates/oa-arinc424/Cargo.toml"),
        "build configuration",
    ),
    (
        "crates/oa-geo/Cargo.toml",
        include_bytes!("../../../crates/oa-geo/Cargo.toml"),
        "build configuration",
    ),
    (
        "crates/oa-math/Cargo.toml",
        include_bytes!("../../../crates/oa-math/Cargo.toml"),
        "build configuration",
    ),
    (
        "crates/oa-ieee/Cargo.toml",
        include_bytes!("../../../crates/oa-ieee/Cargo.toml"),
        "build configuration",
    ),
    (
        "crates/oa-zip/Cargo.toml",
        include_bytes!("../../../crates/oa-zip/Cargo.toml"),
        "build configuration",
    ),
    (
        "crates/oa-sealed/Cargo.toml",
        include_bytes!("../../../crates/oa-sealed/Cargo.toml"),
        "build configuration",
    ),
    (
        "crates/oa-sha256/Cargo.toml",
        include_bytes!("../../../crates/oa-sha256/Cargo.toml"),
        "build configuration",
    ),
    (
        "tools/oa-demo-scenario-prep/Cargo.toml",
        // seal-allow: compile-time snapshot of this tool's own manifest for provenance
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")),
        "build configuration",
    ),
];

fn main() {
    if let Err(error) = run() {
        eprintln!("oa-demo-scenario-prep: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    if let [list_flag, cifp_flag, cifp_path] = raw_args.as_slice() {
        if list_flag == "--list-candidates" && cifp_flag == "--cifp" {
            return list_candidates(Path::new(cifp_path));
        }
        if list_flag == "--diagnose-candidates" && cifp_flag == "--cifp" {
            return diagnose_candidates(Path::new(cifp_path));
        }
        if list_flag == "--list-route-chains" && cifp_flag == "--cifp" {
            return list_route_chains(Path::new(cifp_path), false);
        }
        if list_flag == "--first-route-chain" && cifp_flag == "--cifp" {
            return list_route_chains(Path::new(cifp_path), true);
        }
    }
    let args = Args::parse(raw_args)?;
    let (cifp_archive, archive_sha) = read_guarded(&args.cifp, MAX_CIFP_ARCHIVE)?;
    if args.cifp.file_name().and_then(|v| v.to_str()) != Some(CIFP_NAME) {
        return Err(format!("CIFP input must be named {CIFP_NAME}"));
    }
    if archive_sha != CIFP_SHA256 {
        return Err(format!(
            "CIFP archive SHA-256 mismatch: expected {CIFP_SHA256}, found {archive_sha}"
        ));
    }
    let archive = oa_zip::Archive::parse(&cifp_archive).map_err(|e| e.to_string())?;
    let entry = archive
        .entries()
        .iter()
        .find(|e| e.name() == CIFP_ENTRY)
        .ok_or_else(|| format!("pinned archive has no {CIFP_ENTRY} entry"))?;
    let cifp_bytes = archive.extract_entry(entry).map_err(|e| e.to_string())?;
    let entry_sha = oa_sha256::to_hex(&oa_sha256::sha256(&cifp_bytes));
    let cifp = parse_bytes(&cifp_bytes, CIFP_ENTRY).map_err(|e| e.to_string())?;
    if !is_clean(&cifp) {
        return Err(
            "CIFP parser recorded one or more record errors; route preparation refuses".into(),
        );
    }

    if args.star_route_type == '4' && args.star_runway.is_none() {
        return Err(
            "a type-4 STAR transition must include an explicit --star-runway type-6 selector"
                .into(),
        );
    }
    if args.approach_route_type == 'A' && args.approach_final_type.is_none() {
        return Err(
            "an approach transition must include an explicit --approach-final route type".into(),
        );
    }
    let mut selected = Vec::new();
    if args.four_streams {
        for selectors in FOUR_STREAM_SELECTORS {
            let mut raw = vec![
                "--cifp".into(),
                args.cifp.to_string_lossy().into_owned(),
                "--performance-spec".into(),
                args.performance_spec.to_string_lossy().into_owned(),
                "--performance-sha256".into(),
                args.performance_sha256.clone(),
            ];
            raw.extend(selectors.iter().map(|s| (*s).to_owned()));
            let selection = Args::parse(raw)?;
            selected.push(resolve_selection(&cifp, &selection)?);
        }
    } else {
        selected.push(resolve_selection(&cifp, &args)?);
    }
    let references: Vec<_> = selected
        .iter()
        .flat_map(|(refs, _, _)| refs.iter().cloned())
        .collect();
    let fix_sequence: Vec<_> = selected
        .iter()
        .flat_map(|(_, route, _)| route.fixes.iter().map(|f| f.ident.clone()))
        .collect();
    let mut missed_approach_note = selected
        .iter()
        .map(|(_, _, note)| note.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if args.four_streams {
        missed_approach_note.push_str("\nFour public arrival streams: SW BENKY6/IRK to27L; SE ESSPO5/FWA to27L; NW FYTTE7/ZZIPR to28C; NE WYNDE3/FNT to28R. Exactly15 aircraft per stream. The simulated reports begin at the first source-timed sample within45NM ofKORD, with negative-time startup pre-roll preserved. Arrival threshold phases areSW0s,NE50s,SE105s,NW165s within each240second cycle;27L intervals alternate105s/135s, and28C/28R intervals are240s, so the arrival push builds through the first15minutes. No artificial visibility activation discards inbound motion. Constant220kt groundspeed gives14.66NM in-trail separation on each stream, exceeding6NM. The synthetic DCTs and fix-to-fix model are labelled; no SWIM or reconstructed sector data is used.\n");
    }

    let (performance_bytes, performance_sha) =
        read_guarded(&args.performance_spec, MAX_PERFORMANCE_SPEC)?;
    if performance_bytes.is_empty() {
        return Err("performance source file is empty".into());
    }
    if performance_sha != args.performance_sha256.to_ascii_lowercase() {
        return Err("performance specification SHA-256 does not match reviewed source pin".into());
    }
    let performance = PerformanceProfileRef {
        source_title: "JCAB 737 Type Certificate Data Sheet No. 38, Revision 12".into(),
        source_url: PERF_SPEC_URL.into(),
        source_version: "Issue 12, 2026-09-24".into(),
        source_sha256: performance_sha.clone(),
        aircraft_type: "Boeing 737-800 NG".into(),
        license: "Reference-only; source document not redistributed; rights status not assessed"
            .into(),
        maximum_operating_ias_kcas: B738_NG_VMO_KCAS,
        maximum_operating_mach_milli: B738_NG_MMO_MILLI,
    };
    let request = ScenarioRequest {
        scenario_id: "ord-normal-push-v1".into(),
        seed: NORMAL_ORD_SEED,
        aircraft_count: NORMAL_ORD_AIRCRAFT,
        duration_ms: if args.four_streams {
            60 * 60 * 1_000
        } else {
            NORMAL_ORD_DURATION_MS
        },
        routes: selected
            .iter()
            .map(|(refs, route, _)| RouteVariant {
                source: RouteSource {
                    cifp_archive_name: CIFP_NAME.into(),
                    cifp_archive_sha256: archive_sha.clone(),
                    cifp_entry_name: CIFP_ENTRY.into(),
                    cifp_entry_sha256: entry_sha.clone(),
                    route: refs.clone(),
                    fix_sequence: route.fixes.iter().map(|fix| fix.ident.clone()).collect(),
                },
                performance: performance.clone(),
            })
            .collect(),
    };
    let mut scenario = generate(&request).map_err(|e| e.to_string())?;
    if args.four_streams {
        for (n, flight) in scenario.flights.iter_mut().enumerate() {
            let (route_ix, arrival_ms) = four_stream_slot(n)?;
            let route = scenario
                .routes
                .get(usize::from(route_ix))
                .ok_or("missing stream route")?;
            flight.route_variant = route_ix;
            flight.arrival_time_ms = arrival_ms;
            flight.route = route.source.route.clone();
            flight.performance = route.performance.clone();
        }
    }
    let resolved: Vec<_> = selected.into_iter().map(|(_, route, _)| route).collect();
    let mut samples = if args.four_streams {
        // Synthetic per-stream clearances, clipped by the planner to published CIFP windows.
        oa_demo_scenario::core::materialize_traffic_with_altitude_preferences(
            &scenario,
            &resolved,
            normal_motion_assumptions(),
            &[30_000, 30_000, 30_000, 30_000],
        )
    } else {
        materialize_traffic(&scenario, &resolved, normal_motion_assumptions())
    }
    .map_err(|e| e.to_string())?;
    if args.four_streams {
        // Crop the source-timed flight at the first sample inside the 45 NM approach area.
        // Negative-time pre-roll preserves motion; no browser-side activation or timing is used.
        let airport = oa_arinc424::find_airport(&cifp, "KORD")
            .ok_or("KORD airport reference point missing")?;
        oa_demo_scenario::core::crop_arrival_area(
            &mut samples,
            airport.position.lat,
            airport.position.lon,
            45,
        )
        .map_err(|e| e.to_string())?;
    }
    samples.sort_by_key(|sample| (sample.virtual_time_ms, sample.aircraft_index));
    let airport =
        oa_arinc424::find_airport(&cifp, "KORD").ok_or("KORD airport reference point missing")?;
    let center_lat_e7 = degree_to_e7(airport.position.lat)?;
    let center_lon_e7 = degree_to_e7(airport.position.lon)?;
    let scenario_lib_sha = oa_sha256::to_hex(&oa_sha256::sha256(SCENARIO_LIB_SOURCE));
    let resolver_sha = oa_sha256::to_hex(&oa_sha256::sha256(SCENARIO_RESOLVER_SOURCE));
    let traffic_sha = oa_sha256::to_hex(&oa_sha256::sha256(SCENARIO_TRAFFIC_SOURCE));
    let prep_tool_sha = oa_sha256::to_hex(&oa_sha256::sha256(PREP_TOOL_SOURCE));
    let source_hashes = [
        archive_sha.as_str(),
        entry_sha.as_str(),
        performance_sha.as_str(),
        scenario_lib_sha.as_str(),
        resolver_sha.as_str(),
        traffic_sha.as_str(),
        prep_tool_sha.as_str(),
    ];
    let oasd_payload = serialize_oasd(
        &scenario,
        &samples,
        source_hashes,
        center_lat_e7,
        center_lon_e7,
    )?;
    let oasd_sha = oa_sha256::to_hex(&oa_sha256::sha256(&oasd_payload));
    let (source_bundle_sha, source_bundle_inventory) =
        source_bundle(&archive_sha, &entry_sha, &performance_sha);
    if args.outputs == OutputMode::Oasd {
        let manifest = oasd_manifest_text(
            &scenario,
            &references,
            &fix_sequence,
            &archive_sha,
            &entry_sha,
            &performance_sha,
            samples.len(),
            &oasd_sha,
            oasd_payload.len(),
            center_lat_e7,
            center_lon_e7,
            &source_bundle_sha,
            &source_bundle_inventory,
            &scenario_lib_sha,
            &resolver_sha,
            &traffic_sha,
            &prep_tool_sha,
            &missed_approach_note,
        );
        let out_root = safe_output_root(args.output_root.as_deref())?;
        write_new(&out_root.join("ord-normal-push-v1.oasd"), &oasd_payload)?;
        if args.four_streams {
            let map = public_map_rust(&cifp, &resolved)?;
            write_new(&out_root.join("ord-map.rs"), map.as_bytes())?;
        }
        write_new(
            &out_root.join("ord-normal-push-v1.provenance.md"),
            manifest.as_bytes(),
        )?;
        sync_directory(&out_root)?;
        println!(
            "scenario={} aircraft={} output=OASD records={} bytes={} sha256={} cifp_cycle={}",
            scenario.scenario_id,
            scenario.flights.len(),
            samples.len(),
            oasd_payload.len(),
            oasd_sha,
            cifp_cycle(&cifp).map_or_else(|| "unknown".into(), |cycle| cycle.to_string()),
        );
        return Ok(());
    }
    let sorted = reports_from_synthetic(&samples)?;
    let row_count = reports_len(&sorted);
    let ads3_frames = encode_ads3_frames(&sorted, row_count)?;
    let (frame_manifest_sha, frame_inventory) = ads3_frame_inventory(&ads3_frames);
    let (truth, truth_stats) = build_truth(&sorted, &DEFAULT_CONFIG).map_err(|e| e.to_string())?;
    let profile = profile_by_name("MSSR_EUROCONTROL_1997").ok_or("named radar profile missing")?;
    let from_ns = sorted
        .time_ns
        .first()
        .copied()
        .ok_or("no synthetic report times")?;
    let to_ns = sorted
        .time_ns
        .last()
        .copied()
        .ok_or("no synthetic report times")?
        .checked_add(1)
        .ok_or("synthetic time overflow")?;
    let site = RadarSite {
        lat_deg: airport.position.lat,
        lon_deg: airport.position.lon,
        height_m: 0.0,
        rotation_period_s: SCAN_TERMINAL_S,
        initial_az_deg: 0.0,
        epoch_ns: from_ns,
        coverage_range_m: 80.0 * 1852.0,
        earth_k: 4.0 / 3.0,
        profile,
        bias_range_m: 0.0,
        bias_az_deg: 0.0,
        split_dr_m: 0.0,
        split_daz_deg: 0.0,
        sac: 250,
        sic: 1,
    };
    let (plots, sensor_stats) = simulate_radar(&truth, &site, NORMAL_ORD_SEED, from_ns, to_ns)
        .map_err(|e| e.to_string())?;
    let cat048 = to_cat048(
        &plots,
        SourceId {
            sac: site.sac,
            sic: site.sic,
        },
    )
    .map_err(|e| e.to_string())?;
    let cat048_sha = oa_sha256::to_hex(&oa_sha256::sha256(&cat048));
    let manifest = manifest_text(
        &scenario,
        &references,
        &fix_sequence,
        &archive_sha,
        &entry_sha,
        &performance_sha,
        row_count,
        &frame_manifest_sha,
        &frame_inventory,
        &ads3_frames,
        truth_stats.rows_kept,
        sensor_stats.detected,
        sensor_stats.false_plots,
        sensor_stats.split_plots,
        &cat048_sha,
        &oasd_sha,
        oasd_payload.len(),
        center_lat_e7,
        center_lon_e7,
        &source_bundle_sha,
        &source_bundle_inventory,
        &oa_sha256::to_hex(&oa_sha256::sha256(SCENARIO_LIB_SOURCE)),
        &oa_sha256::to_hex(&oa_sha256::sha256(SCENARIO_RESOLVER_SOURCE)),
        &oa_sha256::to_hex(&oa_sha256::sha256(SCENARIO_TRAFFIC_SOURCE)),
        &oa_sha256::to_hex(&oa_sha256::sha256(SENSOR_PROFILE_SOURCE)),
        &oa_sha256::to_hex(&oa_sha256::sha256(SENSOR_RADAR_SOURCE)),
        &oa_sha256::to_hex(&oa_sha256::sha256(CIFP_PARSER_LIB_SOURCE)),
        &oa_sha256::to_hex(&oa_sha256::sha256(CIFP_PARSER_FILE_SOURCE)),
        &oa_sha256::to_hex(&oa_sha256::sha256(TRUTH_BUILDER_SOURCE)),
        &oa_sha256::to_hex(&oa_sha256::sha256(PREP_TOOL_SOURCE)),
        &missed_approach_note,
    );
    let out_root = safe_output_root(args.output_root.as_deref())?;
    write_new(&out_root.join("ord-normal-push-v1.oasd"), &oasd_payload)?;
    for frame in &ads3_frames {
        write_new(&out_root.join(&frame.filename), &frame.payload)?;
    }
    write_new(&out_root.join("ord-normal-push-v1.cat048"), &cat048)?;
    write_new(
        &out_root.join("ord-normal-push-v1.provenance.md"),
        manifest.as_bytes(),
    )?;
    sync_directory(&out_root)?;
    println!("scenario={} aircraft={} reports={} ads3_frames={} truth_rows={} detected_plots={} cifp_cycle={} oasd_sha256={} ads3_frame_manifest_sha256={} cat048_sha256={}",
        scenario.scenario_id,
        scenario.flights.len(),
        row_count,
        ads3_frames.len(),
        truth_stats.rows_kept,
        sensor_stats.detected,
        cifp_cycle(&cifp).map_or_else(|| "unknown".into(), |cycle| cycle.to_string()),
        oasd_sha,
        frame_manifest_sha,
        cat048_sha,
    );
    Ok(())
}

const FOUR_STREAM_SELECTORS: [&[&str]; 4] = [
    &[
        "--airway",
        "J151",
        "STL",
        "IRK",
        "--star",
        "BENKY6",
        "4",
        "IRK",
        "--approach",
        "I27L",
        "I",
        "",
        "--star-runway",
        "6",
        "RW27L",
        "--synthetic-direct",
    ],
    &[
        "--airway",
        "J149",
        "ROD",
        "FWA",
        "--star",
        "ESSPO5",
        "4",
        "FWA",
        "--approach",
        "I27L",
        "I",
        "",
        "--star-runway",
        "6",
        "RW27B",
        "--synthetic-direct",
    ],
    &[
        "--airway",
        "Q156",
        "HSTIN",
        "ZZIPR",
        "--star",
        "FYTTE7",
        "4",
        "ZZIPR",
        "--approach",
        "I28C",
        "I",
        "",
        "--star-runway",
        "6",
        "RW28C",
        "--synthetic-direct",
    ],
    &[
        "--airway",
        "J36",
        "DABJU",
        "FNT",
        "--star",
        "WYNDE3",
        "4",
        "FNT",
        "--approach",
        "I28R",
        "I",
        "",
        "--star-runway",
        "6",
        "RW28B",
        "--synthetic-direct",
    ],
];

fn four_stream_slot(index: usize) -> Result<(u16, u64), String> {
    oa_demo_scenario::core::normal_arrival_slot(index)
        .ok_or_else(|| "aircraft index outside normal-push scenario".into())
}

fn resolve_selection(
    cifp: &oa_arinc424::Cifp,
    args: &Args,
) -> Result<
    (
        Vec<ProcedureRouteRef>,
        oa_demo_scenario::ResolvedRoute,
        String,
    ),
    String,
> {
    let airway_ref = ProcedureRouteRef {
        airport: "ENRT".into(),
        kind: ProcedureKind::Airway,
        procedure: args.airway.clone(),
        transition: format!("{}>{}", args.airway_from, args.airway_to),
        route_type: None,
    };
    let mut star_refs = expand_star_refs(
        cifp,
        &ProcedureRouteRef {
            airport: "KORD".into(),
            kind: ProcedureKind::Star,
            procedure: args.star.clone(),
            transition: args.star_transition.clone(),
            route_type: Some(args.star_route_type),
        },
        args.star_runway.clone(),
        None,
    )?;
    let approach_refs = expand_approach_refs(
        cifp,
        &ProcedureRouteRef {
            airport: "KORD".into(),
            kind: ProcedureKind::Approach,
            procedure: args.approach.clone(),
            transition: args.approach_transition.clone(),
            route_type: Some(args.approach_route_type),
        },
        args.approach_final_type,
        None,
    )?;
    let star_end = route_endpoint(cifp, star_refs.last().ok_or("STAR route is empty")?)?.1;
    let approach_start = route_endpoint(
        cifp,
        approach_refs.first().ok_or("approach route is empty")?,
    )?
    .0;
    let mut references = vec![airway_ref];
    references.append(&mut star_refs);
    if star_end != approach_start {
        if !args.synthetic_direct {
            return Err(format!(
                "STAR exits at {star_end} and approach begins at {approach_start}; an explicit --synthetic-direct is required"
            ));
        }
        references.push(ProcedureRouteRef {
            airport: "SYN".into(),
            kind: ProcedureKind::Direct,
            procedure: "DCT".into(),
            transition: format!("{star_end}>{approach_start}"),
            route_type: None,
        });
    } else if args.synthetic_direct {
        return Err(
            "--synthetic-direct was requested but the selected procedures already join exactly"
                .into(),
        );
    }
    references.extend(approach_refs);
    let resolved = resolve_route_chain(cifp, &references).map_err(|e| e.to_string())?;
    let missed_approach_note = missed_approach_summary(cifp, &references)?;

    Ok((references, resolved, missed_approach_note))
}

/// Lists decoder-backed, continuous KORD airway → STAR → approach chains. This mode validates
/// the pinned archive and entry, performs no writes, and refuses rather than truncating output.
fn list_candidates(cifp_path: &Path) -> Result<(), String> {
    let (archive_bytes, archive_sha) = read_guarded(cifp_path, MAX_CIFP_ARCHIVE)?;
    if cifp_path.file_name().and_then(|value| value.to_str()) != Some(CIFP_NAME) {
        return Err(format!("CIFP input must be named {CIFP_NAME}"));
    }
    if archive_sha != CIFP_SHA256 {
        return Err(format!(
            "CIFP archive SHA-256 mismatch: expected {CIFP_SHA256}, found {archive_sha}"
        ));
    }
    let archive = oa_zip::Archive::parse(&archive_bytes).map_err(|error| error.to_string())?;
    let entry = archive
        .entries()
        .iter()
        .find(|entry| entry.name() == CIFP_ENTRY)
        .ok_or_else(|| format!("pinned archive has no {CIFP_ENTRY} entry"))?;
    let cifp_bytes = archive
        .extract_entry(entry)
        .map_err(|error| error.to_string())?;
    let entry_sha = oa_sha256::to_hex(&oa_sha256::sha256(&cifp_bytes));
    let cifp = parse_bytes(&cifp_bytes, CIFP_ENTRY).map_err(|error| error.to_string())?;
    if !is_clean(&cifp) {
        return Err(
            "CIFP parser recorded one or more record errors; candidate listing refuses".into(),
        );
    }

    let mut selector_index_entries = 0usize;
    let mut stars = Vec::new();
    for procedure in oa_arinc424::procedures_at(&cifp, "KORD", CifpProcedureKind::Star) {
        for route in &procedure.routes {
            let (Some(first), Some(last)) = (
                route.legs.first().and_then(|leg| leg.fix.as_ref()),
                route.legs.last().and_then(|leg| leg.fix.as_ref()),
            ) else {
                continue;
            };
            selector_index_entries = selector_index_entries.saturating_add(1);
            if selector_index_entries > MAX_SELECTOR_INDEX_ENTRIES {
                return Err(format!("selector index exceeds fixed cap {MAX_SELECTOR_INDEX_ENTRIES}; no partial list emitted"));
            }
            stars.push((
                procedure.ident.clone(),
                route.route_type,
                route.transition.clone(),
                first.ident.clone(),
                last.ident.clone(),
            ));
        }
    }

    let mut approaches_by_first: BTreeMap<String, Vec<(String, char, String)>> = BTreeMap::new();
    for procedure in oa_arinc424::procedures_at(&cifp, "KORD", CifpProcedureKind::Approach) {
        for route in &procedure.routes {
            let Some(first) = route.legs.first().and_then(|leg| leg.fix.as_ref()) else {
                continue;
            };
            selector_index_entries = selector_index_entries.saturating_add(1);
            if selector_index_entries > MAX_SELECTOR_INDEX_ENTRIES {
                return Err(format!("selector index exceeds fixed cap {MAX_SELECTOR_INDEX_ENTRIES}; no partial list emitted"));
            }
            approaches_by_first
                .entry(first.ident.clone())
                .or_default()
                .push((
                    procedure.ident.clone(),
                    route.route_type,
                    route.transition.clone(),
                ));
        }
    }

    if cifp.airways.len() > MAX_AIRWAY_IDENTIFIERS {
        return Err(format!("airway identifier count exceeds fixed cap {MAX_AIRWAY_IDENTIFIERS}; no partial list emitted"));
    }
    let mut airway_counts: BTreeMap<String, usize> = BTreeMap::new();
    for airway in &cifp.airways {
        let count = airway_counts.entry(airway.ident.clone()).or_default();
        *count = count.saturating_add(1);
    }
    let star_entries: BTreeSet<&str> = stars.iter().map(|star| star.3.as_str()).collect();
    let mut airway_segments_by_to: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut airway_fix_scans = 0usize;
    for airway in &cifp.airways {
        if airway_counts.get(&airway.ident) != Some(&1) {
            continue;
        }
        for (entry_index, fix) in airway.fixes.iter().enumerate() {
            airway_fix_scans = airway_fix_scans.saturating_add(1);
            if airway_fix_scans > MAX_AIRWAY_FIX_SCANS {
                return Err(format!("airway fix scans exceed fixed cap {MAX_AIRWAY_FIX_SCANS}; no partial list emitted"));
            }
            if !star_entries.contains(fix.fix.ident.as_str()) {
                continue;
            }
            for neighbor_index in [entry_index.checked_sub(1), entry_index.checked_add(1)]
                .into_iter()
                .flatten()
            {
                let Some(neighbor) = airway.fixes.get(neighbor_index) else {
                    continue;
                };
                selector_index_entries = selector_index_entries.saturating_add(1);
                if selector_index_entries > MAX_SELECTOR_INDEX_ENTRIES {
                    return Err(format!("selector index exceeds fixed cap {MAX_SELECTOR_INDEX_ENTRIES}; no partial list emitted"));
                }
                airway_segments_by_to
                    .entry(fix.fix.ident.clone())
                    .or_default()
                    .push((airway.ident.clone(), neighbor.fix.ident.clone()));
            }
        }
    }

    let mut out = String::new();
    writeln!(
        out,
        "CIFP candidates: archive_sha256={archive_sha} entry={CIFP_ENTRY} entry_sha256={entry_sha} cycle={} airport=KORD",
        cifp_cycle(&cifp).map_or_else(|| "unknown".into(), |cycle| cycle.to_string())
    )
    .map_err(|_| "candidate output formatting failed")?;
    writeln!(out, "Only route triples accepted by the existing resolver are listed; this is selector discovery, not proof the 737 scenario materializes.")
        .map_err(|_| "candidate output formatting failed")?;
    writeln!(out, "selector columns: --airway IDENT FROM TO | --star IDENT TYPE TRANSITION | --approach IDENT TYPE TRANSITION")
        .map_err(|_| "candidate output formatting failed")?;

    let mut checks = 0usize;
    let mut listed = 0usize;
    let mut seen = BTreeSet::new();
    for (star_ident, star_type, star_transition, star_first, star_last) in &stars {
        let Some(approaches) = approaches_by_first.get(star_last) else {
            continue;
        };
        let Some(airway_segments) = airway_segments_by_to.get(star_first) else {
            continue;
        };
        for (approach_ident, approach_type, approach_transition) in approaches {
            for (airway_ident, airway_from) in airway_segments {
                checks = checks.saturating_add(1);
                if checks > MAX_CANDIDATE_CHECKS {
                    return Err(format!("candidate work exceeds fixed cap {MAX_CANDIDATE_CHECKS}; no partial list emitted"));
                }
                let key = (
                    airway_ident.clone(),
                    airway_from.clone(),
                    star_first.clone(),
                    star_ident.clone(),
                    *star_type,
                    star_transition.clone(),
                    approach_ident.clone(),
                    *approach_type,
                    approach_transition.clone(),
                );
                if seen.contains(&key) {
                    continue;
                }
                let refs = [
                    ProcedureRouteRef {
                        airport: "ENRT".into(),
                        kind: ProcedureKind::Airway,
                        procedure: key.0.clone(),
                        transition: format!("{}>{}", key.1, key.2),
                        route_type: None,
                    },
                    ProcedureRouteRef {
                        airport: "KORD".into(),
                        kind: ProcedureKind::Star,
                        procedure: key.3.clone(),
                        transition: key.5.clone(),
                        route_type: Some(key.4),
                    },
                    ProcedureRouteRef {
                        airport: "KORD".into(),
                        kind: ProcedureKind::Approach,
                        procedure: key.6.clone(),
                        transition: key.8.clone(),
                        route_type: Some(key.7),
                    },
                ];
                if resolve_route_chain(&cifp, &refs).is_err() {
                    continue;
                }
                seen.insert(key.clone());
                listed = listed.saturating_add(1);
                if listed > MAX_LISTED_CANDIDATES {
                    return Err(format!("candidate count exceeds fixed cap {MAX_LISTED_CANDIDATES}; no partial list emitted"));
                }
                writeln!(
                    out,
                    "--airway {} {} {} --star {} {} {} --approach {} {} {}",
                    key.0, key.1, key.2, key.3, key.4, key.5, key.6, key.7, key.8
                )
                .map_err(|_| "candidate output formatting failed")?;
                if out.len() > MAX_CANDIDATE_OUTPUT_BYTES {
                    return Err(format!("candidate output exceeds fixed cap {MAX_CANDIDATE_OUTPUT_BYTES} bytes; no partial list emitted"));
                }
            }
        }
    }
    if listed == 0 {
        return Err(
            "no continuous airway → STAR → approach selector chain was found for KORD".into(),
        );
    }
    print!("{out}");
    Ok(())
}

fn route_endpoint(
    cifp: &oa_arinc424::Cifp,
    reference: &ProcedureRouteRef,
) -> Result<(String, String), String> {
    let route = oa_demo_scenario::resolve_procedure_route(cifp, reference)
        .map_err(|error| error.to_string())?;
    let first = route
        .fixes
        .first()
        .ok_or("selected route has no resolved fixes")?
        .ident
        .clone();
    let last = route
        .fixes
        .last()
        .ok_or("selected route has no resolved fixes")?
        .ident
        .clone();
    Ok((first, last))
}

/// A selected STAR transition of type 4 is followed by its unique common type-5 route. A
/// runway-specific type-6 route is included only when its exact selector is supplied.
fn expand_star_refs(
    cifp: &oa_arinc424::Cifp,
    transition: &ProcedureRouteRef,
    runway: Option<(char, String)>,
    mut diagnostics: Option<&mut RouteListingDiagnostics>,
) -> Result<Vec<ProcedureRouteRef>, String> {
    if runway
        .as_ref()
        .is_some_and(|(route_type, _)| *route_type != '6')
    {
        return Err("--star-runway must select a published type-6 STAR runway transition".into());
    }
    let (_, mut last) = route_endpoint(cifp, transition)?;
    let mut references = vec![transition.clone()];
    if transition.route_type == Some('4') {
        let procedure = cifp
            .procedures
            .iter()
            .find(|procedure| {
                procedure.airport_ident == "KORD"
                    && procedure.kind == CifpProcedureKind::Star
                    && procedure.ident == transition.procedure
            })
            .ok_or("selected STAR procedure disappeared during resolution")?;
        let mut matches = Vec::new();
        for route in &procedure.routes {
            if route.route_type != '5' || !route.transition.is_empty() {
                continue;
            }
            if let Some(diagnostics) = diagnostics.as_deref_mut() {
                diagnostics.count("star.empty_transition_type5_common_groups")?;
            }
            let reference = ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Star,
                procedure: transition.procedure.clone(),
                transition: String::new(),
                route_type: Some('5'),
            };
            match oa_demo_scenario::resolve_procedure_route(cifp, &reference) {
                Ok(resolved) => {
                    if resolved.fixes.first().is_some_and(|fix| fix.ident == last) {
                        matches.push((reference, route));
                    } else if let Some(diagnostics) = diagnostics.as_deref_mut() {
                        diagnostics.count("star.common.candidate_endpoint_mismatch")?;
                    }
                }
                Err(error) => {
                    if let Some(diagnostics) = diagnostics.as_deref_mut() {
                        diagnostics.resolver_refusal("star.common.candidate.resolve", &error)?;
                    }
                }
            }
        }
        if matches.len() != 1 {
            if let Some(diagnostics) = diagnostics.as_deref_mut() {
                diagnostics.count("star.common.selector_nonunique_type5_join")?;
            }
            return Err(format!(
                "STAR {} transition does not have one unique type-5 common route beginning at {last}",
                transition.procedure
            ));
        }
        let (common, _) = matches.pop().ok_or("STAR common route disappeared")?;
        last = route_endpoint(cifp, &common)?.1;
        references.push(common);
    }
    if let Some((route_type, transition_id)) = runway {
        let runway_ref = ProcedureRouteRef {
            airport: "KORD".into(),
            kind: ProcedureKind::Star,
            procedure: transition.procedure.clone(),
            transition: transition_id,
            route_type: Some(route_type),
        };
        let (first, _) = match oa_demo_scenario::resolve_procedure_route(cifp, &runway_ref) {
            Ok(resolved) => {
                let Some(first) = resolved.fixes.first().map(|fix| fix.ident.clone()) else {
                    return Err("selected STAR runway transition has no resolved fixes".into());
                };
                let Some(last) = resolved.fixes.last().map(|fix| fix.ident.clone()) else {
                    return Err("selected STAR runway transition has no resolved fixes".into());
                };
                (first, last)
            }
            Err(error) => {
                if let Some(diagnostics) = diagnostics.as_deref_mut() {
                    diagnostics.resolver_refusal("star.runway.candidate.resolve", &error)?;
                }
                return Err(error.to_string());
            }
        };
        if first != last {
            if let Some(diagnostics) = diagnostics {
                diagnostics.count("star.runway.endpoint_join_refusal")?;
            }
            return Err(format!(
                "selected runway transition begins at {first}, expected common-route endpoint {last}"
            ));
        }
        references.push(runway_ref);
    }
    Ok(references)
}

/// A selected approach transition is followed by the exact empty-transition final route only
/// when the caller supplies its published route type. Unsupported final route records still fail.
fn expand_approach_refs(
    cifp: &oa_arinc424::Cifp,
    transition: &ProcedureRouteRef,
    final_type: Option<char>,
    mut diagnostics: Option<&mut RouteListingDiagnostics>,
) -> Result<Vec<ProcedureRouteRef>, String> {
    if final_type == Some('A') {
        return Err(
            "--approach-final must select a final route type, not type A transition".into(),
        );
    }
    let (_, last) = route_endpoint(cifp, transition)?;
    let mut references = vec![transition.clone()];
    if let Some(route_type) = final_type {
        let procedure = cifp
            .procedures
            .iter()
            .find(|procedure| {
                procedure.airport_ident == "KORD"
                    && procedure.kind == CifpProcedureKind::Approach
                    && procedure.ident == transition.procedure
            })
            .ok_or("selected approach procedure disappeared during resolution")?;
        let mut matches = Vec::new();
        for route in &procedure.routes {
            if route.route_type != route_type || !route.transition.is_empty() {
                continue;
            }
            let reference = ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Approach,
                procedure: transition.procedure.clone(),
                transition: String::new(),
                route_type: Some(route_type),
            };
            match oa_demo_scenario::resolve_procedure_route(cifp, &reference) {
                Ok(resolved) => {
                    if resolved.fixes.first().is_some_and(|fix| fix.ident == last) {
                        matches.push(reference);
                    } else if let Some(diagnostics) = diagnostics.as_deref_mut() {
                        diagnostics.count("approach.final.candidate_endpoint_mismatch")?;
                    }
                }
                Err(error) => {
                    if let Some(diagnostics) = diagnostics.as_deref_mut() {
                        diagnostics.resolver_refusal("approach.final.candidate.resolve", &error)?;
                    }
                }
            }
        }
        if matches.len() != 1 {
            if let Some(diagnostics) = diagnostics {
                diagnostics.count("approach.final.selector_nonunique_route_join")?;
            }
            return Err(format!(
                "approach {} transition does not have one unique type-{route_type} final route beginning at {last}",
                transition.procedure
            ));
        }
        let final_route = matches.pop().ok_or("approach final route disappeared")?;
        references.push(final_route);
    }
    Ok(references)
}

/// Lists complete route assemblies: an airway, STAR transition + common + runway route,
/// approach transition + published final, and an explicit synthetic DCT when the STAR exit and
/// approach entry are different fixes. Unsupported CIFP records remain refused.
#[derive(Default)]
struct RouteListingDiagnostics {
    counts: BTreeMap<String, u64>,
}

impl RouteListingDiagnostics {
    fn count(&mut self, label: &str) -> Result<(), String> {
        if label.len() > 96 {
            return Err("route-list diagnostic label exceeds bound".into());
        }
        if !self.counts.contains_key(label)
            && self.counts.len() >= MAX_ROUTE_LIST_DIAGNOSTIC_BUCKETS
        {
            return Err("route-list diagnostic bucket cap exceeded".into());
        }
        let value = self.counts.entry(label.to_owned()).or_default();
        *value = value
            .checked_add(1)
            .ok_or_else(|| "route-list diagnostic counter overflow".to_owned())?;
        Ok(())
    }

    fn resolver_refusal(&mut self, stage: &str, error: &RouteResolveError) -> Result<(), String> {
        self.count(&format!("{stage}.{}", route_resolve_error_class(error)))
    }

    fn render(&self) -> Result<String, String> {
        let mut out =
            String::from("ROUTE-LIST-DIAGNOSTICS v1 (counts only; no route recommendations)\n");
        for (label, count) in &self.counts {
            writeln!(out, "{label}={count}")
                .map_err(|_| "route-list diagnostic formatting failed")?;
            if out.len() > MAX_DIAGNOSTIC_OUTPUT_BYTES {
                return Err("route-list diagnostic output cap exceeded".into());
            }
        }
        Ok(out)
    }
}

fn route_resolve_error_class(error: &RouteResolveError) -> String {
    match error {
        RouteResolveError::UnsupportedKind => "UnsupportedKind".into(),
        RouteResolveError::ProcedureNotFound => "ProcedureNotFound".into(),
        RouteResolveError::TransitionNotFound => "TransitionNotFound".into(),
        RouteResolveError::AmbiguousTransition => "AmbiguousTransition".into(),
        RouteResolveError::MissingFix(_) => "MissingFix".into(),
        RouteResolveError::FixNotFound(_) => "FixNotFound".into(),
        RouteResolveError::AmbiguousFix(_) => "AmbiguousFix".into(),
        RouteResolveError::UnknownAltitude(_) => "UnknownAltitude".into(),
        RouteResolveError::UnsupportedAltitudeDescription(_, code) => {
            format!("UnsupportedAltitudeDescription_U+{:04X}", u32::from(*code))
        }
        RouteResolveError::InvalidAltitudeValue(_, field, kind) => {
            let kind = match kind {
                oa_demo_scenario::AltitudeValueKind::Missing => "Missing",
                oa_demo_scenario::AltitudeValueKind::Ground => "Ground",
                oa_demo_scenario::AltitudeValueKind::Unlimited => "Unlimited",
                oa_demo_scenario::AltitudeValueKind::Unknown => "Unknown",
            };
            format!("InvalidAltitudeValue_field{field}_{kind}")
        }
        RouteResolveError::UnexpectedAltitudeValue(_) => "UnexpectedAltitudeValue".into(),
        RouteResolveError::ContradictoryAltitudeWindow(_) => "ContradictoryAltitudeWindow".into(),
        RouteResolveError::ConflictingVerticalGuidance(_) => "ConflictingVerticalGuidance".into(),
        RouteResolveError::BadSpeedLimit(_) => "BadSpeedLimit".into(),
        RouteResolveError::BadDirectConnector => "BadDirectConnector".into(),
        RouteResolveError::MissingPublishedMap => "MissingPublishedMap".into(),
        RouteResolveError::MultiplePublishedMaps => "MultiplePublishedMaps".into(),
    }
}

fn traffic_error_class(error: &TrafficError) -> &'static str {
    match error {
        TrafficError::RouteCount => "RouteCount",
        TrafficError::RouteReference(_) => "RouteReference",
        TrafficError::RouteFixCount(_) => "RouteFixCount",
        TrafficError::RouteFixLimit(_) => "RouteFixLimit",
        TrafficError::RouteFixOrder(_) => "RouteFixOrder",
        TrafficError::InvalidLatLon(_, _) => "InvalidLatLon",
        TrafficError::DegenerateSegment(_, _) => "DegenerateSegment",
        TrafficError::BadAltitudeWindow(_, _) => "BadAltitudeWindow",
        TrafficError::BadSpeedLimit(_, _) => "BadSpeedLimit",
        TrafficError::IasProxyExceedsProcedureLimit(_, _, _, _) => "IasProxyExceedsProcedureLimit",
        TrafficError::Assumption(_) => "Assumption",
        TrafficError::Geodesic => "Geodesic",
        TrafficError::VerticalRate(_, _) => "VerticalRate",
        TrafficError::TimeRange => "TimeRange",
        TrafficError::OutputLimit => "OutputLimit",
    }
}

fn list_route_chains(cifp_path: &Path, first_only: bool) -> Result<(), String> {
    let (archive_bytes, archive_sha) = read_guarded(cifp_path, MAX_CIFP_ARCHIVE)?;
    if cifp_path.file_name().and_then(|value| value.to_str()) != Some(CIFP_NAME) {
        return Err(format!("CIFP input must be named {CIFP_NAME}"));
    }
    if archive_sha != CIFP_SHA256 {
        return Err(format!(
            "CIFP archive SHA-256 mismatch: expected {CIFP_SHA256}, found {archive_sha}"
        ));
    }
    let archive = oa_zip::Archive::parse(&archive_bytes).map_err(|error| error.to_string())?;
    let entry = archive
        .entries()
        .iter()
        .find(|entry| entry.name() == CIFP_ENTRY)
        .ok_or_else(|| format!("pinned archive has no {CIFP_ENTRY} entry"))?;
    let cifp_bytes = archive
        .extract_entry(entry)
        .map_err(|error| error.to_string())?;
    let entry_sha = oa_sha256::to_hex(&oa_sha256::sha256(&cifp_bytes));
    let cifp = parse_bytes(&cifp_bytes, CIFP_ENTRY).map_err(|error| error.to_string())?;
    if !is_clean(&cifp) {
        return Err(
            "CIFP parser recorded errors; route-chain listing refuses partial input".into(),
        );
    }

    let mut stars = Vec::new();
    let mut star_starts = BTreeSet::new();
    let mut selector_entries = 0usize;
    let mut diagnostic = RouteListingDiagnostics::default();
    let mut scanned_route_groups = 0usize;
    for procedure in oa_arinc424::procedures_at(&cifp, "KORD", CifpProcedureKind::Star) {
        for route in &procedure.routes {
            if route.route_type != '4' {
                continue;
            }
            scanned_route_groups = scanned_route_groups.saturating_add(1);
            if scanned_route_groups > MAX_DIAGNOSTIC_ROUTES {
                return Err(
                    "route-list diagnostic route-group cap exceeded; no partial output emitted"
                        .into(),
                );
            }
            diagnostic.count("star.type4_transition_groups")?;
            let base = ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Star,
                procedure: procedure.ident.clone(),
                transition: route.transition.clone(),
                route_type: Some('4'),
            };
            let (first, _) = match oa_demo_scenario::resolve_procedure_route(&cifp, &base) {
                Ok(resolved) => {
                    let Some(first) = resolved.fixes.first().map(|fix| fix.ident.clone()) else {
                        diagnostic.count("star.transition.empty_resolved_route")?;
                        continue;
                    };
                    let Some(last) = resolved.fixes.last().map(|fix| fix.ident.clone()) else {
                        diagnostic.count("star.transition.empty_resolved_route")?;
                        continue;
                    };
                    (first, last)
                }
                Err(error) => {
                    diagnostic.resolver_refusal("star.transition.resolve", &error)?;
                    continue;
                }
            };
            let common_only = match expand_star_refs(&cifp, &base, None, Some(&mut diagnostic)) {
                Ok(references) => references,
                Err(_) => {
                    diagnostic.count("star.common.expansion_refusal")?;
                    continue;
                }
            };
            let common_route = match oa_demo_scenario::resolve_route_chain(&cifp, &common_only) {
                Ok(resolved) => resolved,
                Err(error) => {
                    diagnostic.resolver_refusal("star.common.resolve", &error)?;
                    continue;
                }
            };
            let Some(common_end) = common_route.fixes.last().map(|fix| fix.ident.clone()) else {
                diagnostic.count("star.common.empty_resolved_route")?;
                continue;
            };
            star_starts.insert(first.clone());
            for runway_route in &procedure.routes {
                if runway_route.route_type != '6' || runway_route.transition.is_empty() {
                    continue;
                }
                diagnostic.count("star.type6_runway_candidates")?;
                let runway = (runway_route.route_type, runway_route.transition.clone());
                let full_refs = match expand_star_refs(
                    &cifp,
                    &base,
                    Some(runway.clone()),
                    Some(&mut diagnostic),
                ) {
                    Ok(references) => references,
                    Err(_) => {
                        diagnostic.count("star.runway.expansion_refusal")?;
                        continue;
                    }
                };
                let full_route = match oa_demo_scenario::resolve_route_chain(&cifp, &full_refs) {
                    Ok(resolved) => resolved,
                    Err(error) => {
                        diagnostic.resolver_refusal("star.runway.resolve", &error)?;
                        continue;
                    }
                };
                if full_route.fixes.first().map(|fix| &fix.ident) != Some(&first)
                    || common_end.is_empty()
                {
                    diagnostic.count("star.runway.endpoint_join_refusal")?;
                    continue;
                }
                let Some(last) = full_route.fixes.last().map(|fix| fix.ident.clone()) else {
                    diagnostic.count("star.runway.empty_resolved_route")?;
                    continue;
                };
                selector_entries = selector_entries.saturating_add(1);
                if selector_entries > MAX_SELECTOR_INDEX_ENTRIES {
                    return Err(
                        "STAR route-chain index cap exceeded; no partial list emitted".into(),
                    );
                }
                diagnostic.count("resolved.star_runway_chains")?;
                stars.push((base.clone(), runway, first.clone(), last, full_refs));
            }
        }
    }
    let mut approaches = Vec::new();
    for procedure in oa_arinc424::procedures_at(&cifp, "KORD", CifpProcedureKind::Approach) {
        for route in &procedure.routes {
            if route.route_type != 'A' || route.transition.is_empty() {
                continue;
            }
            scanned_route_groups = scanned_route_groups.saturating_add(1);
            if scanned_route_groups > MAX_DIAGNOSTIC_ROUTES {
                return Err(
                    "route-list diagnostic route-group cap exceeded; no partial output emitted"
                        .into(),
                );
            }
            diagnostic.count("approach.typeA_transition_groups")?;
            let base = ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Approach,
                procedure: procedure.ident.clone(),
                transition: route.transition.clone(),
                route_type: Some('A'),
            };
            let (first, last_transition) =
                match oa_demo_scenario::resolve_procedure_route(&cifp, &base) {
                    Ok(resolved) => {
                        let Some(first) = resolved.fixes.first().map(|fix| fix.ident.clone())
                        else {
                            diagnostic.count("approach.transition.empty_resolved_route")?;
                            continue;
                        };
                        let Some(last) = resolved.fixes.last().map(|fix| fix.ident.clone()) else {
                            diagnostic.count("approach.transition.empty_resolved_route")?;
                            continue;
                        };
                        (first, last)
                    }
                    Err(error) => {
                        diagnostic.resolver_refusal("approach.transition.resolve", &error)?;
                        continue;
                    }
                };
            for final_route in &procedure.routes {
                if !final_route.transition.is_empty() || final_route.route_type == 'A' {
                    continue;
                }
                diagnostic.count("approach.empty_transition_final_groups")?;
                if final_route
                    .legs
                    .first()
                    .and_then(|leg| leg.fix.as_ref())
                    .map(|fix| fix.ident.as_str())
                    != Some(last_transition.as_str())
                {
                    diagnostic.count("approach.final_start_mismatch")?;
                    continue;
                }
                let final_type = final_route.route_type;
                let full_refs = match expand_approach_refs(
                    &cifp,
                    &base,
                    Some(final_type),
                    Some(&mut diagnostic),
                ) {
                    Ok(references) => references,
                    Err(_) => {
                        diagnostic.count("approach.final.expansion_refusal")?;
                        continue;
                    }
                };
                let full_route = match oa_demo_scenario::resolve_route_chain(&cifp, &full_refs) {
                    Ok(resolved) => resolved,
                    Err(error) => {
                        diagnostic.resolver_refusal("approach.final.resolve", &error)?;
                        continue;
                    }
                };
                let Some(last) = full_route.fixes.last().map(|fix| fix.ident.clone()) else {
                    diagnostic.count("approach.final.empty_resolved_route")?;
                    continue;
                };
                approaches.push((base.clone(), final_type, first.clone(), last, full_refs));
                diagnostic.count("resolved.approach_transition_final_chains")?;
                selector_entries = selector_entries.saturating_add(1);
                if selector_entries > MAX_SELECTOR_INDEX_ENTRIES {
                    return Err(
                        "approach route-chain index cap exceeded; no partial list emitted".into(),
                    );
                }
            }
        }
    }
    if stars.is_empty() || approaches.is_empty() {
        let reason = match (stars.is_empty(), approaches.is_empty()) {
            (true, true) => "no fully resolvable STAR runway chain or approach transition + published final chain",
            (true, false) => "no fully resolvable KORD type-4 STAR transition + common + type-6 runway chain",
            (false, true) => "no resolvable KORD approach transition + published final route chain",
            (false, false) => "route candidate groups were unexpectedly empty",
        };
        return Err(format!("{reason}\n{}", diagnostic.render()?));
    }

    let mut airway_counts: BTreeMap<String, usize> = BTreeMap::new();
    for airway in &cifp.airways {
        *airway_counts.entry(airway.ident.clone()).or_default() += 1;
    }
    if airway_counts.len() > MAX_AIRWAY_IDENTIFIERS {
        return Err("airway identifier cap exceeded; no partial list emitted".into());
    }
    let mut airway_to_start: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut airway_scans = 0usize;
    for airway in &cifp.airways {
        if airway_counts.get(&airway.ident) != Some(&1) {
            continue;
        }
        for (index, fix) in airway.fixes.iter().enumerate() {
            airway_scans = airway_scans.saturating_add(1);
            if airway_scans > MAX_AIRWAY_FIX_SCANS {
                return Err("airway scan cap exceeded; no partial list emitted".into());
            }
            if !star_starts.contains(&fix.fix.ident) {
                continue;
            }
            for neighbor_index in [index.checked_sub(1), index.checked_add(1)]
                .into_iter()
                .flatten()
            {
                let Some(neighbor) = airway.fixes.get(neighbor_index) else {
                    continue;
                };
                let entry = airway_to_start.entry(fix.fix.ident.clone()).or_default();
                let segment = (airway.ident.clone(), neighbor.fix.ident.clone());
                if !entry.contains(&segment) {
                    entry.push(segment);
                }
                selector_entries = selector_entries.saturating_add(1);
                if selector_entries > MAX_SELECTOR_INDEX_ENTRIES {
                    return Err(
                        "airway route-chain index cap exceeded; no partial list emitted".into(),
                    );
                }
            }
        }
    }

    if first_only {
        let mut pair_checks = 0usize;
        let mut chain_checks = 0usize;
        let mut selected = None;
        'stars: for (star, runway, star_first, star_last, star_refs) in &stars {
            let Some(airway_segments) = airway_to_start.get(star_first) else {
                diagnostic.count("pair.star_has_no_adjacent_airway")?;
                continue;
            };
            for (approach, final_type, approach_first, approach_last, approach_refs) in &approaches
            {
                pair_checks = pair_checks.saturating_add(1);
                if pair_checks > MAX_CANDIDATE_CHECKS {
                    return Err(
                        "route-pair checks exceed fixed cap; no partial selection emitted".into(),
                    );
                }
                diagnostic.count("pair.star_approach_pairs")?;
                if !runway_transition_matches_approach(&runway.1, approach_last) {
                    diagnostic.count("pair.runway_compatibility_refusal")?;
                    continue;
                }
                let synthetic_direct = star_last != approach_first;
                let mut refs = Vec::with_capacity(
                    1 + star_refs.len() + approach_refs.len() + usize::from(synthetic_direct),
                );
                let candidate = first_verified_candidate(
                    airway_segments.iter(),
                    |(airway, airway_from)| {
                        chain_checks = chain_checks.saturating_add(1);
                        if chain_checks > MAX_CANDIDATE_CHECKS {
                            return Err(
                                "complete route-chain checks exceed fixed cap; no partial selection emitted"
                                    .into(),
                            );
                        }
                        diagnostic.count("pair.complete_chain_attempts")?;
                        refs.clear();
                        refs.push(ProcedureRouteRef {
                            airport: "ENRT".into(),
                            kind: ProcedureKind::Airway,
                            procedure: airway.clone(),
                            transition: format!("{airway_from}>{star_first}"),
                            route_type: None,
                        });
                        refs.extend(star_refs.iter().cloned());
                        if synthetic_direct {
                            refs.push(ProcedureRouteRef {
                                airport: "SYN".into(),
                                kind: ProcedureKind::Direct,
                                procedure: "DCT".into(),
                                transition: format!("{star_last}>{approach_first}"),
                                route_type: None,
                            });
                        }
                        refs.extend(approach_refs.iter().cloned());
                        let resolved = match oa_demo_scenario::resolve_route_chain(&cifp, &refs) {
                            Ok(route) => route,
                            Err(error) => {
                                diagnostic
                                    .resolver_refusal("pair.complete_chain.resolve", &error)?;
                                return Ok(None);
                            }
                        };
                        if let Err(error) = validate_route_motion(
                            &resolved,
                            normal_motion_assumptions(),
                            B738_NG_VMO_KCAS,
                        ) {
                            diagnostic.count(&format!(
                                "pair.complete_chain.motion.{}",
                                traffic_error_class(&error)
                            ))?;
                            return Ok(None);
                        }
                        diagnostic.count("pair.complete_chain.motion_validated")?;
                        Ok(Some((
                            (
                                airway.clone(),
                                airway_from.clone(),
                                star.procedure.clone(),
                                star.transition.clone(),
                                runway.0,
                                runway.1.clone(),
                                approach.procedure.clone(),
                                approach.transition.clone(),
                                *final_type,
                                synthetic_direct,
                            ),
                            chain_checks,
                            star_first.clone(),
                        )))
                    },
                )?;
                if let Some((key, verified_chains, selected_star_first)) = candidate {
                    selected = Some((key, verified_chains, selected_star_first));
                    break 'stars;
                }
            }
        }

        let Some((key, verified_chains, selected_star_first)) = selected else {
            return Err(format!(
                "no complete resolvable route chain found under the declared route composition; first-route selection exhausted its bounded source-order scan\n{}",
                diagnostic.render()?
            ));
        };
        let mut out = String::new();
        writeln!(
            out,
            "KORD first complete route: archive_sha256={archive_sha} entry_sha256={entry_sha} cycle={} airport=KORD",
            cifp_cycle(&cifp).map_or_else(|| "unknown".into(), |cycle| cycle.to_string())
        )
        .map_err(|_| "route-chain formatting failed")?;
        writeln!(out, "Selection is the first resolver-, runway-, and bounded-motion-validated composition in deterministic CIFP source order; the complete route population is not enumerated.")
            .map_err(|_| "route-chain formatting failed")?;
        writeln!(out, "The route uses the published STAR type-4 transition, its matching type-5 common route and type-6 runway transition, an adjacent published airway segment, and the approach transition plus its matching empty-transition final route; a synthetic direct connector is explicitly marked when needed.")
            .map_err(|_| "route-chain formatting failed")?;
        write_route_chain_command(&mut out, &key, &selected_star_first)?;
        writeln!(
            out,
            "Selection summary: mode=first-source-order complete_chain_checks_before_selection={verified_chains} total_population=not_enumerated"
        )
        .map_err(|_| "route-chain formatting failed")?;
        out.push_str(&diagnostic.render()?);
        if out.len() > MAX_CANDIDATE_OUTPUT_BYTES {
            return Err(
                "first route-chain output cap exceeded; no partial selection emitted".into(),
            );
        }
        print!("{out}");
        return Ok(());
    }

    let mut out = String::new();
    writeln!(
        out,
        "KORD complete-route candidates: archive_sha256={archive_sha} entry_sha256={entry_sha} cycle={} airport=KORD",
        cifp_cycle(&cifp).map_or_else(|| "unknown".into(), |cycle| cycle.to_string())
    )
    .map_err(|_| "route-chain formatting failed")?;
    writeln!(out, "Each listed chain uses published CIFP airway/STAR/approach segments whose STAR runway transition matches the resolved approach-final runway; `--synthetic-direct` explicitly marks the straight synthetic flight-plan connector when STAR and approach fixes differ.")
        .map_err(|_| "route-chain formatting failed")?;
    writeln!(out, "The published STAR type-4 transition is followed by its matching type-5 common route and selected type-6 runway transition; the approach transition is followed by its matching empty-transition final route.")
        .map_err(|_| "route-chain formatting failed")?;
    writeln!(out, "Normal-final fixes stop at the unique explicit col-43 published MAP marker; later approach-final rows are missed-approach branches and are not materialized.")
        .map_err(|_| "route-chain formatting failed")?;
    writeln!(out, "Generated positions are geodesic fix-to-fix samples, not an execution model for CIFP CF/CA/VI/VM/FM/hold path terminators; route selectors do not assert operational procedure compliance.")
        .map_err(|_| "route-chain formatting failed")?;

    let mut pair_checks = 0usize;
    let mut chain_checks = 0usize;
    let mut listed = 0usize;
    let mut seen = BTreeSet::new();
    for (star, runway, star_first, star_last, star_refs) in &stars {
        let Some(airway_segments) = airway_to_start.get(star_first) else {
            diagnostic.count("pair.star_has_no_adjacent_airway")?;
            continue;
        };
        for (approach, final_type, approach_first, approach_last, approach_refs) in &approaches {
            pair_checks = pair_checks.saturating_add(1);
            if pair_checks > MAX_CANDIDATE_CHECKS {
                return Err("route-pair checks exceed fixed cap; no partial list emitted".into());
            }
            diagnostic.count("pair.star_approach_pairs")?;
            if !runway_transition_matches_approach(&runway.1, approach_last) {
                diagnostic.count("pair.runway_compatibility_refusal")?;
                continue;
            }
            let dct = star_last != approach_first;
            let mut refs =
                Vec::with_capacity(1 + star_refs.len() + approach_refs.len() + usize::from(dct));
            for (airway, airway_from) in airway_segments {
                chain_checks = chain_checks.saturating_add(1);
                if chain_checks > MAX_CANDIDATE_CHECKS {
                    return Err(
                        "complete route-chain checks exceed fixed cap; no partial list emitted"
                            .into(),
                    );
                }
                diagnostic.count("pair.complete_chain_attempts")?;
                let airway_ref = ProcedureRouteRef {
                    airport: "ENRT".into(),
                    kind: ProcedureKind::Airway,
                    procedure: airway.clone(),
                    transition: format!("{airway_from}>{star_first}"),
                    route_type: None,
                };
                refs.clear();
                refs.push(airway_ref);
                refs.extend(star_refs.iter().cloned());
                if dct {
                    refs.push(ProcedureRouteRef {
                        airport: "SYN".into(),
                        kind: ProcedureKind::Direct,
                        procedure: "DCT".into(),
                        transition: format!("{star_last}>{approach_first}"),
                        route_type: None,
                    });
                }
                refs.extend(approach_refs.iter().cloned());
                if let Err(error) = oa_demo_scenario::resolve_route_chain(&cifp, &refs) {
                    diagnostic.resolver_refusal("pair.complete_chain.resolve", &error)?;
                    continue;
                }
                let key = (
                    airway.clone(),
                    airway_from.clone(),
                    star.procedure.clone(),
                    star.transition.clone(),
                    runway.0,
                    runway.1.clone(),
                    approach.procedure.clone(),
                    approach.transition.clone(),
                    *final_type,
                    dct,
                );
                if !seen.insert(key.clone()) {
                    continue;
                }
                increment_listed_route_count(&mut listed)?;
                write_route_chain_command(&mut out, &key, star_first)?;
                if out.len() > MAX_CANDIDATE_OUTPUT_BYTES {
                    return Err(
                        "complete route-chain output cap exceeded; no partial list emitted".into(),
                    );
                }
            }
        }
    }
    if listed == 0 {
        return Err(format!(
            "no complete resolvable route chain found under the declared route composition\n{}",
            diagnostic.render()?
        ));
    }
    out.push_str(&diagnostic.render()?);
    print!("{out}");
    Ok(())
}

fn first_verified_candidate<T, R>(
    candidates: impl IntoIterator<Item = T>,
    mut verify: impl FnMut(T) -> Result<Option<R>, String>,
) -> Result<Option<R>, String> {
    for candidate in candidates {
        if let Some(verified) = verify(candidate)? {
            return Ok(Some(verified));
        }
    }
    Ok(None)
}

fn runway_transition_matches_approach(star_transition: &str, approach_final_fix: &str) -> bool {
    fn runway_code(value: &str) -> Option<(u8, Option<char>)> {
        let suffix = value.strip_prefix("RW")?;
        if suffix.len() != 2 && suffix.len() != 3 {
            return None;
        }
        let runway_number = suffix.get(..2)?.parse::<u8>().ok()?;
        if !(1..=36).contains(&runway_number) {
            return None;
        }
        let designator = match suffix.as_bytes().get(2) {
            None => None,
            Some(b'L' | b'R' | b'C' | b'B') => Some(char::from(*suffix.as_bytes().get(2)?)),
            _ => return None,
        };
        Some((runway_number, designator))
    }

    let (star_number, star_designator) = match runway_code(star_transition) {
        Some(code) => code,
        None => return false,
    };
    let (approach_number, approach_designator) = match runway_code(approach_final_fix) {
        Some(code) => code,
        None => return false,
    };
    if star_number != approach_number {
        return false;
    }
    match (star_designator, approach_designator) {
        (Some('B'), Some('L' | 'R')) => true,
        (Some(star), Some(approach)) => star == approach && star != 'B',
        (None, None) => true,
        _ => false,
    }
}

fn increment_listed_route_count(listed: &mut usize) -> Result<(), String> {
    *listed = listed.saturating_add(1);
    if *listed > MAX_LISTED_CANDIDATES {
        return Err("complete route-chain candidate cap exceeded; no partial list emitted".into());
    }
    Ok(())
}

fn write_route_chain_command(
    out: &mut String,
    key: &(
        String,
        String,
        String,
        String,
        char,
        String,
        String,
        String,
        char,
        bool,
    ),
    star_first: &str,
) -> Result<(), String> {
    write!(
        out,
        "--airway {} {} {} --star {} 4 {} --approach {} A {} --star-runway {} {} --approach-final {}",
        key.0, key.1, star_first, key.2, key.3, key.6, key.7, key.4, key.5, key.8
    )
    .map_err(|_| "route-chain formatting failed")?;
    if key.9 {
        out.push_str(" --synthetic-direct");
    }
    out.push('\n');
    Ok(())
}

/// Prints bounded, source-derived KORD route endpoints and resolver refusal categories. This is
/// diagnostic only; it does not suggest connectors or synthesize missing route geometry.
fn diagnose_candidates(cifp_path: &Path) -> Result<(), String> {
    let (archive_bytes, archive_sha) = read_guarded(cifp_path, MAX_CIFP_ARCHIVE)?;
    if cifp_path.file_name().and_then(|value| value.to_str()) != Some(CIFP_NAME) {
        return Err(format!("CIFP input must be named {CIFP_NAME}"));
    }
    if archive_sha != CIFP_SHA256 {
        return Err(format!(
            "CIFP archive SHA-256 mismatch: expected {CIFP_SHA256}, found {archive_sha}"
        ));
    }
    let archive = oa_zip::Archive::parse(&archive_bytes).map_err(|error| error.to_string())?;
    let entry = archive
        .entries()
        .iter()
        .find(|entry| entry.name() == CIFP_ENTRY)
        .ok_or_else(|| format!("pinned archive has no {CIFP_ENTRY} entry"))?;
    let cifp_bytes = archive
        .extract_entry(entry)
        .map_err(|error| error.to_string())?;
    let entry_sha = oa_sha256::to_hex(&oa_sha256::sha256(&cifp_bytes));
    let cifp = parse_bytes(&cifp_bytes, CIFP_ENTRY).map_err(|error| error.to_string())?;
    if !is_clean(&cifp) {
        return Err("CIFP parser recorded errors; diagnostic refuses partial input".into());
    }

    let mut out = String::new();
    writeln!(
        out,
        "KORD route diagnostic: archive_sha256={archive_sha} entry_sha256={entry_sha} cycle={} airport=KORD",
        cifp_cycle(&cifp).map_or_else(|| "unknown".into(), |cycle| cycle.to_string())
    )
    .map_err(|_| "diagnostic formatting failed")?;
    writeln!(out, "Public CIFP selectors/endpoints and parser terminators only; refusal is not a route recommendation.")
        .map_err(|_| "diagnostic formatting failed")?;

    let mut rows = 0usize;
    let mut legs_scanned = 0usize;
    let mut star_last: BTreeMap<String, usize> = BTreeMap::new();
    let mut star_first: BTreeMap<String, usize> = BTreeMap::new();
    let mut approach_first: BTreeMap<String, usize> = BTreeMap::new();
    let mut refusal_counts: BTreeMap<String, usize> = BTreeMap::new();
    for kind in [CifpProcedureKind::Star, CifpProcedureKind::Approach] {
        for procedure in oa_arinc424::procedures_at(&cifp, "KORD", kind) {
            for route in &procedure.routes {
                rows = rows.saturating_add(1);
                legs_scanned = legs_scanned.saturating_add(route.legs.len());
                if rows > MAX_DIAGNOSTIC_ROUTES || legs_scanned > MAX_DIAGNOSTIC_LEGS {
                    return Err(
                        "diagnostic route/leg cap exceeded; no partial output emitted".into(),
                    );
                }
                let first = route
                    .legs
                    .first()
                    .and_then(|leg| leg.fix.as_ref())
                    .map(|fix| fix.ident.clone());
                let last = route
                    .legs
                    .last()
                    .and_then(|leg| leg.fix.as_ref())
                    .map(|fix| fix.ident.clone());
                let kind_label = match kind {
                    CifpProcedureKind::Star => "STAR",
                    CifpProcedureKind::Approach => "APPROACH",
                    CifpProcedureKind::Sid => continue,
                };
                if kind == CifpProcedureKind::Star {
                    if let Some(first) = &first {
                        *star_first.entry(first.clone()).or_default() += 1;
                    }
                    if let Some(last) = &last {
                        *star_last.entry(last.clone()).or_default() += 1;
                    }
                } else if let Some(first) = &first {
                    *approach_first.entry(first.clone()).or_default() += 1;
                }
                let reference = ProcedureRouteRef {
                    airport: "KORD".into(),
                    kind: match kind {
                        CifpProcedureKind::Star => ProcedureKind::Star,
                        CifpProcedureKind::Approach => ProcedureKind::Approach,
                        CifpProcedureKind::Sid => continue,
                    },
                    procedure: procedure.ident.clone(),
                    transition: route.transition.clone(),
                    route_type: Some(route.route_type),
                };
                let resolution = match oa_demo_scenario::resolve_procedure_route(&cifp, &reference)
                {
                    Ok(_) => "OK".to_string(),
                    Err(error) => {
                        let category = format!("{error:?}");
                        let bucket = category
                            .split('(')
                            .next()
                            .unwrap_or("unknown refusal")
                            .to_string();
                        *refusal_counts.entry(bucket).or_default() += 1;
                        category
                    }
                };
                let mut terminators = String::new();
                for leg in &route.legs {
                    if !terminators.is_empty() {
                        terminators.push(',');
                    }
                    write!(terminators, "{:?}", leg.path_terminator)
                        .map_err(|_| "diagnostic formatting failed")?;
                    if out.len().saturating_add(terminators.len()) > MAX_DIAGNOSTIC_OUTPUT_BYTES {
                        return Err(
                            "diagnostic output cap exceeded; no partial output emitted".into()
                        );
                    }
                }
                writeln!(
                    out,
                    "{kind_label}|{}|{}|{:?}|first={}|last={}|terms={terminators}|resolve={resolution}",
                    procedure.ident,
                    route.route_type,
                    route.transition,
                    first.as_deref().unwrap_or("-"),
                    last.as_deref().unwrap_or("-")
                )
                .map_err(|_| "diagnostic formatting failed")?;
                if out.len() > MAX_DIAGNOSTIC_OUTPUT_BYTES {
                    return Err("diagnostic output cap exceeded; no partial output emitted".into());
                }
            }
        }
    }
    let exact_joins = star_last.iter().fold(0usize, |total, (fix, star_routes)| {
        total.saturating_add(
            star_routes.saturating_mul(approach_first.get(fix).copied().unwrap_or(0)),
        )
    });
    writeln!(out, "SUMMARY|routes={rows}|legs={legs_scanned}|STAR-last/APP-first exact route joins={exact_joins}")
        .map_err(|_| "diagnostic formatting failed")?;
    for (category, count) in refusal_counts {
        writeln!(out, "REFUSAL|{category}|{count}").map_err(|_| "diagnostic formatting failed")?;
    }
    for (fix, count) in &star_first {
        if *count > 0 {
            writeln!(out, "STAR-FIRST|{fix}|routes={count}")
                .map_err(|_| "diagnostic formatting failed")?;
        }
    }
    for (fix, count) in &approach_first {
        if *count > 0 {
            writeln!(out, "APP-FIRST|{fix}|routes={count}")
                .map_err(|_| "diagnostic formatting failed")?;
        }
    }
    if out.len() > MAX_DIAGNOSTIC_OUTPUT_BYTES {
        return Err("diagnostic output cap exceeded; no partial output emitted".into());
    }
    print!("{out}");
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    All,
    Oasd,
}

struct EncodedAds3Frame {
    filename: String,
    start_row: usize,
    row_count: usize,
    payload: Vec<u8>,
    sha256: String,
}

struct Args {
    cifp: PathBuf,
    performance_spec: PathBuf,
    performance_sha256: String,
    airway: String,
    airway_from: String,
    airway_to: String,
    star: String,
    star_route_type: char,
    star_transition: String,
    star_runway: Option<(char, String)>,
    approach: String,
    approach_route_type: char,
    approach_transition: String,
    approach_final_type: Option<char>,
    synthetic_direct: bool,
    outputs: OutputMode,
    four_streams: bool,
    output_root: Option<PathBuf>,
}

impl Args {
    fn parse(raw: Vec<String>) -> Result<Self, String> {
        if raw.len() < 18 {
            return Err("usage: oa-demo-scenario-prep --list-candidates --cifp PINNED_CIFP_261029.zip | --list-route-chains --cifp PINNED_CIFP_261029.zip | --first-route-chain --cifp PINNED_CIFP_261029.zip | --diagnose-candidates --cifp PINNED_CIFP_261029.zip | --cifp PATH --performance-spec PATH --performance-sha256 HEX --airway IDENT FROM TO --star IDENT TYPE TRANSITION [--star-runway TYPE TRANSITION] --approach IDENT TYPE TRANSITION [--approach-final TYPE] [--synthetic-direct] [--outputs all|oasd]".into());
        }
        let mut values = raw.into_iter();
        if take_next(&mut values)? != "--cifp" {
            return Err("expected --cifp".into());
        }
        let cifp = PathBuf::from(take_next(&mut values)?);
        if take_next(&mut values)? != "--performance-spec" {
            return Err("expected --performance-spec".into());
        }
        let performance_spec = PathBuf::from(take_next(&mut values)?);
        if take_next(&mut values)? != "--performance-sha256" {
            return Err("expected --performance-sha256".into());
        }
        let performance_sha256 = take_next(&mut values)?;
        if performance_sha256.len() != 64
            || !performance_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("performance SHA-256 must contain 64 hex digits".into());
        }
        if take_next(&mut values)? != "--airway" {
            return Err("expected --airway".into());
        }
        let airway = take_next(&mut values)?;
        let airway_from = take_next(&mut values)?;
        let airway_to = take_next(&mut values)?;
        if take_next(&mut values)? != "--star" {
            return Err("expected --star".into());
        }
        let star = take_next(&mut values)?;
        let star_route_type =
            one_char(&take_next(&mut values)?).ok_or("STAR route type must be one character")?;
        let star_transition = take_next(&mut values)?;
        if take_next(&mut values)? != "--approach" {
            return Err("expected --approach".into());
        }
        let approach = take_next(&mut values)?;
        let approach_route_type = one_char(&take_next(&mut values)?)
            .ok_or("approach route type must be one character")?;
        let approach_transition = take_next(&mut values)?;
        let mut star_runway = None;
        let mut approach_final_type = None;
        let mut synthetic_direct = false;
        let mut outputs = OutputMode::All;
        let mut four_streams = false;
        let mut output_root = None;
        let mut outputs_seen = false;
        while let Some(flag) = values.next() {
            match flag.as_str() {
                "--star-runway" => {
                    if star_runway.is_some() {
                        return Err("--star-runway may be supplied once".into());
                    }
                    let route_type = one_char(&take_next(&mut values)?)
                        .ok_or("STAR runway route type must be one character")?;
                    let transition = take_next(&mut values)?;
                    star_runway = Some((route_type, transition));
                }
                "--approach-final" => {
                    if approach_final_type.is_some() {
                        return Err("--approach-final may be supplied once".into());
                    }
                    approach_final_type = Some(
                        one_char(&take_next(&mut values)?)
                            .ok_or("approach final route type must be one character")?,
                    );
                }
                "--synthetic-direct" => {
                    if synthetic_direct {
                        return Err("--synthetic-direct may be supplied once".into());
                    }
                    synthetic_direct = true;
                }
                "--outputs" => {
                    if outputs_seen {
                        return Err("--outputs may be supplied once".into());
                    }
                    outputs_seen = true;
                    outputs = match take_next(&mut values)?.as_str() {
                        "all" => OutputMode::All,
                        "oasd" => OutputMode::Oasd,
                        _ => return Err("--outputs must be all or oasd".into()),
                    };
                }
                "--four-streams" => four_streams = true,
                "--out-root" => output_root = Some(PathBuf::from(take_next(&mut values)?)),
                _ => return Err(format!("unexpected option {flag}")),
            }
        }
        Ok(Self {
            cifp,
            performance_spec,
            performance_sha256,
            airway,
            airway_from,
            airway_to,
            star,
            star_route_type,
            star_transition,
            star_runway,
            approach,
            approach_route_type,
            approach_transition,
            approach_final_type,
            synthetic_direct,
            outputs,
            four_streams,
            output_root,
        })
    }
}

fn take_next(values: &mut impl Iterator<Item = String>) -> Result<String, String> {
    values.next().ok_or_else(|| "missing argument".to_string())
}

fn read_guarded(path: &Path, max_bytes: u64) -> Result<(Vec<u8>, String), String> {
    let (file, length) =
        oa_sealed::open_regular(path).map_err(|e| format!("guarded input refused: {e}"))?;
    if length == 0 || length > max_bytes {
        return Err(format!("input size {length} outside 1..={max_bytes} bytes"));
    }
    let capacity = usize::try_from(length).map_err(|_| "input does not fit memory index")?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() != capacity {
        return Err("input length changed while reading".into());
    }
    let digest = oa_sha256::to_hex(&oa_sha256::sha256(&bytes));
    Ok((bytes, digest))
}

fn reports_from_synthetic(samples: &[SyntheticReport]) -> Result<Reports, String> {
    let mut reports = Reports::default();
    for sample in samples {
        if sample.aircraft_index >= NORMAL_ORD_AIRCRAFT {
            return Err("scenario-owned aircraft index is outside the fixed baseline".into());
        }
        let time_ns = SYNTHETIC_EPOCH_NS
            .checked_add(
                sample
                    .virtual_time_ms
                    .checked_mul(1_000_000)
                    .ok_or("synthetic time conversion overflow")?,
            )
            .ok_or("synthetic time conversion overflow")?;
        if !(oa_adsb::TIME_MIN_NS..oa_adsb::TIME_MAX_NS).contains(&time_ns) {
            return Err("synthetic time is outside the ADS-B tool range".into());
        }
        let track_cdeg = sample.track_ddeg.saturating_mul(10);
        let speed_kt = sample.ground_speed_dkt / 10;
        let row = Row {
            time_ns,
            icao: NON_ICAO | (0xA00000 + u32::from(sample.aircraft_index)),
            lat_e7: sample.lat_e7,
            lon_e7: sample.lon_e7,
            baro_alt_ft: sample.altitude_ft,
            geom_alt_ft: NONE_I32,
            vrate_fpm: sample.vertical_rate_fpm,
            gs_dkt: sample.ground_speed_dkt,
            track_ddeg: sample.track_ddeg,
            squawk: NONE_U16,
            flags: if sample.flags & 1 != 0 { F_NEW_LEG } else { 0 },
            source: 8,
            callsign: [0; 8],
            category: 0xA3,
            nic: 11,
            nac_p: 11,
            nac_v: 4,
            sil: 3,
            sil_type: SIL_PER_SAMPLE,
            version: 2,
            gva: 2,
            sda: 2,
            nic_baro: 1,
            ws_kt: NONE_U16,
            wd_deg: NONE_U16,
            tas_kt: speed_kt,
            ias_kt: speed_kt,
            mach_milli: NONE_U16,
            mag_heading_cdeg: NONE_U16,
            true_heading_cdeg: track_cdeg,
            oat_c: NONE_I16,
            tat_c: NONE_I16,
            roll_cdeg: NONE_I16,
            track_rate_cdps: NONE_I16,
        };
        reports_push(&mut reports, &row);
    }
    Ok(reports)
}

fn encode_ads3_frames(
    reports: &Reports,
    total_rows: usize,
) -> Result<Vec<EncodedAds3Frame>, String> {
    const MAX_TOTAL_ROWS: usize = 360_000;
    const MAX_FRAMES: usize = MAX_TOTAL_ROWS.div_ceil(MAX_ROWS_PER_FRAME);
    if total_rows == 0 || total_rows > MAX_TOTAL_ROWS {
        return Err("ADS3 source row count is outside 1..=360000".into());
    }
    let mut frames = Vec::with_capacity(total_rows.div_ceil(MAX_ROWS_PER_FRAME));
    let mut start = 0usize;
    while start < total_rows {
        let remaining = total_rows
            .checked_sub(start)
            .ok_or("ADS3 row range underflow")?;
        let count = remaining.min(MAX_ROWS_PER_FRAME);
        let end = start.checked_add(count).ok_or("ADS3 row range overflow")?;
        let mut payload = Vec::new();
        encode_payload(&mut payload, reports, start, end).map_err(|e| e.to_string())?;
        let sequence = frames
            .len()
            .checked_add(1)
            .ok_or("ADS3 frame sequence overflow")?;
        if sequence > MAX_FRAMES {
            return Err("ADS3 frame count exceeds bounded eight-file manifest".into());
        }
        let filename = if sequence == 1 {
            "ord-normal-push-v1.ads3".to_string()
        } else {
            format!("ord-normal-push-v1.ads3.{sequence:04}")
        };
        let sha256 = oa_sha256::to_hex(&oa_sha256::sha256(&payload));
        frames.push(EncodedAds3Frame {
            filename,
            start_row: start,
            row_count: count,
            payload,
            sha256,
        });
        start = end;
    }
    let conserved = frames.iter().try_fold(0usize, |sum, frame| {
        sum.checked_add(frame.row_count)
            .ok_or("ADS3 row conservation overflow")
    })?;
    if conserved != total_rows {
        return Err("ADS3 frame manifest does not conserve the complete source rows".into());
    }
    Ok(frames)
}

fn ads3_frame_inventory(frames: &[EncodedAds3Frame]) -> (String, String) {
    let mut inventory = String::new();
    for (index, frame) in frames.iter().enumerate() {
        let _ = writeln!(
            inventory,
            "{:04}\t{}\t{}\t{}\t{}\t{}",
            index + 1,
            frame.filename,
            frame.start_row,
            frame.row_count,
            frame.payload.len(),
            frame.sha256,
        );
    }
    let digest = oa_sha256::to_hex(&oa_sha256::sha256(inventory.as_bytes()));
    (digest, inventory)
}

/// Generate the fixed demo map from the already parsed, pinned public CIFP only.
fn public_map_rust(
    cifp: &oa_arinc424::Cifp,
    routes: &[oa_demo_scenario::core::ResolvedRoute],
) -> Result<String, String> {
    let mut out = format!("// Generated from {CIFP_NAME}, SHA-256 {CIFP_SHA256}.\n// Public CIFP geometry only. Straight fix-to-fix route drawing; synthetic DCTs are dashed.\n#![allow(clippy::type_complexity)] // Cold generated constant geometry.\npub const ROUTES: &[&[(i32, i32, &str, bool)]] = &[\n");
    let entries = ["IRK", "FWA", "ZZIPR", "FNT"];
    for (route, entry) in routes.iter().zip(entries) {
        let start = route
            .fixes
            .iter()
            .position(|fix| fix.ident == entry)
            .ok_or("map STAR entry missing")?;
        out.push_str("    &[\n");
        for (index, fix) in route.fixes.iter().enumerate().skip(start) {
            let synthetic = index
                .checked_sub(1)
                .and_then(|n| route.fixes.get(n))
                .is_some_and(|previous| {
                    route.reference.iter().any(|reference| {
                        reference.kind == ProcedureKind::Direct
                            && reference.transition == format!("{}>{}", previous.ident, fix.ident)
                    })
                });
            writeln!(
                out,
                "        ({}, {}, {:?}, {}),",
                degree_to_e7(fix.lat_deg)?,
                degree_to_e7(fix.lon_deg)?,
                fix.ident,
                synthetic
            )
            .map_err(|e| e.to_string())?;
        }
        out.push_str("    ],\n");
    }
    out.push_str("];\npub const RUNWAYS: &[(i32, i32, i32, i32, &str)] = &[\n");
    for (landing, reciprocal) in [("RW27L", "RW09R"), ("RW28C", "RW10C"), ("RW28R", "RW10L")] {
        let a = oa_arinc424::runways_at(cifp, "KORD")
            .find(|runway| runway.ident == landing)
            .ok_or("map landing runway missing")?;
        let b = oa_arinc424::runways_at(cifp, "KORD")
            .find(|runway| runway.ident == reciprocal)
            .ok_or("map reciprocal runway missing")?;
        writeln!(
            out,
            "    ({}, {}, {}, {}, {:?}),",
            degree_to_e7(a.threshold.lat)?,
            degree_to_e7(a.threshold.lon)?,
            degree_to_e7(b.threshold.lat)?,
            degree_to_e7(b.threshold.lon)?,
            landing
        )
        .map_err(|e| e.to_string())?;
    }
    out.push_str("];\n");
    let ord = oa_arinc424::find_airport(cifp, "KORD").ok_or("map KORD missing")?;
    writeln!(
        out,
        "pub const KORD: (i32, i32) = ({}, {});",
        degree_to_e7(ord.position.lat)?,
        degree_to_e7(ord.position.lon)?
    )
    .map_err(|e| e.to_string())?;
    Ok(out)
}

fn serialize_oasd(
    scenario: &oa_demo_scenario::SyntheticScenario,
    samples: &[SyntheticReport],
    source_hash_hex: [&str; 7],
    center_lat_e7: i32,
    center_lon_e7: i32,
) -> Result<Vec<u8>, String> {
    const HEADER: usize = 352;
    const RECORD: usize = 32;
    const MAX_RECORDS: usize = 360_000;
    const MAX_BYTES: usize = 16 * 1024 * 1024;
    const EPOCH_NS: i64 = SYNTHETIC_EPOCH_NS;
    if samples.is_empty() || samples.len() > MAX_RECORDS {
        return Err("OASD row count is outside 1..=360000".into());
    }
    let total = HEADER
        .checked_add(
            samples
                .len()
                .checked_mul(RECORD)
                .ok_or("OASD size overflow")?,
        )
        .ok_or("OASD size overflow")?;
    if total > MAX_BYTES {
        return Err("OASD artifact exceeds 16 MiB".into());
    }
    let id = scenario.scenario_id.as_bytes();
    if id.is_empty()
        || id.len() > 32
        || !id
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
    {
        return Err("OASD scenario ID must be 1..=32 ASCII identifier bytes".into());
    }
    let source_hashes: [[u8; 32]; 7] = source_hash_hex
        .into_iter()
        .map(parse_sha256_hex)
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| "OASD source hash count mismatch")?;
    if source_hashes
        .iter()
        .any(|hash| hash.iter().all(|b| *b == 0))
    {
        return Err("OASD source SHA-256 digests must be nonzero".into());
    }
    let mut rows = Vec::with_capacity(samples.len() * RECORD);
    let mut previous: Option<(i64, u16)> = None;
    for sample in samples {
        let key = (sample.virtual_time_ms, sample.aircraft_index);
        if previous.is_some_and(|prior| key <= prior) {
            return Err(
                "OASD rows must be strictly ordered by (virtual_time_ms, aircraft_index)".into(),
            );
        }
        previous = Some(key);
        if sample.aircraft_index >= NORMAL_ORD_AIRCRAFT
            || !(-21_600_000..=scenario.duration_ms as i64).contains(&sample.virtual_time_ms)
            || !(-900_000_000..=900_000_000).contains(&sample.lat_e7)
            || !(-1_800_000_000..=1_800_000_000).contains(&sample.lon_e7)
            || !(-2_000..=60_000).contains(&sample.altitude_ft)
            || sample.ground_speed_dkt > 4_000
            || sample.track_ddeg >= 3_600
            || sample.flags & !3 != 0
            || sample.vertical_rate_fpm.unsigned_abs() > 1_800
        {
            return Err("scenario record is outside OASD v1 bounds".into());
        }
        rows.extend_from_slice(&sample.aircraft_index.to_le_bytes());
        rows.extend_from_slice(&sample.flags.to_le_bytes());
        rows.extend_from_slice(&sample.virtual_time_ms.to_le_bytes());
        rows.extend_from_slice(&sample.lat_e7.to_le_bytes());
        rows.extend_from_slice(&sample.lon_e7.to_le_bytes());
        rows.extend_from_slice(&sample.altitude_ft.to_le_bytes());
        rows.extend_from_slice(&sample.ground_speed_dkt.to_le_bytes());
        rows.extend_from_slice(&sample.track_ddeg.to_le_bytes());
        rows.extend_from_slice(&sample.vertical_rate_fpm.to_le_bytes());
    }
    let rows_sha = oa_sha256::sha256(&rows);
    let mut out = vec![0u8; HEADER];
    write_range(&mut out, 0, b"OASD")?;
    put_u16(&mut out, 4, 1)?;
    put_u16(&mut out, 6, HEADER as u16)?;
    put_u16(&mut out, 8, RECORD as u16)?;
    // flags/reserved bytes are zero-initialized.
    write_range(&mut out, 16, id)?;
    put_u64(&mut out, 48, scenario.seed)?;
    put_i64(&mut out, 56, EPOCH_NS)?;
    put_u64(&mut out, 64, scenario.duration_ms)?;
    put_u32(&mut out, 72, 5_000)?;
    put_u32(&mut out, 76, samples.len() as u32)?;
    for (i, digest) in source_hashes.iter().enumerate() {
        let start = 80 + i * 32;
        write_range(&mut out, start, digest)?;
    }
    write_range(&mut out, 304, &rows_sha)?;
    write_range(&mut out, 336, &center_lat_e7.to_le_bytes())?;
    write_range(&mut out, 340, &center_lon_e7.to_le_bytes())?;
    out.extend_from_slice(&rows);
    ScenarioArtifact::parse(&out)
        .map_err(|error| format!("OASD writer self-check refused artifact: {error:?}"))?;
    Ok(out)
}

fn source_bundle(
    cifp_archive_sha: &str,
    cifp_entry_sha: &str,
    performance_sha: &str,
) -> (String, String) {
    let mut entries: Vec<(String, String, String)> =
        Vec::with_capacity(SOURCE_BUNDLE_FILES.len() + 3);
    entries.push((
        "generator data input".into(),
        "FAA/CIFP_261029.zip".into(),
        cifp_archive_sha.to_ascii_lowercase(),
    ));
    entries.push((
        "generator data input".into(),
        "FAA/CIFP_261029.zip!/FAACIFP18".into(),
        cifp_entry_sha.to_ascii_lowercase(),
    ));
    entries.push((
        "generator data input".into(),
        "JCAB/TCDS_No38_Rev12.pdf".into(),
        performance_sha.to_ascii_lowercase(),
    ));
    for (path, bytes, role) in SOURCE_BUNDLE_FILES {
        entries.push((
            (*role).into(),
            (*path).into(),
            oa_sha256::to_hex(&oa_sha256::sha256(bytes)),
        ));
    }
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    let mut inventory = String::new();
    for (role, path, digest) in entries {
        inventory.push_str(&role);
        inventory.push('\t');
        inventory.push_str(&path);
        inventory.push('\t');
        inventory.push_str(&digest);
        inventory.push('\n');
    }
    let digest = oa_sha256::to_hex(&oa_sha256::sha256(inventory.as_bytes()));
    (digest, inventory)
}

fn degree_to_e7(degrees: f64) -> Result<i32, String> {
    let scaled = degrees * 10_000_000.0;
    if !scaled.is_finite() || scaled < i32::MIN as f64 || scaled > i32::MAX as f64 {
        return Err("KORD ARP coordinate cannot be represented as signed E7".into());
    }
    let rounded = scaled.round();
    if rounded < i32::MIN as f64 || rounded > i32::MAX as f64 {
        return Err("rounded KORD ARP coordinate cannot be represented as signed E7".into());
    }
    Ok(rounded as i32)
}

fn parse_sha256_hex(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("OASD source digest must contain 64 hexadecimal digits".into());
    }
    let mut out = [0u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        let pair = value
            .get(index * 2..index * 2 + 2)
            .ok_or("invalid OASD source digest")?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| "invalid OASD source digest")?;
    }
    Ok(out)
}

fn write_range(out: &mut [u8], at: usize, bytes: &[u8]) -> Result<(), String> {
    let end = at
        .checked_add(bytes.len())
        .ok_or("OASD header offset overflow")?;
    let target = out
        .get_mut(at..end)
        .ok_or("OASD header field is out of bounds")?;
    target.copy_from_slice(bytes);
    Ok(())
}

fn put_u16(out: &mut [u8], at: usize, value: u16) -> Result<(), String> {
    write_range(out, at, &value.to_le_bytes())
}

fn put_u32(out: &mut [u8], at: usize, value: u32) -> Result<(), String> {
    write_range(out, at, &value.to_le_bytes())
}

fn put_u64(out: &mut [u8], at: usize, value: u64) -> Result<(), String> {
    write_range(out, at, &value.to_le_bytes())
}

fn put_i64(out: &mut [u8], at: usize, value: i64) -> Result<(), String> {
    write_range(out, at, &value.to_le_bytes())
}

fn one_char(value: &str) -> Option<char> {
    let mut chars = value.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

fn safe_output_root(requested: Option<&Path>) -> Result<PathBuf, String> {
    let root = requested.map_or_else(|| PathBuf::from(OUTPUT_ROOT), Path::to_path_buf);
    oa_sealed::refuse_io(&root).map_err(|e| format!("private output root refused: {e}"))?;
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    oa_sealed::refuse_io(&root).map_err(|e| format!("private output root refused: {e}"))?;
    let resolved = fs::canonicalize(&root).map_err(|e| e.to_string())?;
    let private = fs::canonicalize("/home/connoravila/Documents/Avila-Labs/openatca-private")
        .map_err(|e| format!("private root must exist: {e}"))?;
    if !resolved.starts_with(private) {
        return Err("output path is outside openatca-private".into());
    }
    Ok(resolved)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    oa_sealed::refuse_io(path).map_err(|e| format!("private output refused: {e}"))?;
    // seal-allow: create-new write to a locally refused private output; it never opens existing contents
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file: File = options.open(path).map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), String> {
    // seal-allow: opens the checked private output directory only for directory-entry fsync, not data reads
    File::open(path)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| e.to_string())
}

fn missed_approach_summary(
    cifp: &oa_arinc424::Cifp,
    references: &[ProcedureRouteRef],
) -> Result<String, String> {
    let mut out = String::from(
        "## Approach missed-branch disposition\n\nNormal-final output ends at the unique explicit CIFP col-43 `M` published MAP marker. Source rows after that marker are retained here as a separate missed-approach branch and are not materialized as normal-landing fixes or aircraft motion. This is a route-slice decision, not support for missed-approach execution.\n\n",
    );
    for pair in references.windows(2) {
        let [reference, next] = pair else { continue };
        if reference.kind != ProcedureKind::Star
            || reference.route_type != Some('6')
            || next.kind != ProcedureKind::Direct
        {
            continue;
        }
        for route in cifp
            .procedures
            .iter()
            .filter(|p| {
                p.airport_ident == reference.airport
                    && p.ident == reference.procedure
                    && p.kind == CifpProcedureKind::Star
            })
            .flat_map(|p| &p.routes)
            .filter(|r| {
                Some(r.route_type) == reference.route_type && r.transition == reference.transition
            })
        {
            if let Some(leg) = route
                .legs
                .last()
                .filter(|l| l.path_terminator == oa_arinc424::PathTerminator::FM)
            {
                writeln!(out, "- Synthetic heading exit: {} type-6 {} sequence {} FM starts at {:?}; published heading leg is not flown. Explicit synthetic DCT {} replaces it; crossing constraints are retained.", reference.procedure, reference.transition, leg.sequence, leg.fix.as_ref().map(|f| f.ident.as_str()), next.transition)
                    .map_err(|_| "heading-exit provenance formatting failed")?;
            }
        }
    }
    for reference in references.iter().filter(|reference| {
        reference.kind == ProcedureKind::Approach && reference.transition.is_empty()
    }) {
        let matching = cifp
            .procedures
            .iter()
            .filter(|procedure| {
                procedure.airport_ident == reference.airport
                    && procedure.ident == reference.procedure
                    && procedure.kind == CifpProcedureKind::Approach
            })
            .flat_map(|procedure| procedure.routes.iter())
            .filter(|route| {
                Some(route.route_type) == reference.route_type && route.transition.is_empty()
            })
            .collect::<Vec<_>>();
        let [route] = matching.as_slice() else {
            return Err(format!(
                "cannot uniquely summarize missed branch for {} {}",
                reference.procedure,
                reference.route_type.unwrap_or('?')
            ));
        };
        let markers = route
            .legs
            .iter()
            .enumerate()
            .filter(|(_, leg)| leg.description.get(3).copied() == Some('M'))
            .collect::<Vec<_>>();
        let [(map_index, _)] = markers.as_slice() else {
            return Err(format!(
                "approach final {} {} lacks one unique explicit MAP marker",
                reference.procedure,
                reference.route_type.unwrap_or('?')
            ));
        };
        let missed_rows = route
            .legs
            .get(map_index.saturating_add(1)..)
            .ok_or_else(|| "approach MAP index is outside the source route".to_string())?;
        let first = missed_rows.first().map(|leg| leg.sequence);
        let last = missed_rows.last().map(|leg| leg.sequence);
        writeln!(
            out,
            "- CIFP missed branch after {} type-{} MAP: {} source rows; sequence range {:?}–{:?}; intentionally unmaterialized.",
            reference.procedure,
            reference.route_type.unwrap_or('?'),
            missed_rows.len(),
            first,
            last
        )
        .map_err(|_| "missed-approach provenance formatting failed")?;
    }
    if out.ends_with("\n\n") {
        out.push_str("- No selected final approach route.\n");
    }
    out.push('\n');
    Ok(out)
}

// This writer assembles a fixed provenance schema from separately validated source/result parts.
#[allow(clippy::too_many_arguments)]
fn oasd_manifest_text(
    scenario: &oa_demo_scenario::SyntheticScenario,
    references: &[ProcedureRouteRef],
    fixes: &[String],
    cifp_sha: &str,
    entry_sha: &str,
    performance_sha: &str,
    row_count: usize,
    oasd_sha: &str,
    oasd_bytes: usize,
    center_lat_e7: i32,
    center_lon_e7: i32,
    source_bundle_sha: &str,
    source_bundle_inventory: &str,
    scenario_lib_sha: &str,
    resolver_sha: &str,
    traffic_sha: &str,
    prep_tool_sha: &str,
    missed_approach_note: &str,
) -> String {
    let mut text = String::new();
    text.push_str("# Private generated OASD scenario provenance\n\n");
    text.push_str("Selected output format: OASD only. ADS3 and CAT048 products were not constructed. Traffic and positions are wholly synthetic; no recordings, SWIM content, real callsigns, real addresses, or facility adaptation were read.\n\n");
    text.push_str("Path model difference: route references retain the selected CIFP components and fix ordering; generated positions interpolate geodesics between fixes. The path is not an execution of course-to-fix, course-to-altitude, vector/intercept, manual-termination, or holding semantics, and is not operational procedure guidance. Any SYN/DCT segment is synthetic flight-plan geometry, not a published CIFP leg.\n\n");
    text.push_str(missed_approach_note);
    text.push_str(&format!("- Scenario ID: {}\n- Format version: {}\n- Seed: {}\n- Aircraft: {}\n- Scenario duration: {} ms\n- Full OASD record count: {row_count}\n- OASD v1 artifact bytes: {oasd_bytes}\n- OASD whole-file SHA-256: {oasd_sha}\n- Time origin: {SYNTHETIC_EPOCH_NS} ns; row times are signed virtual milliseconds from T0\n- Sample interval: 5000 ms\n- CIFP archive: {CIFP_NAME}; SHA-256: {cifp_sha}\n- CIFP entry: {CIFP_ENTRY}; SHA-256: {entry_sha}\n- Performance source: JCAB 737 TCDS No. 38 Rev 12; SHA-256: {performance_sha}\n- Source-bound scene center: KORD airport reference point from pinned CIFP, rounded to E7 (`round(degrees × 10^7)`); lat_e7={center_lat_e7}, lon_e7={center_lon_e7}\n- Kinematics: 220 kt independent synthetic groundspeed; 220 kt IAS proxy only for procedure/VMO checking; 18,000 ft initial altitude; 1,800 fpm rate cap; no wind, atmosphere conversion, turn, bank, acceleration, or full aircraft-performance model\n- Selected route fixes: {}\n", scenario.scenario_id, scenario.format_version, scenario.seed, scenario.flights.len(), scenario.duration_ms, fixes.join(" → ")));
    for reference in references {
        if reference.kind == ProcedureKind::Direct {
            text.push_str(&format!(
                "- SYNTHETIC DCT connector: {} (straight geodesic between these two published CIFP fixes; not a published CIFP leg or operational route)\n",
                reference.transition
            ));
        } else {
            text.push_str(&format!(
                "- CIFP route: {} {:?} {} / {}\n",
                reference.airport, reference.kind, reference.procedure, reference.transition
            ));
        }
    }
    text.push_str(&format!("\nHeader source SHA-256 values: lib.rs={scenario_lib_sha}; shell/resolve.rs={resolver_sha}; core/traffic.rs={traffic_sha}; prep main.rs={prep_tool_sha}. `scenario_lib_sha256` is exactly the `src/lib.rs` facade file hash. Core scenario intent has its own entry in the source bundle.\n\nCanonical source bundle SHA-256: {source_bundle_sha}\n\n## Canonical source bundle inventory\n\n{source_bundle_inventory}"));
    text
}

// This writer assembles a fixed provenance schema from separately validated source/result parts.
#[allow(clippy::too_many_arguments)]
fn manifest_text(
    scenario: &oa_demo_scenario::SyntheticScenario,
    references: &[ProcedureRouteRef],
    fixes: &[String],
    cifp_sha: &str,
    entry_sha: &str,
    performance_sha: &str,
    rows: usize,
    frame_manifest_sha: &str,
    frame_inventory: &str,
    ads3_frames: &[EncodedAds3Frame],
    truth_rows: u64,
    detected: u64,
    false_plots: u64,
    split_plots: u64,
    cat048_sha: &str,
    oasd_sha: &str,
    oasd_bytes: usize,
    center_lat_e7: i32,
    center_lon_e7: i32,
    source_bundle_sha: &str,
    source_bundle_inventory: &str,
    scenario_lib_sha: &str,
    resolver_sha: &str,
    traffic_sha: &str,
    sensor_profile_sha: &str,
    sensor_radar_sha: &str,
    cifp_parser_lib_sha: &str,
    cifp_parser_file_sha: &str,
    truth_builder_sha: &str,
    prep_tool_sha: &str,
    missed_approach_note: &str,
) -> String {
    let mut text = String::new();
    text.push_str("# Private generated scenario provenance\n\n");
    text.push_str(&format!("Selected output formats: OASD v1, ADS3, and CAT048; full OASD/ADS3 source row count: {rows}.\nSynthetic-only traffic. No recordings, SWIM content, real callsigns, real addresses, or facility adaptation were read.\n\n"));
    text.push_str("Path model difference: route references retain the selected CIFP components and fix ordering; generated positions interpolate geodesics between fixes. The path is not an execution of course-to-fix, course-to-altitude, vector/intercept, manual-termination, or holding semantics, and is not operational procedure guidance. Any SYN/DCT segment is synthetic flight-plan geometry, not a published CIFP leg.\n\n");
    text.push_str(missed_approach_note);
    text.push_str(&format!("- Scenario ID: {}\n- Format version: {}\n- Seed: {}\n- Aircraft: {}\n- Arrival window: {} ms\n- Report time origin: {SYNTHETIC_EPOCH_NS} ns\n- CIFP archive: {CIFP_NAME}\n- CIFP archive SHA-256: {cifp_sha}\n- CIFP entry: {CIFP_ENTRY}\n- CIFP entry SHA-256: {entry_sha}\n", scenario.scenario_id, scenario.format_version, scenario.seed, scenario.flights.len(), scenario.duration_ms));
    for reference in references {
        if reference.kind == ProcedureKind::Direct {
            text.push_str(&format!(
                "- SYNTHETIC DCT connector: {} (straight geodesic between these two published CIFP fixes; not a published CIFP leg or operational route)\n",
                reference.transition
            ));
        } else {
            text.push_str(&format!(
                "- CIFP route: {} {:?} {} / {}\n",
                reference.airport, reference.kind, reference.procedure, reference.transition
            ));
        }
    }
    text.push_str(&format!("- Ordered fix sequence: {}\n- OASD v1 artifact: `ord-normal-push-v1.oasd`; {oasd_bytes} bytes; SHA-256 {oasd_sha}; 352-byte little-endian header; 32-byte little-endian records; row digest covers exact record bytes.\n- OASD source-bound scene center: KORD airport reference point from the pinned CIFP entry, rounded to E7 using `round(degrees × 10^7)`; center_lat_e7={center_lat_e7}, center_lon_e7={center_lon_e7}.\n- Canonical source bundle SHA-256: {source_bundle_sha} (ordered UTF-8 TSV inventory follows; includes code, build configuration, input data, and role labels). The `scenario_lib_sha256` OASD field is only the exact `src/lib.rs` facade hash. `shell/artifact.rs` is validation-only. `core/turn.rs` is not a generator input because the current materializer does not call it.\n- Performance source: JCAB 737 Type Certificate Data Sheet No. 38 Rev 12\n- Performance source SHA-256: {performance_sha}\n- Airframe limits referenced: VMO 340 KCAS / MMO Mach 0.82; synthetic IAS proxy 220 kt and independent groundspeed assumption 220 kt. CIFP speed constraints and VMO are checked against the IAS proxy only; there is no atmosphere or wind conversion to groundspeed or Mach, and MMO is not evaluated.\n- Synthetic kinematics: 18,000 ft initial altitude; max 1,800 fpm; 5,000 ms cadence; no wind model; constant groundspeed; instantaneous turns at fixes; no acceleration, bank, or aircraft performance model\n- Named radar profile: MSSR_EUROCONTROL_1997\n- Sensor site assumptions: KORD public airport reference point; 0 m height; 80 NM range; zero range/azimuth bias and split offsets; 4/3 earth model; SAC 250/SIC 1\n- Truth rows kept: {truth_rows}\n- Sensor detections: {detected}; false plots: {false_plots}; split plots: {split_plots}\n- Synthetic ADS3 report count: {rows}\n- Ordered ADS3 frame manifest SHA-256: {frame_manifest_sha}\n- ADS3 frame count: {}\n- CAT048 payload SHA-256: {cat048_sha}\n- Source SHA-256 `crates/oa-demo-scenario/src/lib.rs`: {scenario_lib_sha}\n- Source SHA-256 `crates/oa-demo-scenario/src/shell/resolve.rs`: {resolver_sha}\n- Source SHA-256 `crates/oa-demo-scenario/src/core/traffic.rs`: {traffic_sha}\n- Source SHA-256 `crates/oa-sensorsim/src/profile.rs`: {sensor_profile_sha}\n- Source SHA-256 `crates/oa-sensorsim/src/radar.rs`: {sensor_radar_sha}\n- Source SHA-256 `crates/oa-arinc424/src/lib.rs`: {cifp_parser_lib_sha}\n- Source SHA-256 `crates/oa-arinc424/src/file.rs`: {cifp_parser_file_sha}\n- Source SHA-256 `crates/oa-truth/src/shell/build.rs`: {truth_builder_sha}\n- Source SHA-256 prep tool: {prep_tool_sha}\n\n## ADS3 frame inventory\n\n{frame_inventory}\n## Canonical source bundle inventory\n\n{source_bundle_inventory}", fixes.join(" → "), ads3_frames.len()));
    text
}

#[cfg(test)]
mod route_listing_diagnostic_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn synthetic_final_resolution_refusal_keeps_exact_class_and_join_count() {
        let mut diagnostics = RouteListingDiagnostics::default();
        diagnostics
            .count("approach.typeA_transition_groups")
            .expect("bounded synthetic group count");
        diagnostics
            .count("approach.empty_transition_final_groups")
            .expect("bounded synthetic final count");
        diagnostics
            .resolver_refusal(
                "approach.final.resolve",
                &RouteResolveError::MultiplePublishedMaps,
            )
            .expect("bounded synthetic refusal count");
        diagnostics
            .resolver_refusal(
                "approach.final.candidate.resolve",
                &RouteResolveError::UnsupportedAltitudeDescription(4, 'H'),
            )
            .expect("bounded synthetic descriptor refusal count");
        diagnostics
            .resolver_refusal(
                "approach.final.candidate.resolve",
                &RouteResolveError::InvalidAltitudeValue(
                    5,
                    1,
                    oa_demo_scenario::AltitudeValueKind::Ground,
                ),
            )
            .expect("bounded synthetic value refusal count");
        let rendered = diagnostics.render().expect("bounded summary renders");
        assert!(rendered.contains("approach.typeA_transition_groups=1"));
        assert!(rendered.contains("approach.empty_transition_final_groups=1"));
        assert!(rendered.contains("approach.final.resolve.MultiplePublishedMaps=1"));
        assert!(rendered
            .contains("approach.final.candidate.resolve.UnsupportedAltitudeDescription_U+0048=1"));
        assert!(rendered
            .contains("approach.final.candidate.resolve.InvalidAltitudeValue_field1_Ground=1"));
    }

    #[test]
    fn diagnostic_bucket_cap_refuses_new_categories() {
        let mut diagnostics = RouteListingDiagnostics::default();
        for index in 0..MAX_ROUTE_LIST_DIAGNOSTIC_BUCKETS {
            diagnostics
                .count(&format!("synthetic.category.{index}"))
                .expect("bucket count remains within fixed cap");
        }
        assert!(diagnostics.count("overflow.category").is_err());
    }

    #[test]
    fn first_verified_candidate_is_deterministic_and_stops_after_full_validation() {
        let candidates = [
            ("route-refusal", false, false),
            ("motion-refusal", true, false),
            ("first-valid", true, true),
            ("later-valid", true, true),
        ];
        let mut visited = Vec::new();
        let selected =
            first_verified_candidate(candidates, |(name, route_resolves, motion_valid)| {
                visited.push(name);
                Ok((route_resolves && motion_valid).then_some(name))
            })
            .expect("synthetic source-order validation succeeds");

        assert_eq!(selected, Some("first-valid"));
        assert_eq!(visited, ["route-refusal", "motion-refusal", "first-valid"]);
    }

    #[test]
    fn complete_route_list_still_refuses_after_its_fixed_candidate_cap() {
        let mut listed = 0usize;
        for _ in 0..MAX_LISTED_CANDIDATES {
            increment_listed_route_count(&mut listed).expect("within complete-list cap");
        }
        assert_eq!(listed, MAX_LISTED_CANDIDATES);
        assert!(increment_listed_route_count(&mut listed)
            .expect_err("full-list cap remains fail-closed")
            .contains("no partial list emitted"));
    }

    #[test]
    fn first_route_command_marks_the_synthetic_connector_explicitly() {
        let key = (
            "J12".to_owned(),
            "FIXA".to_owned(),
            "STAR1".to_owned(),
            "TRANS1".to_owned(),
            '6',
            "RWY".to_owned(),
            "APP1".to_owned(),
            "TRANS2".to_owned(),
            'F',
            true,
        );
        let mut line = String::new();
        write_route_chain_command(&mut line, &key, "STARFIX")
            .expect("synthetic selected route formats");
        assert_eq!(
            line,
            "--airway J12 FIXA STARFIX --star STAR1 4 TRANS1 --approach APP1 A TRANS2 --star-runway 6 RWY --approach-final F --synthetic-direct\n"
        );

        let mut raw = vec![
            "--cifp".to_owned(),
            "CIFP_261029.zip".to_owned(),
            "--performance-spec".to_owned(),
            "tcds.pdf".to_owned(),
            "--performance-sha256".to_owned(),
            "a".repeat(64),
        ];
        raw.extend(line.split_whitespace().map(str::to_owned));
        let parsed = Args::parse(raw).expect("emitted selector round-trips through CLI parser");
        assert_eq!(parsed.airway, "J12");
        assert_eq!(parsed.airway_from, "FIXA");
        assert_eq!(parsed.airway_to, "STARFIX");
        assert_eq!(parsed.star, "STAR1");
        assert_eq!(parsed.star_transition, "TRANS1");
        assert_eq!(parsed.star_runway, Some(('6', "RWY".to_owned())));
        assert_eq!(parsed.approach, "APP1");
        assert_eq!(parsed.approach_transition, "TRANS2");
        assert_eq!(parsed.approach_final_type, Some('F'));
        assert!(parsed.synthetic_direct);
    }

    #[test]
    fn runway_transition_matching_observes_parallel_runway_b_convention() {
        assert!(runway_transition_matches_approach("RW04B", "RW04L"));
        assert!(runway_transition_matches_approach("RW04B", "RW04R"));
        assert!(!runway_transition_matches_approach("RW04B", "RW27L"));
        assert!(!runway_transition_matches_approach("RW04B", "RW04"));
        assert!(runway_transition_matches_approach("RW27L", "RW27L"));
        assert!(!runway_transition_matches_approach("RW27L", "RW27R"));
        assert!(!runway_transition_matches_approach("RW00B", "RW00L"));
        assert!(!runway_transition_matches_approach("RW04B", "FIXA"));
    }
    #[test]
    fn four_streams_have_fifteen_aircraft_and_more_than_six_nm_in_trail() {
        let mut counts = [0_u32; 4];
        let mut previous = [None; 4];
        for n in 0..60 {
            let (stream, arrival) = four_stream_slot(n).unwrap();
            assert!(arrival < 3_600_000);
            let i = usize::from(stream);
            *counts.get_mut(i).expect("stream index") += 1;
            if let Some(last) = *previous.get(i).expect("stream index") {
                let gap_ms = arrival - last;
                assert!(gap_ms * 220 >= 6 * 3_600_000);
            }
            *previous.get_mut(i).expect("stream index") = Some(arrival);
        }
        assert_eq!(counts, [15, 15, 15, 15]);
    }
}

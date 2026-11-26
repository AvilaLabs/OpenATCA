#![forbid(unsafe_code)]
//! Native shell latency harness for the public oa-demo-engine API. Clock and asset I/O stay here.

use std::{env, io::Read, path::Path, process, time::Instant};

use oa_demo_engine::{
    encode_frame_packet, from_oasd, DemoEngine, DemoEngineConfig, EngineCounters,
    DEMO_FRAME_HEIGHT, DEMO_FRAME_STRIDE, FRAME_PACKET_HEADER_LEN, SYNTHETIC_ADDRESS_BASE,
};
use oa_demo_scenario::core::SyntheticReport;

const DURATION_MS: u64 = 3_600_000;
const STEP_MS: u64 = 5_000;
const ALERT_SCAN_MS: u64 = 15_000;
const ALERT_REPETITIONS: usize = 100;
const PACKET_BYTES: usize =
    FRAME_PACKET_HEADER_LEN + DEMO_FRAME_STRIDE as usize * DEMO_FRAME_HEIGHT as usize;

fn main() {
    if let Err(error) = run() {
        eprintln!("oa-demo-latency: {error}");
        process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: oa-demo-latency <scenario.oasd> <asset-sha256> <source-commit>")?;
    let expected_hash = parse_hash(&args.next().ok_or("asset SHA-256 required")?)?;
    let source_commit = args.next().ok_or("source commit required")?;
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let bytes = read_bounded(Path::new(&path), 16 * 1024 * 1024)?;
    let actual_hash = oa_sha256::core::sha256::sha256(&bytes);
    if actual_hash != expected_hash {
        return Err("scenario asset SHA-256 does not match frozen hash".into());
    }
    let engine = from_oasd(&bytes, expected_hash).map_err(|e| format!("scenario refused: {e}"))?;
    let normal = measure_normal(engine)?;
    let alert = measure_alert_pair()?;
    let display_target_passed = normal.p99_ns < 100_000_000;
    println!(
        "{{\"schema\":\"openatca-latency-v1\",\"source_commit\":\"{}\",\"scenario_asset_sha256\":\"{}\",\"targets\":{{\"display_p99_limit_ms\":100,\"display_p99_pass\":{},\"alert_same_scan\":true,\"handoff_p99_limit_ms\":10,\"amendment_p99_limit_ms\":10}},\"scan_input_to_frame_packet\":{},\"converging_pair_alert_scan_to_frame_packet\":{},\"counts\":{{\"scenario1\":{},\"scenario2\":{{\"samples\":{},\"alerts_rendered\":{},\"alerts_missing\":{},\"refused\":0,\"lost\":0,\"engine\":{}}}}},\"warmup\":{{\"scenario1_virtual_ms\":0,\"scenario1_frame_count\":1,\"scenario2_virtual_ms\":10000,\"scenario2_frames_per_repetition\":1,\"timed\":false}},\"measurement_scope\":\"720 native scan intervals for scenario1 from 5000 to 3600000ms inclusive; each timer includes due report ingestion from the prepared, hashed OASD rows, advance_to, frame render and canonical packet encoding. Asset read/validation and engine preparation are outside timing. Scenario2 uses 100 identical independent engines; setup and one no-alert warmup frame are outside each timer.\"}}",
        json_escape(&source_commit),
        hex(&actual_hash),
        display_target_passed,
        normal.distribution,
        alert.distribution,
        counters_json(normal.counters),
        alert.samples,
        alert.alerts_rendered,
        ALERT_REPETITIONS - alert.alerts_rendered,
        counters_json(alert.counters),
    );
    Ok(())
}

struct ResultSet {
    counters: EngineCounters,
    distribution: String,
    p99_ns: u64,
}

fn measure_normal(mut engine: DemoEngine) -> Result<ResultSet, String> {
    let mut packet = vec![0u8; PACKET_BYTES];
    let mut samples = Vec::with_capacity((DURATION_MS / STEP_MS) as usize);
    engine
        .advance_to(0)
        .map_err(|e| format!("scenario1 t=0 warmup refused: {e}"))?;
    let warmup = engine
        .render_frame()
        .map_err(|e| format!("scenario1 warmup render refused: {e}"))?;
    encode_frame_packet(&warmup, &mut packet)
        .map_err(|e| format!("scenario1 warmup packet refused: {e}"))?;
    for virtual_ms in (STEP_MS..=DURATION_MS).step_by(STEP_MS as usize) {
        let start = Instant::now();
        engine
            .advance_to(virtual_ms)
            .map_err(|e| format!("scenario1 advance refused at {virtual_ms}ms: {e}"))?;
        let frame = engine
            .render_frame()
            .map_err(|e| format!("scenario1 render refused at {virtual_ms}ms: {e}"))?;
        encode_frame_packet(&frame, &mut packet)
            .map_err(|e| format!("scenario1 packet refused at {virtual_ms}ms: {e}"))?;
        samples.push(start.elapsed().as_nanos().min(u64::MAX as u128) as u64);
    }
    let counters = engine.counters();
    check_counters(counters, "scenario1")?;
    if counters.scenario_duration_ms != DURATION_MS
        || counters.reports_processed != counters.reports_supplied
        || counters.frames_rendered != samples.len() as u64 + 1
    {
        return Err(format!("scenario1 incomplete: {}", counters_json(counters)));
    }
    let distribution = distribution(&mut samples);
    let p99_ns = percentile(&samples, 99);
    Ok(ResultSet {
        counters,
        distribution,
        p99_ns,
    })
}

struct AlertResult {
    samples: usize,
    alerts_rendered: usize,
    distribution: String,
    counters: EngineCounters,
}

fn measure_alert_pair() -> Result<AlertResult, String> {
    let mut samples = Vec::with_capacity(ALERT_REPETITIONS);
    let mut final_counters = None;
    for _ in 0..ALERT_REPETITIONS {
        let mut engine = converging_pair()?;
        engine
            .advance_to(10_000)
            .map_err(|e| format!("scenario2 warmup refused: {e}"))?;
        if !engine.active_ca_alerts().is_empty() {
            return Err("converging-pair alert occurred before the frozen 15000ms scan".into());
        }
        let mut packet = vec![0u8; PACKET_BYTES];
        let warmup = engine
            .render_frame()
            .map_err(|e| format!("scenario2 warmup render refused: {e}"))?;
        if warmup.counters.ca_open_alerts != 0 {
            return Err("scenario2 warmup frame already contains an alert".into());
        }
        let warmup_digest = warmup.pixel_digest;
        encode_frame_packet(&warmup, &mut packet)
            .map_err(|e| format!("scenario2 warmup packet refused: {e}"))?;
        let start = Instant::now();
        engine
            .advance_to(ALERT_SCAN_MS)
            .map_err(|e| format!("scenario2 alert scan refused: {e}"))?;
        let (frame_alert_count, frame_digest) = {
            let frame = engine
                .render_frame()
                .map_err(|e| format!("scenario2 render refused: {e}"))?;
            encode_frame_packet(&frame, &mut packet)
                .map_err(|e| format!("scenario2 packet refused: {e}"))?;
            (frame.counters.ca_open_alerts, frame.pixel_digest)
        };
        let elapsed = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let alerts = engine.active_ca_alerts();
        let expected_a = SYNTHETIC_ADDRESS_BASE;
        let expected_b = SYNTHETIC_ADDRESS_BASE.saturating_add(1);
        let exact_pair_created = alerts.len() == 1
            && alerts.first().is_some_and(|alert| {
                (alert.addr_a == expected_a && alert.addr_b == expected_b)
                    || (alert.addr_a == expected_b && alert.addr_b == expected_a)
            })
            && alerts.first().is_some_and(|alert| {
                alert.start_ns == (ALERT_SCAN_MS as i64).saturating_mul(1_000_000)
            });
        if !exact_pair_created
            || frame_alert_count != alerts.len() as u64
            || frame_digest == warmup_digest
        {
            return Err("scan did not create the expected pair alert in the rendered frame".into());
        }
        samples.push(elapsed);
        let counters = engine.counters();
        check_counters(counters, "scenario2")?;
        if counters.reports_processed != counters.reports_supplied {
            return Err(format!(
                "scenario2 did not consume all reports: {}",
                counters_json(counters)
            ));
        }
        if final_counters.is_some_and(|previous| previous != counters) {
            return Err("scenario2 repetitions produced different engine counts".into());
        }
        final_counters = Some(counters);
        if counters.ca_open_alerts == 0 || frame_alert_count == 0 {
            return Err("rendered scenario2 frame does not carry the current alert count".into());
        }
    }
    let distribution = distribution(&mut samples);
    Ok(AlertResult {
        samples: samples.len(),
        alerts_rendered: samples.len(),
        distribution,
        counters: final_counters.ok_or("scenario2 produced no samples")?,
    })
}

fn converging_pair() -> Result<DemoEngine, String> {
    let config = DemoEngineConfig::normal_ord(11, 15_000, 0, 5_000, 419_800_000, -878_000_000);
    let offsets = [(10_000, 1_440_000), (15_000, 1_345_000)];
    let mut rows = Vec::with_capacity(offsets.len() * 2);
    for (time_ms, offset) in offsets {
        for (aircraft_index, sign, heading) in [(0, 1, 1_800), (1, -1, 0)] {
            rows.push(SyntheticReport {
                virtual_time_ms: time_ms,
                aircraft_index,
                flags: 0,
                lat_e7: 419_800_000 + sign * offset,
                lon_e7: -878_000_000,
                altitude_ft: 10_000,
                ground_speed_dkt: 4_000,
                track_ddeg: heading,
                vertical_rate_fpm: 0,
            });
        }
    }
    DemoEngine::from_reports(config, rows)
        .map_err(|e| format!("scenario2 construction refused: {e}"))
}

fn check_counters(c: EngineCounters, where_: &str) -> Result<(), String> {
    if c.targets_refused != 0
        || c.tracker_output_overflow != 0
        || c.prediction_unavailable != 0
        || c.ca_work_short != 0
        || c.ca_input_short != 0
        || c.input_keys_unmapped != 0
        || c.input_buffer_full != 0
        || c.input_command_refused != 0
        || c.pointer_misses != 0
    {
        return Err(format!("{where_} lost/refused work: {}", counters_json(c)));
    }
    Ok(())
}

fn counters_json(c: EngineCounters) -> String {
    format!(
        "{{\"labels_placed\":{},\"label_overlap_pairs\":{},\"label_overlap_pairs_outside_terminal\":{},\"label_overlap_pairs_inside_terminal\":{},\"arrivals_completed\":{},\"ca_cycles\":{},\"ca_open_alerts\":{},\"ca_work_short\":{},\"ca_input_short\":{},\"reports_supplied\":{},\"reports_processed\":{},\"prediction_unavailable\":{},\"targets_refused\":{},\"report_order_ties\":{},\"scenario_start_virtual_ms\":{},\"earliest_report_virtual_ms\":{},\"preroll_ms\":{},\"scenario_duration_ms\":{},\"sample_interval_ms\":{},\"max_catchup_ticks\":{},\"tracker_output_overflow\":{},\"input_keys_unmapped\":{},\"input_buffer_full\":{},\"input_commands\":{},\"input_command_refused\":{},\"pointer_misses\":{},\"frames_rendered\":{}}}",
        c.labels_placed,
        c.label_overlap_pairs,
        c.label_overlap_pairs_outside_terminal,
        c.label_overlap_pairs_inside_terminal,
        c.arrivals_completed,
        c.ca_cycles,
        c.ca_open_alerts,
        c.ca_work_short,
        c.ca_input_short,
        c.reports_supplied,
        c.reports_processed,
        c.prediction_unavailable,
        c.targets_refused,
        c.report_order_ties,
        c.scenario_start_virtual_ms,
        c.earliest_report_virtual_ms,
        c.preroll_ms,
        c.scenario_duration_ms,
        c.sample_interval_ms,
        c.max_catchup_ticks,
        c.tracker_output_overflow,
        c.input_keys_unmapped,
        c.input_buffer_full,
        c.input_commands,
        c.input_command_refused,
        c.pointer_misses,
        c.frames_rendered
    )
}

fn distribution(samples: &mut [u64]) -> String {
    samples.sort_unstable();
    let at = |percent: usize| -> u64 {
        let rank = (samples.len() * percent).div_ceil(100).max(1);
        samples.get(rank - 1).copied().unwrap_or(0)
    };
    format!(
        "{{\"unit\":\"ns\",\"n\":{},\"p50\":{},\"p95\":{},\"p99\":{},\"max\":{}}}",
        samples.len(),
        at(50),
        at(95),
        at(99),
        samples.last().copied().unwrap_or(0)
    )
}

fn percentile(samples: &[u64], percent: usize) -> u64 {
    let rank = (samples.len() * percent).div_ceil(100).max(1);
    samples.get(rank - 1).copied().unwrap_or(0)
}

fn parse_hash(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 {
        return Err("asset SHA-256 must have 64 hex digits".into());
    }
    let mut hash = [0; 32];
    for (i, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(value.get(i * 2..i * 2 + 2).ok_or("bad hash")?, 16)
            .map_err(|_| "asset SHA-256 is not hexadecimal")?;
    }
    Ok(hash)
}

fn read_bounded(path: &Path, cap: usize) -> Result<Vec<u8>, String> {
    let (file, size) =
        oa_sealed::open_regular(path).map_err(|_| "scenario asset refused by sealed-path guard")?;
    let cap = u64::try_from(cap).map_err(|_| "input cap is not representable")?;
    if size == 0 || size > cap {
        return Err("scenario asset is not a nonempty regular file within 16 MiB".into());
    }
    let capacity = usize::try_from(size).map_err(|_| "scenario size is not representable")?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(cap.checked_add(1).ok_or("input cap overflow")?)
        .read_to_end(&mut bytes)
        .map_err(|_| "scenario asset read failed")?;
    if bytes.len() != capacity {
        return Err("scenario asset length changed during read".into());
    }
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn json_escape(value: &str) -> String {
    value.chars().flat_map(char::escape_default).collect()
}

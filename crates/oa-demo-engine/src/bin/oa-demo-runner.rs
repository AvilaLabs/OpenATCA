#![forbid(unsafe_code)]

use oa_demo_engine::{
    encode_frame_packet, from_oasd, normalize_host_key_modifiers, EngineError, DEMO_FRAME_HEIGHT,
    DEMO_FRAME_STRIDE, FRAME_PACKET_HEADER_LEN,
};
use std::env;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_TRACE_BYTES: usize = 4 * 1024 * 1024;
const MAX_EVENTS: usize = 16_384;
const PACKET_BYTES: usize =
    FRAME_PACKET_HEADER_LEN + DEMO_FRAME_STRIDE as usize * DEMO_FRAME_HEIGHT as usize;

#[derive(Clone)]
enum Event {
    Advance(u64),
    Key {
        usage: u32,
        modifiers: u32,
        down: bool,
        repeat: bool,
    },
    Pointer {
        x: i32,
        y: i32,
        buttons: u32,
        kind: u32,
    },
}

struct Trace {
    source_commit: String,
    asset_sha: String,
    events: Vec<(String, Event, i32, String)>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("oa-demo-runner: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let scenario = read_bounded(&args.scenario, 16 * 1024 * 1024)?;
    let trace_bytes = read_bounded(&args.trace, MAX_TRACE_BYTES)?;
    let trace_text = std::str::from_utf8(&trace_bytes).map_err(|_| "trace is not UTF-8")?;
    let trace = parse_trace(trace_text)?;
    if trace.source_commit != args.source_commit {
        return Err("trace SOURCE does not match --source-commit".into());
    }
    let asset_hash = oa_sha256::core::sha256::sha256(&scenario);
    if hex(&asset_hash) != trace.asset_sha {
        return Err("trace ASSET does not match the complete scenario artifact".into());
    }
    let mut engine = from_oasd(&scenario, asset_hash).map_err(display_error)?;
    let mut result = String::with_capacity(512 + trace.events.len() * 128);
    result.push_str("F2RESULT1\n");
    push_row(&mut result, &["SOURCE", &trace.source_commit]);
    push_row(&mut result, &["ASSET", &trace.asset_sha]);
    push_row(&mut result, &["SCENARIO", "ord-normal-push-v1"]);
    push_row(&mut result, &["SEED", "0x4F52442D4E505631"]);
    let reset_status = engine
        .advance_to(0)
        .map_or_else(|error| error.status_code(), |_| 0);
    if reset_status != 0 {
        return Err(format!("reset advance refused with status {reset_status}"));
    }
    push_row(&mut result, &["RESET", "0"]);
    let mut packet = vec![0u8; PACKET_BYTES];
    let frame0 = engine.render_frame().map_err(display_error)?;
    let frame0_sha = encode_frame_packet(&frame0, &mut packet).map_err(display_error)?;
    push_row(&mut result, &["FRAME", "0", &hex(&frame0_sha)]);
    note_frame(&engine, 0);
    push_row(&mut result, &["EVENTS", &trace.events.len().to_string()]);

    for (offset, (input_line, event, _expected_status, _expected_frame)) in
        trace.events.iter().enumerate()
    {
        let index = offset + 1;
        result.push_str(input_line);
        result.push('\n');
        let applied = match event {
            Event::Advance(ms) => engine.advance_to(*ms),
            Event::Key {
                usage,
                modifiers,
                down,
                repeat,
            } => engine.handle_key(*usage, *modifiers, *down, *repeat),
            Event::Pointer {
                x,
                y,
                buttons,
                kind,
            } => engine.handle_pointer(*x, *y, *buttons, *kind),
        };
        let (status, frame_sha) = match applied {
            Err(error) => (
                error.status_code(),
                oa_sha256::core::sha256::sha256(&packet),
            ),
            Ok(()) => match engine.render_frame() {
                Err(error) => (
                    error.status_code(),
                    oa_sha256::core::sha256::sha256(&packet),
                ),
                Ok(frame) => match encode_frame_packet(&frame, &mut packet) {
                    Ok(digest) => (0, digest),
                    Err(error) => (
                        error.status_code(),
                        oa_sha256::core::sha256::sha256(&packet),
                    ),
                },
            },
        };
        push_row(
            &mut result,
            &["RESULT", &index.to_string(), &status.to_string()],
        );
        push_row(
            &mut result,
            &["FRAME", &index.to_string(), &hex(&frame_sha)],
        );
        note_frame(&engine, index);
    }
    if result.len() > MAX_TRACE_BYTES {
        return Err("F2RESULT1 exceeds the 4 MiB output bound".into());
    }
    write_new_private(&args.out, result.as_bytes())?;
    eprintln!(
        "virtual_ms={} on_screen_aircraft={} reports_processed={} targets_refused={} ca_cycles={} ca_open_alerts={} ca_work_short={} ca_input_short={}",
        engine.current_virtual_ms().unwrap_or(0),
        engine.frame_target_count(),
        engine.counters().reports_processed,
        engine.counters().targets_refused,
        engine.counters().ca_cycles,
        engine.counters().ca_open_alerts,
        engine.counters().ca_work_short,
        engine.counters().ca_input_short
    );
    Ok(())
}

fn note_frame(engine: &oa_demo_engine::DemoEngine, index: usize) {
    eprintln!("frame={} virtual_ms={} on_screen_aircraft={} ca_cycles={} ca_open_alerts={} ca_work_short={} ca_input_short={}",
        index, engine.current_virtual_ms().unwrap_or(0), engine.frame_target_count(),
        engine.counters().ca_cycles, engine.counters().ca_open_alerts,
        engine.counters().ca_work_short, engine.counters().ca_input_short);
    if matches!(engine.current_virtual_ms(), Some(0 | 300_000 | 900_000)) {
        for alert in engine.active_ca_alerts() {
            eprintln!(
                "synthetic_ca_pair a={} b={} first_ns={} last_ns={} min_h_m={} min_v_ft={}",
                alert.addr_a,
                alert.addr_b,
                alert.start_ns,
                alert.end_ns,
                alert.min_h_miss_m,
                alert.min_v_miss_ft
            );
        }
    }
}

struct Args {
    scenario: PathBuf,
    trace: PathBuf,
    out: PathBuf,
    source_commit: String,
}

fn parse_args() -> Result<Args, String> {
    let mut scenario = None;
    let mut trace = None;
    let mut out = None;
    let mut source_commit = None;
    let mut it = env::args_os().skip(1);
    while let Some(key) = it.next() {
        let value = it.next().ok_or("missing value for command-line option")?;
        match key.to_str() {
            Some("--scenario") if scenario.is_none() => scenario = Some(PathBuf::from(value)),
            Some("--trace") if trace.is_none() => trace = Some(PathBuf::from(value)),
            Some("--out") if out.is_none() => out = Some(PathBuf::from(value)),
            Some("--source-commit") if source_commit.is_none() => {
                source_commit = Some(value.into_string().map_err(|_| "source commit is not UTF-8")?);
            }
            _ => return Err("usage: oa-demo-runner --scenario OASD --trace F2TRACE1 --source-commit HEX40 --out NEW_RESULT".into()),
        }
    }
    let source_commit = source_commit.ok_or("--source-commit is required")?;
    if !is_lower_hex(&source_commit, 40) {
        return Err("--source-commit must be 40 lowercase hexadecimal characters".into());
    }
    Ok(Args {
        scenario: scenario.ok_or("--scenario is required")?,
        trace: trace.ok_or("--trace is required")?,
        out: out.ok_or("--out is required")?,
        source_commit,
    })
}

fn read_bounded(path: &Path, cap: usize) -> Result<Vec<u8>, String> {
    let (file, size) =
        oa_sealed::open_regular(path).map_err(|_| "input file refused by sealed-path guard")?;
    let cap_u64 = u64::try_from(cap).map_err(|_| "input cap is not representable")?;
    if size == 0 || size > cap_u64 {
        return Err("input is not a nonempty regular file within its byte cap".into());
    }
    let capacity = usize::try_from(size).map_err(|_| "input size is not representable")?;
    let mut bytes = Vec::with_capacity(capacity);
    let limit = cap_u64.checked_add(1).ok_or("input cap overflow")?;
    file.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|_| "input read failed")?;
    if bytes.len() != capacity {
        return Err("input length changed while reading".into());
    }
    Ok(bytes)
}

fn parse_trace(text: &str) -> Result<Trace, String> {
    if !text.is_ascii() || !text.ends_with('\n') || text.contains('\r') {
        return Err("trace must use LF lines and end with LF".into());
    }
    let lines: Vec<&str> = text
        .strip_suffix('\n')
        .ok_or("trace missing final LF")?
        .split('\n')
        .collect();
    if lines.len() < 7 || lines.first() != Some(&"F2TRACE1") {
        return Err("bad F2TRACE1 header".into());
    }
    let _build_sha = header_value(lines.get(1).copied(), "BUILD", 64)?;
    let source_commit = header_value(lines.get(2).copied(), "SOURCE", 40)?;
    let asset_sha = header_value(lines.get(3).copied(), "ASSET", 64)?;
    if lines.get(4) != Some(&"SCENARIO\tord-normal-push-v1")
        || lines.get(5) != Some(&"SEED\t0x4F52442D4E505631")
    {
        return Err("unsupported F2TRACE1 scenario or seed".into());
    }
    parse_frame(lines.get(6).copied(), 0)?;
    let mut events = Vec::new();
    let mut current_ms = 0u64;
    let mut i = 7usize;
    while i < lines.len() {
        if events.len() >= MAX_EVENTS {
            return Err("trace exceeds 16384 events".into());
        }
        let input = *lines.get(i).ok_or("missing event input")?;
        let event = parse_event(input)?;
        let result_line = *lines.get(i + 1).ok_or("missing event result")?;
        let status = parse_result(result_line)?;
        let frame_line = *lines.get(i + 2).ok_or("missing event frame")?;
        let frame_sha = parse_frame(Some(frame_line), events.len() + 1)?;
        if let Event::Advance(target_ms) = &event {
            if *target_ms <= current_ms {
                return Err(
                    "advance timestamps must increase beyond the last accepted time".into(),
                );
            }
            if status == 0 {
                current_ms = *target_ms;
            }
        }
        events.push((input.to_owned(), event, status, frame_sha));
        i = i.checked_add(3).ok_or("trace line count overflow")?;
    }
    Ok(Trace {
        source_commit,
        asset_sha,
        events,
    })
}

fn header_value(line: Option<&str>, key: &str, len: usize) -> Result<String, String> {
    let line = line.ok_or("missing trace header")?;
    let mut fields = line.split('\t');
    if fields.next() != Some(key) {
        return Err("trace header order mismatch".into());
    }
    let value = fields.next().ok_or("missing trace header value")?;
    if fields.next().is_some() || !is_lower_hex(value, len) {
        return Err("invalid trace digest or commit".into());
    }
    Ok(value.to_owned())
}

fn parse_event(line: &str) -> Result<Event, String> {
    let fields: Vec<&str> = line.split('\t').collect();
    match fields.first().copied() {
        Some("A") if fields.len() == 2 => {
            let target_ms: u64 = number(fields.get(1).copied())?;
            if target_ms > 3_600_000 {
                return Err("advance timestamp exceeds scenario duration".into());
            }
            Ok(Event::Advance(target_ms))
        }
        Some("K") if fields.len() == 5 => {
            let usage = number(fields.get(1).copied())?;
            let modifiers: u32 = number(fields.get(2).copied())?;
            let down = bit(fields.get(3).copied())?;
            let repeat = bit(fields.get(4).copied())?;
            if usage > 0xe7 || modifiers & !15 != 0 {
                return Err("keyboard event outside F2TRACE1 bounds".into());
            }
            Ok(Event::Key {
                usage,
                modifiers: normalize_host_key_modifiers(modifiers),
                down,
                repeat,
            })
        }
        Some("P") if fields.len() == 5 => {
            let x = signed(fields.get(1).copied())?;
            let y = signed(fields.get(2).copied())?;
            let buttons = number(fields.get(3).copied())?;
            let raw_kind: u32 = number(fields.get(4).copied())?;
            if !(0..1280).contains(&x) || !(0..720).contains(&y) {
                return Err("pointer coordinates outside F2TRACE1 bounds".into());
            }
            if !matches!(raw_kind, 1..=4) {
                return Err("pointer kind outside F2TRACE1 bounds".into());
            }
            let kind = match raw_kind {
                1 => 1,
                2 => 2,
                3 => 0,
                4 => 3,
                _ => return Err("bad pointer kind".into()),
            };
            Ok(Event::Pointer {
                x,
                y,
                buttons,
                kind,
            })
        }
        _ => Err("malformed F2TRACE1 event".into()),
    }
}

fn number<T: std::str::FromStr>(text: Option<&str>) -> Result<T, String> {
    let text = text.ok_or("missing integer field")?;
    if !(text == "0"
        || (!text.is_empty()
            && text
                .as_bytes()
                .first()
                .is_some_and(|b| (b'1'..=b'9').contains(b))
            && text.bytes().all(|b| b.is_ascii_digit())))
    {
        return Err("integer field is not canonical unsigned decimal".into());
    }
    text.parse().map_err(|_| "invalid integer field".into())
}
fn signed(text: Option<&str>) -> Result<i32, String> {
    let text = text.ok_or("missing integer field")?;
    let bytes = text.as_bytes();
    let canonical = text == "0"
        || (bytes.first() == Some(&b'-')
            && bytes.get(1).is_some_and(|b| (b'1'..=b'9').contains(b))
            && bytes
                .get(2..)
                .is_some_and(|rest| rest.iter().all(u8::is_ascii_digit)))
        || (bytes.first().is_some_and(|b| (b'1'..=b'9').contains(b))
            && bytes.iter().all(u8::is_ascii_digit));
    if !canonical {
        return Err("integer field is not canonical signed decimal".into());
    }
    text.parse().map_err(|_| "invalid integer field".into())
}
fn bit(text: Option<&str>) -> Result<bool, String> {
    match text {
        Some("0") => Ok(false),
        Some("1") => Ok(true),
        _ => Err("boolean field must be 0 or 1".into()),
    }
}
fn parse_result(line: &str) -> Result<i32, String> {
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != 2 || fields.first() != Some(&"RESULT") {
        return Err("trace RESULT must contain one status".into());
    }
    signed(fields.get(1).copied())
}

fn parse_frame(line: Option<&str>, index: usize) -> Result<String, String> {
    let line = line.ok_or("missing trace FRAME")?;
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != 3
        || fields.first() != Some(&"FRAME")
        || number::<usize>(fields.get(1).copied())? != index
    {
        return Err("trace FRAME index/order mismatch".into());
    }
    let hash = *fields.get(2).ok_or("missing frame hash")?;
    if !is_lower_hex(hash, 64) {
        return Err("invalid frame hash".into());
    }
    Ok(hash.to_owned())
}

fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        if let Some(ch) = b"0123456789abcdef".get(usize::from(byte >> 4)) {
            out.push(char::from(*ch));
        }
        if let Some(ch) = b"0123456789abcdef".get(usize::from(byte & 15)) {
            out.push(char::from(*ch));
        }
    }
    out
}
fn push_row(out: &mut String, fields: &[&str]) {
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push('\t');
        }
        out.push_str(field);
    }
    out.push('\n');
}
fn display_error(error: EngineError) -> String {
    error.to_string()
}

fn write_new_private(path: &PathBuf, bytes: &[u8]) -> Result<(), String> {
    oa_sealed::refuse_io(path).map_err(|_| "result path refused by sealed-path guard")?;
    // seal-allow: create-new result output after sealed-path refusal; no existing file is read or replaced
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "output must be a new path".to_owned())?;
    if file.write_all(bytes).is_err() || file.sync_all().is_err() {
        // Preserve the create-new partial artifact: unlink-by-name could remove a replacement.
        return Err("result write or sync failed".into());
    }
    Ok(())
}

#[cfg(test)]
mod trace_tests {
    use super::*;

    #[test]
    fn reads_the_documented_browser_result_status_and_refuses_extra_fields() {
        assert_eq!(parse_result("RESULT\t0"), Ok(0));
        assert_eq!(parse_result("RESULT\t-10"), Ok(-10));
        assert!(parse_result("RESULT\t1\t0").is_err());
        assert!(parse_result("RESULT\t-0").is_err());
    }
}

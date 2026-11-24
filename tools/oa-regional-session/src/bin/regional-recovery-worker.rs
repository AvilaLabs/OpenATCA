//! Test-only process worker for synthetic regional-runtime recovery. Shell (B1/B3/B7).
//! This is a local fault-injection protocol, not a production supervisor or remote wire API.

use oa_regional_session::cli::synthetic_model;
use oa_regional_session::runtime::{regional_new, regional_pixels, regional_step};
use oa_regional_session::surveillance::SurveillanceConfig;
use oa_regional_session::{flush_receipt_group, parse_event_line, push_event, session_cursor_new};
use std::io::{self, BufRead, BufReader, Write};

const FIXTURE: &str = include_str!("../../examples/synthetic-session.tsv");
const MAX_LINE: usize = 1024;
const SETTINGS: &str = "OA-REGIONAL-RECOVERY-SETTINGS\t1\ncentre\tZAU1\ncentre_lat_e7\t410000000\ncentre_lon_e7\t-870000000\nrange_nm\t180.0\ntracks\t32\nframes\t512\n";

fn setup() -> Result<(oa_regional_session::runtime::RegionalRuntime, Vec<oa_regional_session::ReceiptBatch>, [u8; 32], [u8; 32]), String> {
    let snapshot = oa_airspace::core::snapshot::encode(&synthetic_model()?)
        .map_err(|e| format!("snapshot encode: {e:?}"))?;
    let snapshot_hash = oa_sha256::sha256(&snapshot);
    let model = oa_airspace::core::snapshot::decode(&snapshot)
        .map_err(|e| format!("snapshot decode: {e:?}"))?;
    let cfg = SurveillanceConfig {
        centre_lat_e7: 410_000_000,
        centre_lon_e7: -870_000_000,
        range_nm: 180.0,
    };
    let runtime = regional_new(model, snapshot_hash, *b"ZAU1", cfg)?;
    let mut cursor = session_cursor_new();
    let mut batches = Vec::new();
    let mut rows = FIXTURE.lines();
    if rows.next() != Some("OA-RS\t1") {
        return Err("bad embedded fixture header".into());
    }
    for row in rows.filter(|row| !row.is_empty()) {
        let event = parse_event_line(row).map_err(|e| format!("fixture parse: {e:?}"))?;
        if let Some(batch) = push_event(&mut cursor, event)
            .map_err(|e| format!("fixture admission: {e:?}"))?
        {
            batches.push(batch);
        }
    }
    if let Some(batch) = flush_receipt_group(&mut cursor)
        .map_err(|e| format!("fixture close: {e:?}"))?
    {
        batches.push(batch);
    }
    if batches.is_empty() || batches.len() > 512 {
        return Err("fixture frame count outside bounded envelope".into());
    }
    Ok((runtime, batches, oa_sha256::sha256(SETTINGS.as_bytes()), snapshot_hash))
}

fn admitted_prefix_bytes(count: usize, batches: &[oa_regional_session::ReceiptBatch]) -> Result<Vec<u8>, String> {
    let cutoff = if count == 0 { None } else { Some(batches.get(count - 1).ok_or("replay count exceeds admitted prefix")?.receipt_ns) };
    let mut bytes = Vec::with_capacity(FIXTURE.len());
    for (index, line) in FIXTURE.lines().enumerate() {
        if index == 0 {
            bytes.extend_from_slice(line.as_bytes());
            bytes.push(b'\n');
            continue;
        }
        let receipt = line.split('\t').nth(1).ok_or("malformed embedded fixture row")?
            .parse::<i64>().map_err(|_| "invalid embedded receipt time")?;
        if cutoff.is_some_and(|value| receipt <= value) {
            bytes.extend_from_slice(line.as_bytes());
            bytes.push(b'\n');
        }
    }
    Ok(bytes)
}

fn read_line(reader: &mut BufReader<io::Stdin>) -> io::Result<Option<String>> {
    let mut bytes = Vec::with_capacity(MAX_LINE);
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(io::ErrorKind::UnexpectedEof, "unterminated protocol line"))
            };
        }
        let take = available.iter().position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if bytes.len().saturating_add(take) > MAX_LINE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "protocol line over bound"));
        }
        let complete = available.get(take - 1) == Some(&b'\n');
        bytes.extend_from_slice(&available[..take]);
        reader.consume(take);
        if complete {
            break;
        }
    }
    bytes.pop();
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "protocol is not UTF-8"))
}

fn main() {
    if let Err(error) = run() {
        let _ = writeln!(io::stdout().lock(), "ERROR {}", error.replace('\n', " ").replace('\r', " "));
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        return Err("expected one positive generation argument".into());
    }
    let generation = args[1].parse::<u64>().map_err(|_| "invalid generation")?;
    if generation == 0 {
        return Err("generation must be positive".into());
    }
    let (mut runtime, batches, settings_hash, snapshot_hash) = setup()?;
    let mut processed = 0usize;
    let mut replayed = false;
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    while let Some(line) = read_line(&mut reader).map_err(|e| e.to_string())? {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["REPLAY", gen, count, prefix_hash, settings, snapshot] if !replayed => {
                if gen.parse::<u64>().ok() != Some(generation) || settings != oa_sha256::to_hex(&settings_hash) || snapshot != oa_sha256::to_hex(&snapshot_hash) {
                    return Err("replay settings, adaptation, or generation mismatch".into());
                }
                let count = count.parse::<usize>().map_err(|_| "invalid replay count")?;
                if count > batches.len() {
                    return Err("replay count exceeds admitted prefix".into());
                }
                let prefix_bytes = admitted_prefix_bytes(count, &batches)?;
                if prefix_hash != oa_sha256::to_hex(&oa_sha256::sha256(&prefix_bytes)) {
                    return Err("replay prefix digest mismatch".into());
                }
                for batch in batches.iter().take(count) {
                    let frame = regional_step(&mut runtime, batch)?;
                    processed += 1;
                    let pixels = regional_pixels(&runtime)?;
                    let pixels_hash = oa_sha256::sha256(pixels);
                    let state_hash = oa_sha256::sha256(format!("{frame:?}").as_bytes());
                    writeln!(stdout, "REPLAYED\t{generation}\t{}\t{}\t{}\t{}\t{}", processed, frame.sequence, oa_sha256::to_hex(&frame.surveillance.frame_sha256), oa_sha256::to_hex(&pixels_hash), oa_sha256::to_hex(&state_hash)).map_err(|e| e.to_string())?;
                    stdout.flush().map_err(|e| e.to_string())?;
                }
                replayed = true;
                if count == 0 {
                    writeln!(stdout, "REPLAYED\t{generation}\t0\t0\t-\t-\t-").map_err(|e| e.to_string())?;
                    stdout.flush().map_err(|e| e.to_string())?;
                }
            }
            ["STEP", gen, index] if replayed => {
                if gen.parse::<u64>().ok() != Some(generation) {
                    return Err("step generation mismatch".into());
                }
                let index = index.parse::<usize>().map_err(|_| "invalid step index")?;
                if index != processed || index >= batches.len() {
                    return Err("step is not the next retained input".into());
                }
                let frame = regional_step(&mut runtime, &batches[index])?;
                processed += 1;
                let pixels = regional_pixels(&runtime)?;
                let pixel_hash = oa_sha256::sha256(pixels);
                let state_hash = oa_sha256::sha256(format!("{frame:?}").as_bytes());
                let trajectory_rows = frame.flights.iter().filter(|flight| flight.trajectory.is_some()).count();
                let source_lag_ns = frame.surveillance.track_measurement_seen_ns
                    .map(|seen| frame.surveillance.time_ns.saturating_sub(seen))
                    .unwrap_or(0);
                let selected = frame.flights.iter().find(|flight| flight.address == 0xABC101).and_then(|flight| flight.view);
                let handoff_state = selected.map_or(u8::MAX, |view| view.handoff_state);
                let owner = selected.map_or("-", |view| {
                    std::str::from_utf8(&view.owner_unit).unwrap_or("?").trim_end_matches('\0')
                });
                writeln!(stdout, "OUTPUT\t{generation}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}", frame.sequence, frame.input.receipt_ns, oa_sha256::to_hex(&frame.surveillance.frame_sha256), oa_sha256::to_hex(&pixel_hash), oa_sha256::to_hex(&state_hash), frame.surveillance.tracks.len(), frame.flight_report.flights, trajectory_rows, frame.surveillance.ca_open.len(), source_lag_ns, frame.controller_dispatches.len(), handoff_state, owner).map_err(|e| e.to_string())?;
                stdout.flush().map_err(|e| e.to_string())?;
            }
            ["APPLY_ONLY", gen, index] if replayed => {
                if gen.parse::<u64>().ok() != Some(generation) {
                    return Err("step generation mismatch".into());
                }
                let index = index.parse::<usize>().map_err(|_| "invalid step index")?;
                if index != processed || index >= batches.len() {
                    return Err("step is not the next retained input".into());
                }
                let frame = regional_step(&mut runtime, &batches[index])?;
                processed += 1;
                writeln!(stdout, "APPLIED\t{generation}\t{}\t{}", processed, frame.sequence).map_err(|e| e.to_string())?;
                stdout.flush().map_err(|e| e.to_string())?;
            }
            ["STALL"] if replayed => {
                // The coordinator applies a named response deadline and kills this worker on
                // expiry. No unbounded wait is treated as a successful process response.
                let _ = read_line(&mut reader).map_err(|e| e.to_string())?;
            }
            ["ACK", gen, sequence] if replayed => {
                if gen.parse::<u64>().ok() != Some(generation) || sequence.parse::<usize>().ok() != Some(processed) {
                    return Err("acknowledgment fence mismatch".into());
                }
                writeln!(stdout, "ACKED\t{generation}\t{processed}").map_err(|e| e.to_string())?;
                stdout.flush().map_err(|e| e.to_string())?;
            }
            ["EXIT"] => return Ok(()),
            _ => return Err("malformed or out-of-order protocol record".into()),
        }
    }
    Ok(())
}

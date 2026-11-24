#![forbid(unsafe_code)]
//! Decode one adsb.lol V2 JSON body from stdin and emit one OAF2 MESSAGE frame with an ADS3
//! payload. Raw-body retention, HTTP receipt headers and the completion manifest belong to the
//! bounded capture wrapper; this process performs no network or filesystem I/O.

use std::io::{self, Read, Write};
use std::process::ExitCode;

use oa_adsb::{
    decode_snapshot, encode_payload, reports_len, reports_row, reports_push,
    Reports, MAX_SNAPSHOT_BYTES, MAX_ROWS_PER_FRAME, ZAU_BOX,
};
use oa_feed::{encode_frame_mono, FLAG_BYTES, MAX_FRAME_LEN};

fn usage() -> &'static str {
    "usage: oa-adsb-snapshot --received-unix-ms N --received-mono-ns N --seq N [--max-age-ms N]"
}

fn run() -> Result<(), String> {
    let mut received_ms = None;
    let mut mono_ns = None;
    let mut seq = None;
    let mut max_age_ms = 15_000u32;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let value = |it: &mut std::iter::Skip<std::env::Args>, name: &str| {
            it.next().ok_or_else(|| format!("{name} needs a value"))
        };
        match a.as_str() {
            "--received-unix-ms" => received_ms = Some(value(&mut args, "--received-unix-ms")?.parse::<i64>().map_err(|_| "bad received unix milliseconds".to_string())?),
            "--received-mono-ns" => mono_ns = Some(value(&mut args, "--received-mono-ns")?.parse::<u64>().map_err(|_| "bad monotonic nanoseconds".to_string())?),
            "--seq" => seq = Some(value(&mut args, "--seq")?.parse::<u64>().map_err(|_| "bad frame sequence".to_string())?),
            "--max-age-ms" => max_age_ms = value(&mut args, "--max-age-ms")?.parse::<u32>().map_err(|_| "bad age limit".to_string())?,
            "--help" | "-h" => { eprintln!("{}", usage()); return Ok(()); }
            _ => return Err(format!("unknown argument `{a}`\n{}", usage())),
        }
    }
    let received_ms = received_ms.ok_or_else(|| format!("missing --received-unix-ms\n{}", usage()))?;
    let mono_ns = mono_ns.ok_or_else(|| format!("missing --received-mono-ns\n{}", usage()))?;
    let seq = seq.ok_or_else(|| format!("missing --seq\n{}", usage()))?;
    let mut body = Vec::with_capacity(MAX_SNAPSHOT_BYTES);
    io::stdin().take((MAX_SNAPSHOT_BYTES + 1) as u64).read_to_end(&mut body).map_err(|e| e.to_string())?;
    if body.len() > MAX_SNAPSHOT_BYTES { return Err(format!("input exceeds {MAX_SNAPSHOT_BYTES} bytes")); }
    let mut reports = Reports::default();
    let st = decode_snapshot(&body, received_ms, max_age_ms, &ZAU_BOX, &mut reports).map_err(|e| e.to_string())?;
    let n = reports_len(&reports);
    if n == 0 {
        eprintln!("snapshot: bytes={} reported={} accepted=0 no_position={} last_position_only={} invalid_address={} invalid_age={} too_old={} invalid_position={} outside_box={} reserved={} sealed_window={} non_adsb={} mlat_fields={} tisb_fields={} bad_optional={}", st.bytes, st.aircraft_reported, st.no_position, st.last_position_only, st.invalid_address, st.invalid_age, st.too_old, st.invalid_position, st.outside_box, st.reserved_address, st.sealed_window_dropped, st.non_adsb_position, st.with_mlat_fields, st.with_tisb_fields, st.bad_optional_fields);
        return Ok(());
    }
    if n > MAX_ROWS_PER_FRAME { return Err("snapshot row cap exceeds one ADS3 frame".into()); }
    let mut ix: Vec<u32> = (0..n as u32).collect();
    ix.sort_by_key(|&i| { let r = reports_row(&reports, i as usize).unwrap_or(oa_adsb::ROW_EMPTY); (r.time_ns, r.icao) });
    let mut ordered = Reports::default();
    for i in ix { if let Some(r) = reports_row(&reports, i as usize) { reports_push(&mut ordered, &r); } }
    let mut payload = Vec::new();
    encode_payload(&mut payload, &ordered, 0, reports_len(&ordered)).map_err(|e| e.to_string())?;
    let mut frame = vec![0u8; MAX_FRAME_LEN];
    let len = encode_frame_mono(&mut frame, 1, FLAG_BYTES, seq, received_ms.saturating_mul(1_000_000), received_ms, mono_ns, &payload).ok_or("OAF frame overflow")?;
    io::stdout().write_all(frame.get(..len).ok_or("short encoded frame")?).and_then(|()| io::stdout().flush()).map_err(|e| e.to_string())?;
    eprintln!("snapshot: bytes={} server_now_ms={} server_cache_ms={} reported={} accepted={} no_position={} last_position_only={} invalid_address={} invalid_age={} too_old={} invalid_position={} outside_box={} reserved={} sealed_window={} non_adsb={} mlat_fields={} tisb_fields={} bad_optional={} seen_pos_ms_min={} seen_pos_ms_max={} source_types={:?}", st.bytes, st.server_now_ms, st.server_cache_ms, st.aircraft_reported, st.accepted, st.no_position, st.last_position_only, st.invalid_address, st.invalid_age, st.too_old, st.invalid_position, st.outside_box, st.reserved_address, st.sealed_window_dropped, st.non_adsb_position, st.with_mlat_fields, st.with_tisb_fields, st.bad_optional_fields, st.min_seen_pos_ms, st.max_seen_pos_ms, st.source_types);
    Ok(())
}

fn main() -> ExitCode {
    match run() { Ok(()) => ExitCode::SUCCESS, Err(e) => { eprintln!("oa-adsb-snapshot: {e}"); ExitCode::FAILURE } }
}

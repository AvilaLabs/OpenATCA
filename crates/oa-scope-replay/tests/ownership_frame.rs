//! Full replay composition: current ownership remains visible beside degraded/frozen warnings.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::{reports_push, Reports, ROW_EMPTY};
use oa_airspace::model::{empty_model, Meta, Model};
use oa_display::ownership::{OwnershipPhase, OwnershipStatus, LAYER_OWNERSHIP, OWNERSHIP_ID_BYTES};
use oa_display::palette;
use oa_display::raster::{frame_digest, FrameSpec};
use oa_display::safety::{FeedState, FeedStatus};
use oa_display::traffic::DEFAULT_STYLE;
use oa_safety::inhibit::inhibits_new;
use oa_safety::net::{state_report, state_table_new, Area, NetId, NetReport, Reason, NETS};
use oa_scope_replay::adapt::{callsigns_from_reports, row_columns, ColumnBuf};
use oa_scope_replay::frame::{
    replay_build_with_overlays, replay_build_with_safety, replay_new, replay_raster, ReplayConfig,
    ReplaySafety,
};
use oa_scope_replay::rows::{Callsigns, RowStore, DEFAULT_CONVERT};
use oa_scope_replay::store::{store_add, store_finish};
use oa_track::{output_new, TrackOutput, O_HAS_ALT, O_TICK};

const S: i64 = 1_000_000_000;
const T0: i64 = 1_794_772_800 * S;
const ADDR: u32 = 0xA00001;
const CALLSIGN: [u8; 8] = *b"DAL56\0\0\0";
const WIDTH: u32 = 800;
const HEIGHT: u32 = 360;
const SPEC: FrameSpec = FrameSpec {
    width: WIDTH,
    height: HEIGHT,
    stride: WIDTH * 4,
};

fn model() -> Model {
    empty_model(Meta {
        nasr_effective: 20_261_029,
        cifp_cycle: 2611,
        artcc: "ZAU".into(),
        buffer_m: 0,
        nasr_sha256: [1; 32],
        cifp_sha256: [2; 32],
    })
}

fn output() -> TrackOutput {
    let mut out = output_new(4);
    out.t_ns.push(T0);
    out.addr.push(ADDR);
    out.ident.push(u16::MAX);
    out.ident_ns.push(i64::MIN);
    out.ident_conflict.push(0);
    out.lat_e7.push(419_786_000);
    out.lon_e7.push(-879_048_000);
    out.vel_east_mps.push(40.0);
    out.vel_north_mps.push(0.0);
    out.pos_cov_m2.push([100.0, 0.0, 100.0]);
    out.vel_cov_m2s2.push([1.0, 0.0, 1.0]);
    out.turn_rate_rps.push(0.0);
    out.alt_ft.push(12_000.0);
    out.alt_var_ft2.push(100.0);
    out.vrate_fpm.push(0.0);
    out.mu_h.push([1.0, 0.0]);
    out.mu_v.push([1.0, 0.0]);
    out.flags.push(O_HAS_ALT | O_TICK);
    out
}

fn store_of(out: &TrackOutput) -> RowStore {
    let mut store = RowStore::default();
    store_add(
        &mut store,
        &row_columns(out, &mut ColumnBuf::default()),
        i64::MIN,
    )
    .unwrap();
    assert_eq!(store_finish(&mut store), 0);
    store
}

fn callsigns() -> Callsigns {
    let mut reports = Reports::default();
    let mut row = ROW_EMPTY;
    row.time_ns = T0;
    row.icao = ADDR;
    row.callsign = CALLSIGN;
    reports_push(&mut reports, &row);
    callsigns_from_reports(&reports).unwrap().0
}

fn safety_inputs() -> (
    oa_safety::net::StateTable,
    oa_safety::inhibit::InhibitTable,
    [FeedStatus; 1],
) {
    let mut states = state_table_new(10 * S, 5 * S).unwrap();
    for net in NETS {
        let report = if net == NetId::Msaw {
            NetReport::Degraded {
                reason: Reason::BudgetExceeded,
                area: Area::Whole,
                count: 1,
            }
        } else {
            NetReport::Running
        };
        state_report(&mut states, net, report, T0, T0).unwrap();
    }
    let feeds = [FeedStatus {
        name: *b"ADSB\0\0\0\0",
        state: FeedState::Lost { age_ms: 2_000 },
    }];
    (states, inhibits_new(), feeds)
}

fn id(s: &str) -> [u8; OWNERSHIP_ID_BYTES] {
    let mut out = [0; OWNERSHIP_ID_BYTES];
    for (to, from) in out.iter_mut().zip(s.bytes()) {
        *to = from;
    }
    out
}

fn texts(replay: &oa_scope_replay::frame::Replay) -> Vec<&str> {
    (0..replay.list.texts.x.len())
        .map(|i| {
            let start = usize::try_from(replay.list.texts.start[i]).unwrap();
            let len = usize::try_from(replay.list.texts.len[i]).unwrap();
            replay.list.text.get(start..start + len).unwrap()
        })
        .collect()
}

fn text_index(replay: &oa_scope_replay::frame::Replay, text: &str) -> usize {
    texts(replay)
        .iter()
        .position(|candidate| *candidate == text)
        .unwrap()
}

/// Count exact palette pixels inside the raster bounds of one right- or left-aligned text entry.
fn text_pixels(replay: &oa_scope_replay::frame::Replay, entry: usize) -> usize {
    let bytes = palette::bytes_of(replay.list.texts.colour[entry]).unwrap();
    let len = usize::try_from(replay.list.texts.len[entry]).unwrap();
    let width16 = oa_display::font::text_width_units(len)
        .saturating_mul(i32::from(replay.list.texts.size[entry]))
        .saturating_mul(16)
        / oa_display::font::CAP_HEIGHT;
    let anchor = replay.list.texts.x[entry];
    let (left16, right16) = match replay.list.texts.align[entry] {
        oa_display::list::ALIGN_RIGHT => (anchor - width16, anchor),
        oa_display::list::ALIGN_LEFT => (anchor, anchor + width16),
        _ => panic!("fixture text uses an unexpected alignment"),
    };
    let x0 = usize::try_from((left16.max(0) / 16) as u32).unwrap();
    let x1 = usize::try_from(((right16.max(0) + 15) / 16) as u32)
        .unwrap()
        .min(WIDTH as usize);
    let y0 = usize::try_from((replay.list.texts.y[entry].max(0) / 16) as u32).unwrap();
    let y1 = usize::try_from(
        ((replay.list.texts.y[entry] + i32::from(replay.list.texts.size[entry]) * 16 + 15).max(0)
            / 16) as u32,
    )
    .unwrap()
    .min(HEIGHT as usize);
    let mut count = 0;
    for y in y0.min(HEIGHT as usize)..y1 {
        for x in x0.min(WIDTH as usize)..x1 {
            let start = y * SPEC.stride as usize + x * 4;
            if replay.buf.get(start..start + 4) == Some(bytes.as_slice()) {
                count += 1;
            }
        }
    }
    count
}

fn text_is_in_frame(replay: &oa_scope_replay::frame::Replay, entry: usize) -> bool {
    let len = usize::try_from(replay.list.texts.len[entry]).unwrap();
    let width16 = oa_display::font::text_width_units(len)
        .saturating_mul(i32::from(replay.list.texts.size[entry]))
        .saturating_mul(16)
        / oa_display::font::CAP_HEIGHT;
    let anchor = replay.list.texts.x[entry];
    let (left16, right16) = match replay.list.texts.align[entry] {
        oa_display::list::ALIGN_RIGHT => (anchor - width16, anchor),
        oa_display::list::ALIGN_LEFT => (anchor, anchor + width16),
        _ => return false,
    };
    let top16 = replay.list.texts.y[entry];
    left16 >= 0
        && right16 <= (WIDTH as i32) * 16
        && top16 >= 0
        && top16 + i32::from(replay.list.texts.size[entry]) * 16 <= (HEIGHT as i32) * 16
}

#[test]
fn ownership_after_timeout_stays_actionable_beside_frozen_and_degraded_warnings() {
    let mut replay = replay_new(
        model(),
        &ReplayConfig {
            centre_lat_e7: 419_786_000,
            centre_lon_e7: -879_048_000,
            range_nm: 30.0,
            width: WIDTH,
            height: HEIGHT,
            max_targets: 4,
            style: DEFAULT_STYLE,
            convert: DEFAULT_CONVERT,
        },
    )
    .unwrap();

    let (states, inhibits, feeds) = safety_inputs();
    let safety = ReplaySafety {
        states: &states,
        inhibits: &inhibits,
        feeds: &feeds,
        newest_track_ns: Some(T0),
        frozen_limit_ns: S,
    };
    let store = store_of(&output());
    let baseline = replay_build_with_safety(
        &mut replay,
        &store,
        &callsigns(),
        T0 + 2 * S,
        "T+2S",
        &safety,
    )
    .unwrap();
    assert_eq!(baseline.traffic.drawn, 1);
    assert!(!baseline.stamp_clipped);
    assert!(!baseline.ownership_clipped);
    replay_raster(&mut replay).unwrap();
    let baseline_digest = frame_digest(&replay.spec, &replay.buf).unwrap();

    let absent = replay_build_with_overlays(
        &mut replay,
        &store,
        &callsigns(),
        T0 + 2 * S,
        "T+2S",
        Some(&safety),
        None,
    )
    .unwrap();
    assert!(!absent.ownership_clipped);
    replay_raster(&mut replay).unwrap();
    assert_eq!(
        frame_digest(&replay.spec, &replay.buf).unwrap(),
        baseline_digest,
        "omitting ownership preserves the existing safety-frame pixels"
    );

    // A handoff timeout leaves ZAU12 as owner. This input is the current flight state after the
    // explicit timeout event; no NAM transport acknowledgment participates in this mapping.
    let owner = OwnershipStatus {
        callsign: id("DAL56"),
        current_owner: id("ZAU12"),
        phase: OwnershipPhase::Current,
    };
    let info = replay_build_with_overlays(
        &mut replay,
        &store,
        &callsigns(),
        T0 + 2 * S,
        "T+2S",
        Some(&safety),
        Some(&owner),
    )
    .unwrap();
    assert!(!info.ownership_clipped);
    replay_raster(&mut replay).unwrap();

    let labels = texts(&replay);
    let frozen = "PICTURE FROZEN 2.0S";
    let degraded = "MSAW DEGRADED: BUDGET EXCEEDED, WHOLE, COUNT 1, AGE 2S";
    let feed_lost = "FEED ADSB LOST 2.0S";
    let owner_line = "FLIGHT DAL56 OWNER ZAU12 | CURRENT";
    assert!(labels.contains(&frozen), "{labels:?}");
    assert!(labels.contains(&degraded), "{labels:?}");
    assert!(labels.contains(&feed_lost), "{labels:?}");
    assert!(labels.contains(&owner_line), "{labels:?}");
    assert!(labels
        .iter()
        .any(|label| label.starts_with("T+2S  CURRENT POSITIONS 1 / DRAWN 1")));

    let frozen_i = text_index(&replay, frozen);
    let degraded_i = text_index(&replay, degraded);
    let feed_i = text_index(&replay, feed_lost);
    let owner_i = text_index(&replay, owner_line);
    let stamp_i = labels
        .iter()
        .position(|label| label.starts_with("T+2S  CURRENT POSITIONS 1 / DRAWN 1"))
        .unwrap();
    assert_eq!(replay.list.texts.colour[frozen_i], palette::ALERT);
    assert_eq!(replay.list.texts.colour[degraded_i], palette::ALERT);
    assert_eq!(replay.list.texts.colour[owner_i], palette::TARGET);
    assert_eq!(replay.list.texts.layer[owner_i], LAYER_OWNERSHIP);
    assert!(replay.list.texts.x[owner_i] > 0);
    assert!(replay.list.texts.x[owner_i] < (WIDTH as i32) * 16);
    assert!(replay.list.texts.y[owner_i] > replay.list.texts.y[degraded_i]);
    assert!(replay.list.texts.y[owner_i] + 10 * 16 < (HEIGHT as i32) * 16);
    assert!(replay.list.texts.y[owner_i] + 10 * 16 < replay.list.texts.y[stamp_i]);
    assert!(text_is_in_frame(&replay, frozen_i));
    assert!(text_is_in_frame(&replay, degraded_i));
    assert!(text_is_in_frame(&replay, feed_i));
    assert!(text_is_in_frame(&replay, owner_i));
    assert!(text_is_in_frame(&replay, stamp_i));
    assert!(
        text_pixels(&replay, frozen_i) > 0,
        "frozen warning has raster pixels"
    );
    assert!(
        text_pixels(&replay, degraded_i) > 0,
        "degraded safety warning has raster pixels"
    );
    assert!(
        text_pixels(&replay, feed_i) > 0,
        "lost-feed warning has raster pixels"
    );
    assert!(
        text_pixels(&replay, owner_i) > 0,
        "owner line has raster pixels"
    );
    assert!(!replay.list.faulted);
    let digest = frame_digest(&replay.spec, &replay.buf).unwrap();
    replay_raster(&mut replay).unwrap();
    assert_eq!(frame_digest(&replay.spec, &replay.buf).unwrap(), digest);
}

#[test]
fn narrow_ownership_status_is_reported_as_clipped_without_a_partial_line() {
    let width = 250;
    let mut replay = replay_new(
        model(),
        &ReplayConfig {
            centre_lat_e7: 419_786_000,
            centre_lon_e7: -879_048_000,
            range_nm: 30.0,
            width,
            height: HEIGHT,
            max_targets: 4,
            style: DEFAULT_STYLE,
            convert: DEFAULT_CONVERT,
        },
    )
    .unwrap();
    let owner = OwnershipStatus {
        callsign: id("DAL56"),
        current_owner: id("ZAU12"),
        phase: OwnershipPhase::StaleOffer { to: id("ZMP34") },
    };
    let info = replay_build_with_overlays(
        &mut replay,
        &RowStore::default(),
        &Callsigns::default(),
        T0,
        "T",
        None,
        Some(&owner),
    )
    .unwrap();
    assert!(info.ownership_clipped);
    assert!(texts(&replay)
        .iter()
        .all(|text| !text.starts_with("FLIGHT DAL56 OWNER")));
    assert!(!replay.list.faulted);
}

#[test]
fn cold_start_requires_fresh_track_net_and_feed_inputs_independently() {
    let mut replay = replay_new(
        model(),
        &ReplayConfig {
            centre_lat_e7: 419_786_000,
            centre_lon_e7: -879_048_000,
            range_nm: 30.0,
            width: WIDTH,
            height: HEIGHT,
            max_targets: 4,
            style: DEFAULT_STYLE,
            convert: DEFAULT_CONVERT,
        },
    )
    .unwrap();
    let mut states = state_table_new(10 * S, 0).unwrap();
    let inhibits = inhibits_new();
    let mut feeds = [
        FeedStatus {
            name: *b"ADSB\0\0\0\0",
            state: FeedState::Lost { age_ms: 5_000 },
        },
        FeedStatus {
            name: *b"RADAR\0\0\0",
            state: FeedState::Lost { age_ms: 5_000 },
        },
    ];
    let empty = RowStore::default();

    let cold = replay_build_with_safety(
        &mut replay,
        &empty,
        &Callsigns::default(),
        T0,
        "START",
        &ReplaySafety {
            states: &states,
            inhibits: &inhibits,
            feeds: &feeds,
            newest_track_ns: None,
            frozen_limit_ns: 5 * S,
        },
    )
    .unwrap();
    assert_eq!(cold.traffic.drawn, 0);
    replay_raster(&mut replay).unwrap();
    let cold_labels = texts(&replay);
    assert!(cold_labels.contains(&"PICTURE FROZEN, NO TRACK UPDATE"));
    assert!(cold_labels.contains(&"MSAW UNAVAILABLE: NEVER REPORTED, AGE --"));
    assert!(cold_labels.contains(&"FEED ADSB LOST 5.0S"));
    assert!(cold_labels.contains(&"FEED RADAR LOST 5.0S"));

    // One synthetic accepted track update makes the picture fresh and restores ADS-B. The MSAW
    // net has not reported and the separate radar feed is still lost.
    feeds[0].state = FeedState::Live;
    feeds[1].state = FeedState::Lost { age_ms: 6_000 };
    let store = store_of(&output());
    let first_track = replay_build_with_safety(
        &mut replay,
        &store,
        &callsigns(),
        T0,
        "TRACK RETURNED",
        &ReplaySafety {
            states: &states,
            inhibits: &inhibits,
            feeds: &feeds,
            newest_track_ns: Some(T0),
            frozen_limit_ns: 5 * S,
        },
    )
    .unwrap();
    assert_eq!(first_track.traffic.drawn, 1);
    replay_raster(&mut replay).unwrap();
    let track_labels = texts(&replay);
    assert!(track_labels
        .iter()
        .all(|line| !line.starts_with("PICTURE ")));
    assert!(track_labels.contains(&"MSAW UNAVAILABLE: NEVER REPORTED, AGE --"));
    assert!(track_labels.contains(&"FEED RADAR LOST 6.0S"));
    assert!(track_labels
        .iter()
        .all(|line| !line.starts_with("FEED ADSB")));

    // A fresh MSAW report and independent radar recovery clear only their own warnings. The other
    // three nets remain unavailable because this startup supplied no reports for them.
    state_report(&mut states, NetId::Msaw, NetReport::Running, T0 + S, T0 + S).unwrap();
    feeds[1].state = FeedState::Live;
    let recovered = replay_build_with_safety(
        &mut replay,
        &store,
        &callsigns(),
        T0 + S,
        "HEALTH RETURNED",
        &ReplaySafety {
            states: &states,
            inhibits: &inhibits,
            feeds: &feeds,
            newest_track_ns: Some(T0),
            frozen_limit_ns: 5 * S,
        },
    )
    .unwrap();
    assert_eq!(recovered.traffic.drawn, 1);
    replay_raster(&mut replay).unwrap();
    let recovered_labels = texts(&replay);
    assert!(recovered_labels
        .iter()
        .all(|line| !line.starts_with("PICTURE ")));
    assert!(recovered_labels
        .iter()
        .all(|line| !line.starts_with("FEED ")));
    assert!(recovered_labels
        .iter()
        .all(|line| !line.starts_with("MSAW ")));
    assert!(recovered_labels
        .iter()
        .any(|line| line.starts_with("CONFLICT ALERT UNAVAILABLE: NEVER REPORTED")));
    assert!(recovered_labels
        .iter()
        .any(|line| line.starts_with("AREA PROXIMITY UNAVAILABLE: NEVER REPORTED")));
    assert!(recovered_labels
        .iter()
        .any(|line| line.starts_with("APPROACH PATH UNAVAILABLE: NEVER REPORTED")));
}

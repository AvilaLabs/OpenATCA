//! Feed freshness from observed OAF frames through the normal scope safety composition.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::io::Cursor;

use oa_airspace::model::{empty_model, Meta, Model};
use oa_display::safety::{FeedState, FeedStatus};
use oa_display::status_strip::{regional_status_plan, StatusStripRow};
use oa_display::traffic::DEFAULT_STYLE;
use oa_feed::health::{observe_frame, FeedHealth, STATE_CONNECTED};
use oa_feed::{encode_frame, read_frames, Kind, HEADER_LEN, TRAILER_LEN};
use oa_safety::inhibit::inhibits_new;
use oa_safety::net::state_table_new;
use oa_scope_replay::adapt::{feed_status_from_health, FeedFreshnessLimits};
use oa_scope_replay::frame::{
    replay_build_with_safety, replay_new, replay_new_with_top_inset, replay_raster, ReplayConfig,
    ReplaySafety,
};
use oa_scope_replay::rows::{Callsigns, RowStore, DEFAULT_CONVERT};

const S: i64 = 1_000_000_000;
const FEED_NAME: [u8; 8] = *b"ADSB\0\0\0\0";

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

fn append_frame(wire: &mut Vec<u8>, kind: Kind, seq: u64, recv_ns: i64, payload: &[u8]) {
    let mut bytes = vec![0; HEADER_LEN + payload.len() + TRAILER_LEN];
    let n = encode_frame(&mut bytes, kind as u8, 0, seq, recv_ns, 0, payload)
        .expect("small synthetic frame fits its exact buffer");
    wire.extend_from_slice(bytes.get(..n).expect("encoder length is in the buffer"));
}

fn observe_wire(wire: &[u8]) -> FeedHealth {
    let mut health = FeedHealth::new();
    let damaged = read_frames(&mut Cursor::new(wire), &mut |frame| {
        observe_frame(&mut health, frame, frame.recv_unix_ns);
    })
    .expect("synthetic wire reads");
    assert_eq!(damaged, 0);
    health
}

fn frame_texts(replay: &oa_scope_replay::frame::Replay) -> Vec<String> {
    replay
        .list
        .texts
        .start
        .iter()
        .zip(&replay.list.texts.len)
        .map(|(&start, &len)| {
            let start = usize::try_from(start).expect("text offset fits usize");
            let len = usize::try_from(len).expect("text length fits usize");
            replay
                .list
                .text
                .get(start..start + len)
                .expect("display text range is valid")
                .to_owned()
        })
        .collect()
}

fn render_feed_status(
    status: FeedStatus,
    now_ns: i64,
    newest_track_ns: Option<i64>,
) -> Vec<String> {
    let mut replay = replay_new(
        model(),
        &ReplayConfig {
            centre_lat_e7: 419_786_000,
            centre_lon_e7: -879_048_000,
            range_nm: 30.0,
            width: 800,
            height: 360,
            max_targets: 4,
            style: DEFAULT_STYLE,
            convert: DEFAULT_CONVERT,
        },
    )
    .expect("synthetic scope initializes");
    let states = state_table_new(10 * S, 0).expect("synthetic state table initializes");
    let inhibits = inhibits_new();
    let feeds = [status];
    replay_build_with_safety(
        &mut replay,
        &RowStore::default(),
        &Callsigns::default(),
        now_ns,
        "FEED FRESHNESS FIXTURE",
        &ReplaySafety {
            states: &states,
            inhibits: &inhibits,
            feeds: &feeds,
            newest_track_ns,
            frozen_limit_ns: 5 * S,
            use_reserved_status_header: false,
            traffic_block_policies: None,
        },
    )
    .expect("synthetic scope frame builds");
    replay_raster(&mut replay).expect("synthetic scope frame rasterizes");
    frame_texts(&replay)
}

#[test]
fn status_heartbeat_does_not_refresh_message_age_or_clear_frozen_picture() {
    let limits = FeedFreshnessLimits::new(5 * S, 10 * S).unwrap();
    let mut wire = Vec::new();
    append_frame(&mut wire, Kind::Message, 0, 100 * S, b"synthetic report");
    append_frame(&mut wire, Kind::Status, 1, 110 * S, b"state=connected\n");
    let health = observe_wire(&wire);
    assert_eq!(health.last_recv_ns, 110 * S);
    assert_eq!(health.last_message_recv_ns, Some(100 * S));
    assert_eq!(health.state, STATE_CONNECTED);

    let now_ns = 120 * S;
    let lost = feed_status_from_health(FEED_NAME, &health, now_ns, limits);
    assert_eq!(lost.state, FeedState::Lost { age_ms: 20_000 });
    let texts = render_feed_status(lost, now_ns, Some(110 * S));
    assert!(texts.iter().any(|line| line == "FEED ADSB LOST 20.0S"));
    assert!(texts
        .iter()
        .any(|line| line.starts_with("PICTURE FROZEN 10.0S")));

    let mut status_only_wire = Vec::new();
    append_frame(
        &mut status_only_wire,
        Kind::Status,
        0,
        119 * S,
        b"state=connected\n",
    );
    let status_only = observe_wire(&status_only_wire);
    let no_message = feed_status_from_health(FEED_NAME, &status_only, now_ns, limits);
    assert_eq!(no_message.state, FeedState::NoMessageSeen);
    let texts = render_feed_status(no_message, now_ns, None);
    assert!(texts.iter().any(|line| line == "FEED ADSB NO MESSAGE SEEN"));
    assert!(texts.iter().any(|line| line.starts_with("PICTURE FROZEN")));

    let mut rejected_wire = Vec::new();
    append_frame(
        &mut rejected_wire,
        Kind::Reject,
        0,
        119 * S,
        b"reason=unsupported_type",
    );
    let rejected_health = observe_wire(&rejected_wire);
    let rejected = feed_status_from_health(FEED_NAME, &rejected_health, now_ns, limits);
    assert_eq!(rejected.state, FeedState::RejectedInput { count: 1 });
    let texts = render_feed_status(rejected, now_ns, None);
    assert!(texts.iter().any(|line| line == "FEED ADSB REJECTED 1"));

    let mut fresh_wire = Vec::new();
    append_frame(
        &mut fresh_wire,
        Kind::Message,
        0,
        119 * S,
        b"synthetic report",
    );
    let fresh = observe_wire(&fresh_wire);
    let live = feed_status_from_health(FEED_NAME, &fresh, now_ns, limits);
    assert_eq!(live.state, FeedState::Live);
    assert_eq!(
        feed_status_from_health(FEED_NAME, &fresh, 124 * S, limits).state,
        FeedState::Slow { age_ms: 5_000 }
    );
    assert_eq!(
        feed_status_from_health(FEED_NAME, &fresh, 129 * S, limits).state,
        FeedState::Lost { age_ms: 10_000 }
    );
    let texts = render_feed_status(live, now_ns, Some(110 * S));
    assert!(!texts.iter().any(|line| line.starts_with("FEED ADSB ")));
    assert!(texts
        .iter()
        .any(|line| line.starts_with("PICTURE FROZEN 10.0S")));

    let future = feed_status_from_health(FEED_NAME, &fresh, 118 * S, limits);
    assert_eq!(future.state, FeedState::ClockMismatch);
    let texts = render_feed_status(future, now_ns, Some(110 * S));
    assert!(texts.iter().any(|line| line == "FEED ADSB CLOCK MISMATCH"));

    let too_old = feed_status_from_health(FEED_NAME, &fresh, i64::MAX, limits);
    assert_eq!(too_old.state, FeedState::LostAgeOverflow);
    let texts = render_feed_status(too_old, now_ns, Some(110 * S));
    assert!(texts.iter().any(|line| line == "FEED ADSB LOST >49D"));

    let mut disconnected_health = health.clone();
    disconnected_health.state = oa_feed::health::STATE_DISCONNECTED;
    let disconnected = feed_status_from_health(FEED_NAME, &disconnected_health, now_ns, limits);
    assert_eq!(disconnected.state, FeedState::Disconnected);
    let texts = render_feed_status(disconnected, now_ns, Some(110 * S));
    assert!(texts.iter().any(|line| line == "FEED ADSB DISCONNECTED"));
}

#[test]
fn reserved_status_header_uses_compact_band_only_when_explicitly_enabled() {
    let status_plan = regional_status_plan().expect("shared status plan is valid");
    let safety_rows = [
        status_plan
            .row(StatusStripRow::Safety(0))
            .expect("first safety row is reserved")
            .y_px()
            * 16,
        status_plan
            .row(StatusStripRow::Safety(1))
            .expect("second safety row is reserved")
            .y_px()
            * 16,
    ];
    let mut replay = replay_new_with_top_inset(
        model(),
        &ReplayConfig {
            centre_lat_e7: 419_786_000,
            centre_lon_e7: -879_048_000,
            range_nm: 30.0,
            width: 800,
            height: 600,
            max_targets: 4,
            style: DEFAULT_STYLE,
            convert: DEFAULT_CONVERT,
        },
        status_plan.top_inset_px(),
    )
    .expect("synthetic scope with reserved status band initializes");
    let states = state_table_new(10 * S, 0).expect("synthetic state table initializes");
    let inhibits = inhibits_new();
    let feeds = [FeedStatus {
        name: FEED_NAME,
        state: FeedState::Lost { age_ms: 20_000 },
    }];
    replay_build_with_safety(
        &mut replay,
        &RowStore::default(),
        &Callsigns::default(),
        120 * S,
        "RESERVED STATUS HEADER FIXTURE",
        &ReplaySafety {
            states: &states,
            inhibits: &inhibits,
            feeds: &feeds,
            newest_track_ns: None,
            frozen_limit_ns: 5 * S,
            use_reserved_status_header: true,
            traffic_block_policies: None,
        },
    )
    .expect("explicit regional-style header builds");

    let mut status_rows = 0usize;
    let mut summary_visible = false;
    for ((&start, &len), &y) in replay
        .list
        .texts
        .start
        .iter()
        .zip(&replay.list.texts.len)
        .zip(&replay.list.texts.y)
    {
        let start = usize::try_from(start).expect("text offset fits usize");
        let len = usize::try_from(len).expect("text length fits usize");
        let text = replay
            .list
            .text
            .get(start..start + len)
            .expect("display text span is valid");
        if text.starts_with("PICTURE ") || text.starts_with("NET ") || text.starts_with("FEED ") {
            status_rows += 1;
            assert!(safety_rows.contains(&y));
        }
        summary_visible |= text.starts_with('+') && text.contains("MORE STATES NOT SHOWN");
    }
    assert!(status_rows > 0);
    assert!(summary_visible);
    replay_raster(&mut replay).expect("explicitly placed status frame rasterizes");
}

#[test]
fn feed_freshness_limits_are_explicit_and_ordered() {
    assert!(FeedFreshnessLimits::new(0, S).is_err());
    assert!(FeedFreshnessLimits::new(S, S).is_err());
    assert!(FeedFreshnessLimits::new(S, 2 * S).is_ok());
}

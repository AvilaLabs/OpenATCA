//! Synthetic outage drill from tracker measurements through the scope's safety display.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use oa_airspace::model::{empty_model, Meta};
use oa_display::safety::{FeedState, FeedStatus};
use oa_display::traffic::{targets_columns, DEFAULT_STYLE, T_COAST};
use oa_safety::inhibit::{inhibits_new, InhibitTable};
use oa_safety::net::{state_report, state_table_new, NetId, NetReport, StateTable, NETS};
use oa_scope_replay::adapt::{row_columns, ColumnBuf};
use oa_scope_replay::frame::{
    replay_build_with_safety, replay_new, replay_raster, Replay, ReplayConfig, ReplaySafety,
};
use oa_scope_replay::rows::{Callsigns, RowStore, DEFAULT_CONVERT};
use oa_scope_replay::store::{store_add, store_finish};
use oa_track::{
    output_new, tracker_advance, tracker_new, tracker_newest_update_ns, tracker_stats,
    tracker_update, Measurement, TrackOutput, Tracker, DEFAULT_CONFIG, M_HAS_VEL, NONE_I32,
    NO_IDENT, O_REACQUIRED,
};

const S: i64 = 1_000_000_000;
const EPOCH: i64 = 1_700_000_000 * S;
const A: u32 = 0xA00001;
const B: u32 = 0xA00002;
const LAT: i32 = 419_000_000;
const LON: i32 = -879_000_000;

struct Drill {
    tracker: Tracker,
    out: TrackOutput,
    scope: Replay,
    states: StateTable,
    inhibits: Box<InhibitTable>,
    feeds: [FeedStatus; 2],
}

impl Drill {
    fn new() -> Self {
        let tracker = tracker_new(&oa_track::TrackerConfig {
            max_tracks: 8,
            out_capacity: 256,
            log_capacity: 16,
            confirm_pool: 8,
            ..DEFAULT_CONFIG
        })
        .unwrap();
        let model = empty_model(Meta {
            nasr_effective: 0,
            cifp_cycle: 0,
            artcc: "ZAU".into(),
            buffer_m: 0,
            nasr_sha256: [0; 32],
            cifp_sha256: [0; 32],
        });
        let scope = replay_new(
            model,
            &ReplayConfig {
                centre_lat_e7: LAT,
                centre_lon_e7: LON,
                range_nm: 30.0,
                width: 640,
                height: 480,
                max_targets: 8,
                style: DEFAULT_STYLE,
                convert: DEFAULT_CONVERT,
            },
        )
        .unwrap();
        Self {
            tracker,
            out: output_new(256),
            scope,
            states: state_table_new(10 * S, 0).unwrap(),
            inhibits: Box::new(inhibits_new()),
            feeds: [
                FeedStatus {
                    name: *b"ADSB\0\0\0\0",
                    state: FeedState::Live,
                },
                FeedStatus {
                    name: *b"RADAR\0\0\0",
                    state: FeedState::Live,
                },
            ],
        }
    }

    fn offer(&mut self, addr: u32, sensor: u16, second: i32) {
        tracker_update(
            &mut self.tracker,
            &Measurement {
                toa_ns: EPOCH + i64::from(second) * S,
                pos_cov_m2: [100.0, 0.0, 100.0],
                vel_cov_m2s2: [4.0, 0.0, 4.0],
                vel_east_mps: 83.0,
                vel_north_mps: 0.0,
                lat_e7: LAT + i32::from(sensor) * 80_000,
                lon_e7: LON + second * 10_000,
                alt_ft: 12_000,
                vrate_fpm: NONE_I32,
                addr,
                alt_quant_ft: 25,
                sensor,
                ident: NO_IDENT,
                flags: M_HAS_VEL,
                category: 0,
                vel_lag_s: 0.0,
            },
            &mut self.out,
        );
    }

    fn frame(&mut self, second: i64, expected_warnings: &[&str]) {
        let at = EPOCH + second * S;
        tracker_advance(&mut self.tracker, at, &mut self.out);
        let mut store = RowStore::default();
        let stats = store_add(
            &mut store,
            &row_columns(&self.out, &mut ColumnBuf::default()),
            i64::MIN,
        )
        .unwrap();
        assert_eq!(stats.kept, stats.rows_in);
        assert_eq!(store_finish(&mut store), 0);
        let newest = tracker_newest_update_ns(&self.tracker);
        replay_build_with_safety(
            &mut self.scope,
            &store,
            &Callsigns::default(),
            at,
            "OUTAGE DRILL",
            &ReplaySafety {
                states: &self.states,
                inhibits: &self.inhibits,
                feeds: &self.feeds,
                newest_track_ns: (newest != i64::MIN).then_some(newest),
                frozen_limit_ns: 5 * S,
            },
        )
        .unwrap();
        replay_raster(&mut self.scope).unwrap();
        let list = &self.scope.list;
        let warnings: Vec<_> = list
            .texts
            .start
            .iter()
            .zip(&list.texts.len)
            .map(|(&start, &len)| &list.text[start as usize..start as usize + len as usize])
            .filter(|s| {
                s.starts_with("FEED ") || s.starts_with("PICTURE ") || s.starts_with("MSAW ")
            })
            .collect();
        assert_eq!(
            warnings, expected_warnings,
            "unexpected warnings at {second}s"
        );
        assert_eq!(targets_columns(&self.scope.set).addr, &[A, B]);
    }
}

// REQ: OA-DSP-043, OA-DSP-061, OA-DSP-063, OA-DSP-064, OA-TRK-343
#[test]
fn blackout_and_staggered_recovery_keep_tracks_and_clear_only_recovered_warnings() {
    let mut d = Drill::new();
    for second in 0..=10 {
        d.offer(A, 1, second);
        d.offer(B, 2, second);
    }
    let last_healthy = EPOCH + 10 * S;
    for net in NETS {
        state_report(
            &mut d.states,
            net,
            NetReport::Running,
            last_healthy,
            last_healthy,
        )
        .unwrap();
    }
    d.frame(10, &[]);
    let started = tracker_stats(&d.tracker).started;
    assert_eq!(started, 2);

    // Both feeds stop. Only MSAW loses its heartbeat. Tracker ticks still display coasting
    // positions, but they must not make the picture look fresh.
    let blackout = EPOCH + 40 * S;
    for net in NETS.into_iter().filter(|&net| net != NetId::Msaw) {
        state_report(&mut d.states, net, NetReport::Running, blackout, blackout).unwrap();
    }
    d.feeds[0].state = FeedState::Lost { age_ms: 30_000 };
    d.feeds[1].state = FeedState::Lost { age_ms: 30_000 };
    d.frame(
        40,
        &[
            "PICTURE FROZEN 30.0S",
            "MSAW UNAVAILABLE: STATE STALE, AGE 30S",
            "FEED ADSB LOST 30.0S",
            "FEED RADAR LOST 30.0S",
        ],
    );
    assert_eq!(tracker_newest_update_ns(&d.tracker), last_healthy);
    assert!(targets_columns(&d.scope.set)
        .flags
        .iter()
        .all(|&flag| flag & T_COAST != 0));

    // A feed becomes live before it supplies any accepted track update.
    d.feeds[0].state = FeedState::Live;
    d.feeds[1].state = FeedState::Lost { age_ms: 31_000 };
    d.frame(
        41,
        &[
            "PICTURE FROZEN 31.0S",
            "MSAW UNAVAILABLE: STATE STALE, AGE 31S",
            "FEED RADAR LOST 31.0S",
        ],
    );

    // ADS-B reacquires A, but RADAR's B is coasting and MSAW is still stale.
    d.offer(A, 1, 41);
    d.frame(
        41,
        &[
            "MSAW UNAVAILABLE: STATE STALE, AGE 31S",
            "FEED RADAR LOST 31.0S",
        ],
    );
    let flags = targets_columns(&d.scope.set).flags;
    assert_eq!(flags[0] & T_COAST, 0);
    assert_ne!(flags[1] & T_COAST, 0);

    d.feeds[1].state = FeedState::Live;
    d.offer(B, 2, 42);
    d.frame(42, &["MSAW UNAVAILABLE: STATE STALE, AGE 32S"]);
    for (addr, second) in [(A, 41), (B, 42)] {
        assert!(d.out.addr.iter().enumerate().any(|(i, &stored)| {
            stored == addr
                && d.out.t_ns[i] == EPOCH + second * S
                && d.out.flags[i] & O_REACQUIRED != 0
        }));
    }
    assert_eq!(tracker_stats(&d.tracker).started, started);
    assert_eq!(
        tracker_stats(&d.tracker).applied,
        tracker_stats(&d.tracker).offered
    );

    let restored = EPOCH + 43 * S;
    state_report(
        &mut d.states,
        NetId::Msaw,
        NetReport::Running,
        restored,
        restored,
    )
    .unwrap();
    d.frame(43, &[]);
}

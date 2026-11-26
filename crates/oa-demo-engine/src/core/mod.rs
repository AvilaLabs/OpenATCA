//! Class: core (RULESET B1, B7). Pure owner of demonstration tracker, flight, workstation and
//! raster state. Time and normalized input events are supplied as values by the shell.

mod ord_map;

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use oa_demo_scenario::core::{SyntheticReport, MAX_DURATION_MS, NORMAL_ORD_AIRCRAFT};
use oa_display::core::{
    interaction::{self, FlightIdentity, InteractionResult, InteractionState, TargetView},
    list::{
        list_clear, list_new, push_circle, push_line, push_text, DisplayError, DisplayList,
        ListCaps, ALIGN_LEFT,
    },
    project::{project, projection_new, Projection},
    raster::{frame_len, render, FrameSpec, RenderError},
    traffic::{
        caps_add, targets_clear, targets_new, targets_push, traffic_caps, Target, TargetSet,
        TrafficError, DEFAULT_STYLE, MAX_TRAIL, NO_ALT, NO_REACQ, T_COAST, T_UNCERTAIN,
    },
    workstation::{build_r_side, r_side_overlay_caps, RSideFrame},
};
use oa_flight::{
    flights_apply, flights_new, key_gufi, op_new, FlightConfig, FlightKey, FlightTable, Outcome,
    OP_SET_PLAN, ST_ACTIVE,
};
use oa_geo::Ellipsoid;
use oa_safety::{
    inhibit::{inhibits_new, InhibitTable},
    net::{state_table_new, StateTable},
};
use oa_track_core::{
    output_clear, output_new, r95_m_of, tracker_advance, tracker_live_iter, tracker_new,
    tracker_predict, tracker_update, Measurement, TrackOutput, Tracker, TrackerConfig,
    DEFAULT_CONFIG, M_HAS_VEL, O_COAST, O_HAS_ALT, O_UNCERTAIN,
};
use oa_workstation::{
    execute_prototype_entry, position_new, Context, Coordination, EntryReceipt, Position, R_SIDE,
};

/// Browser/native presentation frame dimensions. The raster format is B,G,R,unused-byte.
pub const DEMO_FRAME_WIDTH: u32 = 1280;
pub const DEMO_FRAME_HEIGHT: u32 = 720;
pub const DEMO_FRAME_STRIDE: u32 = DEMO_FRAME_WIDTH * 4;
pub const FRAME_PACKET_HEADER_LEN: usize = 24;
pub const FRAME_PACKET_FORMAT_BGRX8_LE: u16 = 1;
pub const MAX_SCENARIO_REPORTS: usize = 360_000;
pub const MAX_AIRCRAFT: usize = NORMAL_ORD_AIRCRAFT as usize;
pub const DEFAULT_SCENARIO_DURATION_MS: u64 = MAX_DURATION_MS;
pub const NORMAL_SAMPLE_INTERVAL_MS: u32 = 5_000;
pub const SYNTHETIC_POSITION_SIGMA_M: f32 = 25.0;
pub const SYNTHETIC_SENSOR_ID: u16 = 0xF201;
/// Positive, unique 24-bit synthetic address range; bit 24 remains clear (ICAO namespace).
pub const SYNTHETIC_ADDRESS_BASE: u32 = 0x0001_0000;
pub const POINTER_MOVE: u32 = 0;
pub const POINTER_DOWN: u32 = 1;
pub const POINTER_UP: u32 = 2;
pub const POINTER_CANCEL: u32 = 3;

/// Convert browser/F2TRACE1 modifier bits (Ctrl, Alt, Shift, Meta) into this engine's HID
/// convention (Shift, Ctrl, Alt, Meta). Callers validate that no bits above the low nibble are set.
pub const fn normalize_host_key_modifiers(modifiers: u32) -> u32 {
    ((modifiers & 0x04) >> 2)
        | ((modifiers & 0x01) << 1)
        | ((modifiers & 0x02) << 1)
        | (modifiers & 0x08)
}

const DEMO_RANGE_NM: u16 = 60;
const RANGE_STEPS_NM: [u16; 6] = [20, 40, 60, 90, 120, 250];
const DEMO_CENTRE: [u8; 4] = *b"ORD ";
const HISTORY_LIMIT: usize = MAX_TRAIL;
const TARGET_PICK_RADIUS_PX: i32 = 12;
const TRACK_OUTPUT_CAPACITY: usize = MAX_AIRCRAFT;
const FLIGHT_LOG_CAPACITY: usize = 128;
const SAFETY_STALE_LIMIT_NS: i64 = 10_000_000_000;
/// The scenario materializer allows routes to begin at most six hours before T0.
const MAX_PRE_SCENARIO_MS: i64 = 6 * 60 * 60 * 1_000;
/// Maximum work is derived from the scenario's six-hour pre-roll and one-hour duration caps at 5 s cadence.
const MAX_CATCHUP_TICKS: u64 =
    ((MAX_PRE_SCENARIO_MS as u64 + MAX_DURATION_MS) / NORMAL_SAMPLE_INTERVAL_MS as u64) + 2;

/// Caller-selected deterministic settings. `seed` and `duration_ms` are taken from the validated
/// prepared scenario bundle, not from browser randomness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DemoEngineConfig {
    pub seed: u64,
    pub duration_ms: u64,
    pub epoch_unix_ns: i64,
    pub sample_interval_ms: u32,
    pub centre_lat_e7: i32,
    pub centre_lon_e7: i32,
    pub position_sigma_m: f32,
    pub tracker: TrackerConfig,
}

impl DemoEngineConfig {
    pub fn normal_ord(
        seed: u64,
        duration_ms: u64,
        epoch_unix_ns: i64,
        sample_interval_ms: u32,
        centre_lat_e7: i32,
        centre_lon_e7: i32,
    ) -> Self {
        let mut tracker = DEFAULT_CONFIG;
        tracker.max_tracks = MAX_AIRCRAFT;
        tracker.out_capacity = TRACK_OUTPUT_CAPACITY;
        tracker.log_capacity = FLIGHT_LOG_CAPACITY;
        let interval_ns = i64::from(sample_interval_ms) * 1_000_000;
        tracker.expected_interval_ns = interval_ns;
        tracker.init_interval_ns = interval_ns;
        tracker.tick_ns = interval_ns;
        tracker.drop_after_ns = 60_000_000_000;
        Self {
            seed,
            duration_ms,
            epoch_unix_ns,
            sample_interval_ms,
            centre_lat_e7,
            centre_lon_e7,
            position_sigma_m: SYNTHETIC_POSITION_SIGMA_M,
            tracker,
        }
    }
}

/// Cumulative, named loss/refusal counts; fields saturate rather than wrap.
// A7.2: cold diagnostic snapshot, one per engine rather than one per tracked aircraft.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EngineCounters {
    pub labels_placed: u64,
    pub label_overlap_pairs: u64,
    pub label_overlap_pairs_outside_terminal: u64,
    pub label_overlap_pairs_inside_terminal: u64,
    pub arrivals_completed: u64,
    pub ca_cycles: u64,
    pub ca_open_alerts: u64,
    pub ca_work_short: u64,
    pub ca_input_short: u64,
    pub reports_supplied: u64,
    pub reports_processed: u64,
    pub prediction_unavailable: u64,
    pub targets_refused: u64,
    pub report_order_ties: u64,
    pub scenario_start_virtual_ms: u64,
    pub earliest_report_virtual_ms: i64,
    pub preroll_ms: u64,
    pub scenario_duration_ms: u64,
    pub sample_interval_ms: u32,
    pub max_catchup_ticks: u64,
    pub tracker_output_overflow: u64,
    pub input_keys_unmapped: u64,
    pub input_buffer_full: u64,
    pub input_commands: u64,
    pub input_command_refused: u64,
    pub pointer_misses: u64,
    pub frames_rendered: u64,
}

/// Error class for an engine operation. Refusals are explicit and do not expose partial frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineError {
    Config(&'static str),
    Scenario(&'static str),
    Time(&'static str),
    Display(DisplayError),
    Render(RenderError),
    Tracker(&'static str),
    Flight(&'static str),
    Workstation(&'static str),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(s) => write!(f, "demo engine configuration: {s}"),
            Self::Scenario(s) => write!(f, "demo scenario: {s}"),
            Self::Time(s) => write!(f, "demo virtual time: {s}"),
            Self::Display(e) => write!(f, "demo display: {e}"),
            Self::Render(e) => write!(f, "demo raster: {e}"),
            Self::Tracker(s) => write!(f, "demo tracker: {s}"),
            Self::Flight(s) => write!(f, "demo flight state: {s}"),
            Self::Workstation(s) => write!(f, "demo workstation: {s}"),
        }
    }
}

impl ::core::error::Error for EngineError {}

impl EngineError {
    /// Stable native/WASM status codes for F2TRACE1 comparison.
    pub const fn status_code(&self) -> i32 {
        match self {
            Self::Config(_) => -1,
            Self::Scenario(_) => -2,
            Self::Time(_) => -3,
            Self::Display(_) => -4,
            Self::Render(_) => -5,
            Self::Tracker(_) => -6,
            Self::Flight(_) => -7,
            Self::Workstation(_) => -8,
        }
    }
}

impl From<DisplayError> for EngineError {
    fn from(value: DisplayError) -> Self {
        Self::Display(value)
    }
}

impl From<RenderError> for EngineError {
    fn from(value: RenderError) -> Self {
        Self::Render(value)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AircraftHistory {
    address: u32,
    callsign: [u8; 8],
    len: u8,
    lat: [i32; HISTORY_LIMIT],
    lon: [i32; HISTORY_LIMIT],
    gs_dkt: u16,
    track_ddeg: u16,
    generation: u32,
    present: bool,
    flight_registered: bool,
}

/// A borrowed stable view of the most recently rendered bytes. It is valid until the next engine
/// call that mutates or renders the engine.
#[derive(Clone, Copy, Debug)]
pub struct FrameView<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: &'a [u8],
    /// SHA-256 of visible raster pixels only; use [`encode_frame_packet`] for the canonical packet hash.
    pub pixel_digest: [u8; 32],
    pub counters: EngineCounters,
    /// Actual CA availability; the terrain-warning adaptation is not configured.
    pub ca_available: bool,
    pub msaw_available: bool,
}

/// Stateful synthetic demonstration. The report vector is bounded, validated, and sorted once
/// at construction; Rust owns all tracker, flight, workstation, and render state thereafter.
pub struct DemoEngine {
    config: DemoEngineConfig,
    reports: Vec<SyntheticReport>,
    next_report: usize,
    tracker: Tracker,
    tracker_output: TrackOutput,
    flights: FlightTable,
    position: Position,
    coordination: Coordination,
    interaction: InteractionState,
    receipt: Option<EntryReceipt>,
    target_set: TargetSet,
    target_views: Vec<TargetView>,
    target_positions: Vec<(i32, i32)>,
    display_list: DisplayList,
    projection: Projection,
    view_centre_lat_e7: i32,
    view_centre_lon_e7: i32,
    ellipsoid: Ellipsoid,
    frame_spec: FrameSpec,
    pixels: Vec<u8>,
    digest: [u8; 32],
    histories: [AircraftHistory; MAX_AIRCRAFT],
    safety_states: StateTable,
    exited_aircraft_mask: u64,
    ca_state: oa_ca::CaState,
    ca_rows: Vec<oa_ca::CaTrack>,
    ca_closed: oa_ca::AlertLog,
    ca_events: oa_ca::EventLog,
    safety_inhibits: InhibitTable,
    input: [u8; 96],
    input_len: usize,
    current_virtual_ms: Option<u64>,
    current_ns: i64,
    last_tick_ns: i64,
    view_sequence: u32,
    input_sequence: u32,
    counters: EngineCounters,
    noted_output_overflow: u64,
}

impl DemoEngine {
    /// Constructs the pure engine from bounded synthetic rows and explicit time/config values.
    /// The artifact shell validates the source bundle before calling this constructor.
    pub fn from_reports(
        config: DemoEngineConfig,
        mut reports: Vec<SyntheticReport>,
    ) -> Result<Self, EngineError> {
        if config.duration_ms == 0 || config.duration_ms > MAX_DURATION_MS {
            return Err(EngineError::Config(
                "duration_ms must be within the prepared scenario limit",
            ));
        }
        if i64::rem_euclid(config.epoch_unix_ns, 1_000_000) != 0
            || config.sample_interval_ms != NORMAL_SAMPLE_INTERVAL_MS
        {
            return Err(EngineError::Config(
                "epoch must align to milliseconds and cadence must match the prepared demo",
            ));
        }
        if !config.position_sigma_m.is_finite() || config.position_sigma_m <= 0.0 {
            return Err(EngineError::Config(
                "position_sigma_m must be positive and finite",
            ));
        }
        if !(-900_000_000..=900_000_000).contains(&config.centre_lat_e7)
            || !(-1_800_000_000..=1_800_000_000).contains(&config.centre_lon_e7)
        {
            return Err(EngineError::Config(
                "prepared center is not a valid coordinate",
            ));
        }
        if reports.is_empty() {
            return Err(EngineError::Scenario("prepared report set is empty"));
        }
        if reports.len() > MAX_SCENARIO_REPORTS {
            return Err(EngineError::Scenario(
                "prepared report set exceeds 360000 rows",
            ));
        }
        let earliest_ms = -MAX_PRE_SCENARIO_MS;
        for report in &reports {
            if usize::from(report.aircraft_index) >= MAX_AIRCRAFT {
                return Err(EngineError::Scenario("aircraft index exceeds scenario cap"));
            }
            if report.virtual_time_ms < earliest_ms
                || report.virtual_time_ms > i64::try_from(config.duration_ms).unwrap_or(i64::MAX)
            {
                return Err(EngineError::Scenario(
                    "report time is outside the scenario window",
                ));
            }
            if report.flags & !3 != 0
                || !(-900_000_000..=900_000_000).contains(&report.lat_e7)
                || !(-1_800_000_000..=1_800_000_000).contains(&report.lon_e7)
                || report.altitude_ft < -2_000
                || report.altitude_ft > 100_000
                || report.ground_speed_dkt > 4_000
                || report.track_ddeg >= 3_600
            {
                return Err(EngineError::Scenario(
                    "report fields are outside the engine contract",
                ));
            }
        }
        let mut indexed: Vec<(i64, u16, usize, SyntheticReport)> = reports
            .drain(..)
            .enumerate()
            .map(|(ordinal, row)| (row.virtual_time_ms, row.aircraft_index, ordinal, row))
            .collect();
        indexed.sort_by_key(|entry| (entry.0, entry.1, entry.2));
        let tie_count = indexed
            .windows(2)
            .filter(|pair| {
                pair.first()
                    .zip(pair.get(1))
                    .is_some_and(|(first, second)| first.0 == second.0 && first.1 == second.1)
            })
            .count() as u64;
        let reports: Vec<SyntheticReport> = indexed.into_iter().map(|entry| entry.3).collect();
        let earliest_report_virtual_ms = reports
            .first()
            .map(|row| row.virtual_time_ms)
            .ok_or(EngineError::Scenario("prepared report set is empty"))?;
        let preroll_ms = if earliest_report_virtual_ms < 0 {
            u64::try_from(-earliest_report_virtual_ms).unwrap_or(u64::MAX)
        } else {
            0
        };
        let tick_ms = u64::from(config.sample_interval_ms);
        let horizon_ms = config.duration_ms.saturating_add(preroll_ms);
        let max_catchup_ticks = horizon_ms
            .checked_add(tick_ms.saturating_sub(1))
            .map(|n| n / tick_ms)
            .and_then(|n| n.checked_add(1))
            .ok_or(EngineError::Config("catch-up work bound overflow"))?;
        if max_catchup_ticks > MAX_CATCHUP_TICKS {
            return Err(EngineError::Scenario(
                "pre-roll plus duration exceeds bounded tracker work",
            ));
        }

        let mut tracker_config = config.tracker;
        tracker_config.max_tracks = MAX_AIRCRAFT;
        tracker_config.out_capacity = TRACK_OUTPUT_CAPACITY;
        tracker_config.log_capacity = FLIGHT_LOG_CAPACITY;
        let tracker = tracker_new(&tracker_config)
            .map_err(|_| EngineError::Tracker("configuration refused"))?;
        let tracker_output = output_new(TRACK_OUTPUT_CAPACITY);
        let flight_config = FlightConfig {
            capacity: MAX_AIRCRAFT,
            lag_floor_ms: 0,
            max_future_ms: 0,
            refusal_log: FLIGHT_LOG_CAPACITY,
            conflict_log: FLIGHT_LOG_CAPACITY,
            removal_log: FLIGHT_LOG_CAPACITY,
        };
        let flights = flights_new(&flight_config)
            .map_err(|_| EngineError::Flight("configuration refused"))?;
        let mut histories = [AircraftHistory::default(); MAX_AIRCRAFT];
        for report in &reports {
            let ix = usize::from(report.aircraft_index);
            if let Some(history) = histories.get_mut(ix) {
                if !history.present {
                    history.address = synthetic_address(report.aircraft_index);
                    history.callsign = synthetic_callsign(report.aircraft_index);
                    history.generation = 1;
                    history.present = true;
                }
            }
        }
        let position = position_new(*b"ORD     ", R_SIDE, DEMO_RANGE_NM)
            .map_err(|_| EngineError::Workstation("position configuration refused"))?;
        let projection = projection_new(
            config.centre_lat_e7,
            config.centre_lon_e7,
            f64::from(position.range_nm),
            DEMO_FRAME_WIDTH,
            DEMO_FRAME_HEIGHT,
            104,
        )?;
        let style = DEFAULT_STYLE;
        let t_caps = traffic_caps(MAX_AIRCRAFT, &style)?;
        let r_caps = caps_add(&t_caps, &r_side_overlay_caps())?;
        let safety = r_caps;
        let caps = caps_add(
            &safety,
            &ListCaps {
                texts: 5,
                text_bytes: 512,
                lines: MAX_AIRCRAFT,
                rects: 0,
                circles: 0,
                polylines: 0,
                poly_vertices: 0,
            },
        )?;
        let points: usize = ord_map::ROUTES.iter().map(|route| route.len()).sum();
        let map_caps = ListCaps {
            lines: points * 8 + ord_map::RUNWAYS.len(),
            circles: points + 1,
            texts: points + ord_map::RUNWAYS.len() + 1,
            text_bytes: (points + ord_map::RUNWAYS.len() + 1) * 16,
            rects: 0,
            polylines: 0,
            poly_vertices: 0,
        };
        let display_list = list_new(caps_add(&caps, &map_caps)?)?;
        let target_set = targets_new(MAX_AIRCRAFT)?;
        let target_views = Vec::with_capacity(MAX_AIRCRAFT);
        let target_positions = Vec::with_capacity(MAX_AIRCRAFT);
        let frame_spec = FrameSpec {
            width: DEMO_FRAME_WIDTH,
            height: DEMO_FRAME_HEIGHT,
            stride: DEMO_FRAME_STRIDE,
        };
        let pixels = vec![0; frame_len(&frame_spec)];
        let first_report_ms = reports
            .first()
            .map(|row| row.virtual_time_ms)
            .ok_or(EngineError::Scenario("prepared report set is empty"))?;
        let first_report_ns = first_report_ms
            .checked_mul(1_000_000)
            .and_then(|relative_ns| config.epoch_unix_ns.checked_add(relative_ns))
            .ok_or(EngineError::Config("first report timestamp overflow"))?;
        let initial_time_ns = first_report_ns.min(config.epoch_unix_ns);
        let safety_states = state_table_new(SAFETY_STALE_LIMIT_NS, 0)
            .map_err(|_| EngineError::Config("safety state table configuration refused"))?;
        Ok(Self {
            config,
            counters: EngineCounters {
                reports_supplied: reports.len() as u64,
                report_order_ties: tie_count,
                scenario_start_virtual_ms: 0,
                earliest_report_virtual_ms,
                preroll_ms,
                scenario_duration_ms: config.duration_ms,
                sample_interval_ms: config.sample_interval_ms,
                max_catchup_ticks,
                ..EngineCounters::default()
            },
            reports,
            next_report: 0,
            tracker,
            tracker_output,
            flights,
            position,
            coordination: oa_workstation::EMPTY_COORDINATION,
            interaction: interaction::interaction_new(),
            receipt: None,
            target_set,
            target_views,
            target_positions,
            display_list,
            projection,
            view_centre_lat_e7: config.centre_lat_e7,
            view_centre_lon_e7: config.centre_lon_e7,
            ellipsoid: oa_geo::wgs84(),
            frame_spec,
            pixels,
            digest: [0; 32],
            histories,
            safety_states,
            exited_aircraft_mask: 0,
            ca_state: oa_ca::ca_new(&demo_ca_config()).map_err(EngineError::Config)?,
            ca_rows: Vec::with_capacity(MAX_AIRCRAFT),
            ca_closed: oa_ca::alert_log_new(MAX_AIRCRAFT * (MAX_AIRCRAFT - 1) / 2)
                .map_err(EngineError::Config)?,
            ca_events: oa_ca::event_log_new(MAX_AIRCRAFT).map_err(EngineError::Config)?,
            safety_inhibits: inhibits_new(),
            input: [0; 96],
            input_len: 0,
            current_virtual_ms: None,
            current_ns: initial_time_ns,
            last_tick_ns: initial_time_ns,
            view_sequence: 0,
            input_sequence: 0,
            noted_output_overflow: 0,
        })
    }

    /// Advances monotonically in virtual milliseconds from scenario T0. All due reports are
    /// applied in deterministic timestamp/aircraft/input order before a final tracker advance.
    pub fn advance_to(&mut self, virtual_ms: u64) -> Result<(), EngineError> {
        if virtual_ms > self.config.duration_ms {
            return Err(EngineError::Time("target exceeds scenario duration"));
        }
        if self
            .current_virtual_ms
            .is_some_and(|current| virtual_ms < current)
        {
            return Err(EngineError::Time("time cannot move backwards"));
        }
        let target_ns = self.virtual_to_unix_ns(virtual_ms as i64)?;
        let mut processed = 0u64;
        while let Some(report) = self.reports.get(self.next_report).copied() {
            if report.virtual_time_ms > virtual_ms as i64 {
                break;
            }
            let report_ns = self.virtual_to_unix_ns(report.virtual_time_ms)?;
            if report_ns < self.current_ns {
                return Err(EngineError::Time(
                    "prepared report is older than current engine state",
                ));
            }
            self.advance_tracker_until(report_ns, false)?;
            let measurement = measurement_of(report, report_ns, self.config.position_sigma_m);
            tracker_update(&mut self.tracker, &measurement, &mut self.tracker_output);
            self.note_output_overflow();
            output_clear(&mut self.tracker_output);
            self.record_report(report)?;
            self.next_report += 1;
            processed = processed.saturating_add(1);
        }
        self.advance_tracker_until(target_ns, true)?;
        self.counters.reports_processed = self.counters.reports_processed.saturating_add(processed);
        self.current_virtual_ms = Some(virtual_ms);
        Ok(())
    }

    /// Applies one USB HID keyboard event. Text mapping and command execution remain Rust-owned.
    pub fn handle_key(
        &mut self,
        hid_usage: u32,
        modifiers: u32,
        down: bool,
        repeat: bool,
    ) -> Result<(), EngineError> {
        if !down || repeat {
            return Ok(());
        }
        if modifiers == 0 && self.apply_view_key(hid_usage)? {
            return Ok(());
        }
        if hid_usage == 0x28 {
            return self.submit_entry();
        }
        if hid_usage == 0x2A {
            if self.input_len > 0 {
                self.input_len -= 1;
                if let Some(byte) = self.input.get_mut(self.input_len) {
                    *byte = 0;
                }
            }
            return Ok(());
        }
        let Some(ch) = hid_ascii(hid_usage, modifiers) else {
            self.counters.input_keys_unmapped = self.counters.input_keys_unmapped.saturating_add(1);
            return Ok(());
        };
        if self.input_len >= self.input.len() {
            self.counters.input_buffer_full = self.counters.input_buffer_full.saturating_add(1);
            return Ok(());
        }
        if let Some(slot) = self.input.get_mut(self.input_len) {
            *slot = ch;
            self.input_len += 1;
        }
        Ok(())
    }

    /// Preview navigation is explicit engine state, independent of host drawing or time.
    fn apply_view_key(&mut self, hid_usage: u32) -> Result<bool, EngineError> {
        match hid_usage {
            0x2d => {
                self.position.range_nm = RANGE_STEPS_NM
                    .iter()
                    .copied()
                    .find(|&range| range > self.position.range_nm)
                    .unwrap_or(250)
            }
            0x2e => {
                self.position.range_nm = RANGE_STEPS_NM
                    .iter()
                    .rev()
                    .copied()
                    .find(|&range| range < self.position.range_nm)
                    .unwrap_or(20)
            }
            0x4a => {
                self.view_centre_lat_e7 = self.config.centre_lat_e7;
                self.view_centre_lon_e7 = self.config.centre_lon_e7;
            }
            0x4f..=0x52 => {
                let bearing = match hid_usage {
                    0x4f => 90.0,
                    0x50 => -90.0,
                    0x51 => 180.0,
                    _ => 0.0,
                };
                let point = oa_geo::LatLon::new(
                    f64::from(self.view_centre_lat_e7) * 1e-7,
                    f64::from(self.view_centre_lon_e7) * 1e-7,
                )
                .map_err(|_| EngineError::Workstation("view center invalid"))?;
                let moved = self
                    .ellipsoid
                    .direct(point, bearing, f64::from(self.position.range_nm) * 185.2)
                    .map_err(|_| EngineError::Workstation("view pan failed"))?;
                // View-only E7 quantization truncates by less than0.012m; no platform math call.
                self.view_centre_lat_e7 = (moved.position.lat() * 1e7) as i32;
                self.view_centre_lon_e7 = (moved.position.lon() * 1e7) as i32;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Applies a normalized pointer event. `kind` uses [`POINTER_MOVE`], [`POINTER_DOWN`],
    /// [`POINTER_UP`] or [`POINTER_CANCEL`]; bit zero of `buttons` is the primary button.
    pub fn handle_pointer(
        &mut self,
        x: i32,
        y: i32,
        buttons: u32,
        kind: u32,
    ) -> Result<(), EngineError> {
        if kind > POINTER_CANCEL {
            return Err(EngineError::Workstation("unknown pointer event kind"));
        }
        if kind == POINTER_CANCEL || (kind == POINTER_DOWN && buttons & 1 == 0) {
            interaction::clear_selection(&mut self.interaction);
            self.coordination = oa_workstation::EMPTY_COORDINATION;
            return Ok(());
        }
        if kind != POINTER_DOWN || buttons & 1 == 0 {
            return Ok(());
        }
        let mut best: Option<(i64, usize)> = None;
        for (ix, (_view, position)) in self
            .target_views
            .iter()
            .zip(&self.target_positions)
            .enumerate()
        {
            let Some((x16, y16)) = project(&self.projection, position.0, position.1) else {
                continue;
            };
            let dx = i64::from(x) - i64::from(x16 / 16);
            let dy = i64::from(y) - i64::from(y16 / 16);
            let d2 = dx * dx + dy * dy;
            if d2 <= i64::from(TARGET_PICK_RADIUS_PX * TARGET_PICK_RADIUS_PX)
                && best.is_none_or(|(old, _)| d2 < old)
            {
                best = Some((d2, ix));
            }
        }
        let Some((_, best_ix)) = best else {
            self.counters.pointer_misses = self.counters.pointer_misses.saturating_add(1);
            interaction::clear_selection(&mut self.interaction);
            self.coordination = oa_workstation::EMPTY_COORDINATION;
            return Ok(());
        };
        let view = self
            .target_views
            .get(best_ix)
            .copied()
            .ok_or(EngineError::Workstation("selected view disappeared"))?;
        let identity = if view.identity_present == 1 {
            Some(view.identity)
        } else {
            None
        };
        let result = interaction::select_target(
            &mut self.interaction,
            &self.target_views,
            self.view_sequence,
            view.address,
            view.generation,
            identity,
        );
        if result == InteractionResult::Applied {
            let history_index = view
                .address
                .checked_sub(SYNTHETIC_ADDRESS_BASE)
                .and_then(|index| usize::try_from(index).ok())
                .filter(|index| *index < MAX_AIRCRAFT);
            if let Some(h) = history_index.and_then(|index| self.histories.get(index)) {
                self.coordination.id = h.callsign;
            }
        }
        Ok(())
    }

    /// Builds the workstation frame with the actual CA state and explicit unconfigured nets.
    pub fn render_frame(&mut self) -> Result<FrameView<'_>, EngineError> {
        self.rebuild_targets()?;
        list_clear(&mut self.display_list);
        self.projection = projection_new(
            self.view_centre_lat_e7,
            self.view_centre_lon_e7,
            f64::from(self.position.range_nm),
            DEMO_FRAME_WIDTH,
            DEMO_FRAME_HEIGHT,
            104,
        )?;
        draw_public_map(&mut self.display_list, &self.projection)?;
        let interaction_frame = &self.interaction;
        build_r_side(
            &mut self.display_list,
            &RSideFrame {
                projection: &self.projection,
                ellipsoid: &self.ellipsoid,
                position: &self.position,
                targets: &self.target_set,
                traffic_style: &DEFAULT_STYLE,
                interaction: interaction_frame,
                entry_receipt: self.receipt.as_ref(),
            },
        )?;
        let (labels, overlaps, outside_overlaps, inside_overlaps) = declutter_labels(
            &mut self.display_list,
            &self.projection,
            &self.target_positions,
            oa_display::core::traffic::targets_columns(&self.target_set).callsign,
        )?;
        self.counters.labels_placed = labels as u64;
        self.counters.label_overlap_pairs = overlaps as u64;
        self.counters.label_overlap_pairs_outside_terminal = outside_overlaps as u64;
        self.counters.label_overlap_pairs_inside_terminal = inside_overlaps as u64;
        draw_demo_safety_status(
            &mut self.display_list,
            &self.safety_states,
            &self.safety_inhibits,
            self.current_ns,
            self.counters.ca_open_alerts,
        )?;
        let mut range_text = oa_display::core::text::text_new();
        oa_display::core::text::text_push_str(&mut range_text, "RANGE ");
        oa_display::core::text::text_push_dec(
            &mut range_text,
            u64::from(self.position.range_nm),
            1,
        );
        oa_display::core::text::text_push_str(&mut range_text, " NM");
        push_text(
            &mut self.display_list,
            (900 * 16, 28 * 16),
            12,
            ALIGN_LEFT,
            oa_display::core::palette::DIM_TEXT,
            15,
            oa_display::core::text::text_str(&range_text),
        )?;
        self.digest = render(&self.display_list, &self.frame_spec, &mut self.pixels)?;
        self.counters.frames_rendered = self.counters.frames_rendered.saturating_add(1);
        self.view_sequence = self
            .view_sequence
            .checked_add(1)
            .ok_or(EngineError::Workstation("view sequence exhausted"))?;
        Ok(FrameView {
            width: self.frame_spec.width,
            height: self.frame_spec.height,
            stride: self.frame_spec.stride,
            pixels: &self.pixels,
            pixel_digest: self.digest,
            counters: self.counters,
            ca_available: matches!(
                oa_safety::net::net_status(
                    &self.safety_states,
                    &self.safety_inhibits,
                    oa_safety::net::NetId::ConflictAlert,
                    self.current_ns
                )
                .state,
                oa_safety::net::NetState::Running | oa_safety::net::NetState::Degraded { .. }
            ),
            msaw_available: false,
        })
    }

    /// Synthetic-preview diagnostic view of the actual bounded CA pair table.
    pub fn active_ca_alerts(&self) -> &[oa_ca::Alert] {
        oa_ca::ca_open(&self.ca_state)
    }

    /// Current virtual time, if the host has advanced the engine.
    pub fn current_virtual_ms(&self) -> Option<u64> {
        self.current_virtual_ms
    }

    /// Snapshot of bounded cumulative counters.
    pub fn counters(&self) -> EngineCounters {
        self.counters
    }

    /// Number of target centers drawn in the last rendered map area, using the display's
    /// own clipping rule. Overlapping labels still count as separate aircraft.
    pub fn frame_target_count(&self) -> usize {
        self.target_positions
            .iter()
            .filter(|&&(lat, lon)| oa_display::core::traffic::on_map(&self.projection, lat, lon))
            .count()
    }

    fn submit_entry(&mut self) -> Result<(), EngineError> {
        self.input_sequence = self
            .input_sequence
            .checked_add(1)
            .ok_or(EngineError::Workstation("input sequence exhausted"))?;
        self.counters.input_commands = self.counters.input_commands.saturating_add(1);
        let input = ::core::str::from_utf8(self.input.get(..self.input_len).unwrap_or_default())
            .map_err(|_| EngineError::Workstation("entry buffer is not UTF-8"))?;
        let selected = interaction::selected_address(&self.interaction)
            .and_then(|addr| addr.checked_sub(SYNTHETIC_ADDRESS_BASE))
            .and_then(|index| usize::try_from(index).ok())
            .filter(|ix| *ix < MAX_AIRCRAFT);
        let key = selected.map_or_else(
            || flight_key(self.config.seed, 0),
            |ix| flight_key(self.config.seed, ix as u16),
        );
        let base_ms = i64::div_euclid(self.config.epoch_unix_ns, 1_000_000)
            .checked_add(i64::try_from(self.current_virtual_ms.unwrap_or(0)).unwrap_or(i64::MAX));
        let at_ms = base_ms.ok_or(EngineError::Time("entry timestamp overflow"))?;
        let context = Context {
            key,
            centre: DEMO_CENTRE,
            at_ms,
            message: u64::from(self.input_sequence),
        };
        self.receipt = Some(execute_prototype_entry(
            input,
            &mut self.position,
            &mut self.flights,
            &mut self.coordination,
            &context,
        ));
        if self
            .receipt
            .is_some_and(|receipt| receipt.outcome == oa_workstation::EntryOutcome::Refused)
        {
            self.counters.input_command_refused =
                self.counters.input_command_refused.saturating_add(1);
        }
        self.input.fill(0);
        self.input_len = 0;
        Ok(())
    }

    fn virtual_to_unix_ns(&self, virtual_ms: i64) -> Result<i64, EngineError> {
        virtual_ms
            .checked_mul(1_000_000)
            .and_then(|relative_ns| self.config.epoch_unix_ns.checked_add(relative_ns))
            .ok_or(EngineError::Time("Unix timestamp overflow"))
    }

    fn advance_tracker_until(
        &mut self,
        target_ns: i64,
        include_endpoint: bool,
    ) -> Result<(), EngineError> {
        if target_ns < self.current_ns {
            return Err(EngineError::Time(
                "tracker target is earlier than current time",
            ));
        }
        let tick_ns = self.config.tracker.tick_ns;
        if tick_ns <= 0 {
            return Err(EngineError::Config("tracker tick must be positive"));
        }
        let delta = target_ns - self.last_tick_ns;
        // Ingest every report at a scan timestamp before that scan's CA evaluation.
        let steps = if include_endpoint {
            delta / tick_ns
        } else {
            delta.saturating_sub(1).max(0) / tick_ns
        };
        if u64::try_from(steps).unwrap_or(u64::MAX) > self.counters.max_catchup_ticks {
            return Err(EngineError::Time(
                "catch-up exceeds the one-hour tick bound",
            ));
        }
        let mut next = self.last_tick_ns;
        for _ in 0..steps {
            next = next
                .checked_add(tick_ns)
                .ok_or(EngineError::Time("tracker clock overflow"))?;
            tracker_advance(&mut self.tracker, next, &mut self.tracker_output);
            self.run_ca_scan(next);
            self.note_output_overflow();
            output_clear(&mut self.tracker_output);
        }
        self.last_tick_ns = next;
        self.current_ns = target_ns;
        Ok(())
    }

    fn run_ca_scan(&mut self, at_ns: i64) {
        self.ca_rows.clear();
        for (key, _) in tracker_live_iter(&self.tracker) {
            if key
                .checked_sub(SYNTHETIC_ADDRESS_BASE)
                .filter(|&n| n < MAX_AIRCRAFT as u32)
                .is_some_and(|n| self.exited_aircraft_mask & (1_u64 << n) != 0)
            {
                continue;
            }
            if let Some((row, _, _)) = tracker_predict(&self.tracker, key, at_ns) {
                self.ca_rows.push(oa_ca::CaTrack {
                    lat_deg: f64::from(row.lat_e7) * 1e-7,
                    lon_deg: f64::from(row.lon_e7) * 1e-7,
                    addr: key,
                    vel_east_mps: row.vel_east_mps,
                    vel_north_mps: row.vel_north_mps,
                    r95_m: r95_m_of(&row.pos_cov_m2) as f32,
                    alt_ft: if row.flags & O_HAS_ALT != 0 {
                        row.alt_ft
                    } else {
                        f32::NAN
                    },
                    vrate_fpm: if row.flags & O_HAS_ALT != 0 {
                        row.vrate_fpm
                    } else {
                        f32::NAN
                    },
                    flags: (if row.flags & O_COAST != 0 {
                        oa_ca::F_COAST
                    } else {
                        0
                    }) | (if row.flags & O_UNCERTAIN != 0 {
                        oa_ca::F_UNCERTAIN
                    } else {
                        0
                    }),
                    ground: oa_ca::AIRBORNE,
                });
            }
        }
        oa_ca::alert_log_clear(&mut self.ca_closed);
        oa_ca::event_log_clear(&mut self.ca_events);
        let out = oa_ca::ca_cycle(
            &mut self.ca_state,
            at_ns,
            &self.ca_rows,
            &mut self.ca_closed,
            &mut self.ca_events,
            &mut self.safety_states,
        );
        self.counters.ca_cycles = self.counters.ca_cycles.saturating_add(1);
        self.counters.ca_work_short = self.counters.ca_work_short.saturating_add(out.work_short);
        self.counters.ca_input_short = self.counters.ca_input_short.saturating_add(out.input_short);
        self.counters.ca_open_alerts = oa_ca::ca_open_len(&self.ca_state) as u64;
    }

    fn note_output_overflow(&mut self) {
        self.counters.tracker_output_overflow =
            self.counters.tracker_output_overflow.saturating_add(
                self.tracker_output
                    .overflow
                    .saturating_sub(self.noted_output_overflow),
            );
        self.noted_output_overflow = self.tracker_output.overflow;
    }

    fn record_report(&mut self, report: SyntheticReport) -> Result<(), EngineError> {
        if report.flags & 2 != 0 && report.aircraft_index < MAX_AIRCRAFT as u16 {
            let bit = 1_u64 << report.aircraft_index;
            if self.exited_aircraft_mask & bit == 0 {
                self.counters.arrivals_completed =
                    self.counters.arrivals_completed.saturating_add(1);
                self.exited_aircraft_mask |= bit;
            }
        }
        if let Some(history) = self.histories.get_mut(usize::from(report.aircraft_index)) {
            if !history.flight_registered {
                let report_ms = i64::div_euclid(self.config.epoch_unix_ns, 1_000_000)
                    .checked_add(report.virtual_time_ms)
                    .ok_or(EngineError::Time("synthetic flight timestamp overflow"))?;
                let mut op = op_new(
                    OP_SET_PLAN,
                    flight_key(self.config.seed, report.aircraft_index),
                    DEMO_CENTRE,
                    report_ms,
                    report_ms,
                    u64::from(report.aircraft_index) + 1,
                );
                op.state = ST_ACTIVE;
                op.callsign = history.callsign;
                op.plan_id = synthetic_plan_id(report.aircraft_index);
                if matches!(flights_apply(&mut self.flights, &op), Outcome::Refused(_)) {
                    return Err(EngineError::Flight("synthetic plan initialization refused"));
                }
                history.flight_registered = true;
            }
            history.gs_dkt = report.ground_speed_dkt;
            history.track_ddeg = report.track_ddeg;
            if usize::from(history.len) == HISTORY_LIMIT {
                history.lat.copy_within(1..HISTORY_LIMIT, 0);
                history.lon.copy_within(1..HISTORY_LIMIT, 0);
                history.len = (HISTORY_LIMIT - 1) as u8;
            }
            let n = usize::from(history.len);
            if let (Some(lat), Some(lon)) = (history.lat.get_mut(n), history.lon.get_mut(n)) {
                *lat = report.lat_e7;
                *lon = report.lon_e7;
                history.len = history.len.saturating_add(1);
            }
        }
        Ok(())
    }

    fn rebuild_targets(&mut self) -> Result<(), EngineError> {
        targets_clear(&mut self.target_set);
        self.target_views.clear();
        self.target_positions.clear();
        let at_ns = self.current_ns;
        for (key, _) in tracker_live_iter(&self.tracker) {
            if key
                .checked_sub(SYNTHETIC_ADDRESS_BASE)
                .filter(|&n| n < MAX_AIRCRAFT as u32)
                .is_some_and(|n| self.exited_aircraft_mask & (1_u64 << n) != 0)
            {
                continue;
            }
            let Some((row, _, _last_ns)) = tracker_predict(&self.tracker, key, at_ns) else {
                self.counters.prediction_unavailable =
                    self.counters.prediction_unavailable.saturating_add(1);
                continue;
            };
            let index = key
                .checked_sub(SYNTHETIC_ADDRESS_BASE)
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| *value < MAX_AIRCRAFT)
                .unwrap_or(MAX_AIRCRAFT);
            let Some(history) = self.histories.get(index).copied().filter(|h| h.present) else {
                self.counters.prediction_unavailable =
                    self.counters.prediction_unavailable.saturating_add(1);
                continue;
            };
            let mut trail = [(0, 0); MAX_TRAIL];
            let n = usize::from(history.len).saturating_sub(1).min(MAX_TRAIL);
            for i in 0..n {
                let source = usize::from(history.len).saturating_sub(2 + i);
                if let (Some(la), Some(lo), Some(dst)) = (
                    history.lat.get(source),
                    history.lon.get(source),
                    trail.get_mut(i),
                ) {
                    *dst = (*la, *lo);
                }
            }
            let alt_ft = if row.flags & O_HAS_ALT != 0 && row.alt_ft.is_finite() {
                row.alt_ft as i32
            } else {
                NO_ALT
            };
            let flags = (if row.flags & O_COAST != 0 { T_COAST } else { 0 })
                | (if row.flags & O_UNCERTAIN != 0 {
                    T_UNCERTAIN
                } else {
                    0
                });
            let target = Target {
                lat_e7: row.lat_e7,
                lon_e7: row.lon_e7,
                alt_ft,
                addr: history.address,
                r95_m: r95_m_of(&row.pos_cov_m2) as f32,
                trail,
                callsign: history.callsign,
                track_ddeg: history.track_ddeg,
                gs_dkt: history.gs_dkt,
                reacq_age_s: NO_REACQ,
                flags,
                trail_len: n as u8,
            };
            match targets_push(&mut self.target_set, &target) {
                Ok(()) => {}
                Err(TrafficError::Malformed { .. } | TrafficError::Full { .. }) => {
                    self.counters.targets_refused = self.counters.targets_refused.saturating_add(1);
                    continue;
                }
            }
            let gufi = flight_key(self.config.seed, index as u16).gufi;
            let identity = FlightIdentity {
                gufi: u128::to_be_bytes(gufi),
                aircraft_address: history.address,
                centre: DEMO_CENTRE,
                plan_id: synthetic_plan_id(index as u16),
            };
            self.target_views.push(TargetView {
                address: history.address,
                generation: history.generation,
                identity_claims: 1,
                identity_present: 1,
                reserved: 0,
                identity,
            });
            self.target_positions.push((row.lat_e7, row.lon_e7));
        }
        Ok(())
    }
}

/// Writes the canonical 24-byte F2 OAF2 header and BGRX8 pixel payload into a caller-owned buffer,
/// then returns SHA-256 over the complete packet. Native and WASM hosts use this same encoder.
pub fn encode_frame_packet(frame: &FrameView<'_>, out: &mut [u8]) -> Result<[u8; 32], EngineError> {
    if frame.width != DEMO_FRAME_WIDTH
        || frame.height != DEMO_FRAME_HEIGHT
        || frame.stride != DEMO_FRAME_STRIDE
        || frame.pixels.len() != frame.stride as usize * frame.height as usize
    {
        return Err(EngineError::Render(RenderError::BadFrame(
            "F2 frame geometry mismatch",
        )));
    }
    let total = FRAME_PACKET_HEADER_LEN
        .checked_add(frame.pixels.len())
        .ok_or(EngineError::Render(RenderError::BadFrame(
            "F2 packet length overflow",
        )))?;
    if out.len() != total {
        return Err(EngineError::Render(RenderError::BadFrame(
            "F2 packet output has wrong length",
        )));
    }
    let header = out
        .get_mut(..FRAME_PACKET_HEADER_LEN)
        .ok_or(EngineError::Render(RenderError::BadFrame(
            "short F2 packet header",
        )))?;
    header
        .get_mut(..4)
        .ok_or(EngineError::Render(RenderError::BadFrame("short F2 magic")))?
        .copy_from_slice(b"OAF2");
    header
        .get_mut(4..6)
        .ok_or(EngineError::Render(RenderError::BadFrame(
            "short F2 version",
        )))?
        .copy_from_slice(&u16::to_le_bytes(1));
    header
        .get_mut(6..8)
        .ok_or(EngineError::Render(RenderError::BadFrame(
            "short F2 format",
        )))?
        .copy_from_slice(&u16::to_le_bytes(FRAME_PACKET_FORMAT_BGRX8_LE));
    header
        .get_mut(8..12)
        .ok_or(EngineError::Render(RenderError::BadFrame("short F2 width")))?
        .copy_from_slice(&u32::to_le_bytes(frame.width));
    header
        .get_mut(12..16)
        .ok_or(EngineError::Render(RenderError::BadFrame(
            "short F2 height",
        )))?
        .copy_from_slice(&u32::to_le_bytes(frame.height));
    header
        .get_mut(16..20)
        .ok_or(EngineError::Render(RenderError::BadFrame(
            "short F2 stride",
        )))?
        .copy_from_slice(&u32::to_le_bytes(frame.stride));
    let payload_len = u32::try_from(frame.pixels.len())
        .map_err(|_| EngineError::Render(RenderError::BadFrame("F2 pixels exceed u32 length")))?;
    header
        .get_mut(20..24)
        .ok_or(EngineError::Render(RenderError::BadFrame(
            "short F2 payload length",
        )))?
        .copy_from_slice(&u32::to_le_bytes(payload_len));
    out.get_mut(FRAME_PACKET_HEADER_LEN..)
        .ok_or(EngineError::Render(RenderError::BadFrame(
            "short F2 pixel payload",
        )))?
        .copy_from_slice(frame.pixels);
    Ok(oa_sha256::core::sha256::sha256(out))
}

fn synthetic_address(index: u16) -> u32 {
    SYNTHETIC_ADDRESS_BASE.saturating_add(u32::from(index))
}

fn synthetic_callsign(index: u16) -> [u8; 8] {
    let mut out = [0; 8];
    let number = index.saturating_add(1);
    let digits = [
        b'0' + ((number / 1000) % 10) as u8,
        b'0' + ((number / 100) % 10) as u8,
        b'0' + ((number / 10) % 10) as u8,
        b'0' + (number % 10) as u8,
    ];
    out[..3].copy_from_slice(b"SYN");
    out[3..7].copy_from_slice(&digits);
    out
}

fn synthetic_plan_id(index: u16) -> [u8; 12] {
    let mut out = *b"SCN000000000";
    let digits = synthetic_callsign(index);
    out[3..7].copy_from_slice(&digits[3..7]);
    out
}

/// Explicit synthetic preview adaptation; these values are not ERAM/STARS facility settings.
fn demo_ca_config() -> oa_ca::CaConfig {
    let volume = oa_ca::Volume {
        area: oa_ca::Area::Box {
            lat_min_e7: -900_000_000,
            lat_max_e7: 900_000_000,
            lon_min_e7: -1_800_000_000,
            lon_max_e7: 1_800_000_000,
        },
        alt_lo_ft: -2_000,
        alt_hi_ft: 100_000,
        h_m: 3.0 * 1852.0,
        v_ft: 1000.0,
    };
    let pairs = MAX_AIRCRAFT * (MAX_AIRCRAFT - 1) / 2;
    oa_ca::CaConfig {
        max_tracks: MAX_AIRCRAFT,
        max_open: pairs,
        max_candidates: pairs,
        look_ahead_ns: 60_000_000_000,
        hysteresis_ns: 5_000_000_000,
        work_budget: 1_000_000,
        eval_budget: pairs,
        max_speed_mps: 700.0,
        r95_cap_m: 20_000.0,
        grid_band_m: 20_000.0,
        default_volume: volume,
        volumes: [volume; oa_ca::MAX_VOLUMES],
        n_volumes: 0,
    }
}

fn draw_demo_safety_status(
    list: &mut DisplayList,
    states: &StateTable,
    inhibits: &InhibitTable,
    now_ns: i64,
    alerts: u64,
) -> Result<(), DisplayError> {
    use oa_display::core::{
        palette,
        text::{text_new, text_push_dec, text_push_str, text_str},
    };
    use oa_safety::net::{net_status, NetId, NetState};
    let status = net_status(states, inhibits, NetId::ConflictAlert, now_ns);
    let mut line = text_new();
    let (label, colour) = match status.state {
        NetState::Running => (
            "CA ACTIVE, ",
            if alerts == 0 {
                palette::TEXT
            } else {
                palette::ALERT
            },
        ),
        NetState::Degraded { .. } => ("CA DEGRADED, ", palette::ALERT),
        NetState::Inhibited => ("CA INHIBITED, ", palette::ALERT),
        NetState::Unavailable { .. } => ("CA UNAVAILABLE, ", palette::ALERT),
    };
    text_push_str(&mut line, label);
    text_push_dec(&mut line, alerts, 1);
    text_push_str(&mut line, " ALERTS");
    push_text(
        list,
        (8 * 16, 50 * 16),
        12,
        ALIGN_LEFT,
        colour,
        15,
        text_str(&line),
    )?;
    for (n, text) in [
        "MSAW NOT CONFIGURED IN THIS PREVIEW",
        "APPROACH-PATH MONITORING NOT IN THIS PREVIEW",
        "AREA PROXIMITY NOT IN THIS PREVIEW",
    ]
    .into_iter()
    .enumerate()
    {
        push_text(
            list,
            (8 * 16, (68 + n as i32 * 14) * 16),
            10,
            ALIGN_LEFT,
            palette::DIM_TEXT,
            15,
            text,
        )?;
    }
    Ok(())
}

// Fixed scratch geometry uses sixteenths of canvas pixels; the font owns pixel bounds.
type LabelBounds = oa_display::core::font::TextBounds;
const EMPTY_LABEL: LabelBounds = LabelBounds {
    x0: 0,
    y0: 0,
    x1: 0,
    y1: 0,
};
const MAX_MAP_LABELS: usize = 256;

fn overlap_area(a: LabelBounds, b: LabelBounds) -> i64 {
    i64::from((a.x1.min(b.x1) - a.x0.max(b.x0)).max(0))
        * i64::from((a.y1.min(b.y1) - a.y0.max(b.y0)).max(0))
}

fn text_at(list: &DisplayList, n: usize) -> &str {
    list.texts
        .start
        .get(n)
        .zip(list.texts.len.get(n))
        .and_then(|(&start, &len)| list.text.get(start as usize..start as usize + len as usize))
        .unwrap_or("")
}

fn text_bounds(list: &DisplayList, start: usize, end: usize) -> LabelBounds {
    let size = list.texts.size.get(start).copied().unwrap_or(1);
    let chars = (start..end)
        .map(|n| text_at(list, n).chars().count())
        .max()
        .unwrap_or(0);
    oa_display::core::font::text_block_bounds_px(
        (
            list.texts.x.get(start).copied().unwrap_or(0),
            list.texts.y.get(start).copied().unwrap_or(0),
        ),
        size,
        list.texts.align.get(start).copied().unwrap_or(ALIGN_LEFT),
        chars,
        (end - start) as u32,
        if end - start > 1 {
            14
        } else {
            u32::from(size) + 2
        },
    )
}

fn target_bounds(center: (i32, i32)) -> LabelBounds {
    LabelBounds {
        x0: center.0 / 16 - 9,
        y0: center.1 / 16 - 9,
        x1: center.0 / 16 + 10,
        y1: center.1 / 16 + 10,
    }
}

/// Eight compass directions, each with short/long leaders; stable ties keep NE short.
fn choose_label_bounds(
    center: (i32, i32),
    width: i32,
    height: i32,
    placed: &[LabelBounds],
    targets: &[(i32, i32)],
    fixes: &[LabelBounds],
    projection: &Projection,
) -> LabelBounds {
    let (x, y) = (center.0 / 16, center.1 / 16);
    let view = LabelBounds {
        x0: 2,
        y0: projection.top_inset as i32 + 2,
        x1: projection.width as i32 - 2,
        y1: projection.height as i32 - 2,
    };
    let mut best = EMPTY_LABEL;
    let mut best_cost = i64::MAX;
    for (dx, dy) in [
        (1, -1),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
    ] {
        for length in [60, 160] {
            let x0 = x + dx * length
                - if dx < 0 {
                    width
                } else if dx == 0 {
                    width / 2
                } else {
                    0
                };
            let y0 = y + dy * length
                - if dy < 0 {
                    height
                } else if dy == 0 {
                    height / 2
                } else {
                    0
                };
            let candidate = LabelBounds {
                x0,
                y0,
                x1: x0 + width,
                y1: y0 + height,
            };
            let outside = i64::from(width) * i64::from(height) - overlap_area(candidate, view);
            let cost = outside * 1000
                + placed
                    .iter()
                    .map(|&b| overlap_area(candidate, b))
                    .sum::<i64>()
                + targets
                    .iter()
                    .map(|&at| overlap_area(candidate, target_bounds(at)))
                    .sum::<i64>()
                + fixes
                    .iter()
                    .map(|&b| overlap_area(candidate, b))
                    .sum::<i64>();
            if cost < best_cost {
                best_cost = cost;
                best = candidate;
            }
        }
    }
    best
}

fn declutter_labels(
    list: &mut DisplayList,
    projection: &Projection,
    positions: &[(i32, i32)],
    callsigns: &[[u8; 8]],
) -> Result<(usize, usize, usize, usize), DisplayError> {
    let mut targets = [(0, 0); MAX_AIRCRAFT];
    let mut keys = [[0u8; 8]; MAX_AIRCRAFT];
    let mut near_airport = [false; MAX_AIRCRAFT];
    let mut count = 0;
    for (source, &(lat, lon)) in positions.iter().enumerate() {
        if oa_display::core::traffic::on_map(projection, lat, lon) {
            if let (Some(at), Some(slot), Some(key), Some(near)) = (
                project(projection, lat, lon),
                targets.get_mut(count),
                keys.get_mut(count),
                near_airport.get_mut(count),
            ) {
                *slot = at;
                *key = callsigns.get(source).copied().unwrap_or([0; 8]);
                *near = ord_distance_m(lat, lon).is_some_and(|d| d <= 5.0 * 1852.0);
                count += 1;
            }
        }
    }
    let targets = targets.get(..count).unwrap_or(&[]);
    let mut starts = [0usize; MAX_AIRCRAFT];
    let mut ends = [0usize; MAX_AIRCRAFT];
    let mut order = [0usize; MAX_AIRCRAFT];
    let mut cursor = 0;
    let mut block_count = 0;
    for (source, &center) in targets.iter().enumerate() {
        let old_x = center.0 + 14 * 16;
        let old_y = center.1 - 30 * 16;
        let Some(start) = (cursor..list.texts.x.len()).find(|&n| {
            list.texts.layer.get(n) == Some(&10)
                && list.texts.x.get(n) == Some(&old_x)
                && list.texts.y.get(n) == Some(&old_y)
        }) else {
            continue;
        };
        let mut end = start;
        while end < start + 3
            && list.texts.layer.get(end) == Some(&10)
            && list.texts.x.get(end) == Some(&old_x)
            && list
                .texts
                .y
                .get(end)
                .is_some_and(|&y| y == old_y + (end - start) as i32 * 14 * 16)
        {
            end += 1;
        }
        cursor = end;
        if let (Some(a), Some(b), Some(o)) = (
            starts.get_mut(source),
            ends.get_mut(source),
            order.get_mut(block_count),
        ) {
            *a = start;
            *b = end;
            *o = source;
            block_count += 1;
        }
    }
    if let Some(o) = order.get_mut(..block_count) {
        o.sort_unstable_by_key(|&i| (keys.get(i).copied().unwrap_or([0; 8]), i));
    }
    let mut fixes = [EMPTY_LABEL; MAX_MAP_LABELS];
    let mut fix_rows = [0usize; MAX_MAP_LABELS];
    let mut fix_count = 0;
    for n in 0..list.texts.x.len() {
        if matches!(list.texts.layer.get(n), Some(4 | 6)) && !text_at(list, n).is_empty() {
            let b = text_bounds(list, n, n + 1);
            if b.x1 > 0
                && b.y1 > projection.top_inset as i32
                && b.x0 < projection.width as i32
                && b.y0 < projection.height as i32
            {
                if let (Some(slot), Some(row)) =
                    (fixes.get_mut(fix_count), fix_rows.get_mut(fix_count))
                {
                    *slot = b;
                    *row = n;
                    fix_count += 1;
                }
            }
        }
    }
    let mut placed = [EMPTY_LABEL; MAX_AIRCRAFT];
    let mut placed_near = [false; MAX_AIRCRAFT];
    let mut placed_count = 0;
    for &source in order.get(..block_count).unwrap_or(&[]) {
        let Some(&center) = targets.get(source) else {
            continue;
        };
        let start = starts.get(source).copied().unwrap_or(0);
        let end = ends.get(source).copied().unwrap_or(0);
        let old = text_bounds(list, start, end);
        let mut bounds = choose_label_bounds(
            center,
            old.x1 - old.x0,
            old.y1 - old.y0,
            placed.get(..placed_count).unwrap_or(&[]),
            targets,
            fixes.get(..fix_count).unwrap_or(&[]),
            projection,
        );
        // Mandatory traffic wins over optional map names. If their current cost would
        // force a traffic collision, let only the names covering a clear candidate yield,
        // then evaluate the same sixteen candidates against the remaining obstacles.
        let traffic_collision = placed
            .get(..placed_count)
            .unwrap_or(&[])
            .iter()
            .any(|&p| overlap_area(bounds, p) > 0)
            || targets
                .iter()
                .any(|&t| overlap_area(bounds, target_bounds(t)) > 0);
        if traffic_collision {
            let clear = choose_label_bounds(
                center,
                old.x1 - old.x0,
                old.y1 - old.y0,
                placed.get(..placed_count).unwrap_or(&[]),
                targets,
                &[],
                projection,
            );
            for fix in fixes.get_mut(..fix_count).unwrap_or(&mut []) {
                if overlap_area(clear, *fix) > 0 {
                    *fix = EMPTY_LABEL;
                }
            }
            bounds = choose_label_bounds(
                center,
                old.x1 - old.x0,
                old.y1 - old.y0,
                placed.get(..placed_count).unwrap_or(&[]),
                targets,
                fixes.get(..fix_count).unwrap_or(&[]),
                projection,
            );
        }
        for n in start..end {
            if let Some(x) = list.texts.x.get_mut(n) {
                *x += (bounds.x0 - old.x0) * 16;
            }
            if let Some(y) = list.texts.y.get_mut(n) {
                *y += (bounds.y0 - old.y0) * 16;
            }
        }
        if let (Some(slot), Some(near)) = (
            placed.get_mut(placed_count),
            placed_near.get_mut(placed_count),
        ) {
            *slot = text_bounds(list, start, end);
            *near = near_airport.get(source).copied().unwrap_or(false);
            placed_count += 1;
        }
        let endpoint = (
            (center.0 / 16).clamp(bounds.x0, bounds.x1) * 16,
            (center.1 / 16).clamp(bounds.y0, bounds.y1) * 16,
        );
        push_line(
            list,
            center,
            endpoint,
            oa_display::core::palette::DIM_TEXT,
            8,
        )?;
    }
    // Fix names yield to aircraft blocks and symbols; KORD precedes active IAF/FAF, then other names.
    if let Some(rows) = fix_rows.get_mut(..fix_count) {
        rows.sort_unstable_by_key(|&n| {
            let name = text_at(list, n);
            (
                if name == "KORD" {
                    0
                } else if matches!(name, "JLOWW" | "SEIKA" | "WILLT") {
                    1
                } else {
                    2
                },
                n,
            )
        });
    }
    let mut kept = [EMPTY_LABEL; MAX_MAP_LABELS];
    let mut kept_count = 0;
    for &n in fix_rows.get(..fix_count).unwrap_or(&[]) {
        let b = text_bounds(list, n, n + 1);
        let collision = placed
            .get(..placed_count)
            .unwrap_or(&[])
            .iter()
            .any(|&p| overlap_area(b, p) > 0)
            || targets
                .iter()
                .any(|&t| overlap_area(b, target_bounds(t)) > 0)
            || kept
                .get(..kept_count)
                .unwrap_or(&[])
                .iter()
                .any(|&p| overlap_area(b, p) > 0);
        if collision {
            if let Some(len) = list.texts.len.get_mut(n) {
                *len = 0;
            }
        } else if let Some(slot) = kept.get_mut(kept_count) {
            *slot = b;
            kept_count += 1;
        }
    }
    let mut overlaps = 0;
    let mut outside = 0;
    let mut inside = 0;
    for a in 0..placed_count {
        for b in a + 1..placed_count {
            if placed
                .get(a)
                .zip(placed.get(b))
                .is_some_and(|(&a, &b)| overlap_area(a, b) > 0)
            {
                overlaps += 1;
                if placed_near.get(a) == Some(&true) && placed_near.get(b) == Some(&true) {
                    inside += 1;
                } else {
                    outside += 1;
                }
            }
        }
    }
    Ok((placed_count, overlaps, outside, inside))
}

fn ord_distance_m(lat: i32, lon: i32) -> Option<f64> {
    let origin = oa_geo::LatLon::new(
        f64::from(ord_map::KORD.0) * 1e-7,
        f64::from(ord_map::KORD.1) * 1e-7,
    )
    .ok()?;
    let point = oa_geo::LatLon::new(f64::from(lat) * 1e-7, f64::from(lon) * 1e-7).ok()?;
    oa_geo::wgs84()
        .inverse(origin, point)
        .ok()
        .map(|r| r.distance_m)
}

fn show_fix_name(lat: i32, lon: i32, name: &str) -> bool {
    // The selected public I27L/I28C/I28R final routes mark these fixes with the CIFP
    // description F (column43). No IAF transition is active in these selected final chains.
    matches!(name, "JLOWW" | "SEIKA" | "WILLT")
        || ord_distance_m(lat, lon).is_some_and(|distance| distance >= 12.0 * 1852.0)
}

/// Fixed map primitives use only the source-pinned public CIFP constant table.
fn draw_public_map(list: &mut DisplayList, projection: &Projection) -> Result<(), DisplayError> {
    use oa_display::core::palette;
    for route in ord_map::ROUTES {
        let mut previous = None;
        for &(lat, lon, name, synthetic_direct) in *route {
            if let Some(at) = project(projection, lat, lon) {
                if let Some(from) = previous {
                    if synthetic_direct {
                        // Explicit synthetic DCT: eight separated dash segments.
                        let (x, y): (i32, i32) = from;
                        for dash in 0..8 {
                            let interpolate = |n: i32| {
                                (
                                    x + ((i64::from(at.0 - x) * i64::from(n)) / 16) as i32,
                                    y + ((i64::from(at.1 - y) * i64::from(n)) / 16) as i32,
                                )
                            };
                            push_line(
                                list,
                                interpolate(dash * 2),
                                interpolate(dash * 2 + 1),
                                palette::RING,
                                2,
                            )?;
                        }
                    } else {
                        push_line(list, from, at, palette::AIRWAY, 2)?;
                    }
                }
                push_circle(list, at, 2 * 16, palette::FIX, 3)?;
                if show_fix_name(lat, lon, name) {
                    push_text(
                        list,
                        (at.0 + 4 * 16, at.1 + 4 * 16),
                        9,
                        ALIGN_LEFT,
                        palette::DIM_TEXT,
                        4,
                        name,
                    )?;
                }
                previous = Some(at);
            } else {
                previous = None;
            }
        }
    }
    for &(a_lat, a_lon, b_lat, b_lon, name) in ord_map::RUNWAYS {
        if let (Some(a), Some(b)) = (
            project(projection, a_lat, a_lon),
            project(projection, b_lat, b_lon),
        ) {
            push_line(list, a, b, palette::AIRPORT, 5)?;
            if projection.range_nm < 60.0 {
                push_text(
                    list,
                    (a.0 + 4 * 16, a.1 - 14 * 16),
                    9,
                    ALIGN_LEFT,
                    palette::AIRPORT,
                    6,
                    name,
                )?;
            }
        }
    }
    if let Some(at) = project(projection, ord_map::KORD.0, ord_map::KORD.1) {
        push_circle(list, at, 4 * 16, palette::AIRPORT, 5)?;
        push_text(
            list,
            (at.0 + 7 * 16, at.1 + 7 * 16),
            10,
            ALIGN_LEFT,
            palette::AIRPORT,
            6,
            "KORD",
        )?;
    }
    Ok(())
}

fn flight_key(seed: u64, index: u16) -> FlightKey {
    let gufi = (u128::from(seed) << 64) | u128::from(index.saturating_add(1));
    key_gufi(gufi)
}

fn measurement_of(report: SyntheticReport, toa_ns: i64, position_sigma_m: f32) -> Measurement {
    let addr = synthetic_address(report.aircraft_index);
    let speed_mps = f64::from(report.ground_speed_dkt) * 0.1 * (1852.0 / 3600.0);
    let angle = f64::from(report.track_ddeg) * 0.1_f64.to_radians();
    let east = (speed_mps * oa_math::sin(angle)) as f32;
    let north = (speed_mps * oa_math::cos(angle)) as f32;
    let var = position_sigma_m * position_sigma_m;
    Measurement {
        toa_ns,
        pos_cov_m2: [var, 0.0, var],
        vel_cov_m2s2: [1.0, 0.0, 1.0],
        vel_east_mps: east,
        vel_north_mps: north,
        lat_e7: report.lat_e7,
        lon_e7: report.lon_e7,
        alt_ft: report.altitude_ft,
        vrate_fpm: report.vertical_rate_fpm,
        addr,
        alt_quant_ft: 100,
        sensor: SYNTHETIC_SENSOR_ID,
        ident: oa_track_core::NO_IDENT,
        flags: M_HAS_VEL,
        category: 0,
        vel_lag_s: 0.0,
    }
}

fn hid_ascii(usage: u32, modifiers: u32) -> Option<u8> {
    let shifted = modifiers & 1 != 0;
    match usage {
        0x04..=0x1D => {
            let ch = b'a'.checked_add(u8::try_from(usage - 0x04).ok()?)?;
            Some(if shifted { ch.to_ascii_uppercase() } else { ch })
        }
        0x1E..=0x27 => {
            let digits = if shifted {
                *b"!@#$%^&*()"
            } else {
                *b"1234567890"
            };
            digits.get(usize::try_from(usage - 0x1E).ok()?).copied()
        }
        0x2C => Some(b' '),
        0x2D => Some(if shifted { b'_' } else { b'-' }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{hid_ascii, normalize_host_key_modifiers};

    #[test]
    fn host_modifier_mapping_preserves_all_sixteen_masks_and_shift_semantics() {
        for mask in 0..16_u32 {
            let normalized = normalize_host_key_modifiers(mask);
            let expected =
                ((mask & 0x04) >> 2) | ((mask & 0x01) << 1) | ((mask & 0x02) << 1) | (mask & 0x08);
            assert_eq!(normalized, expected, "host mask {mask:#06b}");

            let shifted = mask & 0x04 != 0;
            assert_eq!(
                hid_ascii(0x04, normalized),
                Some(if shifted { b'A' } else { b'a' }),
                "letter for host mask {mask:#06b}"
            );
            assert_eq!(
                hid_ascii(0x1E, normalized),
                Some(if shifted { b'!' } else { b'1' }),
                "digit for host mask {mask:#06b}"
            );
            assert_eq!(
                hid_ascii(0x2D, normalized),
                Some(if shifted { b'_' } else { b'-' }),
                "dash for host mask {mask:#06b}"
            );
        }
        assert_eq!(normalize_host_key_modifiers(0x01), 0x02);
        assert_eq!(
            hid_ascii(0x04, normalize_host_key_modifiers(0x01)),
            Some(b'a')
        );
    }
    #[test]
    fn engine_navigation_changes_the_frame_and_home_restores_center(
    ) -> Result<(), super::EngineError> {
        let config =
            super::DemoEngineConfig::normal_ord(7, 2_700_000, 0, 5_000, 419_800_000, -878_000_000);
        let reports = [-10_000, -5_000, 0]
            .into_iter()
            .map(|virtual_time_ms| super::SyntheticReport {
                aircraft_index: 0,
                flags: 0,
                virtual_time_ms,
                lat_e7: 419_800_000,
                lon_e7: -878_000_000,
                altitude_ft: 10_000,
                ground_speed_dkt: 2_200,
                track_ddeg: 900,
                vertical_rate_fpm: 0,
            })
            .collect();
        let mut engine = super::DemoEngine::from_reports(config, reports)?;
        engine.advance_to(0)?;
        let initial = engine.render_frame()?.pixel_digest;
        engine.handle_key(0x4f, 0, true, false)?;
        assert_ne!(engine.render_frame()?.pixel_digest, initial);
        engine.handle_key(0x4a, 0, true, false)?;
        assert_eq!(engine.render_frame()?.pixel_digest, initial);
        let range = engine.position.range_nm;
        engine.handle_key(0x2d, 0, true, false)?;
        assert!(engine.position.range_nm > range);
        let zoomed = engine.position.range_nm;
        engine.handle_key(0x2e, 0, true, false)?;
        assert!(engine.position.range_nm < zoomed);
        Ok(())
    }
    #[test]
    fn label_placement_keeps_default_ties_and_avoids_occupied_labels(
    ) -> Result<(), super::DisplayError> {
        let projection = super::projection_new(419_800_000, -878_000_000, 80.0, 1280, 720, 104)?;
        let center = (640 * 16, 400 * 16);
        let first = super::choose_label_bounds(center, 72, 32, &[], &[center], &[], &projection);
        assert_eq!(
            (first.x0, first.y0),
            (center.0 / 16 + 60, center.1 / 16 - 32 - 60)
        );
        let second =
            super::choose_label_bounds(center, 72, 32, &[first], &[center], &[], &projection);
        assert_eq!(super::overlap_area(first, second), 0);
        assert_ne!((first.x0, first.y0), (second.x0, second.y0));
        let repeated =
            super::choose_label_bounds(center, 72, 32, &[first], &[center], &[], &projection);
        assert_eq!((second.x0, second.y0), (repeated.x0, repeated.y0));
        Ok(())
    }
    #[test]
    fn conflict_alert_runs_after_same_scan_reports_and_preserves_active_pair(
    ) -> Result<(), super::EngineError> {
        let config =
            super::DemoEngineConfig::normal_ord(7, 2_700_000, 0, 5_000, 419_800_000, -878_000_000);
        let mut reports = super::Vec::new();
        for virtual_time_ms in [-10_000, -5_000, 0] {
            for aircraft_index in 0..2 {
                reports.push(super::SyntheticReport {
                    aircraft_index,
                    flags: 0,
                    virtual_time_ms,
                    lat_e7: 419_800_000,
                    lon_e7: -878_000_000,
                    altitude_ft: 10_000,
                    ground_speed_dkt: 2_200,
                    track_ddeg: 900,
                    vertical_rate_fpm: 0,
                });
            }
        }
        let mut engine = super::DemoEngine::from_reports(config, reports)?;
        engine.advance_to(0)?;
        assert!(engine.counters.ca_cycles >= 2);
        assert_eq!(engine.counters.ca_open_alerts, 1);
        assert_eq!(engine.counters.ca_work_short, 0);
        assert!(engine.render_frame()?.ca_available);
        let cycles = engine.counters.ca_cycles;
        engine.advance_to(0)?;
        assert_eq!(engine.counters.ca_cycles, cycles);
        assert_eq!(engine.counters.ca_open_alerts, 1);
        Ok(())
    }
    // REQ: OA-DEMO-001
    #[test]
    fn terminal_map_hides_inner_names_and_keeps_published_final_approach_fixes() {
        for name in ["JLOWW", "SEIKA", "WILLT"] {
            assert!(super::show_fix_name(
                super::ord_map::KORD.0,
                super::ord_map::KORD.1,
                name
            ));
        }
        assert!(!super::show_fix_name(
            super::ord_map::KORD.0,
            super::ord_map::KORD.1,
            "NONCRITICAL"
        ));
        assert!(super::show_fix_name(401350167, -925917139, "IRK"));
    }
    // REQ: OA-DEMO-001
    #[test]
    fn normal_push_full_hour_has_no_conflict_alerts() -> Result<(), super::EngineError> {
        // Compiled public-CIFP/synthetic fixture; no SWIM or external input is read.
        let bytes = include_bytes!("../../../oa-demo-scenario/tests/data/ord-normal-push-v1.oasd");
        let mut engine = crate::shell::from_oasd(bytes, oa_sha256::core::sha256::sha256(bytes))?;
        let mut conflicts = Vec::new();
        let mut conflict_cycles = 0;
        for time in (0..=3_600_000).step_by(5_000) {
            engine.advance_to(time)?;
            if engine.counters.ca_open_alerts != 0 {
                conflict_cycles += 1;
                for alert in engine.active_ca_alerts() {
                    if conflicts.len() < 12
                        && !conflicts
                            .iter()
                            .any(|&(_, a, b)| a == alert.addr_a && b == alert.addr_b)
                    {
                        if conflicts.len() < 3 {
                            eprintln!(
                                "normal_fixture_conflict t={time} alert={alert:?} rows={:?}",
                                engine
                                    .ca_rows
                                    .iter()
                                    .filter(|r| r.addr == alert.addr_a || r.addr == alert.addr_b)
                                    .collect::<Vec<_>>()
                            );
                        }
                        conflicts.push((time, alert.addr_a, alert.addr_b));
                    }
                }
            }
            assert_eq!(engine.counters.ca_work_short, 0);
            assert_eq!(engine.counters.ca_input_short, 0);
        }
        assert_eq!(
            conflict_cycles, 0,
            "synthetic normal push invalid; first pairs: {conflicts:?}"
        );
        assert_eq!(engine.counters.arrivals_completed, 60);
        Ok(())
    }
    // REQ: OA-DEMO-001
    #[test]
    fn e38_fifteen_minute_fixture_exposes_rendered_text_collisions(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut rows = Vec::new();
        for line in include_str!("../../tests/data/e38d312-15m-layout.tsv")
            .lines()
            .filter(|l| !l.starts_with('#'))
        {
            let mut fields = line.split('\t');
            let name = fields.next().ok_or("missing name")?;
            let x = fields.next().ok_or("missing x")?.parse::<i32>()?;
            let y = fields.next().ok_or("missing y")?.parse::<i32>()?;
            let size = fields.next().ok_or("missing size")?.parse::<u8>()?;
            let lines = fields.next().ok_or("missing lines")?.parse::<u32>()?;
            assert!(fields.next().is_none());
            let b = oa_display::core::font::text_block_bounds_px(
                (x * 16, y * 16),
                size,
                0,
                name.len(),
                lines,
                14,
            );
            rows.push((name, b));
        }
        let mut conflicts = Vec::new();
        for (a, &(an, aa)) in rows.iter().enumerate() {
            for &(bn, bb) in rows.iter().skip(a + 1) {
                if super::overlap_area(aa, bb) > 0 {
                    conflicts.push((an, bn));
                }
            }
        }
        eprintln!("e38_frozen_overlap_pairs={} {conflicts:?}", conflicts.len());
        assert!(conflicts.len() >= 5);
        assert!(conflicts
            .iter()
            .any(|&(a, b)| (a == "TUBEZ" && b == "SYN0026") || (b == "TUBEZ" && a == "SYN0026")));
        assert!(conflicts
            .iter()
            .any(|&(a, b)| (a == "RW27L" && b == "RW28C") || (b == "RW27L" && a == "RW28C")));
        Ok(())
    }

    // REQ: OA-DEMO-001
    #[test]
    fn rendered_block_and_map_geometry_is_independently_collision_free(
    ) -> Result<(), super::EngineError> {
        let bytes = include_bytes!("../../../oa-demo-scenario/tests/data/ord-normal-push-v1.oasd");
        let mut engine = crate::shell::from_oasd(bytes, oa_sha256::core::sha256::sha256(bytes))?;
        for minute in [0, 5, 15, 30, 60] {
            engine.advance_to(minute * 60_000)?;
            engine.render_frame()?;
            let l = &engine.display_list;
            let mut blocks = Vec::new();
            let mut fixes = Vec::new();
            let mut row = 0;
            while row < l.texts.x.len() {
                let text = super::text_at(l, row);
                if l.texts.layer.get(row) == Some(&10) && text.starts_with("SYN") {
                    let mut end = row + 1;
                    while end < row + 3
                        && l.texts.layer.get(end) == Some(&10)
                        && l.texts.x.get(end) == l.texts.x.get(row)
                        && l.texts.y.get(end)
                            == Some(
                                &(l.texts.y.get(row).copied().unwrap_or(0)
                                    + (end - row) as i32 * 14 * 16),
                            )
                    {
                        end += 1;
                    }
                    blocks.push((text, super::text_bounds(l, row, end)));
                    row = end;
                } else {
                    if matches!(l.texts.layer.get(row), Some(4 | 6)) && !text.is_empty() {
                        fixes.push((text, super::text_bounds(l, row, row + 1)));
                    }
                    row += 1;
                }
            }
            assert_eq!(blocks.len(), engine.frame_target_count());
            let mut collisions = Vec::new();
            for (a, &(an, aa)) in blocks.iter().enumerate() {
                for &(bn, bb) in blocks.iter().skip(a + 1) {
                    if super::overlap_area(aa, bb) > 0 {
                        collisions.push((an, bn));
                    }
                }
            }
            for &(a, aa) in &blocks {
                for &(b, bb) in &fixes {
                    if super::overlap_area(aa, bb) > 0 {
                        collisions.push((a, b));
                    }
                }
            }
            for (a, &(an, aa)) in fixes.iter().enumerate() {
                for &(bn, bb) in fixes.iter().skip(a + 1) {
                    if super::overlap_area(aa, bb) > 0 {
                        collisions.push((an, bn));
                    }
                }
            }
            eprintln!(
                "actual_frame minute={minute} blocks={} fix_labels={} collisions={collisions:?}",
                blocks.len(),
                fixes.len()
            );
            assert!(collisions.is_empty(), "minute {minute}: {collisions:?}");
        }
        Ok(())
    }
    // REQ: OA-DEMO-001
    #[test]
    fn normal_push_requested_frames_have_no_outer_terminal_label_overlap(
    ) -> Result<(), super::EngineError> {
        let bytes = include_bytes!("../../../oa-demo-scenario/tests/data/ord-normal-push-v1.oasd");
        let mut engine = crate::shell::from_oasd(bytes, oa_sha256::core::sha256::sha256(bytes))?;
        for minute in [0, 5, 15, 30, 60] {
            engine.advance_to(minute * 60_000)?;
            engine.render_frame()?;
            let count = engine.frame_target_count();
            assert!(count <= 30, "minute {minute}: {count} targets");
            assert_eq!(engine.counters.labels_placed, count as u64);
            eprintln!(
                "normal_frame minute={minute} targets={count} outer_overlaps={} inner_overlaps={}",
                engine.counters.label_overlap_pairs_outside_terminal,
                engine.counters.label_overlap_pairs_inside_terminal
            );
            assert_eq!(
                engine.counters.label_overlap_pairs_outside_terminal, 0,
                "minute {minute}: outer-terminal data blocks overlap"
            );
        }
        Ok(())
    }
}

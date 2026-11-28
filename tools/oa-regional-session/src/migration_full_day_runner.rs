//! Full-day M4/H2 development runner. This is an offline diagnostic, not a live service.
#![forbid(unsafe_code)]

use crate::input::{EventKind, NormalizedEvent, SourceKind};
use crate::migration_amendment::{
    amendment_accumulator_new, amendment_observe, amendment_summary, AmendmentAccumulator,
};
use crate::migration_full_day::FullDayCounts;
use crate::migration_g3::{self, CandidatePosition, SourceReference, TrainingModel};
use crate::migration_snapshot;
use crate::recordings::adapt_recorded_frames_with_sequence_starts;
use crate::recordings_reader::{RecordedFrame, RecordingCounts, RecordingSource};
use crate::runtime::{
    regional_new_with_profile, regional_profile_limits, regional_step, FlightEvidence,
    RegionalFrame, RegionalRunProfile, RegionalRuntime,
};
use crate::session::ReceiptBatch;
use crate::surveillance::{ObservedTrack, SurveillanceConfig};
use oa_flight_sfdps::audit::RowIdentity;
use oa_flight_sfdps::migration_projection::{row_facts, SfdpsRowFacts};

pub const DAY_START_NS: i64 = 1_795_046_400_000_000_000;
pub const DAY_END_NS: i64 = 1_795_132_800_000_000_000;
pub const DAY_TICK_NS: i64 = 1_000_000_000;
pub const MAX_RUNTIME_FRAMES: u64 = 86_400;
pub const MAX_NORMALIZED_EVENTS: u64 = 10_000_000;
pub const MAX_SFDPS_ROWS: u64 = 10_000_000;
pub const MAX_EVENT_GROUP: usize = 256;
pub const MAX_RECEIVE_GROUP_FRAMES: usize = 4096;
pub const MAX_RECEIVE_GROUP_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_XML_ELEMENTS: usize = 262_144;
pub const MAX_XML_ATTRIBUTES: usize = 524_288;
pub const MAX_XML_TEXT_NODES: usize = 262_144;
pub const MAX_FIXM_STRING_BYTES: usize = 1 << 20;
pub const MAP_RANGE_NM: f64 = 1_000.0;

fn fixm_summary_refusal_count(
    summary: &oa_fixm::Summary,
    emitted_rows: u32,
) -> Result<u64, String> {
    if summary.rows != emitted_rows {
        return Err(format!(
            "FIXM summary row count {} differs from emitted Events rows {}",
            summary.rows, emitted_rows
        ));
    }
    if summary.sealed_window_dropped > 0 {
        if summary.rows != 0
            || summary.rejected != 0
            || summary.not_decoded != 0
            || summary.messages != 0
        {
            return Err("FIXM sealed-window disposition contradicts other summary counts".into());
        }
        return Ok(u64::from(summary.sealed_window_dropped));
    }
    let accounted = summary
        .rows
        .checked_add(summary.rejected)
        .and_then(|value| value.checked_add(summary.not_decoded))
        .ok_or("FIXM summary disposition count overflow")?;
    if summary.not_decoded == 0 && accounted != summary.messages {
        return Err(format!(
            "FIXM messages {} do not reconcile to rows {} + rejected {}",
            summary.messages, summary.rows, summary.rejected
        ));
    }
    if summary.not_decoded > 0 && accounted < summary.messages {
        return Err(format!(
            "FIXM prefix disposition total {} is below messages visited {}",
            accounted, summary.messages
        ));
    }
    Ok(u64::from(summary.rejected) + u64::from(summary.not_decoded))
}

// Budget: each receipt group holds at most MAX_EVENT_GROUP normalized events. The fixed
// normalized event (including its largest 16-fix/256-byte route variant) plus original receive
// timestamp and the explicit Option<u64> source-message identity is 448 bytes; the latter adds
// 16 bytes to the prior layout. The unchanged 256-entry Vec holds 114,688 element bytes.
#[repr(C)]
struct PendingEvent {
    event: NormalizedEvent,
    original_receive_ns: i64,
}
const _: () = assert!(core::mem::size_of::<PendingEvent>() == 448);

#[derive(Clone, Copy)]
struct PositionAvailability {
    address: u32,
    source_ns: i64,
    receive_ns: i64,
}

struct ComparisonContext<'a> {
    counts: &'a mut FullDayCounts,
    amendments: &'a mut AmendmentAccumulator,
    g3_model: &'a TrainingModel,
    tracks: &'a [ObservedTrack],
    flights: &'a [crate::flight::SharedFlightView],
    availability: &'a [PositionAvailability],
}

/// Owns one runtime and only the last bounded output projection. Every returned frame is
/// aggregated and dropped immediately; no day-length output history is retained.
pub struct FullDayRunner {
    runtime: RegionalRuntime,
    g3_model: TrainingModel,
    centre: [u8; 4],
    counts: FullDayCounts,
    next_tick_ns: i64,
    last_tick_ns: Option<i64>,
    pending_receipt_ns: Option<i64>,
    pending: Vec<PendingEvent>,
    sequence_starts: [u64; 2],
    prior_sequence: [Option<u64>; 5],
    prior_source_ns: [Option<i64>; 5],
    event_total: u64,
    sfdps_rows: u64,
    latest_tracks: Vec<ObservedTrack>,
    latest_flights: Vec<crate::flight::SharedFlightView>,
    availability: Vec<PositionAvailability>,
    xml_parser: oa_xml::Parser,
    fixm_decoder: oa_fixm::Decoder,
    fixm_events: oa_fixm::Events,
    amendments: AmendmentAccumulator,
}

impl FullDayRunner {
    pub fn new(
        model: oa_airspace::core::model::Model,
        model_sha: [u8; 32],
        cfg: SurveillanceConfig,
        g3_model: TrainingModel,
    ) -> Result<Self, String> {
        let runtime = regional_new_with_profile(
            model,
            model_sha,
            *b"ZAU1",
            cfg,
            RegionalRunProfile::recorded_peak(),
        )?;
        let profile = regional_profile_limits(&runtime);
        if profile != RegionalRunProfile::recorded_peak() {
            return Err("runtime did not retain the requested recorded-peak profile".into());
        }
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(MAX_EVENT_GROUP)
            .map_err(|_| "event group allocation refused")?;
        let mut latest_tracks = Vec::new();
        latest_tracks
            .try_reserve_exact(4096)
            .map_err(|_| "track projection allocation refused")?;
        let mut latest_flights = Vec::new();
        latest_flights
            .try_reserve_exact(8192)
            .map_err(|_| "flight projection allocation refused")?;
        let mut availability = Vec::new();
        availability
            .try_reserve_exact(4096)
            .map_err(|_| "availability index allocation refused")?;
        let xml_parser = oa_xml::Parser::new(oa_xml::Limits {
            max_input_bytes: oa_feed::MAX_PAYLOAD,
            max_elements: MAX_XML_ELEMENTS,
            max_attributes: MAX_XML_ATTRIBUTES,
            max_text_nodes: MAX_XML_TEXT_NODES,
            ..oa_xml::Limits::default()
        })
        .map_err(|error| format!("FIXM parser allocation: {error:?}"))?;
        let mut fixm_limits = oa_fixm::Limits::new(oa_sealed::sealed_drop_window());
        fixm_limits.max_elements = MAX_XML_ELEMENTS;
        let fixm_decoder = oa_fixm::Decoder::new(fixm_limits);
        let fixm_events = oa_fixm::new_events(4096, MAX_FIXM_STRING_BYTES);
        let amendments = amendment_accumulator_new()
            .map_err(|error| format!("amendment baseline allocation refused: {error:?}"))?;
        Ok(Self {
            runtime,
            g3_model,
            centre: *b"ZAU1",
            counts: FullDayCounts::default(),
            next_tick_ns: DAY_START_NS,
            last_tick_ns: None,
            pending_receipt_ns: None,
            pending,
            sequence_starts: [0; 2],
            prior_sequence: [None; 5],
            prior_source_ns: [None; 5],
            event_total: 0,
            sfdps_rows: 0,
            latest_tracks,
            latest_flights,
            availability,
            xml_parser,
            fixm_decoder,
            fixm_events,
            amendments,
        })
    }

    pub fn record_stream_summary(
        &mut self,
        summary: &crate::recordings_stream::StreamSummary,
    ) -> Result<(), String> {
        if summary.streams.is_empty() || summary.counts.messages != self.counts.input_frames {
            return Err("stream and processed message counts do not reconcile".into());
        }
        if !summary
            .streams
            .iter()
            .any(|stream| stream.source == RecordingSource::Adsb && stream.messages > 0)
            || !summary
                .streams
                .iter()
                .any(|stream| stream.source == RecordingSource::Sfdps && stream.messages > 0)
        {
            return Err("full-day profile requires message frames from both sources".into());
        }
        self.counts.input_frames = summary.counts.frames;
        self.counts.input_status_frames = summary.counts.status_frames;
        self.counts.input_reject_frames = summary.counts.reject_frames;
        Ok(())
    }

    pub fn process_receive_group(&mut self, frames: &[RecordedFrame]) -> Result<(), String> {
        let Some(first) = frames.first() else {
            return Ok(());
        };
        let receive_ns = first.receipt_ns;
        if frames.len() > MAX_RECEIVE_GROUP_FRAMES
            || frames.iter().any(|frame| frame.receipt_ns != receive_ns)
        {
            return Err(
                "same-receive group exceeds fixed capacity or has inconsistent clocks".into(),
            );
        }
        let mut payload_bytes = 0usize;
        for frame in frames {
            payload_bytes = payload_bytes
                .checked_add(frame.payload.len())
                .ok_or("receive-group byte count overflow")?;
        }
        if payload_bytes > MAX_RECEIVE_GROUP_BYTES {
            return Err("same-receive group payload exceeds fixed capacity".into());
        }

        // Convert the receive group once so XML/FIXM tables are reused for every message in this
        // exact timestamp group. The adapter and raw parser remain the authority for source rows.
        self.counts.input_frames = self
            .counts
            .input_frames
            .checked_add(u64::try_from(frames.len()).map_err(|_| "input frame count overflow")?)
            .ok_or("input frame count overflow")?;
        let decoded_bytes =
            u64::try_from(payload_bytes).map_err(|_| "payload byte count overflow")?;
        let recording = RecordingCounts {
            total_frames: u64::try_from(frames.len()).map_err(|_| "frame count overflow")?,
            total_decoded_bytes: decoded_bytes,
            total_bytes: decoded_bytes,
            ..RecordingCounts::default()
        };
        let adapted = adapt_recorded_frames_with_sequence_starts(
            frames,
            recording,
            100_000,
            self.centre,
            self.sequence_starts,
        )
        .map_err(|error| format!("recorded message adaptation refused: {error:?}"))?;
        self.counts.adaptation_quarantine_rows = self
            .counts
            .adaptation_quarantine_rows
            .checked_add(
                u64::try_from(adapted.quarantine.len()).map_err(|_| "quarantine count overflow")?,
            )
            .ok_or("quarantine count overflow")?;
        self.counts.adapter_unprocessed_frames = self
            .counts
            .adapter_unprocessed_frames
            .checked_add(adapted.counts.unprocessed_frames)
            .ok_or("unprocessed-frame count overflow")?;
        let event_count =
            u64::try_from(adapted.events.len()).map_err(|_| "event count overflow")?;
        self.event_total = self
            .event_total
            .checked_add(event_count)
            .ok_or("event count overflow")?;
        if self.event_total > MAX_NORMALIZED_EVENTS {
            return Err("normalized-event full-day cap exceeded".into());
        }
        self.counts.adapted_events = self
            .counts
            .adapted_events
            .checked_add(event_count)
            .ok_or("event count overflow")?;
        for (event, provenance) in adapted.events.into_iter().zip(adapted.provenance) {
            let starts_index = match event.source {
                SourceKind::Surveillance => Some(0),
                SourceKind::Flight | SourceKind::Route => Some(1),
                SourceKind::Weather | SourceKind::Adaptation => None,
            };
            if let Some(index) = starts_index {
                let sequence_start = self
                    .sequence_starts
                    .get_mut(index)
                    .ok_or("normalized sequence source index invariant")?;
                *sequence_start = (*sequence_start).max(
                    event
                        .sequence
                        .checked_add(1)
                        .ok_or("normalized sequence overflow")?,
                );
            }
            let event = match event.kind {
                EventKind::Position {
                    latitude_e7,
                    longitude_e7,
                    ..
                } => {
                    self.counts.spatial_positions_seen = self
                        .counts
                        .spatial_positions_seen
                        .checked_add(1)
                        .ok_or("spatial position count overflow")?;
                    let box_filter = oa_adsb::ZAU_BOX;
                    let inside = (box_filter.lat_min_e7..=box_filter.lat_max_e7)
                        .contains(&latitude_e7)
                        && (box_filter.lon_min_e7..=box_filter.lon_max_e7).contains(&longitude_e7);
                    if inside {
                        self.counts.spatial_positions_inside = self
                            .counts
                            .spatial_positions_inside
                            .checked_add(1)
                            .ok_or("spatial position count overflow")?;
                    } else {
                        self.counts.spatial_positions_outside = self
                            .counts
                            .spatial_positions_outside
                            .checked_add(1)
                            .ok_or("spatial position count overflow")?;
                        continue;
                    }
                    event
                }
                _ => event,
            };
            self.push_event(PendingEvent {
                event,
                original_receive_ns: provenance.input_receipt_ns,
            })?;
        }

        let tick = receive_ns
            .div_euclid(DAY_TICK_NS)
            .checked_mul(DAY_TICK_NS)
            .ok_or("receive tick overflow")?;
        self.advance_until(tick)?;
        for frame in frames
            .iter()
            .filter(|frame| frame.source == RecordingSource::Sfdps)
        {
            self.compare_sfdps_frame(frame)?;
        }
        Ok(())
    }

    fn push_event(&mut self, pending: PendingEvent) -> Result<(), String> {
        let receipt_ns = pending.event.receipt_ns;
        if !(DAY_START_NS..=DAY_END_NS).contains(&receipt_ns) {
            return Err("normalized issue time falls outside the fixed full-day profile".into());
        }
        if self
            .pending_receipt_ns
            .is_some_and(|prior| receipt_ns < prior)
        {
            return Err("normalized receipt order regressed".into());
        }
        if self
            .pending_receipt_ns
            .is_some_and(|prior| receipt_ns > prior)
        {
            self.advance_until(self.pending_receipt_ns.ok_or("pending receipt invariant")?)?;
        }
        if self.pending.len() >= MAX_EVENT_GROUP {
            return Err("normalized receipt group exceeds 256-event capacity".into());
        }
        self.pending
            .try_reserve(1)
            .map_err(|_| "normalized event allocation refused")?;
        self.pending_receipt_ns = Some(receipt_ns);
        self.pending.push(pending);
        Ok(())
    }

    fn advance_until(&mut self, target_ns: i64) -> Result<(), String> {
        if !(DAY_START_NS..DAY_END_NS).contains(&target_ns) {
            return if target_ns < DAY_START_NS {
                Ok(())
            } else {
                Err("runtime target is outside half-open full-day interval".into())
            };
        }
        loop {
            if self.next_tick_ns > target_ns {
                break;
            }
            let tick = self.next_tick_ns;
            if self
                .pending_receipt_ns
                .is_some_and(|receipt| receipt < tick)
            {
                return Err("normalized event group missed its issue tick".into());
            }
            let (events, source_clock_regressions) = if self.pending_receipt_ns == Some(tick) {
                self.take_pending_events(tick)?
            } else {
                (Vec::new(), 0)
            };
            self.apply_tick(tick, events, source_clock_regressions)?;
            self.next_tick_ns = self
                .next_tick_ns
                .checked_add(DAY_TICK_NS)
                .ok_or("runtime clock overflow")?;
        }
        Ok(())
    }

    fn take_pending_events(&mut self, tick: i64) -> Result<(Vec<NormalizedEvent>, u16), String> {
        if self.pending_receipt_ns != Some(tick) {
            return Err("pending event group tick mismatch".into());
        }
        self.pending
            .sort_by_key(|item| (item.event.source, item.event.sequence));
        let mut events = Vec::new();
        events
            .try_reserve_exact(self.pending.len())
            .map_err(|_| "receipt batch allocation refused")?;
        let mut clock_regressions = 0u16;
        for index in 0..self.pending.len() {
            let (event, original_receive_ns) = {
                let item = self
                    .pending
                    .get(index)
                    .ok_or("pending event index invariant")?;
                (item.event, item.original_receive_ns)
            };
            let source_ix = match event.source {
                SourceKind::Surveillance => 0,
                SourceKind::Flight => 1,
                SourceKind::Route => 2,
                SourceKind::Weather => 3,
                SourceKind::Adaptation => 4,
            };
            let prior_sequence = self
                .prior_sequence
                .get(source_ix)
                .copied()
                .ok_or("prior sequence source index invariant")?;
            let prior_source_ns = self
                .prior_source_ns
                .get(source_ix)
                .copied()
                .ok_or("prior source time index invariant")?;
            if prior_sequence.is_some_and(|prior| event.sequence <= prior) {
                return Err("normalized source sequence is not strictly increasing".into());
            }
            if prior_source_ns.is_some_and(|prior| event.source_ns < prior) {
                clock_regressions = clock_regressions
                    .checked_add(1)
                    .ok_or("clock regression count overflow")?;
            }
            *self
                .prior_sequence
                .get_mut(source_ix)
                .ok_or("prior sequence source index invariant")? = Some(event.sequence);
            *self
                .prior_source_ns
                .get_mut(source_ix)
                .ok_or("prior source time index invariant")? = Some(event.source_ns);
            if let EventKind::Position { .. } = event.kind {
                self.note_position_availability(event, original_receive_ns)?;
            }
            events.push(event);
        }
        self.pending.clear();
        self.pending_receipt_ns = None;
        self.counts.source_clock_regressions = self
            .counts
            .source_clock_regressions
            .checked_add(u64::from(clock_regressions))
            .ok_or("source clock regression count overflow")?;
        let batch = ReceiptBatch {
            receipt_ns: tick,
            source_clock_regressions: u64::from(clock_regressions),
            events,
        };
        crate::session::summarize_input_batch(&batch)
            .map_err(|error| format!("receipt accounting: {error:?}"))?;
        Ok((batch.events, clock_regressions))
    }

    fn note_position_availability(
        &mut self,
        event: NormalizedEvent,
        receive_ns: i64,
    ) -> Result<(), String> {
        if event.identity.aircraft_address == 0 || receive_ns < 0 || receive_ns > event.receipt_ns {
            return Err("invalid input-availability evidence for position".into());
        }
        let address = event.identity.aircraft_address;
        match self
            .availability
            .binary_search_by_key(&address, |row| row.address)
        {
            Ok(index) => {
                let slot = self
                    .availability
                    .get_mut(index)
                    .ok_or("availability index invariant")?;
                if event.source_ns >= slot.source_ns {
                    *slot = PositionAvailability {
                        address,
                        source_ns: event.source_ns,
                        receive_ns,
                    };
                }
            }
            Err(index) => {
                if self.availability.len() >= 4096 {
                    return Err("position-availability index capacity exceeded".into());
                }
                self.availability
                    .try_reserve(1)
                    .map_err(|_| "position-availability index allocation refused")?;
                self.availability.insert(
                    index,
                    PositionAvailability {
                        address,
                        source_ns: event.source_ns,
                        receive_ns,
                    },
                );
            }
        }
        Ok(())
    }

    fn apply_tick(
        &mut self,
        tick: i64,
        events: Vec<NormalizedEvent>,
        source_clock_regressions: u16,
    ) -> Result<(), String> {
        if self.last_tick_ns.is_some_and(|prior| tick <= prior) {
            return Err("runtime issue time is not strictly increasing".into());
        }
        self.counts.runtime_frames = self
            .counts
            .runtime_frames
            .checked_add(1)
            .ok_or("runtime frame count overflow")?;
        if self.counts.runtime_frames > MAX_RUNTIME_FRAMES {
            return Err("runtime frame cap exceeded".into());
        }
        let batch = ReceiptBatch {
            receipt_ns: tick,
            source_clock_regressions: u64::from(source_clock_regressions),
            events,
        };
        crate::session::summarize_input_batch(&batch)
            .map_err(|error| format!("runtime input accounting: {error:?}"))?;
        let frame = regional_step(&mut self.runtime, &batch)
            .map_err(|error| format!("regional runtime refused frame: {error}"))?;
        self.copy_projection(&frame)?;
        self.last_tick_ns = Some(tick);
        Ok(())
    }

    fn copy_projection(&mut self, frame: &RegionalFrame) -> Result<(), String> {
        self.latest_tracks.clear();
        self.latest_tracks
            .extend_from_slice(&frame.surveillance.tracks);
        if self.latest_tracks.len() > 4096 {
            return Err("runtime output exceeded track projection cap".into());
        }
        self.latest_flights.clear();
        for FlightEvidence { view, .. } in &frame.flights {
            if let Some(view) = view {
                if self.latest_flights.len() >= 8192 {
                    return Err("runtime output exceeded flight projection cap".into());
                }
                self.latest_flights.push(*view);
            }
        }
        self.counts.candidate_flight_rows_seen = self
            .counts
            .candidate_flight_rows_seen
            .checked_add(
                u64::try_from(self.latest_flights.len())
                    .map_err(|_| "flight row count overflow")?,
            )
            .ok_or("flight row count overflow")?;
        let alerts = frame
            .surveillance
            .ca_open
            .len()
            .checked_add(frame.surveillance.ca_closed.len())
            .and_then(|n| n.checked_add(frame.surveillance.ca_altitude_lost.len()))
            .ok_or("alert row count overflow")?;
        self.counts.candidate_alert_rows_seen = self
            .counts
            .candidate_alert_rows_seen
            .checked_add(u64::try_from(alerts).map_err(|_| "alert row count overflow")?)
            .ok_or("alert row count overflow")?;
        Ok(())
    }

    fn compare_sfdps_frame(&mut self, frame: &RecordedFrame) -> Result<(), String> {
        oa_fixm::clear_events(&mut self.fixm_events);
        let document = self
            .xml_parser
            .parse(&frame.payload)
            .map_err(|error| format!("FIXM source projection parse refusal: {error:?}"))?;
        let summary = self
            .fixm_decoder
            .decode_prefix(&document, &frame.payload, &mut self.fixm_events)
            .map_err(|error| format!("FIXM source projection decode refusal: {error:?}"))?;
        let emitted_rows =
            u32::try_from(self.fixm_events.len).map_err(|_| "FIXM emitted row count overflow")?;
        let refusal_count = match fixm_summary_refusal_count(&summary, emitted_rows) {
            Ok(count) => count,
            Err(error) => {
                self.counts.source_projection_refusals = self
                    .counts
                    .source_projection_refusals
                    .checked_add(1)
                    .ok_or("source projection refusal count overflow")?;
                return Err(format!(
                    "SFDPS source projection denominator invalid: {error}"
                ));
            }
        };
        if refusal_count > 0 {
            self.counts.source_projection_refusals = self
                .counts
                .source_projection_refusals
                .checked_add(refusal_count)
                .ok_or("source projection refusal count overflow")?;
            return Err(format!(
                "SFDPS source projection refused {refusal_count} messages (rows={}, rejected={}, not_decoded={}, sealed_window_dropped={}); no complete report written",
                summary.rows, summary.rejected, summary.not_decoded, summary.sealed_window_dropped
            ));
        }
        let observed_ms = frame.receipt_ns.div_euclid(1_000_000);
        for row in 0..self.fixm_events.len {
            self.sfdps_rows = self
                .sfdps_rows
                .checked_add(1)
                .ok_or("SFDPS row count overflow")?;
            if self.sfdps_rows > MAX_SFDPS_ROWS {
                return Err("SFDPS row cap exceeded".into());
            }
            let identity = RowIdentity {
                stream: u32::from(frame.stream_index),
                decoded_offset: frame.byte_offset,
                recorder_sequence: frame.sequence,
                row,
            };
            let facts = row_facts(&self.fixm_events, row, identity, observed_ms)
                .map_err(|error| format!("SFDPS row facts incomplete: {error}"))?;
            let mut context = ComparisonContext {
                counts: &mut self.counts,
                amendments: &mut self.amendments,
                g3_model: &self.g3_model,
                tracks: &self.latest_tracks,
                flights: &self.latest_flights,
                availability: &self.availability,
            };
            Self::compare_one_row(
                &mut context,
                &facts,
                &self.fixm_events,
                row,
                frame.receipt_ns,
            )?;
        }
        oa_fixm::clear_events(&mut self.fixm_events);
        Ok(())
    }

    fn compare_one_row(
        context: &mut ComparisonContext<'_>,
        facts: &SfdpsRowFacts<'_>,
        rows: &oa_fixm::Events,
        row: usize,
        receive_ns: i64,
    ) -> Result<(), String> {
        let address = rows.address.get(row).copied().unwrap_or(0);
        let exact_view = context.flights.iter().find(|view| view.gufi == facts.gufi);
        let address_view = (address != 0)
            .then(|| context.flights.iter().find(|view| view.address == address))
            .flatten();
        let mapped_view = exact_view.or(address_view);
        let candidate_gufi = mapped_view.map_or(0, |view| view.gufi);
        let candidate_address = mapped_view.map_or(0, |view| view.address);
        let candidate_track = if candidate_address == 0 {
            None
        } else {
            context
                .tracks
                .iter()
                .find(|track| track.address == candidate_address)
        };
        let candidate_position = candidate_track.map(|track| {
            let availability = context
                .availability
                .binary_search_by_key(&track.address, |entry| entry.address)
                .ok()
                .and_then(|index| context.availability.get(index))
                .filter(|entry| entry.source_ns == track.newest_source_ns)
                .map(|entry| entry.receive_ns);
            CandidatePosition {
                address: track.address,
                state_ns: track.state_ns,
                newest_source_ns: track.newest_source_ns,
                newest_available_ns: availability,
                lat_e7: track.lat_e7,
                lon_e7: track.lon_e7,
                altitude_ft: (track.altitude_ft != oa_track::NONE_I32).then_some(track.altitude_ft),
            }
        });
        let receive_ms = facts.observed_time_ms;
        let source = SourceReference {
            gufi: facts.gufi,
            source_time_ms: facts.source_time_ms,
            receive_time_ms: receive_ms,
            published_owner: facts.controlling_unit.value,
        };
        let result = migration_g3::compare_row_asof_at_receive_ns(
            &source,
            receive_ns,
            candidate_gufi,
            candidate_address,
            candidate_position.as_ref(),
            context.g3_model,
        );
        context
            .counts
            .record_g3(&result)
            .map_err(|error| format!("G3 disposition count: {error:?}"))?;

        let snapshot_comparison = if let Some(view) = exact_view.or(address_view) {
            let comparison = migration_snapshot::compare_published_view(facts, view)
                .map_err(|error| format!("flight-state comparison: {error:?}"))?;
            comparison
        } else if facts.gufi == 0 {
            let empty = oa_flight::FLIGHT_FREE;
            migration_snapshot::compare_published_row(facts, &empty)
                .map_err(|error| format!("missing-GUFI comparison: {error:?}"))?
        } else {
            migration_snapshot::compare_published_unavailable(facts)
                .map_err(|error| format!("candidate-unavailable comparison: {error:?}"))?
        };
        context
            .counts
            .record_snapshot(&snapshot_comparison)
            .map_err(|error| format!("flight-state count: {error:?}"))?;
        amendment_observe(
            context.amendments,
            &snapshot_comparison,
            facts.gufi,
            facts.source_time_ms,
            facts.observed_time_ms,
            facts.route,
        )
        .map_err(|error| format!("amendment propagation count: {error:?}"))?;
        context.counts.amendment_propagation = amendment_summary(context.amendments);
        context.counts.unobservable_amendment_decision_rows = context
            .counts
            .unobservable_amendment_decision_rows
            .checked_add(1)
            .ok_or("amendment denominator overflow")?;
        if facts.handoff_event.state
            == oa_flight_sfdps::migration_projection::SourceFieldState::Present
        {
            context.counts.unobservable_handoff_rows = context
                .counts
                .unobservable_handoff_rows
                .checked_add(1)
                .ok_or("handoff denominator overflow")?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<FullDayCounts, String> {
        if self
            .pending_receipt_ns
            .is_some_and(|receipt| receipt < DAY_END_NS)
        {
            let receipt = self.pending_receipt_ns.ok_or("pending receipt invariant")?;
            self.advance_until(receipt)?;
        }
        if !self.pending.is_empty() {
            return Err("normalized events at the closing boundary could not be applied within the half-open day".into());
        }
        self.advance_until(DAY_END_NS - DAY_TICK_NS)?;
        if self.counts.input_reject_frames != 0
            || self.counts.adaptation_quarantine_rows != 0
            || self.counts.adapter_unprocessed_frames != 0
            || self.counts.source_projection_refusals != 0
        {
            return Err("input or adapter refusals prevent a complete-day evidence result".into());
        }
        if self.counts.runtime_frames != MAX_RUNTIME_FRAMES {
            return Err("runtime did not produce the complete 24-hour frame partition".into());
        }
        if self.counts.source_sfdps_rows == 0 {
            return Err("full-day SFDPS partition produced no decoded source rows".into());
        }
        if !self.counts.reconciles() {
            return Err(
                "source and candidate comparison denominators failed reconciliation".into(),
            );
        }
        Ok(self.counts)
    }
}

/// Run the fixed 24-hour partition from guarded, ordered OAF segments. The current receive-time
/// group is bounded to 4096 messages/4 MiB; no other input history is retained.
pub fn run_full_day(
    inputs: &[crate::recordings_stream::StreamInput],
    runner: FullDayRunner,
) -> Result<(FullDayCounts, crate::recordings_stream::StreamSummary), String> {
    let mut runner = runner;
    let mut group = Vec::new();
    group
        .try_reserve_exact(64)
        .map_err(|_| "receive group allocation refused")?;
    let mut group_receipt = None;
    let mut group_bytes = 0usize;
    let summary = crate::recordings_stream::stream_oaf_segments_in_window(
        inputs,
        Some((DAY_START_NS, DAY_END_NS)),
        |frame| {
            if group_receipt.is_some_and(|prior| frame.receipt_ns != prior) {
                runner.process_receive_group(&group)?;
                group.clear();
                group_bytes = 0;
                group_receipt = None;
            }
            if group.len() >= MAX_RECEIVE_GROUP_FRAMES {
                return Err("same-receive input group exceeds frame cap".into());
            }
            group_bytes = group_bytes
                .checked_add(frame.payload.len())
                .ok_or("receive group byte count overflow")?;
            if group_bytes > MAX_RECEIVE_GROUP_BYTES {
                return Err("same-receive input group exceeds payload cap".into());
            }
            group_receipt = Some(frame.receipt_ns);
            group
                .try_reserve(1)
                .map_err(|_| "receive group allocation refused")?;
            group.push(frame);
            Ok(())
        },
    )?;
    if !group.is_empty() {
        runner.process_receive_group(&group)?;
    }
    runner.record_stream_summary(&summary)?;
    let counts = runner.finish()?;
    Ok((counts, summary))
}

#[cfg(test)]
mod fixm_summary_tests {
    use super::fixm_summary_refusal_count;

    // REQ: OA-MIG-411
    #[test]
    fn rejected_and_unemitted_fixm_messages_cannot_disappear_from_denominator() {
        let rejected = oa_fixm::Summary {
            messages: 4,
            rows: 3,
            rejected: 1,
            ..oa_fixm::Summary::default()
        };
        assert_eq!(fixm_summary_refusal_count(&rejected, 3), Ok(1));

        let capacity = oa_fixm::Summary {
            messages: 4,
            rows: 3,
            rejected: 1,
            not_decoded: 2,
            ..oa_fixm::Summary::default()
        };
        assert_eq!(fixm_summary_refusal_count(&capacity, 3), Ok(3));
        assert!(fixm_summary_refusal_count(&rejected, 2).is_err());

        let inconsistent = oa_fixm::Summary {
            messages: 4,
            rows: 2,
            rejected: 1,
            ..oa_fixm::Summary::default()
        };
        assert!(fixm_summary_refusal_count(&inconsistent, 2).is_err());
    }
}

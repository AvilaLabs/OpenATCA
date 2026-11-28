//! Thin shell adapter facade for root CLI dispatch. It opens sealed-guarded files, validates OAF
//! frames, decodes source payloads, and produces the ordinary OA-RS v1 normalized input text.

use crate::recordings::{
    adapt_recorded_frames, serialize_provenance, serialize_quarantine, serialize_recorded_events,
    AdapterError, RecordedAdaptation,
};
use crate::recordings_reader::{
    read_oaf_inputs, RecordingFrames, RecordingInput, RecordingReadError, RecordingSource,
    RecordingStreamProvenance, MAX_RECORDING_FRAMES,
};
use crate::recordings_stream::{
    stream_normalized_events, stream_recorded_hour_normalized_events, NormalizedStreamSummary,
    StreamInput,
};

#[derive(Debug)]
pub enum RecordingCliError {
    Read(RecordingReadError),
    Adapt(AdapterError),
    Stream(String),
}

// A7.2: cold shell output for one offline recording conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingCliOutput {
    pub normalized_tsv: String,
    pub provenance_tsv: String,
    pub quarantine_tsv: String,
    pub adaptation: RecordedAdaptation,
    pub frame_count: u64,
    pub streams: Vec<RecordingStreamProvenance>,
}

// A7.2: cold shell result for a bounded receive-window conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowRecordingCliOutput {
    pub normalized_tsv: String,
    pub provenance_tsv: String,
    pub summary: NormalizedStreamSummary,
    pub window_ns: (i64, i64),
    pub flight_centre_scope: Option<[u8; 4]>,
    pub excluded_other_centre_events: u64,
    pub selected_normalized_events: u64,
}

pub const MAX_WINDOW_OUTPUT_BYTES: usize = 256 * 1024 * 1024;
/// Explicit hourly normalization and provenance envelope for the dense public recording profile.
pub const MAX_RECORDED_HOUR_BYTES: usize = 768 * 1024 * 1024;
const MAX_NORMALIZED_ROW_RESERVATION: usize = crate::input::MAX_EVENT_LINE_BYTES;
const MAX_PROVENANCE_ROW_RESERVATION: usize = 512;
const NS_PER_HOUR: i64 = 3_600_000_000_000;
pub const RECORDED_HOUR_MAX_WARMUP_NS: i64 = 600_000_000_000;
/// Recorded-hour importer span including the explicit ten-minute replay warmup allowance.
pub const MAX_RECORDED_HOUR_WINDOW_NS: i64 = NS_PER_HOUR + RECORDED_HOUR_MAX_WARMUP_NS;

pub fn validate_recorded_hour_window(start_ns: i64, end_ns: i64) -> Result<(i64, i64), String> {
    validate_window_span(start_ns, end_ns, MAX_RECORDED_HOUR_WINDOW_NS)
}

fn validate_window_span(
    start_ns: i64,
    end_ns: i64,
    max_span_ns: i64,
) -> Result<(i64, i64), String> {
    let span = end_ns
        .checked_sub(start_ns)
        .ok_or_else(|| "receive window span overflow".to_owned())?;
    if start_ns < 0 || span <= 0 || span > max_span_ns {
        return Err(
            "receive window must be nonnegative, positive, and within its profile cap".into(),
        );
    }
    Ok((start_ns, end_ns))
}

fn reserve_window_output(
    output: &mut String,
    additional: usize,
    label: &str,
    byte_limit: usize,
) -> Result<(), String> {
    let next = output
        .len()
        .checked_add(additional)
        .ok_or_else(|| format!("{label} byte count overflow"))?;
    if next > byte_limit {
        return Err(format!(
            "{label} exceeds the {} MiB window-output cap",
            byte_limit / (1024 * 1024)
        ));
    }
    output
        .try_reserve(additional)
        .map_err(|_| format!("{label} allocation refused"))
}

fn provenance_stream_prefix(summary: &NormalizedStreamSummary) -> Result<String, String> {
    use core::fmt::Write;
    let mut prefix = String::from("OA-RS-PROVENANCE\t1\n");
    for stream in &summary.recording.streams {
        let source = match stream.source {
            RecordingSource::Adsb => "ADSB",
            RecordingSource::Sfdps => "SFDPS",
        };
        writeln!(
            prefix,
            "STREAM\t{}\t{}\t{}\t{}\t{}\t{}",
            stream.stream_index,
            source,
            oa_sha256::to_hex(&stream.sha256),
            stream.bytes,
            stream.expanded_bytes,
            u8::from(stream.gzip)
        )
        .map_err(|_| "stream provenance formatting failed")?;
    }
    Ok(prefix)
}

struct ScopedWindowWriter<'a> {
    normalized_tsv: &'a mut String,
    provenance_rows: &'a mut String,
    flight_centre_scope: Option<[u8; 4]>,
    excluded_other_centre_events: u64,
    event_index: usize,
    output_byte_limit: usize,
}

fn flight_event_is_out_of_scope(
    event: &crate::input::NormalizedEvent,
    scope: Option<[u8; 4]>,
) -> bool {
    scope.is_some_and(|centre| {
        event.source == crate::input::SourceKind::Flight && event.centre != centre
    })
}

impl ScopedWindowWriter<'_> {
    fn visit(
        &mut self,
        event: crate::input::NormalizedEvent,
        origin: crate::recordings::EventProvenance,
    ) -> Result<(), String> {
        if flight_event_is_out_of_scope(&event, self.flight_centre_scope) {
            self.excluded_other_centre_events = self
                .excluded_other_centre_events
                .checked_add(1)
                .ok_or_else(|| "excluded-centre event count overflow".to_owned())?;
            return Ok(());
        }
        reserve_window_output(
            self.normalized_tsv,
            MAX_NORMALIZED_ROW_RESERVATION,
            "normalized TSV",
            self.output_byte_limit,
        )?;
        crate::recordings::append_recorded_event_row(self.normalized_tsv, &event)
            .map_err(|error| format!("normalized event serialization refused: {error:?}"))?;
        reserve_window_output(
            self.provenance_rows,
            MAX_PROVENANCE_ROW_RESERVATION,
            "provenance TSV",
            self.output_byte_limit,
        )?;
        crate::recordings::append_provenance_event_row(
            self.provenance_rows,
            self.event_index,
            &event,
            origin,
        )
        .map_err(|error| format!("provenance serialization refused: {error:?}"))?;
        self.event_index = self
            .event_index
            .checked_add(1)
            .ok_or_else(|| "provenance event index overflow".to_owned())?;
        Ok(())
    }
}

/// Converts an explicit half-open receive interval using the guarded streaming reader. Only
/// normalized rows and provenance rows are retained; source frames and per-frame quarantine
/// locators are not materialized.
pub fn convert_recordings_window(
    inputs: &[RecordingInput],
    adsb_centre: [u8; 4],
    window_ns: (i64, i64),
) -> Result<WindowRecordingCliOutput, RecordingCliError> {
    convert_recordings_window_with_streamer(
        inputs,
        adsb_centre,
        window_ns,
        WindowStreamProfile::Legacy,
        None,
    )
}

/// Converts an explicit offline recorded-hour window using its separately bounded 8,192-event
/// issue-time group profile. Legacy conversion remains capped at 256.
pub fn convert_recorded_hour_window(
    inputs: &[RecordingInput],
    adsb_centre: [u8; 4],
    window_ns: (i64, i64),
) -> Result<WindowRecordingCliOutput, RecordingCliError> {
    convert_recordings_window_with_streamer(
        inputs,
        adsb_centre,
        window_ns,
        WindowStreamProfile::RecordedHour,
        None,
    )
}

/// Converts an hour window and optionally retains only Flight events from one exact native centre.
/// This is a centre-local replay scope; it does not model interfacility handoffs.
pub fn convert_recorded_hour_window_scoped(
    inputs: &[RecordingInput],
    adsb_centre: [u8; 4],
    window_ns: (i64, i64),
    flight_centre_scope: Option<[u8; 4]>,
) -> Result<WindowRecordingCliOutput, RecordingCliError> {
    convert_recordings_window_with_streamer(
        inputs,
        adsb_centre,
        window_ns,
        WindowStreamProfile::RecordedHour,
        flight_centre_scope,
    )
}

#[derive(Clone, Copy)]
enum WindowStreamProfile {
    Legacy,
    RecordedHour,
}

fn convert_recordings_window_with_streamer(
    inputs: &[RecordingInput],
    adsb_centre: [u8; 4],
    window_ns: (i64, i64),
    profile: WindowStreamProfile,
    flight_centre_scope: Option<[u8; 4]>,
) -> Result<WindowRecordingCliOutput, RecordingCliError> {
    match profile {
        WindowStreamProfile::Legacy => validate_window_span(window_ns.0, window_ns.1, NS_PER_HOUR),
        WindowStreamProfile::RecordedHour => {
            validate_recorded_hour_window(window_ns.0, window_ns.1)
        }
    }
    .map_err(RecordingCliError::Stream)?;
    if flight_centre_scope.is_some_and(|centre| !crate::input::is_valid_centre_code(&centre)) {
        return Err(RecordingCliError::Stream(
            "flight centre scope must be a padded native ARTCC code".into(),
        ));
    }
    let mut streams = Vec::new();
    streams
        .try_reserve_exact(inputs.len())
        .map_err(|_| RecordingCliError::Stream("stream input allocation refused".into()))?;
    for input in inputs {
        streams.push(StreamInput {
            source: input.source,
            path: input.path.clone(),
        });
    }
    let mut normalized_tsv = String::from("OA-RS\t1\n");
    let mut provenance_rows = String::new();
    let output_byte_limit = match profile {
        WindowStreamProfile::Legacy => MAX_WINDOW_OUTPUT_BYTES,
        WindowStreamProfile::RecordedHour => MAX_RECORDED_HOUR_BYTES,
    };
    let (summary, excluded_other_centre_events, selected_normalized_events) = {
        let mut writer = ScopedWindowWriter {
            normalized_tsv: &mut normalized_tsv,
            provenance_rows: &mut provenance_rows,
            flight_centre_scope,
            excluded_other_centre_events: 0,
            event_index: 0,
            output_byte_limit,
        };
        let summary = match profile {
            WindowStreamProfile::Legacy => stream_normalized_events(
                &streams,
                Some(window_ns),
                adsb_centre,
                &mut |event, origin| writer.visit(event, origin),
            ),
            WindowStreamProfile::RecordedHour => stream_recorded_hour_normalized_events(
                &streams,
                Some(window_ns),
                adsb_centre,
                &mut |event, origin| writer.visit(event, origin),
            ),
        }
        .map_err(RecordingCliError::Stream)?;
        let excluded = writer.excluded_other_centre_events;
        let selected = u64::try_from(writer.event_index)
            .map_err(|_| RecordingCliError::Stream("selected event count overflow".into()))?;
        let accounted = selected
            .checked_add(excluded)
            .ok_or_else(|| RecordingCliError::Stream("centre-scope event count overflow".into()))?;
        if accounted != summary.normalized_events {
            return Err(RecordingCliError::Stream(
                "centre scope did not account for all normalized events".into(),
            ));
        }
        (summary, excluded, selected)
    };
    let prefix = provenance_stream_prefix(&summary).map_err(RecordingCliError::Stream)?;
    reserve_window_output(
        &mut provenance_rows,
        prefix.len(),
        "provenance TSV",
        output_byte_limit,
    )
    .map_err(RecordingCliError::Stream)?;
    provenance_rows.insert_str(0, &prefix);
    Ok(WindowRecordingCliOutput {
        normalized_tsv,
        provenance_tsv: provenance_rows,
        summary,
        window_ns,
        flight_centre_scope,
        excluded_other_centre_events,
        selected_normalized_events,
    })
}

/// Runs the actual OAF→decoder→regional normalized-input path. Root owns argument dispatch and
/// destination creation; this function remains deterministic for an identical set of source files.
pub fn convert_recordings(
    inputs: &[RecordingInput],
    adsb_centre: [u8; 4],
) -> Result<RecordingCliOutput, RecordingCliError> {
    let RecordingFrames {
        frames,
        counts,
        streams,
    } = read_oaf_inputs(inputs).map_err(RecordingCliError::Read)?;
    let frame_count = counts.total_frames;
    let adaptation = adapt_recorded_frames(&frames, counts, MAX_RECORDING_FRAMES, adsb_centre)
        .map_err(RecordingCliError::Adapt)?;
    let normalized_tsv =
        serialize_recorded_events(&adaptation.events).map_err(RecordingCliError::Adapt)?;
    let provenance_tsv = serialize_provenance(&adaptation.events, &adaptation.provenance, &streams)
        .map_err(RecordingCliError::Adapt)?;
    let quarantine_tsv =
        serialize_quarantine(&adaptation.quarantine).map_err(RecordingCliError::Adapt)?;
    Ok(RecordingCliOutput {
        normalized_tsv,
        provenance_tsv,
        quarantine_tsv,
        adaptation,
        frame_count,
        streams,
    })
}

#[cfg(test)]
mod recorded_hour_window_tests {
    use super::*;

    #[test]
    fn centre_scope_filters_only_other_flight_events_and_keeps_provenance() -> Result<(), String> {
        // Kills filtering ADS-B by the flight-centre option or rewriting accepted event provenance.
        use crate::input::{EventKind, FlightOperation, Identity, NormalizedEvent, SourceKind};
        use crate::recordings::EventProvenance;
        use crate::recordings_reader::RecordingSource;

        let make_event = |centre| NormalizedEvent {
            receipt_ns: 2_000_000_000,
            source_ns: 1_000_000_000,
            sequence: 4,
            source_message_id: Some(3),
            source: SourceKind::Flight,
            identity: Identity {
                flight_gufi: 7,
                aircraft_address: 0,
            },
            centre,
            kind: EventKind::Flight(FlightOperation::Depart),
        };
        let origin = EventProvenance {
            source: RecordingSource::Sfdps,
            stream_index: 2,
            frame_sequence: 9,
            input_receipt_ns: 2_000_000_000,
            jms_timestamp_ms: 2_000,
            mono_ns: None,
            oaf_flags: 0,
            frame_byte_offset: 123,
            skipped_before: 0,
            emitted_item_ordinal: 1,
            native_flags: 0,
            native_source: *b"TH",
        };
        let zau = make_event(*b"ZAU\0");
        let zxx = make_event(*b"ZXX\0");
        let adsb = NormalizedEvent {
            receipt_ns: 2_000_000_000,
            source_ns: 1_000_000_000,
            sequence: 5,
            source_message_id: None,
            source: SourceKind::Surveillance,
            identity: Identity {
                flight_gufi: 0,
                aircraft_address: 0x00ab_cdef,
            },
            centre: *b"ZXX\0",
            kind: EventKind::Position {
                latitude_e7: 410_000_000,
                longitude_e7: -870_000_000,
                altitude_ft: Some(10_000),
                east_mmps: 120_000,
                north_mmps: 50_000,
                vertical_rate_fpm: None,
            },
        };
        let adsb_origin = EventProvenance {
            source: RecordingSource::Adsb,
            ..origin
        };
        let mut normalized = String::from("OA-RS\t1\n");
        let mut provenance = String::new();
        let (excluded, selected) = {
            let mut scoped = ScopedWindowWriter {
                normalized_tsv: &mut normalized,
                provenance_rows: &mut provenance,
                flight_centre_scope: Some(*b"ZAU\0"),
                excluded_other_centre_events: 0,
                event_index: 0,
                output_byte_limit: MAX_RECORDED_HOUR_BYTES,
            };
            scoped.visit(zau, origin)?;
            scoped.visit(zxx, origin)?;
            scoped.visit(adsb, adsb_origin)?;
            (scoped.excluded_other_centre_events, scoped.event_index)
        };
        assert_eq!(excluded, 1);
        assert_eq!(selected, 2);
        let mut normalized_rows = normalized.lines();
        if normalized_rows.next().is_none() {
            return Err("normalized header missing".into());
        }
        let selected_events = normalized_rows
            .map(|row| {
                crate::input::parse_event_line(row)
                    .map_err(|error| format!("selected row parse failed: {error:?}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(selected_events, vec![zau, adsb]);
        let mut provenance_rows = provenance.lines();
        let flight_origin_row = provenance_rows
            .next()
            .ok_or_else(|| "selected flight provenance row missing".to_owned())?;
        if provenance_rows.next().is_none() {
            return Err("selected ADS-B provenance row missing".into());
        }
        assert!(provenance_rows.next().is_none());
        let fields = flight_origin_row.split('\t').collect::<Vec<_>>();
        assert_eq!(fields.get(2).copied(), Some("FLIGHT"));
        assert_eq!(
            fields.get(3).and_then(|value| value.parse::<u64>().ok()),
            Some(zau.sequence)
        );
        assert_eq!(
            fields.get(5).and_then(|value| value.parse::<u8>().ok()),
            Some(origin.stream_index)
        );
        assert_eq!(
            fields.get(6).and_then(|value| value.parse::<u64>().ok()),
            Some(origin.frame_sequence)
        );
        assert_eq!(
            fields.get(7).and_then(|value| value.parse::<i64>().ok()),
            Some(origin.input_receipt_ns)
        );
        assert_eq!(
            fields.get(8).and_then(|value| value.parse::<u64>().ok()),
            Some(origin.frame_byte_offset)
        );

        let mut all_centres_normalized = String::from("OA-RS\t1\n");
        let mut all_centres_provenance = String::new();
        let (all_excluded, all_selected) = {
            let mut all_centres = ScopedWindowWriter {
                normalized_tsv: &mut all_centres_normalized,
                provenance_rows: &mut all_centres_provenance,
                flight_centre_scope: None,
                excluded_other_centre_events: 0,
                event_index: 0,
                output_byte_limit: MAX_RECORDED_HOUR_BYTES,
            };
            all_centres.visit(zau, origin)?;
            all_centres.visit(zxx, origin)?;
            all_centres.visit(adsb, adsb_origin)?;
            (
                all_centres.excluded_other_centre_events,
                all_centres.event_index,
            )
        };
        assert_eq!(all_excluded, 0);
        assert_eq!(all_selected, 3);
        let mut all_centres_rows = all_centres_normalized.lines();
        if all_centres_rows.next().is_none() {
            return Err("all-centres normalized header missing".into());
        }
        let all_events = all_centres_rows
            .map(|row| {
                crate::input::parse_event_line(row)
                    .map_err(|error| format!("all-centres row parse failed: {error:?}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(all_events, vec![zau, zxx, adsb]);
        Ok(())
    }

    #[test]
    fn public_hour_scope_refuses_invalid_centre_code() {
        let result = convert_recorded_hour_window_scoped(&[], *b"ZAU1", (1, 2), Some(*b"Z\0AU"));
        assert!(matches!(result, Err(RecordingCliError::Stream(_))));
    }

    #[test]
    fn recorded_hour_import_accepts_exact_warmup_span_and_refuses_overrun_or_inversion() {
        let start_ns = 1_700_000_000_000_000_000i64;
        let display_span_ns = NS_PER_HOUR;
        let max_span_ns = MAX_RECORDED_HOUR_WINDOW_NS;
        let exact_end_ns = start_ns + max_span_ns;
        assert!(validate_recorded_hour_window(start_ns, exact_end_ns).is_ok());
        assert!(validate_recorded_hour_window(start_ns, exact_end_ns + 1).is_err());
        assert!(validate_recorded_hour_window(exact_end_ns, start_ns).is_err());
        assert!(validate_recorded_hour_window(start_ns, start_ns).is_err());

        let legacy_end_ns = start_ns + display_span_ns;
        assert!(validate_window_span(start_ns, legacy_end_ns, NS_PER_HOUR).is_ok());
        assert!(validate_window_span(start_ns, exact_end_ns, NS_PER_HOUR).is_err());
    }
}

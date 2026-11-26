//! Bounded aggregation for the preregistered M4/H2 development replay.
//!
//! This module accepts one comparison result at a time. It never retains source rows, flight
//! identities, candidate frames, or alert records. A caller must feed every eligible SFDPS row
//! into exactly one G3 disposition and each independently comparable published-state row into
//! the field buckets below; unsupported event families stay explicitly unobservable.

use crate::migration_g3::{ModelError, OutcomeCounts, RowResult};
use crate::migration_snapshot::{FieldOutcome, RowComparison, RowIdentityState};

pub const PROFILE_ID: &str = "M4-H2-DEV-COMPARISON-V3-CAUSAL-ASOF";
pub const MAX_REPORT_BYTES: usize = 64 * 1024;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FieldCounts {
    pub equivalent: u64,
    pub different_unresolved: u64,
    pub source_absent: u64,
    pub source_unknown: u64,
    pub source_unsupported: u64,
    pub candidate_unavailable: u64,
    pub candidate_not_yet_available: u64,
    pub candidate_not_modeled: u64,
    pub identity_mismatch: u64,
    pub total: u64,
}
const _: () = assert!(core::mem::size_of::<FieldCounts>() == 80);

impl FieldCounts {
    fn record(&mut self, outcome: FieldOutcome) -> Result<(), ModelError> {
        self.total = self.total.checked_add(1).ok_or(ModelError::Capacity)?;
        let slot = match outcome {
            FieldOutcome::Equivalent => &mut self.equivalent,
            FieldOutcome::Different => &mut self.different_unresolved,
            FieldOutcome::SourceAbsent => &mut self.source_absent,
            FieldOutcome::SourceUnknown => &mut self.source_unknown,
            FieldOutcome::SourceUnsupported => &mut self.source_unsupported,
            FieldOutcome::CandidateUnavailable => &mut self.candidate_unavailable,
            FieldOutcome::CandidateNotYetAvailable => &mut self.candidate_not_yet_available,
            FieldOutcome::CandidateNotModeled => &mut self.candidate_not_modeled,
            FieldOutcome::IdentityMismatch => &mut self.identity_mismatch,
        };
        *slot = slot.checked_add(1).ok_or(ModelError::Capacity)?;
        Ok(())
    }

    pub fn reconciles(&self) -> bool {
        self.equivalent
            .checked_add(self.different_unresolved)
            .and_then(|n| n.checked_add(self.source_absent))
            .and_then(|n| n.checked_add(self.source_unknown))
            .and_then(|n| n.checked_add(self.source_unsupported))
            .and_then(|n| n.checked_add(self.candidate_unavailable))
            .and_then(|n| n.checked_add(self.candidate_not_yet_available))
            .and_then(|n| n.checked_add(self.candidate_not_modeled))
            .and_then(|n| n.checked_add(self.identity_mismatch))
            == Some(self.total)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotCounts {
    pub matched_gufi: u64,
    pub missing_gufi: u64,
    pub candidate_different_gufi: u64,
    pub invalid_clock: u64,
    pub candidate_unavailable: u64,
    pub total_rows: u64,
    pub fields: FieldCounts,
}
const _: () = assert!(core::mem::size_of::<SnapshotCounts>() == 128);

impl SnapshotCounts {
    pub fn record(&mut self, row: &RowComparison) -> Result<(), ModelError> {
        self.total_rows = self.total_rows.checked_add(1).ok_or(ModelError::Capacity)?;
        let slot = match row.identity_state {
            RowIdentityState::MatchedGufi => &mut self.matched_gufi,
            RowIdentityState::MissingGufi => &mut self.missing_gufi,
            RowIdentityState::CandidateDifferentGufi => &mut self.candidate_different_gufi,
            RowIdentityState::InvalidClock => &mut self.invalid_clock,
            RowIdentityState::CandidateUnavailable => &mut self.candidate_unavailable,
        };
        *slot = slot.checked_add(1).ok_or(ModelError::Capacity)?;
        for field in row.fields.iter().take(usize::from(row.field_count)) {
            self.fields.record(field.outcome)?;
        }
        Ok(())
    }

    /// Conserve a decoded source row for which no candidate flight table row exists. The row is
    /// identity matched at source, but candidate values are unavailable; each profile field is
    /// accounted rather than omitted from the denominator.
    pub fn record_candidate_unavailable(&mut self) -> Result<(), ModelError> {
        self.total_rows = self.total_rows.checked_add(1).ok_or(ModelError::Capacity)?;
        self.matched_gufi = self
            .matched_gufi
            .checked_add(1)
            .ok_or(ModelError::Capacity)?;
        const FIELD_COUNT: usize = crate::migration_snapshot::MAX_ROW_FIELDS;
        let next_total = self
            .fields
            .total
            .checked_add(FIELD_COUNT as u64)
            .ok_or(ModelError::Capacity)?;
        let next_unavailable = self
            .fields
            .candidate_unavailable
            .checked_add(FIELD_COUNT as u64)
            .ok_or(ModelError::Capacity)?;
        self.fields.total = next_total;
        self.fields.candidate_unavailable = next_unavailable;
        Ok(())
    }

    pub fn reconciles(&self) -> bool {
        self.matched_gufi
            .checked_add(self.missing_gufi)
            .and_then(|n| n.checked_add(self.candidate_different_gufi))
            .and_then(|n| n.checked_add(self.invalid_clock))
            .and_then(|n| n.checked_add(self.candidate_unavailable))
            == Some(self.total_rows)
            && self.fields.reconciles()
    }
}

/// Fixed-size aggregate state for one complete input partition.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FullDayCounts {
    pub source_sfdps_rows: u64,
    pub g3: OutcomeCounts,
    pub flight_state: SnapshotCounts,
    pub candidate_flight_rows_seen: u64,
    pub candidate_alert_rows_seen: u64,
    pub input_frames: u64,
    pub adapted_events: u64,
    pub runtime_frames: u64,
    pub unobservable_handoff_rows: u64,
    pub unobservable_amendment_rows: u64,
    pub unobservable_legacy_alerts: u64,
    pub unsupported_point_out_rows: u64,
    pub unsupported_interface_rows: u64,
    pub input_status_frames: u64,
    pub input_reject_frames: u64,
    pub adaptation_quarantine_rows: u64,
    pub adapter_unprocessed_frames: u64,
    pub source_projection_refusals: u64,
    pub source_clock_regressions: u64,
    pub spatial_positions_seen: u64,
    pub spatial_positions_inside: u64,
    pub spatial_positions_outside: u64,
}
const _: () = assert!(core::mem::size_of::<FullDayCounts>() == 424);

pub struct ReportMetadata<'a> {
    pub input_bytes: u64,
    pub input_sha256: &'a str,
    pub executable_sha256: &'a str,
    pub airspace_sha256: &'a str,
    pub g3_artifact_sha256: &'a str,
    pub policy_sha256: &'a str,
    pub streams: &'a [crate::recordings_stream::StreamDigest],
}

impl FullDayCounts {
    pub fn record_g3(&mut self, result: &RowResult) -> Result<(), ModelError> {
        self.source_sfdps_rows = self
            .source_sfdps_rows
            .checked_add(1)
            .ok_or(ModelError::Capacity)?;
        self.g3.record(result)
    }

    pub fn record_snapshot(&mut self, row: &RowComparison) -> Result<(), ModelError> {
        self.flight_state.record(row)
    }

    pub fn reconciles(&self) -> bool {
        self.g3.reconciles()
            && self.g3.total == self.source_sfdps_rows
            && self.flight_state.reconciles()
            && self.flight_state.total_rows == self.source_sfdps_rows
    }
}

/// Render only aggregate counts and explicit coverage states. The output is deterministic and
/// contains no source IDs, coordinates, callsigns, route text, or per-flight rows.
pub fn render_report(
    counts: &FullDayCounts,
    metadata: &ReportMetadata<'_>,
) -> Result<String, &'static str> {
    let ReportMetadata {
        input_bytes,
        input_sha256,
        executable_sha256,
        airspace_sha256,
        g3_artifact_sha256,
        policy_sha256,
        streams,
    } = metadata;
    if !counts.reconciles()
        || [
            input_sha256,
            executable_sha256,
            airspace_sha256,
            g3_artifact_sha256,
            policy_sha256,
        ]
        .iter()
        .any(|value| value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("aggregate or digest reconciliation failed");
    }
    let mut report = format!(
        "OA-M4-H2-FULL-DAY\t2\nprofile\t{PROFILE_ID}\nevaluation_start_ns\t{}\nevaluation_end_ns_exclusive\t{}\ninput_bytes\t{input_bytes}\ninput_sha256\t{input_sha256}\nexecutable_sha256\t{executable_sha256}\nairspace_sha256\t{airspace_sha256}\ng3_artifact_sha256\t{g3_artifact_sha256}\npolicy_sha256\t{policy_sha256}\ninput_frames\t{}\ninput_status_frames\t{}\ninput_reject_frames\t{}\nadapted_events\t{}\nadaptation_quarantine_rows\t{}\nadapter_unprocessed_frames\t{}\nsource_projection_refusals\t{}\nsource_clock_regressions\t{}\nspatial_filter\tOA_ADSB_ZAU_BOX_V1\nspatial_positions_seen\t{}\nspatial_positions_inside\t{}\nspatial_positions_outside\t{}\nruntime_frames\t{}\nsfdps_rows\t{}\n",
        crate::migration_full_day_runner::DAY_START_NS, crate::migration_full_day_runner::DAY_END_NS,
        counts.input_frames, counts.input_status_frames, counts.input_reject_frames,
        counts.adapted_events, counts.adaptation_quarantine_rows,
        counts.adapter_unprocessed_frames, counts.source_projection_refusals,
        counts.source_clock_regressions, counts.spatial_positions_seen,
        counts.spatial_positions_inside, counts.spatial_positions_outside,
        counts.runtime_frames, counts.source_sfdps_rows
    );
    let g3 = counts.g3;
    use std::fmt::Write as _;
    writeln!(
        report,
        "sector_g3_v2_asof\ttotal={}\tsupported_match={}\tsupported_mismatch={}\tcontested={}\tunknown_support={}\tno_training_cell={}\tcandidate_unavailable={}\tidentity_mismatch={}\tinvalid_time={}\tinvalid_position={}\tmissing_altitude={}\tunknown_source_owner={}\tinvalid_altitude={}\tcandidate_position_stale={}\tcandidate_state_future={}\tcandidate_state_stale={}\tcandidate_availability_unknown={}",
        g3.total, g3.supported_match, g3.supported_mismatch, g3.contested, g3.unknown_support,
        g3.no_training_cell, g3.candidate_unavailable, g3.identity_mismatch, g3.invalid_time,
        g3.invalid_position, g3.missing_altitude, g3.unknown_source_owner, g3.invalid_altitude,
        g3.candidate_position_stale, g3.candidate_state_future, g3.candidate_state_stale,
        g3.candidate_availability_unknown
    )
    .map_err(|_| "report formatting failed")?;
    for (index, stream) in streams.iter().enumerate() {
        let source = match stream.source {
            crate::recordings_reader::RecordingSource::Adsb => "ADSB",
            crate::recordings_reader::RecordingSource::Sfdps => "SFDPS",
        };
        writeln!(report, "input_stream\t{index}\t{source}\tbytes={}\tmessages={}\tfirst_frame_ns={}\tlast_frame_ns={}\tmax_gap_ns={}\tsha256={}",
            stream.bytes,
            stream.messages,
            stream.first_frame_receive_ns.map_or_else(|| "NONE".to_owned(), |value| value.to_string()),
            stream.last_frame_receive_ns.map_or_else(|| "NONE".to_owned(), |value| value.to_string()),
            stream.max_frame_gap_ns,
            oa_sha256::to_hex(&stream.sha256),
        )
            .map_err(|_| "report formatting failed")?;
    }
    let state = counts.flight_state;
    let f = state.fields;
    writeln!(
        report,
        "flight_data_state\trows={}\tmatched_gufi={}\tmissing_gufi={}\tcandidate_different_gufi={}\tinvalid_clock={}\tcandidate_unavailable_rows={}\tfields={}\tequivalent={}\tdifferent_unresolved={}\tsource_absent={}\tsource_unknown={}\tsource_unsupported={}\tcandidate_unavailable={}\tcandidate_not_yet_available={}\tcandidate_not_modeled={}\tidentity_mismatch={}",
        state.total_rows, state.matched_gufi, state.missing_gufi, state.candidate_different_gufi,
        state.invalid_clock, state.candidate_unavailable, f.total, f.equivalent, f.different_unresolved, f.source_absent,
        f.source_unknown, f.source_unsupported, f.candidate_unavailable,
        f.candidate_not_yet_available, f.candidate_not_modeled, f.identity_mismatch
    )
    .map_err(|_| "report formatting failed")?;
    for (family, coverage, rows) in [
        (
            "handoff",
            "UNOBSERVABLE_NO_INDEPENDENTLY_KEYED_CANDIDATE_EVENT_STREAM",
            counts.unobservable_handoff_rows,
        ),
        (
            "flight_data_amendment",
            "UNOBSERVABLE_SNAPSHOT_IS_NOT_AN_AMENDMENT_EVENT",
            counts.unobservable_amendment_rows,
        ),
        (
            "legacy_alert",
            "MISSING_NO_VALIDATED_LEGACY_ALERT_FEED",
            counts.unobservable_legacy_alerts,
        ),
        (
            "point_out",
            "UNSUPPORTED_NO_SOURCE_EVENT_OR_CANDIDATE_DECISION_STREAM",
            counts.unsupported_point_out_rows,
        ),
        (
            "interface_message",
            "UNSUPPORTED_NO_OBSERVABLE_ICD_BACKED_OUTPUT_STREAM",
            counts.unsupported_interface_rows,
        ),
    ] {
        writeln!(report, "coverage\t{family}\t{coverage}\tdenominator={rows}")
            .map_err(|_| "report formatting failed")?;
    }
    if report.len() > MAX_REPORT_BYTES {
        return Err("report exceeds fixed output cap");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::{FieldCounts, FullDayCounts, SnapshotCounts};
    use crate::migration_g3::{Disposition, RowResult};
    use crate::migration_snapshot::{
        CanonicalValue, ComparedField, FieldComparison, FieldOutcome, ReviewClass, RowComparison,
        RowIdentityState, MAX_ROW_FIELDS,
    };
    use oa_flight_sfdps::migration_projection::SourceFieldState;

    // REQ: OA-MIG-411
    #[test]
    fn counts_conserve_g3_and_snapshot_denominators() -> Result<(), String> {
        let mut counts = FullDayCounts::default();
        for disposition in [
            Disposition::SupportedMatch,
            Disposition::CandidateUnavailable,
        ] {
            counts
                .record_g3(&RowResult {
                    disposition,
                    gufi: 0,
                    address: 0,
                    state_ns: 0,
                    receive_ns: 0,
                    source_time_ns: 0,
                    lat_bin: 0,
                    lon_bin: 0,
                    altitude_band: 0,
                    source_owner: [0; 8],
                    predicted_owner: [0; 8],
                    support_rows: 0,
                    support_flights: 0,
                    support_time_bins: 0,
                })
                .map_err(|error| format!("{error:?}"))?;
        }
        let field = FieldComparison {
            field: ComparedField::SectorOwnerUnit,
            source_state: SourceFieldState::Present,
            outcome: FieldOutcome::Equivalent,
            review_class: ReviewClass::Equivalent,
            source_value: CanonicalValue::default(),
            candidate_value: CanonicalValue::default(),
            source_time_ns: 1,
            source_receive_ns: 1,
            candidate_source_ns: 1,
            candidate_model_available_ns: 1,
        };
        let comparison = RowComparison {
            source_row: oa_flight_sfdps::audit::RowIdentity {
                stream: 0,
                decoded_offset: 0,
                recorder_sequence: 0,
                row: 0,
            },
            source_gufi: 1,
            candidate_gufi: 1,
            identity_state: RowIdentityState::MatchedGufi,
            field_count: 1,
            fields: [field; MAX_ROW_FIELDS],
            source_fields_present: 1,
            source_fields_absent: 0,
            source_fields_malformed: 0,
            source_fields_unknown: 0,
            source_fields_unsupported: 0,
            exact_matches: 1,
            differences_unresolved: 0,
            candidate_unavailable: 0,
            candidate_not_yet_available: 0,
            candidate_not_modeled: 0,
        };
        counts
            .record_snapshot(&comparison)
            .map_err(|error| format!("{error:?}"))?;
        counts
            .record_snapshot(&comparison)
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(counts.source_sfdps_rows, 2);
        assert_eq!(counts.g3.total, 2);
        assert_eq!(counts.flight_state.total_rows, 2);
        assert_eq!(counts.flight_state.fields.equivalent, 2);
        assert!(counts.reconciles());
        Ok(())
    }

    // REQ: OA-MIG-411
    #[test]
    fn denominator_overflow_fails_instead_of_wrapping() {
        let mut fields = FieldCounts {
            total: u64::MAX,
            ..FieldCounts::default()
        };
        assert!(fields.record(FieldOutcome::Equivalent).is_err());
        let mut rows = SnapshotCounts {
            total_rows: u64::MAX,
            ..SnapshotCounts::default()
        };
        let invalid = RowComparison {
            source_row: oa_flight_sfdps::audit::RowIdentity {
                stream: 0,
                decoded_offset: 0,
                recorder_sequence: 0,
                row: 0,
            },
            source_gufi: 0,
            candidate_gufi: 0,
            identity_state: RowIdentityState::MissingGufi,
            field_count: 0,
            fields: [FieldComparison {
                field: ComparedField::SectorOwnerUnit,
                source_state: SourceFieldState::Absent,
                outcome: FieldOutcome::SourceAbsent,
                review_class: ReviewClass::NotComparable,
                source_value: CanonicalValue::default(),
                candidate_value: CanonicalValue::default(),
                source_time_ns: 0,
                source_receive_ns: 0,
                candidate_source_ns: 0,
                candidate_model_available_ns: 0,
            }; MAX_ROW_FIELDS],
            source_fields_present: 0,
            source_fields_absent: 0,
            source_fields_malformed: 0,
            source_fields_unknown: 0,
            source_fields_unsupported: 0,
            exact_matches: 0,
            differences_unresolved: 0,
            candidate_unavailable: 0,
            candidate_not_yet_available: 0,
            candidate_not_modeled: 0,
        };
        assert!(rows.record(&invalid).is_err());
    }

    // REQ: OA-MIG-411
    #[test]
    fn missing_candidate_counts_every_declared_snapshot_field() {
        let mut snapshot = SnapshotCounts::default();
        assert!(snapshot.record_candidate_unavailable().is_ok());
        assert_eq!(snapshot.total_rows, 1);
        assert_eq!(snapshot.matched_gufi, 1);
        assert_eq!(snapshot.fields.total, 11);
        assert_eq!(snapshot.fields.candidate_unavailable, 11);
        assert!(snapshot.reconciles());
    }
}

use crate::{
    AssessmentState, ExpectedProcessor, InventoryAssessment, InventoryQuery,
    MetricWindowAssessment, MetricWindowContract, PerformanceSample, ProcessorAssessment,
    ProcessorReport, StatusError, FLAG_CLOCK_DOMAIN_MISMATCH, FLAG_DISK_ADAPTATION_MISMATCH,
    FLAG_DUPLICATE_SEQUENCE, FLAG_EPOCH_MISMATCH, FLAG_EPOCH_ORDER_UNKNOWN, FLAG_EPOCH_REGRESSION,
    FLAG_FIRMWARE_MISMATCH, FLAG_FUTURE, FLAG_INVALID_TIME, FLAG_LOADED_ADAPTATION_MISMATCH,
    FLAG_METRIC_MISMATCH, FLAG_MISSING_PROCESSOR, FLAG_MISSING_SEQUENCE, FLAG_PROFILE_MISMATCH,
    FLAG_RELEASE_MANIFEST_MISMATCH, FLAG_RUNNING_IMAGE_MISMATCH, FLAG_SAMPLE_OUTSIDE_WINDOW,
    FLAG_SEQUENCE_OUTSIDE_WINDOW, FLAG_SEQUENCE_OUT_OF_ORDER, FLAG_SOURCE_MISMATCH, FLAG_STALE,
    FLAG_TELEMETRY_LOSS, FLAG_TIME_OUT_OF_ORDER, FLAG_UNEXPECTED_PROCESSOR, INCOMPLETE_FLAGS,
    MAX_EXPECTED_PROCESSORS, MAX_INVENTORY_REPORTS, MAX_PERFORMANCE_SAMPLES, MAX_SEQUENCE_SLOTS,
    SEQUENCE_WORDS,
};

fn checked_ref<T>(slice: &[T], index: usize) -> Result<&T, StatusError> {
    slice.get(index).ok_or(StatusError::CapacityInvariant)
}

fn checked_mut<T>(slice: &mut [T], index: usize) -> Result<&mut T, StatusError> {
    slice.get_mut(index).ok_or(StatusError::CapacityInvariant)
}

fn state_from_flags(flags: u32) -> AssessmentState {
    let incomplete = flags & INCOMPLETE_FLAGS != 0;
    let mismatch = flags & crate::MISMATCH_FLAGS != 0;
    match (incomplete, mismatch) {
        (true, true) => AssessmentState::IncompleteAndMismatch,
        (true, false) => AssessmentState::Incomplete,
        (false, true) => AssessmentState::Mismatch,
        (false, false) => AssessmentState::ClaimsMatchUnverified,
    }
}

fn increment(counter: &mut u32) -> Result<(), StatusError> {
    *counter = counter.checked_add(1).ok_or(StatusError::CounterOverflow)?;
    Ok(())
}

fn add_u64(counter: &mut u64, value: u64) -> Result<(), StatusError> {
    *counter = counter
        .checked_add(value)
        .ok_or(StatusError::CounterOverflow)?;
    Ok(())
}

fn seq_span(first: u64, next: u64) -> Result<u64, StatusError> {
    if next <= first {
        return Err(StatusError::InvalidSequenceWindow);
    }
    let span = next
        .checked_sub(first)
        .ok_or(StatusError::InvalidSequenceWindow)?;
    if span > MAX_SEQUENCE_SLOTS {
        return Err(StatusError::InvalidSequenceWindow);
    }
    Ok(span)
}

fn mark(row: &mut ProcessorAssessment, aggregate_flags: &mut u32, flag: u32) {
    row.flags |= flag;
    *aggregate_flags |= flag;
}

fn mark_metric(assessment: &mut MetricWindowAssessment, flag: u32) {
    assessment.flags |= flag;
}

/// Compare one complete expected processor inventory with its normalized event-ordered reports.
///
/// `reports` must preserve the source arrival order so sequence/time disorder remains observable.
/// The declared sequence intervals are independent of the observed rows; report minima/maxima are
/// never used to construct a denominator. Exact byte equality is only a comparison of claims.
pub fn assess_inventory(
    expected: &[ExpectedProcessor],
    reports: &[ProcessorReport],
    query: InventoryQuery,
) -> Result<InventoryAssessment, StatusError> {
    let InventoryQuery {
        previous_epoch,
        epoch,
        clock_domain_id,
        release_manifest_sha256,
        query_time_ns,
        freshness_limit_ns,
    } = query;
    if expected.is_empty() {
        return Err(StatusError::EmptyInventory);
    }
    if expected.len() > MAX_EXPECTED_PROCESSORS {
        return Err(StatusError::ExpectedInventoryOverCapacity);
    }
    if reports.len() > MAX_INVENTORY_REPORTS {
        return Err(StatusError::ReportRowsOverCapacity);
    }
    if query_time_ns < 0 {
        return Err(StatusError::InvalidQueryTime);
    }
    if freshness_limit_ns <= 0 {
        return Err(StatusError::InvalidFreshnessLimit);
    }

    let mut expected_sequences = 0u64;
    let mut spans = [0u64; MAX_EXPECTED_PROCESSORS];
    let mut i = 0usize;
    while i < expected.len() {
        let current = checked_ref(expected, i)?;
        let span = seq_span(current.sequence_first, current.sequence_next)?;
        *checked_mut(&mut spans, i)? = span;
        expected_sequences = expected_sequences
            .checked_add(span)
            .ok_or(StatusError::CounterOverflow)?;
        let mut j = 0usize;
        while j < i {
            if checked_ref(expected, i)?.processor_id == checked_ref(expected, j)?.processor_id {
                return Err(StatusError::DuplicateExpectedProcessor);
            }
            j += 1;
        }
        i += 1;
    }

    let mut processors = [crate::EMPTY_PROCESSOR_ASSESSMENT; MAX_EXPECTED_PROCESSORS];
    i = 0;
    while i < expected.len() {
        checked_mut(&mut processors, i)?.processor_id = checked_ref(expected, i)?.processor_id;
        i += 1;
    }

    // 64 KiB fixed bitmap. This is created only after the expected/report caps are checked.
    let mut seen = [[0u64; SEQUENCE_WORDS]; MAX_EXPECTED_PROCESSORS];
    let mut has_last_sequence = [false; MAX_EXPECTED_PROCESSORS];
    let mut last_sequence = [0u64; MAX_EXPECTED_PROCESSORS];
    let mut has_last_time = [false; MAX_EXPECTED_PROCESSORS];
    let mut last_time = [0i64; MAX_EXPECTED_PROCESSORS];
    let epoch_regressed = previous_epoch.is_some_and(|previous| epoch.0 < previous.0);
    let mut assessment = InventoryAssessment {
        previous_epoch,
        epoch,
        epoch_order_checked: previous_epoch.is_some(),
        epoch_regressed,
        clock_domain_id,
        query_time_ns,
        freshness_limit_ns,
        release_manifest_sha256,
        expected_processor_count: expected.len() as u32,
        report_rows: reports.len() as u32,
        unexpected_reports: 0,
        missing_processors: 0,
        expected_sequences,
        unique_sequences: 0,
        missing_sequences: 0,
        duplicate_sequences: 0,
        out_of_order_sequences: 0,
        out_of_order_times: 0,
        stale_rows: 0,
        future_rows: 0,
        invalid_time_rows: 0,
        source_mismatches: 0,
        epoch_mismatches: 0,
        clock_domain_mismatches: 0,
        release_manifest_mismatches: 0,
        firmware_mismatches: 0,
        running_image_mismatches: 0,
        loaded_adaptation_mismatches: 0,
        disk_adaptation_mismatches: 0,
        telemetry_loss_events: 0,
        sequences_outside_window: 0,
        flags: if previous_epoch.is_none() {
            FLAG_EPOCH_ORDER_UNKNOWN
        } else if epoch_regressed {
            FLAG_EPOCH_REGRESSION
        } else {
            0
        },
        state: AssessmentState::Incomplete,
        provenance_unverified: true,
        processor_result_count: expected.len() as u32,
        processors,
    };

    for report in reports {
        let mut found = None;
        i = 0;
        while i < expected.len() {
            if checked_ref(expected, i)?.processor_id == report.processor_id {
                found = Some(i);
                break;
            }
            i += 1;
        }
        let Some(index) = found else {
            increment(&mut assessment.unexpected_reports)?;
            assessment.flags |= FLAG_UNEXPECTED_PROCESSOR;
            continue;
        };

        let expected_row = *checked_ref(expected, index)?;
        let row = checked_mut(&mut assessment.processors, index)?;
        increment(&mut row.report_rows)?;
        if report.source_id != expected_row.source_id {
            increment(&mut row.source_mismatches)?;
            mark(row, &mut assessment.flags, FLAG_SOURCE_MISMATCH);
        }
        if report.epoch != epoch {
            increment(&mut row.epoch_mismatches)?;
            mark(row, &mut assessment.flags, FLAG_EPOCH_MISMATCH);
        }
        if report.clock_domain_id != clock_domain_id {
            increment(&mut row.clock_domain_mismatches)?;
            mark(row, &mut assessment.flags, FLAG_CLOCK_DOMAIN_MISMATCH);
        }
        if report.release_manifest_sha256 != release_manifest_sha256 {
            increment(&mut row.release_manifest_mismatches)?;
            mark(row, &mut assessment.flags, FLAG_RELEASE_MANIFEST_MISMATCH);
        }
        if report.firmware_sha256 != expected_row.firmware_sha256 {
            increment(&mut row.firmware_mismatches)?;
            mark(row, &mut assessment.flags, FLAG_FIRMWARE_MISMATCH);
        }
        if report.running_image_sha256 != expected_row.running_image_sha256 {
            increment(&mut row.running_image_mismatches)?;
            mark(row, &mut assessment.flags, FLAG_RUNNING_IMAGE_MISMATCH);
        }
        if report.loaded_adaptation_sha256 != expected_row.adaptation_sha256 {
            increment(&mut row.loaded_adaptation_mismatches)?;
            mark(row, &mut assessment.flags, FLAG_LOADED_ADAPTATION_MISMATCH);
        }
        if report.disk_adaptation_sha256 != expected_row.adaptation_sha256 {
            increment(&mut row.disk_adaptation_mismatches)?;
            mark(row, &mut assessment.flags, FLAG_DISK_ADAPTATION_MISMATCH);
        }
        if report.telemetry_loss_events > 0 {
            mark(row, &mut assessment.flags, FLAG_TELEMETRY_LOSS);
            add_u64(
                &mut row.telemetry_loss_events,
                u64::from(report.telemetry_loss_events),
            )?;
        }
        if report.sample_time_ns < 0 {
            increment(&mut row.invalid_time_rows)?;
            mark(row, &mut assessment.flags, FLAG_INVALID_TIME);
        } else if report.sample_time_ns > query_time_ns {
            increment(&mut row.future_rows)?;
            mark(row, &mut assessment.flags, FLAG_FUTURE);
        } else {
            let age = query_time_ns
                .checked_sub(report.sample_time_ns)
                .ok_or(StatusError::CounterOverflow)?;
            if age > freshness_limit_ns {
                increment(&mut row.stale_rows)?;
                mark(row, &mut assessment.flags, FLAG_STALE);
            }
        }

        if report.sequence < expected_row.sequence_first
            || report.sequence >= expected_row.sequence_next
        {
            increment(&mut row.sequences_outside_window)?;
            mark(row, &mut assessment.flags, FLAG_SEQUENCE_OUTSIDE_WINDOW);
        } else {
            let offset = report.sequence - expected_row.sequence_first;
            let word = (offset / 64) as usize;
            let bit = 1u64 << (offset % 64);
            let processor_seen = checked_mut(&mut seen, index)?;
            let seen_word = checked_ref(processor_seen, word)?;
            if *seen_word & bit != 0 {
                increment(&mut row.duplicate_sequences)?;
                mark(row, &mut assessment.flags, FLAG_DUPLICATE_SEQUENCE);
            } else {
                *checked_mut(processor_seen, word)? |= bit;
                increment(&mut row.unique_sequences)?;
                if *checked_ref(&has_last_sequence, index)?
                    && report.sequence < *checked_ref(&last_sequence, index)?
                {
                    increment(&mut row.out_of_order_sequences)?;
                    mark(row, &mut assessment.flags, FLAG_SEQUENCE_OUT_OF_ORDER);
                }
                if !*checked_ref(&has_last_sequence, index)?
                    || report.sequence > *checked_ref(&last_sequence, index)?
                {
                    *checked_mut(&mut last_sequence, index)? = report.sequence;
                    *checked_mut(&mut has_last_sequence, index)? = true;
                }
            }
        }
        if report.sample_time_ns >= 0 {
            if *checked_ref(&has_last_time, index)?
                && report.sample_time_ns < *checked_ref(&last_time, index)?
            {
                increment(&mut row.out_of_order_times)?;
                mark(row, &mut assessment.flags, FLAG_TIME_OUT_OF_ORDER);
            }
            if !*checked_ref(&has_last_time, index)?
                || report.sample_time_ns > *checked_ref(&last_time, index)?
            {
                *checked_mut(&mut last_time, index)? = report.sample_time_ns;
                *checked_mut(&mut has_last_time, index)? = true;
            }
        }
    }

    i = 0;
    while i < expected.len() {
        let span = *checked_ref(&spans, i)?;
        let row = checked_mut(&mut assessment.processors, i)?;
        if row.report_rows == 0 {
            increment(&mut assessment.missing_processors)?;
            mark(row, &mut assessment.flags, FLAG_MISSING_PROCESSOR);
        }
        row.missing_sequences = (span as u32)
            .checked_sub(row.unique_sequences)
            .ok_or(StatusError::CounterOverflow)?;
        if row.missing_sequences > 0 {
            row.flags |= FLAG_MISSING_SEQUENCE;
            assessment.flags |= FLAG_MISSING_SEQUENCE;
        }
        add_u64(
            &mut assessment.unique_sequences,
            u64::from(row.unique_sequences),
        )?;
        add_u64(
            &mut assessment.missing_sequences,
            u64::from(row.missing_sequences),
        )?;
        add_u64(
            &mut assessment.telemetry_loss_events,
            row.telemetry_loss_events,
        )?;
        increment_by(&mut assessment.duplicate_sequences, row.duplicate_sequences)?;
        increment_by(
            &mut assessment.out_of_order_sequences,
            row.out_of_order_sequences,
        )?;
        increment_by(&mut assessment.out_of_order_times, row.out_of_order_times)?;
        increment_by(&mut assessment.stale_rows, row.stale_rows)?;
        increment_by(&mut assessment.future_rows, row.future_rows)?;
        increment_by(&mut assessment.invalid_time_rows, row.invalid_time_rows)?;
        increment_by(&mut assessment.source_mismatches, row.source_mismatches)?;
        increment_by(&mut assessment.epoch_mismatches, row.epoch_mismatches)?;
        increment_by(
            &mut assessment.clock_domain_mismatches,
            row.clock_domain_mismatches,
        )?;
        increment_by(
            &mut assessment.release_manifest_mismatches,
            row.release_manifest_mismatches,
        )?;
        increment_by(&mut assessment.firmware_mismatches, row.firmware_mismatches)?;
        increment_by(
            &mut assessment.running_image_mismatches,
            row.running_image_mismatches,
        )?;
        increment_by(
            &mut assessment.loaded_adaptation_mismatches,
            row.loaded_adaptation_mismatches,
        )?;
        increment_by(
            &mut assessment.disk_adaptation_mismatches,
            row.disk_adaptation_mismatches,
        )?;
        increment_by(
            &mut assessment.sequences_outside_window,
            row.sequences_outside_window,
        )?;
        i += 1;
    }
    assessment.state = state_from_flags(assessment.flags);
    Ok(assessment)
}

fn increment_by(counter: &mut u32, value: u32) -> Result<(), StatusError> {
    *counter = counter
        .checked_add(value)
        .ok_or(StatusError::CounterOverflow)?;
    Ok(())
}

/// Compare a self-reported fixed-profile metric window with its independently declared sequence
/// interval. Any missing, stale, future, duplicated, out-of-order, mismatched or lossy evidence
/// keeps the result incomplete. A complete within-limit result remains unverified and is not PASS.
pub fn assess_metric_window(
    contract: &MetricWindowContract,
    samples: &[PerformanceSample],
    query_time_ns: i64,
    freshness_limit_ns: i64,
) -> Result<MetricWindowAssessment, StatusError> {
    if samples.len() > MAX_PERFORMANCE_SAMPLES {
        return Err(StatusError::PerformanceSamplesOverCapacity);
    }
    if query_time_ns < 0 {
        return Err(StatusError::InvalidQueryTime);
    }
    if freshness_limit_ns <= 0 {
        return Err(StatusError::InvalidFreshnessLimit);
    }
    if contract.window_end_ns <= contract.window_start_ns
        || contract.window_start_ns < 0
        || contract.window_end_ns > query_time_ns
    {
        return Err(StatusError::InvalidPerformanceWindow);
    }
    if contract.sequence_first == contract.sequence_next {
        return Err(StatusError::EmptyPerformanceWindow);
    }
    let span = seq_span(contract.sequence_first, contract.sequence_next)?;
    let expected_samples = u32::try_from(span).map_err(|_| StatusError::InvalidSequenceWindow)?;
    if expected_samples == 0 {
        return Err(StatusError::EmptyPerformanceWindow);
    }

    let epoch_regressed = contract
        .previous_epoch
        .is_some_and(|previous| contract.epoch.0 < previous.0);
    let mut assessment = MetricWindowAssessment {
        profile_id: contract.profile_id,
        metric_id: contract.metric_id,
        processor_id: contract.processor_id,
        previous_epoch: contract.previous_epoch,
        epoch: contract.epoch,
        epoch_order_checked: contract.previous_epoch.is_some(),
        epoch_regressed,
        clock_domain_id: contract.clock_domain_id,
        query_time_ns,
        freshness_limit_ns,
        window_start_ns: contract.window_start_ns,
        window_end_ns: contract.window_end_ns,
        expected_samples,
        reported_rows: samples.len() as u32,
        unique_sequences: 0,
        missing_samples: 0,
        duplicate_samples: 0,
        out_of_order_samples: 0,
        out_of_order_times: 0,
        stale_samples: 0,
        future_samples: 0,
        invalid_time_samples: 0,
        unexpected_samples: 0,
        sequences_outside_window: 0,
        profile_mismatches: 0,
        metric_mismatches: 0,
        epoch_mismatches: 0,
        clock_domain_mismatches: 0,
        sample_outside_window: 0,
        telemetry_loss_events: 0,
        usable_samples: 0,
        observed_maximum: 0,
        has_observed_maximum: false,
        maximum_allowed: contract.maximum_allowed,
        maximum_exceeded: false,
        flags: if contract.previous_epoch.is_none() {
            FLAG_EPOCH_ORDER_UNKNOWN
        } else if epoch_regressed {
            FLAG_EPOCH_REGRESSION
        } else {
            0
        },
        state: AssessmentState::Incomplete,
        provenance_unverified: true,
    };
    let mut seen = [0u64; SEQUENCE_WORDS];
    let mut has_last_sequence = false;
    let mut last_sequence = 0u64;
    let mut has_last_time = false;
    let mut last_time = 0i64;

    for sample in samples {
        let mut usable = true;
        let processor_matches = sample.processor_id == contract.processor_id;
        if !processor_matches {
            increment(&mut assessment.unexpected_samples)?;
            mark_metric(&mut assessment, FLAG_UNEXPECTED_PROCESSOR);
            usable = false;
        }
        if sample.profile_id != contract.profile_id {
            increment(&mut assessment.profile_mismatches)?;
            mark_metric(&mut assessment, FLAG_PROFILE_MISMATCH);
            usable = false;
        }
        if sample.metric_id != contract.metric_id {
            increment(&mut assessment.metric_mismatches)?;
            mark_metric(&mut assessment, FLAG_METRIC_MISMATCH);
            usable = false;
        }
        if sample.epoch != contract.epoch {
            increment(&mut assessment.epoch_mismatches)?;
            mark_metric(&mut assessment, FLAG_EPOCH_MISMATCH);
            usable = false;
        }
        if sample.clock_domain_id != contract.clock_domain_id {
            increment(&mut assessment.clock_domain_mismatches)?;
            mark_metric(&mut assessment, FLAG_CLOCK_DOMAIN_MISMATCH);
            usable = false;
        }
        if sample.sample_time_ns < 0 {
            increment(&mut assessment.invalid_time_samples)?;
            mark_metric(&mut assessment, FLAG_INVALID_TIME);
            usable = false;
        } else if sample.sample_time_ns > query_time_ns {
            increment(&mut assessment.future_samples)?;
            mark_metric(&mut assessment, FLAG_FUTURE);
            usable = false;
        } else {
            let age = query_time_ns
                .checked_sub(sample.sample_time_ns)
                .ok_or(StatusError::CounterOverflow)?;
            if age > freshness_limit_ns {
                increment(&mut assessment.stale_samples)?;
                mark_metric(&mut assessment, FLAG_STALE);
                usable = false;
            }
        }
        if sample.sample_time_ns < contract.window_start_ns
            || sample.sample_time_ns >= contract.window_end_ns
        {
            increment(&mut assessment.sample_outside_window)?;
            mark_metric(&mut assessment, FLAG_SAMPLE_OUTSIDE_WINDOW);
            usable = false;
        }
        if sample.telemetry_loss_events > 0 {
            mark_metric(&mut assessment, FLAG_TELEMETRY_LOSS);
            add_u64(
                &mut assessment.telemetry_loss_events,
                u64::from(sample.telemetry_loss_events),
            )?;
        }

        if processor_matches {
            if sample.sequence < contract.sequence_first
                || sample.sequence >= contract.sequence_next
            {
                increment(&mut assessment.sequences_outside_window)?;
                mark_metric(&mut assessment, FLAG_SEQUENCE_OUTSIDE_WINDOW);
                usable = false;
            } else {
                let offset = sample.sequence - contract.sequence_first;
                let word = (offset / 64) as usize;
                let bit = 1u64 << (offset % 64);
                if *checked_ref(&seen, word)? & bit != 0 {
                    increment(&mut assessment.duplicate_samples)?;
                    mark_metric(&mut assessment, FLAG_DUPLICATE_SEQUENCE);
                    usable = false;
                } else {
                    *checked_mut(&mut seen, word)? |= bit;
                    increment(&mut assessment.unique_sequences)?;
                    if has_last_sequence && sample.sequence < last_sequence {
                        increment(&mut assessment.out_of_order_samples)?;
                        mark_metric(&mut assessment, FLAG_SEQUENCE_OUT_OF_ORDER);
                        usable = false;
                    }
                    if !has_last_sequence || sample.sequence > last_sequence {
                        last_sequence = sample.sequence;
                        has_last_sequence = true;
                    }
                }
            }
        }
        if processor_matches && sample.sample_time_ns >= 0 {
            if has_last_time && sample.sample_time_ns < last_time {
                increment(&mut assessment.out_of_order_times)?;
                mark_metric(&mut assessment, FLAG_TIME_OUT_OF_ORDER);
                usable = false;
            }
            if !has_last_time || sample.sample_time_ns > last_time {
                last_time = sample.sample_time_ns;
                has_last_time = true;
            }
        }
        if usable {
            increment(&mut assessment.usable_samples)?;
            if !assessment.has_observed_maximum || sample.value > assessment.observed_maximum {
                assessment.observed_maximum = sample.value;
                assessment.has_observed_maximum = true;
            }
        }
    }

    assessment.missing_samples = expected_samples
        .checked_sub(assessment.unique_sequences)
        .ok_or(StatusError::CounterOverflow)?;
    if assessment.missing_samples > 0 {
        mark_metric(&mut assessment, FLAG_MISSING_SEQUENCE);
    }
    assessment.maximum_exceeded =
        assessment.has_observed_maximum && assessment.observed_maximum > assessment.maximum_allowed;
    let identity_flags = assessment.flags & crate::MISMATCH_FLAGS != 0;
    let incomplete_flags = assessment.flags & INCOMPLETE_FLAGS != 0;
    assessment.state = if identity_flags || incomplete_flags {
        state_from_flags(assessment.flags)
    } else if assessment.maximum_exceeded {
        AssessmentState::ClaimedLimitExceededUnverified
    } else {
        AssessmentState::ClaimsMatchUnverified
    };
    Ok(assessment)
}

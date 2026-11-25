#!/usr/bin/env python3
"""Development-only, causal turn-onset delay scorer (Python standard library only).

Truth labels come from a retrospective 1 Hz smoothed reference CSV. Estimator detections use
distinct observable output rows from a separate CSV; no row is duplicated or held forward.
"""

import argparse
import bisect
import csv
import math
import statistics
import sys
from collections import defaultdict
from pathlib import Path
import sealed_guard

SYSTEMS = ("oa", "cv", "sfdps")
MAX_BYTES = 32 * 1024 * 1024
MAX_ROWS = 250_000
PERSIST_SAMPLES = 3
TURN_RATE_DEG_S = 0.5
STRAIGHT_RATE_DEG_S = 0.25
STRAIGHT_S = 10
TRUTH_MIN_SPEED_KT = 30.0
MAX_DELAY_S = 30
EARLY_MATCH_S = 5
MAX_UPDATE_GAP_MS = 30_000
MAX_TRANSITION_MS = 30_000
MAX_MATCH_WORK = 5_000_000
# A matched event may scan unused alarms for near, candidates, any, min, and removal.
# Charging five full scans before any of them conservatively bounds every input-sized pass.
MATCH_SCAN_PASSES = 5


def wrap180(angle):
    """Signed circular difference in [-180, 180)."""
    return (angle + 180.0) % 360.0 - 180.0


def valid_truth_sample(course_deg, speed_kt):
    """Whether a reference row supports truth-rate labels and their horizons."""
    return (course_deg is not None and speed_kt is not None
            and speed_kt >= TRUTH_MIN_SPEED_KT)


def optional_float(text):
    if text is None or text.strip() == "":
        return None
    value = float(text)
    if not math.isfinite(value):
        raise ValueError("non-finite numeric value; use an empty cell for missing data")
    return value


def course(text):
    value = optional_float(text)
    if value is not None and not 0.0 <= value < 360.0:
        raise ValueError("course must be in [0, 360)")
    return value


def iter_csv(path, required, time_columns, optional_time_columns=()):
    sealed_guard.refuse(path)
    if path.stat().st_size > MAX_BYTES:
        raise ValueError(f"{path}: input exceeds {MAX_BYTES} bytes")
    with sealed_guard.open_read(path, "r", newline="", encoding="utf-8") as stream:
        reader = csv.DictReader(stream)
        missing = [name for name in required if name not in (reader.fieldnames or [])]
        if missing:
            raise ValueError(f"{path}: missing columns: {','.join(missing)}")
        if len(set(reader.fieldnames or [])) != len(reader.fieldnames or []):
            raise ValueError(f"{path}: duplicate column names")
        for n, row in enumerate(reader, 1):
            if n > MAX_ROWS:
                raise ValueError(f"{path}: input exceeds {MAX_ROWS} rows")
            if None in row or any(value is None for value in row.values()):
                raise ValueError(f"{path}:{n + 1}: malformed row width")
            # The production guard API accepts UTC nanoseconds; these CSV schemas store UTC
            # milliseconds. Convert only parseable integer timestamp cells for the guard, leaving
            # malformed values for this reader's explicit parser to reject below.
            guard_row = dict(row)
            for name in (*time_columns, *optional_time_columns):
                value = row.get(name)
                if value is None or value.strip() == "":
                    continue
                try:
                    guard_row[name] = str(int(value) * 1_000_000)
                except (ValueError, TypeError):
                    continue
            if sealed_guard.row_in_window(
                guard_row,
                time_columns,
                optional=optional_time_columns,
                keep_malformed=True,
            ):
                raise ValueError(f"{path}:{n + 1}: timestamp lies in a sealed window")
            yield n + 1, row


def load_truth(path):
    grouped = defaultdict(list)
    for line, row in iter_csv(path, ("leg", "time_ms", "truth_course_deg", "truth_speed_kt"), ("time_ms",)):
        leg = row["leg"]
        if not leg or len(leg) > 128:
            raise ValueError(f"{path}:{line}: leg must be 1..128 characters")
        try:
            t = int(row["time_ms"])
            if not -(2**63) <= t < 2**63:
                raise ValueError("timestamp outside signed 64-bit milliseconds")
            c = course(row["truth_course_deg"])
            speed = optional_float(row["truth_speed_kt"])
        except (ValueError, TypeError) as exc:
            raise ValueError(f"{path}:{line}: {exc}") from exc
        grouped[leg].append((t, c, speed))
    for leg, rows in grouped.items():
        rows.sort(key=lambda r: r[0])
        if any(b[0] <= a[0] for a, b in zip(rows, rows[1:])):
            raise ValueError(f"{path}: duplicate timestamp in leg {leg!r}")
        if any(b[0] - a[0] != 1000 for a, b in zip(rows, rows[1:])):
            raise ValueError(f"{path}: truth leg {leg!r} must be a complete 1 Hz grid")
    return grouped


def load_estimates(path):
    grouped = defaultdict(list)
    for line, row in iter_csv(path,
                              ("leg", "state_time_ms", "system", "course_deg", "availability_ms"),
                              ("state_time_ms",), ("availability_ms",)):
        leg, system = row["leg"], row["system"]
        if not leg or len(leg) > 128:
            raise ValueError(f"{path}:{line}: leg must be 1..128 characters")
        if system not in SYSTEMS:
            raise ValueError(f"{path}:{line}: system must be one of {','.join(SYSTEMS)}")
        try:
            t = int(row["state_time_ms"])
            if not -(2**63) <= t < 2**63:
                raise ValueError("timestamp outside signed 64-bit milliseconds")
            c = course(row["course_deg"])
        except (ValueError, TypeError) as exc:
            raise ValueError(f"{path}:{line}: {exc}") from exc
        if c is None:
            raise ValueError(f"{path}:{line}: an output row must contain course")
        raw_availability = row["availability_ms"].strip()
        try:
            availability_ms = int(raw_availability) if raw_availability else None
        except ValueError as exc:
            raise ValueError(f"{path}:{line}: availability_ms must be an integer millisecond") from exc
        if availability_ms is not None and not -(2**63) <= availability_ms < 2**63:
            raise ValueError(f"{path}:{line}: availability_ms outside signed 64-bit milliseconds")
        if availability_ms is not None and availability_ms < t:
            raise ValueError(f"{path}:{line}: availability time precedes state time")
        grouped[(leg, system)].append((t, c, availability_ms))
    for key, rows in grouped.items():
        rows.sort(key=lambda r: r[0])
        if any(b[0] <= a[0] for a, b in zip(rows, rows[1:])):
            raise ValueError(f"{path}: duplicate output timestamp for {key!r}")
    return grouped


def _truth_events_v2(rows):
    """Retrospective signed course-rate labels; future rates only confirm truth events."""
    events, previous = [], None
    quiet_start = run_start = None
    run_sign, run_len, active = 0, 0, False
    for t, c, speed in rows:
        if not valid_truth_sample(c, speed):
            previous, quiet_start, run_start = None, None, None
            run_sign, run_len, active = 0, 0, False
            continue
        if previous is None or t - previous[0] != 1000:
            previous, quiet_start, run_start = (t, c), None, None
            run_sign, run_len, active = 0, 0, False
            continue
        previous_t = previous[0]
        rate = wrap180(c - previous[1])
        previous = (t, c)
        if active:
            if abs(rate) < STRAIGHT_RATE_DEG_S:
                quiet_start = previous_t if quiet_start is None else quiet_start
                if t - quiet_start >= STRAIGHT_S * 1000:
                    active, quiet_start = False, t
            else:
                quiet_start = None
            continue
        if abs(rate) < STRAIGHT_RATE_DEG_S:
            quiet_start = previous_t if quiet_start is None else quiet_start
            run_start, run_sign, run_len = None, 0, 0
            continue
        armed = quiet_start is not None and t - quiet_start >= STRAIGHT_S * 1000
        sign = 1 if rate > 0.0 else -1
        if armed and abs(rate) >= TURN_RATE_DEG_S:
            if run_len == 0 or sign != run_sign:
                run_start, run_sign, run_len = t, sign, 1
            else:
                run_len += 1
            if run_len == PERSIST_SAMPLES:
                events.append((run_start, run_sign))
                active, quiet_start = True, None
                run_start, run_sign, run_len = None, 0, 0
        else:
            quiet_start = None
            run_start, run_sign, run_len = None, 0, 0
    return events


def _detect_v2(updates):
    """Causal detector on distinct updates; a stale held value never adds confirmations."""
    alarms = []
    previous, quiet_start, run_start = None, None, None
    run_sign, run_len, active = 0, 0, False
    for t, c in updates:
        if previous is None:
            previous = (t, c)
            continue
        dt_ms = t - previous[0]
        if dt_ms <= 0:
            continue
        if dt_ms > MAX_UPDATE_GAP_MS:
            previous, quiet_start, run_start = (t, c), None, None
            run_sign, run_len, active = 0, 0, False
            continue
        previous_t = previous[0]
        rate = wrap180(c - previous[1]) / (dt_ms / 1000.0)
        previous = (t, c)
        if active:
            if abs(rate) < STRAIGHT_RATE_DEG_S:
                quiet_start = previous_t if quiet_start is None else quiet_start
                if t - quiet_start >= STRAIGHT_S * 1000:
                    active, quiet_start = False, t
            else:
                quiet_start = None
            continue
        if abs(rate) < STRAIGHT_RATE_DEG_S:
            quiet_start = previous_t if quiet_start is None else quiet_start
            run_start, run_sign, run_len = None, 0, 0
            continue
        armed = quiet_start is not None and t - quiet_start >= STRAIGHT_S * 1000
        sign = 1 if rate > 0.0 else -1
        if armed and abs(rate) >= TURN_RATE_DEG_S:
            if run_len == 0 or sign != run_sign:
                run_start, run_sign, run_len = t, sign, 1
            else:
                run_len += 1
            if run_len == PERSIST_SAMPLES:
                alarms.append((t, run_sign))
                active, quiet_start = True, None
                run_start, run_sign, run_len = None, 0, 0
        else:
            quiet_start = None
            run_start, run_sign, run_len = None, 0, 0
    return alarms


V3_DIAGNOSTIC_KEYS = (
    "transition_started", "transition_confirmed", "transition_cancelled_quiet",
    "transition_cancelled_reverse", "transition_cancelled_timeout",
    "transition_cancelled_invalid", "transition_cancelled_gap",
    "transition_pending_at_end",
)


def _bump(diagnostics, name):
    if diagnostics is not None:
        diagnostics[name] = diagnostics.get(name, 0) + 1


def _new_v3_diagnostics():
    return dict.fromkeys(V3_DIAGNOSTIC_KEYS, 0)


def _truth_events_v3(rows, diagnostics=None):
    """Experimental retrospective v3 labels with an explicit armed transition state."""
    events, previous = [], None
    quiet_start = None
    candidate_start = candidate_sign = None
    run_len, active = 0, False
    for t, c, speed in rows:
        if not valid_truth_sample(c, speed):
            if candidate_start is not None:
                _bump(diagnostics, "transition_cancelled_invalid")
            previous, quiet_start, candidate_start, candidate_sign = None, None, None, None
            run_len, active = 0, False
            continue
        if previous is None or t - previous[0] != 1000:
            if candidate_start is not None:
                _bump(diagnostics, "transition_cancelled_gap")
            previous, quiet_start, candidate_start, candidate_sign = (t, c), None, None, None
            run_len, active = 0, False
            continue
        previous_t = previous[0]
        rate = wrap180(c - previous[1])
        previous = (t, c)
        if active:
            if abs(rate) < STRAIGHT_RATE_DEG_S:
                quiet_start = previous_t if quiet_start is None else quiet_start
                if t - quiet_start >= STRAIGHT_S * 1000:
                    active = False
            else:
                quiet_start = None
            continue
        if candidate_start is not None:
            if abs(rate) < STRAIGHT_RATE_DEG_S:
                _bump(diagnostics, "transition_cancelled_quiet")
                candidate_start, candidate_sign, run_len = None, None, 0
                quiet_start = previous_t
                continue
            sign = 1 if rate > 0.0 else -1
            if sign != candidate_sign:
                _bump(diagnostics, "transition_cancelled_reverse")
                candidate_start, candidate_sign, run_len, quiet_start = None, None, 0, None
                continue
            if t - candidate_start > MAX_TRANSITION_MS:
                _bump(diagnostics, "transition_cancelled_timeout")
                candidate_start, candidate_sign, run_len, quiet_start = None, None, 0, None
                continue
            if abs(rate) >= TURN_RATE_DEG_S:
                run_len += 1
                if run_len == PERSIST_SAMPLES:
                    events.append((candidate_start, candidate_sign))
                    _bump(diagnostics, "transition_confirmed")
                    active, quiet_start = True, None
                    candidate_start, candidate_sign, run_len = None, None, 0
            else:
                run_len = 0
            continue
        if abs(rate) < STRAIGHT_RATE_DEG_S:
            quiet_start = previous_t if quiet_start is None else quiet_start
            continue
        # Only intervals observed before this departure can qualify the straight baseline.
        armed = quiet_start is not None and previous_t - quiet_start >= STRAIGHT_S * 1000
        if armed and abs(rate) >= STRAIGHT_RATE_DEG_S:
            candidate_start = t
            candidate_sign = 1 if rate > 0.0 else -1
            run_len = int(abs(rate) >= TURN_RATE_DEG_S)
            quiet_start = None
            _bump(diagnostics, "transition_started")
        else:
            quiet_start = None
    if candidate_start is not None:
        _bump(diagnostics, "transition_pending_at_end")
    return events


def _detect_v3(updates, diagnostics=None):
    """Causal v3 detector; confirmation uses three distinct qualifying updates."""
    alarms = []
    previous, quiet_start = None, None
    candidate_start = candidate_sign = None
    run_len, active = 0, False
    for t, c in updates:
        if previous is None:
            previous = (t, c)
            continue
        dt_ms = t - previous[0]
        if dt_ms <= 0:
            continue
        if dt_ms > MAX_UPDATE_GAP_MS:
            if candidate_start is not None:
                _bump(diagnostics, "transition_cancelled_gap")
            previous, quiet_start, candidate_start, candidate_sign = (t, c), None, None, None
            run_len, active = 0, False
            continue
        previous_t = previous[0]
        rate = wrap180(c - previous[1]) / (dt_ms / 1000.0)
        previous = (t, c)
        if active:
            if abs(rate) < STRAIGHT_RATE_DEG_S:
                quiet_start = previous_t if quiet_start is None else quiet_start
                if t - quiet_start >= STRAIGHT_S * 1000:
                    active = False
            else:
                quiet_start = None
            continue
        if candidate_start is not None:
            if abs(rate) < STRAIGHT_RATE_DEG_S:
                _bump(diagnostics, "transition_cancelled_quiet")
                candidate_start, candidate_sign, run_len = None, None, 0
                quiet_start = previous_t
                continue
            sign = 1 if rate > 0.0 else -1
            if sign != candidate_sign:
                _bump(diagnostics, "transition_cancelled_reverse")
                candidate_start, candidate_sign, run_len, quiet_start = None, None, 0, None
                continue
            if t - candidate_start > MAX_TRANSITION_MS:
                _bump(diagnostics, "transition_cancelled_timeout")
                candidate_start, candidate_sign, run_len, quiet_start = None, None, 0, None
                continue
            if abs(rate) >= TURN_RATE_DEG_S:
                run_len += 1
                if run_len == PERSIST_SAMPLES:
                    alarms.append((t, candidate_sign))
                    _bump(diagnostics, "transition_confirmed")
                    active, quiet_start = True, None
                    candidate_start, candidate_sign, run_len = None, None, 0
            else:
                run_len = 0
            continue
        if abs(rate) < STRAIGHT_RATE_DEG_S:
            quiet_start = previous_t if quiet_start is None else quiet_start
            continue
        # The current rate interval is the departure; it cannot complete the straight baseline.
        armed = quiet_start is not None and previous_t - quiet_start >= STRAIGHT_S * 1000
        if armed and abs(rate) >= STRAIGHT_RATE_DEG_S:
            candidate_start = t
            candidate_sign = 1 if rate > 0.0 else -1
            run_len = int(abs(rate) >= TURN_RATE_DEG_S)
            quiet_start = None
            _bump(diagnostics, "transition_started")
        else:
            quiet_start = None
    if candidate_start is not None:
        _bump(diagnostics, "transition_pending_at_end")
    return alarms


def truth_events(rows, protocol="v2", diagnostics=None):
    if protocol == "v2":
        return _truth_events_v2(rows)
    if protocol == "v3":
        return _truth_events_v3(rows, diagnostics)
    raise ValueError("protocol must be 'v2' or 'v3'")


def detect(updates, protocol="v2", diagnostics=None):
    if protocol == "v2":
        return _detect_v2(updates)
    if protocol == "v3":
        return _detect_v3(updates, diagnostics)
    raise ValueError("protocol must be 'v2' or 'v3'")


def reference_gap(valid_truth_times, onset):
    return any(onset + s * 1000 not in valid_truth_times
               for s in range(-STRAIGHT_S, MAX_DELAY_S + 1))


def missing_event(update_times, onset, budget):
    lo = bisect.bisect_left(update_times, onset - STRAIGHT_S * 1000)
    hi = bisect.bisect_right(update_times, onset + MAX_DELAY_S * 1000)
    inspected = hi - lo
    if inspected > budget:
        raise ValueError(f"matching exceeded the bounded work limit ({MAX_MATCH_WORK})")
    relevant = update_times[lo:hi]
    if not relevant:
        return True, inspected
    points = [onset - STRAIGHT_S * 1000, *relevant, onset + MAX_DELAY_S * 1000]
    return any(b - a > MAX_UPDATE_GAP_MS for a, b in zip(points, points[1:])), inspected


def score(truth_rows, events, updates, alarms):
    unused = list(alarms)
    matched, gaps, misses, early = [], 0, 0, 0
    gap_events = []
    unavailable = 0
    work = len(events) * (STRAIGHT_S + MAX_DELAY_S + 1)
    if work > MAX_MATCH_WORK:
        raise ValueError(f"matching exceeded the bounded work limit ({MAX_MATCH_WORK})")
    valid_truth_times = {t for t, c, speed in truth_rows if valid_truth_sample(c, speed)}
    update_times = [t for t, _, _ in updates]
    opposite = 0
    for onset, direction in events:
        if reference_gap(valid_truth_times, onset):
            gaps += 1
            gap_events.append(onset)
            continue
        unavailable_now, inspected = missing_event(update_times, onset, MAX_MATCH_WORK - work)
        unavailable += int(unavailable_now)
        work += inspected
        if work > MAX_MATCH_WORK:
            raise ValueError(f"matching exceeded the bounded work limit ({MAX_MATCH_WORK})")
        lo, hi = onset - EARLY_MATCH_S * 1000, onset + MAX_DELAY_S * 1000
        scan_work = MATCH_SCAN_PASSES * len(unused)
        if work + scan_work > MAX_MATCH_WORK:
            raise ValueError(f"matching exceeded the bounded work limit ({MAX_MATCH_WORK})")
        work += scan_work
        near = [a for a in unused if lo <= a[0] <= hi]
        candidates = [a for a in near if a[1] == direction]
        if not candidates and any(a[1] != direction for a in near):
            opposite += 1
        if not candidates:
            misses += 1
            continue
        alarm = min(candidates, key=lambda a: (abs(a[0] - onset), a[0]))
        unused.remove(alarm)
        delay = (alarm[0] - onset) / 1000.0
        matched.append(delay)
        early += int(delay < 0)
    gap_associated_alarms = 0
    retained_alarms = []
    for alarm in unused:
        if work >= MAX_MATCH_WORK:
            raise ValueError(f"matching exceeded the bounded work limit ({MAX_MATCH_WORK})")
        work += 1
        associated = False
        for onset in gap_events:
            if work >= MAX_MATCH_WORK:
                raise ValueError(f"matching exceeded the bounded work limit ({MAX_MATCH_WORK})")
            work += 1
            if -EARLY_MATCH_S * 1000 <= alarm[0] - onset <= MAX_DELAY_S * 1000:
                associated = True
                break
        if associated:
            gap_associated_alarms += 1
        else:
            retained_alarms.append(alarm)
    unused = retained_alarms
    flight_hours = sum(1 for _, c, speed in truth_rows if c is not None and speed is not None) / 3600.0
    return {"truth_events": len(events), "matched": len(matched), "truth_gaps": gaps,
            "opposite_direction_misses": opposite,
            "gap_associated_alarms": gap_associated_alarms,
            "missing_output_events": unavailable, "never_detected": misses, "early": early,
            "false_turns": len(unused), "update_rows": len(updates),
            "false_turns_per_flight_hour": len(unused) / flight_hours if flight_hours else None,
            "flight_hours": flight_hours, "delays": matched}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--truth", required=True, type=Path, help="1 Hz smoothed reference CSV")
    parser.add_argument("--estimates", required=True, type=Path, help="distinct output update CSV")
    parser.add_argument("--protocol", choices=("v2", "v3"), default="v2",
                        help="metric protocol version (default: v2)")
    args = parser.parse_args(argv)
    try:
        truth = load_truth(args.truth)
        estimates = load_estimates(args.estimates)
        totals = {n: defaultdict(float) for n in SYSTEMS}
        totals_delays = {n: [] for n in SYSTEMS}
        update_intervals = {n: [] for n in SYSTEMS}
        availability_intervals = {n: [] for n in SYSTEMS}
        availability_lags = {n: [] for n in SYSTEMS}
        availability_rows = {n: 0 for n in SYSTEMS}
        truth_transition_diagnostics = _new_v3_diagnostics()
        detector_transition_diagnostics = {n: _new_v3_diagnostics() for n in SYSTEMS}
        event_count = 0
        for leg, rows in truth.items():
            events = truth_events(rows, args.protocol,
                                  truth_transition_diagnostics if args.protocol == "v3" else None)
            event_count += len(events)
            for name in SYSTEMS:
                updates = estimates.get((leg, name), [])
                alarms = detect(((t, c) for t, c, _ in updates), args.protocol,
                                detector_transition_diagnostics[name] if args.protocol == "v3" else None)
                result = score(rows, events, updates, alarms)
                for key in ("truth_events", "matched", "truth_gaps", "missing_output_events", "never_detected", "opposite_direction_misses", "gap_associated_alarms", "early", "false_turns", "update_rows", "flight_hours"):
                    totals[name][key] += result[key]
                totals_delays[name].extend(result["delays"])
                update_intervals[name].extend(b[0] - a[0] for a, b in zip(updates, updates[1:]))
                received = [t for _, _, t in updates if t is not None]
                received.sort()
                availability_rows[name] += len(received)
                availability_intervals[name].extend(b - a for a, b in zip(received, received[1:]))
                availability_lags[name].extend(t - state_t for state_t, _, t in updates if t is not None)
        if args.protocol == "v2":
            print("protocol=turn-onset-dev-v2 truth=retrospective-smoothed-reference signed_ground_track_rate_only")
            print(f"turn_rate_deg_s={TURN_RATE_DEG_S:g} straight_rate_deg_s={STRAIGHT_RATE_DEG_S:g} straight_s={STRAIGHT_S} persistence_samples={PERSIST_SAMPLES} max_delay_s={MAX_DELAY_S} early_match_s={EARLY_MATCH_S} max_update_gap_s={MAX_UPDATE_GAP_MS / 1000:g}")
        else:
            print("protocol=turn-onset-dev-v3-experimental truth=retrospective-smoothed-reference signed_ground_track_rate_only")
            print(f"turn_rate_deg_s={TURN_RATE_DEG_S:g} straight_rate_deg_s={STRAIGHT_RATE_DEG_S:g} straight_s={STRAIGHT_S} persistence_samples={PERSIST_SAMPLES} max_transition_s={MAX_TRANSITION_MS / 1000:g} max_delay_s={MAX_DELAY_S} early_match_s={EARLY_MATCH_S} max_update_gap_s={MAX_UPDATE_GAP_MS / 1000:g}")
            print("v3_truth_transition_diagnostics=" + ",".join(
                f"{key}:{truth_transition_diagnostics[key]}" for key in V3_DIAGNOSTIC_KEYS))
            for name in SYSTEMS:
                diag = detector_transition_diagnostics[name]
                print(f"v3_{name}_transition_diagnostics=" + ",".join(
                    f"{key}:{diag[key]}" for key in V3_DIAGNOSTIC_KEYS))
        print("terminal_event_partition=shared_truth_events=truth_reference_gaps+matched+never_detected;opposite_direction_misses_subset_of_never_detected;missing_output_events_may_overlap_never_detected")
        print("system,shared_truth_events,matched,truth_reference_gaps,missing_output_events,never_detected,opposite_direction_misses,gap_associated_alarms,early,false_turns,output_update_rows,state_interval_p50_s,state_interval_p90_s,state_interval_p99_s,state_interval_max_s,availability_rows,availability_interval_p50_s,availability_interval_p90_s,availability_interval_p99_s,availability_interval_max_s,availability_minus_state_p50_s,availability_minus_state_p90_s,availability_minus_state_p99_s,availability_minus_state_max_s,observed_flight_hours,false_turns_per_flight_hour,median_state_delay_s,p90_state_delay_s")
        for name in SYSTEMS:
            s, ds = totals[name], sorted(totals_delays[name])
            q90 = ds[max(0, math.ceil(.9 * len(ds)) - 1)] if ds else None
            med = statistics.median(ds) if ds else None
            rate = s["false_turns"] / s["flight_hours"] if s["flight_hours"] else None
            intervals = sorted(update_intervals[name])
            interval_q = lambda p: intervals[max(0, math.ceil(p * len(intervals)) - 1)] / 1000 if intervals else None
            avail_intervals = sorted(availability_intervals[name])
            avail_q = lambda p: avail_intervals[max(0, math.ceil(p * len(avail_intervals)) - 1)] / 1000 if avail_intervals else None
            lags = sorted(availability_lags[name])
            lag_q = lambda p: lags[max(0, math.ceil(p * len(lags)) - 1)] / 1000 if lags else None
            fmt = lambda x: "NA" if x is None else f"{x:.3f}"
            print(f"{name},{event_count},{int(s['matched'])},{int(s['truth_gaps'])},{int(s['missing_output_events'])},{int(s['never_detected'])},{int(s['opposite_direction_misses'])},{int(s['gap_associated_alarms'])},{int(s['early'])},{int(s['false_turns'])},{int(s['update_rows'])},{fmt(interval_q(.5))},{fmt(interval_q(.9))},{fmt(interval_q(.99))},{fmt(intervals[-1] / 1000 if intervals else None)},{availability_rows[name]},{fmt(avail_q(.5))},{fmt(avail_q(.9))},{fmt(avail_q(.99))},{fmt(avail_intervals[-1] / 1000 if avail_intervals else None)},{fmt(lag_q(.5))},{fmt(lag_q(.9))},{fmt(lag_q(.99))},{fmt(lags[-1] / 1000 if lags else None)},{s['flight_hours']:.4f},{fmt(rate)},{fmt(med)},{fmt(q90)}")
    except (OSError, ValueError) as exc:
        print(f"turn_onset: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

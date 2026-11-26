"""Source-only acceptance checks for the isolated allocator exercise.

This module reads repository manifests and Rust source; it does not invoke Cargo.
"""
from __future__ import annotations

import json
from pathlib import Path
import re
import tomllib


HARNESS = Path("validation/oa-alloc-check")
PRESENTATION = Path("crates/oa-display-present")
WASM_ABI = Path("tools/oa-demo-wasm")
PRESENTATION_SOURCES = {"src/lib.rs", "src/linux.rs"}
CORE_FFI = Path("crates/openatca-core-ffi")
CORE_FFI_DEPS = {
    "oa-geo": "../oa-geo", "oa-ieee": "../oa-ieee",
    "oa-math": "../oa-math", "oa-sha256": "../oa-sha256",
    "oa-csv": "../oa-csv", "oa-json": "../oa-json", "oa-xml": "../oa-xml",
}
CORE_FFI_CRATE_TYPES = ["cdylib", "staticlib", "rlib"]
CORE_FFI_SOURCE_FILES = {"lib.rs", "ffi.rs", "text.rs"}
STRICT_RUNTIME_CLIPPY_LINTS = ("unwrap_used", "expect_used", "panic", "indexing_slicing")
# Pinned to oa-airspace::core::route::MAX_ROUTE_WORK: full identity scans across the
# 11,640-point regional model need 744,960 visits for 64 endpoints, plus expansion.
ROUTE_CONVERSION_WORK_LIMIT = 1_048_576
HARNESS_DEPS = {
    "oa-track": "../../crates/oa-track", "oa-nam": "../../crates/oa-nam",
    "oa-display": "../../crates/oa-display", "oa-scope-replay": "../../crates/oa-scope-replay",
    "oa-airspace": "../../crates/oa-airspace", "oa-geo": "../../crates/oa-geo",
    "oa-safety": "../../crates/oa-safety",
}
HELPER_MODULES = ("pending_windows.rs", "nam_window.rs", "display_window.rs", "safety_window.rs", "route_window.rs")
FIXTURE_IDS = {
    "tracker_update_ooo_refusal_capacity_advance_flush",
    "tracker_default_4096_capacity_recovery",
    "tracker_output_log_overflow_reuse",
    "tracker_clock_hold_stepback_recovery",
    "fusion_bucket_plot_cap_refusal_recovery",
    "fusion_adsb_pending_queue_capacity_recovery",
    "fusion_plot_pending_queue_capacity_recovery",
    "fusion_adsb_hard_assoc_registration",
    "fusion_anonymous_init_and_assoc",
    "nam_session_activate_traffic_retry_full_deactivate",
    "display_rows_traffic_frame_refusal",
    "display_raw_fallback_draw_capacity_invalid_refusal",
    "display_controller_interaction_queue",
    "safety_state_inhibit_display",
    "airspace_route_direct_airway_procedure_atomic_refusal",
}
TRACKER_FIELDS = {
    "applied": 4, "refiltered": 1, "refused": 1, "capacity_refused": 1,
    "full_capacity": 8, "recovery_applied": 1, "predictions": 1, "advance_rows": 1, "flush_rows": 1,
    "ended": 1, "rows": 1,
}
TRACKER_VECTOR_FIELDS = (
    "held", "key", "ident", "ident_ns", "ident_conflict", "state", "first_ns", "confirmed_ns", "last_ns", "disagree_until_ns",
    "expected_ns", "updates", "hits", "cur", "ring_len", "ring_head", "cat", "anchor",
    "ring_m", "mv_on", "mv_start_ns", "mv_first_ns", "mv_first_sens", "mv_old_vote_ns",
    "mv_resync_until_ns", "mv_snap", "snaps", "snap_free", "free", "index",
)
TRACK_LOG_VECTOR_FIELDS = (
    "addr", "first_ns", "confirmed_ns", "last_ns", "updates", "reason",
)
FUSION_FIELDS = {
    "plots": 40, "hard_applied": 1, "candidates_confirmed": 2, "refused": 1,
    "rows": 1, "radar_tracks": 2, "registration_used": 1,
    "registration_history": 1, "anon_applied": 2, "local_rows": 2,
    "init_visits": 1, "init_tests": 1, "coast_rows": 1, "quiet_advance_rows": 1,
}

# Evidence ledger: each measured API is tied to its owning production source and
# to a result field that demonstrates its route had an observable effect. This
# is intentionally scoped to deterministic fixtures; it does not imply that
# every input through these APIs has been measured.
ROUTE_LEDGER = {
    "tracker_update": ("crates/oa-track-core/src/tracker.rs", "applied"),
    "tracker_out_of_order_refilter": ("crates/oa-track-core/src/tracker.rs", "refiltered"),
    "tracker_invalid_input_refusal": ("crates/oa-track-core/src/tracker.rs", "refused"),
    "tracker_capacity_refusal": ("crates/oa-track-core/src/tracker.rs", "capacity_refused"),
    "tracker_capacity_recovery": ("crates/oa-track-core/src/tracker.rs", "recovery_applied"),
    "tracker_default_capacity": ("crates/oa-track-core/src/tracker.rs", "default_capacity.occupied"),
    "tracker_default_confirmed_capacity": ("crates/oa-track-core/src/tracker.rs", "default_capacity.confirmed"),
    "tracker_history_wrap_refilter": ("crates/oa-track-core/src/tracker.rs", "default_capacity.history_refiltered"),
    "tracker_history_wrap_replay": ("crates/oa-track-core/src/tracker.rs", "default_capacity.history_replayed"),
    "tracker_history_retention_refusal": ("crates/oa-track-core/src/tracker.rs", "default_capacity.late_ring"),
    "tracker_output_overflow": ("crates/oa-track-core/src/tracker.rs", "output_log.output_overflow"),
    "tracker_log_overflow": ("crates/oa-track-core/src/tracker.rs", "output_log.log_overflow"),
    "tracker_output_reuse": ("crates/oa-track-core/src/types.rs", "output_log.output_rows_after_reuse"),
    "tracker_output_payload_stability": ("crates/oa-track-core/src/tracker.rs", "output_log.output_payload_unchanged"),
    "tracker_clock_hold": ("crates/oa-track-core/src/tracker.rs", "clock_gate.held"),
    "tracker_clock_forward_recovery": ("crates/oa-track-core/src/tracker.rs", "clock_gate.held_applied"),
    "tracker_clock_backstep": ("crates/oa-track-core/src/tracker.rs", "clock_gate.step_back"),
    "tracker_clock_recovery": ("crates/oa-track-core/src/tracker.rs", "clock_gate.recovered"),
    "tracker_prediction": ("crates/oa-track-core/src/tracker.rs", "predictions"),
    "tracker_advance": ("crates/oa-track-core/src/tracker.rs", "advance_rows"),
    "tracker_flush": ("crates/oa-track-core/src/tracker.rs", "flush_rows"),
    "fusion_adsb_ingest": ("crates/oa-track/src/fusion.rs", "hard_applied"),
    "fusion_plot_ingest": ("crates/oa-track/src/fusion.rs", "plots"),
    "fusion_advance": ("crates/oa-track/src/fusion.rs", "quiet_advance_rows"),
    "fusion_finish": ("crates/oa-track/src/fusion.rs", "rows"),
    "fusion_registration": ("crates/oa-track/src/fusion.rs", "registration_used"),
    "fusion_anonymous_initiation": ("crates/oa-track/src/fusion.rs", "candidates_confirmed"),
    "fusion_local_association": ("crates/oa-track/src/fusion.rs", "anon_applied"),
    "fusion_bucket_plot_cap": ("crates/oa-track/src/fusion.rs", "bucket_capacity.bucket_plot_overflow"),
    "fusion_bucket_known_match": ("crates/oa-track/src/fusion.rs", "bucket_capacity.known_hard_applied"),
    "fusion_bucket_unknown_identity": ("crates/oa-track/src/fusion.rs", "bucket_capacity.unmatched_hard_no_track"),
    "fusion_bucket_capacity_recovery": ("crates/oa-track/src/fusion.rs", "bucket_capacity.recovery_applied"),
    "fusion_adsb_pending_overflow": ("crates/oa-track/src/fusion.rs", "adsb_pending.pending_overflow"),
    "fusion_adsb_pending_drain": ("crates/oa-track/src/fusion.rs", "adsb_pending.drained"),
    "fusion_adsb_pending_recovery": ("crates/oa-track/src/fusion.rs", "adsb_pending.recovery_applied"),
    "fusion_plot_pending_overflow": ("crates/oa-track/src/fusion.rs", "plot_pending.pending_overflow"),
    "fusion_plot_pending_bucket_cap": ("crates/oa-track/src/fusion.rs", "plot_pending.bucket_plot_overflow"),
    "fusion_plot_pending_recovery": ("crates/oa-track/src/fusion.rs", "plot_pending.recovery_applied"),
    "nam_session_activation": ("crates/oa-nam/src/core/session.rs", "nam_session.irq_sent"),
    "nam_session_traffic": ("crates/oa-nam/src/core/session.rs", "nam_session.atm_sent"),
    "nam_session_resend": ("crates/oa-nam/src/core/session.rs", "nam_session.resent"),
    "nam_session_timeout": ("crates/oa-nam/src/core/session.rs", "nam_session.timed_out"),
    "nam_session_deliver": ("crates/oa-nam/src/core/session.rs", "nam_session.delivered"),
    "nam_session_response": ("crates/oa-nam/src/core/session.rs", "nam_session.responses"),
    "nam_session_keepalive": ("crates/oa-nam/src/core/session.rs", "nam_session.asm_sent"),
    "nam_session_peer_silent": ("crates/oa-nam/src/core/session.rs", "nam_session.peer_silent"),
    "nam_session_pending_full": ("crates/oa-nam/src/core/session.rs", "nam_session.pending_full"),
    "nam_session_discard": ("crates/oa-nam/src/core/session.rs", "nam_session.discards"),
    "nam_session_abandon": ("crates/oa-nam/src/core/session.rs", "nam_session.abandoned"),
    "nam_session_deactivation": ("crates/oa-nam/src/core/session.rs", "nam_session.trq_sent"),
    "display_rows_to_targets": ("crates/oa-scope-replay/src/core/rows.rs", "display_frame.convert_added"),
    "display_track_end": ("crates/oa-scope-replay/src/core/rows.rs", "display_frame.convert_ended"),
    "display_traffic_draw": ("crates/oa-display/src/core/traffic.rs", "display_frame.traffic_drawn"),
    "display_uncertain_radius": ("crates/oa-display/src/core/traffic.rs", "display_frame.r95_unknown"),
    "display_callsign_conflict": ("crates/oa-display/src/core/traffic.rs", "display_frame.cs_conflict"),
    "display_altitude_marker": ("crates/oa-display/src/core/traffic.rs", "display_frame.alt_clamped"),
    "display_set_refusal": ("crates/oa-display/src/core/traffic.rs", "display_frame.refused"),
    "display_malformed_target": ("crates/oa-display/src/core/traffic.rs", "display_frame.malformed"),
    "display_frame_chain": ("crates/oa-scope-replay/src/core/frame.rs", "display_frame.frame_drawn"),
    "display_map_scene": ("crates/oa-display/src/core/scene.rs", "display_frame.scene_items"),
    "display_frame_refusal": ("crates/oa-scope-replay/src/core/frame.rs", "display_frame.refusing_refused"),
    "display_raw_frame_draw": ("crates/oa-scope-replay/src/core/frame.rs", "display_frame.raw_drawn"),
    "display_raw_capacity_refusal": ("crates/oa-scope-replay/src/core/frame.rs", "display_frame.raw_capacity_refused"),
    "display_raw_invalid_refusal": ("crates/oa-scope-replay/src/core/frame.rs", "display_frame.raw_invalid_refused"),
    "display_invalid_position": ("crates/oa-scope-replay/src/core/rows.rs", "display_frame.convert_invalid_position"),
    "display_unprojectable": ("crates/oa-scope-replay/src/core/rows.rs", "display_frame.convert_unprojectable"),
    "display_history_dot_dropped": ("crates/oa-scope-replay/src/core/rows.rs", "display_frame.convert_trail_dropped"),
    "display_time_overflow": ("crates/oa-scope-replay/src/core/rows.rs", "display_frame.convert_time_overflow"),
    "display_unplaced_history_dot": ("crates/oa-display/src/core/traffic.rs", "display_frame.draw_trail_dropped"),
    "display_stamp_clipped": ("crates/oa-scope-replay/src/core/frame.rs", "display_frame.stamp_clipped"),
    "display_frame_digest": ("crates/oa-display/src/core/raster.rs", "display_frame.digests"),
    "display_interaction_selection": ("crates/oa-display/src/core/interaction.rs", "display_frame.interaction_selected"),
    "display_interaction_retry_refusal": ("crates/oa-display/src/core/interaction.rs", "display_frame.interaction_refused"),
    "display_interaction_enqueue": ("crates/oa-display/src/core/interaction.rs", "display_frame.interaction_queued"),
    "display_interaction_preflight": ("crates/oa-display/src/core/interaction.rs", "display_frame.interaction_preflighted"),
    "display_interaction_completion": ("crates/oa-display/src/core/interaction.rs", "display_frame.interaction_completed"),
    "display_interaction_presentation": ("crates/oa-display/src/core/interaction.rs", "display_frame.interaction_presented"),
    "display_interaction_rasterization": ("crates/oa-display/src/core/raster.rs", "display_frame.interaction_rasterized"),
    "display_msaw_markers": ("crates/oa-display/src/core/alert_markers.rs", "display_frame.msaw_presented"),
    "display_msaw_refusal": ("crates/oa-display/src/core/alert_markers.rs", "display_frame.msaw_refused"),
    "display_msaw_rasterization": ("crates/oa-display/src/core/raster.rs", "display_frame.msaw_rasterized"),
    "safety_state_report": ("crates/oa-safety/src/net.rs", "safety.state_reports"),
    "safety_net_status": ("crates/oa-safety/src/net.rs", "safety.stale_unavailable"),
    "safety_inhibit_create": ("crates/oa-safety/src/inhibit.rs", "safety.created"),
    "safety_inhibit_refusal": ("crates/oa-safety/src/inhibit.rs", "safety.refused"),
    "safety_table_full": ("crates/oa-safety/src/inhibit.rs", "safety.refused_full"),
    "safety_suppress_log": ("crates/oa-safety/src/inhibit.rs", "safety.suppressed_logged"),
    "safety_suppressed_overflow": ("crates/oa-safety/src/inhibit.rs", "safety.suppressed_overflow"),
    "safety_inhibit_remove": ("crates/oa-safety/src/inhibit.rs", "safety.removed"),
    "safety_inhibit_expire": ("crates/oa-safety/src/inhibit.rs", "safety.expired"),
    "safety_events_take": ("crates/oa-safety/src/inhibit.rs", "safety.events"),
    "safety_banner_draw": ("crates/oa-display/src/core/safety.rs", "safety.banner_lines"),
    "safety_inhibit_draw": ("crates/oa-display/src/core/safety.rs", "safety.inhibitions_drawn"),
    "safety_state_backdated": ("crates/oa-safety/src/net.rs", "safety.state_backdated"),
    "safety_state_future_refusal": ("crates/oa-safety/src/net.rs", "safety.state_future_refused"),
    "safety_banner_clamp": ("crates/oa-display/src/core/safety.rs", "safety.banner_hidden"),
    "safety_build_error": ("crates/oa-display/src/core/safety.rs", "safety.error_builds"),
    "route_direct_fix_identity": ("crates/oa-airspace/src/core/route.rs", "route_conversion.direct_points"),
    "route_airway_expansion": ("crates/oa-airspace/src/core/route.rs", "route_conversion.airway_points"),
    "route_procedure_transition": ("crates/oa-airspace/src/core/route.rs", "route_conversion.procedure_points"),
    "route_atomic_refusal": ("crates/oa-airspace/src/core/route.rs", "route_conversion.refusals"),
}

ROUTE_SYMBOLS = {
    "tracker_update": "tracker_update", "tracker_out_of_order_refilter": "tracker_update",
    "tracker_invalid_input_refusal": "tracker_update", "tracker_capacity_refusal": "tracker_update",
    "tracker_capacity_recovery": "tracker_update",
    "tracker_default_capacity": "tracker_update",
    "tracker_default_confirmed_capacity": "tracker_update",
    "tracker_history_wrap_refilter": "tracker_update",
    "tracker_history_wrap_replay": "tracker_update",
    "tracker_history_retention_refusal": "tracker_update",
    "tracker_output_overflow": "tracker_update",
    "tracker_log_overflow": "tracker_flush",
    "tracker_output_reuse": "output_clear",
    "tracker_output_payload_stability": "tracker_update",
    "tracker_clock_hold": "tracker_update",
    "tracker_clock_forward_recovery": "tracker_update",
    "tracker_clock_backstep": "tracker_update",
    "tracker_clock_recovery": "tracker_update",
    "tracker_prediction": "tracker_predict", "tracker_advance": "tracker_advance",
    "tracker_flush": "tracker_flush", "fusion_adsb_ingest": "fusion_adsb",
    "fusion_plot_ingest": "fusion_plots", "fusion_advance": "fusion_advance",
    "fusion_finish": "fusion_finish", "fusion_registration": "fusion_registration",
    "fusion_anonymous_initiation": "fusion_plots", "fusion_local_association": "fusion_plots",
    "fusion_bucket_plot_cap": "process_bucket",
    "fusion_bucket_known_match": "associate_bucket",
    "fusion_bucket_unknown_identity": "associate_bucket",
    "fusion_bucket_capacity_recovery": "fusion_plots",
    "fusion_adsb_pending_overflow": "fusion_adsb",
    "fusion_adsb_pending_drain": "fusion_advance",
    "fusion_adsb_pending_recovery": "fusion_adsb",
    "fusion_plot_pending_overflow": "fusion_plots",
    "fusion_plot_pending_bucket_cap": "process_bucket",
    "fusion_plot_pending_recovery": "fusion_plots",
    "nam_session_activation": "step",
    "nam_session_traffic": "step",
    "nam_session_resend": "step",
    "nam_session_timeout": "step",
    "nam_session_deliver": "step",
    "nam_session_response": "step",
    "nam_session_keepalive": "step",
    "nam_session_peer_silent": "step",
    "nam_session_pending_full": "step",
    "nam_session_discard": "step",
    "nam_session_abandon": "step",
    "nam_session_deactivation": "step",
    "display_rows_to_targets": "targets_at", "display_track_end": "targets_at",
    "display_traffic_draw": "build_traffic", "display_uncertain_radius": "build_traffic",
    "display_callsign_conflict": "build_traffic", "display_altitude_marker": "alt_text",
    "display_set_refusal": "targets_push", "display_malformed_target": "targets_push",
    "display_frame_chain": "replay_build", "display_map_scene": "build_scene",
    "display_frame_refusal": "replay_build",
    "display_raw_frame_draw": "replay_build_raw",
    "display_raw_capacity_refusal": "replay_build_raw",
    "display_raw_invalid_refusal": "replay_build_raw",
    "display_invalid_position": "targets_at",
    "display_unprojectable": "targets_at",
    "display_history_dot_dropped": "targets_at",
    "display_time_overflow": "targets_at",
    "display_unplaced_history_dot": "build_traffic",
    "display_stamp_clipped": "replay_build",
    "display_frame_digest": "frame_digest",
    "display_interaction_selection": "select_target",
    "display_interaction_retry_refusal": "enqueue_intent",
    "display_interaction_enqueue": "enqueue_intent",
    "display_interaction_preflight": "preflight_oldest",
    "display_interaction_completion": "complete_oldest",
    "display_interaction_presentation": "present_interaction",
    "display_interaction_rasterization": "rasterize",
    "display_msaw_markers": "append_alert_markers",
    "display_msaw_refusal": "append_alert_markers",
    "display_msaw_rasterization": "rasterize",
    "safety_state_report": "state_report",
    "safety_net_status": "net_status",
    "safety_inhibit_create": "inhibit_create",
    "safety_inhibit_refusal": "inhibit_create",
    "safety_table_full": "inhibit_create",
    "safety_suppress_log": "inhibit_suppress",
    "safety_suppressed_overflow": "inhibit_suppress",
    "safety_inhibit_remove": "inhibit_remove",
    "safety_inhibit_expire": "inhibit_expire",
    "safety_events_take": "events_take",
    "safety_banner_draw": "build_safety",
    "safety_inhibit_draw": "build_safety",
    "safety_state_backdated": "state_report",
    "safety_state_future_refusal": "state_report",
    "safety_banner_clamp": "build_safety",
    "safety_build_error": "build_safety",
    "route_direct_fix_identity": "convert_route",
    "route_airway_expansion": "convert_route",
    "route_procedure_transition": "convert_route",
    "route_atomic_refusal": "convert_route",
}


def allocation_route_ledger_errors(repo: Path) -> list[str]:
    """Ensure each declared route is anchored in production source and observable evidence."""
    errors: list[str] = []
    for route_id, (relative, field) in ROUTE_LEDGER.items():
        source_path = repo / relative
        try:
            source = _rust_tokens(source_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, ValueError) as exc:
            errors.append(f"route ledger source unavailable for {route_id}: {exc}")
            continue
        symbol = ROUTE_SYMBOLS[route_id]
        if not re.search(rf"\bfn\s+{re.escape(symbol)}\b", source):
            errors.append(f"route ledger production symbol missing for {route_id}: {symbol}")
        if not field:
            errors.append(f"route ledger has no expected evidence field for {route_id}")
    tracker_path = repo / "crates/oa-track-core/src/tracker.rs"
    try:
        tracker_source = tracker_path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        return errors + [f"tracker capacity probes unavailable: {exc}"]
    tracker_decl = re.search(r"pub struct Tracker\s*\{(.*?)\n\}", tracker_source, re.S)
    if tracker_decl is None:
        errors.append("Tracker vector field inventory is unavailable")
    else:
        declared_tracker_vectors = set(re.findall(r"(?m)^\s*(\w+)\s*:\s*Vec\s*<", tracker_decl.group(1)))
        if declared_tracker_vectors != set(TRACKER_VECTOR_FIELDS):
            errors.append("tracker capacity inventory does not match every Tracker Vec field")
    try:
        types_source = (repo / "crates/oa-track-core/src/types.rs").read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        return errors + [f"TrackLog vector inventory unavailable: {exc}"]
    log_decl = re.search(r"pub struct TrackLog\s*\{(.*?)\n\}", types_source, re.S)
    if log_decl is None:
        errors.append("TrackLog vector field inventory is unavailable")
    else:
        declared_log_vectors = set(re.findall(r"(?m)^\s*pub\s+(\w+)\s*:\s*Vec\s*<", log_decl.group(1)))
        if declared_log_vectors != set(TRACK_LOG_VECTOR_FIELDS):
            errors.append("tracker capacity inventory does not match every TrackLog Vec field")
    helper = re.search(
        r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*fn\s+tracker_buffer_capacities\s*\([^)]*\)\s*->\s*\[usize;\s*36\]\s*\{(.*?)\n\}",
        tracker_source,
        re.S,
    )
    if helper is None:
        errors.append("all 36 Tracker and TrackLog Vec capacities must remain in an exact cfg(test) helper")
    else:
        tracker_calls = re.findall(r"\bt\.(?!log\.)(\w+)\.capacity\(\)", helper.group(1))
        log_calls = re.findall(r"\bt\.log\.(\w+)\.capacity\(\)", helper.group(1))
        if (len(tracker_calls) + len(log_calls) != len(TRACKER_VECTOR_FIELDS) + len(TRACK_LOG_VECTOR_FIELDS)
                or len(set(tracker_calls)) != len(TRACKER_VECTOR_FIELDS)
                or set(tracker_calls) != set(TRACKER_VECTOR_FIELDS)
                or len(set(log_calls)) != len(TRACK_LOG_VECTOR_FIELDS)
                or set(log_calls) != set(TRACK_LOG_VECTOR_FIELDS)
                or helper.group(1).count(".capacity()") != 36):
            errors.append("tracker capacity helper must probe all 36 Tracker and TrackLog vectors exactly once")
    unit_probe = re.search(
        r"//\s*REQ:\s*OA-TRK-017\s*#\s*\[\s*test\s*\]\s*fn\s+all_tracker_buffer_capacities_stay_fixed_through_bounded_updates_and_flush\s*\([^)]*\)\s*\{(.*?)//\s*REQ:\s*OA-TRK-355",
        tracker_source,
        re.S,
    )
    if unit_probe is None:
        errors.append("the OA-TRK-017 core capacity-stability test is required alongside the C5 harness")
    else:
        probe = unit_probe.group(1)
        if ("let before = tracker_buffer_capacities(&t);" not in probe
                or "assert_eq!(before, tracker_buffer_capacities(&t));" not in probe
                or "tracker_advance(" not in probe or "tracker_flush(" not in probe
                or "capacity_refused" not in probe):
            errors.append("the OA-TRK-017 test must compare all capacity probes across warm/exhaustion/flush paths")
    return errors


def _inside(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def _read_toml(path: Path) -> tuple[dict, str | None]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8")), None
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        return {}, f"cannot read valid TOML at {path}: {exc}"


def _manifest_path(manifest: Path, raw: str, root: Path) -> tuple[Path | None, str | None]:
    candidate = (manifest.parent / raw).resolve()
    if not _inside(candidate, root):
        return None, f"path dependency escapes repository: {manifest}: {raw}"
    target = (candidate / "Cargo.toml").resolve()
    if not _inside(target, root):
        return None, f"path dependency manifest escapes repository: {manifest}: {raw}"
    return target, None


def _member_manifests(root: Path, workspace: dict) -> tuple[list[Path], list[str]]:
    errors: list[str] = []
    members: set[Path] = set()
    patterns = workspace.get("members", [])
    if not isinstance(patterns, list):
        return [], ["workspace members must be an array of literal paths"]
    for pattern in patterns:
        if not isinstance(pattern, str):
            errors.append("workspace member entries must be strings")
            continue
        member_path = Path(pattern)
        if member_path.is_absolute() or ".." in member_path.parts or any(c in pattern for c in "*?[]"):
            errors.append(f"workspace member must be a repository-relative literal path: {pattern}")
            continue
        candidate = root / member_path
        resolved_candidate = candidate.resolve()
        if not _inside(resolved_candidate, root):
            errors.append(f"workspace member path escapes repository: {pattern}")
            continue
        if resolved_candidate.is_dir():
            manifest = resolved_candidate / "Cargo.toml"
        elif resolved_candidate.name == "Cargo.toml":
            manifest = resolved_candidate
        else:
            errors.append(f"workspace member must name a crate directory: {pattern}")
            continue
        resolved = manifest.resolve()
        if not _inside(resolved, root):
            errors.append(f"workspace member manifest escapes repository: {pattern}")
            continue
        members.add(resolved)
    return sorted(members), errors


def _presentation_exception_errors(root: Path, members: list[Path]) -> list[str]:
    """Check the one exact product package allowed to isolate Linux OS unsafe code."""
    errors: list[str] = []
    declared_dir = root / PRESENTATION
    expected_dir = declared_dir.resolve()
    declared_manifest = declared_dir / "Cargo.toml"
    expected_manifest = declared_manifest.resolve()
    if (declared_dir.is_symlink() or expected_dir != declared_dir
            or not _inside(expected_dir, root) or declared_manifest.is_symlink()):
        errors.append("oa-display-present package path may not use symlinks")
        return errors
    named_members: list[Path] = []
    for manifest in members:
        data, problem = _read_toml(manifest)
        if problem:
            continue
        package = data.get("package", {})
        if isinstance(package, dict) and package.get("name") == "oa-display-present":
            named_members.append(manifest)
    if not expected_manifest.exists():
        if named_members:
            errors.append("oa-display-present may exist only at crates/oa-display-present/Cargo.toml")
        return errors
    if expected_manifest not in members:
        errors.append("crates/oa-display-present must be an explicit product workspace member")
    if any(manifest != expected_manifest for manifest in named_members):
        errors.append("oa-display-present may exist only at crates/oa-display-present/Cargo.toml")
    data, problem = _read_toml(expected_manifest)
    if problem:
        return errors + [problem]
    package = data.get("package", {})
    if not isinstance(package, dict) or package.get("name") != "oa-display-present":
        errors.append("the exact presentation package path must declare name = oa-display-present")
        package = {}
    for key in ("version", "edition", "license", "rust-version", "authors"):
        package_value = package.get(key)
        if not isinstance(package_value, dict) or package_value.get("workspace") is not True:
            errors.append(f"oa-display-present package.{key} must inherit workspace.package")
    metadata = package.get("metadata", {}) if isinstance(package, dict) else {}
    openatca = metadata.get("openatca", {}) if isinstance(metadata, dict) else {}
    if not isinstance(openatca, dict) or openatca.get("class") != "shell":
        errors.append("oa-display-present must have package.metadata.openatca.class = shell")
    lint_tables = data.get("lints", {})
    if not isinstance(lint_tables, dict):
        lint_tables = {}
    if lint_tables.get("workspace") is True:
        errors.append("oa-display-present must not inherit workspace lints (forbid blocks its one private OS module)")
    rust_lints = lint_tables.get("rust", {})
    if not isinstance(rust_lints, dict) or rust_lints.get("unsafe_code") != "deny":
        errors.append("oa-display-present must explicitly deny the unsafe_code Rust lint")
    clippy_lints = lint_tables.get("clippy", {})
    if (not isinstance(clippy_lints, dict)
            or set(clippy_lints) != set(STRICT_RUNTIME_CLIPPY_LINTS)
            or any(clippy_lints.get(name) != "deny" for name in STRICT_RUNTIME_CLIPPY_LINTS)):
        errors.append("oa-display-present must explicitly deny the four strict workspace Clippy lints")
    if any(data.get(section) for section in ("dependencies", "dev-dependencies", "build-dependencies", "target")):
        errors.append("oa-display-present must remain a zero-dependency package")

    source_paths = sorted(expected_dir.rglob("*.rs"))
    source_names: set[str] = set()
    source_text: dict[str, str] = {}
    for source in source_paths:
        relative = source.relative_to(expected_dir).as_posix()
        if source.is_symlink() or not _inside(source.resolve(), expected_dir):
            errors.append(f"oa-display-present Rust source escapes its package: {relative}")
            continue
        source_names.add(relative)
        try:
            source_text[relative] = source.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as exc:
            errors.append(f"cannot read oa-display-present Rust source {relative}: {exc}")
    if source_names != PRESENTATION_SOURCES:
        errors.append("oa-display-present Rust source inventory must be exactly src/lib.rs and src/linux.rs")
    lib_tokens = _rust_tokens(source_text.get("src/lib.rs", ""))
    linux_tokens = _rust_tokens(source_text.get("src/linux.rs", ""))
    if len(re.findall(r"#!\s*\[\s*deny\s*\(\s*unsafe_code\s*\)\s*\]", lib_tokens)) != 1:
        errors.append("oa-display-present src/lib.rs must declare exactly one #![deny(unsafe_code)]")
    if len(re.findall(r"\bmod\s+linux\s*;", lib_tokens)) != 1 or re.search(r"\bpub\s+mod\s+linux\b", lib_tokens):
        errors.append("oa-display-present must expose linux.rs only as a private module")
    outside_os_forbidden = re.compile(
        r"\b(?:unsafe|extern|no_mangle|export_name|link_section)\b|"
        r"\ballow\s*\([^]]*\bunsafe_code\b|#\s*\[\s*link\b|#\s*\[\s*path\b|"
        r"\bpath\s*=|\binclude(?:_str|_bytes)?\s*!"
    )
    if outside_os_forbidden.search(lib_tokens):
        errors.append("oa-display-present src/lib.rs may not contain OS escape tokens or attributes")
    if len(re.findall(r"#!\s*\[\s*allow\s*\(\s*unsafe_code\s*\)\s*\]", linux_tokens)) != 1:
        errors.append("oa-display-present src/linux.rs must contain exactly one module-level unsafe allowance")
    all_allow_attrs = re.findall(r"\ballow\s*\([^]]*\bunsafe_code\b[^]]*\)", linux_tokens)
    if len(all_allow_attrs) != 1:
        errors.append("oa-display-present src/linux.rs may contain no nested unsafe_code allowances")
    if re.search(r"#\s*\[\s*link\b|#\s*\[\s*path\b|\bpath\s*=|"
                 r"\b(?:no_mangle|export_name|link_section)\b|\binclude(?:_str|_bytes)?\s*!", linux_tokens):
        errors.append("oa-display-present may not use source inclusion or path attributes")

    return errors


def _core_ffi_exception_errors(root: Path, members: list[Path]) -> list[str]:
    """Check the exact provisional C ABI package boundary required by RULESET A0."""
    errors: list[str] = []
    declared_dir = root / CORE_FFI
    expected_dir = declared_dir.resolve()
    declared_manifest = declared_dir / "Cargo.toml"
    expected_manifest = declared_manifest.resolve()
    if (declared_dir.is_symlink() or expected_dir != declared_dir
            or not _inside(expected_dir, root) or declared_manifest.is_symlink()):
        errors.append("openatca-core-ffi package path may not use symlinks")
        return errors

    named_members: list[Path] = []
    for manifest in members:
        data, problem = _read_toml(manifest)
        if problem:
            continue
        package = data.get("package", {})
        if isinstance(package, dict) and package.get("name") == "openatca-core-ffi":
            named_members.append(manifest)
    if not expected_manifest.exists():
        if named_members:
            errors.append("openatca-core-ffi may exist only at crates/openatca-core-ffi/Cargo.toml")
        return errors
    if expected_manifest not in members:
        errors.append("crates/openatca-core-ffi must be an explicit product workspace member")
    if any(manifest != expected_manifest for manifest in named_members):
        errors.append("openatca-core-ffi may exist only at crates/openatca-core-ffi/Cargo.toml")

    data, problem = _read_toml(expected_manifest)
    if problem:
        return errors + [problem]
    package = data.get("package", {})
    if not isinstance(package, dict) or package.get("name") != "openatca-core-ffi":
        errors.append("the exact FFI package path must declare name = openatca-core-ffi")
        package = {}
    for key in ("version", "edition", "license", "rust-version", "authors"):
        package_value = package.get(key)
        if not isinstance(package_value, dict) or package_value.get("workspace") is not True:
            errors.append(f"openatca-core-ffi package.{key} must inherit workspace.package")
    metadata = package.get("metadata", {}) if isinstance(package, dict) else {}
    openatca = metadata.get("openatca", {}) if isinstance(metadata, dict) else {}
    if not isinstance(openatca, dict) or openatca.get("class") != "shell":
        errors.append("openatca-core-ffi must have package.metadata.openatca.class = shell")

    lint_tables = data.get("lints", {})
    if not isinstance(lint_tables, dict):
        lint_tables = {}
    if lint_tables.get("workspace") is True:
        errors.append("openatca-core-ffi must not inherit workspace lints (A0 requires local unsafe_code deny)")
    rust_lints = lint_tables.get("rust", {})
    if not isinstance(rust_lints, dict) or rust_lints != {"unsafe_code": "deny"}:
        errors.append("openatca-core-ffi must explicitly set only [lints.rust] unsafe_code = deny")
    clippy_lints = lint_tables.get("clippy", {})
    if (not isinstance(clippy_lints, dict)
            or set(clippy_lints) != set(STRICT_RUNTIME_CLIPPY_LINTS)
            or any(clippy_lints.get(name) != "deny" for name in STRICT_RUNTIME_CLIPPY_LINTS)):
        errors.append("openatca-core-ffi must explicitly deny the four strict workspace Clippy lints")

    lib = data.get("lib", {})
    if not isinstance(lib, dict) or lib.get("crate-type") != CORE_FFI_CRATE_TYPES:
        errors.append("openatca-core-ffi must expose exactly cdylib, staticlib and rlib crate types")
    actual_deps = data.get("dependencies", {})
    if not isinstance(actual_deps, dict) or set(actual_deps) != set(CORE_FFI_DEPS):
        errors.append("openatca-core-ffi dependencies must be exactly the four reviewed core crates and oa-csv, oa-json, oa-xml")
        actual_deps = actual_deps if isinstance(actual_deps, dict) else {}
    for name, relative in CORE_FFI_DEPS.items():
        declaration = actual_deps.get(name)
        if not isinstance(declaration, dict) or declaration.get("path") != relative:
            errors.append(f"openatca-core-ffi dependency {name} must use the exact core path {relative}")
    for section in ("dev-dependencies", "build-dependencies", "target"):
        if data.get(section):
            errors.append(f"openatca-core-ffi must not add {section}")

    source_dir = declared_dir / "src"
    package_dir = declared_dir.resolve()
    if source_dir.is_symlink() or not source_dir.is_dir() or not _inside(source_dir.resolve(), package_dir):
        errors.append("openatca-core-ffi source directory must be a real in-package directory")
    else:
        actual_sources: set[str] = set()
        for source in source_dir.rglob("*.rs"):
            relative = source.relative_to(source_dir).as_posix()
            if source.is_symlink() or not _inside(source.resolve(), package_dir):
                errors.append(f"openatca-core-ffi Rust source may not use symlinks or escape its package: {relative}")
            actual_sources.add(relative)
        if actual_sources != CORE_FFI_SOURCE_FILES:
            errors.append("openatca-core-ffi Rust source inventory must be exactly src/lib.rs, src/ffi.rs and src/text.rs")
        for relative in CORE_FFI_SOURCE_FILES:
            source = source_dir / relative
            if source.is_symlink() or not source.is_file() or not _inside(source.resolve(), package_dir):
                errors.append(f"openatca-core-ffi required source must be a regular in-package file: src/{relative}")
        text_source = source_dir / "text.rs"
        try:
            text_tokens = _rust_tokens(text_source.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, ValueError):
            text_tokens = ""
        if len(re.findall(r"#!\s*\[\s*forbid\s*\(\s*unsafe_code\s*\)\s*\]", text_tokens)) != 1:
            errors.append("openatca-core-ffi src/text.rs must retain exactly one module-level unsafe-code prohibition")

    return errors


def _wasm_abi_exception_errors(root: Path, members: list[Path]) -> list[str]:
    """Mirror only the reviewed shell lint exception for the fixed scalar WASM ABI package."""
    errors: list[str] = []
    declared_dir = root / WASM_ABI
    expected_dir = declared_dir.resolve()
    declared_manifest = declared_dir / "Cargo.toml"
    expected_manifest = declared_manifest.resolve()
    if (declared_dir.is_symlink() or expected_dir != declared_dir
            or not _inside(expected_dir, root) or declared_manifest.is_symlink()):
        errors.append("oa-demo-wasm package path may not use symlinks")
        return errors

    named_members: list[Path] = []
    for manifest in members:
        data, problem = _read_toml(manifest)
        if problem:
            continue
        package = data.get("package", {})
        if isinstance(package, dict) and package.get("name") == "oa-demo-wasm":
            named_members.append(manifest)
    if not expected_manifest.exists():
        if named_members:
            errors.append("oa-demo-wasm may exist only at tools/oa-demo-wasm/Cargo.toml")
        return errors
    if expected_manifest not in members:
        errors.append("tools/oa-demo-wasm must be an explicit product workspace member")
    if any(manifest != expected_manifest for manifest in named_members):
        errors.append("oa-demo-wasm may exist only at tools/oa-demo-wasm/Cargo.toml")

    data, problem = _read_toml(expected_manifest)
    if problem:
        return errors + [problem]
    package = data.get("package", {})
    if not isinstance(package, dict) or package.get("name") != "oa-demo-wasm":
        errors.append("the exact WASM ABI path must declare name = oa-demo-wasm")
        package = {}
    for key in ("version", "edition", "license", "rust-version", "authors"):
        package_value = package.get(key)
        if not isinstance(package_value, dict) or package_value.get("workspace") is not True:
            errors.append(f"oa-demo-wasm package.{key} must inherit workspace.package")
    metadata = package.get("metadata", {}) if isinstance(package, dict) else {}
    openatca = metadata.get("openatca", {}) if isinstance(metadata, dict) else {}
    if not isinstance(openatca, dict) or openatca.get("class") != "shell":
        errors.append("oa-demo-wasm must have package.metadata.openatca.class = shell")

    lint_tables = data.get("lints", {})
    if not isinstance(lint_tables, dict):
        lint_tables = {}
    if lint_tables.get("workspace") is True:
        errors.append("oa-demo-wasm must not inherit workspace lints (its exact ABI exception is locally denied)")
    rust_lints = lint_tables.get("rust", {})
    if (not isinstance(rust_lints, dict) or set(rust_lints) != {"unsafe_code"}
            or rust_lints.get("unsafe_code") != "deny"):
        errors.append("oa-demo-wasm must locally deny only the unsafe_code Rust lint")
    clippy_lints = lint_tables.get("clippy", {})
    if (not isinstance(clippy_lints, dict)
            or set(clippy_lints) != set(STRICT_RUNTIME_CLIPPY_LINTS)
            or any(clippy_lints.get(name) != "deny" for name in STRICT_RUNTIME_CLIPPY_LINTS)):
        errors.append("oa-demo-wasm must locally deny exactly the four strict workspace Clippy lints")
    return errors


def _product_global_allocator_errors(members: list[Path]) -> list[str]:
    """The allocator harness is excluded; product source may never name GlobalAlloc."""
    errors: list[str] = []
    for manifest in members:
        package_dir = manifest.parent
        for source in package_dir.rglob("*.rs"):
            if source.is_symlink() or not _inside(source.resolve(), package_dir.resolve()):
                errors.append(f"product Rust source escapes its package: {source}")
                continue
            try:
                tokens = _rust_tokens(source.read_text(encoding="utf-8"))
            except (OSError, UnicodeError, ValueError) as exc:
                errors.append(f"cannot scan product Rust source {source}: {exc}")
                continue
            if re.search(r"\bGlobalAlloc\b", tokens) or re.search(
                r"#\s*\[\s*global_allocator\s*\]", tokens
            ):
                errors.append(f"product source may not implement or name GlobalAlloc: {source}")
    return errors


def _all_deps(manifest: dict):
    """Yield dependency declarations, including target-specific tables."""
    for section in ("dependencies", "dev-dependencies", "build-dependencies"):
        values = manifest.get(section, {})
        if isinstance(values, dict):
            yield from values.items()
    workspace = manifest.get("workspace", {})
    if isinstance(workspace, dict) and isinstance(workspace.get("dependencies"), dict):
        yield from workspace["dependencies"].items()
    targets = manifest.get("target", {})
    if isinstance(targets, dict):
        for target in targets.values():
            if not isinstance(target, dict):
                continue
            for section in ("dependencies", "dev-dependencies", "build-dependencies"):
                values = target.get(section, {})
                if isinstance(values, dict):
                    yield from values.items()


def _local_dependency_graph(start: list[Path], root: Path, errors: list[str]) -> set[Path]:
    seen: set[Path] = set()
    pending = list(start)
    while pending:
        manifest_path = pending.pop()
        if manifest_path in seen:
            continue
        if not _inside(manifest_path, root):
            errors.append(f"manifest escapes repository: {manifest_path}")
            continue
        seen.add(manifest_path)
        data, problem = _read_toml(manifest_path)
        if problem:
            errors.append(problem)
            continue
        for alias, declaration in _all_deps(data):
            if not isinstance(declaration, dict) or "path" not in declaration:
                continue
            raw = declaration.get("path")
            if not isinstance(raw, str):
                errors.append(f"invalid path dependency in {manifest_path}: {alias}")
                continue
            target, problem = _manifest_path(manifest_path, raw, root)
            if problem:
                errors.append(problem)
            elif target is not None:
                pending.append(target)
    return seen


def _check_dep_backedges(manifests: set[Path], harness_dir: Path, root: Path,
                         errors: list[str]) -> None:
    for manifest_path in manifests:
        data, problem = _read_toml(manifest_path)
        if problem:
            errors.append(problem)
            continue
        for alias, declaration in _all_deps(data):
            package = declaration.get("package") if isinstance(declaration, dict) else None
            if alias == "oa-alloc-check" or package == "oa-alloc-check":
                errors.append(f"product dependency names allocator harness: {manifest_path} ({alias})")
            if not isinstance(declaration, dict) or "path" not in declaration:
                continue
            raw = declaration.get("path")
            if not isinstance(raw, str):
                continue
            target, problem = _manifest_path(manifest_path, raw, root)
            if problem:
                errors.append(problem)
            elif target is not None and _inside(target.parent, harness_dir):
                errors.append(f"product dependency points back to allocator harness: {manifest_path} ({alias})")


_RAW_STRING_START = re.compile(r'(?:br|cr|r)(#*)"')


def _rust_tokens(source: str) -> str:
    """Discard comments and string/char contents before token-level source checks."""
    # A small lexical scrubber avoids false hits in comments/strings while still
    # being deliberately fail-closed when braces or quotes are malformed.
    out: list[str] = []
    i = 0
    state = "code"
    depth = 0
    while i < len(source):
        if state == "code":
            if source.startswith("//", i):
                state = "line"
                out.extend("  ")
                i += 2
            elif source.startswith("/*", i):
                state, depth = "block", 1
                out.extend("  ")
                i += 2
            elif (raw := _RAW_STRING_START.match(source, i)) is not None:
                closing = '"' + raw.group(1)
                end = source.find(closing, raw.end())
                if end < 0:
                    raise ValueError("unterminated Rust raw string")
                end += len(closing)
                out.extend("\n" if char == "\n" else " " for char in source[i:end])
                i = end
            elif source[i] == '"':
                quote = source[i]
                state = "string"
                out.append(" ")
                i += 1
            elif source[i] == "'":
                # Keep Rust lifetimes such as 'static as tokens; blank only a
                # character literal whose closing quote is present.
                end = i + 1
                if end < len(source) and source[end] == "\\":
                    end += 2
                else:
                    end += 1
                if end < len(source) and source[end] == "'":
                    out.extend(" " * (end + 1 - i))
                    i = end + 1
                else:
                    out.append(source[i])
                    i += 1
            else:
                out.append(source[i])
                i += 1
        elif state == "line":
            if source[i] == "\n":
                state = "code"
                out.append("\n")
            else:
                out.append(" ")
            i += 1
        elif state == "block":
            if source.startswith("/*", i):
                depth += 1
                out.extend("  ")
                i += 2
            elif source.startswith("*/", i):
                depth -= 1
                out.extend("  ")
                i += 2
                if depth == 0:
                    state = "code"
            else:
                out.append("\n" if source[i] == "\n" else " ")
                i += 1
        else:
            if source[i] == "\\":
                out.extend("  ")
                i += min(2, len(source) - i)
            elif source[i] == quote:
                out.append(" ")
                i += 1
                state = "code"
            else:
                out.append("\n" if source[i] == "\n" else " ")
                i += 1
    if state not in ("code", "line"):
        raise ValueError("unterminated Rust string or comment")
    return "".join(out)


def allocation_layout_errors(repo: Path) -> list[str]:
    """Return source-layout violations for the standalone allocator harness."""
    errors: list[str] = []
    root = Path(repo).resolve()
    root_manifest = (root / "Cargo.toml").resolve()
    if not _inside(root_manifest, root):
        return ["root Cargo.toml escapes repository"]
    workspace, problem = _read_toml(root_manifest)
    if problem:
        return [problem]

    workspace_table = workspace.get("workspace", {})
    if not isinstance(workspace_table, dict):
        return ["root workspace table must be a TOML table"]
    root_lints = workspace_table.get("lints", {})
    if not isinstance(root_lints, dict):
        root_lints = {}
    root_rust_lints = root_lints.get("rust", {})
    if not isinstance(root_rust_lints, dict):
        root_rust_lints = {}
    if root_rust_lints.get("unsafe_code") != "forbid":
        errors.append("root workspace rust unsafe_code lint must remain forbid")
    root_clippy_lints = root_lints.get("clippy", {})
    if (not isinstance(root_clippy_lints, dict)
            or any(root_clippy_lints.get(name) != "deny" for name in STRICT_RUNTIME_CLIPPY_LINTS)):
        errors.append("root workspace must keep all four strict runtime Clippy lints denied")

    members, member_errors = _member_manifests(root, workspace.get("workspace", {}))
    errors.extend(member_errors)
    errors.extend(_product_global_allocator_errors(members))
    errors.extend(_presentation_exception_errors(root, members))
    errors.extend(_core_ffi_exception_errors(root, members))
    errors.extend(_wasm_abi_exception_errors(root, members))
    presentation_manifest = (root / PRESENTATION / "Cargo.toml").resolve()
    core_ffi_manifest = (root / CORE_FFI / "Cargo.toml").resolve()
    wasm_abi_manifest = (root / WASM_ABI / "Cargo.toml").resolve()
    for member in members:
        if not _inside(member, root):
            errors.append(f"workspace member manifest escapes repository: {member}")
            continue
        member_data, member_problem = _read_toml(member)
        if member_problem:
            errors.append(member_problem)
        elif member not in {presentation_manifest, core_ffi_manifest, wasm_abi_manifest} and member_data.get("lints", {}).get("workspace") is not True:
            errors.append(f"product workspace member must inherit workspace lints: {member}")
    harness_dir = (root / HARNESS).resolve()
    if not _inside(harness_dir, root):
        return errors + ["allocator harness path escapes repository"]
    excludes = workspace_table.get("exclude", [])
    if not isinstance(excludes, list) or any(not isinstance(p, str) for p in excludes):
        errors.append("workspace exclude entries must be strings")
        excludes = []
    for pattern in excludes:
        relative = Path(pattern)
        if relative.is_absolute() or ".." in relative.parts or any(c in pattern for c in "*?[]"):
            errors.append(f"workspace exclude must be a repository-relative literal path: {pattern}")
    if HARNESS.as_posix() not in excludes:
        errors.append("allocator harness must be excluded from the product workspace")
    if any(member.parent == harness_dir or _inside(harness_dir, member.parent) for member in members):
        errors.append("allocator harness must not be a product workspace member")

    graph = _local_dependency_graph([root_manifest, *members], root, errors)
    _check_dep_backedges(graph, harness_dir, root, errors)

    manifest_path = (harness_dir / "Cargo.toml").resolve()
    if not _inside(manifest_path, root):
        return errors + ["allocator harness Cargo.toml escapes repository"]
    harness, problem = _read_toml(manifest_path)
    if problem:
        return errors + [problem]
    package = harness.get("package", {})
    if not isinstance(package, dict) or package.get("publish") is not False:
        errors.append("allocator harness package must set publish = false")
    if "workspace" not in harness:
        errors.append("allocator harness must declare its own [workspace]")
    lints = harness.get("lints", {})
    clippy = lints.get("clippy", {}) if isinstance(lints, dict) else {}
    if (not isinstance(clippy, dict) or any(clippy.get(name) != "deny" for name in
            ("unwrap_used", "expect_used", "panic", "indexing_slicing"))):
        errors.append("allocator harness must deny the four strict runtime lints")
    deps = harness.get("dependencies", {})
    if not isinstance(deps, dict) or set(deps) != set(HARNESS_DEPS):
        errors.append("allocator harness may declare only its declared local crate dependencies")
    else:
        for name, crate_path in HARNESS_DEPS.items():
            if (not isinstance(deps[name], dict) or deps[name].get("path") != crate_path
                    or set(deps[name]) != {"path"}):
                errors.append(f"allocator harness {name} dependency must be the local crate path")
    for section in ("dev-dependencies", "build-dependencies", "target"):
        if harness.get(section):
            errors.append(f"allocator harness may not declare {section}")

    src_dir = harness_dir / "src"
    src_real = src_dir.resolve()
    if src_dir.exists() and not _inside(src_real, root):
        return errors + ["allocator harness src directory escapes repository"]
    src_sources = sorted(p.name for p in src_real.glob("*.rs")) if src_real.is_dir() else []
    root_sources = sorted(p.name for p in harness_dir.glob("*.rs"))
    if (set(src_sources) >= {"allocator.rs", "main.rs"}
            and set(src_sources) <= {"allocator.rs", "main.rs", *HELPER_MODULES}):
        base = src_dir
    elif root_sources == ["allocator.rs", "main.rs"] and not src_dir.exists():
        base = harness_dir
    else:
        errors.append("allocator harness source files must be main.rs, allocator.rs and optional pending_windows.rs, nam_window.rs and display_window.rs")
        base = src_dir if src_dir.is_dir() else harness_dir
    try:
        if base.is_dir() and any(p.is_symlink() or p.is_dir() for p in base.iterdir()):
            errors.append("allocator harness source directories may not contain nested directories")
    except OSError as exc:
        errors.append(f"cannot inspect allocator harness source directory: {exc}")
    main_path, allocator_path = base / "main.rs", base / "allocator.rs"
    if any(not _inside(p.resolve(), root) for p in (main_path, allocator_path)):
        return errors + ["allocator harness source file escapes repository"]
    if main_path.is_file() and allocator_path.is_file():
        try:
            main = _rust_tokens(main_path.read_text(encoding="utf-8"))
            allocator = _rust_tokens(allocator_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, ValueError) as exc:
            errors.append(f"cannot read allocator harness source: {exc}")
        else:
            if not re.search(r"#!\s*\[\s*deny\s*\(\s*unsafe_code\s*\)\s*\]", main):
                errors.append("main.rs must begin with #![deny(unsafe_code)]")
            if re.search(r"\bunsafe\b", main):
                errors.append("main.rs may not contain unsafe tokens")
            if re.search(r"#\s*!?\s*\[\s*allow\s*\([^]]*\bunsafe_code\b", main):
                errors.append("main.rs may not allow unsafe_code")
            if not re.search(r"#!\s*\[\s*allow\s*\(\s*unsafe_code\s*\)\s*\]", allocator):
                errors.append("allocator.rs must locally allow unsafe_code")
            if len(re.findall(r"#!\s*\[\s*allow\s*\(\s*unsafe_code\s*\)\s*\]", allocator)) != 1:
                errors.append("allocator.rs must contain exactly one local unsafe_code allowance")
            all_allows = re.findall(r"#\s*!?\s*\[\s*allow\s*\([^]]*\bunsafe_code\b[^]]*\)\s*\]", allocator)
            if len(all_allows) != 1:
                errors.append("allocator.rs may contain only its module-level unsafe_code allowance")
            if len(re.findall(r"#\s*\[\s*global_allocator\s*\]", allocator)) != 1:
                errors.append("allocator.rs must contain exactly one global allocator attribute")
            if not re.search(r"\bunsafe\s+impl\s+(?:<[^>]*>\s*)?GlobalAlloc\b", allocator):
                errors.append("allocator.rs must contain the unsafe GlobalAlloc implementation")
    for helper_name in HELPER_MODULES:
        pending_path = base / helper_name
        if pending_path.exists():
            if pending_path.is_symlink() or not _inside(pending_path.resolve(), root):
                errors.append(f"{helper_name} escapes its source boundary")
            else:
                try:
                    pending = _rust_tokens(pending_path.read_text(encoding="utf-8"))
                except (OSError, UnicodeError, ValueError) as exc:
                    errors.append(f"cannot read {helper_name}: {exc}")
                else:
                    if not re.search(r"#!\s*\[\s*forbid\s*\(\s*unsafe_code\s*\)\s*\]", pending):
                        errors.append(f"{helper_name} must forbid unsafe_code")
                    if re.search(r"\bunsafe\b", pending):
                        errors.append(f"{helper_name} may not contain unsafe tokens")
                    if re.search(r"#\s*!?\s*\[\s*allow\s*\([^]]*\bunsafe_code\b", pending):
                        errors.append(f"{helper_name} may not allow unsafe_code")
    return errors


def _nonnegative_int(value) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def _object(value) -> bool:
    return isinstance(value, dict)


def evaluate_allocator_result(text: str, expect_injected: bool = False) -> list[str]:
    """Validate one JSON result from the bounded M3 allocator harness."""
    errors: list[str] = []
    if not isinstance(expect_injected, bool):
        return ["expect_injected must be a boolean"]
    try:
        result = json.loads(text)
    except (TypeError, ValueError, json.JSONDecodeError) as exc:
        return [f"result is not valid complete JSON: {exc}"]
    if not _object(result):
        return ["result must be a JSON object"]
    required_keys = {"status", "injection_requested", "fixture_ids", "route_ids", "controls",
                     "tracker", "default_capacity", "output_log", "clock_gate",
                     "bucket_capacity", "adsb_pending", "plot_pending", "fusion", "nam_session",
                     "display_frame", "safety", "route_conversion"}
    if set(result) != required_keys:
        errors.append("result must contain exactly the required allocator evidence fields")
    expected_status = "FAIL_M3_ALLOCATION" if expect_injected else "PASS_M3_EXERCISED"
    if result.get("status") != expected_status:
        errors.append(f"status must be {expected_status}")
    if result.get("injection_requested") is not expect_injected:
        errors.append("injection_requested does not match requested mode")

    fixture_ids = result.get("fixture_ids")
    if (not isinstance(fixture_ids, list) or len(fixture_ids) != len(FIXTURE_IDS)
            or any(not isinstance(item, str) for item in fixture_ids)
            or len(set(fixture_ids)) != len(FIXTURE_IDS) or set(fixture_ids) != FIXTURE_IDS):
        errors.append("fixture_ids must contain exactly the required unique fixtures")

    route_ids = result.get("route_ids")
    if (not isinstance(route_ids, list) or len(route_ids) != len(ROUTE_LEDGER)
            or any(not isinstance(item, str) for item in route_ids)
            or len(set(route_ids)) != len(ROUTE_LEDGER) or set(route_ids) != set(ROUTE_LEDGER)):
        errors.append("route_ids must contain exactly the unique source-backed allocation routes")
    for route_id, (_, field) in ROUTE_LEDGER.items():
        if "." in field:
            group, name = field.split(".", 1)
        else:
            group = "tracker" if route_id.startswith("tracker_") else "fusion"
            name = field
        evidence = result.get(group)
        value = evidence.get(name) if isinstance(evidence, dict) else None
        if value is not True and (not _nonnegative_int(value) or value < 1):
            errors.append(f"{group}.{name} must positively demonstrate route {route_id}")

    controls = result.get("controls")
    control_names = {"alloc", "zeroed", "realloc"}
    if not _object(controls) or set(controls) != control_names | {"overflow"}:
        errors.append("controls must contain exactly alloc, zeroed, realloc, and overflow")
        controls = {}
    if controls.get("overflow") is not False:
        errors.append("controls.overflow must be false")
    for name in control_names:
        value = controls.get(name)
        if not _nonnegative_int(value):
            errors.append(f"controls.{name} must be a nonnegative JSON integer")
    if any(controls.get(name) != count for name, count in {"alloc": 2, "zeroed": 1, "realloc": 1}.items()):
        errors.append("controls must report alloc=2, zeroed=1, realloc=1")

    tracker = result.get("tracker")
    if not _object(tracker):
        errors.append("tracker must be an object")
        tracker = {}
    for name in control_names:
        value = tracker.get(name)
        if not _nonnegative_int(value):
            errors.append(f"tracker.{name} must be a nonnegative JSON integer")
        elif not expect_injected and value != 0:
            errors.append(f"normal tracker.{name} must be zero")
        elif expect_injected and name == "alloc" and value < 1:
            errors.append("injected tracker.alloc must be at least one")
    if tracker.get("overflow") is not False:
        errors.append("tracker.overflow must be false")
    for name, minimum in TRACKER_FIELDS.items():
        value = tracker.get(name)
        if not _nonnegative_int(value):
            errors.append(f"tracker.{name} must be a nonnegative JSON integer")
        elif value < minimum:
            errors.append(f"tracker.{name} is below required exercised coverage")

    fusion = result.get("fusion")
    if not _object(fusion):
        errors.append("fusion must be an object")
        fusion = {}
    for name in control_names:
        value = fusion.get(name)
        if not _nonnegative_int(value):
            errors.append(f"fusion.{name} must be a nonnegative JSON integer")
        elif not expect_injected and value != 0:
            errors.append(f"normal fusion.{name} must be zero")
        elif expect_injected and name == "alloc" and value < 1:
            errors.append("injected fusion.alloc must be at least one")
    if fusion.get("overflow") is not False:
        errors.append("fusion.overflow must be false")
    for name, minimum in FUSION_FIELDS.items():
        value = fusion.get(name)
        if not _nonnegative_int(value):
            errors.append(f"fusion.{name} must be a nonnegative JSON integer")
        elif value < minimum:
            errors.append(f"fusion.{name} is below required exercised coverage")

    extra_fields = {
        "route_conversion": {"iterations": 10, "direct_points": 2, "airway_points": 2,
                             "procedure_points": 2, "connected_links": 4, "refusals": 1,
                             "recoveries": 1, "work_used": 1, "work_limit": ROUTE_CONVERSION_WORK_LIMIT},
        "default_capacity": {"occupied": 4096, "confirmed": 4096, "refused": 1, "recovered": 1,
                             "history_refiltered": 1, "history_replayed": 1, "late_ring": 1},
        "output_log": {"output_overflow": 1, "output_rows_after_reuse": 1,
                        "log_overflow": 1, "log_rows": 1},
        "clock_gate": {"held": 1, "held_applied": 1, "step_forward": 1,
                       "late": 5, "step_back": 1, "future_dropped": 1, "recovered": 1},
        "bucket_capacity": {"plots_offered": 16_386, "bucket_plot_overflow": 1,
                            "known_hard_applied": 1, "unmatched_hard_no_track": 16_383,
                            "recovery_applied": 1},
        "nam_session": {"irq_sent": 3, "atm_sent": 129, "resent": 1, "timed_out": 1,
                        "delivered": 1, "responses": 1, "asm_sent": 1, "peer_silent": 1,
                        "lrm_sent": 1, "trq_sent": 1, "abandoned": 128, "peak_pending": 128,
                        "pending_full": 1, "discards": 1, "dropped": 0, "script_errors": 0},
        "display_frame": {"convert_added": 1, "convert_ended": 1, "traffic_drawn": 30,
                          "uncertain": 1, "cs_conflict": 1, "r95_unknown": 1, "refused": 1,
                          "alt_clamped": 1, "malformed": 1, "frame_drawn": 50,
                          "scene_items": 1, "refusing_refused": 1,
                          "convert_invalid_position": 1, "convert_unprojectable": 1,
                          "convert_trail_dropped": 1, "convert_time_overflow": 1,
                          "draw_trail_dropped": 1, "stamp_clipped": 1, "digests": 20,
                          "raw_drawn": 1, "raw_capacity_refused": 1,
                          "raw_invalid_refused": 1, "interaction_selected": 1,
                          "interaction_refused": 1, "interaction_queued": 1,
                          "interaction_preflighted": 1, "interaction_completed": 1,
                          "interaction_presented": 1, "interaction_rasterized": 1,
                          "msaw_presented": 20, "msaw_refused": 40, "msaw_rasterized": 20},
        "safety": {"state_reports": 4, "stale_unavailable": 4, "created": 3, "refused": 3,
                   "refused_full": 1, "suppressed_logged": 4096, "suppressed_overflow": 3,
                   "removed": 1, "expired": 1, "events": 1, "banner_lines": 160,
                   "inhibitions_drawn": 20, "inhibitions_unplaced": 20, "state_backdated": 1,
                   "state_future_refused": 1, "banner_hidden": 20, "error_builds": 20},
        "adsb_pending": {"offered": 1_048_577, "pending_overflow": 1,
                         "drained": 1_048_576, "bucket_plot_overflow": 0, "recovery_applied": 1,
                         "tracker_applied": 1, "tracker_duplicates": 1_048_575,
                         "tracker_refused": 0, "tracker_capacity_refused": 0,
                         "tracker_late_window": 0, "tracker_late_ring": 0,
                         "tracker_jump_rejected": 0, "tracker_numerical_refused": 0,
                         "hard_extra": 0, "hard_mismatch": 0},
        "plot_pending": {"offered": 1_048_577, "pending_overflow": 1,
                         "drained": 1_048_576, "bucket_plot_overflow": 1_032_192,
                         "recovery_applied": 1, "tracker_applied": 1,
                         "tracker_duplicates": 0, "tracker_refused": 0,
                         "tracker_capacity_refused": 0, "tracker_late_window": 0,
                         "tracker_late_ring": 0, "hard_extra": 16_383,
                         "tracker_jump_rejected": 0, "tracker_numerical_refused": 0,
                         "hard_mismatch": 0},
    }
    for group, minima in extra_fields.items():
        evidence = result.get(group)
        required = set(minima) | {"overflow", "alloc", "zeroed", "realloc"}
        if group == "nam_session":
            required |= {"final_inactive"}
        if group == "output_log":
            required |= {"output_aligned", "output_preserved_addr", "output_payload_unchanged", "log_aligned"}
        if group == "route_conversion":
            required |= {"atomic_empty", "injected"}
        if not _object(evidence) or set(evidence) != required:
            errors.append(f"{group} must contain exactly its declared route evidence and allocator counters")
            evidence = {}
        if evidence.get("overflow") is not False:
            errors.append(f"{group}.overflow must be false")
        for counter in control_names:
            value = evidence.get(counter)
            if not _nonnegative_int(value):
                errors.append(f"{group}.{counter} must be a nonnegative JSON integer")
            elif not expect_injected and value != 0:
                errors.append(f"normal {group}.{counter} must be zero")
            elif expect_injected and counter == "alloc" and value < 1:
                errors.append(f"injected {group}.alloc must be at least one")
        for name, minimum in minima.items():
            value = evidence.get(name)
            if not _nonnegative_int(value):
                errors.append(f"{group}.{name} must be a nonnegative JSON integer")
            elif value < minimum:
                errors.append(f"{group}.{name} is below required exercised coverage")
    exact_fields = {
        "route_conversion": {"iterations": 10, "direct_points": 2, "airway_points": 2,
                             "procedure_points": 2, "connected_links": 4, "refusals": 1,
                             "atomic_empty": True, "recoveries": 1, "work_limit": ROUTE_CONVERSION_WORK_LIMIT,
                             "injected": True},
        "default_capacity": {"occupied": 4096, "confirmed": 4096, "refused": 1,
                             "history_refiltered": 1, "history_replayed": 1, "late_ring": 1},
        "output_log": {"log_overflow": 1, "log_rows": 1},
        "display_frame": {"traffic_drawn": 64, "refused": 100, "malformed": 20, "digests": 20,
                          "raw_drawn": 20, "raw_capacity_refused": 40,
                          "raw_invalid_refused": 20, "interaction_selected": 20,
                          "interaction_refused": 20, "interaction_queued": 20,
                          "interaction_preflighted": 20, "interaction_completed": 20,
                          "interaction_presented": 20, "interaction_rasterized": 20,
                          "msaw_presented": 20, "msaw_refused": 40, "msaw_rasterized": 20},
        "safety": {"state_reports": 4, "stale_unavailable": 4, "created": 3, "refused": 3,
                   "refused_full": 1, "suppressed_logged": 4096, "suppressed_overflow": 3,
                   "removed": 1, "state_backdated": 1, "state_future_refused": 1,
                   "error_builds": 20},
        "nam_session": {"peak_pending": 128, "pending_full": 1, "abandoned": 128,
                        "peer_silent": 1, "lrm_sent": 1, "dropped": 0, "script_errors": 0},
        "clock_gate": {"held": 2, "held_applied": 2, "step_forward": 1, "late": 5,
                       "step_back": 1, "future_dropped": 3, "recovered": 1},
        "bucket_capacity": {"plots_offered": 16_386, "bucket_plot_overflow": 1,
                             "known_hard_applied": 1, "unmatched_hard_no_track": 16_383,
                             "recovery_applied": 1},
        "adsb_pending": {"offered": 1_048_577, "pending_overflow": 1, "drained": 1_048_576,
                         "bucket_plot_overflow": 0, "recovery_applied": 2,
                         "tracker_applied": 1, "tracker_duplicates": 1_048_575,
                         "tracker_refused": 0, "tracker_capacity_refused": 0,
                         "tracker_late_window": 0, "tracker_late_ring": 0,
                         "tracker_jump_rejected": 0, "tracker_numerical_refused": 0,
                         "hard_extra": 0, "hard_mismatch": 0},
        "plot_pending": {"offered": 1_048_577, "pending_overflow": 1, "drained": 1_048_576,
                         "bucket_plot_overflow": 1_032_192, "recovery_applied": 1,
                         "tracker_applied": 1, "tracker_duplicates": 0,
                         "tracker_refused": 0, "tracker_capacity_refused": 0,
                         "tracker_late_window": 0, "tracker_late_ring": 0,
                         "tracker_jump_rejected": 0, "tracker_numerical_refused": 0,
                         "hard_extra": 16_383, "hard_mismatch": 0},
    }
    for group, fields in exact_fields.items():
        evidence = result.get(group)
        for name, expected in fields.items():
            if not _object(evidence) or evidence.get(name) != expected:
                errors.append(f"{group}.{name} must equal {expected}")
    route_conversion = result.get("route_conversion")
    if _object(route_conversion):
        used, limit = route_conversion.get("work_used"), route_conversion.get("work_limit")
        if _nonnegative_int(used) and _nonnegative_int(limit) and used > limit:
            errors.append("route_conversion.work_used exceeds work_limit")
    nam_session = result.get("nam_session")
    if not _object(nam_session) or nam_session.get("final_inactive") is not True:
        errors.append("nam_session.final_inactive must be true")
    output_log = result.get("output_log")
    for name in ("output_aligned", "output_preserved_addr", "output_payload_unchanged", "log_aligned"):
        if not _object(output_log) or output_log.get(name) is not True:
            errors.append(f"output_log.{name} must be true")
    return errors

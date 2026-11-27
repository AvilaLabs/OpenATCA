//! Offline route/flight/trajectory bridge (shell, B1/B3/C6). Converted geometry and every
//! restriction remain bound to the snapshot, full flight key and exact admitted route bytes.
//! This adapter never guesses an active leg, identity, airspeed or altitude crossing solution.

use crate::flight::SharedFlightView;
use crate::input::{EventKind, NormalizedEvent, SourceKind, MAX_FILED_ROUTE_BYTES};
use crate::surveillance::ObservedTrack;
use oa_airspace::core::model::{self, Model};
use oa_airspace::core::route::{
    convert_route, empty_converted_route, ConvertedRoute, RouteFailure, RouteReport, RouteSource,
    RouteStep,
};
use oa_airspace::shell::route_parse::parse_route_steps;
use oa_legacy_traj::core::trajectory::{
    self as t, AltitudeConstraintKind as A, SpeedConstraintKind as S,
};
use std::borrow::Cow;

/// C6/C7: one session retains at most 256 route/cursor revisions across all flights, matching
/// the shared selector's bound. Exhaustion stops the run before conversion or storage.
pub const MAX_ROUTE_REVISIONS: usize = 256;
/// Separate current-route pool for explicit high-volume offline replay. The legacy selector,
/// cursor history and quadratic comparison ceilings above remain unchanged.
pub const MAX_CURRENT_ROUTE_PLANS: usize = 8192;
/// Startup-reserved current-route storage budget (~250 MiB payload plus fence/index metadata).
pub const MAX_CURRENT_ROUTE_POOL_BYTES: usize = 320 * 1024 * 1024;
/// One-day as-of horizon for offline full-day replay. Returned frames copy all route-derived
/// values and retain no route-store references, so no reader pin survives a successful frame.
/// Cutoffs and delayed source rows older than this watermark are explicitly refused.
pub const CURRENT_ROUTE_HISTORY_RETENTION_MS: i64 = 24 * 60 * 60 * 1000;
/// Maximum bytes in the versioned canonical route payload encoding. The checked maximum is
/// 91-byte header + 256 active points × 125 bytes per point = 32,091 bytes.
const MAX_ROUTE_HASH_BYTES: usize = 512 * 1024;
const ROUTE_HASH_HEADER_BYTES: usize = 91;
const ROUTE_HASH_POINT_BYTES: usize = 125;
const MAX_CURSOR_COMPARISONS: usize = MAX_ROUTE_REVISIONS * (MAX_ROUTE_REVISIONS - 1) / 2;
const PATH_DF: u8 = 7;
const PATH_IF: u8 = 15;
const PATH_TF: u8 = 18;
pub(crate) const HOLD_PROFILE_POINTS: usize = 240;
pub(crate) const EMPTY_HOLD_ROUTE_POINT: t::RoutePoint = t::RoutePoint {
    lat_e7: 0,
    lon_e7: 0,
    min_altitude_ft: t::NO_ALTITUDE_FT,
    max_altitude_ft: t::NO_ALTITUDE_FT,
    min_speed_mmps: t::NO_SPEED_MMPS,
    max_speed_mmps: t::NO_SPEED_MMPS,
    fix: t::FixIdentity {
        ident: t::RouteIdentifier {
            len: 8,
            bytes: *b"HOLD0001\0\0\0\0",
        },
        region: *b"K1\0\0",
        scope: t::RouteIdentifier {
            len: 0,
            bytes: [0; 12],
        },
        class: 1,
    },
    altitude_constraint_kind: t::AltitudeConstraintKind::None,
    speed_constraint_kind: t::SpeedConstraintKind::None,
    geometry: t::PathGeometry::GreatCircleLeg,
    speed_reference: t::SpeedReference::GroundSpeed,
    reserved: [0; 1],
};

// A3/C6: cold immutable offline revisions, capped above. Complete converter output retains
// airway/procedure/MEA/turn provenance even where the baseline predictor cannot use it.
pub struct RouteRevision {
    pub evidence: t::RevisionEvidence,
    pub text_sha256: [u8; 32],
    pub report: RouteReport,
    pub converted: ConvertedRoute,
}

/// A rejected filed-route amendment that still fences stale accepted route evidence.
/// A7.2: cold refusal metadata, distinct from an accepted route payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteRefusalFence {
    pub key: t::FlightKey,
    /// None means the rejected source timestamp could not safely order this amendment. Until a
    /// strictly later source-time revision is available, route queries fail closed.
    pub source_ms: Option<i64>,
    pub available_ms: i64,
    pub message_id: u64,
    pub attempted_text_sha256: [u8; 32],
    pub reason: u16,
}

/// A replaced route's discarded as-of interval. Queries in this interval fail explicitly rather
/// than accidentally selecting the newer current route as historical truth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteHistoryExpiry {
    pub key: t::FlightKey,
    pub from_available_ms: i64,
    pub until_available_ms: i64,
    pub until_inclusive: bool,
}

// A7.2: cold per-item stage value; inline fixed-size converted route avoids a new allocation.
#[allow(clippy::large_enum_variant)]
pub enum RouteStage {
    Noop,
    Revision(RouteRevision),
    Retire {
        key: t::FlightKey,
        available_ms: i64,
    },
    Cursor(t::ActiveRouteCursor, [u8; 32]),
    Refusal {
        reason: u16,
        fence: Option<RouteRefusalFence>,
    },
}
// A7.2: cold shell container, size guard waived.
pub struct RouteStore {
    pub revisions: Vec<RouteRevision>,
    /// Failed filed-route amendments retained so a later success cannot rewrite past cutoffs.
    pub route_refusals: Vec<RouteRefusalFence>,
    pub history_expiries: Vec<RouteHistoryExpiry>,
    pub revision_capacity: usize,
    pub refusal_capacity: usize,
    pub reclaim_current_by_key: bool,
    pub history_watermark_ms: i64,
    pub cursors: Vec<t::ActiveRouteCursor>,
    pub cursor_text_hashes: Vec<[u8; 32]>,
    pub rejected_legacy_routes: u64,
    pub snapshot_sha256: [u8; 32],
}

pub fn route_store_new(snapshot_sha256: [u8; 32]) -> RouteStore {
    route_store_new_with_capacity(snapshot_sha256, MAX_ROUTE_REVISIONS, false)
}

pub fn route_store_new_with_capacity(
    snapshot_sha256: [u8; 32],
    revision_capacity: usize,
    reclaim_current_by_key: bool,
) -> RouteStore {
    RouteStore {
        revisions: Vec::with_capacity(revision_capacity),
        route_refusals: Vec::with_capacity(revision_capacity),
        history_expiries: Vec::with_capacity(revision_capacity),
        revision_capacity,
        refusal_capacity: revision_capacity,
        reclaim_current_by_key,
        history_watermark_ms: 0,
        cursors: Vec::with_capacity(MAX_ROUTE_REVISIONS),
        cursor_text_hashes: Vec::with_capacity(MAX_ROUTE_REVISIONS),
        rejected_legacy_routes: 0,
        snapshot_sha256,
    }
}

pub fn route_store_reserved_bytes(store: &RouteStore) -> Option<usize> {
    store
        .revisions
        .capacity()
        .checked_mul(core::mem::size_of::<RouteRevision>())?
        .checked_add(
            store
                .route_refusals
                .capacity()
                .checked_mul(core::mem::size_of::<RouteRefusalFence>())?,
        )?
        .checked_add(
            store
                .history_expiries
                .capacity()
                .checked_mul(core::mem::size_of::<RouteHistoryExpiry>())?,
        )?
        .checked_add(
            store
                .cursors
                .capacity()
                .checked_mul(core::mem::size_of::<t::ActiveRouteCursor>())?,
        )?
        .checked_add(
            store
                .cursor_text_hashes
                .capacity()
                .checked_mul(core::mem::size_of::<[u8; 32]>())?,
        )
}

fn revision_capacity_ok(store: &RouteStore) -> bool {
    if store.reclaim_current_by_key {
        store.revisions.len() < store.revision_capacity
    } else {
        store
            .revisions
            .len()
            .saturating_add(store.route_refusals.len())
            < store.revision_capacity
    }
}

fn refusal_capacity_ok(store: &RouteStore) -> bool {
    if store.reclaim_current_by_key {
        store.route_refusals.len() < store.refusal_capacity
    } else {
        store
            .revisions
            .len()
            .saturating_add(store.route_refusals.len())
            < store.refusal_capacity
    }
}

pub fn route_history_expired(
    store: &RouteStore,
    key: t::FlightKey,
    cutoff_ms: i64,
) -> Result<bool, String> {
    if store.reclaim_current_by_key && cutoff_ms < store.history_watermark_ms {
        return Ok(true);
    }
    if store.history_expiries.len() > store.revision_capacity {
        return Err("route history-expiry budget exceeded".into());
    }
    let mut matching_key_seen = false;
    for expiry in &store.history_expiries {
        if !valid_refusal_key(expiry.key)
            || expiry.from_available_ms < 0
            || expiry.until_available_ms < expiry.from_available_ms
        {
            return Err("invalid route history-expiry interval".into());
        }
        if expiry.key == key {
            if matching_key_seen {
                return Err("duplicate route history-expiry key".into());
            }
            matching_key_seen = true;
            if expiry.from_available_ms <= cutoff_ms
                && (cutoff_ms < expiry.until_available_ms
                    || (expiry.until_inclusive && cutoff_ms == expiry.until_available_ms))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(crate) fn revision_admission_ok(store: &RouteStore, key: t::FlightKey) -> bool {
    if store.reclaim_current_by_key && store.revisions.iter().any(|r| r.evidence.key == key) {
        return store.history_expiries.len() < store.revision_capacity
            || store.history_expiries.iter().any(|e| e.key == key);
    }
    revision_capacity_ok(store)
}

pub(crate) fn candidate_history_watermark(store: &RouteStore, now_ms: i64) -> i64 {
    if store.reclaim_current_by_key {
        store
            .history_watermark_ms
            .max(now_ms.saturating_sub(CURRENT_ROUTE_HISTORY_RETENTION_MS))
    } else {
        store.history_watermark_ms
    }
}

pub(crate) fn source_time_is_retained(store: &RouteStore, source_ms: i64) -> bool {
    !store.reclaim_current_by_key || source_ms >= store.history_watermark_ms
}

pub(crate) fn advance_route_history_watermark(store: &mut RouteStore, now_ms: i64) {
    if !store.reclaim_current_by_key {
        return;
    }
    let watermark = candidate_history_watermark(store, now_ms);
    store.history_watermark_ms = watermark;
    store.history_expiries.retain(|expiry| {
        expiry.until_available_ms > watermark
            || (expiry.until_inclusive && expiry.until_available_ms == watermark)
    });
    let mut index = 0;
    while let Some(cursor) = store.cursors.get(index) {
        if cursor.source_ms < watermark {
            if store.cursor_text_hashes.get(index).is_none() {
                break;
            }
            store.cursors.swap_remove(index);
            store.cursor_text_hashes.swap_remove(index);
        } else {
            index += 1;
        }
    }
    store.route_refusals.retain(|fence| {
        if fence.available_ms > watermark {
            return true;
        }
        let blocked_through = fence.source_ms.unwrap_or(fence.available_ms);
        store.revisions.iter().any(|revision| {
            revision.evidence.key == fence.key && revision.evidence.source_ms <= blocked_through
        })
    });
}

pub(crate) fn route_revision_admission(store: &RouteStore, revision: &RouteRevision) -> bool {
    if !revision_admission_ok(store, revision.evidence.key) {
        return false;
    }
    if !store.reclaim_current_by_key {
        return true;
    }
    let watermark = candidate_history_watermark(store, revision.evidence.available_ms);
    let retained_expiries = store
        .history_expiries
        .iter()
        .filter(|expiry| {
            expiry.until_available_ms > watermark
                || (expiry.until_inclusive && expiry.until_available_ms == watermark)
        })
        .count();
    let existing_expiry = store
        .history_expiries
        .iter()
        .any(|expiry| expiry.key == revision.evidence.key);
    match store
        .revisions
        .iter()
        .find(|current| current.evidence.key == revision.evidence.key)
    {
        Some(current) => {
            revision.evidence.source_ms >= current.evidence.source_ms
                && revision.evidence.available_ms >= current.evidence.available_ms
                && (retained_expiries < store.revision_capacity || existing_expiry)
        }
        None => true,
    }
}

pub(crate) fn route_stage_retirement(
    store: &RouteStore,
    key: t::FlightKey,
    available_ms: i64,
) -> Result<RouteStage, String> {
    if !store.reclaim_current_by_key {
        return Ok(RouteStage::Noop);
    }
    if !valid_refusal_key(key) || available_ms < 0 {
        return Err(
            "route retirement requires the explicit current-route pool and valid identity".into(),
        );
    }
    if let Some(current) = store.revisions.iter().find(|r| r.evidence.key == key) {
        if available_ms < current.evidence.available_ms {
            return Err("route retirement precedes current route availability".into());
        }
        let watermark = candidate_history_watermark(store, available_ms);
        let retained_expiries = store
            .history_expiries
            .iter()
            .filter(|expiry| {
                expiry.until_available_ms > watermark
                    || (expiry.until_inclusive && expiry.until_available_ms == watermark)
            })
            .count();
        if !store
            .history_expiries
            .iter()
            .any(|expiry| expiry.key == key)
            && retained_expiries >= store.revision_capacity
        {
            return Err("route history-expiry capacity exhausted".into());
        }
        Ok(RouteStage::Retire { key, available_ms })
    } else {
        Ok(RouteStage::Noop)
    }
}

/// Build an immutable route-store revision from an already resolved ICAO Item 15.
/// The source message hash, complete flight identity, and snapshot digest stay bound to the
/// existing trajectory consumer's revision evidence.
pub fn icao_route_revision(
    key: t::FlightKey,
    resolved: &oa_airspace::shell::route_parse::ResolvedItem15,
    source_ms: i64,
    available_ms: i64,
    message_id: u64,
    text_sha256: [u8; 32],
    snapshot_sha256: [u8; 32],
) -> Result<RouteRevision, String> {
    if !valid_refusal_key(key) {
        return Err("cannot retain route revision without a valid full flight key".into());
    }
    if source_ms < 0 || available_ms < source_ms {
        return Err("route revision timestamps are invalid or out of order".into());
    }
    if resolved.report.failure != RouteFailure::None {
        return Err("cannot retain a route that did not resolve completely".into());
    }
    let payload_sha256 = canonical_route_digest(
        snapshot_sha256,
        text_sha256,
        resolved.report,
        &resolved.route,
    )?;
    Ok(RouteRevision {
        evidence: t::RevisionEvidence {
            source_ms,
            available_ms,
            message_id,
            payload_sha256,
            key,
            reserved: [0; 4],
        },
        text_sha256,
        report: resolved.report,
        converted: resolved.route.clone(),
    })
}

fn valid_refusal_key(key: t::FlightKey) -> bool {
    key.gufi.iter().any(|&byte| byte != 0)
        && (1..=0x00ff_ffff).contains(&key.aircraft_address)
        && key.centre.iter().all(u8::is_ascii_alphanumeric)
        && key.plan_id.iter().any(|&byte| byte != 0)
}

fn refusal_fence(
    event: &NormalizedEvent,
    plan_id: [u8; 12],
    available_ms: i64,
    source_ms: Option<i64>,
    text: &[u8; MAX_FILED_ROUTE_BYTES],
    text_len: u16,
    reason: u16,
) -> Result<RouteRefusalFence, String> {
    let key = event_key(event, plan_id);
    if !valid_refusal_key(key) {
        return Err("cannot fence failed route amendment without a valid full flight key".into());
    }
    let hashed_len = usize::from(text_len).min(text.len());
    let bytes = text
        .get(..hashed_len)
        .ok_or("route refusal digest range invariant")?;
    Ok(RouteRefusalFence {
        key,
        source_ms,
        available_ms,
        message_id: event.sequence,
        attempted_text_sha256: oa_sha256::sha256(bytes),
        reason,
    })
}

fn refuse_filed_route(
    store: &RouteStore,
    event: &NormalizedEvent,
    plan_id: [u8; 12],
    text: &[u8; MAX_FILED_ROUTE_BYTES],
    text_len: u16,
    reason: u16,
) -> Result<RouteStage, String> {
    if !refusal_capacity_ok(store) {
        return Err(
            "route refusal fence capacity exhausted; refusing to expose stale route".into(),
        );
    }
    let available_ms = ms(event.receipt_ns)
        .map_err(|_| "route refusal has no valid receipt cutoff; session must fail closed")?;
    let source_ms = if event.source_ns >= 0
        && event.source_ns % 1_000_000 == 0
        && event.source_ns <= event.receipt_ns
        && !oa_sealed::in_sealed_window(event.source_ns)
    {
        Some(event.source_ns / 1_000_000)
    } else {
        None
    };
    let fence = refusal_fence(
        event,
        plan_id,
        available_ms,
        source_ms,
        text,
        text_len,
        reason,
    )?;
    Ok(RouteStage::Refusal {
        reason,
        fence: Some(fence),
    })
}

pub fn route_refusal_blocks(
    store: &RouteStore,
    selected: t::RevisionEvidence,
    cutoff_ms: i64,
) -> Result<bool, String> {
    if store.route_refusals.len() > store.refusal_capacity
        || (!store.reclaim_current_by_key
            && store.revisions.len() + store.route_refusals.len() > store.revision_capacity)
    {
        return Err("route revision/fence budget exceeded".into());
    }
    for fence in &store.route_refusals {
        if !valid_refusal_key(fence.key)
            || fence.available_ms < 0
            || fence
                .source_ms
                .is_some_and(|source| source < 0 || source > fence.available_ms)
            || fence.reason == 0
        {
            return Err("invalid route refusal fence".into());
        }
        if fence.available_ms > cutoff_ms || fence.key != selected.key {
            continue;
        }
        let blocked_through_ms = fence.source_ms.unwrap_or(fence.available_ms);
        if selected.source_ms <= blocked_through_ms {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Purely validate and stage a route item. A failed filed amendment produces separate refusal
/// metadata so callers can quarantine it without exposing an older route as current.
pub fn route_stage_event(
    store: &RouteStore,
    model: &Model,
    event: &NormalizedEvent,
) -> Result<RouteStage, String> {
    match event.kind {
        EventKind::FiledRoute {
            plan_id,
            text_len,
            text,
            ..
        } => {
            if !valid_refusal_key(event_key(event, plan_id)) {
                return Err("cannot process filed route without a valid full flight key".into());
            }
            if event.source != SourceKind::Route {
                return refuse_filed_route(
                    store,
                    event,
                    plan_id,
                    &text,
                    text_len,
                    oa_quarantine::REASON_INPUT_INVALID,
                );
            }
            let source_ms = match ms(event.source_ns) {
                Ok(value) => value,
                Err(_) => {
                    return refuse_filed_route(
                        store,
                        event,
                        plan_id,
                        &text,
                        text_len,
                        oa_quarantine::REASON_INPUT_INVALID,
                    )
                }
            };
            let available_ms = ms(event.receipt_ns)?;
            if store.reclaim_current_by_key
                && source_ms < candidate_history_watermark(store, available_ms)
            {
                return refuse_filed_route(
                    store,
                    event,
                    plan_id,
                    &text,
                    text_len,
                    oa_quarantine::REASON_INPUT_INVALID,
                );
            }
            if source_ms > available_ms || usize::from(text_len) > MAX_FILED_ROUTE_BYTES {
                return refuse_filed_route(
                    store,
                    event,
                    plan_id,
                    &text,
                    text_len,
                    oa_quarantine::REASON_ALTITUDE_ROUTE_INVALID,
                );
            }
            if !revision_admission_ok(store, event_key(event, plan_id)) {
                return Err("route revision/fence capacity exhausted".into());
            }
            let maximum_encoding_size = hash_size(t::MAX_ROUTE_POINTS)?;
            if maximum_encoding_size > MAX_ROUTE_HASH_BYTES {
                return Err("route digest bound invariant".into());
            }
            let text_view = match bounded_text(&text, text_len) {
                Ok(value) => value,
                Err(_) => {
                    return refuse_filed_route(
                        store,
                        event,
                        plan_id,
                        &text,
                        text_len,
                        oa_quarantine::REASON_ALTITUDE_ROUTE_INVALID,
                    )
                }
            };
            let plan = match parse_route_steps(text_view) {
                Ok(value) => value,
                Err(_) => {
                    return refuse_filed_route(
                        store,
                        event,
                        plan_id,
                        &text,
                        text_len,
                        oa_quarantine::REASON_ALTITUDE_ROUTE_INVALID,
                    )
                }
            };
            let mut converted = empty_converted_route();
            let report = convert_route(model, &plan, &mut converted);
            if report.failure == RouteFailure::MalformedModel {
                return Err("route model integrity failure".into());
            }
            if report.failure != RouteFailure::None {
                let reason = if report.failure == RouteFailure::Ambiguous {
                    oa_quarantine::REASON_WAYPOINT_NAME_AMBIGUOUS
                } else {
                    oa_quarantine::REASON_ALTITUDE_ROUTE_INVALID
                };
                return refuse_filed_route(store, event, plan_id, &text, text_len, reason);
            }
            let text_sha256 = oa_sha256::sha256(text_view.as_bytes());
            let payload_sha256 =
                canonical_route_digest(store.snapshot_sha256, text_sha256, report, &converted)?;
            if store.revisions.iter().any(|revision| {
                revision.evidence.key == event_key(event, plan_id)
                    && revision.evidence.source_ms == source_ms
                    && revision.text_sha256 != text_sha256
            }) {
                return refuse_filed_route(
                    store,
                    event,
                    plan_id,
                    &text,
                    text_len,
                    oa_quarantine::REASON_IDENTITY_OR_LOCATION_CONFLICT,
                );
            }
            let revision = RouteRevision {
                evidence: t::RevisionEvidence {
                    source_ms,
                    available_ms,
                    message_id: event.sequence,
                    payload_sha256,
                    key: event_key(event, plan_id),
                    reserved: [0; 4],
                },
                text_sha256,
                report,
                converted,
            };
            if !route_revision_admission(store, &revision) {
                return refuse_filed_route(
                    store,
                    event,
                    plan_id,
                    &text,
                    text_len,
                    oa_quarantine::REASON_INPUT_INVALID,
                );
            }
            Ok(RouteStage::Revision(revision))
        }
        EventKind::ActiveCursor {
            plan_id,
            route_payload_sha256,
            fix_len,
            fix: text,
        } => {
            if event.source != SourceKind::Route || event.source_ns > event.receipt_ns {
                return Ok(RouteStage::Refusal {
                    reason: oa_quarantine::REASON_ALTITUDE_ROUTE_INVALID,
                    fence: None,
                });
            }
            let source_ms = match ms(event.source_ns) {
                Ok(value) => value,
                Err(_) => {
                    return Ok(RouteStage::Refusal {
                        reason: oa_quarantine::REASON_INPUT_INVALID,
                        fence: None,
                    })
                }
            };
            let available_ms = ms(event.receipt_ns)?;
            if store.reclaim_current_by_key
                && source_ms < candidate_history_watermark(store, available_ms)
            {
                return Ok(RouteStage::Refusal {
                    reason: oa_quarantine::REASON_INPUT_INVALID,
                    fence: None,
                });
            }
            if store.cursors.len() >= MAX_ROUTE_REVISIONS {
                return Ok(RouteStage::Refusal {
                    reason: oa_quarantine::REASON_ALTITUDE_ROUTE_INVALID,
                    fence: None,
                });
            }
            let text_view = match bounded_text(&text, fix_len) {
                Ok(value) if usize::from(fix_len) <= MAX_FILED_ROUTE_BYTES => value,
                _ => {
                    return Ok(RouteStage::Refusal {
                        reason: oa_quarantine::REASON_ALTITUDE_ROUTE_INVALID,
                        fence: None,
                    })
                }
            };
            let plan = match parse_route_steps(text_view) {
                Ok(value) if value.len == 1 => value,
                _ => {
                    return Ok(RouteStage::Refusal {
                        reason: oa_quarantine::REASON_ALTITUDE_ROUTE_INVALID,
                        fence: None,
                    })
                }
            };
            let next_fix = match plan.steps.first() {
                Some(RouteStep::Fix(key)) => fix(*key),
                _ => {
                    return Ok(RouteStage::Refusal {
                        reason: oa_quarantine::REASON_ALTITUDE_ROUTE_INVALID,
                        fence: None,
                    })
                }
            };
            Ok(RouteStage::Cursor(
                t::ActiveRouteCursor {
                    source_ms,
                    available_ms,
                    message_id: event.sequence,
                    route_payload_sha256: [0; 32],
                    key: event_key(event, plan_id),
                    next_fix,
                    reserved: [0; 5],
                },
                route_payload_sha256,
            ))
        }
        _ => Ok(RouteStage::Noop),
    }
}

pub fn route_commit_stage(store: &mut RouteStore, stage: RouteStage) {
    match stage {
        RouteStage::Noop | RouteStage::Refusal { .. } => {}
        RouteStage::Retire { key, available_ms } => {
            if let Some(index) = store
                .revisions
                .iter()
                .position(|revision| revision.evidence.key == key)
            {
                let old = store.revisions.swap_remove(index).evidence;
                if let Some(expiry) = store
                    .history_expiries
                    .iter_mut()
                    .find(|expiry| expiry.key == key)
                {
                    expiry.until_available_ms = available_ms;
                    expiry.until_inclusive = true;
                } else {
                    store.history_expiries.push(RouteHistoryExpiry {
                        key,
                        from_available_ms: old.available_ms,
                        until_available_ms: available_ms,
                        until_inclusive: true,
                    });
                }
            }
        }
        RouteStage::Revision(revision) => {
            if store.reclaim_current_by_key {
                if let Some(index) = store
                    .revisions
                    .iter()
                    .position(|current| current.evidence.key == revision.evidence.key)
                {
                    let old = store.revisions.swap_remove(index).evidence;
                    if old.available_ms < revision.evidence.available_ms {
                        if let Some(expiry) = store
                            .history_expiries
                            .iter_mut()
                            .find(|expiry| expiry.key == revision.evidence.key)
                        {
                            expiry.until_available_ms = revision.evidence.available_ms;
                            expiry.until_inclusive = false;
                        } else {
                            store.history_expiries.push(RouteHistoryExpiry {
                                key: revision.evidence.key,
                                from_available_ms: old.available_ms,
                                until_available_ms: revision.evidence.available_ms,
                                until_inclusive: false,
                            });
                        }
                    }
                    store.revisions.push(revision);
                } else {
                    if let Some(expiry) = store
                        .history_expiries
                        .iter_mut()
                        .find(|expiry| expiry.key == revision.evidence.key)
                    {
                        expiry.until_available_ms = revision.evidence.available_ms;
                        expiry.until_inclusive = false;
                    }
                    store.revisions.push(revision);
                }
            } else {
                store.revisions.push(revision);
            }
        }
        RouteStage::Cursor(cursor, text_hash) => {
            store.cursors.push(cursor);
            store.cursor_text_hashes.push(text_hash);
        }
    }
}

pub fn route_rejection_fence(
    store: &RouteStore,
    event: &NormalizedEvent,
    stage: &RouteStage,
    reason: u16,
) -> Result<Option<RouteRefusalFence>, String> {
    match stage {
        RouteStage::Refusal { fence, .. } => Ok(fence.map(|mut fence| {
            fence.reason |= reason;
            fence
        })),
        RouteStage::Revision(revision) => {
            let EventKind::FiledRoute { .. } = event.kind else {
                return Ok(None);
            };
            if !refusal_capacity_ok(store) {
                return Err("route refusal fence capacity exhausted".into());
            }
            Ok(Some(RouteRefusalFence {
                key: revision.evidence.key,
                source_ms: Some(revision.evidence.source_ms),
                available_ms: revision.evidence.available_ms,
                message_id: revision.evidence.message_id,
                attempted_text_sha256: revision.text_sha256,
                reason,
            }))
        }
        RouteStage::Noop | RouteStage::Retire { .. } | RouteStage::Cursor(..) => Ok(None),
    }
}

pub fn route_commit_refusal(
    store: &mut RouteStore,
    fence: RouteRefusalFence,
) -> Result<(), String> {
    if !refusal_capacity_ok(store) {
        return Err("route refusal fence capacity exhausted".into());
    }
    store.route_refusals.push(fence);
    Ok(())
}

pub fn flight_key(view: &SharedFlightView) -> t::FlightKey {
    t::FlightKey {
        gufi: view.gufi.to_be_bytes(),
        aircraft_address: view.address,
        centre: view.centre,
        plan_id: view.plan_id,
    }
}

fn event_key(event: &NormalizedEvent, plan_id: [u8; 12]) -> t::FlightKey {
    t::FlightKey {
        gufi: event.identity.flight_gufi.to_be_bytes(),
        aircraft_address: event.identity.aircraft_address,
        centre: event.centre,
        plan_id,
    }
}

fn ms(ns: i64) -> Result<i64, String> {
    if ns < 0 || ns % 1_000_000 != 0 {
        return Err("trajectory evidence requires exact whole milliseconds".into());
    }
    if oa_sealed::in_sealed_window(ns) {
        return Err("sealed timestamp refused".into());
    }
    Ok(ns / 1_000_000)
}

fn bounded_text(bytes: &[u8], len: u16) -> Result<&str, String> {
    std::str::from_utf8(
        bytes
            .get(..usize::from(len))
            .ok_or("text length exceeds stored payload")?,
    )
    .map_err(|_| "invalid route text".into())
}

fn hash_size(point_count: usize) -> Result<usize, String> {
    if point_count > t::MAX_ROUTE_POINTS {
        return Err("converted route exceeds trajectory capacity".into());
    }
    let points_bytes = point_count
        .checked_mul(ROUTE_HASH_POINT_BYTES)
        .ok_or("route hash size overflow")?;
    let total = ROUTE_HASH_HEADER_BYTES
        .checked_add(points_bytes)
        .ok_or("route hash size overflow")?;
    if total > MAX_ROUTE_HASH_BYTES {
        return Err("route hash encoding budget exceeded".into());
    }
    Ok(total)
}

fn encode_label<const N: usize>(bytes: &mut Vec<u8>, label: oa_airspace::core::route::Label<N>) {
    bytes.extend_from_slice(&label.bytes);
    bytes.push(label.len);
}

/// Canonical V1 digest input. Integer fields are little-endian; only active point rows are
/// encoded. Every converted source/procedure/MEA/turn field participates in the digest.
pub fn canonical_route_digest(
    snapshot: [u8; 32],
    text_sha256: [u8; 32],
    report: RouteReport,
    route: &ConvertedRoute,
) -> Result<[u8; 32], String> {
    let n = usize::from(route.len);
    let expected = hash_size(n)?;
    let mut bytes = Vec::with_capacity(expected);
    bytes.extend_from_slice(b"OA-ROUTE\x01");
    bytes.extend_from_slice(&snapshot);
    bytes.extend_from_slice(&text_sha256);
    bytes.push(report.failure as u8);
    bytes.extend_from_slice(&[
        report.offered,
        report.resolved,
        report.unsupported,
        report.refused,
        report.unexamined,
    ]);
    bytes.extend_from_slice(&report.work_used.to_le_bytes());
    bytes.extend_from_slice(&report.work_limit.to_le_bytes());
    bytes.extend_from_slice(&route.len.to_le_bytes());
    bytes.extend_from_slice(&route.reserved);
    for i in 0..n {
        bytes.extend_from_slice(&value(&route.lat_e7, i)?.to_le_bytes());
        bytes.extend_from_slice(&value(&route.lon_e7, i)?.to_le_bytes());
        encode_label(&mut bytes, value(&route.ident, i)?);
        encode_label(&mut bytes, value(&route.region, i)?);
        encode_label(&mut bytes, value(&route.scope, i)?);
        bytes.extend_from_slice(&[
            value(&route.class, i)?,
            value(&route.source, i)?,
            value(&route.path_term, i)?,
            value(&route.alt_desc, i)?,
        ]);
        bytes.extend_from_slice(&value(&route.alt1_ft, i)?.to_le_bytes());
        bytes.extend_from_slice(&value(&route.alt2_ft, i)?.to_le_bytes());
        bytes.extend_from_slice(&value(&route.speed_kt, i)?.to_le_bytes());
        bytes.push(value(&route.turn, i)?);
        bytes.extend_from_slice(&value(&route.mea_ft, i)?.to_le_bytes());
        bytes.extend_from_slice(&value(&route.gps_mea_ft, i)?.to_le_bytes());
        bytes.push(value(&route.mea_dir, i)?);
        bytes.push(value(&route.gps_mea_dir, i)?);
        encode_label(&mut bytes, value(&route.airway_designator, i)?);
        bytes.push(value(&route.airway_location, i)?);
        encode_label(&mut bytes, value(&route.procedure_airport_ident, i)?);
        encode_label(&mut bytes, value(&route.procedure_airport_region, i)?);
        encode_label(&mut bytes, value(&route.procedure_ident, i)?);
        encode_label(&mut bytes, value(&route.procedure_transition, i)?);
        bytes.push(value(&route.procedure_kind, i)?);
        bytes.push(value(&route.procedure_route_type, i)?);
        bytes.push(value(&route.connected, i)?);
    }
    if bytes.len() != expected {
        return Err("canonical route encoding size mismatch".into());
    }
    Ok(oa_sha256::sha256(&bytes))
}

fn fix(key: oa_airspace::core::route::FixIdentity) -> t::FixIdentity {
    t::FixIdentity {
        ident: t::RouteIdentifier {
            len: key.ident.len,
            bytes: key.ident.bytes,
        },
        region: key.region.bytes,
        scope: t::RouteIdentifier {
            len: key.scope.len,
            bytes: key.scope.bytes,
        },
        class: key.class,
    }
}

/// Admit route intent only after its receipt batch becomes visible. Bad conversion is retained
/// as a failed revision; it never silently falls back to an older successful route.
pub fn route_event(
    store: &mut RouteStore,
    model: &Model,
    event: &NormalizedEvent,
) -> Result<(), String> {
    if event.source_ns > event.receipt_ns {
        return Err("route source after availability".into());
    }
    match event.kind {
        EventKind::FiledRoute {
            plan_id,
            text_len,
            text,
            ..
        } => {
            if event.source != SourceKind::Route {
                return Err("filed route has wrong source kind".into());
            }
            if !revision_admission_ok(store, event_key(event, plan_id)) {
                return Err("route revision capacity exhausted".into());
            }
            let source_ms = ms(event.source_ns)?;
            let available_ms = ms(event.receipt_ns)?;
            if usize::from(text_len) > MAX_FILED_ROUTE_BYTES {
                return Err("filed route text exceeds input limit".into());
            }
            let _maximum_encoding_size = hash_size(t::MAX_ROUTE_POINTS)?;
            let text = bounded_text(&text, text_len)?;
            let plan =
                parse_route_steps(text).map_err(|e| format!("normalized route parse: {e:?}"))?;
            let mut converted = empty_converted_route();
            let report = convert_route(model, &plan, &mut converted);
            let text_sha256 = oa_sha256::sha256(text.as_bytes());
            let payload_sha256 =
                canonical_route_digest(store.snapshot_sha256, text_sha256, report, &converted)?;
            let evidence = t::RevisionEvidence {
                source_ms,
                available_ms,
                message_id: event.sequence,
                payload_sha256,
                key: event_key(event, plan_id),
                reserved: [0; 4],
            };
            route_commit_stage(
                store,
                RouteStage::Revision(RouteRevision {
                    evidence,
                    text_sha256,
                    report,
                    converted,
                }),
            );
        }
        EventKind::ActiveCursor {
            plan_id,
            route_payload_sha256,
            fix_len,
            fix: text,
        } => {
            if event.source != SourceKind::Route {
                return Err("active route cursor has wrong source kind".into());
            }
            if store.cursors.len() >= MAX_ROUTE_REVISIONS {
                return Err("route cursor capacity exhausted".into());
            }
            let source_ms = ms(event.source_ns)?;
            let available_ms = ms(event.receipt_ns)?;
            if usize::from(fix_len) > MAX_FILED_ROUTE_BYTES {
                return Err("cursor identity text exceeds input limit".into());
            }
            let plan = parse_route_steps(bounded_text(&text, fix_len)?)
                .map_err(|e| format!("cursor identity parse: {e:?}"))?;
            if plan.len != 1 {
                return Err("cursor requires one full FIX identity".into());
            }
            let next = match plan.steps.first() {
                Some(RouteStep::Fix(key)) => fix(*key),
                _ => {
                    return Err(
                        "cursor requires a FIX identity, not a connector or procedure".into(),
                    )
                }
            };
            store.cursors.push(t::ActiveRouteCursor {
                source_ms,
                available_ms,
                message_id: event.sequence,
                route_payload_sha256: [0; 32],
                key: event_key(event, plan_id),
                next_fix: next,
                reserved: [0; 5],
            });
            // The external digest identifies exact filed text. On issue it is resolved to the
            // complete converted payload digest (snapshot + text + all converted fields).
            store.cursor_text_hashes.push(route_payload_sha256);
        }
        EventKind::Route { .. } => {
            store.rejected_legacy_routes = store.rejected_legacy_routes.saturating_add(1);
        }
        _ => {}
    }
    Ok(())
}

fn value<T: Copy>(column: &[T], i: usize) -> Result<T, String> {
    column
        .get(i)
        .copied()
        .ok_or_else(|| "inconsistent converted route columns".into())
}

fn same_flight(a: t::FlightKey, b: t::FlightKey) -> bool {
    a.gufi == b.gufi && a.aircraft_address == b.aircraft_address && a.centre == b.centre
}

fn stamp_in_cutoff(stamp: oa_flight::Stamp, cutoff_ms: i64) -> bool {
    stamp.ts_ms >= 0
        && stamp.avail_ms >= 0
        && stamp.ts_ms <= stamp.avail_ms
        && stamp.avail_ms <= cutoff_ms
}

fn flight_view_in_cutoff(view: &SharedFlightView, cutoff_ms: i64) -> bool {
    [
        view.plan_stamp,
        view.assigned_stamp,
        view.interim_stamp,
        view.identity_stamp,
        view.address_stamp,
        view.owner_stamp,
        view.handoff_stamp,
    ]
    .iter()
    .all(|stamp| stamp_in_cutoff(*stamp, cutoff_ms))
}

pub fn trajectory_points(route: &ConvertedRoute) -> Result<Vec<t::RoutePoint>, String> {
    let n = usize::from(route.len);
    if n > t::MAX_ROUTE_POINTS {
        return Err("converted route exceeds trajectory capacity".into());
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let source = value(&route.source, i)?;
        let path_term = value(&route.path_term, i)?;
        let connected = value(&route.connected, i)?;
        let turn = value(&route.turn, i)?;
        if turn != model::turn::NONE {
            return Err("route turn constraint is unsupported by the trajectory model".into());
        }
        let source_is_direct_fix = source == RouteSource::DirectFix as u8;
        let source_is_direct_connector = source == RouteSource::DirectConnector as u8;
        let source_is_airway = source == RouteSource::Airway as u8;
        let source_is_procedure = source == RouteSource::Procedure as u8;
        if !(source_is_direct_fix
            || source_is_direct_connector
            || source_is_airway
            || source_is_procedure)
        {
            return Err("converted route has an unknown source code".into());
        }
        if connected > 1 {
            return Err("converted route has invalid connected flag".into());
        }
        if (source_is_direct_fix || source_is_airway) && path_term != model::path_term::NONE {
            return Err("non-procedure route point has a procedure path term".into());
        }
        if source_is_direct_connector && path_term != PATH_DF {
            return Err("explicit direct connector requires its converted DF path term".into());
        }
        if source_is_procedure && !matches!(path_term, PATH_IF | PATH_DF | PATH_TF) {
            return Err("procedure path term is unsupported by trajectory geometry".into());
        }
        let gps_mea = value(&route.gps_mea_ft, i)?;
        let gps_mea_dir = value(&route.gps_mea_dir, i)?;
        if gps_mea != model::ALT_NONE || gps_mea_dir != model::compass::NONE {
            return Err("GPS MEA cannot be applied without aircraft equipment evidence".into());
        }
        let mut low = t::NO_ALTITUDE_FT;
        let mut high = t::NO_ALTITUDE_FT;
        let mut altitude_kind = match value(&route.alt_desc, i)? {
            model::alt_desc::NONE => A::None,
            model::alt_desc::AT_OR_ABOVE => {
                low = value(&route.alt1_ft, i)?;
                A::Minimum
            }
            model::alt_desc::BETWEEN => {
                low = value(&route.alt2_ft, i)?;
                high = value(&route.alt1_ft, i)?;
                A::Window
            }
            _ => A::Unsupported,
        };
        let mea = value(&route.mea_ft, i)?;
        // A direction-scoped conventional MEA is conservatively enforced in every direction.
        // This can refuse a route that would not need the MEA in its actual direction, but never
        // weakens the bound. The exact direction remains in the canonical route evidence.
        if mea != model::ALT_NONE {
            low = if low == t::NO_ALTITUDE_FT {
                mea
            } else {
                low.max(mea)
            };
            altitude_kind = if altitude_kind == A::Unsupported {
                A::Unsupported
            } else if high != t::NO_ALTITUDE_FT {
                A::Window
            } else {
                A::Minimum
            };
        }
        let speed = value(&route.speed_kt, i)?;
        let has_speed = speed != model::NONE_U16;
        // Exact rational kt -> mm/s, rounded to nearest integer before checked narrowing.
        let speed_mmps = if has_speed {
            u32::from(speed)
                .checked_mul(463_000)
                .and_then(|v| v.checked_add(450))
                .map(|v| v / 900)
                .ok_or("speed conversion overflow")?
        } else {
            t::NO_SPEED_MMPS
        };
        // `connected` records a supported coded edge. Convert only path/source pairs whose
        // geometry is represented by the trajectory model. IF is a fix initializer, not an
        // incoming segment; DF and explicit direct connectors are direct-to, TF and airway legs
        // are great-circle legs. A first route point has no preceding route edge.
        let geometry = if i == 0 {
            t::PathGeometry::GreatCircleLeg
        } else if connected == 0 {
            t::PathGeometry::VectorsGap
        } else if source_is_direct_connector {
            t::PathGeometry::DirectTo
        } else if source_is_airway || (source_is_procedure && path_term == PATH_TF) {
            t::PathGeometry::GreatCircleLeg
        } else if source_is_procedure && path_term == PATH_DF {
            t::PathGeometry::DirectTo
        } else if source_is_procedure && path_term == PATH_IF {
            return Err("IF procedure leg cannot claim a connected incoming path".into());
        } else {
            return Err("connected route source/path-term has unsupported geometry".into());
        };
        out.push(t::RoutePoint {
            lat_e7: value(&route.lat_e7, i)?,
            lon_e7: value(&route.lon_e7, i)?,
            min_altitude_ft: low,
            max_altitude_ft: high,
            min_speed_mmps: t::NO_SPEED_MMPS,
            max_speed_mmps: speed_mmps,
            fix: fix(oa_airspace::core::route::FixIdentity {
                ident: value(&route.ident, i)?,
                region: value(&route.region, i)?,
                scope: value(&route.scope, i)?,
                class: value(&route.class, i)?,
            }),
            altitude_constraint_kind: altitude_kind,
            speed_constraint_kind: if has_speed { S::Maximum } else { S::None },
            speed_reference: if has_speed {
                t::SpeedReference::IndicatedAirspeed
            } else {
                t::SpeedReference::Unknown
            },
            geometry,
            reserved: [0; 1],
        });
    }
    Ok(out)
}

fn later_stamp(a: oa_flight::Stamp, b: oa_flight::Stamp) -> oa_flight::Stamp {
    if (a.avail_ms, a.ts_ms, a.msg) >= (b.avail_ms, b.ts_ms, b.msg) {
        a
    } else {
        b
    }
}

/// Build the predictor's effective-clearance evidence from the separately stamped assigned and
/// interim fields. Explicit interim NIL uses assigned value with the later field stamp.
pub fn clearance_evidence(view: &SharedFlightView) -> t::Clearance {
    // SharedFlightView retains assigned and interim values and stamps independently. Its
    // effective_clearance is a convenience summary; derive the supported form here from those
    // source fields so unsupported non-NIL interim forms cannot fall back to assigned. For an
    // explicit interim NIL, the effective value is assigned while the later assigned/clear stamp
    // preserves when that effective state became current.
    let (alt, stamp) = match view.interim.form {
        oa_flight::ALT_NONE => (view.assigned, view.assigned_stamp),
        oa_flight::ALT_NIL => (
            view.assigned,
            later_stamp(view.assigned_stamp, view.interim_stamp),
        ),
        _ => (view.interim, view.interim_stamp),
    };
    let (form, low, high, at_fix) = match alt.form {
        oa_flight::ALT_SIMPLE => (
            t::ClearanceForm::Exact,
            alt.a_ft,
            alt.a_ft,
            t::NO_ALTITUDE_FT,
        ),
        oa_flight::ALT_BLOCK => (
            t::ClearanceForm::Block,
            alt.a_ft,
            alt.b_ft,
            t::NO_ALTITUDE_FT,
        ),
        oa_flight::ALT_AT_FIX => (
            t::ClearanceForm::AltitudeAtFix,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
            alt.a_ft,
        ),
        oa_flight::ALT_NONE | oa_flight::ALT_NIL => (
            t::ClearanceForm::Absent,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
        ),
        oa_flight::ALT_VFR => (
            t::ClearanceForm::Vfr,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
        ),
        _ => (
            t::ClearanceForm::Unsupported,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
        ),
    };
    t::Clearance {
        source_ms: stamp.ts_ms,
        available_ms: stamp.avail_ms,
        message_id: stamp.msg,
        min_altitude_ft: low,
        max_altitude_ft: high,
        altitude_at_fix_ft: at_fix,
        form,
        reserved: [0; 3],
    }
}

// A7.2: cold one-receipt adapter value, used only to call both pure issue-time modes on identical inputs.
struct PreparedRouteIssue<'a> {
    key: t::FlightKey,
    route: t::RouteProvenance,
    cutoff_ms: i64,
    max_track_age_ms: i64,
    owner: t::OwnerProvenance,
    route_cursor: Option<t::ActiveRouteCursor>,
    track: t::CurrentTrack,
    clearance: t::Clearance,
    eta_target: t::EtaTarget,
    points: Cow<'a, [t::RoutePoint]>,
}

/// One scan-matched active view and its observed track for shell conflict queries.
pub struct ActiveRouteInput<'a> {
    pub view: &'a SharedFlightView,
    pub observed: &'a ObservedTrack,
}

/// A full-key, scan-bound direct-hold profile prepared once by the regional runtime.
#[derive(Clone, Copy)]
pub struct HoldRouteProfile<'a> {
    pub state: &'a t::HoldPhaseState,
    pub config: crate::holding::HoldPredictionIntent,
    pub revision: u64,
    pub points: &'a [t::RoutePoint; HOLD_PROFILE_POINTS],
}

pub struct PairRouteProbeInput<'a> {
    pub first: ActiveRouteInput<'a>,
    pub first_hold: Option<HoldRouteProfile<'a>>,
    pub second: ActiveRouteInput<'a>,
    pub second_hold: Option<HoldRouteProfile<'a>>,
    pub cutoff_ns: i64,
    pub frame_sequence: u64,
}

/// Caller-supplied active SUA geometry and altitude band for one shared route query.
pub struct SuaRouteProbeInput<'a> {
    pub active: ActiveRouteInput<'a>,
    pub cutoff_ns: i64,
    pub frame_sequence: u64,
    pub area: &'a oa_airspace::core::geom::Area,
    pub floor_ft: i32,
    pub ceiling_ft: i32,
}

/// Explicit operator-supplied route amendment and scan-matched counterpart for a detached trial.
pub struct TrialRouteProbeInput<'a> {
    pub candidate_event: &'a NormalizedEvent,
    pub candidate_view: &'a SharedFlightView,
    pub candidate_track: &'a ObservedTrack,
    pub counterpart_view: &'a SharedFlightView,
    pub counterpart_track: &'a ObservedTrack,
    pub cutoff_ns: i64,
    pub frame_sequence: u64,
}

fn prepared_input<'a>(prepared: &'a PreparedRouteIssue<'_>) -> t::TrajectoryIssueInput<'a> {
    t::TrajectoryIssueInput {
        key: prepared.key,
        route: prepared.route,
        cutoff_ms: prepared.cutoff_ms,
        max_track_age_ms: prepared.max_track_age_ms,
        owner: prepared.owner,
        route_cursor: prepared.route_cursor,
        track: prepared.track,
        clearance: prepared.clearance,
        eta_target: prepared.eta_target,
        points: prepared.points.as_ref(),
    }
}

/// Prepare a bounded synthetic direct-hold path for the same issue-time sampler used by
/// filed-route pair and SUA probes. Its evidence digest describes the accepted hold parameters,
/// not the filed route; unsupported hold procedures never reach this adapter.
fn prepare_hold_phase_issue<'a>(
    view: &SharedFlightView,
    observed: &ObservedTrack,
    cutoff_ns: i64,
    frame_sequence: u64,
    profile: HoldRouteProfile<'a>,
) -> Result<PreparedRouteIssue<'a>, String> {
    let state = profile.state;
    let config = profile.config;
    let key = flight_key(view);
    let cutoff_ms = ms(cutoff_ns)?;
    if state.key != key
        || state.last_advance_ns != observed.state_ns
        || (state.entry_model == 0 && state.entry_state_ns != config.entry_state_ns)
        || (state.entry_model == 1
            && (config.entry_state_ns != 0 || state.entry_available_ns > state.last_advance_ns))
        || state.inbound_true_course_cdeg != config.inbound_true_course_cdeg
        || state.leg_duration_ms != config.leg_duration_ms
        || state.turn_rate_cdeg_s != config.turn_rate_cdeg_s
        || state.max_scan_gap_ns != config.max_scan_gap_ns
        || state.max_speed_error_mmps != config.max_speed_error_mmps
    {
        return Err("hold phase has not reconciled this exact scan/full key".into());
    }
    if observed.newest_source_ns > observed.state_ns
        || observed.state_ns > cutoff_ns
        || observed.newest_source_ns > cutoff_ns
    {
        return Err("hold observed track is future-stamped".into());
    }
    let east = f64::from(observed.east_mmps);
    let north = f64::from(observed.north_mmps);
    let speed = oa_ieee::round(oa_math::hypot(east, north));
    if !speed.is_finite() || speed < 1.0 || speed >= f64::from(u32::MAX) {
        return Err("hold observed groundspeed unrepresentable".into());
    }
    if (speed - f64::from(state.groundspeed_mmps)).abs() > f64::from(state.max_speed_error_mmps) {
        return Err("hold observed groundspeed exceeds its explicit model tolerance".into());
    }
    let mut payload = [0_u8; 164];
    payload[..12].copy_from_slice(b"HOLDPHASE001");
    payload[12..48].copy_from_slice(&key_bytes(key));
    payload[48..56].copy_from_slice(&state.entry_state_ns.to_le_bytes());
    payload[56..60].copy_from_slice(&state.fix_lat_e7.to_le_bytes());
    payload[60..64].copy_from_slice(&state.fix_lon_e7.to_le_bytes());
    payload[64..66].copy_from_slice(&state.inbound_true_course_cdeg.to_le_bytes());
    payload[66] = state.turn;
    payload[67..71].copy_from_slice(&state.groundspeed_mmps.to_le_bytes());
    payload[71..75].copy_from_slice(&state.leg_duration_ms.to_le_bytes());
    payload[75..77].copy_from_slice(&state.turn_rate_cdeg_s.to_le_bytes());
    payload[77..85].copy_from_slice(&state.max_scan_gap_ns.to_le_bytes());
    payload[85..89].copy_from_slice(&config.max_speed_error_mmps.to_le_bytes());
    payload[89..93].copy_from_slice(&config.max_position_error_mm.to_le_bytes());
    payload[93..95].copy_from_slice(&config.max_heading_error_cdeg.to_le_bytes());
    payload[95] = 1;
    payload[96] = state.entry_model;
    payload[97] = state.entry_procedure;
    payload[98] = state.entry_turn_direction;
    payload[99..101].copy_from_slice(&state.entry_start_heading_true_cdeg.to_le_bytes());
    payload[101..105].copy_from_slice(&state.entry_start_lat_e7.to_le_bytes());
    payload[105..109].copy_from_slice(&state.entry_start_lon_e7.to_le_bytes());
    payload[109..117].copy_from_slice(&state.entry_available_ns.to_le_bytes());
    payload[117..125].copy_from_slice(&state.entry_first_turn_ns.to_le_bytes());
    payload[125..133].copy_from_slice(&state.entry_straight_leg_ns.to_le_bytes());
    payload[133..141].copy_from_slice(&state.entry_intercept_turn_ns.to_le_bytes());
    payload[141..149].copy_from_slice(&state.entry_join_ns.to_le_bytes());
    payload[149..157].copy_from_slice(&state.entry_regular_phase_offset_ns.to_le_bytes());
    payload[157] = match config.course_basis {
        crate::holding::HoldEntryCourseBasis::SyntheticNoWindCourse => 1,
        crate::holding::HoldEntryCourseBasis::WindOrHeadingUnknown => 2,
    };
    payload[158..162].copy_from_slice(&config.entry_capture_radius_mm.to_le_bytes());
    let route = t::RouteProvenance {
        key,
        source_ms: ms(state.entry_state_ns)?,
        available_ms: cutoff_ms,
        message_id: profile.revision,
        payload_sha256: oa_sha256::sha256(&payload),
        reserved: [0; 4],
    };
    let owner = t::OwnerProvenance {
        source_ms: view.owner_stamp.ts_ms,
        available_ms: view.owner_stamp.avail_ms,
        message_id: view.owner_stamp.msg,
        owner_unit: view.owner_unit,
        owner_state: if view.owner_unit == [0; 8] {
            t::OwnerState::Unknown
        } else {
            t::OwnerState::Known
        },
        reserved: [0; 7],
    };
    let target = profile
        .points
        .first()
        .ok_or("hold sample set is empty")?
        .fix;
    let cursor = t::ActiveRouteCursor {
        source_ms: route.source_ms,
        available_ms: cutoff_ms,
        message_id: profile.revision,
        route_payload_sha256: route.payload_sha256,
        key,
        next_fix: target,
        reserved: [0; 5],
    };
    Ok(PreparedRouteIssue {
        key,
        route,
        cutoff_ms,
        max_track_age_ms: 5_000,
        owner,
        route_cursor: Some(cursor),
        track: t::CurrentTrack {
            state_ms: ms(observed.state_ns)?,
            newest_source_ms: ms(observed.newest_source_ns)?,
            available_ms: cutoff_ms,
            message_id: frame_sequence,
            aircraft_address: observed.address,
            lat_e7: observed.lat_e7,
            lon_e7: observed.lon_e7,
            altitude_ft: observed.altitude_ft,
            vertical_rate_fpm: observed.vertical_rate_fpm,
            groundspeed_mmps: state.groundspeed_mmps,
        },
        clearance: clearance_evidence(view),
        eta_target: t::EtaTarget {
            present: true,
            fix: target,
        },
        points: Cow::Borrowed(profile.points.as_slice()),
    })
}

/// Build the bounded hold route profile once per accepted phase and scan. All probes borrow the
/// same immutable 240-point slice, avoiding route reconstruction for each pair/SUA candidate.
pub(crate) fn fill_hold_phase_profile(
    view: &SharedFlightView,
    observed: &ObservedTrack,
    cutoff_ns: i64,
    state: &t::HoldPhaseState,
    config: crate::holding::HoldPredictionIntent,
    points: &mut [t::RoutePoint; HOLD_PROFILE_POINTS],
) -> Result<(), String> {
    // The 240 points span 22 minutes. The two-minute tail keeps the route sampler's
    // straight great-circle legs covered through its complete 20-minute probe horizon.
    const HOLD_SAMPLE_STEP_NS: i64 = 5_500_000_000;
    if state.key != flight_key(view)
        || state.last_advance_ns != observed.state_ns
        || (state.entry_model == 0 && state.entry_state_ns != config.entry_state_ns)
        || (state.entry_model == 1
            && (config.entry_state_ns != 0 || state.entry_available_ns > state.last_advance_ns))
        || state.inbound_true_course_cdeg != config.inbound_true_course_cdeg
        || state.leg_duration_ms != config.leg_duration_ms
        || state.turn_rate_cdeg_s != config.turn_rate_cdeg_s
        || state.max_scan_gap_ns != config.max_scan_gap_ns
        || state.max_speed_error_mmps != config.max_speed_error_mmps
        || observed.newest_source_ns > observed.state_ns
        || observed.state_ns > cutoff_ns
        || observed.newest_source_ns > cutoff_ns
    {
        return Err("hold profile identity/scan is not reconciled".into());
    }
    let measured = oa_ieee::round(oa_math::hypot(
        f64::from(observed.east_mmps),
        f64::from(observed.north_mmps),
    ));
    if !measured.is_finite()
        || !(1.0..f64::from(u32::MAX)).contains(&measured)
        || (measured - f64::from(state.groundspeed_mmps)).abs()
            > f64::from(state.max_speed_error_mmps)
    {
        return Err("hold profile groundspeed is outside declared model".into());
    }
    for (index, sample) in (1..=HOLD_PROFILE_POINTS as i64).enumerate() {
        let at_ns = cutoff_ns
            .checked_add(sample * HOLD_SAMPLE_STEP_NS)
            .ok_or("hold forecast timestamp overflow")?;
        let snapshot = t::hold_phase_predict(state, at_ns)
            .map_err(|reason| format!("hold phase forecast unavailable: {reason:?}"))?;
        let point = points
            .get_mut(index)
            .ok_or("hold profile output capacity invariant failed")?;
        let ident = hold_profile_fix(index).ok_or("hold profile fix identity capacity failed")?;
        *point = t::RoutePoint {
            lat_e7: snapshot.lat_e7,
            lon_e7: snapshot.lon_e7,
            min_altitude_ft: t::NO_ALTITUDE_FT,
            max_altitude_ft: t::NO_ALTITUDE_FT,
            min_speed_mmps: t::NO_SPEED_MMPS,
            max_speed_mmps: t::NO_SPEED_MMPS,
            fix: t::FixIdentity {
                ident: t::RouteIdentifier {
                    len: 8,
                    bytes: ident,
                },
                region: view.centre,
                scope: t::RouteIdentifier {
                    len: 0,
                    bytes: [0; 12],
                },
                class: 1,
            },
            altitude_constraint_kind: t::AltitudeConstraintKind::None,
            speed_constraint_kind: t::SpeedConstraintKind::None,
            geometry: t::PathGeometry::GreatCircleLeg,
            speed_reference: t::SpeedReference::GroundSpeed,
            reserved: [0; 1],
        };
    }
    Ok(())
}

fn hold_profile_fix(point_index: usize) -> Option<[u8; 12]> {
    let mut ident = *b"HLD00000\0\0\0\0";
    let mut value = point_index;
    for position in (3..8).rev() {
        *ident.get_mut(position)? = b'0' + (value % 10) as u8;
        value /= 10;
    }
    Some(ident)
}

fn key_bytes(key: t::FlightKey) -> [u8; 36] {
    let mut bytes = [0; 36];
    bytes[..16].copy_from_slice(&key.gufi);
    bytes[16..20].copy_from_slice(&key.aircraft_address.to_le_bytes());
    bytes[20..24].copy_from_slice(&key.centre);
    bytes[24..].copy_from_slice(&key.plan_id);
    bytes
}

/// Converts one receipt into the exact issue-time values shared by horizontal and optional
/// vertical forecasting. Both modes use this selector, cursor, track, clearance and route slice.
fn prepare_route_issue(
    store: &RouteStore,
    view: &SharedFlightView,
    observed: &ObservedTrack,
    cutoff_ns: i64,
    frame_sequence: u64,
) -> Result<PreparedRouteIssue<'static>, String> {
    let cutoff_ms = ms(cutoff_ns)?;
    if !flight_view_in_cutoff(view, cutoff_ms) {
        return Err("flight view has invalid or future-stamped state at cutoff".into());
    }
    let key = flight_key(view);
    if store.revisions.len() > store.revision_capacity
        || store.route_refusals.len() > store.refusal_capacity
        || (!store.reclaim_current_by_key
            && store.revisions.len() + store.route_refusals.len() > store.revision_capacity)
        || store.cursors.len() > MAX_ROUTE_REVISIONS
        || store.cursors.len() != store.cursor_text_hashes.len()
    {
        return Err("route store budget exceeded or cursor columns inconsistent".into());
    }
    if route_history_expired(store, key, cutoff_ms)? {
        return Err("route as-of history expired after current-route reclamation".into());
    }
    // The selector's quadratic evidence contract remains capped at 256 candidates. The current
    // route pool is indexed by full FlightKey, so only this aircraft's current evidence enters it.
    let evidence: Vec<_> = store
        .revisions
        .iter()
        .filter(|r| r.evidence.key == key)
        .map(|r| r.evidence)
        .collect();
    let selected = t::select_revision_evidence(key, cutoff_ms, &evidence);
    if selected.available == t::Availability::Unavailable {
        return Err(format!(
            "route evidence: {:?}; counts={:?}",
            selected.refusal, selected.counts
        ));
    }
    if selected.selected.key != key {
        return Err("newest route revision does not match frozen flight plan".into());
    }
    if route_refusal_blocks(store, selected.selected, cutoff_ms)? {
        return Err("route refusal fence makes selected revision unavailable".into());
    }
    let revision = store
        .revisions
        .iter()
        .find(|r| r.evidence == selected.selected)
        .ok_or("selected revision unavailable")?;
    if revision.report.failure != RouteFailure::None {
        return Err(format!("route conversion: {:?}", revision.report));
    }
    let pairs = store
        .cursors
        .len()
        .checked_mul(store.cursors.len().saturating_sub(1))
        .map(|n| n / 2)
        .ok_or("route cursor comparison count overflow")?;
    if pairs > MAX_CURSOR_COMPARISONS {
        return Err("route cursor comparison budget exceeded".into());
    }
    // Detect same-source conflicts across the entire eligible history before selecting its newest
    // row. Comparing only against the current winner can hide an older conflict behind a newer
    // cursor, which would make selection depend on storage order.
    for (i, (left, left_hash)) in store
        .cursors
        .iter()
        .zip(&store.cursor_text_hashes)
        .enumerate()
    {
        if left.available_ms > cutoff_ms || !same_flight(left.key, key) {
            continue;
        }
        if left.source_ms > left.available_ms
            || left.available_ms < 0
            || left.source_ms < 0
            || left.reserved != [0; 5]
        {
            return Err("invalid active cursor evidence".into());
        }
        for (right, right_hash) in store
            .cursors
            .iter()
            .zip(&store.cursor_text_hashes)
            .skip(i + 1)
        {
            if right.available_ms > cutoff_ms
                || !same_flight(right.key, key)
                || right.source_ms != left.source_ms
            {
                continue;
            }
            if right.source_ms > right.available_ms
                || right.available_ms < 0
                || right.source_ms < 0
                || right.reserved != [0; 5]
            {
                return Err("invalid active cursor evidence".into());
            }
            if right.key != left.key || right.next_fix != left.next_fix || right_hash != left_hash {
                return Err("same-source cursor conflict".into());
            }
        }
    }
    let mut selected_cursor: Option<(t::ActiveRouteCursor, [u8; 32])> = None;
    for (candidate, text_hash) in store.cursors.iter().zip(&store.cursor_text_hashes) {
        if candidate.available_ms > cutoff_ms || !same_flight(candidate.key, key) {
            continue;
        }
        if candidate.source_ms > candidate.available_ms
            || candidate.available_ms < 0
            || candidate.source_ms < 0
            || candidate.reserved != [0; 5]
        {
            return Err("invalid active cursor evidence".into());
        }
        if selected_cursor.is_some_and(|(prior, _)| {
            (candidate.source_ms, candidate.message_id) <= (prior.source_ms, prior.message_id)
        }) {
            continue;
        }
        selected_cursor = Some((*candidate, *text_hash));
    }
    let cursor = match selected_cursor {
        Some((mut chosen, text_hash)) => {
            if chosen.key != key {
                return Err("newest active cursor belongs to a different plan revision".into());
            }
            if text_hash != revision.text_sha256 {
                return Err("active cursor binds a different filed-route payload".into());
            }
            chosen.route_payload_sha256 = revision.evidence.payload_sha256;
            Some(chosen)
        }
        None => None,
    };
    let points = trajectory_points(&revision.converted)?;
    let east = f64::from(observed.east_mmps);
    let north = f64::from(observed.north_mmps);
    let speed = oa_ieee::round(oa_ieee::sqrt(east * east + north * north));
    if !speed.is_finite() || speed < 0.0 || speed >= f64::from(u32::MAX) {
        return Err("observed groundspeed unrepresentable".into());
    }
    let state_ms = ms(observed.state_ns)?;
    let newest_source_ms = ms(observed.newest_source_ns)?;
    if state_ms > cutoff_ms || newest_source_ms > cutoff_ms || newest_source_ms > state_ms {
        return Err("observed track is future-stamped or has source after state".into());
    }
    let track = t::CurrentTrack {
        state_ms,
        newest_source_ms,
        available_ms: cutoff_ms,
        message_id: frame_sequence,
        aircraft_address: observed.address,
        lat_e7: observed.lat_e7,
        lon_e7: observed.lon_e7,
        altitude_ft: observed.altitude_ft,
        vertical_rate_fpm: observed.vertical_rate_fpm,
        groundspeed_mmps: speed as u32,
    };
    let owner = t::OwnerProvenance {
        source_ms: view.owner_stamp.ts_ms,
        available_ms: view.owner_stamp.avail_ms,
        message_id: view.owner_stamp.msg,
        owner_unit: view.owner_unit,
        owner_state: if view.owner_unit == [0; 8] {
            t::OwnerState::Unknown
        } else {
            t::OwnerState::Known
        },
        reserved: [0; 7],
    };
    let r = revision.evidence;
    let route = t::RouteProvenance {
        key: r.key,
        source_ms: r.source_ms,
        available_ms: r.available_ms,
        message_id: r.message_id,
        payload_sha256: r.payload_sha256,
        reserved: [0; 4],
    };
    let target = points.last().ok_or("converted route is empty")?.fix;
    Ok(PreparedRouteIssue {
        key,
        route,
        cutoff_ms,
        max_track_age_ms: 5000,
        owner,
        route_cursor: cursor,
        track,
        clearance: clearance_evidence(view),
        eta_target: t::EtaTarget {
            present: true,
            fix: target,
        },
        points: Cow::Owned(points),
    })
}

/// Returns one issue-time horizontal result using the same prepared input as optional vertical mode.
pub fn route_issue(
    store: &RouteStore,
    view: &SharedFlightView,
    observed: &ObservedTrack,
    cutoff_ns: i64,
    frame_sequence: u64,
) -> Result<t::TrajectoryIssueOutput, String> {
    let prepared = prepare_route_issue(store, view, observed, cutoff_ns, frame_sequence)?;
    Ok(t::issue_trajectory(&prepared_input(&prepared)))
}

/// Returns the horizontal baseline and bounded vertical profile from one shared prepared input.
pub fn route_issue_with_performance(
    store: &RouteStore,
    view: &SharedFlightView,
    observed: &ObservedTrack,
    cutoff_ns: i64,
    frame_sequence: u64,
    performance: t::PerformanceEnvelope,
) -> Result<(t::TrajectoryIssueOutput, t::VerticalProfileOutput), String> {
    let prepared = prepare_route_issue(store, view, observed, cutoff_ns, frame_sequence)?;
    let input = prepared_input(&prepared);
    Ok((
        t::issue_trajectory(&input),
        t::issue_vertical_profile(&input, performance),
    ))
}

/// Evaluate two scan-matched active plans with the shared route predictor. This is a query over
/// the current admitted route/flight snapshots; it does not commit or revise either plan.
pub fn route_pair_probe(
    store: &RouteStore,
    first: ActiveRouteInput<'_>,
    second: ActiveRouteInput<'_>,
    cutoff_ns: i64,
    frame_sequence: u64,
    scratch: &mut crate::conflict_probe::ConflictProbeScratch,
) -> Result<oa_ca::probe::ProbeResult, String> {
    let first = prepare_route_issue(store, first.view, first.observed, cutoff_ns, frame_sequence)?;
    let second = prepare_route_issue(
        store,
        second.view,
        second.observed,
        cutoff_ns,
        frame_sequence,
    )?;
    Ok(crate::conflict_probe::probe_active_pair(
        &prepared_input(&first),
        &prepared_input(&second),
        scratch,
    ))
}

/// Pair probe using an already scan-reconciled direct hold for either participant. A missing
/// phase leaves that flight on its admitted filed route; callers must pass a phase only for the
/// exact full key and scan that it reconciled.
pub fn route_pair_probe_with_holds(
    store: &RouteStore,
    input: PairRouteProbeInput<'_>,
    scratch: &mut crate::conflict_probe::ConflictProbeScratch,
) -> Result<oa_ca::probe::ProbeResult, String> {
    let first = match input.first_hold {
        Some(profile) => prepare_hold_phase_issue(
            input.first.view,
            input.first.observed,
            input.cutoff_ns,
            input.frame_sequence,
            profile,
        )?,
        None => prepare_route_issue(
            store,
            input.first.view,
            input.first.observed,
            input.cutoff_ns,
            input.frame_sequence,
        )?,
    };
    let second = match input.second_hold {
        Some(profile) => prepare_hold_phase_issue(
            input.second.view,
            input.second.observed,
            input.cutoff_ns,
            input.frame_sequence,
            profile,
        )?,
        None => prepare_route_issue(
            store,
            input.second.view,
            input.second.observed,
            input.cutoff_ns,
            input.frame_sequence,
        )?,
    };
    Ok(crate::conflict_probe::probe_active_pair(
        &prepared_input(&first),
        &prepared_input(&second),
        scratch,
    ))
}

/// Screen one active plan against a startup-owned continuous SUA geometry using the shared
/// route predictor. The caller's admitted model supplies the geometry and MSL band.
pub fn route_sua_probe(
    store: &RouteStore,
    input: SuaRouteProbeInput<'_>,
    scratch: &mut crate::conflict_probe::ConflictProbeScratch,
) -> Result<oa_airspace::core::conflict_probe::SuaProbeResult, String> {
    let prepared = prepare_route_issue(
        store,
        input.active.view,
        input.active.observed,
        input.cutoff_ns,
        input.frame_sequence,
    )?;
    let trajectory =
        crate::conflict_probe::sample_active_trajectory(&prepared_input(&prepared), scratch)
            .ok_or("shared route profile unavailable for SUA screening")?;
    Ok(crate::conflict_probe::probe_sua(
        trajectory,
        input.area,
        true,
        i64::MIN,
        i64::MAX,
        input.floor_ft,
        input.ceiling_ft,
    ))
}

/// Screen a current direct-hold trajectory against a SUA using the same shared sampler used by
/// filed routes. The phase must already have reconciled the supplied observed scan.
pub fn route_hold_sua_probe(
    input: SuaRouteProbeInput<'_>,
    profile: HoldRouteProfile<'_>,
    scratch: &mut crate::conflict_probe::ConflictProbeScratch,
) -> Result<oa_airspace::core::conflict_probe::SuaProbeResult, String> {
    let prepared = prepare_hold_phase_issue(
        input.active.view,
        input.active.observed,
        input.cutoff_ns,
        input.frame_sequence,
        profile,
    )?;
    let trajectory =
        crate::conflict_probe::sample_active_trajectory(&prepared_input(&prepared), scratch)
            .ok_or("shared hold profile unavailable for SUA screening")?;
    Ok(crate::conflict_probe::probe_sua(
        trajectory,
        input.area,
        true,
        i64::MIN,
        i64::MAX,
        input.floor_ft,
        input.ceiling_ft,
    ))
}

/// Evaluate an explicitly supplied filed-route candidate against another active plan. The
/// candidate is converted in a temporary route store; no admitted route or flight state changes.
pub fn route_trial_pair_probe(
    active_store: &RouteStore,
    model: &Model,
    input: TrialRouteProbeInput<'_>,
    scratch: &mut crate::conflict_probe::ConflictProbeScratch,
) -> Result<oa_ca::probe::ProbeResult, String> {
    route_trial_pair_probe_with_holds(active_store, model, input, None, None, scratch)
}

/// Trial a filed candidate while retaining modeled direct-hold paths for the active baseline
/// and counterpart. The detached candidate remains an explicit filed-route replacement.
pub fn route_trial_pair_probe_with_holds(
    active_store: &RouteStore,
    model: &Model,
    input: TrialRouteProbeInput<'_>,
    active_hold: Option<HoldRouteProfile<'_>>,
    counterpart_hold: Option<HoldRouteProfile<'_>>,
    scratch: &mut crate::conflict_probe::ConflictProbeScratch,
) -> Result<oa_ca::probe::ProbeResult, String> {
    if input.candidate_event.source != SourceKind::Route
        || !matches!(input.candidate_event.kind, EventKind::FiledRoute { .. })
        || input.candidate_view.gufi != input.candidate_event.identity.flight_gufi
        || input.candidate_view.address != input.candidate_event.identity.aircraft_address
        || input.candidate_view.centre != input.candidate_event.centre
        || input.candidate_view.gufi == input.counterpart_view.gufi
    {
        return Err(
            "trial request does not identify a distinct active route and counterpart".into(),
        );
    }
    let mut candidate_store = route_store_new(active_store.snapshot_sha256);
    let stage = route_stage_event(&candidate_store, model, input.candidate_event)?;
    if !matches!(&stage, RouteStage::Revision(_)) {
        return Err("trial route did not produce an accepted detached revision".into());
    }
    route_commit_stage(&mut candidate_store, stage);
    let active = match active_hold {
        Some(profile) => prepare_hold_phase_issue(
            input.candidate_view,
            input.candidate_track,
            input.cutoff_ns,
            input.frame_sequence,
            profile,
        )?,
        None => prepare_route_issue(
            active_store,
            input.candidate_view,
            input.candidate_track,
            input.cutoff_ns,
            input.frame_sequence,
        )?,
    };
    let active_cursor = active
        .route_cursor
        .ok_or("trial candidate requires an explicit active route cursor")?;
    let candidate_text_hash = candidate_store
        .revisions
        .first()
        .map(|revision| revision.text_sha256)
        .ok_or("detached trial revision is missing")?;
    // Carry forward only the already-admitted active next-fix cursor. The candidate must still
    // contain that exact unique fix; prepare_route_issue validates it against candidate geometry.
    candidate_store.cursors.push(active_cursor);
    candidate_store.cursor_text_hashes.push(candidate_text_hash);
    let counterpart = match counterpart_hold {
        Some(profile) => prepare_hold_phase_issue(
            input.counterpart_view,
            input.counterpart_track,
            input.cutoff_ns,
            input.frame_sequence,
            profile,
        )?,
        None => prepare_route_issue(
            active_store,
            input.counterpart_view,
            input.counterpart_track,
            input.cutoff_ns,
            input.frame_sequence,
        )?,
    };
    let trial = prepare_route_issue(
        &candidate_store,
        input.candidate_view,
        input.candidate_track,
        input.cutoff_ns,
        input.frame_sequence,
    )?;
    crate::conflict_probe::probe_trial_plan(
        &prepared_input(&active),
        &prepared_input(&counterpart),
        0,
        &prepared_input(&trial),
        scratch,
    )
    .ok_or_else(|| "trial candidate changes non-route active planning inputs".into())
}

/// Converts a receipt nanosecond timestamp through the shared exact-millisecond gate.
pub(crate) fn receipt_cutoff_ms(cutoff_ns: i64) -> Result<i64, String> {
    ms(cutoff_ns)
}

#[cfg(test)]
mod refusal_fence_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn key(plan_id: [u8; 12]) -> t::FlightKey {
        t::FlightKey {
            gufi: 1u128.to_be_bytes(),
            aircraft_address: 0x00ab_cdef,
            centre: *b"ZAU1",
            plan_id,
        }
    }

    fn revision(key: t::FlightKey, source_ms: i64, available_ms: i64) -> t::RevisionEvidence {
        t::RevisionEvidence {
            source_ms,
            available_ms,
            message_id: 1,
            payload_sha256: [1; 32],
            key,
            reserved: [0; 4],
        }
    }

    fn stored_revision(key: t::FlightKey, source_ms: i64, available_ms: i64) -> RouteRevision {
        RouteRevision {
            evidence: revision(key, source_ms, available_ms),
            text_sha256: [source_ms as u8; 32],
            report: RouteReport {
                failure: RouteFailure::None,
                offered: 0,
                resolved: 0,
                unsupported: 0,
                refused: 0,
                unexamined: 0,
                work_used: 0,
                work_limit: oa_airspace::core::route::MAX_ROUTE_WORK,
            },
            converted: empty_converted_route(),
        }
    }

    // REQ: OA-REG-018
    #[test]
    fn route_refusal_fence_is_scoped_and_preserved_for_historical_cutoffs() {
        let plan_a = key(*b"PLAN00000001");
        let plan_b = key(*b"PLAN00000002");
        let mut store = route_store_new([0; 32]);
        store.route_refusals.push(RouteRefusalFence {
            key: plan_a,
            source_ms: Some(2_000),
            available_ms: 2_000,
            message_id: 2,
            attempted_text_sha256: [2; 32],
            reason: oa_quarantine::REASON_ALTITUDE_ROUTE_INVALID,
        });

        assert!(!route_refusal_blocks(&store, revision(plan_a, 1_000, 1_000), 1_000).unwrap());
        assert!(route_refusal_blocks(&store, revision(plan_a, 1_000, 2_000), 2_000).unwrap());
        assert!(route_refusal_blocks(&store, revision(plan_a, 2_000, 2_000), 2_000).unwrap());
        assert!(!route_refusal_blocks(&store, revision(plan_a, 3_000, 3_000), 3_000).unwrap());
        // A later success changes only cutoffs where it is available. The retained refusal still
        // blocks the older revision at the prior cutoff.
        assert!(route_refusal_blocks(&store, revision(plan_a, 1_000, 1_000), 2_000).unwrap());
        assert!(!route_refusal_blocks(&store, revision(plan_b, 1_000, 1_000), 2_000).unwrap());

        store.route_refusals.first_mut().unwrap().source_ms = None;
        assert!(route_refusal_blocks(&store, revision(plan_a, 1_999, 2_000), 2_000).unwrap());
        assert!(!route_refusal_blocks(&store, revision(plan_a, 2_001, 2_001), 2_001).unwrap());
    }

    // REQ: OA-REG-020
    #[test]
    fn current_route_reuse_expires_old_as_of_queries_and_bounds_churn() {
        let plan = key(*b"PLAN00000001");
        let mut store = route_store_new_with_capacity([0; 32], 2, true);
        route_commit_stage(
            &mut store,
            RouteStage::Revision(stored_revision(plan, 10, 10)),
        );
        route_commit_stage(
            &mut store,
            RouteStage::Revision(stored_revision(plan, 20, 20)),
        );
        assert_eq!(store.revisions.len(), 1);
        assert_eq!(store.history_expiries.len(), 1);
        assert!(!route_history_expired(&store, plan, 9).unwrap());
        assert!(route_history_expired(&store, plan, 10).unwrap());
        assert!(route_history_expired(&store, plan, 19).unwrap());
        assert!(!route_history_expired(&store, plan, 20).unwrap());

        for source_ms in 21..=100 {
            route_commit_stage(
                &mut store,
                RouteStage::Revision(stored_revision(plan, source_ms, source_ms)),
            );
        }
        assert_eq!(
            store.revisions.len(),
            1,
            "replacement reuses the current slot"
        );
        assert_eq!(
            store.history_expiries.len(),
            1,
            "expiry index remains per full key"
        );
        assert!(route_history_expired(&store, plan, 50).unwrap());
        assert!(!route_history_expired(&store, plan, 100).unwrap());

        let retirement = route_stage_retirement(&store, plan, 100).unwrap();
        route_commit_stage(&mut store, retirement);
        assert!(store.revisions.is_empty());
        assert!(route_history_expired(&store, plan, 100).unwrap());
        route_commit_stage(
            &mut store,
            RouteStage::Revision(stored_revision(plan, 120, 120)),
        );
        assert_eq!(store.revisions.len(), 1);
        assert!(route_history_expired(&store, plan, 119).unwrap());
        assert!(!route_history_expired(&store, plan, 120).unwrap());
        advance_route_history_watermark(&mut store, CURRENT_ROUTE_HISTORY_RETENTION_MS + 120);
        assert_eq!(store.history_watermark_ms, 120);
        assert!(store.history_expiries.is_empty());
        assert!(route_history_expired(&store, plan, 119).unwrap());
        assert!(!route_history_expired(&store, plan, 120).unwrap());
    }

    // REQ: OA-REG-020
    #[test]
    fn expiry_index_exhaustion_refuses_retirement_without_mutation() {
        let first = key(*b"PLAN00000001");
        let second = key(*b"PLAN00000002");
        let mut store = route_store_new_with_capacity([0; 32], 1, true);
        route_commit_stage(
            &mut store,
            RouteStage::Revision(stored_revision(first, 10, 10)),
        );
        let retirement = route_stage_retirement(&store, first, 11).unwrap();
        route_commit_stage(&mut store, retirement);
        route_commit_stage(
            &mut store,
            RouteStage::Revision(stored_revision(second, 20, 20)),
        );

        let before_revision_count = store.revisions.len();
        let before_expiry_count = store.history_expiries.len();
        assert!(route_stage_retirement(&store, second, 21).is_err());
        assert_eq!(store.revisions.len(), before_revision_count);
        assert_eq!(store.history_expiries.len(), before_expiry_count);
        assert_eq!(
            store.revisions.first().map(|r| r.evidence.key),
            Some(second)
        );
    }

    // REQ: OA-REG-020
    #[test]
    fn history_watermark_reclaims_only_rows_outside_supported_asof_window() {
        let plan = key(*b"PLAN00000001");
        let mut store = route_store_new_with_capacity([0; 32], 4, true);
        store.revisions.push(stored_revision(plan, 10, 10));
        store.route_refusals.push(RouteRefusalFence {
            key: plan,
            source_ms: Some(11),
            available_ms: 20,
            message_id: 1,
            attempted_text_sha256: [1; 32],
            reason: oa_quarantine::REASON_INPUT_INVALID,
        });
        store.cursors.push(t::ActiveRouteCursor {
            source_ms: 10,
            available_ms: 20,
            message_id: 1,
            route_payload_sha256: [0; 32],
            key: plan,
            next_fix: t::FixIdentity {
                ident: t::RouteIdentifier {
                    len: 0,
                    bytes: [0; 12],
                },
                region: [0; 4],
                scope: t::RouteIdentifier {
                    len: 0,
                    bytes: [0; 12],
                },
                class: 0,
            },
            reserved: [0; 5],
        });
        store.cursor_text_hashes.push([2; 32]);

        let watermark = CURRENT_ROUTE_HISTORY_RETENTION_MS + 120;
        advance_route_history_watermark(&mut store, watermark);
        assert_eq!(store.history_watermark_ms, 120);
        assert!(store.cursors.is_empty());
        assert!(store.cursor_text_hashes.is_empty());
        // The expired refusal still protects an older current route, so it cannot be reclaimed.
        assert_eq!(store.route_refusals.len(), 1);
        assert!(!source_time_is_retained(&store, 119));
        assert!(route_history_expired(&store, plan, 119).unwrap());

        assert!(store.revisions.pop().is_some());
        store.revisions.push(stored_revision(plan, 12, watermark));
        advance_route_history_watermark(&mut store, watermark + 1);
        assert!(store.route_refusals.is_empty());
    }
}

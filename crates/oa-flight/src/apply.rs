//! Applying one operation to the table: the lifecycle state machine, the field rules, the lag.
//! Core (pure).

use crate::table::{
    flight_add, flight_is_live, index_address, index_tfms, slot_of_key, tfms_complete,
    unindex_address, unindex_tfms,
};
use crate::types::{
    Alt, Conflict, Flight, FlightKey, FlightTable, Op, Outcome, Refusal, Stamp, View, ALT_AT_FIX,
    ALT_BLOCK, ALT_MAX_FT, ALT_MIN_FT, ALT_NIL, ALT_NONE, ALT_SIMPLE, ALT_VFR, ALT_VFR_ON_TOP_PLUS,
    C_ADDRESS_TWICE, C_PLAN_CHANGED, C_PLAN_REFILED, C_TFMS_KEY_TWICE, F_ADDRESS, F_ASSIGNED,
    F_HANDOFF, F_IDENTITY, F_INTERIM, F_OWNER, F_REQUESTED, HO_ACCEPTED, HO_NONE, HO_OFFERED,
    KEY_GUFI, KEY_TFMS, MAX_VIEWS, OP_ADDRESS, OP_ASSIGN, OP_CANCEL, OP_COMPLETE, OP_COUNT,
    OP_DEPART, OP_DROP, OP_HO_ACCEPT, OP_HO_FAIL, OP_HO_INITIATE, OP_HO_RETRACT, OP_HO_TAKE,
    OP_HO_UPDATE, OP_IDENTITY, OP_INTERIM, OP_OWNER, OP_REQUEST, OP_SET_PLAN, R_BAD_KEY,
    R_BAD_TIME, R_BAD_TRANSITION, R_BAD_VALUE, R_COUNT, R_HANDOFF_PENDING, R_NO_HANDOFF, R_NO_VIEW,
    R_SAME_STAMP_DIFFERENT, R_STALE, R_TABLE_FULL, R_TERMINAL, R_UNKNOWN_FLIGHT, R_VIEWS_FULL,
    ST_ACTIVE, ST_CANCELLED, ST_COMPLETED, ST_DEPARTED, ST_DROPPED, ST_NONE, ST_PROPOSED,
};

/// An operation of this kind on this flight at these times, every other part blank.
pub fn op_new(kind: u8, key: FlightKey, centre: [u8; 4], ts_ms: i64, seen_ms: i64, msg: u64) -> Op {
    Op {
        kind,
        key,
        centre,
        ts_ms,
        seen_ms,
        msg,
        state: ST_NONE,
        alt: Alt {
            form: ALT_NONE,
            a_ft: 0,
            b_ft: 0,
        },
        fix: [0; 12],
        unit_a: [0; 8],
        unit_b: [0; 8],
        plan_id: [0; 12],
        address: 0,
        callsign: [0; 8],
        dep: [0; 12],
        arr: [0; 12],
    }
}

/// What a lifecycle operation does to a state.
enum Step {
    To(u8),
    Same,
    Refuse(u8),
}

fn is_terminal(s: u8) -> bool {
    s == ST_COMPLETED || s == ST_CANCELLED
}

/// The lifecycle state machine for a plan view that exists, as a function of its state and the
/// operation (`want` is the state a [`OP_SET_PLAN`] asks for). Proposed goes to active or is
/// cancelled or dropped; active departs, completes, is cancelled or dropped; departed completes
/// or is dropped (an airborne flight is not cancelled); dropped may be reinstated by an active
/// plan or completed or cancelled; completed and cancelled are final, and a repeat of the same
/// end is a restatement. No step goes back (an active plan is not proposed again) and none skips
/// one (a plan that was never active neither departs nor completes).
fn lifecycle(cur: u8, kind: u8, want: u8) -> Step {
    use Step::{Refuse, Same, To};
    match (cur, kind) {
        (ST_PROPOSED, OP_SET_PLAN) => match want {
            ST_PROPOSED => Same,
            _ => To(ST_ACTIVE),
        },
        (ST_PROPOSED, OP_CANCEL) => To(ST_CANCELLED),
        (ST_PROPOSED, OP_DROP) => To(ST_DROPPED),
        (ST_PROPOSED, _) => Refuse(R_BAD_TRANSITION),
        (ST_ACTIVE, OP_SET_PLAN) => match want {
            ST_PROPOSED => Refuse(R_BAD_TRANSITION),
            _ => Same,
        },
        (ST_ACTIVE, OP_DEPART) => To(ST_DEPARTED),
        (ST_ACTIVE, OP_COMPLETE) => To(ST_COMPLETED),
        (ST_ACTIVE, OP_CANCEL) => To(ST_CANCELLED),
        (ST_ACTIVE, OP_DROP) => To(ST_DROPPED),
        (ST_DEPARTED, OP_SET_PLAN) => match want {
            ST_PROPOSED => Refuse(R_BAD_TRANSITION),
            _ => Same,
        },
        (ST_DEPARTED, OP_DEPART) => Same,
        (ST_DEPARTED, OP_COMPLETE) => To(ST_COMPLETED),
        (ST_DEPARTED, OP_DROP) => To(ST_DROPPED),
        (ST_DEPARTED, _) => Refuse(R_BAD_TRANSITION),
        (ST_DROPPED, OP_SET_PLAN) => match want {
            ST_PROPOSED => Refuse(R_BAD_TRANSITION),
            _ => To(ST_ACTIVE),
        },
        (ST_DROPPED, OP_COMPLETE) => To(ST_COMPLETED),
        (ST_DROPPED, OP_CANCEL) => To(ST_CANCELLED),
        (ST_DROPPED, OP_DROP) => Same,
        (ST_DROPPED, _) => Refuse(R_BAD_TRANSITION),
        (ST_COMPLETED, OP_COMPLETE) | (ST_CANCELLED, OP_CANCEL) => Same,
        (ST_COMPLETED | ST_CANCELLED, _) => Refuse(R_TERMINAL),
        _ => Refuse(R_BAD_TRANSITION),
    }
}

/// Lists a refusal and counts it.
fn refuse(t: &mut FlightTable, op: &Op, avail_ms: i64, reason: u8, state: u8) -> Outcome {
    let k = usize::from(op.kind).min(OP_COUNT - 1) * R_COUNT + usize::from(reason).min(R_COUNT - 1);
    if let Some(c) = t.stats.refused.get_mut(k) {
        *c += 1;
    }
    if t.refusals.len() < t.cfg.refusal_log {
        t.refusals.push(Refusal {
            key: op.key,
            ts_ms: op.ts_ms,
            avail_ms,
            msg: op.msg,
            op: op.kind,
            reason,
            state,
        });
    } else {
        t.stats.refusals_unlisted += 1;
    }
    Outcome::Refused(reason)
}

fn list_conflict(t: &mut FlightTable, kind: u8, a: FlightKey, b: FlightKey, ts_ms: i64, msg: u64) {
    if let Some(c) = t.stats.conflicts.get_mut(usize::from(kind)) {
        *c += 1;
    }
    if t.conflicts.len() < t.cfg.conflict_log {
        t.conflicts.push(Conflict {
            a,
            b,
            ts_ms,
            msg,
            kind,
        });
    } else {
        t.stats.conflicts_unlisted += 1;
    }
}

fn key_valid(k: &FlightKey) -> bool {
    match k.kind {
        KEY_GUFI => k.gufi != 0 && k.callsign == [0; 8] && k.dep == [0; 12] && k.arr == [0; 12],
        KEY_TFMS => k.gufi == 0 && k.callsign != [0; 8] && k.dep != [0; 12] && k.arr != [0; 12],
        _ => false,
    }
}

fn in_range(ft: i32) -> bool {
    (ALT_MIN_FT..=ALT_MAX_FT).contains(&ft)
}

/// Whether `a` is a well-formed altitude of one of the `allowed` forms, with every unused number
/// zero.
fn alt_ok(a: &Alt, allowed: &[u8]) -> bool {
    if !allowed.contains(&a.form) {
        return false;
    }
    match a.form {
        ALT_SIMPLE | ALT_VFR_ON_TOP_PLUS => in_range(a.a_ft) && a.b_ft == 0,
        ALT_BLOCK => in_range(a.a_ft) && in_range(a.b_ft) && a.a_ft <= a.b_ft,
        ALT_AT_FIX => in_range(a.a_ft) && in_range(a.b_ft),
        ALT_VFR | ALT_NIL => a.a_ft == 0 && a.b_ft == 0,
        _ => false,
    }
}

/// What a field-change attempt found.
enum Gate {
    /// The value is what the record holds.
    Same,
    /// The change may be applied.
    Go,
    Refuse(u8),
}

/// The staleness rule of a stamped field: a restatement of the held value changes nothing; a
/// different value stamped before the held one is stale, and one stamped at the same time is
/// refused (the earlier stays).
fn gate(stamp: &Stamp, ts_ms: i64, same: bool) -> Gate {
    if same {
        return Gate::Same;
    }
    if stamp.avail_ms == 0 {
        return Gate::Go;
    }
    if ts_ms < stamp.ts_ms {
        Gate::Refuse(R_STALE)
    } else if ts_ms == stamp.ts_ms {
        Gate::Refuse(R_SAME_STAMP_DIFFERENT)
    } else {
        Gate::Go
    }
}

fn is_zero8(u: &[u8; 8]) -> bool {
    *u == [0; 8]
}

/// When `op` would become available if it were applied now: `max(seen, stamped + lag floor)`,
/// held at the clock. A driver that answers a question "as of" a time calls this to know
/// which operations are still in its future.
pub fn flights_op_avail(t: &FlightTable, op: &Op) -> i64 {
    op.seen_ms
        .max(op.ts_ms.saturating_add(t.cfg.lag_floor_ms))
        .max(t.clock_ms)
}

/// Applies one operation. The availability time of the operation is
/// `max(seen, stamped + lag floor)`, held at the clock when an earlier operation was available
/// later; operations are to be given in the order they were seen. The outcome says whether the
/// record changed, the operation restated it, or it was refused (and counted and listed).
pub fn flights_apply(t: &mut FlightTable, op: &Op) -> Outcome {
    t.stats.ops += 1;
    if op.kind == 0 || usize::from(op.kind) >= OP_COUNT || !key_valid(&op.key) {
        return refuse(t, op, op.seen_ms, R_BAD_KEY, ST_NONE);
    }
    if op.ts_ms.saturating_sub(op.seen_ms) > t.cfg.max_future_ms {
        return refuse(t, op, op.seen_ms, R_BAD_TIME, ST_NONE);
    }
    let floor = op.ts_ms.saturating_add(t.cfg.lag_floor_ms);
    let mut avail = op.seen_ms;
    if floor > avail {
        avail = floor;
        t.stats.lag_raised += 1;
    }
    if avail < t.clock_ms {
        avail = t.clock_ms;
        t.stats.clock_held += 1;
    }
    t.clock_ms = avail;
    let out = apply_at(t, op, avail);
    match out {
        Outcome::Applied => t.stats.applied += 1,
        Outcome::Unchanged => t.stats.unchanged += 1,
        Outcome::Refused(_) => {}
    }
    out
}

fn apply_at(t: &mut FlightTable, op: &Op, avail: i64) -> Outcome {
    if op.kind == OP_SET_PLAN && op.state != ST_PROPOSED && op.state != ST_ACTIVE {
        return refuse(t, op, avail, R_BAD_VALUE, ST_NONE);
    }
    let slot = match slot_of_key(t, &op.key) {
        Some(s) => s,
        None if op.kind == OP_SET_PLAN => {
            let Some(s) = flight_add(t, &op.key) else {
                return refuse(t, op, avail, R_TABLE_FULL, ST_NONE);
            };
            if op.key.kind == KEY_TFMS {
                if let Some(f) = t.flights.get_mut(s as usize) {
                    f.callsign = op.key.callsign;
                    f.dep = op.key.dep;
                    f.arr = op.key.arr;
                }
                index_tfms(t, s);
            }
            s
        }
        None => return refuse(t, op, avail, R_UNKNOWN_FLIGHT, ST_NONE),
    };
    let Some(f) = t.flights.get(slot as usize).copied() else {
        return refuse(t, op, avail, R_UNKNOWN_FLIGHT, ST_NONE);
    };
    let state = view_state(&f, &op.centre);
    let result = match op.kind {
        OP_SET_PLAN | OP_DEPART | OP_COMPLETE | OP_CANCEL | OP_DROP => plan_op(t, slot, op, avail),
        _ if !flight_is_live(&f) => Err(R_TERMINAL),
        OP_ASSIGN => alt_op(t, slot, op, avail, F_ASSIGNED),
        OP_INTERIM => alt_op(t, slot, op, avail, F_INTERIM),
        OP_REQUEST => alt_op(t, slot, op, avail, F_REQUESTED),
        OP_OWNER => owner_op(t, slot, op, avail),
        OP_HO_INITIATE | OP_HO_UPDATE | OP_HO_ACCEPT | OP_HO_RETRACT | OP_HO_FAIL | OP_HO_TAKE => {
            handoff_op(t, slot, op, avail)
        }
        OP_ADDRESS => address_op(t, slot, op, avail),
        OP_IDENTITY => identity_op(t, slot, op, avail),
        _ => Err(R_BAD_KEY),
    };
    match result {
        Ok(changed) => {
            if let Some(g) = t.flights.get_mut(slot as usize) {
                g.last_avail_ms = avail;
                if changed {
                    g.ops_applied = g.ops_applied.saturating_add(1);
                }
            }
            if changed {
                Outcome::Applied
            } else {
                Outcome::Unchanged
            }
        }
        Err(reason) => refuse(t, op, avail, reason, state),
    }
}

fn view_state(f: &Flight, centre: &[u8; 4]) -> u8 {
    f.views
        .iter()
        .take(usize::from(f.n_views))
        .find(|v| v.centre == *centre)
        .map_or(ST_NONE, |v| v.state)
}

fn stamp_of(op: &Op, avail: i64) -> Stamp {
    Stamp {
        ts_ms: op.ts_ms,
        avail_ms: avail,
        msg: op.msg,
    }
}

/// A plan operation on the view of the op's centre. `Ok(changed)` or the refusal reason.
fn plan_op(t: &mut FlightTable, slot: u32, op: &Op, avail: i64) -> Result<bool, u8> {
    let f = *t.flights.get(slot as usize).ok_or(R_UNKNOWN_FLIGHT)?;
    let stamp = stamp_of(op, avail);
    let Some(k) = f
        .views
        .iter()
        .take(usize::from(f.n_views))
        .position(|v| v.centre == op.centre)
    else {
        if op.kind != OP_SET_PLAN {
            return Err(R_NO_VIEW);
        }
        if usize::from(f.n_views) >= MAX_VIEWS {
            return Err(R_VIEWS_FULL);
        }
        if op.centre == [0; 4] {
            return Err(R_BAD_VALUE);
        }
        let g = t.flights.get_mut(slot as usize).ok_or(R_UNKNOWN_FLIGHT)?;
        let n = usize::from(g.n_views);
        if let Some(v) = g.views.get_mut(n) {
            *v = View {
                centre: op.centre,
                state: op.state,
                plan_id: op.plan_id,
                stamp,
            };
        }
        g.n_views += 1;
        t.stats.views_added += 1;
        return Ok(true);
    };
    let v = f.views.get(k).copied().ok_or(R_NO_VIEW)?;
    let mut new = v;
    let mut changed = false;
    let mut conflict = None;
    if op.kind == OP_SET_PLAN && op.plan_id != [0; 12] && op.plan_id != v.plan_id {
        if v.plan_id == [0; 12] {
            new.plan_id = op.plan_id;
        } else if is_terminal(v.state) {
            // The same key filed again after its earlier plan ended: a new plan on the view.
            new.plan_id = op.plan_id;
            new.state = op.state;
            conflict = Some(C_PLAN_REFILED);
        } else {
            new.plan_id = op.plan_id;
            conflict = Some(C_PLAN_CHANGED);
        }
        changed = true;
    }
    if conflict != Some(C_PLAN_REFILED) {
        match lifecycle(v.state, op.kind, op.state) {
            Step::Refuse(r) => return Err(r),
            Step::Same => {}
            Step::To(s) => {
                new.state = s;
                changed = true;
            }
        }
    }
    if !changed {
        return Ok(false);
    }
    match gate(&v.stamp, op.ts_ms, false) {
        Gate::Refuse(r) => return Err(r),
        Gate::Same | Gate::Go => {}
    }
    new.stamp = stamp;
    if v.state == ST_DROPPED && new.state == ST_ACTIVE {
        t.stats.reinstated += 1;
    }
    if let Some(slot_view) = t
        .flights
        .get_mut(slot as usize)
        .and_then(|g| g.views.get_mut(k))
    {
        *slot_view = new;
    }
    if let Some(c) = conflict {
        list_conflict(t, c, f.key, f.key, op.ts_ms, op.msg);
    }
    Ok(true)
}

/// Assign, interim or requested altitude.
fn alt_op(t: &mut FlightTable, slot: u32, op: &Op, avail: i64, field: usize) -> Result<bool, u8> {
    let allowed: &[u8] = match field {
        F_ASSIGNED => &[
            ALT_SIMPLE,
            ALT_VFR_ON_TOP_PLUS,
            ALT_VFR,
            ALT_BLOCK,
            ALT_AT_FIX,
        ],
        F_INTERIM => &[ALT_SIMPLE, ALT_NIL],
        _ => &[ALT_SIMPLE, ALT_VFR_ON_TOP_PLUS, ALT_VFR],
    };
    if !alt_ok(&op.alt, allowed) || (op.alt.form == ALT_AT_FIX) != (op.fix != [0; 12]) {
        return Err(R_BAD_VALUE);
    }
    let f = t.flights.get(slot as usize).ok_or(R_UNKNOWN_FLIGHT)?;
    let (held, held_fix) = match field {
        F_ASSIGNED => (f.assigned, f.assigned_fix),
        F_INTERIM => (f.interim, [0; 12]),
        _ => (f.requested, [0; 12]),
    };
    let same = held == op.alt && held_fix == op.fix;
    match gate(f.stamps.get(field).ok_or(R_BAD_VALUE)?, op.ts_ms, same) {
        Gate::Same => return Ok(false),
        Gate::Refuse(r) => return Err(r),
        Gate::Go => {}
    }
    let stamp = stamp_of(op, avail);
    let g = t.flights.get_mut(slot as usize).ok_or(R_UNKNOWN_FLIGHT)?;
    match field {
        F_ASSIGNED => {
            g.assigned = op.alt;
            g.assigned_fix = op.fix;
        }
        F_INTERIM => g.interim = op.alt,
        _ => g.requested = op.alt,
    }
    if let Some(s) = g.stamps.get_mut(field) {
        *s = stamp;
    }
    Ok(true)
}

/// A unit takes ownership. Ends a pending handoff the owner has caught up with.
fn owner_op(t: &mut FlightTable, slot: u32, op: &Op, avail: i64) -> Result<bool, u8> {
    if is_zero8(&op.unit_a) {
        return Err(R_BAD_VALUE);
    }
    let f = *t.flights.get(slot as usize).ok_or(R_UNKNOWN_FLIGHT)?;
    match gate(
        f.stamps.get(F_OWNER).ok_or(R_BAD_VALUE)?,
        op.ts_ms,
        f.owner == op.unit_a,
    ) {
        Gate::Same => return Ok(false),
        Gate::Refuse(r) => return Err(r),
        Gate::Go => {}
    }
    let stamp = stamp_of(op, avail);
    let mut completed = false;
    let mut implicit = false;
    if let Some(g) = t.flights.get_mut(slot as usize) {
        g.owner = op.unit_a;
        if let Some(s) = g.stamps.get_mut(F_OWNER) {
            *s = stamp;
        }
        if g.ho_state == HO_ACCEPTED && g.ho_accepted_by == op.unit_a {
            completed = true;
        } else if g.ho_state == HO_OFFERED && g.ho_to == op.unit_a {
            implicit = true;
        }
        if completed || implicit {
            clear_handoff(g, stamp);
        }
    }
    if completed {
        t.stats.handoffs_completed += 1;
    }
    if implicit {
        t.stats.handoffs_implicit += 1;
    }
    Ok(true)
}

fn clear_handoff(g: &mut Flight, stamp: Stamp) {
    g.ho_state = HO_NONE;
    g.ho_from = [0; 8];
    g.ho_to = [0; 8];
    g.ho_accepted_by = [0; 8];
    if let Some(s) = g.stamps.get_mut(F_HANDOFF) {
        *s = stamp;
    }
}

/// The handoff steps.
fn handoff_op(t: &mut FlightTable, slot: u32, op: &Op, avail: i64) -> Result<bool, u8> {
    let f = *t.flights.get(slot as usize).ok_or(R_UNKNOWN_FLIGHT)?;
    let stamp = stamp_of(op, avail);
    let hstamp = *f.stamps.get(F_HANDOFF).ok_or(R_BAD_VALUE)?;
    // What the flight's handoff and owner become, `None` for a refusal reason.
    let mut ho = (f.ho_state, f.ho_from, f.ho_to, f.ho_accepted_by);
    let mut owner = f.owner;
    let mut take = false;
    let mut ended = false;
    match op.kind {
        OP_HO_INITIATE => {
            if is_zero8(&op.unit_a) || is_zero8(&op.unit_b) {
                return Err(R_BAD_VALUE);
            }
            match f.ho_state {
                HO_NONE => ho = (HO_OFFERED, op.unit_a, op.unit_b, [0; 8]),
                HO_OFFERED if f.ho_from == op.unit_a && f.ho_to == op.unit_b => {}
                _ => return Err(R_HANDOFF_PENDING),
            }
        }
        OP_HO_UPDATE => {
            if is_zero8(&op.unit_b) {
                return Err(R_BAD_VALUE);
            }
            match f.ho_state {
                HO_NONE => return Err(R_NO_HANDOFF),
                HO_OFFERED => ho.2 = op.unit_b,
                _ => return Err(R_BAD_TRANSITION),
            }
        }
        OP_HO_ACCEPT => {
            if is_zero8(&op.unit_b) {
                return Err(R_BAD_VALUE);
            }
            match f.ho_state {
                HO_NONE => return Err(R_NO_HANDOFF),
                HO_OFFERED => {
                    ho.0 = HO_ACCEPTED;
                    ho.3 = op.unit_b;
                }
                _ if f.ho_accepted_by == op.unit_b => {}
                _ => return Err(R_BAD_TRANSITION),
            }
        }
        OP_HO_RETRACT | OP_HO_FAIL => {
            if f.ho_state == HO_NONE {
                return Err(R_NO_HANDOFF);
            }
            ho = (HO_NONE, [0; 8], [0; 8], [0; 8]);
            ended = true;
        }
        _ => {
            // OP_HO_TAKE
            if is_zero8(&op.unit_b) {
                return Err(R_BAD_VALUE);
            }
            if f.ho_state == HO_NONE {
                return Err(R_NO_HANDOFF);
            }
            ho = (HO_NONE, [0; 8], [0; 8], [0; 8]);
            owner = op.unit_b;
            take = true;
        }
    }
    let same = ho == (f.ho_state, f.ho_from, f.ho_to, f.ho_accepted_by) && owner == f.owner;
    match gate(&hstamp, op.ts_ms, same) {
        Gate::Same => return Ok(false),
        Gate::Refuse(r) => return Err(r),
        Gate::Go => {}
    }
    if take {
        // The owner changes with it: the owner's own stamp must not be newer.
        let ostamp = f.stamps.get(F_OWNER).ok_or(R_BAD_VALUE)?;
        if owner != f.owner {
            if let Gate::Refuse(r) = gate(ostamp, op.ts_ms, false) {
                return Err(r);
            }
        }
    }
    if let Some(g) = t.flights.get_mut(slot as usize) {
        (g.ho_state, g.ho_from, g.ho_to, g.ho_accepted_by) = ho;
        if let Some(s) = g.stamps.get_mut(F_HANDOFF) {
            *s = stamp;
        }
        if take && owner != g.owner {
            g.owner = owner;
            if let Some(s) = g.stamps.get_mut(F_OWNER) {
                *s = stamp;
            }
        }
    }
    if take {
        t.stats.handoffs_completed += 1;
    } else if ended {
        if op.kind == OP_HO_RETRACT {
            t.stats.handoffs_retracted += 1;
        } else {
            t.stats.handoffs_failed += 1;
        }
    }
    Ok(true)
}

fn address_op(t: &mut FlightTable, slot: u32, op: &Op, avail: i64) -> Result<bool, u8> {
    if op.address == 0 || op.address > 0x00FF_FFFF {
        return Err(R_BAD_VALUE);
    }
    let f = *t.flights.get(slot as usize).ok_or(R_UNKNOWN_FLIGHT)?;
    match gate(
        f.stamps.get(F_ADDRESS).ok_or(R_BAD_VALUE)?,
        op.ts_ms,
        f.address == op.address,
    ) {
        Gate::Same => return Ok(false),
        Gate::Refuse(r) => return Err(r),
        Gate::Go => {}
    }
    unindex_address(t, slot);
    if let Some(g) = t.flights.get_mut(slot as usize) {
        g.address = op.address;
        if let Some(s) = g.stamps.get_mut(F_ADDRESS) {
            *s = stamp_of(op, avail);
        }
    }
    if let Some(other) = index_address(t, slot) {
        let live = t
            .flights
            .get(other as usize)
            .filter(|o| flight_is_live(o))
            .map(|o| o.key);
        if let Some(b) = live {
            list_conflict(t, C_ADDRESS_TWICE, f.key, b, op.ts_ms, op.msg);
        }
    }
    Ok(true)
}

fn identity_op(t: &mut FlightTable, slot: u32, op: &Op, avail: i64) -> Result<bool, u8> {
    let f = *t.flights.get(slot as usize).ok_or(R_UNKNOWN_FLIGHT)?;
    let callsign = if op.callsign == [0; 8] {
        f.callsign
    } else {
        op.callsign
    };
    let dep = if op.dep == [0; 12] { f.dep } else { op.dep };
    let arr = if op.arr == [0; 12] { f.arr } else { op.arr };
    if op.callsign == [0; 8] && op.dep == [0; 12] && op.arr == [0; 12] {
        return Err(R_BAD_VALUE);
    }
    match gate(
        f.stamps.get(F_IDENTITY).ok_or(R_BAD_VALUE)?,
        op.ts_ms,
        callsign == f.callsign && dep == f.dep && arr == f.arr,
    ) {
        Gate::Same => return Ok(false),
        Gate::Refuse(r) => return Err(r),
        Gate::Go => {}
    }
    unindex_tfms(t, slot);
    if let Some(g) = t.flights.get_mut(slot as usize) {
        g.callsign = callsign;
        g.dep = dep;
        g.arr = arr;
        if let Some(s) = g.stamps.get_mut(F_IDENTITY) {
            *s = stamp_of(op, avail);
        }
    }
    let complete = t.flights.get(slot as usize).is_some_and(tfms_complete);
    if complete {
        if let Some(other) = index_tfms(t, slot) {
            let live = t
                .flights
                .get(other as usize)
                .filter(|o| flight_is_live(o))
                .map(|o| o.key);
            if let Some(b) = live {
                list_conflict(t, C_TFMS_KEY_TWICE, f.key, b, op.ts_ms, op.msg);
            }
        }
    }
    Ok(true)
}

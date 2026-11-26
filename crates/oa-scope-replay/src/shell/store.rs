//! Building the row store and the callsigns (**shell**, B1, C1, C6): these allocate, and they run
//! once per drain before the frames are made (offline build), so they are not part of the core's
//! per-frame work. The core ([`crate::core::rows`]) owns the data types and [`targets_at`]
//! (crate::core::rows::targets_at) reads them without allocating.
//!
//! The capacities are a parameter ([`StoreCaps`], [`MAX_CALLSIGN_REPORTS`] through
//! [`callsigns_build_with`]) so that a test can use small ones; the production values are the
//! constants of the core.

use crate::core::rows::{
    alt_to_ft, velocity_of, CallsignStats, Callsigns, Row, RowColumns, RowStore, RowsError,
    StoreStats, MAX_CALLSIGN_REPORTS, MAX_ROWS_PER_ADDRESS, MAX_STORE_ADDRESSES, MAX_STORE_ROWS,
    RF_HAS_ALT, RF_REACQUIRED,
};
use oa_display::core::traffic::{NO_ALT, NO_U16};
use oa_track::{NO_IDENT, NO_IDENT_NS};

/// The capacities of the store: addresses, rows of one address, rows in all.
// A7.2: cold struct, size guard waived (one per run).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreCaps {
    pub addresses: usize,
    pub rows_per_address: usize,
    pub rows: usize,
}

/// The production capacities (see the analysis in the core).
pub const STORE_CAPS: StoreCaps = StoreCaps {
    addresses: MAX_STORE_ADDRESSES,
    rows_per_address: MAX_ROWS_PER_ADDRESS,
    rows: MAX_STORE_ROWS,
};

fn check_len(column: &'static str, got: usize, want: usize) -> Result<(), RowsError> {
    if got == want {
        Ok(())
    } else {
        Err(RowsError::ColumnLength { column, got, want })
    }
}

/// Adds the rows of `cols` at or after `keep_from_ns` (earlier ones are not needed for any frame,
/// and are counted as before the window). The column lengths and the room left in the store are
/// checked before any row is read; a row that cannot be added later (its address would be
/// the store's last, or its address is full) is refused and counted.
pub fn store_add(
    store: &mut RowStore,
    cols: &RowColumns<'_>,
    keep_from_ns: i64,
) -> Result<StoreStats, RowsError> {
    store_add_with(store, cols, keep_from_ns, &STORE_CAPS)
}

/// [`store_add`] with the capacities given.
pub fn store_add_with(
    store: &mut RowStore,
    cols: &RowColumns<'_>,
    keep_from_ns: i64,
    caps: &StoreCaps,
) -> Result<StoreStats, RowsError> {
    let n = cols.t_ns.len();
    check_len("addr", cols.addr.len(), n)?;
    check_len("ident", cols.ident.len(), n)?;
    check_len("ident_ns", cols.ident_ns.len(), n)?;
    check_len("ident_conflict", cols.ident_conflict.len(), n)?;
    check_len("lat_e7", cols.lat_e7.len(), n)?;
    check_len("lon_e7", cols.lon_e7.len(), n)?;
    check_len("vel_east_mps", cols.vel_east_mps.len(), n)?;
    check_len("vel_north_mps", cols.vel_north_mps.len(), n)?;
    check_len("r95_m", cols.r95_m.len(), n)?;
    check_len("alt_ft", cols.alt_ft.len(), n)?;
    check_len("flags", cols.flags.len(), n)?;
    // Room, from the sizes the input declares, before the work (C7).
    let wanted = cols.t_ns.iter().filter(|&&t| t >= keep_from_ns).count();
    if store.rows.saturating_add(wanted) > caps.rows {
        return Err(RowsError::TooMany {
            what: "stored tracker rows",
            wanted: store.rows.saturating_add(wanted),
            max: caps.rows,
        });
    }
    let mut st = StoreStats {
        rows_in: n as u64,
        ..StoreStats::default()
    };
    for i in 0..n {
        let (
            Some(&t_ns),
            Some(&addr),
            Some(&ident),
            Some(&ident_ns),
            Some(&ident_conflict),
            Some(&lat_e7),
            Some(&lon_e7),
            Some(&ve),
            Some(&vn),
            Some(&r95),
            Some(&alt),
            Some(&flags),
        ) = (
            cols.t_ns.get(i),
            cols.addr.get(i),
            cols.ident.get(i),
            cols.ident_ns.get(i),
            cols.ident_conflict.get(i),
            cols.lat_e7.get(i),
            cols.lon_e7.get(i),
            cols.vel_east_mps.get(i),
            cols.vel_north_mps.get(i),
            cols.r95_m.get(i),
            cols.alt_ft.get(i),
            cols.flags.get(i),
        )
        else {
            st.missing_column += 1;
            continue;
        };
        if t_ns < keep_from_ns {
            st.before_window += 1;
            continue;
        }
        let unavailable = ident == NO_IDENT && ident_ns == NO_IDENT_NS && ident_conflict == 0;
        let conflicted = ident == NO_IDENT && ident_ns != NO_IDENT_NS && ident_conflict == 1;
        let observed = ident <= 0o7777 && ident_ns != NO_IDENT_NS && ident_conflict == 0;
        if !(unavailable || conflicted || observed) {
            st.invalid_identity = st.invalid_identity.saturating_add(1);
            continue;
        }
        st.velocity_not_finite += u64::from(!ve.is_finite() || !vn.is_finite());
        let (gs_dkt, track_ddeg) = velocity_of(ve, vn);
        if gs_dkt >= NO_U16 - 1 {
            st.speed_clamped += 1;
        }
        let alt_ft = if flags & RF_HAS_ALT != 0 {
            if alt.is_finite() {
                let (a, clamped) = alt_to_ft(alt);
                st.alt_clamped += u64::from(clamped);
                a
            } else {
                st.alt_not_finite += 1;
                NO_ALT
            }
        } else {
            NO_ALT
        };
        let r95_m = if r95.is_finite() && r95 <= f64::from(f32::MAX) {
            r95 as f32
        } else {
            st.r95_bad += 1;
            f32::NAN
        };
        let row = Row {
            t_ns,
            ident,
            ident_ns,
            ident_conflict,
            lat_e7,
            lon_e7,
            alt_ft,
            r95_m,
            gs_dkt,
            track_ddeg,
            flags,
        };
        let known = store.by_addr.contains_key(&addr);
        if !known && store.by_addr.len() >= caps.addresses {
            st.address_table_full += 1;
            continue;
        }
        let v = store.by_addr.entry(addr).or_default();
        if v.t_ns.len() >= caps.rows_per_address {
            st.address_rows_full += 1;
            continue;
        }
        v.t_ns.push(row.t_ns);
        v.ident.push(row.ident);
        v.ident_ns.push(row.ident_ns);
        v.ident_conflict.push(row.ident_conflict);
        v.lat_e7.push(row.lat_e7);
        v.lon_e7.push(row.lon_e7);
        v.alt_ft.push(row.alt_ft);
        v.r95_m.push(row.r95_m);
        v.gs_dkt.push(row.gs_dkt);
        v.track_ddeg.push(row.track_ddeg);
        v.flags.push(row.flags);
        if row.flags & RF_REACQUIRED != 0 {
            v.reacq_t.push(row.t_ns);
        }
        store.rows += 1;
        st.kept += 1;
    }
    Ok(st)
}

/// `col` reordered by `order` (the old index of each new position); false, with `col` unchanged,
/// when an index is out of range (cannot happen: the order is built from the same length).
fn gather<T: Copy>(col: &mut Vec<T>, order: &[(i64, u32)]) -> bool {
    let new: Vec<T> = order
        .iter()
        .filter_map(|k| col.get(k.1 as usize).copied())
        .collect();
    if new.len() != col.len() {
        return false;
    }
    *col = new;
    true
}

/// Puts every address's rows in time order (stable, so equal times keep the tracker's order).
/// Returns the number of addresses whose rows could not be ordered (cannot happen; it is a count,
/// not a panic, and the caller fails the run on it).
pub fn store_finish(store: &mut RowStore) -> u64 {
    let mut failed = 0u64;
    for v in store.by_addr.values_mut() {
        if v.t_ns.is_sorted() {
            continue;
        }
        let mut order: Vec<(i64, u32)> = v.t_ns.iter().copied().zip(0u32..).collect();
        order.sort_by_key(|k| k.0);
        let ok = gather(&mut v.t_ns, &order)
            && gather(&mut v.ident, &order)
            && gather(&mut v.ident_ns, &order)
            && gather(&mut v.ident_conflict, &order)
            && gather(&mut v.lat_e7, &order)
            && gather(&mut v.lon_e7, &order)
            && gather(&mut v.alt_ft, &order)
            && gather(&mut v.r95_m, &order)
            && gather(&mut v.gs_dkt, &order)
            && gather(&mut v.track_ddeg, &order)
            && gather(&mut v.flags, &order);
        v.reacq_t.sort_unstable();
        failed += u64::from(!ok);
    }
    failed
}

/// `cs` with its trailing spaces and NULs all NUL, so that padding does not make two spellings
/// of one callsign differ.
fn normalise_callsign(cs: [u8; 8]) -> [u8; 8] {
    let mut out = cs;
    let mut tail = true;
    for b in out.iter_mut().rev() {
        if tail && (*b == b' ' || *b == 0) {
            *b = 0;
        } else {
            tail = false;
        }
    }
    out
}

/// The callsigns heard: the report columns of one length. An empty callsign is not a change. Two
/// different callsigns at one (address, time) are a conflict at that instant (E6), whatever the
/// order they came in; the next report with a callsign ends it.
pub fn callsigns_build(
    time_ns: &[i64],
    icao: &[u32],
    callsign: &[[u8; 8]],
) -> Result<(Callsigns, CallsignStats), RowsError> {
    callsigns_build_with(time_ns, icao, callsign, MAX_CALLSIGN_REPORTS)
}

/// [`callsigns_build`] taking at most `max_reports` reports.
pub fn callsigns_build_with(
    time_ns: &[i64],
    icao: &[u32],
    callsign: &[[u8; 8]],
    max_reports: usize,
) -> Result<(Callsigns, CallsignStats), RowsError> {
    check_len("icao", icao.len(), time_ns.len())?;
    check_len("callsign", callsign.len(), time_ns.len())?;
    if time_ns.len() > max_reports {
        return Err(RowsError::TooMany {
            what: "reports for callsigns",
            wanted: time_ns.len(),
            max: max_reports,
        });
    }
    let mut st = CallsignStats {
        reports_in: time_ns.len() as u64,
        ..CallsignStats::default()
    };
    let mut all: Vec<(i64, u32, [u8; 8])> = Vec::new();
    for ((&t, &a), &cs) in time_ns.iter().zip(icao).zip(callsign) {
        // A callsign padded with spaces and one padded with NULs are the same callsign, and a
        // callsign of spaces only is none.
        let cs = normalise_callsign(cs);
        if cs.first().is_some_and(|&b| b != 0) {
            all.push((t, a, cs));
        } else {
            st.empty += 1;
        }
    }
    all.sort_unstable();
    let mut out = Callsigns::default();
    let mut at = 0usize;
    while let Some(&(t, a, first)) = all.get(at) {
        // The group of this (time, address), sorted by callsign: two ends that differ mean a
        // conflict, whatever else is between them.
        let mut end = at;
        while all.get(end).is_some_and(|x| x.0 == t && x.1 == a) {
            end += 1;
        }
        let last = end
            .checked_sub(1)
            .and_then(|e| all.get(e))
            .map_or(first, |x| x.2);
        let conflict = last != first;
        st.conflicts += u64::from(conflict);
        let cs = if conflict { [0; 8] } else { first };
        let v = out.by_addr.entry(a).or_default();
        let same = v.callsign.last() == Some(&cs) && v.conflict.last() == Some(&conflict);
        if !same {
            v.t_ns.push(t);
            v.callsign.push(cs);
            v.conflict.push(conflict);
            st.changes += 1;
        }
        at = end;
    }
    Ok((out, st))
}

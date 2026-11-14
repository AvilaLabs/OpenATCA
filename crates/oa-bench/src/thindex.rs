//! The ERAM track samples ordered by flight and position time. Core (pure).

use crate::legacy::ThTable;

/// Sample indexes sorted by `(flight, position time, index)`; flight `f` owns
/// `order[start[f]..start[f + 1]]`.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct ThIndex {
    pub order: Vec<u32>,
    pub start: Vec<u32>,
}

/// Builds the index for `flights` flights.
pub fn build_th_index(th: &ThTable, flights: usize) -> ThIndex {
    let n = th.flight.len();
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_by_key(|&i| {
        (
            th.flight.get(i as usize).copied().unwrap_or(0),
            th.pos_ms.get(i as usize).copied().unwrap_or(0),
            i,
        )
    });
    let mut start = vec![0u32; flights + 1];
    for &f in &th.flight {
        if let Some(s) = start.get_mut(f as usize + 1) {
            *s += 1;
        }
    }
    let mut acc = 0u32;
    for s in start.iter_mut() {
        acc += *s;
        *s = acc;
    }
    ThIndex { order, start }
}

/// The sorted sample indexes of flight `f`.
pub fn samples_of(ix: &ThIndex, f: u32) -> &[u32] {
    let a = ix.start.get(f as usize).copied().unwrap_or(0) as usize;
    let b = ix.start.get(f as usize + 1).copied().unwrap_or(0) as usize;
    ix.order.get(a..b).unwrap_or(&[])
}

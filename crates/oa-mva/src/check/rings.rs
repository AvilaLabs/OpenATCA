//! Ring integrity. Counts what is wrong with the polygons as given; fixes nothing.

use crate::geom::{locate_in_ring, orient, segments_meet, Meet, Where};
use crate::model::{ring_is_hole, ring_range, ring_vertices, sector_count, Model, SectorIx};

/// Counts of defects over a set of sectors. Plain data (A1).
// A7.2: cold struct, size guard waived (one per report)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RingReport {
    pub sectors: usize,
    pub rings: usize,
    pub vertices: usize,
    /// Rings whose first and last vertices differ (the loader refuses these, so it is 0 for any
    /// loaded model; recomputed here as an independent check).
    pub not_closed: usize,
    /// Pairs of edges of one sector that cross at a point interior to both.
    pub proper_crossings: usize,
    /// Pairs of non-adjacent edges of one sector that touch or overlap.
    pub touches: usize,
    /// Adjacent edges that fold back on themselves (a spike of zero width).
    pub spikes: usize,
    /// Consecutive equal vertices.
    pub repeated_vertices: usize,
    /// Rings with zero area.
    pub zero_area: usize,
    /// Exterior rings that run clockwise (GML wants counter-clockwise).
    pub clockwise_exteriors: usize,
    /// Holes whose first vertex is strictly outside the exterior ring.
    pub holes_outside: usize,
}

/// Twice the signed area of a closed ring in 1e-14 square degrees.
pub fn ring_area2(lats: &[i32], lons: &[i32]) -> i128 {
    let n = lats.len().min(lons.len());
    let mut s: i128 = 0;
    let mut i = 1;
    while i < n {
        if let (Some(&y0), Some(&x0), Some(&y1), Some(&x1)) =
            (lats.get(i - 1), lons.get(i - 1), lats.get(i), lons.get(i))
        {
            s += i128::from(x0) * i128::from(y1) - i128::from(x1) * i128::from(y0);
        }
        i += 1;
    }
    s
}

fn edge(lats: &[i32], lons: &[i32], i: usize) -> Option<((i32, i32), (i32, i32))> {
    Some((
        (*lons.get(i)?, *lats.get(i)?),
        (*lons.get(i + 1)?, *lats.get(i + 1)?),
    ))
}

/// Edge pairs within one ring: crossings, touches and spikes.
fn within_ring(lats: &[i32], lons: &[i32], rep: &mut RingReport) {
    let n_edges = lats.len().min(lons.len()).saturating_sub(1);
    for i in 0..n_edges {
        let Some((a, b)) = edge(lats, lons, i) else {
            continue;
        };
        if a == b {
            rep.repeated_vertices += 1;
        }
        for j in i + 1..n_edges {
            let Some((c, d)) = edge(lats, lons, j) else {
                continue;
            };
            // Edges i and j are adjacent when j = i + 1, or when they are the first and last
            // edge of the ring (the closing vertex joins them).
            let adjacent = j == i + 1 || (i == 0 && j + 1 == n_edges);
            if adjacent {
                // They share one vertex. A fold-back is collinear with the two edges pointing
                // in opposite directions along the path.
                if a != b && c != d && orient(a, b, c) == 0 && orient(a, b, d) == 0 {
                    let ux = i128::from(b.0) - i128::from(a.0);
                    let uy = i128::from(b.1) - i128::from(a.1);
                    let vx = i128::from(d.0) - i128::from(c.0);
                    let vy = i128::from(d.1) - i128::from(c.1);
                    if ux * vx + uy * vy < 0 {
                        rep.spikes += 1;
                    }
                }
                continue;
            }
            match segments_meet(a, b, c, d) {
                Meet::None => {}
                Meet::Proper => rep.proper_crossings += 1,
                Meet::Touch => rep.touches += 1,
            }
        }
    }
}

/// Edge pairs between two different rings of one sector.
fn between_rings((la, lo): (&[i32], &[i32]), (lb, lob): (&[i32], &[i32]), rep: &mut RingReport) {
    let na = la.len().min(lo.len()).saturating_sub(1);
    let nb = lb.len().min(lob.len()).saturating_sub(1);
    for i in 0..na {
        let Some((a, b)) = edge(la, lo, i) else {
            continue;
        };
        for j in 0..nb {
            let Some((c, d)) = edge(lb, lob, j) else {
                continue;
            };
            match segments_meet(a, b, c, d) {
                Meet::None => {}
                Meet::Proper => rep.proper_crossings += 1,
                Meet::Touch => rep.touches += 1,
            }
        }
    }
}

/// Integrity counts of sector `s`.
pub fn sector_rings(m: &Model, s: SectorIx) -> RingReport {
    let mut rep = RingReport {
        sectors: 1,
        ..RingReport::default()
    };
    let range = ring_range(m, s);
    let ext = ring_vertices(m, range.start);
    for r in range.clone() {
        let (lats, lons) = ring_vertices(m, r);
        rep.rings += 1;
        rep.vertices += lats.len();
        if lats.first() != lats.last() || lons.first() != lons.last() {
            rep.not_closed += 1;
        }
        let area2 = ring_area2(lats, lons);
        if area2 == 0 {
            rep.zero_area += 1;
        }
        if !ring_is_hole(m, r) && area2 < 0 {
            rep.clockwise_exteriors += 1;
        }
        within_ring(lats, lons, &mut rep);
        if ring_is_hole(m, r) {
            if let (Some(&la), Some(&lo)) = (lats.first(), lons.first()) {
                if locate_in_ring(ext.0, ext.1, la, lo) == Where::Outside {
                    rep.holes_outside += 1;
                }
            }
        }
        for r2 in range.start..r {
            between_rings((lats, lons), ring_vertices(m, r2), &mut rep);
        }
    }
    rep
}

/// Sums [`sector_rings`] over every sector of the model.
pub fn model_rings(m: &Model) -> RingReport {
    let mut t = RingReport::default();
    for i in 0..sector_count(m) {
        let r = sector_rings(m, SectorIx(i as u32));
        t.sectors += r.sectors;
        t.rings += r.rings;
        t.vertices += r.vertices;
        t.not_closed += r.not_closed;
        t.proper_crossings += r.proper_crossings;
        t.touches += r.touches;
        t.spikes += r.spikes;
        t.repeated_vertices += r.repeated_vertices;
        t.zero_area += r.zero_area;
        t.clockwise_exteriors += r.clockwise_exteriors;
        t.holes_outside += r.holes_outside;
    }
    t
}

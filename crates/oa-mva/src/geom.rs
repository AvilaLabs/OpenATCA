//! Exact planar predicates on 1e-7 degree integer coordinates. Core (pure).
//!
//! x is longitude and y is latitude. Cross products reach about 1.3e19, past `i64`, so every
//! product is taken in `i128`: no rounding anywhere, which is what makes "on the edge" exact.

/// Where a point lies relative to a closed ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    Outside,
    /// On an edge or at a vertex.
    Boundary,
    Inside,
}

/// Twice the signed area of the triangle (a, b, c): positive when c is left of a to b.
pub(crate) fn orient(a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> i128 {
    let (ax, ay) = (i128::from(a.0), i128::from(a.1));
    let (bx, by) = (i128::from(b.0), i128::from(b.1));
    let (cx, cy) = (i128::from(c.0), i128::from(c.1));
    (bx - ax) * (cy - ay) - (by - ay) * (cx - ax)
}

/// Whether `p` lies on the closed segment a-b.
pub(crate) fn on_segment(p: (i32, i32), a: (i32, i32), b: (i32, i32)) -> bool {
    orient(a, b, p) == 0
        && p.0 >= a.0.min(b.0)
        && p.0 <= a.0.max(b.0)
        && p.1 >= a.1.min(b.1)
        && p.1 <= a.1.max(b.1)
}

/// Locates the point (`lat`, `lon`) relative to the closed ring whose vertices are `lats` and
/// `lons` (last vertex equal to the first; the arrays must be the same length). Even-odd rule
/// with a ray toward +longitude; an edge point is [`Where::Boundary`], exactly. A ring of fewer
/// than 2 vertices contains nothing.
pub fn locate_in_ring(lats: &[i32], lons: &[i32], lat: i32, lon: i32) -> Where {
    let n = lats.len().min(lons.len());
    let p = (lon, lat);
    let mut inside = false;
    let mut i = 1;
    while i < n {
        let (Some(&ay), Some(&ax), Some(&by), Some(&bx)) =
            (lats.get(i - 1), lons.get(i - 1), lats.get(i), lons.get(i))
        else {
            break;
        };
        let (a, b) = ((ax, ay), (bx, by));
        if on_segment(p, a, b) {
            return Where::Boundary;
        }
        if (ay > lat) != (by > lat) {
            let t = (i128::from(bx) - i128::from(ax)) * (i128::from(lat) - i128::from(ay))
                - (i128::from(lon) - i128::from(ax)) * (i128::from(by) - i128::from(ay));
            // The crossing is to the right of the point when t has the sign of (by - ay).
            if (by > ay && t > 0) || (by < ay && t < 0) {
                inside = !inside;
            }
        }
        i += 1;
    }
    if inside {
        Where::Inside
    } else {
        Where::Outside
    }
}

/// How two segments a-b and c-d meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Meet {
    None,
    /// Cross at a point interior to both.
    Proper,
    /// Share a point that is an endpoint of at least one, or overlap collinearly.
    Touch,
}

/// Classifies the intersection of segments a-b and c-d exactly.
pub(crate) fn segments_meet(a: (i32, i32), b: (i32, i32), c: (i32, i32), d: (i32, i32)) -> Meet {
    let o1 = orient(a, b, c).signum();
    let o2 = orient(a, b, d).signum();
    let o3 = orient(c, d, a).signum();
    let o4 = orient(c, d, b).signum();
    if o1 * o2 < 0 && o3 * o4 < 0 {
        return Meet::Proper;
    }
    if (o1 == 0 && on_segment(c, a, b))
        || (o2 == 0 && on_segment(d, a, b))
        || (o3 == 0 && on_segment(a, c, d))
        || (o4 == 0 && on_segment(b, c, d))
    {
        return Meet::Touch;
    }
    Meet::None
}

//! The fixed-size polygon an inhibition covers (**core**, B1, B2). Integer arithmetic only.
//!
//! Vertices are 1e-7 degree latitude and longitude. Edges are straight lines in the latitude and
//! longitude plane, not geodesics: an inhibition area is a few tens of nautical miles across at
//! most (a sector, an approach corridor), where the two differ by far less than a pixel, and the
//! display draws the same straight edges between the same vertices, so what is drawn is what is
//! tested. A polygon whose edge spans 180 degrees of longitude or more is refused, so no polygon
//! crosses the antimeridian or encloses a pole.
//!
//! A point on the boundary is **outside**: an inhibition hides alerts, so a doubtful point is
//! never hidden.

/// Most vertices of an inhibition polygon. C2: a sector or an approach corridor is drawn with a
/// handful of vertices; 32 leaves room for a curved boundary approximated by chords. A facility
/// boundary (hundreds of vertices) is not an inhibition area and is refused, not truncated.
pub const MAX_VERTICES: usize = 32;
/// Most pairs of edges the validation compares: the bound of its loops (C7), `n (n - 1) / 2` for
/// the largest polygon.
pub const MAX_EDGE_PAIRS: usize = MAX_VERTICES * (MAX_VERTICES - 1) / 2;

const LAT_E7_MAX: i64 = 900_000_000;
const LON_E7_MAX: i64 = 1_800_000_000;

/// A validated polygon: `n` vertices in order, the ring closed implicitly. Fields past `n` are
/// zero. Only [`polygon_new`] makes one that is valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Polygon {
    pub lat_e7: [i32; MAX_VERTICES],
    pub lon_e7: [i32; MAX_VERTICES],
    pub n: u8,
}
// Budget: 2 columns of 32 i32 and the count (padded to the 4-byte alignment); one per inhibition.
const _: () = assert!(core::mem::size_of::<Polygon>() == 260);

/// The empty polygon, the filler of unused slots. It is not valid and contains no point.
pub const POLYGON_EMPTY: Polygon = Polygon {
    lat_e7: [0; MAX_VERTICES],
    lon_e7: [0; MAX_VERTICES],
    n: 0,
};

/// A vertex as given: latitude and longitude, 1e-7 degree.
pub type Vertex = (i32, i32);

fn at(v: &[Vertex], i: usize) -> Option<(i64, i64)> {
    // The caller indexes modulo the length, so `get` succeeds; `None` would be refused.
    v.get(i).map(|&(la, lo)| (i64::from(la), i64::from(lo)))
}

/// Sign of the cross product of `ab` and `ac`, in 128 bits so no input can overflow.
fn orient(a: (i64, i64), b: (i64, i64), c: (i64, i64)) -> i32 {
    let l = i128::from(b.0 - a.0) * i128::from(c.1 - a.1);
    let r = i128::from(b.1 - a.1) * i128::from(c.0 - a.0);
    match l.cmp(&r) {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}

/// True if `c`, known to be collinear with `ab`, lies on the closed segment.
fn within(a: (i64, i64), b: (i64, i64), c: (i64, i64)) -> bool {
    c.0 >= a.0.min(b.0) && c.0 <= a.0.max(b.0) && c.1 >= a.1.min(b.1) && c.1 <= a.1.max(b.1)
}

/// True if the closed segments `ab` and `cd` share a point.
fn touch(a: (i64, i64), b: (i64, i64), c: (i64, i64), d: (i64, i64)) -> bool {
    let (o1, o2, o3, o4) = (
        orient(a, b, c),
        orient(a, b, d),
        orient(c, d, a),
        orient(c, d, b),
    );
    if o1 != o2 && o3 != o4 {
        return true;
    }
    (o1 == 0 && within(a, b, c))
        || (o2 == 0 && within(a, b, d))
        || (o3 == 0 && within(c, d, a))
        || (o4 == 0 && within(c, d, b))
}

/// Builds a polygon, or says why not. Refused: fewer than 3 or more than [`MAX_VERTICES`]
/// vertices, a vertex off the globe or at a pole, a repeated consecutive vertex, an edge of 180
/// degrees of longitude or more, an edge that doubles back along its neighbour, two edges that
/// cross or touch, and an area of zero. The ring is not closed by repeating the first vertex.
/// Work: at most [`MAX_EDGE_PAIRS`] pair comparisons (C7), counted from the declared length
/// before any is made.
pub fn polygon_new(v: &[Vertex]) -> Result<Polygon, &'static str> {
    let n = v.len();
    if n < 3 {
        return Err("fewer than 3 vertices");
    }
    if n > MAX_VERTICES {
        return Err("more than 32 vertices");
    }
    let mut p = POLYGON_EMPTY;
    for (k, &(la, lo)) in v.iter().enumerate() {
        if i64::from(la).abs() >= LAT_E7_MAX {
            return Err("vertex at a pole or off the globe");
        }
        if i64::from(lo).abs() > LON_E7_MAX {
            return Err("vertex off the globe");
        }
        if let (Some(a), Some(b)) = (p.lat_e7.get_mut(k), p.lon_e7.get_mut(k)) {
            *a = la;
            *b = lo;
        }
    }
    // `n` is at most 32 here.
    p.n = u8::try_from(n).unwrap_or(0);
    let mut twice_area: i128 = 0;
    for i in 0..n {
        let (Some(a), Some(b)) = (at(v, i), at(v, (i + 1) % n)) else {
            return Err("internal: vertex missing");
        };
        if a == b {
            return Err("repeated vertex");
        }
        if (b.1 - a.1).abs() >= LON_E7_MAX {
            return Err("edge spans 180 degrees of longitude");
        }
        twice_area += i128::from(a.1) * i128::from(b.0) - i128::from(b.1) * i128::from(a.0);
    }
    if twice_area == 0 {
        return Err("zero area");
    }
    for i in 0..n {
        for j in (i + 1)..n {
            let (Some(a), Some(b), Some(c), Some(d)) =
                (at(v, i), at(v, (i + 1) % n), at(v, j), at(v, (j + 1) % n))
            else {
                return Err("internal: vertex missing");
            };
            let adjacent = j == i + 1 || (i == 0 && j + 1 == n);
            if adjacent {
                // The edges share one vertex: `s` is that vertex, `x` and `y` the far ends.
                let (s, x, y) = if j == i + 1 { (b, a, d) } else { (a, b, c) };
                let dot = i128::from(x.0 - s.0) * i128::from(y.0 - s.0)
                    + i128::from(x.1 - s.1) * i128::from(y.1 - s.1);
                if orient(x, s, y) == 0 && dot > 0 {
                    // Collinear with the far ends on the same side of the shared vertex: the
                    // second edge folds back over the first.
                    return Err("edge doubles back");
                }
            } else if touch(a, b, c, d) {
                return Err("edges cross or touch");
            }
        }
    }
    Ok(p)
}

/// True if the point is strictly inside `p` (even-odd rule); a point on an edge or a vertex is
/// outside. Allocates nothing; work is at most [`MAX_VERTICES`] edges.
pub fn polygon_contains(p: &Polygon, lat_e7: i32, lon_e7: i32) -> bool {
    let n = usize::from(p.n);
    let pt = (i64::from(lat_e7), i64::from(lon_e7));
    let mut inside = false;
    for i in 0..n {
        let j = (i + 1) % n;
        let (Some(&la), Some(&lo), Some(&lb), Some(&ob)) = (
            p.lat_e7.get(i),
            p.lon_e7.get(i),
            p.lat_e7.get(j),
            p.lon_e7.get(j),
        ) else {
            return false;
        };
        let (a, b) = (
            (i64::from(la), i64::from(lo)),
            (i64::from(lb), i64::from(ob)),
        );
        if orient(a, b, pt) == 0 && within(a, b, pt) {
            return false;
        }
        // x is longitude, y latitude; the ray runs towards larger longitude.
        if (a.0 > pt.0) != (b.0 > pt.0) {
            let t = i128::from(b.1 - a.1) * i128::from(pt.0 - a.0)
                - i128::from(pt.1 - a.1) * i128::from(b.0 - a.0);
            if (b.0 > a.0 && t > 0) || (b.0 < a.0 && t < 0) {
                inside = !inside;
            }
        }
    }
    inside
}

/// The larger of the latitude extent and the longitude extent of the polygon's bounding box,
/// 1e-7 degree.
pub fn polygon_extent(p: &Polygon) -> i64 {
    let n = usize::from(p.n);
    let span = |v: &[i32]| {
        let s = v.get(..n).unwrap_or(&[]);
        let lo = s.iter().copied().min().unwrap_or(0);
        let hi = s.iter().copied().max().unwrap_or(0);
        i64::from(hi) - i64::from(lo)
    };
    span(&p.lat_e7).max(span(&p.lon_e7))
}

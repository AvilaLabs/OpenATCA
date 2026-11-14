//! Tag trees (T.800 B.10.2). Core (pure). Iterative: a node is reached by walking parent
//! indexes up from its leaf into a fixed stack, so there is no recursion.

use crate::bits::HeaderBits;
use crate::error::J2kError;

/// No parent: the root.
const NONE: u32 = u32::MAX;
/// A value not yet known.
const UNKNOWN: u32 = u32::MAX;
/// Depth bound: a tree over 2^16 x 2^16 leaves has 17 levels.
const MAX_DEPTH: usize = 20;

/// One tag tree: nodes are stored level by level, the leaves (row-major) first.
// A7.2: cold struct, size guard waived (a few per resolution)
#[derive(Debug, Clone)]
pub struct TagTree {
    /// Current lower bound of each node.
    low: Vec<u32>,
    /// The node's value once known, else `UNKNOWN`.
    value: Vec<u32>,
    parent: Vec<u32>,
}

/// A tree over `w * h` leaves; `w` and `h` are at least 1.
pub fn new_tree(w: u32, h: u32) -> TagTree {
    let mut parent = Vec::new();
    let (mut lw, mut lh) = (w.max(1), h.max(1));
    let mut base = 0u32;
    loop {
        let n = lw * lh;
        if lw == 1 && lh == 1 {
            parent.extend(std::iter::repeat_n(NONE, 1));
            break;
        }
        let (pw, ph) = (lw.div_ceil(2), lh.div_ceil(2));
        let next = base + n;
        for y in 0..lh {
            for x in 0..lw {
                parent.push(next + (y / 2) * pw + x / 2);
            }
        }
        base = next;
        lw = pw;
        lh = ph;
    }
    let n = parent.len();
    TagTree {
        low: vec![0; n],
        value: vec![UNKNOWN; n],
        parent,
    }
}

/// Decodes enough of the tree to say whether leaf `leaf` has a value below `threshold`
/// (B.10.2). Returns true when it does; the value is then known and [`leaf_value`] gives it.
pub fn decode(
    t: &mut TagTree,
    leaf: u32,
    threshold: u32,
    bits: &mut HeaderBits,
) -> Result<bool, J2kError> {
    let bad = J2kError::BadPacket("tag tree leaf out of range");
    let mut path = [0u32; MAX_DEPTH];
    let mut depth = 0usize;
    let mut node = leaf;
    loop {
        if node as usize >= t.parent.len() || depth >= MAX_DEPTH {
            return Err(bad);
        }
        if let Some(p) = path.get_mut(depth) {
            *p = node;
        }
        depth += 1;
        let up = t
            .parent
            .get(node as usize)
            .copied()
            .ok_or_else(|| bad.clone())?;
        if up == NONE {
            break;
        }
        node = up;
    }
    let mut low = 0u32;
    // Root first.
    for k in (0..depth).rev() {
        let n = path.get(k).copied().unwrap_or(0) as usize;
        let (Some(nl), Some(nv)) = (t.low.get_mut(n), t.value.get_mut(n)) else {
            return Err(bad);
        };
        if low > *nl {
            *nl = low;
        } else {
            low = *nl;
        }
        while low < threshold && low < *nv {
            if bits.bit()? == 1 {
                *nv = low;
            } else {
                low += 1;
            }
        }
        *nl = low;
        if k == 0 {
            return Ok(*nv < threshold);
        }
    }
    Ok(false)
}

/// The value of a leaf, once [`decode`] has reported it below some threshold.
pub fn leaf_value(t: &TagTree, leaf: u32) -> Option<u32> {
    t.value
        .get(leaf as usize)
        .copied()
        .filter(|&v| v != UNKNOWN)
}

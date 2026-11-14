//! Grid MORA (AS) records: minimum off-route altitudes per one-degree cell.

use crate::error::ErrorKind;
use crate::field::Rec;
use crate::types::Meta;

/// Number of one-degree cells in one record (30 degrees of longitude).
pub const CELLS_PER_RECORD: usize = 30;

/// One Grid MORA record: a row of 30 one-degree cells.
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct GridMora {
    pub meta: Meta,
    /// Cols 14-16 (`N41`): the southern edge of the row, signed degrees. The row covers
    /// latitudes `start_lat ..= start_lat + 1`. Verified for the northern hemisphere against
    /// terrain: Mount Whitney (36.58 N 118.29 W) falls in the row's highest cell, 16,800 ft;
    /// the Catskills (41.99 N 74.4 W) in a 6,500 ft cell; Mount Marcy (44.11 N 73.92 W) in a
    /// 7,600 ft cell. Southern rows are all `UNK` in this data, so the southern convention
    /// (edge nearest the south pole = the stated latitude) is not verified.
    pub start_lat: i16,
    /// Cols 17-20 (`W090`): the western edge of the first cell, signed degrees. Verified with
    /// the same terrain checks (cells run west to east).
    pub start_lon: i16,
    /// Cols 31-120: 30 cells of three characters each, west to east from `start_lon`. Hundreds
    /// of feet (`032` is 3200 ft); `None` for `UNK`.
    pub cells: [Option<u32>; CELLS_PER_RECORD],
}

/// MORA in feet for a position inside this row, `None` outside it or for `UNK`.
pub fn mora_value_at(row: &GridMora, lat: f64, lon: f64) -> Option<u32> {
    if !lat.is_finite() || !lon.is_finite() {
        return None;
    }
    if lat.floor() != f64::from(row.start_lat) {
        return None;
    }
    let idx = lon.floor() - f64::from(row.start_lon);
    if !(0.0..CELLS_PER_RECORD as f64).contains(&idx) {
        return None;
    }
    row.cells
        .get(idx as usize)
        .copied()
        .flatten()
        .map(|h| h.saturating_mul(100))
}

pub(crate) fn parse_mora(r: &Rec, meta: Meta) -> Result<GridMora, ErrorKind> {
    let lat_digits = r.uint("starting latitude", 15, 16)?;
    let lat_sign = match r.ch(14) {
        'N' => 1i16,
        'S' => -1i16,
        _ => return Err(r.bad("starting latitude", 14, 16, "expected N or S")),
    };
    let start_lat = lat_digits
        .filter(|&d| d <= 90)
        .and_then(|d| i16::try_from(d).ok())
        .ok_or_else(|| r.bad("starting latitude", 14, 16, "bad degrees"))?
        * lat_sign;
    let lon_sign = match r.ch(17) {
        'E' => 1i16,
        'W' => -1i16,
        _ => return Err(r.bad("starting longitude", 17, 20, "expected E or W")),
    };
    let start_lon = r
        .uint("starting longitude", 18, 20)?
        .filter(|&d| d <= 180)
        .and_then(|d| i16::try_from(d).ok())
        .ok_or_else(|| r.bad("starting longitude", 17, 20, "bad degrees"))?
        * lon_sign;
    let mut cells = [None; CELLS_PER_RECORD];
    for (i, cell) in cells.iter_mut().enumerate() {
        let a = 31 + 3 * i;
        *cell = if r.raw(a, a + 2) == "UNK" {
            None
        } else {
            Some(r.req_uint("MORA cell", a, a + 2)?)
        };
    }
    Ok(GridMora {
        meta,
        start_lat,
        start_lon,
        cells,
    })
}

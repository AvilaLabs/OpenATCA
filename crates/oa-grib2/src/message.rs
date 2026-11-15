//! Messages and sections 0-8, and the decoded [`Field`]. Core (pure): bytes in, fields out.
//!
//! A message is `GRIB` (section 0), identification (1), optional local use (2), then one or more
//! fields, each `[3] 4 5 6 7`: the grid (3) may be omitted when the previous field's grid holds,
//! and a bitmap indicator of 254 reuses the previous field's bitmap. The message ends with
//! `7777` (section 8).
//!
//! A file is any number of messages back to back (as in a byte-range subset of a NOAA file).
//! [`decode_stream`] decodes each message on its own: a message it must refuse (an unsupported
//! packing, say) is recorded in [`Decoded::skipped`] with its offset and the reason, and the
//! next message is still decoded. Only damage to the framing (no `GRIB`, an impossible length)
//! stops the walk ([`Decoded::stopped`]), because after that the next message cannot be found;
//! what was decoded before it is kept.

use crate::bits::{be, u32_at, u8_at};
use crate::error::Grib2Error;
use crate::grid::{grid_dims, parse_grid, Grid};
use crate::pack::{apply_bitmap, parse_bitmap, parse_repr, unpack, BitmapSpec, Repr};
use crate::product::{
    parameter_of, parse_product, parse_ref_time, Level, Parameter, Product, RefTime,
};
use crate::{MAX_FIELDS_PER_MESSAGE, MAX_GRID_POINTS, MAX_MESSAGE_BYTES, MAX_MESSAGE_POINTS};

/// One decoded field. `values` has `nx * ny` entries in scan order (see [`crate::grid`]); a
/// point with no data is NaN, and `missing_count` says how many there are.
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Field {
    pub discipline: u8,
    pub category: u8,
    pub number: u8,
    pub parameter: Parameter,
    pub level: Level,
    pub ref_time: RefTime,
    /// Code table 4.4 unit and value of the forecast lead.
    pub time_unit: u8,
    pub forecast_time: u32,
    pub product_template: u16,
    pub packing_template: u16,
    pub grid: Grid,
    pub values: Vec<f32>,
    pub missing_count: u32,
}

/// What a field is, without its values: everything section 1-5 say. [`inspect_stream`] returns
/// these for files whose packing is not decodable, so their inventory can still be checked.
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct FieldInfo {
    pub discipline: u8,
    pub category: u8,
    pub number: u8,
    pub parameter: Parameter,
    pub level: Level,
    pub ref_time: RefTime,
    pub time_unit: u8,
    pub forecast_time: u32,
    pub product_template: u16,
    /// Number of the data representation template, whether or not this crate can unpack it.
    pub packing_template: u16,
    pub grid: Grid,
}

/// A message the file walk refused.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Skipped {
    pub offset: usize,
    pub error: Grib2Error,
}

/// Result of decoding a file.
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Decoded {
    pub fields: Vec<Field>,
    pub skipped: Vec<Skipped>,
    /// Messages found, decoded or not.
    pub messages: u32,
    /// Set when the walk could not go on: the bytes at this offset are not a well-framed message
    /// (not `GRIB`, a length that is out of bounds or runs past the end of the input). Every
    /// message before it is in `fields` or `skipped`; everything from it on is unread.
    pub stopped: Option<Skipped>,
}

/// Position and discipline of one message in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct MessageSpan {
    pub offset: usize,
    pub length: usize,
}

/// Reads section 0 at `pos` and returns the message length. Refuses edition other than 2 and
/// lengths outside `[20, MAX_MESSAGE_BYTES]` or past the end of `data`.
pub fn message_length(data: &[u8], pos: usize) -> Result<usize, Grib2Error> {
    let head = data
        .get(pos..)
        .ok_or(Grib2Error::Truncated("indicator section"))?;
    if head.len() < 16 {
        return Err(Grib2Error::Truncated("indicator section"));
    }
    if head.get(0..4) != Some(b"GRIB".as_slice()) {
        return Err(Grib2Error::NotGrib { offset: pos });
    }
    let edition = u8_at(head, 7).ok_or(Grib2Error::Truncated("indicator section"))?;
    if edition != 2 {
        return Err(Grib2Error::UnsupportedEdition(edition));
    }
    let len = be(head, 8, 8).ok_or(Grib2Error::Truncated("indicator section"))?;
    // 16 (section 0) + 21 (section 1) + 4 (section 8) is the smallest conceivable message.
    if !(41..=MAX_MESSAGE_BYTES).contains(&len) {
        return Err(Grib2Error::BadMessageLength(len));
    }
    if len > head.len() as u64 {
        return Err(Grib2Error::Truncated("message"));
    }
    Ok(len as usize)
}

/// Lists the messages of a file without decoding them. Stops with an error at the first framing
/// fault.
pub fn scan_messages(data: &[u8]) -> Result<Vec<MessageSpan>, Grib2Error> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let length = message_length(data, pos)?;
        out.push(MessageSpan {
            offset: pos,
            length,
        });
        pos += length;
    }
    Ok(out)
}

/// Decodes every message of `data`. See the module comment for what is skipped and what stops.
pub fn decode_stream(data: &[u8]) -> Decoded {
    let mut out = Decoded {
        fields: Vec::new(),
        skipped: Vec::new(),
        messages: 0,
        stopped: None,
    };
    let mut pos = 0usize;
    while pos < data.len() {
        let length = match message_length(data, pos) {
            Ok(l) => l,
            Err(error) => {
                out.stopped = Some(Skipped { offset: pos, error });
                return out;
            }
        };
        out.messages += 1;
        let Some(msg) = data.get(pos..pos + length) else {
            out.stopped = Some(Skipped {
                offset: pos,
                error: Grib2Error::Truncated("message"),
            });
            return out;
        };
        match decode_message(msg) {
            Ok(mut f) => out.fields.append(&mut f),
            Err(error) => out.skipped.push(Skipped { offset: pos, error }),
        }
        pos += length;
    }
    out
}

struct State {
    ref_time: Option<RefTime>,
    grid: Option<Grid>,
    product: Option<Product>,
    repr: Option<Repr>,
    bitmap: Option<BitmapSpec>,
    /// The last explicit bitmap, for indicator 254.
    last_bitmap: Option<crate::pack::Bitmap>,
}

/// Decodes one whole message (`msg` is exactly its bytes, `GRIB` through `7777`).
pub fn decode_message(msg: &[u8]) -> Result<Vec<Field>, Grib2Error> {
    let entries = walk(msg, true)?;
    let mut out = Vec::with_capacity(entries.len());
    for (info, values) in entries {
        let values = values.unwrap_or_default();
        let missing_count = values.iter().filter(|v| v.is_nan()).count() as u32;
        out.push(Field {
            discipline: info.discipline,
            category: info.category,
            number: info.number,
            parameter: info.parameter,
            level: info.level,
            ref_time: info.ref_time,
            time_unit: info.time_unit,
            forecast_time: info.forecast_time,
            product_template: info.product_template,
            packing_template: info.packing_template,
            grid: info.grid,
            values,
            missing_count,
        });
    }
    Ok(out)
}

/// Result of [`inspect_stream`].
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Inspected {
    pub fields: Vec<FieldInfo>,
    pub skipped: Vec<Skipped>,
    pub messages: u32,
    pub stopped: Option<Skipped>,
}

/// Reads sections 1-6 of every field of every message and skips the data, so a file whose
/// packing is not supported (PNG) can still be inventoried. The data section's
/// framing is still checked. Framing faults stop the walk as in [`decode_stream`].
pub fn inspect_stream(data: &[u8]) -> Inspected {
    let mut out = Inspected {
        fields: Vec::new(),
        skipped: Vec::new(),
        messages: 0,
        stopped: None,
    };
    let mut pos = 0usize;
    while pos < data.len() {
        let length = match message_length(data, pos) {
            Ok(l) => l,
            Err(error) => {
                out.stopped = Some(Skipped { offset: pos, error });
                return out;
            }
        };
        out.messages += 1;
        let Some(msg) = data.get(pos..pos + length) else {
            out.stopped = Some(Skipped {
                offset: pos,
                error: Grib2Error::Truncated("message"),
            });
            return out;
        };
        match walk(msg, false) {
            Ok(entries) => out.fields.extend(entries.into_iter().map(|(info, _)| info)),
            Err(error) => out.skipped.push(Skipped { offset: pos, error }),
        }
        pos += length;
    }
    out
}

/// A field's description and, when the walk unpacks, its values.
type Entry = (FieldInfo, Option<Vec<f32>>);

fn walk(msg: &[u8], want_data: bool) -> Result<Vec<Entry>, Grib2Error> {
    let len = message_length(msg, 0)?;
    if len != msg.len() {
        return Err(Grib2Error::BadMessageLength(msg.len() as u64));
    }
    if msg.get(len - 4..) != Some(b"7777".as_slice()) {
        return Err(Grib2Error::BadEndMarker);
    }
    let discipline = u8_at(msg, 6).ok_or(Grib2Error::Truncated("indicator section"))?;
    let body_end = len - 4;
    let mut st = State {
        ref_time: None,
        grid: None,
        product: None,
        repr: None,
        bitmap: None,
        last_bitmap: None,
    };
    let mut fields: Vec<Entry> = Vec::new();
    let mut points: u64 = 0;
    let mut pos = 16usize;
    let mut last_section = 0u8;
    while pos < body_end {
        let slen = u32_at(msg, pos).ok_or(Grib2Error::Truncated("section header"))?;
        let num = u8_at(msg, pos + 4).ok_or(Grib2Error::Truncated("section header"))?;
        if slen < 5 || pos as u64 + u64::from(slen) > body_end as u64 {
            return Err(Grib2Error::BadSectionLength {
                section: num,
                length: slen,
            });
        }
        let sect = msg
            .get(pos..pos + slen as usize)
            .ok_or(Grib2Error::Truncated("section"))?;
        let order_ok = match num {
            1 => last_section == 0,
            2 => last_section == 1 || last_section == 7,
            3 => last_section == 1 || last_section == 2 || last_section == 7,
            4 => matches!(last_section, 1 | 2 | 3 | 7),
            5 => last_section == 4,
            6 => last_section == 5,
            7 => last_section == 6,
            _ => false,
        };
        if !order_ok {
            return Err(Grib2Error::SectionOrder { section: num });
        }
        match num {
            1 => st.ref_time = Some(parse_ref_time(sect)?),
            2 => {}
            3 => st.grid = Some(parse_grid(sect)?),
            4 => {
                if st.grid.is_none() {
                    return Err(Grib2Error::MissingSection("grid definition"));
                }
                st.product = Some(parse_product(sect)?);
            }
            5 => st.repr = Some(parse_repr_lenient(sect, want_data)?),
            6 => {
                let grid = st
                    .grid
                    .ok_or(Grib2Error::MissingSection("grid definition"))?;
                let (a, b) = grid_dims(&grid);
                let spec = parse_bitmap(sect, a * b)?;
                if let BitmapSpec::Present(bm) = &spec {
                    st.last_bitmap = Some(bm.clone());
                }
                st.bitmap = Some(spec);
            }
            7 => {
                if fields.len() >= MAX_FIELDS_PER_MESSAGE {
                    return Err(Grib2Error::TooManyFields);
                }
                let payload = sect.get(5..).ok_or(Grib2Error::Truncated("data section"))?;
                if let Some(g) = st.grid {
                    let (a, b) = grid_dims(&g);
                    points += u64::from(a) * u64::from(b);
                    if points > MAX_MESSAGE_POINTS {
                        return Err(Grib2Error::GridPoints(points));
                    }
                }
                fields.push(finish_field(&st, discipline, payload, want_data)?);
            }
            _ => return Err(Grib2Error::SectionOrder { section: num }),
        }
        last_section = num;
        pos += slen as usize;
    }
    if pos != body_end {
        return Err(Grib2Error::BadSectionLength {
            section: last_section,
            length: 0,
        });
    }
    if last_section != 7 {
        return Err(Grib2Error::MissingSection("data section"));
    }
    Ok(fields)
}

/// Section 5 for a metadata walk may name an unsupported template; only its number is kept.
fn parse_repr_lenient(sect: &[u8], want_data: bool) -> Result<Repr, Grib2Error> {
    match parse_repr(sect) {
        Err(Grib2Error::UnsupportedPacking(t)) if !want_data => Ok(Repr {
            template: t,
            ndata: 0,
            r: 0.0,
            e: 0,
            d: 0,
            nbits: 0,
            complex: None,
        }),
        other => other,
    }
}

fn finish_field(
    st: &State,
    discipline: u8,
    payload: &[u8],
    want_data: bool,
) -> Result<Entry, Grib2Error> {
    let grid = st
        .grid
        .ok_or(Grib2Error::MissingSection("grid definition"))?;
    let product = st
        .product
        .ok_or(Grib2Error::MissingSection("product definition"))?;
    let repr = st
        .repr
        .ok_or(Grib2Error::MissingSection("data representation"))?;
    let ref_time = st
        .ref_time
        .ok_or(Grib2Error::MissingSection("identification"))?;
    let bitmap = st
        .bitmap
        .as_ref()
        .ok_or(Grib2Error::MissingSection("bitmap"))?;
    let (nx, ny) = grid_dims(&grid);
    let npoints = u64::from(nx) * u64::from(ny);
    if npoints > MAX_GRID_POINTS {
        return Err(Grib2Error::GridPoints(npoints));
    }
    let info = FieldInfo {
        discipline,
        category: product.category,
        number: product.number,
        parameter: parameter_of(discipline, product.category, product.number),
        level: product.level,
        ref_time,
        time_unit: product.time_unit,
        forecast_time: product.forecast_time,
        product_template: product.template,
        packing_template: repr.template,
        grid,
    };
    if !want_data {
        return Ok((info, None));
    }
    // The value count of section 5 must match the grid (or the bitmap's set bits) before any
    // unpacker sizes a vector from it.
    let bitmap_used = match bitmap {
        BitmapSpec::None => None,
        BitmapSpec::Present(bm) => Some(bm),
        BitmapSpec::Previous => Some(st.last_bitmap.as_ref().ok_or(Grib2Error::BadBitmap(
            "indicator 254 with no earlier bitmap",
        ))?),
    };
    match bitmap_used {
        None if u64::from(repr.ndata) != npoints => {
            return Err(Grib2Error::BadPacking("value count differs from the grid"));
        }
        Some(bm) if repr.ndata != bm.set => {
            return Err(Grib2Error::BadBitmap(
                "set bits differ from the number of data values",
            ));
        }
        _ => {}
    }
    let packed = unpack(&repr, payload)?;
    let values = match bitmap_used {
        None => packed,
        Some(bm) => apply_bitmap(bm, &packed)?,
    };
    Ok((info, Some(values)))
}

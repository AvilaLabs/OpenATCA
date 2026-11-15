//! Classic TIFF 6.0 parsing: header, first image file directory, strips and tiles, the
//! compressions and predictors in the crate scope. Core (pure).

use crate::geotiff::{georeference, GeoTags};
use crate::grid::Grid;
use crate::lzw::lzw_decode;
use crate::{
    TiffError, MAX_ASCII_BYTES, MAX_DIMENSION, MAX_FILE_BYTES, MAX_IFD_ENTRIES, MAX_PIXELS,
    MAX_SEGMENT_BYTES, MAX_TAG_COUNT,
};

const TAG_WIDTH: u16 = 256;
const TAG_HEIGHT: u16 = 257;
const TAG_BITS: u16 = 258;
const TAG_COMPRESSION: u16 = 259;
const TAG_ORIENTATION: u16 = 274;
const TAG_STRIP_OFFSETS: u16 = 273;
const TAG_SAMPLES: u16 = 277;
const TAG_ROWS_PER_STRIP: u16 = 278;
const TAG_STRIP_COUNTS: u16 = 279;
const TAG_PLANAR: u16 = 284;
const TAG_PREDICTOR: u16 = 317;
const TAG_TILE_WIDTH: u16 = 322;
const TAG_TILE_LENGTH: u16 = 323;
const TAG_TILE_OFFSETS: u16 = 324;
const TAG_TILE_COUNTS: u16 = 325;
const TAG_SAMPLE_FORMAT: u16 = 339;
const TAG_PIXEL_SCALE: u16 = 33550;
const TAG_TIEPOINT: u16 = 33922;
const TAG_TRANSFORMATION: u16 = 34264;
const TAG_GEO_KEYS: u16 = 34735;
const TAG_GEO_DOUBLES: u16 = 34736;
const TAG_GEO_ASCII: u16 = 34737;
const TAG_NODATA: u16 = 42113;

/// Most output bytes DEFLATE can produce per input byte (258-byte matches at 2 bits each); LZW
/// is far below it. A segment that declares more than this is not honest data.
const MAX_COMPRESSION_RATIO: u64 = 1032;

const COMPRESSION_NONE: u32 = 1;
const COMPRESSION_LZW: u32 = 5;
const COMPRESSION_DEFLATE: u32 = 8;
const COMPRESSION_DEFLATE_ADOBE: u32 = 32946;

/// One 12-byte IFD entry, still unread.
#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    typ: u16,
    count: u64,
    /// Where the value bytes are: inline in the entry, or at the stored offset.
    at: usize,
}

/// Byte reader with the file's byte order. Plain data and free-standing helpers (A1).
#[derive(Clone, Copy)]
struct Src<'a> {
    data: &'a [u8],
    big: bool,
}

fn get<const N: usize>(s: Src<'_>, off: usize, what: &'static str) -> Result<[u8; N], TiffError> {
    let end = off.checked_add(N).ok_or(TiffError::BadOffset(what))?;
    let slice = s.data.get(off..end).ok_or(TiffError::Truncated(what))?;
    let mut a = [0u8; N];
    a.copy_from_slice(slice);
    Ok(a)
}

fn u16_at(s: Src<'_>, off: usize, what: &'static str) -> Result<u16, TiffError> {
    let b = get::<2>(s, off, what)?;
    Ok(if s.big {
        u16::from_be_bytes(b)
    } else {
        u16::from_le_bytes(b)
    })
}

fn u32_at(s: Src<'_>, off: usize, what: &'static str) -> Result<u32, TiffError> {
    let b = get::<4>(s, off, what)?;
    Ok(if s.big {
        u32::from_be_bytes(b)
    } else {
        u32::from_le_bytes(b)
    })
}

fn f64_at(s: Src<'_>, off: usize, what: &'static str) -> Result<f64, TiffError> {
    let b = get::<8>(s, off, what)?;
    Ok(f64::from_bits(if s.big {
        u64::from_be_bytes(b)
    } else {
        u64::from_le_bytes(b)
    }))
}

/// Size in bytes of one value of TIFF data type `typ`, for the types classic TIFF defines.
fn type_size(typ: u16) -> Option<u64> {
    match typ {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 | 11 => Some(4),
        5 | 10 | 12 => Some(8),
        _ => None,
    }
}

fn find(entries: &[Entry], tag: u16) -> Option<Entry> {
    entries.iter().copied().find(|e| e.tag == tag)
}

/// Unsigned values of a BYTE, SHORT or LONG tag, at most [`MAX_TAG_COUNT`].
fn uints(s: Src<'_>, e: Entry) -> Result<Vec<u64>, TiffError> {
    if e.count > MAX_TAG_COUNT {
        return Err(TiffError::TagTooLarge {
            tag: e.tag,
            count: e.count,
        });
    }
    let n = e.count as usize;
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        let x = match e.typ {
            1 => u64::from(get::<1>(s, e.at + i, "tag values")?[0]),
            3 => u64::from(u16_at(s, e.at + i * 2, "tag values")?),
            4 => u64::from(u32_at(s, e.at + i * 4, "tag values")?),
            t => return Err(TiffError::BadTagType { tag: e.tag, typ: t }),
        };
        v.push(x);
    }
    Ok(v)
}

/// The single unsigned value of a tag, which must have count 1.
fn uint1(s: Src<'_>, e: Entry) -> Result<u32, TiffError> {
    if e.count != 1 {
        return Err(TiffError::BadValue("tag must hold exactly one value"));
    }
    let v = uints(s, e)?;
    let x = v.first().copied().unwrap_or(0);
    u32::try_from(x).map_err(|_| TiffError::BadValue("tag value over 32 bits"))
}

/// DOUBLE values of a tag, at most [`MAX_TAG_COUNT`].
pub(crate) fn doubles(
    s: &[u8],
    big: bool,
    typ: u16,
    count: u64,
    at: usize,
) -> Result<Vec<f64>, TiffError> {
    let src = Src { data: s, big };
    if typ != 12 {
        return Err(TiffError::BadTagType { tag: 0, typ });
    }
    if count > MAX_TAG_COUNT {
        return Err(TiffError::TagTooLarge { tag: 0, count });
    }
    (0..count as usize)
        .map(|i| f64_at(src, at + i * 8, "double values"))
        .collect()
}

fn ascii(s: Src<'_>, e: Entry) -> Result<String, TiffError> {
    if e.typ != 2 {
        return Err(TiffError::BadTagType {
            tag: e.tag,
            typ: e.typ,
        });
    }
    if e.count > MAX_ASCII_BYTES {
        return Err(TiffError::TagTooLarge {
            tag: e.tag,
            count: e.count,
        });
    }
    let end =
        e.at.checked_add(e.count as usize)
            .ok_or(TiffError::BadOffset("ascii value"))?;
    let b = s
        .data
        .get(e.at..end)
        .ok_or(TiffError::Truncated("ascii value"))?;
    let b = match b.iter().position(|&c| c == 0) {
        Some(n) => b.get(..n).unwrap_or(b),
        None => b,
    };
    Ok(String::from_utf8_lossy(b).into_owned())
}

/// Sample layout of the image.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    F32,
    I16,
}

fn bytes_per_sample(f: Format) -> usize {
    match f {
        Format::F32 => 4,
        Format::I16 => 2,
    }
}

/// Reads the first image of a classic TIFF / GeoTIFF into an elevation [`Grid`].
pub fn read_geotiff(data: &[u8]) -> Result<Grid, TiffError> {
    if data.len() as u64 > MAX_FILE_BYTES {
        return Err(TiffError::FileTooLarge(data.len() as u64));
    }
    let big = match data.get(..2) {
        Some(b"II") => false,
        Some(b"MM") => true,
        _ => return Err(TiffError::BadHeader),
    };
    let s = Src { data, big };
    match u16_at(s, 2, "header")? {
        42 => {}
        43 => return Err(TiffError::BigTiffUnsupported),
        _ => return Err(TiffError::BadHeader),
    }
    let ifd = u32_at(s, 4, "header")? as usize;
    let n = u64::from(u16_at(s, ifd, "IFD")?);
    if n as usize > MAX_IFD_ENTRIES {
        return Err(TiffError::TooManyEntries(n));
    }
    let mut entries: Vec<Entry> = Vec::with_capacity(n as usize);
    for i in 0..n as usize {
        let at = ifd
            .checked_add(2 + i * 12)
            .ok_or(TiffError::BadOffset("IFD entry"))?;
        let tag = u16_at(s, at, "IFD entry")?;
        let typ = u16_at(s, at + 2, "IFD entry")?;
        let count = u64::from(u32_at(s, at + 4, "IFD entry")?);
        if entries.iter().any(|e| e.tag == tag) {
            return Err(TiffError::DuplicateTag(tag));
        }
        let Some(size) = type_size(typ) else {
            // An unknown type in a tag we do not read is harmless; in one we read it is caught
            // by the typed accessors, which see typ and refuse it.
            entries.push(Entry {
                tag,
                typ,
                count,
                at: at + 8,
            });
            continue;
        };
        let total = size.saturating_mul(count);
        let value_at = if total <= 4 {
            at + 8
        } else {
            u32_at(s, at + 8, "IFD entry")? as usize
        };
        if total > 4 {
            let end = (value_at as u64).saturating_add(total);
            if end > data.len() as u64 {
                // Only fatal if the tag is one we read; recorded as out of range via `at`.
                entries.push(Entry {
                    tag,
                    typ,
                    count,
                    at: usize::MAX / 2,
                });
                continue;
            }
        }
        entries.push(Entry {
            tag,
            typ,
            count,
            at: value_at,
        });
    }
    let req = |tag: u16, name: &'static str| find(&entries, tag).ok_or(TiffError::MissingTag(name));

    let width = uint1(s, req(TAG_WIDTH, "ImageWidth")?)?;
    let height = uint1(s, req(TAG_HEIGHT, "ImageLength")?)?;
    if width < 2 || height < 2 {
        return Err(TiffError::BadValue("image smaller than 2 by 2"));
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(TiffError::TooLarge {
            what: "image dimension",
            value: u64::from(width.max(height)),
            limit: u64::from(MAX_DIMENSION),
        });
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PIXELS {
        return Err(TiffError::TooLarge {
            what: "pixel count",
            value: pixels,
            limit: MAX_PIXELS,
        });
    }
    let samples = match find(&entries, TAG_SAMPLES) {
        Some(e) => uint1(s, e)?,
        None => 1,
    };
    if samples != 1 {
        return Err(TiffError::Unsupported {
            what: "samples per pixel",
            value: samples,
        });
    }
    let planar = match find(&entries, TAG_PLANAR) {
        Some(e) => uint1(s, e)?,
        None => 1,
    };
    if planar != 1 {
        return Err(TiffError::Unsupported {
            what: "planar configuration",
            value: planar,
        });
    }
    let bits = match find(&entries, TAG_BITS) {
        Some(e) => uint1(s, e)?,
        None => return Err(TiffError::MissingTag("BitsPerSample")),
    };
    let sample_format = match find(&entries, TAG_SAMPLE_FORMAT) {
        Some(e) => uint1(s, e)?,
        None => 1,
    };
    let format = match (bits, sample_format) {
        (32, 3) => Format::F32,
        (16, 2) => Format::I16,
        (b, f) => {
            return Err(TiffError::Unsupported {
                what: "sample layout (bits, format)",
                value: b * 100 + f,
            })
        }
    };
    // Samples are placed row 0 first, left to right; any other orientation would put terrain in
    // the wrong place, so it is refused rather than guessed at.
    let orientation = match find(&entries, TAG_ORIENTATION) {
        Some(e) => uint1(s, e)?,
        None => 1,
    };
    if orientation != 1 {
        return Err(TiffError::Unsupported {
            what: "orientation",
            value: orientation,
        });
    }
    let compression = match find(&entries, TAG_COMPRESSION) {
        Some(e) => uint1(s, e)?,
        None => COMPRESSION_NONE,
    };
    if !matches!(
        compression,
        COMPRESSION_NONE | COMPRESSION_LZW | COMPRESSION_DEFLATE | COMPRESSION_DEFLATE_ADOBE
    ) {
        return Err(TiffError::Unsupported {
            what: "compression",
            value: compression,
        });
    }
    let predictor = match find(&entries, TAG_PREDICTOR) {
        Some(e) => uint1(s, e)?,
        None => 1,
    };
    match (predictor, format) {
        (1, _) | (2, Format::I16) | (3, Format::F32) => {}
        (p, _) => {
            return Err(TiffError::Unsupported {
                what: "predictor for this sample type",
                value: p,
            })
        }
    }

    // Segment geometry: tiles if TileWidth is present, else strips.
    let tiled = find(&entries, TAG_TILE_WIDTH).is_some();
    let (seg_w, seg_h, offsets, counts) = if tiled {
        let tw = uint1(s, req(TAG_TILE_WIDTH, "TileWidth")?)?;
        let th = uint1(s, req(TAG_TILE_LENGTH, "TileLength")?)?;
        if tw == 0 || th == 0 || tw % 16 != 0 || th % 16 != 0 {
            return Err(TiffError::BadValue(
                "tile size must be a positive multiple of 16",
            ));
        }
        (
            tw,
            th,
            uints(s, req(TAG_TILE_OFFSETS, "TileOffsets")?)?,
            uints(s, req(TAG_TILE_COUNTS, "TileByteCounts")?)?,
        )
    } else {
        let rps = match find(&entries, TAG_ROWS_PER_STRIP) {
            Some(e) => uint1(s, e)?.min(height),
            None => height,
        };
        if rps == 0 {
            return Err(TiffError::BadValue("RowsPerStrip is zero"));
        }
        (
            width,
            rps,
            uints(s, req(TAG_STRIP_OFFSETS, "StripOffsets")?)?,
            uints(s, req(TAG_STRIP_COUNTS, "StripByteCounts")?)?,
        )
    };
    let across = u64::from(width).div_ceil(u64::from(seg_w));
    let down = u64::from(height).div_ceil(u64::from(seg_h));
    let nseg = if tiled { across * down } else { down };
    if offsets.len() as u64 != nseg || counts.len() as u64 != nseg {
        return Err(TiffError::BadValue(
            "segment offset/count arrays do not match the image size",
        ));
    }
    // Strips are stored at their true height (the last may be short); tiles always in full.
    let bps = bytes_per_sample(format) as u64;
    let full_bytes = u64::from(seg_w) * u64::from(seg_h) * bps;
    if full_bytes > MAX_SEGMENT_BYTES {
        return Err(TiffError::TooLarge {
            what: "strip or tile bytes",
            value: full_bytes,
            limit: MAX_SEGMENT_BYTES,
        });
    }

    let geo_tags = GeoTags {
        pixel_scale: match find(&entries, TAG_PIXEL_SCALE) {
            Some(e) => Some(read_doubles(s, e)?),
            None => None,
        },
        tiepoints: match find(&entries, TAG_TIEPOINT) {
            Some(e) => Some(read_doubles(s, e)?),
            None => None,
        },
        has_transformation: find(&entries, TAG_TRANSFORMATION).is_some(),
        geo_keys: match find(&entries, TAG_GEO_KEYS) {
            Some(e) => Some(uints(s, e)?),
            None => None,
        },
        has_geo_doubles: find(&entries, TAG_GEO_DOUBLES).is_some(),
        has_geo_ascii: find(&entries, TAG_GEO_ASCII).is_some(),
        nodata: match find(&entries, TAG_NODATA) {
            Some(e) => Some(ascii(s, e)?),
            None => None,
        },
    };
    let geo = georeference(&geo_tags, width, height)?;

    // Every segment must lie in the file and be able to hold what the declared image needs,
    // checked before the elevation array is sized from the declaration.
    for (idx, (&off, &cnt)) in offsets.iter().zip(counts.iter()).enumerate() {
        let rows_stored = if tiled {
            u64::from(seg_h)
        } else {
            u64::from(seg_h).min(u64::from(height) - idx as u64 * u64::from(seg_h))
        };
        let expected = u64::from(seg_w) * rows_stored * bps;
        let fits_file = off.checked_add(cnt).is_some_and(|e| e <= data.len() as u64);
        if !fits_file {
            return Err(TiffError::Decompress {
                segment: idx,
                reason: "segment lies outside the file",
            });
        }
        let reachable = if compression == COMPRESSION_NONE {
            cnt >= expected
        } else {
            cnt.saturating_mul(MAX_COMPRESSION_RATIO) >= expected
        };
        if !reachable {
            return Err(TiffError::Decompress {
                segment: idx,
                reason: "segment byte count cannot hold the declared image",
            });
        }
    }

    let mut elev = vec![f32::NAN; pixels as usize];
    let mut nodata_count: u64 = 0;
    for (idx, (&off, &cnt)) in offsets.iter().zip(counts.iter()).enumerate() {
        let (x0, y0, rows_stored) = if tiled {
            let tx = idx as u64 % across;
            let ty = idx as u64 / across;
            (
                tx * u64::from(seg_w),
                ty * u64::from(seg_h),
                u64::from(seg_h),
            )
        } else {
            let y0 = idx as u64 * u64::from(seg_h);
            (0, y0, u64::from(seg_h).min(u64::from(height) - y0))
        };
        let expected = (u64::from(seg_w) * rows_stored * bps) as usize;
        let end = off
            .checked_add(cnt)
            .filter(|&e| e <= data.len() as u64)
            .ok_or(TiffError::Decompress {
                segment: idx,
                reason: "segment lies outside the file",
            })?;
        let comp = data
            .get(off as usize..end as usize)
            .ok_or(TiffError::Decompress {
                segment: idx,
                reason: "segment lies outside the file",
            })?;
        let raw = decompress(comp, compression, expected, idx)?;
        let vals = to_samples(
            &raw,
            seg_w as usize,
            rows_stored as usize,
            format,
            predictor,
            big,
            idx,
        )?;
        // Copy the part of the segment that lies inside the image.
        let copy_w = (u64::from(seg_w)).min(u64::from(width) - x0) as usize;
        for r in 0..rows_stored as usize {
            let y = y0 as usize + r;
            if y >= height as usize {
                break;
            }
            let src_row = vals
                .get(r * seg_w as usize..r * seg_w as usize + copy_w)
                .ok_or(TiffError::Decompress {
                    segment: idx,
                    reason: "internal row bounds",
                })?;
            let dst_at = y * width as usize + x0 as usize;
            let dst = elev
                .get_mut(dst_at..dst_at + copy_w)
                .ok_or(TiffError::BadValue("segment outside image"))?;
            for (d, &v) in dst.iter_mut().zip(src_row.iter()) {
                *d = v;
            }
        }
    }
    // Nodata: the declared value, and any NaN, become NaN; counted once.
    for v in elev.iter_mut() {
        let is_nodata = v.is_nan()
            || match geo.nodata {
                Some(nd) => f64::from(*v) == nd,
                None => false,
            };
        if is_nodata {
            *v = f32::NAN;
            nodata_count += 1;
        }
    }
    Ok(Grid {
        width,
        height,
        lon0: geo.lon0,
        lat0: geo.lat0,
        dlon: geo.dlon,
        dlat: geo.dlat,
        elev,
        nodata_count,
        point_registered: geo.point_registered,
        crs: geo.crs,
        vertical_cs: geo.vertical_cs,
    })
}

fn read_doubles(s: Src<'_>, e: Entry) -> Result<Vec<f64>, TiffError> {
    doubles(s.data, s.big, e.typ, e.count, e.at).map_err(|er| match er {
        TiffError::BadTagType { typ, .. } => TiffError::BadTagType { tag: e.tag, typ },
        TiffError::TagTooLarge { count, .. } => TiffError::TagTooLarge { tag: e.tag, count },
        other => other,
    })
}

pub(crate) fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// Decompresses one strip or tile to exactly `expected` bytes.
fn decompress(
    comp: &[u8],
    compression: u32,
    expected: usize,
    segment: usize,
) -> Result<Vec<u8>, TiffError> {
    let fail = |reason: &'static str| TiffError::Decompress { segment, reason };
    match compression {
        COMPRESSION_NONE => {
            if comp.len() < expected {
                return Err(fail(
                    "uncompressed segment is shorter than its dimensions need",
                ));
            }
            Ok(comp.get(..expected).unwrap_or(comp).to_vec())
        }
        COMPRESSION_LZW => lzw_decode(comp, expected).map_err(|e| match e {
            crate::lzw::LzwError::Truncated => fail("LZW stream ends early"),
            crate::lzw::LzwError::BadCode => fail("LZW code out of range"),
            crate::lzw::LzwError::TooLong => fail("LZW stream is longer than the segment"),
        }),
        _ => {
            // zlib (RFC 1950): CMF/FLG, DEFLATE data, Adler-32 of the output.
            let (cmf, flg) = match (comp.first(), comp.get(1)) {
                (Some(&a), Some(&b)) => (a, b),
                _ => return Err(fail("zlib header missing")),
            };
            if cmf & 0x0f != 8 || (u16::from(cmf) * 256 + u16::from(flg)) % 31 != 0 {
                return Err(fail("bad zlib header"));
            }
            if flg & 0x20 != 0 {
                return Err(fail("zlib preset dictionary is not supported"));
            }
            if comp.len() < 6 {
                return Err(fail("zlib stream too short"));
            }
            let body = comp.get(2..comp.len() - 4).unwrap_or(&[]);
            let trailer = comp.get(comp.len() - 4..).unwrap_or(&[]);
            let out = oa_zip::inflate(body, expected, expected)
                .map_err(|_| fail("DEFLATE data is invalid or longer than the segment"))?;
            if out.len() != expected {
                return Err(fail("DEFLATE data is shorter than the segment"));
            }
            let want = trailer
                .try_into()
                .map(u32::from_be_bytes)
                .map_err(|_| fail("zlib trailer"))?;
            if adler32(&out) != want {
                return Err(fail("zlib Adler-32 mismatch"));
            }
            Ok(out)
        }
    }
}

/// Applies the predictor and converts one segment's bytes to f32 samples, row by row.
fn to_samples(
    raw: &[u8],
    row_w: usize,
    rows: usize,
    format: Format,
    predictor: u32,
    big: bool,
    segment: usize,
) -> Result<Vec<f32>, TiffError> {
    let bps = bytes_per_sample(format);
    let row_bytes = row_w * bps;
    let mut out = Vec::with_capacity(row_w * rows);
    for row in raw.chunks_exact(row_bytes).take(rows) {
        match (format, predictor) {
            (Format::F32, 1) => {
                for &b in row.as_chunks::<4>().0 {
                    let bits = if big {
                        u32::from_be_bytes(b)
                    } else {
                        u32::from_le_bytes(b)
                    };
                    out.push(f32::from_bits(bits));
                }
            }
            (Format::F32, _) => {
                // Predictor 3: byte-wise running sum across the row, then the four byte planes
                // (most significant first, whatever the file's byte order) are interleaved.
                let mut acc = row.to_vec();
                for i in 1..acc.len() {
                    let prev = acc.get(i - 1).copied().unwrap_or(0);
                    if let Some(x) = acc.get_mut(i) {
                        *x = x.wrapping_add(prev);
                    }
                }
                for i in 0..row_w {
                    let p = |k: usize| acc.get(k * row_w + i).copied().unwrap_or(0);
                    out.push(f32::from_bits(u32::from_be_bytes([p(0), p(1), p(2), p(3)])));
                }
            }
            (Format::I16, p) => {
                let mut prev: i16 = 0;
                for (i, &b) in row.as_chunks::<2>().0.iter().enumerate() {
                    let mut v = if big {
                        i16::from_be_bytes(b)
                    } else {
                        i16::from_le_bytes(b)
                    };
                    if p == 2 && i > 0 {
                        v = v.wrapping_add(prev);
                    }
                    prev = v;
                    out.push(f32::from(v));
                }
            }
        }
    }
    if out.len() != row_w * rows {
        return Err(TiffError::Decompress {
            segment,
            reason: "segment holds fewer rows than expected",
        });
    }
    Ok(out)
}

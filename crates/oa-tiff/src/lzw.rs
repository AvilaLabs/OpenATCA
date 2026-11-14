//! TIFF 6.0 LZW decoder (section 13): MSB-first codes of 9 to 12 bits, clear code 256, end code
//! 257, and "early change" (the code width grows one code before the table is full). The old
//! LSB-first variant some pre-1990 writers used is not the TIFF 6.0 format and is not read.
//! Core (pure).

const CLEAR: u16 = 256;
const EOI: u16 = 257;
const FIRST_FREE: u16 = 258;
const TABLE: usize = 4096;

/// Why LZW decoding failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LzwError {
    /// The input ended before the end code and before the expected output was reached.
    Truncated,
    /// A code refers to a table entry that does not exist yet.
    BadCode,
    /// The output would exceed the expected size.
    TooLong,
}

/// Decodes `input` into exactly `expected` bytes. A stream that ends (end code or end of input)
/// before `expected` bytes is [`LzwError::Truncated`]; one that would produce more is
/// [`LzwError::TooLong`].
// E3: indexes below are codes checked `< next` (at most TABLE - 1) or `< TABLE` by construction.
#[allow(clippy::indexing_slicing)]
pub fn lzw_decode(input: &[u8], expected: usize) -> Result<Vec<u8>, LzwError> {
    let mut out: Vec<u8> = Vec::with_capacity(expected);
    let mut prefix = [0u16; TABLE];
    let mut suffix = [0u8; TABLE];
    let mut first = [0u8; TABLE];
    for i in 0..256usize {
        suffix[i] = i as u8;
        first[i] = i as u8;
    }
    let mut next: u16 = FIRST_FREE;
    let mut width: u32 = 9;
    let mut prev: Option<u16> = None;
    let mut acc: u32 = 0;
    let mut nbits: u32 = 0;
    let mut pos = 0usize;
    let mut stack: Vec<u8> = Vec::new();
    loop {
        while nbits < width {
            match input.get(pos) {
                Some(&b) => {
                    acc = (acc << 8) | u32::from(b);
                    nbits += 8;
                    pos += 1;
                }
                None => {
                    return if out.len() == expected {
                        Ok(out)
                    } else {
                        Err(LzwError::Truncated)
                    };
                }
            }
        }
        let code = ((acc >> (nbits - width)) & ((1 << width) - 1)) as u16;
        nbits -= width;
        acc &= (1u32 << nbits) - 1;
        if code == EOI {
            return if out.len() == expected {
                Ok(out)
            } else {
                Err(LzwError::Truncated)
            };
        }
        if code == CLEAR {
            next = FIRST_FREE;
            width = 9;
            prev = None;
            continue;
        }
        // The string for `code`, and its first byte, appended to the output.
        match prev {
            None => {
                if code >= 256 {
                    return Err(LzwError::BadCode);
                }
                out.push(code as u8);
            }
            Some(p) => {
                let first_byte;
                if code < next {
                    stack.clear();
                    let mut c = code;
                    while c >= 256 {
                        stack.push(suffix[usize::from(c)]);
                        c = prefix[usize::from(c)];
                    }
                    stack.push(c as u8);
                    first_byte = first[usize::from(code)];
                    out.extend(stack.iter().rev());
                } else if code == next && next < TABLE as u16 {
                    // The KwKwK case: previous string plus its own first byte.
                    stack.clear();
                    let mut c = p;
                    while c >= 256 {
                        stack.push(suffix[usize::from(c)]);
                        c = prefix[usize::from(c)];
                    }
                    stack.push(c as u8);
                    first_byte = first[usize::from(p)];
                    out.extend(stack.iter().rev());
                    out.push(first_byte);
                } else {
                    return Err(LzwError::BadCode);
                }
                if next < TABLE as u16 {
                    prefix[usize::from(next)] = p;
                    suffix[usize::from(next)] = first_byte;
                    first[usize::from(next)] = first[usize::from(p)];
                    next += 1;
                    width = match next {
                        0..=510 => 9,
                        511..=1022 => 10,
                        1023..=2046 => 11,
                        _ => 12,
                    };
                }
            }
        }
        if out.len() > expected {
            return Err(LzwError::TooLong);
        }
        prev = Some(code);
    }
}

//! The parser: one pass, explicit stack, no recursion. Core (pure).

use crate::{Doc, ErrorKind, JsonError, Kind, Limits};

/// An open container: its node index and the elements (or members) counted so far.
// Budget: at most `max_depth` (64 by default) live at once; two u32 and a flag.
#[derive(Clone, Copy)]
#[repr(C)]
struct Open {
    node: u32,
    count: u32,
    is_object: bool,
}
const _: () = assert!(core::mem::size_of::<Open>() == 12);

struct P<'a> {
    inp: &'a [u8],
    pos: usize,
    lim: Limits,
    doc: Doc<'a>,
}

fn err<T>(offset: usize, kind: ErrorKind) -> Result<T, JsonError> {
    Err(JsonError { offset, kind })
}

/// Parses `input` as one RFC 8259 JSON text under `limits`.
pub fn parse<'a>(input: &'a [u8], limits: &Limits) -> Result<Doc<'a>, JsonError> {
    if input.len() > limits.max_input_bytes || input.len() > u32::MAX as usize {
        return err(
            limits.max_input_bytes.min(input.len()),
            ErrorKind::InputTooLarge,
        );
    }
    if let Err(e) = std::str::from_utf8(input) {
        return err(e.valid_up_to(), ErrorKind::InvalidUtf8);
    }
    let mut p = P {
        inp: input,
        pos: 0,
        lim: *limits,
        doc: Doc {
            input,
            pool: Vec::new(),
            kind: Vec::new(),
            off: Vec::new(),
            len: Vec::new(),
            end: Vec::new(),
        },
    };
    p.run()?;
    Ok(p.doc)
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<u8> {
        self.inp.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    /// Adds a node and returns its index.
    fn add(&mut self, kind: Kind, off: u32, len: u32) -> Result<u32, JsonError> {
        let ix = self.doc.kind.len();
        if ix >= self.lim.max_nodes || ix >= u32::MAX as usize - 1 {
            return err(self.pos, ErrorKind::TooManyNodes);
        }
        self.doc.kind.push(kind);
        self.doc.off.push(off);
        self.doc.len.push(len);
        self.doc.end.push(ix as u32 + 1);
        Ok(ix as u32)
    }

    fn literal(&mut self, word: &[u8], kind: Kind) -> Result<(), JsonError> {
        for (i, &w) in word.iter().enumerate() {
            match self.inp.get(self.pos + i) {
                Some(&b) if b == w => {}
                Some(_) => return err(self.pos + i, ErrorKind::UnexpectedByte),
                None => return err(self.inp.len(), ErrorKind::UnexpectedEnd),
            }
        }
        self.add(kind, 0, 0)?;
        self.pos += word.len();
        Ok(())
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        self.pos - start
    }

    fn number(&mut self) -> Result<(), JsonError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return err(self.pos, ErrorKind::BadNumber),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if self.digits() == 0 {
                return err(self.pos, ErrorKind::BadNumber);
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if self.digits() == 0 {
                return err(self.pos, ErrorKind::BadNumber);
            }
        }
        self.add(Kind::Number, start as u32, (self.pos - start) as u32)?;
        Ok(())
    }

    fn hex4(&self, at: usize) -> Result<u32, JsonError> {
        let mut v = 0u32;
        for i in 0..4 {
            let d = match self.inp.get(at + i) {
                Some(&b) => match b {
                    b'0'..=b'9' => u32::from(b - b'0'),
                    b'a'..=b'f' => u32::from(b - b'a') + 10,
                    b'A'..=b'F' => u32::from(b - b'A') + 10,
                    _ => return err(at + i, ErrorKind::BadEscape),
                },
                None => return err(self.inp.len(), ErrorKind::UnexpectedEnd),
            };
            v = v * 16 + d;
        }
        Ok(v)
    }

    /// Parses a string starting at the opening quote; adds its node.
    fn string(&mut self) -> Result<(), JsonError> {
        let open = self.pos;
        self.pos += 1;
        let start = self.pos;
        // Fast path: no escapes.
        loop {
            if self.pos - start > self.lim.max_string_bytes {
                return err(self.pos, ErrorKind::StringTooLong);
            }
            match self.peek() {
                None => return err(self.inp.len(), ErrorKind::UnexpectedEnd),
                Some(b'"') => {
                    self.add(Kind::String, start as u32, (self.pos - start) as u32)?;
                    self.pos += 1;
                    return Ok(());
                }
                Some(b'\\') => break,
                Some(0..=0x1f) => return err(self.pos, ErrorKind::ControlInString),
                Some(_) => self.pos += 1,
            }
        }
        // Slow path: decode into the pool.
        let pool_start = self.doc.pool.len();
        self.doc
            .pool
            .extend_from_slice(self.inp.get(start..self.pos).unwrap_or(&[]));
        loop {
            if self.pos - open > self.lim.max_string_bytes {
                return err(self.pos, ErrorKind::StringTooLong);
            }
            let Some(b) = self.peek() else {
                return err(self.inp.len(), ErrorKind::UnexpectedEnd);
            };
            match b {
                b'"' => {
                    let len = self.doc.pool.len() - pool_start;
                    self.add(Kind::StringPooled, pool_start as u32, len as u32)?;
                    self.pos += 1;
                    return Ok(());
                }
                0..=0x1f => return err(self.pos, ErrorKind::ControlInString),
                b'\\' => {
                    let Some(&e) = self.inp.get(self.pos + 1) else {
                        return err(self.inp.len(), ErrorKind::UnexpectedEnd);
                    };
                    let plain = match e {
                        b'"' => Some(b'"'),
                        b'\\' => Some(b'\\'),
                        b'/' => Some(b'/'),
                        b'b' => Some(8),
                        b'f' => Some(12),
                        b'n' => Some(b'\n'),
                        b'r' => Some(b'\r'),
                        b't' => Some(b'\t'),
                        _ => None,
                    };
                    if let Some(c) = plain {
                        self.doc.pool.push(c);
                        self.pos += 2;
                    } else if e == b'u' {
                        let mut cp = self.hex4(self.pos + 2)?;
                        let mut used = 6;
                        if (0xd800..0xdc00).contains(&cp) {
                            let ok = self.inp.get(self.pos + 6) == Some(&b'\\')
                                && self.inp.get(self.pos + 7) == Some(&b'u');
                            if !ok {
                                return err(self.pos, ErrorKind::BadEscape);
                            }
                            let lo = self.hex4(self.pos + 8)?;
                            if !(0xdc00..0xe000).contains(&lo) {
                                return err(self.pos + 6, ErrorKind::BadEscape);
                            }
                            cp = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00);
                            used = 12;
                        } else if (0xdc00..0xe000).contains(&cp) {
                            return err(self.pos, ErrorKind::BadEscape);
                        }
                        let Some(ch) = char::from_u32(cp) else {
                            return err(self.pos, ErrorKind::BadEscape);
                        };
                        let mut buf = [0u8; 4];
                        self.doc
                            .pool
                            .extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        self.pos += used;
                    } else {
                        return err(self.pos + 1, ErrorKind::BadEscape);
                    }
                }
                _ => {
                    self.doc.pool.push(b);
                    self.pos += 1;
                }
            }
        }
    }

    /// A member key and its colon, with the cursor left at the value.
    fn key(&mut self) -> Result<(), JsonError> {
        match self.peek() {
            Some(b'"') => self.string()?,
            Some(_) => return err(self.pos, ErrorKind::UnexpectedByte),
            None => return err(self.inp.len(), ErrorKind::UnexpectedEnd),
        }
        self.skip_ws();
        match self.peek() {
            Some(b':') => self.pos += 1,
            Some(_) => return err(self.pos, ErrorKind::UnexpectedByte),
            None => return err(self.inp.len(), ErrorKind::UnexpectedEnd),
        }
        self.skip_ws();
        Ok(())
    }

    fn open(&mut self, stack: &mut Vec<Open>, kind: Kind) -> Result<(), JsonError> {
        if stack.len() >= self.lim.max_depth {
            return err(self.pos, ErrorKind::DepthExceeded);
        }
        let node = self.add(kind, 0, 0)?;
        stack.push(Open {
            node,
            count: 0,
            is_object: kind == Kind::Object,
        });
        self.pos += 1;
        Ok(())
    }

    /// Closes the innermost container: records its element count and subtree end.
    fn close(&mut self, stack: &mut Vec<Open>, count: u32) {
        if let Some(top) = stack.pop() {
            let end = self.doc.kind.len() as u32;
            if let Some(l) = self.doc.len.get_mut(top.node as usize) {
                *l = count;
            }
            if let Some(e) = self.doc.end.get_mut(top.node as usize) {
                *e = end;
            }
        }
        self.pos += 1;
    }

    fn run(&mut self) -> Result<(), JsonError> {
        let mut stack: Vec<Open> = Vec::new();
        self.skip_ws();
        // `want_value`: a value starts at the cursor; otherwise a container is awaiting , or close.
        let mut want_value = true;
        loop {
            if want_value {
                match self.peek() {
                    None => return err(self.inp.len(), ErrorKind::UnexpectedEnd),
                    Some(b'{') => {
                        self.open(&mut stack, Kind::Object)?;
                        self.skip_ws();
                        if self.peek() == Some(b'}') {
                            self.close(&mut stack, 0);
                            want_value = false;
                        } else {
                            self.key()?;
                        }
                    }
                    Some(b'[') => {
                        self.open(&mut stack, Kind::Array)?;
                        self.skip_ws();
                        if self.peek() == Some(b']') {
                            self.close(&mut stack, 0);
                            want_value = false;
                        }
                    }
                    Some(b'"') => {
                        self.string()?;
                        want_value = false;
                    }
                    Some(b't') => {
                        self.literal(b"true", Kind::True)?;
                        want_value = false;
                    }
                    Some(b'f') => {
                        self.literal(b"false", Kind::False)?;
                        want_value = false;
                    }
                    Some(b'n') => {
                        self.literal(b"null", Kind::Null)?;
                        want_value = false;
                    }
                    Some(b'-' | b'0'..=b'9') => {
                        self.number()?;
                        want_value = false;
                    }
                    Some(_) => return err(self.pos, ErrorKind::UnexpectedByte),
                }
                if want_value {
                    continue;
                }
            }
            // A value just ended.
            // Index of the first top-of-stack frame, then count the value it completed.
            let Some(top) = stack.last_mut() else {
                self.skip_ws();
                return match self.peek() {
                    None => Ok(()),
                    Some(_) => err(self.pos, ErrorKind::TrailingGarbage),
                };
            };
            // A container's closing was already counted by the parent below; count scalars and
            // closed children as one element/member of `top` exactly once, here.
            top.count += 1;
            let (is_object, count) = (top.is_object, top.count);
            self.skip_ws();
            match self.peek() {
                None => return err(self.inp.len(), ErrorKind::UnexpectedEnd),
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                    if is_object {
                        self.key()?;
                    }
                    want_value = true;
                }
                Some(b'}') if is_object => {
                    self.close(&mut stack, count);
                }
                Some(b']') if !is_object => {
                    self.close(&mut stack, count);
                }
                Some(_) => return err(self.pos, ErrorKind::UnexpectedByte),
            }
        }
    }
}

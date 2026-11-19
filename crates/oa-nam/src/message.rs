//! Whole-message parse and encode, driven by the table in [`crate::schema`] (Part II section 3,
//! Part III 2.4). Core (pure). `parse` is strict: it accepts exactly the canonical text of every
//! message it understands and refuses the rest with the LRM code a receiver would answer with.
//! [`parse_tolerant`] additionally accepts the one deviation the ICD's own examples show
//! (spaces next to a field separator), and says that it did.

use crate::error::{ErrorKind as K, NamError};
use crate::field::*;
use crate::model::*;
use crate::schema::{kind_of, spec, spec_for, F18Rule, Req, Rule, Spec};

/// The shortest message: an IRQ (NAS-IC-82422100 3.2.2.3.1).
pub const MIN_MESSAGE_LEN: usize = 17;

/// Most amendments in one Field 22: the nine amendable fields, each at most once.
pub const MAX_AMENDMENTS: usize = AMENDABLE.len();

fn err(kind: K, field: u8, offset: usize) -> NamError {
    NamError::new(kind, field).at(offset)
}

// ---------------------------------------------------------------------------------- field 03

fn facility(b: &[u8], kind: K, offset: usize) -> Result<Facility, NamError> {
    match <[u8; 4]>::try_from(b) {
        Ok(f) if all(&f, is_upper) => Ok(Facility(f)),
        _ => Err(err(kind, 3, offset)),
    }
}

/// `SSSS/RRRRNNN`, twelve bytes.
fn msg_id(b: &[u8], first: K, offset: usize) -> Result<MsgId, NamError> {
    let g = |from: usize, to: usize| b.get(from..to).unwrap_or(&[]);
    if b.len() != 12 || b.get(4) != Some(&b'/') {
        return Err(err(K::Field03Syntax, 3, offset));
    }
    let (sk, rk, nk) = if first == K::Reference {
        (K::Reference, K::Reference, K::Reference)
    } else {
        (K::SendingUnit, K::ReceivingUnit, K::MessageNumber)
    };
    let sender = facility(g(0, 4), sk, offset)?;
    let receiver = facility(g(5, 9), rk, offset + 5)?;
    let number = num(g(9, 12)).ok_or(err(nk, 3, offset + 9))? as u16;
    Ok(MsgId {
        sender,
        receiver,
        number,
    })
}

fn fmt_msg_id(id: &MsgId, out: &mut String) {
    out.push_str(&text(&id.sender.0));
    out.push('/');
    out.push_str(&text(&id.receiver.0));
    out.push_str(&format!("{:03}", id.number));
}

/// Parses the first chunk: the identifier, 03(b) and optional 03(c).
pub fn parse_header(b: &[u8]) -> Result<Header, NamError> {
    parse_header_with(b, Profile::default())
}

/// [`parse_header`] under an explicit validation [`Profile`] (03(c) presence differs in Rev E).
pub fn parse_header_with(b: &[u8], profile: Profile) -> Result<Header, NamError> {
    let Some((mn, rest)) = b.split_at_checked(3) else {
        return Err(err(K::Mnemonic, 3, 0));
    };
    let kind = kind_of(mn).ok_or(err(K::Mnemonic, 3, 0))?;
    let id = msg_id(rest.get(..12).unwrap_or(rest), K::SendingUnit, 3)?;
    let reference = match rest.len() {
        12 => None,
        24 => Some(msg_id(rest.get(12..).unwrap_or(&[]), K::Reference, 15)?),
        _ => return Err(err(K::Field03Syntax, 3, 3)),
    };
    match (spec_for(kind, profile).reference, reference.is_some()) {
        (Req::No, true) | (Req::Yes, false) => return Err(err(K::ReferencePresence, 3, 3)),
        _ => {}
    }
    Ok(Header {
        kind,
        id,
        reference,
    })
}

pub fn fmt_header(h: &Header, out: &mut String) {
    out.push_str(spec(h.kind).mnemonic);
    fmt_msg_id(&h.id, out);
    if let Some(r) = &h.reference {
        fmt_msg_id(r, out);
    }
}

// ---------------------------------------------------------------------------------- parse

struct Chunk<'a> {
    bytes: &'a [u8],
    offset: usize,
}

fn field_number(r: &Rule) -> u8 {
    match r {
        Rule::F07 { .. } => 7,
        Rule::F08 => 8,
        Rule::F09 => 9,
        Rule::F10 => 10,
        Rule::F13 { .. } => 13,
        Rule::F14 => 14,
        Rule::F15 => 15,
        Rule::F16 { .. } => 16,
        Rule::F18 { .. } => 18,
        Rule::F22 { .. } => 22,
        Rule::F31 { .. } => 31,
        Rule::F32 => 32,
    }
}

fn req_check(rule: Req, present: bool, kind: K, field: u8) -> Result<(), NamError> {
    match (rule, present) {
        (Req::No, true) | (Req::Yes, false) => Err(NamError::new(kind, field)),
        _ => Ok(()),
    }
}

fn parse_f22(chunks: &[Chunk], abi: bool, profile: Profile) -> Result<Vec<Amendment>, NamError> {
    let mut out: Vec<Amendment> = Vec::new();
    for c in chunks {
        if out.len() == MAX_AMENDMENTS {
            return Err(err(K::TooManyFields, 22, c.offset));
        }
        let a = parse_amendment(c.bytes, profile).map_err(|e| e.at(c.offset))?;
        let n = amend_field_number(&a.data);
        if out.iter().any(|x| amend_field_number(&x.data) == n) {
            return Err(err(K::AmendmentField, 22, c.offset));
        }
        if abi {
            // Only 8, 9, 10, 15 and 18 can be amended in an ABI, each once; the ICD sets no
            // order among them.
            if !matches!(n, 8 | 9 | 10 | 15 | 18) {
                return Err(err(K::AmendmentField, 22, c.offset));
            }
        }
        out.push(a);
    }
    check_f22(&out, abi)?;
    Ok(out)
}

/// What Field 22 must hold: at least one amendment; in CHG and MOD a change of 07(a) is the
/// only change (Part II 3.1.2.3); in ABI fields 9 and 15 are present (Table 9).
fn check_f22(a: &[Amendment], abi: bool) -> Result<(), NamError> {
    if a.is_empty() {
        return Err(NamError::new(K::AmendmentMissing, 22));
    }
    let has = |n: u8| a.iter().any(|x| amend_field_number(&x.data) == n);
    if abi {
        for n in [9, 15] {
            if !has(n) {
                return Err(NamError::new(K::MissingField, n));
            }
        }
    } else if has(7) && a.len() > 1 {
        return Err(NamError::new(K::AmendmentField, 22));
    }
    Ok(())
}

fn parse_f18_rule(c: &Chunk, rule: F18Rule) -> Result<Field18, NamError> {
    let f = match rule {
        F18Rule::Any | F18Rule::RemarkIfAny => parse_f18(c.bytes),
        F18Rule::Remark | F18Rule::Lrm => parse_remark(c.bytes),
    }
    .map_err(|e| e.at(c.offset))?;
    match (rule, &f) {
        (F18Rule::RemarkIfAny, Field18::Items(v)) if !v.iter().any(|i| i.name == "RMK") => {
            Err(err(K::OtherInfo, 18, c.offset))
        }
        (F18Rule::Lrm, f) => {
            let t = remark_text(f).ok_or(err(K::LrmText, 18, c.offset))?;
            parse_lrm_text(t).map_err(|e| e.at(c.offset))?;
            Ok(f.clone())
        }
        _ => Ok(f),
    }
}

/// Parses one message: the bytes from `(` to `)` inclusive, without the four-byte length
/// header of the transport (see [`crate::frame`]).
pub fn parse(b: &[u8]) -> Result<Message, NamError> {
    parse_with(b, Profile::default())
}

/// The kind named by the three letters after the opening parenthesis, if they name a core
/// message: what a receiver needs to pick the LRM code of a refused message
/// ([`NamError::lrm_for`]) even though the message did not parse.
pub fn peek_kind(b: &[u8]) -> Option<Kind> {
    b.get(1..4).and_then(kind_of)
}

/// [`parse`] under an explicit validation [`Profile`].
pub fn parse_with(b: &[u8], profile: Profile) -> Result<Message, NamError> {
    if b.len() < MIN_MESSAGE_LEN || b.len() > MAX_MESSAGE_LEN {
        return Err(NamError::new(K::Length, 0));
    }
    if let Some(i) = b.iter().position(|c| !(0x20..=0x7e).contains(c)) {
        return Err(err(K::Charset, 0, i));
    }
    let (Some(b'('), Some(b')')) = (b.first(), b.last()) else {
        return Err(NamError::new(K::Parens, 0));
    };
    let inner = b.get(1..b.len() - 1).unwrap_or(&[]);
    if let Some(i) = inner.iter().position(|&c| c == b'(' || c == b')') {
        return Err(err(K::ParensInside, 0, i + 1));
    }
    // Every hyphen is a field separator (Part III 2.4 b).
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut start = 0;
    for (i, &c) in inner.iter().enumerate() {
        if c == b'-' {
            chunks.push(Chunk {
                bytes: inner.get(start..i).unwrap_or(&[]),
                offset: start + 1,
            });
            start = i + 1;
        }
    }
    chunks.push(Chunk {
        bytes: inner.get(start..).unwrap_or(&[]),
        offset: start + 1,
    });
    let mut it = chunks.iter();
    let Some(first) = it.next() else {
        return Err(NamError::new(K::Length, 0));
    };
    let header = parse_header_with(first.bytes, profile).map_err(|e| {
        if e.offset == crate::error::NO_OFFSET {
            e
        } else {
            e.at(e.offset + first.offset)
        }
    })?;
    let sp = spec_for(header.kind, profile);
    let mut m = bare(header);
    let body: Vec<&Chunk> = it.collect();
    consume_fields(&mut m, sp, &body, profile)?;
    post_checks(&m, sp, profile)?;
    Ok(m)
}

fn consume_fields(
    m: &mut Message,
    sp: &Spec,
    body: &[&Chunk],
    profile: Profile,
) -> Result<(), NamError> {
    let mut idx = 0;
    for (ri, rule) in sp.fields.iter().enumerate() {
        let n = field_number(rule);
        if let Rule::F22 { abi } = rule {
            let rest: Vec<Chunk> = body
                .get(idx..)
                .unwrap_or(&[])
                .iter()
                .map(|c| Chunk {
                    bytes: c.bytes,
                    offset: c.offset,
                })
                .collect();
            m.f22 = parse_f22(&rest, *abi, profile)?;
            idx = body.len();
            continue;
        }
        // An optional Field 18 ahead of Field 22 is absent when the next chunk is an amendment
        // (digits and a slash, which no Field 18 starts with).
        if matches!(
            rule,
            Rule::F18 {
                required: false,
                ..
            }
        ) && matches!(sp.fields.get(ri + 1), Some(Rule::F22 { .. }))
            && body.get(idx).is_some_and(|c| looks_like_amendment(c.bytes))
        {
            continue;
        }
        let Some(c) = body.get(idx).copied() else {
            // Out of chunks: only a field that may be absent is excused.
            if matches!(
                rule,
                Rule::F18 {
                    required: false,
                    ..
                }
            ) {
                continue;
            }
            let missing = sp
                .fields
                .get(ri..)
                .unwrap_or(&[])
                .iter()
                .filter(|r| {
                    !matches!(
                        r,
                        Rule::F18 {
                            required: false,
                            ..
                        }
                    )
                })
                .count();
            return Err(if missing == 1 {
                NamError::new(K::MissingField, n)
            } else {
                NamError::new(K::TooFewFields, 0)
            });
        };
        idx += 1;
        let at = |e: NamError| e.at(c.offset);
        match rule {
            Rule::F07 { ssr, functional } => {
                let f = parse_f07(c.bytes, *functional).map_err(at)?;
                req_check(*ssr, f.ssr.is_some(), K::SsrPresence, 7).map_err(at)?;
                m.f07 = Some(f);
            }
            Rule::F08 => m.f08 = Some(parse_f08(c.bytes).map_err(at)?),
            Rule::F09 => m.f09 = Some(parse_f09(c.bytes).map_err(at)?),
            Rule::F10 => m.f10 = Some(parse_f10(c.bytes).map_err(at)?),
            Rule::F13 { time } => {
                let f = parse_f13(c.bytes).map_err(at)?;
                time_check(*time, f.time.is_some(), 13).map_err(at)?;
                m.f13 = Some(f);
            }
            Rule::F14 => m.f14 = Some(parse_f14(c.bytes, profile).map_err(at)?),
            Rule::F15 => m.f15 = Some(parse_f15(c.bytes).map_err(at)?),
            Rule::F16 { eet, alt } => {
                let f = parse_f16(c.bytes).map_err(at)?;
                time_check(*eet, f.eet.is_some(), 16).map_err(at)?;
                req_check(*alt, !f.alternates.is_empty(), K::AlternateUnexpected, 16)
                    .map_err(at)?;
                m.f16 = Some(f);
            }
            Rule::F18 { rule, .. } => m.f18 = Some(parse_f18_rule(c, *rule)?),
            Rule::F22 { .. } => {}
            Rule::F31 { sector, text } => {
                // Attachment 2 2.17: only the RTA carries contact data, and only on that
                // interface; elsewhere 31(c) is Part II's 20 alphanumeric characters.
                let (kind31, text_rule) = match (m.header.kind, profile) {
                    (Kind::Rta, Profile::EramAttachment2) => (Text31::Contact, *text),
                    (Kind::Rta, _) => (Text31::Plain, Req::No),
                    _ => (Text31::Plain, *text),
                };
                let f = parse_f31(c.bytes, kind31).map_err(at)?;
                req_check(*sector, f.sector.is_some(), K::Field31, 31).map_err(at)?;
                req_check(text_rule, f.text.is_some(), K::Field31, 31).map_err(at)?;
                m.f31 = Some(f);
            }
            Rule::F32 => m.f32 = Some(parse_f32(c.bytes, profile).map_err(at)?),
        }
    }
    if let Some(extra) = body.get(idx) {
        return Err(err(K::TooManyFields, 0, extra.offset));
    }
    Ok(())
}

/// Rules that need more than one field of the message.
fn post_checks(m: &Message, sp: &Spec, _profile: Profile) -> Result<(), NamError> {
    // CHG (Table 4): 13(b) and Field 18 are required. The one exception is the change of ACID
    // alone, a Field 22 holding the single amendment `07/` (the ICD's own example, 3.1.2.3).
    if sp.kind == Kind::Chg {
        let acid_only = matches!(
            m.f22.as_slice(),
            [Amendment {
                data: AmendData::F07(_),
                ..
            }]
        );
        if !acid_only {
            if m.f13.as_ref().is_some_and(|f| f.time.is_none()) {
                return Err(NamError::new(K::TimeMissing, 13));
            }
            if m.f18.is_none() {
                return Err(NamError::new(K::MissingField, 18));
            }
        }
    }
    // Items 10 and 18 together.
    if let (Some(f10), Some(f18)) = (&m.f10, &m.f18) {
        check_items_10_18(f10, f18)?;
    }
    Ok(())
}

fn looks_like_amendment(b: &[u8]) -> bool {
    let d = b.iter().take_while(|c| c.is_ascii_digit()).count();
    (1..=2).contains(&d) && b.get(d) == Some(&b'/')
}

fn time_check(rule: Req, present: bool, field: u8) -> Result<(), NamError> {
    match (rule, present) {
        (Req::No, true) => Err(NamError::new(K::TimeUnexpected, field)),
        (Req::Yes, false) => Err(NamError::new(K::TimeMissing, field)),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------------- tolerant

/// What [`parse_tolerant`] had to normalise. All zero means the input was canonical.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Deviations {
    /// Spaces immediately before a field separator hyphen (the ICD prints `-IG -C172/L`).
    pub space_before_hyphen: u16,
    /// Spaces immediately after a hyphen (`-AAL451- RJTD0122`).
    pub space_after_hyphen: u16,
}

/// Like [`parse`], but drops spaces next to a hyphen, which the ICD's examples contain and which
/// no field can mean (a hyphen is only ever a separator, Part III 2.4 b). Everything else is as
/// strict as [`parse`]. Not a repair of data: the dropped characters carry no information, and
/// the count is returned so a caller can log or refuse it.
pub fn parse_tolerant(b: &[u8]) -> Result<(Message, Deviations), NamError> {
    parse_tolerant_with(b, Profile::default())
}

/// [`parse_tolerant`] under an explicit validation [`Profile`].
pub fn parse_tolerant_with(b: &[u8], profile: Profile) -> Result<(Message, Deviations), NamError> {
    let mut dev = Deviations::default();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    for (i, &c) in b.iter().enumerate() {
        match c {
            b' ' if b.get(i + 1) == Some(&b'-') => {
                dev.space_before_hyphen = dev.space_before_hyphen.saturating_add(1);
            }
            b' ' if i > 0 && b.get(i - 1) == Some(&b'-') => {
                dev.space_after_hyphen = dev.space_after_hyphen.saturating_add(1);
            }
            _ => out.push(c),
        }
    }
    parse_with(&out, profile).map(|m| (m, dev))
}

// ---------------------------------------------------------------------------------- encode

fn present<T>(o: &Option<T>, n: u8) -> Result<&T, NamError> {
    o.as_ref().ok_or(NamError::new(K::MissingField, n))
}

fn forbid<T>(o: &Option<T>, n: u8) -> Result<(), NamError> {
    if o.is_some() {
        Err(NamError::new(K::TooManyFields, n))
    } else {
        Ok(())
    }
}

/// Writes the canonical text of `m` and checks that it parses back to the same value, so an
/// invalid value is refused here and never reaches the wire (E5). Returns the bytes from `(` to
/// `)`.
pub fn encode(m: &Message) -> Result<Vec<u8>, NamError> {
    encode_with(m, Profile::default())
}

/// [`encode`] under an explicit validation [`Profile`].
pub fn encode_with(m: &Message, profile: Profile) -> Result<Vec<u8>, NamError> {
    let sp = spec_for(m.header.kind, profile);
    let mut s = String::from("(");
    fmt_header(&m.header, &mut s);
    // Fields the message type does not carry must be empty.
    let carries = |n: u8| sp.fields.iter().any(|r| field_number(r) == n);
    if !carries(7) {
        forbid(&m.f07, 7)?;
    }
    if !carries(8) {
        forbid(&m.f08, 8)?;
    }
    if !carries(9) {
        forbid(&m.f09, 9)?;
    }
    if !carries(10) {
        forbid(&m.f10, 10)?;
    }
    if !carries(13) {
        forbid(&m.f13, 13)?;
    }
    if !carries(14) {
        forbid(&m.f14, 14)?;
    }
    if !carries(15) {
        forbid(&m.f15, 15)?;
    }
    if !carries(16) {
        forbid(&m.f16, 16)?;
    }
    if !carries(18) {
        forbid(&m.f18, 18)?;
    }
    if !carries(22) && !m.f22.is_empty() {
        return Err(NamError::new(K::TooManyFields, 22));
    }
    if !carries(31) {
        forbid(&m.f31, 31)?;
    }
    if !carries(32) {
        forbid(&m.f32, 32)?;
    }
    for rule in sp.fields {
        match rule {
            Rule::F07 { .. } => {
                s.push('-');
                fmt_f07(present(&m.f07, 7)?, &mut s);
            }
            Rule::F08 => {
                s.push('-');
                fmt_f08(present(&m.f08, 8)?, &mut s);
            }
            Rule::F09 => {
                s.push('-');
                fmt_f09(present(&m.f09, 9)?, &mut s);
            }
            Rule::F10 => {
                s.push('-');
                fmt_f10(present(&m.f10, 10)?, &mut s);
            }
            Rule::F13 { .. } => {
                s.push('-');
                fmt_f13(present(&m.f13, 13)?, &mut s);
            }
            Rule::F14 => {
                s.push('-');
                fmt_f14(present(&m.f14, 14)?, &mut s);
            }
            Rule::F15 => {
                s.push('-');
                fmt_f15(present(&m.f15, 15)?, &mut s);
            }
            Rule::F16 { .. } => {
                s.push('-');
                fmt_f16(present(&m.f16, 16)?, &mut s);
            }
            Rule::F18 { required, .. } => match (&m.f18, required) {
                (Some(f), _) => {
                    s.push('-');
                    fmt_f18(f, &mut s);
                }
                (None, true) => return Err(NamError::new(K::MissingField, 18)),
                (None, false) => {}
            },
            Rule::F22 { abi } => {
                check_f22(&m.f22, *abi)?;
                for a in &m.f22 {
                    s.push('-');
                    fmt_amendment(a, &mut s);
                }
            }
            Rule::F31 { .. } => {
                s.push('-');
                fmt_f31(present(&m.f31, 31)?, &mut s);
            }
            Rule::F32 => {
                s.push('-');
                fmt_f32(present(&m.f32, 32)?, &mut s);
            }
        }
    }
    s.push(')');
    let bytes = s.into_bytes();
    let back = parse_with(&bytes, profile)?;
    if &back != m {
        return Err(NamError::new(K::NotCanonical, 0));
    }
    Ok(bytes)
}

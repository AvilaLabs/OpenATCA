//! Fixtures shared by the oa-replay tests: SFDPS and TBFM documents built from parameters,
//! frame encoding, and a small deterministic PRNG. Every identifier is made up.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_feed::encode_frame;
use oa_fixm::{new_events, parse_uuid, Decoder, Events, Limits};
use oa_replay::{new_pipeline, process_frame, FeedKind, Pipeline, PipelineConfig};
use oa_tbfm::{decode_bytes, default_limits, new_rows, Rows};
use oa_xml::Parser;

/// 2026-11-14T00:00:00Z in Unix ms.
pub const DAY: i64 = 1_794_614_400_000;
pub const NS: i64 = 1_000_000_000;

/// `DAY + ms` as `YYYY-MM-DDTHH:MM:SS.mmmZ` (valid for a few days either side of DAY).
pub fn iso(ms_since_day: i64) -> String {
    let total = DAY + ms_since_day;
    let days = total.div_euclid(86_400_000);
    let rem = total.rem_euclid(86_400_000);
    // Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}

/// The GUFI text of flight number `n`.
pub fn uuid(n: u32) -> String {
    format!("{n:08x}-1111-4222-8333-{n:012x}")
}

/// The GUFI of flight number `n` as the decoder reads it.
pub fn gufi(n: u32) -> u128 {
    parse_uuid(uuid(n).as_bytes()).unwrap()
}

/// One SFDPS message.
#[derive(Clone)]
pub struct Sf {
    pub id: u32,
    pub ts_ms: i64,
    pub source: String,
    /// Latitude, longitude, position time (ms since DAY).
    pub pos: Option<(f64, f64, i64)>,
    pub alt: Option<i32>,
    pub callsign: Option<String>,
    pub plan: Option<String>,
    pub arr: Option<String>,
    pub dep: Option<String>,
    pub ctype: Option<String>,
    pub cid: Option<String>,
    pub unit: Option<(String, String)>,
    /// `flightStatus@fdpsFlightStatus`, e.g. `ACTIVE`.
    pub status: Option<String>,
}

pub fn sf(id: u32, ts_ms: i64) -> Sf {
    Sf {
        id,
        ts_ms,
        source: "TH".into(),
        pos: None,
        alt: None,
        callsign: None,
        plan: None,
        arr: None,
        dep: None,
        ctype: None,
        cid: None,
        unit: None,
        status: None,
    }
}

impl Sf {
    pub fn pos(mut self, lat: f64, lon: f64, t_ms: i64) -> Sf {
        self.pos = Some((lat, lon, t_ms));
        self
    }
    pub fn call(mut self, s: &str) -> Sf {
        self.callsign = Some(s.into());
        self
    }
    pub fn plan(mut self, s: &str) -> Sf {
        self.plan = Some(s.into());
        self
    }
    pub fn arr(mut self, s: &str) -> Sf {
        self.arr = Some(s.into());
        self
    }
    pub fn dep(mut self, s: &str) -> Sf {
        self.dep = Some(s.into());
        self
    }
    pub fn ctype(mut self, s: &str) -> Sf {
        self.ctype = Some(s.into());
        self
    }
    pub fn cid(mut self, s: &str) -> Sf {
        self.cid = Some(s.into());
        self
    }
    pub fn unit(mut self, u: &str, s: &str) -> Sf {
        self.unit = Some((u.into(), s.into()));
        self
    }
    pub fn status(mut self, s: &str) -> Sf {
        self.status = Some(s.into());
        self
    }
    pub fn alt(mut self, a: i32) -> Sf {
        self.alt = Some(a);
        self
    }
}

fn sf_message(m: &Sf) -> String {
    let mut body = format!(r#"<gufi codeSpace="urn:uuid">{}</gufi>"#, uuid(m.id));
    if let Some((u, s)) = &m.unit {
        body += &format!(
            r#"<controllingUnit xsi:type="ns2:IdentifiedUnitReferenceType" unitIdentifier="{u}" sectorIdentifier="{s}"/>"#
        );
    }
    if let Some(a) = &m.arr {
        body += &format!(r#"<arrival xsi:type="ns5:NasArrivalType" arrivalPoint="{a}"/>"#);
    }
    if let Some(d) = &m.dep {
        body += &format!(r#"<departure xsi:type="ns5:NasDepartureType" departurePoint="{d}"/>"#);
    }
    if let Some((lat, lon, t)) = m.pos {
        body += &format!(
            r#"<enRoute xsi:type="ns5:NasEnRouteType"><position xsi:type="ns5:NasAircraftPositionType" positionTime="{}" reportSource="SURVEILLANCE">"#,
            iso(t)
        );
        if let Some(a) = m.alt {
            body += &format!(r#"<altitude uom="FEET">{a}.0</altitude>"#);
        }
        body += &format!(
            r#"<position xsi:type="ns2:LocationPointType"><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>{lat:.6} {lon:.6}</pos></location></position></position></enRoute>"#
        );
    }
    if m.callsign.is_some() || m.cid.is_some() {
        let mut attrs = String::new();
        if let Some(c) = &m.cid {
            attrs += &format!(r#" computerId="{c}""#);
        }
        if let Some(c) = &m.callsign {
            attrs += &format!(r#" aircraftIdentification="{c}""#);
        }
        body += &format!(
            r#"<flightIdentification xsi:type="ns5:NasFlightIdentificationType"{attrs}/>"#
        );
    }
    if let Some(st) = &m.status {
        body += &format!(
            r#"<flightStatus xsi:type="ns5:NasFlightStatusType" fdpsFlightStatus="{st}"/>"#
        );
    }
    if let Some(t) = &m.ctype {
        body += &format!(
            r#"<aircraftDescription xsi:type="ns5:NasAircraftType"><aircraftType><icaoModelIdentifier>{t}</icaoModelIdentifier></aircraftType></aircraftDescription>"#
        );
    }
    if let Some(p) = &m.plan {
        body += &format!(r#"<flightPlan identifier="{p}"/>"#);
    }
    format!(
        r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="{}" system="SLC" timestamp="{}">{body}</flight></message>"#,
        m.source,
        iso(m.ts_ms)
    )
}

/// An SFDPS document of the given messages.
pub fn sf_doc(msgs: &[Sf]) -> String {
    let inner: String = msgs.iter().map(sf_message).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:ns2="http://www.fixm.aero/base/3.0" xmlns:ns3="http://www.fixm.aero/flight/3.0" xmlns:ns4="http://www.fixm.aero/foundation/3.0">{inner}</ns5:MessageCollection>"#
    )
}

/// Decodes an SFDPS document into events.
pub fn events(xml: &str) -> Events {
    let mut parser = Parser::new(oa_xml::Limits::default()).unwrap();
    let doc = parser.parse(xml.as_bytes()).unwrap();
    let mut dec = Decoder::new(Limits::default());
    let mut ev = new_events(512, 1 << 16);
    dec.decode(&doc, xml.as_bytes(), &mut ev).unwrap();
    ev
}

pub const TBFM_NS: &str = "urn:us:gov:dot:faa:atm:tfm:tbfmmeteringpublication:1.1.0";

/// One TBFM `air` element with sections. `msg_ms` is the message time (ms since DAY).
#[derive(Clone)]
pub struct Tb {
    pub msg_ms: i64,
    pub gufi: Option<String>,
    pub tma: String,
    pub air_type: String,
    pub aid: String,
    pub sections: Vec<String>,
}

pub fn tb(msg_ms: i64, gufi: Option<&str>, tma: &str) -> Tb {
    Tb {
        msg_ms,
        gufi: gufi.map(str::to_string),
        tma: tma.into(),
        air_type: "AMD".into(),
        aid: "TST123".into(),
        sections: Vec::new(),
    }
}

impl Tb {
    pub fn air_type(mut self, t: &str) -> Tb {
        self.air_type = t.into();
        self
    }
    pub fn flt(mut self, typ: &str) -> Tb {
        self.sections.push(format!(
            "<flt><aid>{}</aid><typ>{typ}</typ></flt>",
            self.aid
        ));
        self
    }
    pub fn mrp(mut self, mfx: &str, rwy: &str, mrp_type: &str) -> Tb {
        self.sections.push(format!(
            "<mrp mrpType=\"{mrp_type}\"><tra>TRA</tra><mfx>{mfx}</mfx><rwy>{rwy}</rwy><cat>ARRIVAL</cat></mrp>"
        ));
        self
    }
    pub fn eta(mut self, mfx: &str, eta_mfx_ms: i64) -> Tb {
        self.sections.push(format!(
            "<eta><mfx>{mfx}</mfx><eta_mfx>{}</eta_mfx></eta>",
            iso(eta_mfx_ms).replace(".000Z", "Z")
        ));
        self
    }
    pub fn sta(mut self, mfx: &str, sta_sfx_ms: i64) -> Tb {
        self.sections.push(format!(
            "<sta><mfx>{mfx}</mfx><sta_sfx>{}</sta_sfx></sta>",
            iso(sta_sfx_ms).replace(".000Z", "Z")
        ));
        self
    }
    pub fn sch(mut self, mfx: &str, sfz: &str) -> Tb {
        self.sections.push(format!(
            "<sch><mfx>{mfx}</mfx><man>NO</man><rfz>NO</rfz><sfz>{sfz}</sfz><sus>NO</sus></sch>"
        ));
        self
    }
}

fn tb_air(a: &Tb) -> String {
    let gufi = a
        .gufi
        .as_ref()
        .map(|g| format!(" gufi=\"{g}\""))
        .unwrap_or_default();
    format!(
        "<air{gufi} tmaId=\"{}\" apt=\"QQA\" dap=\"QQB\" aid=\"{}\" airType=\"{}\">{}</air>",
        a.tma,
        a.aid,
        a.air_type,
        a.sections.concat()
    )
}

/// A TBFM message of the given flights, all with message time `msg_ms`.
pub fn tb_doc(msg_ms: i64, centre: &str, airs: &[Tb]) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><env xmlns=\"{TBFM_NS}\" \
         targetNamespace=\"{TBFM_NS}\" envTime=\"{t}\" envSrce=\"TMA.{centre}.FAA.GOV\">\
         <tma msgTime=\"{t}\" msgId=\"4711\">{}</tma></env>",
        airs.iter().map(tb_air).collect::<String>(),
        t = iso(msg_ms)
    )
}

/// Decodes a TBFM document into rows.
pub fn rows(xml: &str) -> Rows {
    let mut parser = Parser::new(default_limits()).unwrap();
    let mut r = new_rows(512);
    decode_bytes(&mut parser, xml.as_bytes(), &mut r).unwrap();
    r
}

/// One OAF1 frame.
pub fn frame(kind: u8, seq: u64, recv_ns: i64, payload: &[u8]) -> Vec<u8> {
    let mut buf = vec![0u8; payload.len() + 64];
    let n = encode_frame(
        &mut buf,
        kind,
        0,
        seq,
        recv_ns,
        recv_ns / 1_000_000,
        payload,
    )
    .unwrap();
    buf.truncate(n);
    buf
}

/// A pipeline with a small table.
pub fn small_pipeline(capacity: usize) -> Pipeline {
    let mut cfg = PipelineConfig::default();
    cfg.table.capacity = capacity;
    new_pipeline(cfg).unwrap()
}

/// Feeds an XML payload as a MESSAGE frame at replay time `recv_ns`.
pub fn feed(p: &mut Pipeline, f: FeedKind, recv_ns: i64, xml: &str) {
    process_frame(p, f, oa_feed::Kind::Message, recv_ns, xml.as_bytes());
}

/// xorshift64*: a deterministic generator for shuffles.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

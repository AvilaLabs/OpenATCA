//! Reading a whole file: header, bounded line reading, grouping and the cycle rule.

use std::collections::BTreeMap;
use std::io::BufRead;

use crate::airport::{Airport, Localizer, Runway};
use crate::airspace::{volume_key, Airspace, AirspaceKind};
use crate::airway::{Airway, AirwayFix};
use crate::error::{Error, ErrorKind};
use crate::field::Rec;
use crate::header::{parse_hdr01, Header};
use crate::mora::{mora_value_at, GridMora};
use crate::navaid::{Ndb, VhfNavaid};
use crate::procedure::{Leg, Procedure, ProcedureKind, Route};
use crate::record::{parse_record, record_meta, section_code, Record};
use crate::types::{cycle_is_after, Cycle};
use crate::waypoint::{Waypoint, WaypointKind};
use crate::{
    MAX_AIRWAY_FIXES, MAX_BOUNDARY_POINTS, MAX_CONTINUATIONS_PER_PRIMARY, MAX_HEADER_RECORDS,
    MAX_LINE_LEN, MAX_RECORDS, MAX_RECORD_ERRORS, MAX_ROUTES_PER_PROCEDURE, MAX_ROUTE_LEGS,
    RECORD_LEN,
};

/// A parsed CIFP file.
#[derive(Debug, Clone, Default)]
pub struct Cifp {
    pub header: Option<Header>,
    pub airports: Vec<Airport>,
    pub runways: Vec<Runway>,
    pub localizers: Vec<Localizer>,
    pub vhf_navaids: Vec<VhfNavaid>,
    pub ndbs: Vec<Ndb>,
    /// Terminal NDBs (PN), each carrying its airport.
    pub terminal_ndbs: Vec<Ndb>,
    pub enroute_waypoints: Vec<Waypoint>,
    pub terminal_waypoints: Vec<Waypoint>,
    pub airways: Vec<Airway>,
    pub procedures: Vec<Procedure>,
    /// Class B/C/D airspace (UC).
    pub controlled_airspace: Vec<Airspace>,
    /// Special use airspace (UR).
    pub special_use_airspace: Vec<Airspace>,
    pub mora: Vec<GridMora>,
    /// Physical records per section code (`PA`, `PD`, `D`, `DB`, `HA`, ...), decoded or
    /// skipped, continuation records included.
    pub counts: BTreeMap<String, usize>,
    /// Physical records per record cycle (cols 129-132). Records keep the cycle of their
    /// last change, so a current file holds many cycles.
    pub cycle_counts: BTreeMap<Cycle, usize>,
    /// Record-level errors (at most [`MAX_RECORD_ERRORS`]); the offending records are not in
    /// the data above.
    pub errors: Vec<Error>,
    /// Record-level errors beyond the limit that were counted but not kept.
    pub errors_dropped: usize,
}

/// The cycle named by the header.
pub fn cifp_cycle(c: &Cifp) -> Option<Cycle> {
    c.header.as_ref().map(|h| h.cycle)
}

/// True when no record failed to decode.
pub fn is_clean(c: &Cifp) -> bool {
    c.errors.is_empty() && c.errors_dropped == 0
}

/// The airport with this identifier (first match).
pub fn find_airport<'a>(c: &'a Cifp, ident: &str) -> Option<&'a Airport> {
    c.airports.iter().find(|a| a.ident == ident)
}

/// The runways of one airport.
pub fn runways_at<'a>(c: &'a Cifp, airport_ident: &'a str) -> impl Iterator<Item = &'a Runway> {
    c.runways
        .iter()
        .filter(move |r| r.airport_ident == airport_ident)
}

/// The SIDs, STARs or approaches of one airport.
pub fn procedures_at<'a>(
    c: &'a Cifp,
    airport_ident: &'a str,
    kind: ProcedureKind,
) -> impl Iterator<Item = &'a Procedure> {
    c.procedures
        .iter()
        .filter(move |p| p.airport_ident == airport_ident && p.kind == kind)
}

/// Grid MORA in feet at a position; `None` when no cell holds a value there.
pub fn mora_at(c: &Cifp, lat: f64, lon: f64) -> Option<u32> {
    c.mora.iter().find_map(|m| mora_value_at(m, lat, lon))
}

/// Parses a whole file held in memory. `source` names it in errors.
pub fn parse_bytes(bytes: &[u8], source: &str) -> Result<Cifp, Error> {
    parse_reader(bytes, source)
}

/// Parses a file from a reader. Lines are read with a hard length bound, so a file with no
/// line breaks cannot make the parser buffer it all.
///
/// Fatal problems (a limit exceeded, no header, a cycle later than the header's, a record
/// count that disagrees with the header, an unreadable stream) return `Err`. A record that
/// fails to decode is recorded in [`Cifp::errors`] and parsing continues.
pub fn parse_reader<R: BufRead>(mut reader: R, source: &str) -> Result<Cifp, Error> {
    let mut b = Builder::new(source);
    let mut buf: Vec<u8> = Vec::with_capacity(MAX_LINE_LEN);
    let mut line_no = 0usize;
    loop {
        buf.clear();
        let got = read_bounded_line(&mut reader, &mut buf, MAX_LINE_LEN)
            .map_err(|k| b.fatal(line_no + 1, k))?;
        if !got {
            break;
        }
        line_no += 1;
        if line_no > MAX_RECORDS {
            return Err(b.fatal(line_no, ErrorKind::TooManyRecords { limit: MAX_RECORDS }));
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        b.line(line_no, &buf)?;
    }
    b.finish(line_no)
}

/// Appends one line (up to and including `\n`) to `buf`. Returns `false` at end of input
/// with nothing read. Fails as soon as the line exceeds `limit` bytes.
fn read_bounded_line<R: BufRead>(
    r: &mut R,
    buf: &mut Vec<u8>,
    limit: usize,
) -> Result<bool, ErrorKind> {
    let mut any = false;
    loop {
        let chunk = match r.fill_buf() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ErrorKind::Io(e.to_string())),
        };
        if chunk.is_empty() {
            return Ok(any);
        }
        any = true;
        let (take, done) = match chunk.iter().position(|&c| c == b'\n') {
            Some(i) => (i + 1, true),
            None => (chunk.len(), false),
        };
        if buf.len().saturating_add(take) > limit {
            return Err(ErrorKind::LineTooLong { limit });
        }
        buf.extend_from_slice(chunk.get(..take).unwrap_or(&[]));
        r.consume(take);
        if done {
            return Ok(true);
        }
    }
}

struct Builder<'s> {
    source: &'s str,
    out: Cifp,
    in_header: bool,
    header_lines: usize,
    data_lines: usize,
    /// Cols 1-38 of the last primary leg, while the previous record was a leg record.
    last_leg_key: Option<String>,
    /// Cols 1-20 and sequence (cols 21-24) of the last airspace record, while the previous
    /// record was an airspace record.
    last_volume: Option<(String, String)>,
}

impl<'s> Builder<'s> {
    fn new(source: &'s str) -> Self {
        Builder {
            source,
            out: Cifp::default(),
            in_header: true,
            header_lines: 0,
            data_lines: 0,
            last_leg_key: None,
            last_volume: None,
        }
    }

    fn fatal(&self, line: usize, kind: ErrorKind) -> Error {
        Error {
            source: self.source.to_string(),
            line,
            kind,
        }
    }

    fn soft(&mut self, line: usize, kind: ErrorKind) {
        if self.out.errors.len() < MAX_RECORD_ERRORS {
            let e = self.fatal(line, kind);
            self.out.errors.push(e);
        } else {
            self.out.errors_dropped += 1;
        }
    }

    fn line(&mut self, line_no: usize, bytes: &[u8]) -> Result<(), Error> {
        let Some(text) = std::str::from_utf8(bytes).ok().filter(|t| t.is_ascii()) else {
            // Not ASCII: report the first offending column, never slice it.
            let column = bytes
                .iter()
                .position(|b| !b.is_ascii())
                .map_or(1, |i| i + 1);
            if self.in_header {
                return Err(self.fatal(line_no, ErrorKind::BadByte { column }));
            }
            self.data_lines += 1;
            self.soft(line_no, ErrorKind::BadByte { column });
            self.reset_context();
            return Ok(());
        };
        if self.in_header {
            if text.starts_with("HDR") {
                return self.header_line(line_no, text);
            }
            if self.header_lines == 0 {
                return Err(self.fatal(line_no, ErrorKind::MissingHeader));
            }
            self.in_header = false;
        } else if text.starts_with("HDR") {
            self.data_lines += 1;
            self.soft(line_no, ErrorKind::HeaderAfterData);
            self.reset_context();
            return Ok(());
        }
        self.data_lines += 1;
        *self.out.counts.entry(section_code(text)).or_insert(0) += 1;
        let record = match parse_record(line_no, text) {
            Ok(r) => r,
            Err(k) => {
                self.soft(line_no, k);
                self.reset_context();
                return Ok(());
            }
        };
        let meta = record_meta(&record);
        *self.out.cycle_counts.entry(meta.cycle).or_insert(0) += 1;
        if let Some(h) = &self.out.header {
            if cycle_is_after(meta.cycle, h.cycle) {
                let kind = ErrorKind::CycleAfterHeader {
                    header: h.cycle.to_string(),
                    record: meta.cycle.to_string(),
                };
                return Err(self.fatal(line_no, kind));
            }
        }
        self.place(line_no, text, record)
            .map_err(|k| self.fatal(line_no, k))
    }

    fn header_line(&mut self, line_no: usize, text: &str) -> Result<(), Error> {
        if self.header_lines >= MAX_HEADER_RECORDS {
            return Err(self.fatal(
                line_no,
                ErrorKind::TooManyHeaderRecords {
                    limit: MAX_HEADER_RECORDS,
                },
            ));
        }
        if text.len() != RECORD_LEN {
            return Err(self.fatal(
                line_no,
                ErrorKind::WrongLength {
                    len: text.len(),
                    expected: RECORD_LEN,
                },
            ));
        }
        self.header_lines += 1;
        if self.header_lines == 1 {
            let h = parse_hdr01(text).map_err(|k| self.fatal(line_no, k))?;
            self.out.header = Some(h);
        } else if let Some(h) = self.out.header.as_mut() {
            h.records.push(text.to_string());
        }
        Ok(())
    }

    fn reset_context(&mut self) {
        self.last_leg_key = None;
        self.last_volume = None;
    }

    fn finish(self, last_line: usize) -> Result<Cifp, Error> {
        let Some(h) = &self.out.header else {
            return Err(self.fatal(last_line, ErrorKind::MissingHeader));
        };
        if h.record_count != self.data_lines {
            let kind = ErrorKind::RecordCountMismatch {
                declared: h.record_count,
                found: self.data_lines,
            };
            return Err(self.fatal(last_line, kind));
        }
        Ok(self.out)
    }

    /// Routes a decoded record into the output, grouping as it goes. `Err` is fatal.
    fn place(&mut self, line_no: usize, text: &str, record: Record) -> Result<(), ErrorKind> {
        let was_leg = matches!(record, Record::Leg(_) | Record::LegContinuation(_));
        let was_airspace = matches!(
            record,
            Record::Airspace(_) | Record::AirspaceContinuation(_)
        );
        if !was_leg {
            self.last_leg_key = None;
        }
        if !was_airspace {
            self.last_volume = None;
        }
        match record {
            Record::Airport(x) => self.out.airports.push(x),
            Record::Runway(x) => self.out.runways.push(x),
            Record::Localizer(x) => self.out.localizers.push(x),
            Record::VhfNavaid(x) => self.out.vhf_navaids.push(x),
            Record::Ndb(x) => self.out.ndbs.push(x),
            Record::TerminalNdb(x) => self.out.terminal_ndbs.push(x),
            Record::Waypoint(x) => match x.kind {
                WaypointKind::Enroute => self.out.enroute_waypoints.push(x),
                WaypointKind::Terminal => self.out.terminal_waypoints.push(x),
            },
            Record::GridMora(x) => self.out.mora.push(x),
            Record::Other { .. } => {}
            Record::AirwayFix(x) => self.place_airway_fix(line_no, x)?,
            Record::Leg(x) => self.place_leg(line_no, text, x)?,
            Record::LegContinuation(c) => self.place_leg_continuation(line_no, text, c)?,
            Record::Airspace(x) => self.place_airspace(line_no, text, x)?,
            Record::AirspaceContinuation(c) => {
                self.place_airspace_continuation(line_no, text, c)?
            }
        }
        Ok(())
    }

    fn place_airway_fix(&mut self, line_no: usize, fix: AirwayFix) -> Result<(), ErrorKind> {
        let same = self.out.airways.last().is_some_and(|a| {
            a.customer_area == fix.meta.customer_area && a.ident == fix.route_ident
        });
        if !same {
            self.out.airways.push(Airway {
                customer_area: fix.meta.customer_area.clone(),
                ident: fix.route_ident.clone(),
                route_type: fix.route_type,
                level: fix.level,
                fixes: Vec::new(),
            });
        }
        let Some(airway) = self.out.airways.last_mut() else {
            return Ok(());
        };
        if let Some(prev) = airway.fixes.last() {
            if fix.sequence <= prev.sequence {
                let kind = ErrorKind::SequenceNotIncreasing {
                    previous: prev.sequence,
                    found: fix.sequence,
                };
                self.soft(line_no, kind);
                return Ok(());
            }
        }
        if airway.fixes.len() >= MAX_AIRWAY_FIXES {
            return Err(ErrorKind::TooManyInGroup {
                what: "airway fixes",
                limit: MAX_AIRWAY_FIXES,
            });
        }
        airway.fixes.push(fix);
        Ok(())
    }

    fn place_leg(&mut self, line_no: usize, text: &str, leg: Leg) -> Result<(), ErrorKind> {
        let same = self.out.procedures.last().is_some_and(|p| {
            p.customer_area == leg.meta.customer_area
                && p.airport_ident == leg.airport_ident
                && p.airport_region == leg.airport_region
                && p.kind == leg.kind
                && p.ident == leg.ident
        });
        if !same {
            self.out.procedures.push(Procedure {
                customer_area: leg.meta.customer_area.clone(),
                airport_ident: leg.airport_ident.clone(),
                airport_region: leg.airport_region.clone(),
                kind: leg.kind,
                ident: leg.ident.clone(),
                routes: Vec::new(),
            });
        }
        let Some(proc) = self.out.procedures.last_mut() else {
            return Ok(());
        };
        let same_route = proc
            .routes
            .last()
            .is_some_and(|r| r.route_type == leg.route_type && r.transition == leg.transition);
        if !same_route {
            if proc.routes.len() >= MAX_ROUTES_PER_PROCEDURE {
                return Err(ErrorKind::TooManyInGroup {
                    what: "routes in one procedure",
                    limit: MAX_ROUTES_PER_PROCEDURE,
                });
            }
            proc.routes.push(Route {
                route_type: leg.route_type,
                transition: leg.transition.clone(),
                legs: Vec::new(),
            });
        }
        let Some(route) = proc.routes.last_mut() else {
            return Ok(());
        };
        if let Some(prev) = route.legs.last() {
            if leg.sequence <= prev.sequence {
                let kind = ErrorKind::SequenceNotIncreasing {
                    previous: prev.sequence,
                    found: leg.sequence,
                };
                self.soft(line_no, kind);
                self.last_leg_key = None;
                return Ok(());
            }
        }
        if route.legs.len() >= MAX_ROUTE_LEGS {
            return Err(ErrorKind::TooManyInGroup {
                what: "legs in one route",
                limit: MAX_ROUTE_LEGS,
            });
        }
        route.legs.push(leg);
        self.last_leg_key = Some(text.get(..38).unwrap_or("").to_string());
        Ok(())
    }

    fn place_leg_continuation(
        &mut self,
        line_no: usize,
        text: &str,
        c: crate::procedure::LegContinuation,
    ) -> Result<(), ErrorKind> {
        let key = text.get(..38).unwrap_or("");
        if self.last_leg_key.as_deref() != Some(key) {
            self.soft(line_no, ErrorKind::OrphanContinuation);
            self.last_leg_key = None;
            return Ok(());
        }
        let leg = self
            .out
            .procedures
            .last_mut()
            .and_then(|p| p.routes.last_mut())
            .and_then(|r| r.legs.last_mut());
        let Some(leg) = leg else {
            self.soft(line_no, ErrorKind::OrphanContinuation);
            return Ok(());
        };
        if leg.continuations.len() >= MAX_CONTINUATIONS_PER_PRIMARY {
            return Err(ErrorKind::TooManyContinuations {
                limit: MAX_CONTINUATIONS_PER_PRIMARY,
            });
        }
        leg.continuations.push(c);
        Ok(())
    }

    fn list(&mut self, kind: AirspaceKind) -> &mut Vec<Airspace> {
        match kind {
            AirspaceKind::Controlled => &mut self.out.controlled_airspace,
            AirspaceKind::SpecialUse => &mut self.out.special_use_airspace,
        }
    }

    fn place_airspace(&mut self, line_no: usize, text: &str, a: Airspace) -> Result<(), ErrorKind> {
        let r = Rec::new(text);
        let vkey = volume_key(&r);
        let same = self.last_volume.as_ref().is_some_and(|(k, _)| *k == vkey);
        if !same {
            let kind = a.kind;
            let seq = r.raw(21, 24).to_string();
            self.list(kind).push(a);
            self.last_volume = Some((vkey, seq));
            return Ok(());
        }
        let kind = a.kind;
        let list = self.list(kind);
        let Some(cur) = list.last_mut() else {
            return Ok(());
        };
        let Some(point) = a.boundary.into_iter().next() else {
            return Ok(());
        };
        let prev_seq = cur.boundary.last().map_or(0, |p| p.sequence);
        if point.sequence <= prev_seq {
            let err = ErrorKind::SequenceNotIncreasing {
                previous: prev_seq,
                found: point.sequence,
            };
            self.soft(line_no, err);
            // Keep the volume open but make sure a continuation of the dropped record is an orphan.
            self.last_volume = Some((vkey, String::new()));
            return Ok(());
        }
        if cur.boundary.len() >= MAX_BOUNDARY_POINTS {
            return Err(ErrorKind::TooManyInGroup {
                what: "boundary points",
                limit: MAX_BOUNDARY_POINTS,
            });
        }
        if cur.lower.is_none() {
            cur.lower = a.lower;
        }
        if cur.upper.is_none() {
            cur.upper = a.upper;
        }
        if cur.name.is_empty() {
            cur.name = a.name;
        }
        cur.boundary.push(point);
        self.last_volume = Some((vkey, r.raw(21, 24).to_string()));
        Ok(())
    }

    fn place_airspace_continuation(
        &mut self,
        line_no: usize,
        text: &str,
        c: crate::airspace::AirspaceContinuation,
    ) -> Result<(), ErrorKind> {
        let r = Rec::new(text);
        let matches = self
            .last_volume
            .as_ref()
            .is_some_and(|(k, seq)| *k == volume_key(&r) && seq == r.raw(21, 24));
        if !matches {
            self.soft(line_no, ErrorKind::OrphanContinuation);
            self.last_volume = None;
            return Ok(());
        }
        let list = self.list(AirspaceKind::SpecialUse);
        let Some(cur) = list.last_mut() else {
            return Ok(());
        };
        if cur.continuations.len() >= MAX_CONTINUATIONS_PER_PRIMARY {
            return Err(ErrorKind::TooManyContinuations {
                limit: MAX_CONTINUATIONS_PER_PRIMARY,
            });
        }
        cur.continuations.push(c);
        Ok(())
    }
}

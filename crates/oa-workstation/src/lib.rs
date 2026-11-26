#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! Workstation input and flight-data interaction, core (pure, RULESET B1).
//! This is an experimental OpenATCA interaction contract, not a certified ERAM/STARS emulator.
//! Shells supply text, stable flight keys, actor and operation stamps. No clock or I/O lives here.
//! Every edit is bounded; assigning altitude edits flight data and never sends pilot instructions.

#[cfg(not(any(
    target_pointer_width = "64",
    all(
        target_arch = "wasm32",
        target_pointer_width = "32",
        target_os = "unknown",
        target_vendor = "unknown"
    )
)))]
compile_error!(
    "this crate supports 64-bit targets and the audited wasm32-unknown-unknown target only"
);

use oa_flight::{
    flight_get, flights_apply, op_new, Alt, FlightKey, FlightTable, Outcome, ALT_NIL, ALT_SIMPLE,
    OP_ASSIGN, OP_HO_ACCEPT, OP_HO_INITIATE, OP_HO_RETRACT, OP_HO_TAKE, OP_INTERIM,
};

pub const MAX_ENTRY: usize = 96;
pub const MAX_BLOCKS: usize = 32;
pub const R_SIDE: u8 = 0;
pub const D_SIDE: u8 = 1;
pub const TERMINAL: u8 = 2;

// A7.2: one transient parsed command, not stored in quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Readout,
    Route,
    Assigned,
    Interim,
    Annotation,
    Offer,
    Accept,
    Control,
    Retract,
    Voice,
    Offset,
    PointOut,
    Acknowledge,
    Range,
    Filter,
    History,
    Vector,
}

/// Whether a parsed prototype command may mutate or query the workstation in the default demo.
///
/// No command is enabled until its exact input, operation, and user-visible outcome are reviewed
/// against current system documentation and recorded in the demo fidelity registry. Historical
/// task-frequency evidence and community emulator syntax do not qualify by themselves.
pub const fn default_command_available(_kind: Kind) -> bool {
    false
}

/// The disposition of one entry in the OpenATCA prototype interaction contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryOutcome {
    Applied,
    Unchanged,
    Refused,
}

/// Source classification attached to the *related task row*. The row does not verify syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceConfidence {
    PrimaryHistoricalTask,
    Community,
    CommunitySyntaxExample,
    OpenAtcaOnly,
    InferredOpenAtcaOnly,
}

/// Provenance for an OpenATCA prototype entry. Each row has its own confidence. The row IDs are
/// related task/context evidence; the grammar and response remain OpenATCA differences and must
/// never be rendered as ERAM/STARS wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: one cold provenance tuple returned with a submitted entry.
pub struct EntryFidelity {
    pub task_row_id: Option<&'static str>,
    pub task_confidence: EvidenceConfidence,
    pub context_row_id: Option<&'static str>,
    pub context_confidence: EvidenceConfidence,
    pub syntax_is_openatca_difference: bool,
    pub response_row_id: Option<&'static str>,
    pub response_confidence: EvidenceConfidence,
    pub response_is_openatca_difference: bool,
}

/// A submitted prototype entry and a status that contains no response text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: one cold response record per submitted entry.
pub struct EntryReceipt {
    pub kind: Option<Kind>,
    pub outcome: EntryOutcome,
    pub fidelity: EntryFidelity,
}

/// A parsed command, no borrowed input or heap allocation (C3).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub target: [u8; 8],
    pub text: [u8; 16],
    pub value: i32,
    pub other: i32,
    pub kind: Kind,
}
// A7.2: transient cold command; enum representation is not used as a wire format.

pub fn identifier(s: &str) -> Result<[u8; 8], &'static str> {
    if s.is_empty() || s.len() > 8 || !s.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return Err("INVALID FLIGHT OR SECTOR IDENTIFIER");
    }
    let mut b = [0; 8];
    for (to, from) in b.iter_mut().zip(s.bytes()) {
        *to = from.to_ascii_uppercase();
    }
    Ok(b)
}

fn number(s: &str, lo: i32, hi: i32) -> Result<i32, &'static str> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err("INVALID NUMBER");
    }
    let n = s.parse::<i32>().map_err(|_| "INVALID NUMBER")?;
    if !(lo..=hi).contains(&n) {
        return Err("VALUE OUT OF RANGE");
    }
    Ok(n)
}

/// Supported grammar is documented in README. Unknown and combined commands are refused.
pub fn parse(input: &str) -> Result<Command, &'static str> {
    if input.len() > MAX_ENTRY || !input.is_ascii() {
        return Err("ENTRY MUST BE ASCII, AT MOST 96 CHARACTERS");
    }
    let mut upper = [0; MAX_ENTRY];
    for (to, from) in upper.iter_mut().zip(input.bytes()) {
        *to = from.to_ascii_uppercase();
    }
    let value = core::str::from_utf8(upper.get(..input.len()).ok_or("ENTRY TOO LONG")?)
        .map_err(|_| "INVALID ENTRY")?;
    let mut words = value.split_ascii_whitespace();
    let verb = words.next().ok_or("NO ENTRY")?;
    let a = words.next();
    let b = words.next();
    let c = words.next();
    if c.is_some() || words.next().is_some() {
        return Err("TOO MANY FIELDS");
    }
    let mut out = Command {
        target: [0; 8],
        text: [0; 16],
        value: 0,
        other: 0,
        kind: Kind::Readout,
    };
    match (verb, a, b) {
        ("QF" | "PLAN", Some(id), None) => {
            out.kind = Kind::Readout;
            out.target = identifier(id)?;
        }
        ("QU" | "ROUTE", Some(id), None) => {
            out.kind = Kind::Route;
            out.target = identifier(id)?;
        }
        ("QZ" | "ALT", Some(n), Some(id)) => {
            out.kind = Kind::Assigned;
            out.value = number(n, 10, 450)? * 100;
            out.target = identifier(id)?;
        }
        ("QQ" | "TEMP", Some(n), Some(id)) => {
            out.kind = Kind::Interim;
            out.value = number(n, 0, 450)? * 100;
            out.target = identifier(id)?;
        }
        ("QS" | "SP", Some(s), Some(id)) => {
            if s.len() > 16 {
                return Err("ANNOTATION EXCEEDS 16 CHARACTERS");
            }
            out.kind = Kind::Annotation;
            out.target = identifier(id)?;
            if s != "*" {
                for (to, from) in out.text.iter_mut().zip(s.bytes()) {
                    *to = from;
                }
            }
        }
        ("HO" | "QN", Some(sector), Some(id)) => {
            out.kind = Kind::Offer;
            out.target = identifier(id)?;
            let unit = identifier(sector)?;
            for (to, from) in out.text.iter_mut().zip(unit) {
                *to = from;
            }
        }
        ("ACCEPT" | "QN", Some(id), None) => {
            out.kind = Kind::Accept;
            out.target = identifier(id)?;
        }
        ("CONTROL", Some(id), None) => {
            out.kind = Kind::Control;
            out.target = identifier(id)?;
        }
        ("CANCEL", Some(id), None) => {
            out.kind = Kind::Retract;
            out.target = identifier(id)?;
        }
        ("VCI", Some(id), None) => {
            out.kind = Kind::Voice;
            out.target = identifier(id)?;
        }
        ("DB", Some(n), Some(id)) => {
            out.kind = Kind::Offset;
            out.value = number(n, 1, 9)?;
            out.target = identifier(id)?;
        }
        ("PO", Some(sector), Some(id)) => {
            out.kind = Kind::PointOut;
            out.target = identifier(id)?;
            for (to, from) in out.text.iter_mut().zip(identifier(sector)?) {
                *to = from;
            }
        }
        ("ACKPO", Some(id), None) => {
            out.kind = Kind::Acknowledge;
            out.target = identifier(id)?;
        }
        ("RANGE", Some(n), None) => {
            out.kind = Kind::Range;
            out.value = number(n, 5, 120)?;
        }
        ("FILTER", Some(lo), Some(hi)) => {
            out.kind = Kind::Filter;
            out.value = number(lo, 0, 600)? * 100;
            out.other = number(hi, 0, 600)? * 100;
            if out.value > out.other {
                return Err("FILTER FLOOR ABOVE CEILING");
            }
        }
        ("HIST", Some(n), None) => {
            out.kind = Kind::History;
            out.value = number(n, 0, 16)?;
        }
        ("VECTOR", Some(n), None) => {
            out.kind = Kind::Vector;
            out.value = number(n, 0, 8)?;
        }
        _ => return Err("UNSUPPORTED ENTRY"),
    }
    Ok(out)
}

fn entry_fidelity(kind: Kind) -> EntryFidelity {
    let (task_row_id, context_row_id, context_confidence) = match kind {
        Kind::Readout => (
            Some("ERAM-CMD-10"),
            Some("ERAM-R-18"),
            EvidenceConfidence::CommunitySyntaxExample,
        ),
        Kind::Route => (
            Some("ERAM-CMD-06"),
            Some("ERAM-R-18"),
            EvidenceConfidence::CommunitySyntaxExample,
        ),
        Kind::Assigned => (
            Some("ERAM-CMD-17"),
            Some("ERAM-R-17"),
            EvidenceConfidence::PrimaryHistoricalTask,
        ),
        Kind::Interim => (
            Some("ERAM-CMD-04"),
            Some("ERAM-R-17"),
            EvidenceConfidence::PrimaryHistoricalTask,
        ),
        Kind::Annotation => (
            Some("ERAM-CMD-18"),
            Some("ERAM-R-17"),
            EvidenceConfidence::PrimaryHistoricalTask,
        ),
        Kind::Offer => (
            Some("ERAM-CMD-03"),
            Some("ERAM-R-12"),
            EvidenceConfidence::PrimaryHistoricalTask,
        ),
        Kind::Accept | Kind::Control => (
            Some("ERAM-CMD-02"),
            Some("ERAM-R-12"),
            EvidenceConfidence::PrimaryHistoricalTask,
        ),
        Kind::Retract => (
            None,
            Some("ERAM-R-12"),
            EvidenceConfidence::PrimaryHistoricalTask,
        ),
        Kind::Voice => (None, None, EvidenceConfidence::InferredOpenAtcaOnly),
        Kind::Offset => (
            Some("ERAM-CMD-01"),
            None,
            EvidenceConfidence::PrimaryHistoricalTask,
        ),
        Kind::PointOut | Kind::Acknowledge => (
            Some("ERAM-CMD-19"),
            Some("ERAM-R-12"),
            EvidenceConfidence::PrimaryHistoricalTask,
        ),
        Kind::Range => (
            Some("ERAM-CMD-14"),
            None,
            EvidenceConfidence::PrimaryHistoricalTask,
        ),
        // The old frequency study's combined-toggle filter is not equivalent to this prototype's
        // altitude floor/ceiling preference, so it has no corresponding task row.
        Kind::Filter => (None, None, EvidenceConfidence::InferredOpenAtcaOnly),
        Kind::History => (None, None, EvidenceConfidence::InferredOpenAtcaOnly),
        Kind::Vector => (
            Some("ERAM-CMD-05"),
            Some("ERAM-R-34"),
            EvidenceConfidence::CommunitySyntaxExample,
        ),
    };
    EntryFidelity {
        task_row_id,
        task_confidence: if task_row_id.is_some() {
            EvidenceConfidence::PrimaryHistoricalTask
        } else {
            EvidenceConfidence::InferredOpenAtcaOnly
        },
        context_row_id,
        context_confidence,
        syntax_is_openatca_difference: true,
        response_row_id: None,
        response_confidence: EvidenceConfidence::OpenAtcaOnly,
        response_is_openatca_difference: true,
    }
}

fn refused_entry_fidelity() -> EntryFidelity {
    EntryFidelity {
        task_row_id: None,
        task_confidence: EvidenceConfidence::InferredOpenAtcaOnly,
        context_row_id: None,
        context_confidence: EvidenceConfidence::InferredOpenAtcaOnly,
        syntax_is_openatca_difference: true,
        response_row_id: None,
        response_confidence: EvidenceConfidence::OpenAtcaOnly,
        response_is_openatca_difference: true,
    }
}

/// Parses and applies one OpenATCA prototype entry while returning only a typed status.
/// Prototype grammar and response strings are deliberately not returned as controller-facing
/// text. The accompanying row references provide context, not proof of syntax parity.
pub fn execute_prototype_entry(
    input: &str,
    position: &mut Position,
    table: &mut FlightTable,
    coordination: &mut Coordination,
    context: &Context,
) -> EntryReceipt {
    let Ok(command) = parse(input) else {
        return EntryReceipt {
            kind: None,
            outcome: EntryOutcome::Refused,
            fidelity: refused_entry_fidelity(),
        };
    };
    let fidelity = entry_fidelity(command.kind);
    if !default_command_available(command.kind) {
        return EntryReceipt {
            kind: Some(command.kind),
            outcome: EntryOutcome::Refused,
            fidelity,
        };
    }
    let outcome = match execute(position, table, coordination, &command, context) {
        Ok("NO CHANGE") => EntryOutcome::Unchanged,
        Ok(_) => EntryOutcome::Applied,
        Err(_) => EntryOutcome::Refused,
    };
    EntryReceipt {
        kind: Some(command.kind),
        outcome,
        fidelity,
    }
}

// A3 exception: at most 32 tiny display-preference records per position, never a traffic table.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub id: [u8; 8],
    pub offset: u8,
    pub route: u8,
    pub manual_offset: u8,
    pub reserved: u8,
}
const _: () = assert!(core::mem::size_of::<Block>() == 12);
pub const EMPTY_BLOCK: Block = Block {
    id: [0; 8],
    offset: 9,
    route: 0,
    manual_offset: 0,
    reserved: 0,
};

// A7.2: one bounded preference state per controller position, cold configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub sector: [u8; 8],
    pub role: u8,
    pub range_nm: u16,
    pub floor_ft: i32,
    pub ceiling_ft: i32,
    pub history: u8,
    pub vector_min: u8,
    pub blocks: [Block; MAX_BLOCKS],
}
pub fn position_new(sector: [u8; 8], role: u8, range_nm: u16) -> Result<Position, &'static str> {
    if role > TERMINAL || !(5..=120).contains(&range_nm) {
        return Err("INVALID POSITION CONFIGURATION");
    }
    Ok(Position {
        sector,
        role,
        range_nm,
        floor_ft: 0,
        ceiling_ft: 60000,
        history: 5,
        vector_min: 1,
        blocks: [EMPTY_BLOCK; MAX_BLOCKS],
    })
}
pub fn block_of(position: &Position, id: &[u8; 8]) -> Block {
    position
        .blocks
        .iter()
        .find(|b| &b.id == id)
        .copied()
        .unwrap_or(EMPTY_BLOCK)
}
fn block_mut<'a>(position: &'a mut Position, id: &[u8; 8]) -> Result<&'a mut Block, &'static str> {
    let slot = position
        .blocks
        .iter()
        .position(|b| &b.id == id)
        .or_else(|| position.blocks.iter().position(|b| b.id == [0; 8]))
        .ok_or("POSITION DISPLAY CAPACITY REACHED")?;
    let b = position
        .blocks
        .get_mut(slot)
        .ok_or("DISPLAY SLOT MISSING")?;
    b.id = *id;
    Ok(b)
}

/// Shared coordination state, keyed by stable callsign. A3 exception: 32 small records maximum.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coordination {
    pub id: [u8; 8],
    pub from: [u8; 8],
    pub to: [u8; 8],
    pub annotation: [u8; 16],
    pub voice: u8,
    pub acknowledged: u8,
    pub reserved: [u8; 2],
}
const _: () = assert!(core::mem::size_of::<Coordination>() == 44);
pub const EMPTY_COORDINATION: Coordination = Coordination {
    id: [0; 8],
    from: [0; 8],
    to: [0; 8],
    annotation: [0; 16],
    voice: 0,
    acknowledged: 0,
    reserved: [0; 2],
};

// A7.2: one transient operation context supplied by the shell.
#[derive(Clone, Copy)]
pub struct Context {
    pub key: FlightKey,
    pub centre: [u8; 4],
    pub at_ms: i64,
    pub message: u64,
}

/// Executes one parsed OpenATCA prototype command against shared flight data. The returned text
/// is prototype-only and must not be shown or described as an ERAM/STARS response; controller-
/// facing adapters should use [`execute_prototype_entry`] and its typed status instead.
pub fn execute(
    position: &mut Position,
    table: &mut FlightTable,
    coordination: &mut Coordination,
    command: &Command,
    context: &Context,
) -> Result<&'static str, &'static str> {
    let valid = match command.kind {
        Kind::Range => (5..=120).contains(&command.value),
        Kind::Filter => {
            (0..=60000).contains(&command.value) && (command.value..=60000).contains(&command.other)
        }
        Kind::History => (0..=16).contains(&command.value),
        Kind::Vector => (0..=8).contains(&command.value),
        Kind::Offset => (1..=9).contains(&command.value),
        Kind::Assigned => (1000..=45000).contains(&command.value) && command.value % 100 == 0,
        Kind::Interim => (0..=45000).contains(&command.value) && command.value % 100 == 0,
        _ => true,
    };
    if !valid {
        return Err("VALUE OUT OF RANGE");
    }
    match command.kind {
        Kind::Range => {
            position.range_nm = u16::try_from(command.value).map_err(|_| "BAD RANGE")?;
            return Ok("RANGE UPDATED");
        }
        Kind::Filter => {
            position.floor_ft = command.value;
            position.ceiling_ft = command.other;
            return Ok("ALTITUDE FILTER UPDATED");
        }
        Kind::History => {
            position.history = u8::try_from(command.value).map_err(|_| "BAD HISTORY")?;
            return Ok("HISTORY UPDATED");
        }
        Kind::Vector => {
            position.vector_min = u8::try_from(command.value).map_err(|_| "BAD VECTOR")?;
            return Ok("VECTOR UPDATED");
        }
        _ => {}
    }
    let f = flight_get(table, &context.key).ok_or("NO SUCH FLIGHT")?;
    if f.callsign != command.target || coordination.id != command.target {
        return Err("FLIGHT IDENTITY DOES NOT MATCH COMMAND");
    }
    let ours = f.owner == position.sector;
    if command.kind == Kind::Readout {
        return Ok("FLIGHT PLAN READOUT");
    }
    if command.kind == Kind::Route {
        let b = block_mut(position, &command.target)?;
        b.route ^= 1;
        return Ok("ROUTE DISPLAY TOGGLED");
    }
    if command.kind == Kind::Offset {
        let block = block_mut(position, &command.target)?;
        block.offset = u8::try_from(command.value).map_err(|_| "BAD OFFSET")?;
        block.manual_offset = 1;
        return Ok("DATA BLOCK REPOSITIONED");
    }
    if command.kind == Kind::Acknowledge {
        if coordination.to != position.sector || coordination.from == [0; 8] {
            return Err("NO POINT-OUT TO THIS POSITION");
        }
        coordination.acknowledged = 1;
        return Ok("POINT-OUT ACKNOWLEDGED");
    }
    if matches!(command.kind, Kind::Accept | Kind::Control) {
        if f.ho_to != position.sector {
            return Err("NO HANDOFF TO THIS POSITION");
        }
    } else if !ours {
        return Err("FLIGHT NOT OWNED BY THIS POSITION");
    }
    if command.kind == Kind::Annotation {
        coordination.annotation = command.text;
        return Ok("ANNOTATION UPDATED");
    }
    if command.kind == Kind::Voice {
        if position.role == D_SIDE {
            return Err("VOICE STATUS REQUIRES THE RADAR POSITION");
        }
        coordination.voice ^= 1;
        return Ok("VOICE STATUS UPDATED");
    }
    if command.kind == Kind::PointOut {
        let mut to = [0; 8];
        for (a, b) in to.iter_mut().zip(command.text) {
            *a = b;
        }
        if to == position.sector || to == [0; 8] {
            return Err("INVALID POINT-OUT RECIPIENT");
        }
        coordination.id = command.target;
        coordination.from = position.sector;
        coordination.to = to;
        coordination.acknowledged = 0;
        return Ok("POINT-OUT SENT");
    }
    let kind = match command.kind {
        Kind::Assigned => OP_ASSIGN,
        Kind::Interim => OP_INTERIM,
        Kind::Offer => OP_HO_INITIATE,
        Kind::Accept => OP_HO_ACCEPT,
        Kind::Control => OP_HO_TAKE,
        Kind::Retract => OP_HO_RETRACT,
        _ => return Err("UNSUPPORTED OPERATION"),
    };
    let mut op = op_new(
        kind,
        context.key,
        context.centre,
        context.at_ms,
        context.at_ms,
        context.message,
    );
    if matches!(command.kind, Kind::Assigned | Kind::Interim) {
        op.alt = Alt {
            form: if command.value == 0 {
                ALT_NIL
            } else {
                ALT_SIMPLE
            },
            a_ft: command.value,
            b_ft: 0,
        };
    } else if command.kind == Kind::Offer {
        op.unit_a = f.owner;
        for (to, from) in op.unit_b.iter_mut().zip(command.text) {
            *to = from;
        }
        if op.unit_b == f.owner {
            return Err("CANNOT HAND OFF TO SAME POSITION");
        }
    } else {
        op.unit_a = f.ho_from;
        op.unit_b = f.ho_to;
    }
    match flights_apply(table, &op) {
        Outcome::Applied => {
            if command.kind == Kind::Control {
                coordination.voice = 0;
            }
            Ok(match command.kind {
                Kind::Assigned => "ASSIGNED ALTITUDE UPDATED",
                Kind::Interim => "INTERIM ALTITUDE UPDATED",
                Kind::Offer => "HANDOFF INITIATED",
                Kind::Accept => "HANDOFF ACCEPTED",
                Kind::Control => "CONTROL TRANSFERRED",
                Kind::Retract => "HANDOFF RETRACTED",
                _ => "ENTRY COMPLETE",
            })
        }
        Outcome::Unchanged => Ok("NO CHANGE"),
        Outcome::Refused(_) => Err("FLIGHT CORE REFUSED OPERATION - STATE UNCHANGED"),
    }
}

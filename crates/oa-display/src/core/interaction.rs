//! Controller interaction state (**core**, B1/B2): operator selection and a bounded queue of
//! typed intents. This module does not change a flight or call the flight subsystem. The caller
//! supplies the current target view on every operation, so stale or ambiguous evidence is refused.

use crate::core::font::{text_width_units, CAP_HEIGHT};
use crate::core::list::{push_text, DisplayError, DisplayList, ALIGN_LEFT};
use crate::core::palette;
use crate::core::project::Projection;
use crate::core::text::{text_new, text_push_hex, text_push_str, text_str};

/// Maximum current target rows inspected by one interaction operation (C7).
pub const MAX_TARGET_VIEWS: usize = 4096;
/// Number of intents the display can hold before the caller drains them (C2).
pub const COMMAND_CAPACITY: usize = 32;
/// Layer above the legend and traffic, below the safety banner.
pub const LAYER_INTERACTION: u8 = 13;
const TEXT_SIZE: u8 = 12;
const TEXT_MARGIN_PX: i64 = 8;
const TEXT_TOP_PAD_PX: i64 = 8;
const TEXT_PITCH_PX: i64 = 14;
const BOTTOM_NOTICE_RESERVE_PX: i64 = 42;
const MAX_OCCLUDER_RECTANGLES: usize = 4096;

/// Full flight identity supplied by the regional runner (E7). `gufi` is in network byte order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct FlightIdentity {
    pub gufi: [u8; 16],
    pub aircraft_address: u32,
    pub centre: [u8; 4],
    pub plan_id: [u8; 12],
}
const _: () = assert!(core::mem::size_of::<FlightIdentity>() == 36);

/// One row in the current regional-runner view. `identity_claims` is the count of flight records
/// claiming this full address in the current frame; values above one are an explicit ambiguity.
// A3 exception: this small plain row is a caller-owned read-only boundary view, capped at 4,096
// rows per operation. InteractionState retains no view rows; its owned hot queue is SoA below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct TargetView {
    pub address: u32,
    pub generation: u32,
    pub identity_claims: u16,
    /// 0 means the runner has no bound flight identity; 1 means `identity` is supplied.
    pub identity_present: u8,
    /// Must be zero, reserved for a future version.
    pub reserved: u8,
    pub identity: FlightIdentity,
}
const _: () = assert!(core::mem::size_of::<TargetView>() == 48);

/// Selection is either a display-only track or one positively bound full flight identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SelectionKind {
    None = 0,
    RawTrack = 1,
    BoundFlight = 2,
}

/// Typed operator request; it is preserved verbatim in the queue for the approved flight bridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C, u8)]
pub enum Intent {
    SetAssignedAltitude(i32),
    ClearAssignedAltitude,
    SetInterimAltitude(i32),
    ClearInterimAltitude,
    OfferHandoff(u32),
    AcceptHandoff(u32),
    TakeHandoff(u32),
    CancelHandoff(u32),
    /// Acknowledge one MSAW aural request by its alert episode start time.
    AcknowledgeMsaw {
        alert_start_ns: i64,
    },
    /// Enable MSAW service for the selected full flight identity until this receipt timestamp.
    /// Zero disables the existing request for the selected identity.
    RequestMsawService {
        expires_at_ns: i64,
    },
}
const _: () = assert!(core::mem::size_of::<Intent>() == 16);

/// Named operator and evidence failures. Rejections are returned synchronously and retained in
/// the status shown for the selected target (E5/E6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Refusal {
    NoSelection = 1,
    RawTrackOnly = 2,
    ViewTooLarge = 3,
    StaleView = 4,
    TrackMissing = 5,
    DuplicateAddress = 6,
    IdentityAmbiguous = 7,
    IdentityMismatch = 8,
    StaleGeneration = 9,
    BackwardsSequence = 10,
    QueueFull = 11,
    CommandSequenceMismatch = 12,
    MalformedView = 13,
    NoPendingCommand = 14,
    InvalidAddress = 15,
    BackwardsSourceTime = 16,
}

/// Result of selection or enqueue; refusals are data, not hidden errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractionResult {
    Applied,
    Refused(Refusal),
}

/// Outcome recorded after a dequeued intent is handled by the flight bridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CommandOutcome {
    None = 0,
    Pending = 1,
    Accepted = 2,
    Refused = 3,
    Stale = 4,
}

/// Queue row returned to the flight bridge. Plain data; identity and source evidence are captured
/// at enqueue, so later address reuse can never retarget it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct QueuedIntent {
    pub identity: FlightIdentity,
    pub source_view_sequence: u32,
    pub input_sequence: u32,
    pub target_generation: u32,
    /// Issue-time monotonic timestamp supplied by the regional runner (microseconds).
    pub source_time_micros: u64,
    pub intent: Intent,
}
const _: () = assert!(core::mem::size_of::<QueuedIntent>() == 72);

/// Fixed controller interaction state. The queue is 32 rows (small enough that fixed-array SoA
/// columns are clear and bounded); each column's exact per-row size is asserted at definition.
#[derive(Clone, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct InteractionState {
    selected_address: u32,
    selected_generation: u32,
    selected_view_sequence: u32,
    selected_identity: Option<FlightIdentity>,
    selection_kind: SelectionKind,
    last_view_sequence: u32,
    has_view_sequence: bool,
    last_input_sequence: u32,
    has_input_sequence: bool,
    last_source_time_micros: u64,
    last_outcome: CommandOutcome,
    last_refusal: u8,
    last_completed_sequence: u32,
    queue_identity: [FlightIdentity; COMMAND_CAPACITY],
    queue_view_sequence: [u32; COMMAND_CAPACITY],
    queue_input_sequence: [u32; COMMAND_CAPACITY],
    queue_generation: [u32; COMMAND_CAPACITY],
    queue_time_micros: [u64; COMMAND_CAPACITY],
    queue_intent: [Intent; COMMAND_CAPACITY],
    queue_len: u8,
}
// A7.1: bounded queue includes the accepted-command clock anchor and typed alert ACK identity.
const _: () = assert!(core::mem::size_of::<InteractionState>() == 2400);
const _: () =
    assert!(core::mem::size_of::<[FlightIdentity; COMMAND_CAPACITY]>() == 36 * COMMAND_CAPACITY);
const _: () = assert!(core::mem::size_of::<[Intent; COMMAND_CAPACITY]>() == 16 * COMMAND_CAPACITY);

/// Empty operator state. Construction is fixed-size and performs no allocation.
pub const fn interaction_new() -> InteractionState {
    InteractionState {
        selected_address: 0,
        selected_generation: 0,
        selected_view_sequence: 0,
        selected_identity: None,
        selection_kind: SelectionKind::None,
        last_view_sequence: 0,
        has_view_sequence: false,
        last_input_sequence: 0,
        has_input_sequence: false,
        last_source_time_micros: 0,
        last_outcome: CommandOutcome::None,
        last_refusal: 0,
        last_completed_sequence: 0,
        queue_identity: [FlightIdentity {
            gufi: [0; 16],
            aircraft_address: 0,
            centre: [0; 4],
            plan_id: [0; 12],
        }; COMMAND_CAPACITY],
        queue_view_sequence: [0; COMMAND_CAPACITY],
        queue_input_sequence: [0; COMMAND_CAPACITY],
        queue_generation: [0; COMMAND_CAPACITY],
        queue_time_micros: [0; COMMAND_CAPACITY],
        queue_intent: [Intent::ClearAssignedAltitude; COMMAND_CAPACITY],
        queue_len: 0,
    }
}

/// Selection kind; `None` means no target is selected.
pub fn selection_kind(state: &InteractionState) -> SelectionKind {
    state.selection_kind
}

/// Selected address, when any.
pub fn selected_address(state: &InteractionState) -> Option<u32> {
    if state.selection_kind == SelectionKind::None {
        None
    } else {
        Some(state.selected_address)
    }
}

/// Selected full identity, only for a bound flight.
pub fn selected_identity(state: &InteractionState) -> Option<FlightIdentity> {
    state.selected_identity
}

/// Number of pending commands.
pub fn pending_count(state: &InteractionState) -> usize {
    usize::from(state.queue_len)
}

/// Current pending/refusal status for the presentation overlay.
pub fn command_outcome(state: &InteractionState) -> CommandOutcome {
    state.last_outcome
}

/// Input sequence of the most recently completed command, or zero before any completion.
pub fn last_completed_sequence(state: &InteractionState) -> u32 {
    state.last_completed_sequence
}

/// Last refusal reason, if an operation was refused.
pub fn last_refusal(state: &InteractionState) -> Option<Refusal> {
    match state.last_refusal {
        1 => Some(Refusal::NoSelection),
        2 => Some(Refusal::RawTrackOnly),
        3 => Some(Refusal::ViewTooLarge),
        4 => Some(Refusal::StaleView),
        5 => Some(Refusal::TrackMissing),
        6 => Some(Refusal::DuplicateAddress),
        7 => Some(Refusal::IdentityAmbiguous),
        8 => Some(Refusal::IdentityMismatch),
        9 => Some(Refusal::StaleGeneration),
        10 => Some(Refusal::BackwardsSequence),
        11 => Some(Refusal::QueueFull),
        12 => Some(Refusal::CommandSequenceMismatch),
        13 => Some(Refusal::MalformedView),
        14 => Some(Refusal::NoPendingCommand),
        15 => Some(Refusal::InvalidAddress),
        16 => Some(Refusal::BackwardsSourceTime),
        _ => None,
    }
}

fn refuse(state: &mut InteractionState, reason: Refusal) -> InteractionResult {
    state.last_refusal = reason as u8;
    state.last_outcome = CommandOutcome::Refused;
    InteractionResult::Refused(reason)
}

fn locate(views: &[TargetView], address: u32) -> Result<(usize, TargetView), Refusal> {
    if views.len() > MAX_TARGET_VIEWS {
        return Err(Refusal::ViewTooLarge);
    }
    let mut found = None;
    for (i, view) in views.iter().enumerate() {
        if view.reserved != 0 || view.identity_present > 1 {
            return Err(Refusal::MalformedView);
        }
        if view.address == 0 {
            return Err(Refusal::InvalidAddress);
        }
        if view.address == address {
            if found.is_some() {
                return Err(Refusal::DuplicateAddress);
            }
            found = Some((i, *view));
        }
    }
    found.ok_or(Refusal::TrackMissing)
}

fn check_view_sequence(state: &InteractionState, view_sequence: u32) -> Result<(), Refusal> {
    if view_sequence == 0 || (state.has_view_sequence && view_sequence < state.last_view_sequence) {
        Err(Refusal::StaleView)
    } else {
        Ok(())
    }
}

/// Select one currently visible address. With `identity=None`, the result is raw-track-only.
/// Binding requires one unambiguous matching full identity and generation.
pub fn select_target(
    state: &mut InteractionState,
    views: &[TargetView],
    view_sequence: u32,
    address: u32,
    expected_generation: u32,
    identity: Option<FlightIdentity>,
) -> InteractionResult {
    if address == 0 {
        return refuse(state, Refusal::InvalidAddress);
    }
    if let Err(reason) = check_view_sequence(state, view_sequence) {
        return refuse(state, reason);
    }
    let (_, view) = match locate(views, address) {
        Ok(found) => found,
        Err(reason) => return refuse(state, reason),
    };
    if view.generation != expected_generation {
        return refuse(state, Refusal::StaleGeneration);
    }
    if let Some(key) = identity {
        if key.aircraft_address != address || view.identity_present != 1 || view.identity != key {
            return refuse(state, Refusal::IdentityMismatch);
        }
        if view.identity_claims != 1 {
            return refuse(state, Refusal::IdentityAmbiguous);
        }
        state.selected_identity = Some(key);
        state.selection_kind = SelectionKind::BoundFlight;
    } else {
        state.selected_identity = None;
        state.selection_kind = SelectionKind::RawTrack;
    }
    state.selected_address = address;
    state.selected_generation = expected_generation;
    state.selected_view_sequence = view_sequence;
    state.last_view_sequence = view_sequence;
    state.has_view_sequence = true;
    state.last_refusal = 0;
    state.last_outcome = CommandOutcome::None;
    InteractionResult::Applied
}

/// Clear operator selection. Pending commands retain their captured key and are unaffected.
pub fn clear_selection(state: &mut InteractionState) {
    state.selection_kind = SelectionKind::None;
    state.selected_identity = None;
}

/// Queue one command for the selected, still-present bound flight. Input sequences strictly
/// increase across accepted commands; a refusal does not consume the sequence, so a caller may
/// retry that request after correcting stale evidence or draining the queue. Accepted issue times
/// are nondecreasing; timestamps are caller-supplied audit evidence, not authority. Every view
/// inspection is capped by MAX_TARGET_VIEWS before its loop (C7).
pub fn enqueue_intent(
    state: &mut InteractionState,
    views: &[TargetView],
    view_sequence: u32,
    input_sequence: u32,
    source_time_micros: u64,
    intent: Intent,
) -> InteractionResult {
    if state.selection_kind == SelectionKind::None {
        return refuse(state, Refusal::NoSelection);
    }
    if state.selection_kind != SelectionKind::BoundFlight {
        return refuse(state, Refusal::RawTrackOnly);
    }
    if state.has_input_sequence && input_sequence <= state.last_input_sequence {
        return refuse(state, Refusal::BackwardsSequence);
    }
    if state.has_input_sequence && source_time_micros < state.last_source_time_micros {
        return refuse(state, Refusal::BackwardsSourceTime);
    }
    if let Err(reason) = check_view_sequence(state, view_sequence) {
        return refuse(state, reason);
    }
    if view_sequence < state.selected_view_sequence {
        return refuse(state, Refusal::StaleView);
    }
    let identity = match state.selected_identity {
        Some(key) => key,
        None => return refuse(state, Refusal::RawTrackOnly),
    };
    let (_, view) = match locate(views, state.selected_address) {
        Ok(found) => found,
        Err(reason) => return refuse(state, reason),
    };
    if view.generation != state.selected_generation {
        return refuse(state, Refusal::StaleGeneration);
    }
    if view.identity_claims != 1 {
        return refuse(state, Refusal::IdentityAmbiguous);
    }
    if view.identity_present != 1 || view.identity != identity {
        return refuse(state, Refusal::IdentityMismatch);
    }
    if usize::from(state.queue_len) >= COMMAND_CAPACITY {
        return refuse(state, Refusal::QueueFull);
    }
    let slot = usize::from(state.queue_len);
    let Some(identity_slot) = state.queue_identity.get_mut(slot) else {
        return InteractionResult::Refused(Refusal::QueueFull);
    };
    let Some(view_slot) = state.queue_view_sequence.get_mut(slot) else {
        return InteractionResult::Refused(Refusal::QueueFull);
    };
    let Some(input_slot) = state.queue_input_sequence.get_mut(slot) else {
        return InteractionResult::Refused(Refusal::QueueFull);
    };
    let Some(generation_slot) = state.queue_generation.get_mut(slot) else {
        return InteractionResult::Refused(Refusal::QueueFull);
    };
    let Some(time_slot) = state.queue_time_micros.get_mut(slot) else {
        return InteractionResult::Refused(Refusal::QueueFull);
    };
    let Some(intent_slot) = state.queue_intent.get_mut(slot) else {
        return InteractionResult::Refused(Refusal::QueueFull);
    };
    *identity_slot = identity;
    *view_slot = view_sequence;
    *input_slot = input_sequence;
    *generation_slot = view.generation;
    *time_slot = source_time_micros;
    *intent_slot = intent;
    state.queue_len += 1;
    state.last_input_sequence = input_sequence;
    state.has_input_sequence = true;
    state.last_source_time_micros = source_time_micros;
    state.last_view_sequence = view_sequence;
    state.has_view_sequence = true;
    state.last_refusal = 0;
    state.last_outcome = CommandOutcome::Pending;
    InteractionResult::Applied
}

/// Copy the oldest pending command to the bridge without removing it.
pub fn next_intent(state: &InteractionState) -> Option<QueuedIntent> {
    Some(QueuedIntent {
        identity: *state.queue_identity.first()?,
        source_view_sequence: *state.queue_view_sequence.first()?,
        input_sequence: *state.queue_input_sequence.first()?,
        target_generation: *state.queue_generation.first()?,
        source_time_micros: *state.queue_time_micros.first()?,
        intent: *state.queue_intent.first()?,
    })
    .filter(|_| state.queue_len > 0)
}

/// Complete the oldest command after the flight bridge has accepted, refused, or found it stale.
/// Removing the head shifts at most 31 rows, a fixed C7 bound.
pub fn complete_oldest(
    state: &mut InteractionState,
    input_sequence: u32,
    outcome: CommandOutcome,
) -> InteractionResult {
    if state.queue_len == 0 || outcome == CommandOutcome::None || outcome == CommandOutcome::Pending
    {
        return refuse(state, Refusal::NoPendingCommand);
    }
    if state.queue_input_sequence.first().copied() != Some(input_sequence) {
        return refuse(state, Refusal::CommandSequenceMismatch);
    }
    let n = usize::from(state.queue_len);
    let completed = state.queue_input_sequence.first().copied().unwrap_or(0);
    state.queue_identity.copy_within(1..n, 0);
    state.queue_view_sequence.copy_within(1..n, 0);
    state.queue_input_sequence.copy_within(1..n, 0);
    state.queue_generation.copy_within(1..n, 0);
    state.queue_time_micros.copy_within(1..n, 0);
    state.queue_intent.copy_within(1..n, 0);
    state.queue_len -= 1;
    state.last_completed_sequence = completed;
    state.last_outcome = outcome;
    state.last_refusal = 0;
    InteractionResult::Applied
}

/// Verify the queued head still names the same visible full identity before bridge dispatch.
/// This is an explicit preflight; the bridge must still validate against authoritative flight data.
pub fn preflight_oldest(
    state: &mut InteractionState,
    views: &[TargetView],
    view_sequence: u32,
) -> InteractionResult {
    let command = match next_intent(state) {
        Some(command) => command,
        None => return refuse(state, Refusal::NoSelection),
    };
    if view_sequence == 0
        || view_sequence < command.source_view_sequence
        || (state.has_view_sequence && view_sequence < state.last_view_sequence)
    {
        return refuse(state, Refusal::StaleView);
    }
    let (_, view) = match locate(views, command.identity.aircraft_address) {
        Ok(found) => found,
        Err(reason) => return refuse(state, reason),
    };
    if view.generation != command.target_generation {
        return refuse(state, Refusal::StaleGeneration);
    }
    if view.identity_claims != 1 {
        return refuse(state, Refusal::IdentityAmbiguous);
    }
    if view.identity_present != 1 || view.identity != command.identity {
        return refuse(state, Refusal::IdentityMismatch);
    }
    state.last_view_sequence = view_sequence;
    state.has_view_sequence = true;
    InteractionResult::Applied
}

fn text_width16(content: &str) -> i64 {
    i64::from(text_width_units(content.chars().count())) * i64::from(TEXT_SIZE) * 16
        / i64::from(CAP_HEIGHT)
}

/// Whether an interaction text block overlaps a higher-priority filled rectangle.
/// The scan is bounded and uses only the rectangle table's logical rows.
fn interaction_occluded(
    list: &DisplayList,
    x16: i64,
    y16: i64,
    width16: i64,
    height16: i64,
) -> Result<bool, DisplayError> {
    let rows = list.rects.x0.len();
    if [
        list.rects.y0.len(),
        list.rects.x1.len(),
        list.rects.y1.len(),
        list.rects.colour.len(),
        list.rects.layer.len(),
    ]
    .iter()
    .any(|&len| len != rows)
    {
        return Err(DisplayError::BadView(
            "interaction display list rectangle storage is inconsistent",
        ));
    }
    if rows > list.caps.rects || rows > MAX_OCCLUDER_RECTANGLES {
        return Err(DisplayError::BadView(
            "interaction display list exceeds the bounded rectangle scan",
        ));
    }

    let right16 = x16 + width16;
    let bottom16 = y16 + height16;
    for index in 0..rows {
        let layer = list
            .rects
            .layer
            .get(index)
            .copied()
            .ok_or(DisplayError::BadView(
                "interaction rectangle layer is missing",
            ))?;
        if layer < 14 {
            continue;
        }
        let x0 = list
            .rects
            .x0
            .get(index)
            .copied()
            .ok_or(DisplayError::BadView("interaction rectangle x0 is missing"))?;
        let x1 = list
            .rects
            .x1
            .get(index)
            .copied()
            .ok_or(DisplayError::BadView("interaction rectangle x1 is missing"))?;
        let y0 = list
            .rects
            .y0
            .get(index)
            .copied()
            .ok_or(DisplayError::BadView("interaction rectangle y0 is missing"))?;
        let y1 = list
            .rects
            .y1
            .get(index)
            .copied()
            .ok_or(DisplayError::BadView("interaction rectangle y1 is missing"))?;
        let left = i64::from(x0.min(x1));
        let top = i64::from(y0.min(y1));
        let right = i64::from(x0.max(x1));
        let bottom = i64::from(y0.max(y1));
        if x16 < right && right16 > left && y16 < bottom && bottom16 > top {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Check the text table and backing storage before appending either interaction line.
fn preflight_text_room(list: &DisplayList, rows: usize, bytes: usize) -> Result<(), DisplayError> {
    if list.faulted {
        return Err(DisplayError::BadView("interaction display list is faulted"));
    }
    let used = list.texts.x.len();
    let columns_aligned = [
        list.texts.y.len(),
        list.texts.size.len(),
        list.texts.align.len(),
        list.texts.start.len(),
        list.texts.len.len(),
        list.texts.colour.len(),
        list.texts.layer.len(),
    ]
    .iter()
    .all(|&column_len| column_len == used);
    if !columns_aligned || used > list.caps.texts || list.text.len() > list.caps.text_bytes {
        return Err(DisplayError::BadView(
            "interaction display list text storage is inconsistent",
        ));
    }
    let rows_needed = used.checked_add(rows).ok_or(DisplayError::TooLarge {
        what: "interaction text rows",
        requested: u128::MAX,
        limit: list.caps.texts as u128,
    })?;
    if rows_needed > list.caps.texts {
        return Err(DisplayError::Full {
            table: "texts",
            capacity: list.caps.texts,
        });
    }
    // B8: core decisions use the declared logical capacities, never allocator-chosen spare
    // capacity. list_new owns/preallocates storage; C5 exercises this API with that owned list.
    let bytes_needed = list
        .text
        .len()
        .checked_add(bytes)
        .ok_or(DisplayError::TooLarge {
            what: "interaction text bytes",
            requested: u128::MAX,
            limit: list.caps.text_bytes as u128,
        })?;
    if bytes_needed > list.caps.text_bytes {
        return Err(DisplayError::Full {
            table: "text bytes",
            capacity: list.caps.text_bytes,
        });
    }
    Ok(())
}

/// Append selected-target and command status text to the display list (core presentation only).
/// The caller supplies the active projection so the text is placed below its reserved legend
/// strip and clear of higher-priority filled rectangles, or refused before list mutation.
pub fn present_interaction(
    state: &InteractionState,
    list: &mut DisplayList,
    projection: &Projection,
) -> Result<(), DisplayError> {
    if state.selection_kind == SelectionKind::None
        && state.queue_len == 0
        && state.last_refusal == 0
    {
        return Ok(());
    }
    if projection.width == 0
        || projection.height == 0
        || projection.width > 1 << 16
        || projection.height > 1 << 16
        || projection.top_inset >= projection.height
    {
        return Err(DisplayError::BadView(
            "interaction projection has invalid screen dimensions",
        ));
    }

    let selection = if state.selection_kind != SelectionKind::None {
        let mut line = text_new();
        text_push_str(
            &mut line,
            if state.selection_kind == SelectionKind::RawTrack {
                "SELECT RAW "
            } else {
                "SELECT FLT "
            },
        );
        text_push_hex(&mut line, state.selected_address, 6);
        Some(line)
    } else {
        None
    };
    let status = match state.last_outcome {
        CommandOutcome::None => "CMD NONE",
        CommandOutcome::Pending => "CMD PENDING",
        CommandOutcome::Accepted => "CMD ACCEPTED",
        CommandOutcome::Refused => "CMD REFUSED",
        CommandOutcome::Stale => "CMD STALE",
    };
    let status = if let Some(reason) = last_refusal(state) {
        refusal_text(reason)
    } else {
        status
    };
    let status_colour = if state.last_outcome == CommandOutcome::Refused || state.last_refusal != 0
    {
        palette::ALERT
    } else {
        palette::TEXT
    };
    let rows = usize::from(selection.is_some()) + 1;
    let text_height_px = if selection.is_some() {
        TEXT_PITCH_PX + i64::from(TEXT_SIZE)
    } else {
        i64::from(TEXT_SIZE)
    };
    let top_px = i64::from(projection.top_inset)
        .checked_add(TEXT_TOP_PAD_PX)
        .ok_or(DisplayError::BadView("interaction position overflows"))?;
    let selection_width16 = selection
        .as_ref()
        .map_or(0, |line| text_width16(text_str(line)));
    let text_width16 = selection_width16.max(text_width16(status));
    let frame_width16 = i64::from(projection.width) * 16;
    if text_width16 + TEXT_MARGIN_PX * 2 * 16 > frame_width16 {
        return Err(DisplayError::BadView(
            "frame is too narrow for interaction status",
        ));
    }

    let left_px = TEXT_MARGIN_PX;
    let text_width_px = text_width16.saturating_add(15) / 16;
    let right_px = i64::from(projection.width) - TEXT_MARGIN_PX - text_width_px;
    let bottom_px = i64::from(projection.height) - BOTTOM_NOTICE_RESERVE_PX - text_height_px;
    let candidates = [
        (right_px, top_px),
        (right_px, bottom_px),
        (left_px, bottom_px),
        (left_px, top_px),
    ];
    let block_width16 = text_width16;
    let block_height16 = text_height_px * 16;
    let mut placed = None;
    for &(x_px, y_px) in &candidates {
        if x_px < 0
            || y_px < i64::from(projection.top_inset)
            || x_px * 16 + block_width16 > (i64::from(projection.width) - TEXT_MARGIN_PX) * 16
            || y_px + text_height_px > i64::from(projection.height) - BOTTOM_NOTICE_RESERVE_PX
        {
            continue;
        }
        let candidate_x16 = x_px * 16;
        let candidate_y16 = y_px * 16;
        if !interaction_occluded(
            list,
            candidate_x16,
            candidate_y16,
            block_width16,
            block_height16,
        )? {
            placed = Some((candidate_x16, candidate_y16));
            break;
        }
    }
    let Some((x16, y16)) = placed else {
        return Err(DisplayError::BadView(
            "no unobscured viewport position fits interaction status",
        ));
    };
    let y16 = i32::try_from(y16)
        .map_err(|_| DisplayError::BadView("interaction position is not representable"))?;
    let second_y16 = i32::try_from(i64::from(y16) + TEXT_PITCH_PX * 16)
        .map_err(|_| DisplayError::BadView("interaction position is not representable"))?;
    let x16 = i32::try_from(x16)
        .map_err(|_| DisplayError::BadView("interaction position is not representable"))?;
    let rows_bytes = selection
        .as_ref()
        .map_or(0, |line| text_str(line).len())
        .checked_add(status.len())
        .ok_or(DisplayError::TooLarge {
            what: "interaction text bytes",
            requested: u128::MAX,
            limit: list.caps.text_bytes as u128,
        })?;
    preflight_text_room(list, rows, rows_bytes)?;

    if let Some(line) = selection.as_ref() {
        push_text(
            list,
            (x16, y16),
            TEXT_SIZE,
            ALIGN_LEFT,
            palette::TEXT,
            LAYER_INTERACTION,
            text_str(line),
        )?;
    }
    push_text(
        list,
        (x16, if selection.is_some() { second_y16 } else { y16 }),
        TEXT_SIZE,
        ALIGN_LEFT,
        status_colour,
        LAYER_INTERACTION,
        status,
    )
}

fn refusal_text(reason: Refusal) -> &'static str {
    match reason {
        Refusal::NoSelection => "REFUSED: NO SELECT",
        Refusal::RawTrackOnly => "REFUSED: RAW TRACK",
        Refusal::ViewTooLarge => "REFUSED: VIEW LIMIT",
        Refusal::StaleView => "REFUSED: STALE VIEW",
        Refusal::TrackMissing => "REFUSED: TRACK GONE",
        Refusal::DuplicateAddress => "REFUSED: DUP ADDRESS",
        Refusal::IdentityAmbiguous => "REFUSED: AMBIGUOUS",
        Refusal::IdentityMismatch => "REFUSED: ID MISMATCH",
        Refusal::StaleGeneration => "REFUSED: NEW TRACK",
        Refusal::BackwardsSequence => "REFUSED: OLD INPUT",
        Refusal::QueueFull => "REFUSED: QUEUE FULL",
        Refusal::CommandSequenceMismatch => "REFUSED: COMMAND MISMATCH",
        Refusal::MalformedView => "REFUSED: BAD VIEW",
        Refusal::NoPendingCommand => "REFUSED: NO PENDING CMD",
        Refusal::InvalidAddress => "REFUSED: BAD ADDRESS",
        Refusal::BackwardsSourceTime => "REFUSED: OLD TIME",
    }
}

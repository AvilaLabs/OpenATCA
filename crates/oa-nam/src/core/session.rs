//! The session layer of NAS-IC-82422100 3.2.2.4 and Figure 3-3 (with NAS-IR-82422100 3.2.2.4
//! and the NAM ICD Rev G B.1.6 to B.1.8 and B.2.1): the INACTIVE / ACTIVATING / ACTIVE /
//! DEACTIVATING state machine, message numbering, and the bookkeeping of unanswered messages.
//! Core (pure, B1): [`step`] is a function `(config, session, event, now) -> (session, actions)`.
//! It reads no clock (time is the `now` argument, B4), does no I/O, allocates nothing, and is
//! bounded: [`MAX_PENDING`] unanswered messages, [`MAX_ACTIONS`] actions per step (C1, C2). It
//! is the only core module of this crate besides `identity`; the codec is shell (see lib.rs).
//! The counting-allocator check of C5 is the `nam_session` route of `validation/oa-alloc-check`.
//!
//! The shell turns [`Action`]s into bytes ([`crate::message::encode`], [`crate::frame::frame`])
//! and sends them, feeds received messages in as [`Event::Received`], and calls [`Event::Tick`]
//! when [`next_deadline`] has passed.

use super::identity::{Facility, Header, Kind, MsgId, Profile};

/// Time in milliseconds on any monotonic scale the shell chooses.
pub type Millis = u64;

/// Unanswered messages tracked at once. The CAATS design workload (NAS-IC-82422100 Table 3-IV)
/// peaks at 51.9 messages per minute for all of En Route that are answered (everything except
/// RTU and the responses themselves); with the 60 s response limit (Part III 6.1) that is about
/// 52 outstanding at once. 128 is 2.4 times that.
pub const MAX_PENDING: usize = 128;
/// Actions one [`step`] can return. The most any event produces is three (a TRQ in ACTIVE:
/// TRS, IRQ, abandoned count); a Tick produces an ASM or an IRQ and then at most
/// `MAX_ACTIONS - 2` response timeouts, the rest wait for the next tick.
pub const MAX_ACTIONS: usize = 8;
/// IRQ numbers remembered to recognise an expected IRS after retries.
pub const IRQ_HISTORY: usize = 8;

/// Message numbers run 000 to 999 and wrap (B.2.1).
pub const NUMBER_MODULUS: u16 = 1000;

/// The four session states (NAS-IC-82422100 3.2.2.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Inactive,
    Activating,
    Active,
    Deactivating,
}

/// The settings the ICD leaves to the partners ("an adaptable time", "a specified timeout").
/// Plain data (A1); a cold struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Config {
    pub local: Facility,
    pub peer: Facility,
    /// No message received for this long in ACTIVE: send an ASM ("on the order of seconds",
    /// NAS-IC-82422100 3.1.2).
    pub asm_idle_ms: Millis,
    /// No LAM for an ASM within this long: send an IRQ and go to ACTIVATING (3.2.2.4.1.3).
    pub asm_response_ms: Millis,
    /// IRS Timeout in ACTIVATING: send another IRQ (3.2.2.4.1.2).
    pub irs_timeout_ms: Millis,
    /// TRS Timeout in DEACTIVATING: go to INACTIVE (3.2.2.4.1.4).
    pub trs_timeout_ms: Millis,
    /// The optional "Max IRQ Retries" of Figure 3-3: after this many resent IRQs, send a TRQ
    /// and go to INACTIVE. `None` retries without limit (3.2.2.4.1.2).
    pub max_irq_retries: Option<u32>,
    /// Resent IRQs reuse the message number (B.1.6) instead of taking the next one
    /// (Attachment 2 2.10, the ERAM/CAATS interface).
    pub irq_retry_same_number: bool,
    /// A message that expects a LAM, RLA, PLA or LRM and has none after this long is reported
    /// ("a VSP period of time", B.1.7.2; 60 s, Part III 6.1).
    pub response_timeout_ms: Millis,
    /// Times to resend an unanswered message with its original number before reporting it
    /// failed (B.1.7.2). 0: none ("automatic retransmission will not be attempted",
    /// Attachments 1 and 3 to 7).
    pub max_resends: u8,
    /// An IRQ received in INACTIVE. `false` (default): follow Rev G B.1.6, which excepts the
    /// IRQ from "ignore everything when not initialised": it is surfaced as
    /// [`Action::InitRequested`] (no IRS, since a system that has not sent its own IRQ is not
    /// ready to answer, and no state change). `true`: follow IRD Figure 3-3, which ignores
    /// everything in INACTIVE but the Activate Request.
    pub inactive_ignores_irq: bool,
    /// The validation profile of the interface this session carries.
    pub profile: Profile,
}

/// A configuration with the defaults this crate chose where the ICD gives no number (see the
/// README): ASM after 10 s of silence, 10 s to answer, 10 s for an IRS or a TRS, no IRQ retry
/// limit, new numbers for repeated IRQs, 60 s response timeout, no resends.
pub const fn default_config(local: Facility, peer: Facility) -> Config {
    Config::for_profile(Profile::EramAttachment2, local, peer)
}

impl Config {
    /// The defaults for a profile. They differ in the IRQ number on a retry: Rev G B.1.6 says
    /// each repeated IRQ keeps its number ([`Profile::Core`]); Attachment 2 2.10 says it
    /// increments on the ERAM to NAV CANADA interface ([`Profile::EramAttachment2`]).
    pub const fn for_profile(profile: Profile, local: Facility, peer: Facility) -> Config {
        Config {
            local,
            peer,
            asm_idle_ms: 10_000,
            asm_response_ms: 10_000,
            irs_timeout_ms: 10_000,
            trs_timeout_ms: 10_000,
            max_irq_retries: None,
            irq_retry_same_number: !matches!(profile, Profile::EramAttachment2),
            response_timeout_ms: 60_000,
            max_resends: 0,
            inactive_ignores_irq: false,
            profile,
        }
    }
}

/// Time from the last message received to the session leaving ACTIVE on a dead partner: idle
/// interval plus the ASM response timeout. NAS-IR-82422100 3.2.2.2-b requires failures to be
/// detected and recovered within 60 s.
pub const fn detection_time_ms(c: &Config) -> Millis {
    c.asm_idle_ms.saturating_add(c.asm_response_ms)
}

/// A message of ours waiting for its logical response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Pending {
    pub deadline: Millis,
    pub number: u16,
    pub kind: Kind,
    pub resends_left: u8,
}
// Budget: 8 (deadline) + 2 (number) + 1 (kind) + 1 (resends_left) = 12, padded to the u64's
// alignment: 16. Held 128 at a time and scanned on every tick.
const _: () = assert!(core::mem::size_of::<Pending>() == 16);

const NO_PENDING: Pending = Pending {
    kind: Kind::Lam,
    number: 0,
    deadline: 0,
    resends_left: 0,
};

/// The whole mutable state of one session with one partner. Plain data (A1); it is copied in
/// and out of [`step`] on every event, so it is guarded (A7), not claimed cold: the 128-entry
/// pending table dominates its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Session {
    pub state: State,
    /// The number the next message we send takes.
    pub next_number: u16,
    /// When the last message from the partner arrived (the idle timer of ACTIVE).
    pub last_rx: Millis,
    /// Number of the last message received from the partner, for sequence checking.
    pub rx_last_number: Option<u16>,
    pub irs_deadline: Option<Millis>,
    pub trs_deadline: Option<Millis>,
    /// IRQs resent in this activation attempt.
    pub irq_retries: u32,
    /// Messages refused for a wrong sender or receiver (LRM codes 1 and 2).
    pub rejected: u32,
    /// Received messages discarded, per [`Discard`] reason (E6); read with [`discard_count`].
    /// Saturating.
    discards: [u32; DISCARD_REASONS],
    /// Requests refused, per [`Refusal`] reason (E6); read with [`refusal_count`]. Saturating.
    refusals: [u32; REFUSAL_REASONS],
    irq_numbers: [u16; IRQ_HISTORY],
    irq_len: u8,
    /// The ASM awaiting its LAM: number and deadline.
    pub asm: Option<(u16, Millis)>,
    // A3: array of structs by exception: 128 entries, a scan reads deadline, number and kind
    // together (and a removal moves whole entries), so a split would add indexing, not speed.
    pending: [Pending; MAX_PENDING],
    pending_len: u16,
}
// Budget: dominated by the 128 x 16 byte pending table (2048); the rest is the state, counters,
// timers and the 8-entry IRQ history. Copied in and out of every step.
// A7.1: grew 2216 -> 2224 bytes for a distinct counter covering wrapped-number refusals.
const _: () = assert!(core::mem::size_of::<Session>() == 2224);

/// A new session: INACTIVE, the first message number 000.
pub const fn new_session() -> Session {
    Session {
        state: State::Inactive,
        next_number: 0,
        last_rx: 0,
        rx_last_number: None,
        irs_deadline: None,
        trs_deadline: None,
        irq_retries: 0,
        rejected: 0,
        discards: [0; DISCARD_REASONS],
        refusals: [0; REFUSAL_REASONS],
        irq_numbers: [0; IRQ_HISTORY],
        irq_len: 0,
        asm: None,
        pending: [NO_PENDING; MAX_PENDING],
        pending_len: 0,
    }
}

impl Session {
    /// The unanswered messages, oldest first.
    pub fn pending(&self) -> &[Pending] {
        self.pending
            .get(..usize::from(self.pending_len))
            .unwrap_or(&[])
    }
}

/// Number of [`Discard`] reasons (the length of the per-reason counters).
pub const DISCARD_REASONS: usize = 8;
/// Number of [`Refusal`] reasons.
pub const REFUSAL_REASONS: usize = 6;

/// Messages discarded for `reason` since the session was created (saturating, E6).
pub fn discard_count(s: &Session, reason: Discard) -> u32 {
    s.discards
        .get(discard_index(reason))
        .copied()
        .unwrap_or(u32::MAX)
}

/// Send requests refused for `reason` since the session was created (saturating, E6).
pub fn refusal_count(s: &Session, reason: Refusal) -> u32 {
    s.refusals
        .get(refusal_index(reason))
        .copied()
        .unwrap_or(u32::MAX)
}

const fn discard_index(r: Discard) -> usize {
    match r {
        Discard::WrongSender => 0,
        Discard::WrongReceiver => 1,
        Discard::IgnoredInState => 2,
        Discard::UnexpectedIrs => 3,
        Discard::UnmatchedResponse => 4,
        Discard::MissingReference => 5,
        Discard::ForeignReference => 6,
        Discard::WrongUnitLrm => 7,
    }
}

const fn refusal_index(r: Refusal) -> usize {
    match r {
        Refusal::NotActive => 0,
        Refusal::ResponseNotAllowed => 1,
        Refusal::NotAnAtmMessage => 2,
        Refusal::NotAResponse => 3,
        Refusal::PendingFull => 4,
        Refusal::MessageNumberInUse => 5,
    }
}

/// What can happen to a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The local "Activate Request" (an operator, or automatic recovery).
    Activate,
    /// The local "Deactivate Request".
    Deactivate,
    /// The transport connection dropped. Not in Figure 3-3: ERAM drops and the partner
    /// reconnects and re-initiates (NAS-IR-82422100 3.2.2.6-c), so the session goes to INACTIVE
    /// and the shell raises an Activate Request once the connection is back.
    ConnectionLost,
    /// Time passed; the timers are checked against `now`.
    Tick,
    /// A message arrived and parsed. Its content beyond the header is the application's.
    Received(Header),
    /// The application wants to send an ATM message of this kind; the session numbers it.
    SendAtm(Kind),
    /// The application selected an explicit peer policy in which this ATM message has no
    /// logical response. It still receives a session number but creates no pending timer.
    SendAtmWithoutResponse(Kind),
    /// The application wants to send a logical response (LAM, LRM, RLA, PLA) to a received
    /// message; the session numbers it.
    SendResponse { kind: Kind, reference: MsgId },
}

// Budget: Header (24) plus the tag, the largest variant is Received; passed in on every step.
const _: () = assert!(core::mem::size_of::<Event>() == 24);

/// Where an inbound message number stands against the last one (B.2.1: implementers should
/// check for out-of-sequence and duplicate messages).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seq {
    First,
    InOrder,
    /// The same number as the last message (a resend, B.1.7.2).
    Duplicate,
    /// Numbers were skipped: how many.
    Gap(u16),
}

/// Why a received message was not acted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discard {
    /// Field 03(b) names a sender other than the partner (Part III 4.2).
    WrongSender,
    /// Field 03(b) names a receiver other than us.
    WrongReceiver,
    /// The state ignores this message (Figure 3-3, B.1.6).
    IgnoredInState,
    /// An IRS whose reference is none of our recent IRQs.
    UnexpectedIrs,
    /// A LAM, LRM, RLA or PLA that answers no message of ours, or the wrong kind of response.
    UnmatchedResponse,
    /// A response with no Field 03(c).
    MissingReference,
    /// A response whose 03(c) names other units than this session's.
    ForeignReference,
    /// A wrong-unit message of kind LRM, which is never answered with an LRM.
    WrongUnitLrm,
}

/// Why a request to send was not carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// ATM messages go out only in ACTIVE.
    NotActive,
    /// Responses go out only in ACTIVE and DEACTIVATING.
    ResponseNotAllowed,
    /// A session message, a response or an ASM was asked for as an ATM message.
    NotAnAtmMessage,
    /// A response kind other than LAM, LRM, RLA, PLA.
    NotAResponse,
    /// [`MAX_PENDING`] messages are unanswered; nothing was numbered.
    PendingFull,
    /// The next sequential message number is still used by an unanswered message; nothing was
    /// numbered or sent. The application may retry after that response arrives or times out.
    MessageNumberInUse,
}

/// What the shell must do. Plain data (A1). Numbers are the Field 03(b) message numbers to
/// write; the sender and receiver come from the [`Config`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Filler in unused slots of [`Actions`]; never in [`Actions::as_slice`].
    Nop,
    SendIrq {
        number: u16,
    },
    SendIrs {
        number: u16,
        reference: MsgId,
    },
    SendTrq {
        number: u16,
    },
    SendTrs {
        number: u16,
        reference: MsgId,
    },
    SendAsm {
        number: u16,
    },
    SendLam {
        number: u16,
        reference: MsgId,
    },
    /// Send the ATM message the application asked for, with this number.
    SendAtm {
        kind: Kind,
        number: u16,
    },
    /// Send the unanswered message `number` again, unchanged (B.1.7.2).
    ResendAtm {
        kind: Kind,
        number: u16,
    },
    /// Send the logical response the application asked for.
    SendResponse {
        kind: Kind,
        number: u16,
        reference: MsgId,
    },
    /// Hand a received ATM message to the application (ACTIVE only).
    Deliver {
        header: Header,
        seq: Seq,
    },
    /// A received logical response answered our message `answers` (number in `header`'s
    /// reference); hand it to the application.
    Response {
        header: Header,
        answers: Kind,
    },
    /// Our ASM was answered.
    AsmAnswered,
    /// An LRM answered no message of ours (an RTU, an ASM, or one already answered or timed
    /// out); the header's reference names the message number. Hand it to the application: the
    /// code and text are in the message body.
    UnmatchedLrm {
        header: Header,
    },
    /// An IRQ arrived in INACTIVE and Rev G B.1.6 applies ([`Config::inactive_ignores_irq`] is
    /// false): the partner wants the interface initialised. Nothing was sent.
    InitRequested {
        header: Header,
    },
    /// A message with a wrong sender or receiver in 03(b) was refused: send this LRM (code 1
    /// or 2, field 3, supporting text per Attachment 2 Table 2-2) to the partner, referencing
    /// the refused message.
    SendLrm {
        number: u16,
        reference: MsgId,
        code: u8,
        field: u8,
    },
    /// No response to our message within the timeout (and no resends left): tell the
    /// responsible position (Part III 3.1 d).
    ResponseTimeout {
        kind: Kind,
        number: u16,
    },
    /// This many messages were awaiting a response when the session left ACTIVE or
    /// DEACTIVATING; their responses will not come.
    AbandonPending {
        count: u16,
    },
    /// An ASM went unanswered: the partner is silent, the interface is being re-initialised.
    PeerSilent,
    /// The maximum number of IRQ retries passed: a TRQ was sent and the session is INACTIVE.
    ActivationFailed,
    Discard {
        reason: Discard,
    },
    Refuse {
        reason: Refusal,
    },
}

// Budget: 30 bytes. The largest variants are SendResponse (kind, number, 10-byte reference), Deliver and
// Response (a 24-byte Header plus a Seq or Kind) and the 2-byte tag; not `repr(C)`, which is not
// meaningful for a data-carrying enum, so the exact size is the guard. Returned in quantity.
const _: () = assert!(core::mem::size_of::<Action>() == 30);

/// The actions of one step, in the order to perform them. A fixed-capacity list (C1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Actions {
    items: [Action; MAX_ACTIONS],
    len: u8,
    /// Actions that did not fit. Always zero by the bound on [`MAX_ACTIONS`]; a test checks it
    /// stays zero, and a non-zero value is never silent (E6).
    pub dropped: u8,
}

// Budget: MAX_ACTIONS (8) x the action size, plus len and dropped; returned by every step.
const _: () = assert!(core::mem::size_of::<Actions>() == 242);

impl Actions {
    pub const fn new() -> Actions {
        Actions {
            items: [Action::Nop; MAX_ACTIONS],
            len: 0,
            dropped: 0,
        }
    }

    pub fn as_slice(&self) -> &[Action] {
        self.items.get(..usize::from(self.len)).unwrap_or(&[])
    }

    fn push(&mut self, a: Action) {
        match self.items.get_mut(usize::from(self.len)) {
            Some(slot) => {
                *slot = a;
                self.len += 1;
            }
            None => self.dropped = self.dropped.saturating_add(1),
        }
    }

    fn room(&self) -> usize {
        MAX_ACTIONS - usize::from(self.len)
    }
}

impl Default for Actions {
    fn default() -> Self {
        Actions::new()
    }
}

// ---------------------------------------------------------------------------------- helpers

/// The session messages of Table 3-III.
pub fn is_session_kind(k: Kind) -> bool {
    matches!(k, Kind::Irq | Kind::Irs | Kind::Trq | Kind::Trs | Kind::Asm)
}

/// The logical responses: LAM, LRM, RLA, PLA.
pub fn is_response_kind(k: Kind) -> bool {
    matches!(k, Kind::Lam | Kind::Lrm | Kind::Rla | Kind::Pla)
}

/// Does an ATM message get a logical response (Table B-1: every message but RTU, and not the
/// responses and session messages).
pub fn expects_response(k: Kind) -> bool {
    !is_session_kind(k) && !is_response_kind(k) && k != Kind::Rtu
}

/// [`expects_response`] under a profile: in Rev E the POA and POJ are acknowledgements that
/// "require no response" (3.7.3.1, 3.7.4.1, B.1.8.5, B.1.8.6); Rev G expects a LAM or LRM.
pub fn expects_response_in(profile: Profile, k: Kind) -> bool {
    expects_response(k) && !(profile == Profile::RevE && matches!(k, Kind::Poa | Kind::Poj))
}

/// Is `resp` an acceptable logical response to our message of kind `req`: an RLA (or LRM) to an
/// RTI, a PLA (or LRM) to a POI, otherwise a LAM (or LRM).
pub fn answers(req: Kind, resp: Kind) -> bool {
    resp == Kind::Lrm
        || match req {
            Kind::Rti => resp == Kind::Rla,
            Kind::Poi => resp == Kind::Pla,
            _ => resp == Kind::Lam,
        }
}

// E8: `next_number` and message numbers are `pub u16`, so a hand-built value can exceed 999.
// Reduce modulo the modulus first; then every sum below is at most 2 x 1000 and cannot overflow.
fn take_number(s: &mut Session) -> u16 {
    let n = s.next_number % NUMBER_MODULUS;
    s.next_number = (n + 1) % NUMBER_MODULUS;
    n
}

fn seq_of(last: Option<u16>, n: u16) -> Seq {
    match last {
        None => Seq::First,
        Some(l) => {
            let d = (n % NUMBER_MODULUS + NUMBER_MODULUS - l % NUMBER_MODULUS) % NUMBER_MODULUS;
            match d {
                0 => Seq::Duplicate,
                1 => Seq::InOrder,
                d => Seq::Gap(d - 1),
            }
        }
    }
}

fn remember_irq(s: &mut Session, n: u16) {
    let len = usize::from(s.irq_len);
    if len < IRQ_HISTORY {
        if let Some(slot) = s.irq_numbers.get_mut(len) {
            *slot = n;
            s.irq_len += 1;
        }
    } else {
        s.irq_numbers.rotate_left(1);
        if let Some(slot) = s.irq_numbers.last_mut() {
            *slot = n;
        }
    }
}

fn irq_known(s: &Session, n: u16) -> bool {
    s.irq_numbers
        .get(..usize::from(s.irq_len))
        .is_some_and(|h| h.contains(&n))
}

/// Sends an IRQ and arms the IRS timeout. A retry may reuse the last number.
fn send_irq(cfg: &Config, s: &mut Session, now: Millis, a: &mut Actions, retry: bool) {
    let last = usize::from(s.irq_len)
        .checked_sub(1)
        .and_then(|i| s.irq_numbers.get(i).copied());
    let number = match (retry && cfg.irq_retry_same_number, last) {
        (true, Some(n)) => n,
        _ => take_number(s),
    };
    remember_irq(s, number);
    s.irs_deadline = Some(now.saturating_add(cfg.irs_timeout_ms));
    a.push(Action::SendIrq { number });
}

fn enter_activating(cfg: &Config, s: &mut Session, now: Millis, a: &mut Actions) {
    s.state = State::Activating;
    s.irq_retries = 0;
    s.irq_len = 0;
    s.asm = None;
    s.trs_deadline = None;
    send_irq(cfg, s, now, a, false);
}

fn abandon_pending(s: &mut Session, a: &mut Actions) {
    if s.pending_len > 0 {
        a.push(Action::AbandonPending {
            count: s.pending_len,
        });
    }
    s.pending_len = 0;
}

fn enter_inactive(s: &mut Session, a: &mut Actions) {
    s.state = State::Inactive;
    s.irs_deadline = None;
    s.trs_deadline = None;
    s.asm = None;
    // The next ACTIVE exchange is a new numbering context; sequence evidence from the prior
    // connection must not label its first message as a duplicate or gap.
    s.rx_last_number = None;
    abandon_pending(s, a);
}

fn remove_pending(s: &mut Session, i: usize) {
    let len = usize::from(s.pending_len);
    if i < len {
        s.pending.copy_within(i + 1..len, i);
        s.pending_len -= 1;
    }
}

fn mine(cfg: &Config, r: &MsgId) -> bool {
    r.sender == cfg.local && r.receiver == cfg.peer
}

/// A received LAM, LRM, RLA or PLA (processed in ACTIVE and DEACTIVATING, never causing a
/// transition).
fn on_response(cfg: &Config, s: &mut Session, h: Header, a: &mut Actions) {
    let Some(r) = h.reference else {
        a.push(Action::Discard {
            reason: Discard::MissingReference,
        });
        return;
    };
    if !mine(cfg, &r) {
        a.push(Action::Discard {
            reason: Discard::ForeignReference,
        });
        return;
    }
    if let Some((n, _)) = s.asm {
        if n == r.number {
            s.asm = None;
            if h.kind == Kind::Lam {
                a.push(Action::AsmAnswered);
            } else if h.kind == Kind::Lrm {
                a.push(Action::UnmatchedLrm { header: h });
            } else {
                a.push(Action::Discard {
                    reason: Discard::UnmatchedResponse,
                });
            }
            return;
        }
    }
    let found = s.pending().iter().position(|p| p.number == r.number);
    match found.and_then(|i| s.pending().get(i).map(|p| (i, p.kind))) {
        Some((i, req)) if answers(req, h.kind) => {
            remove_pending(s, i);
            a.push(Action::Response {
                header: h,
                answers: req,
            });
        }
        // An LRM may answer a message with no computer response, an RTU for one (Table B-1):
        // the application must see it.
        _ if h.kind == Kind::Lrm => a.push(Action::UnmatchedLrm { header: h }),
        _ => a.push(Action::Discard {
            reason: Discard::UnmatchedResponse,
        }),
    }
}

fn ignored(a: &mut Actions) {
    a.push(Action::Discard {
        reason: Discard::IgnoredInState,
    });
}

fn on_received(cfg: &Config, s: &mut Session, h: Header, now: Millis, a: &mut Actions) {
    let wrong = if h.id.sender != cfg.peer {
        Some((Discard::WrongSender, 1))
    } else if h.id.receiver != cfg.local {
        Some((Discard::WrongReceiver, 2))
    } else {
        None
    };
    if let Some((reason, code)) = wrong {
        s.rejected = s.rejected.saturating_add(1);
        if s.state == State::Inactive {
            a.push(Action::Discard { reason });
        } else if h.kind == Kind::Lrm {
            a.push(Action::Discard {
                reason: Discard::WrongUnitLrm,
            });
        } else {
            let number = take_number(s);
            a.push(Action::Discard { reason });
            a.push(Action::SendLrm {
                number,
                reference: h.id,
                code,
                field: 3,
            });
        }
        return;
    }
    let k = h.kind;
    match s.state {
        State::Inactive => {
            if k == Kind::Irq && !cfg.inactive_ignores_irq {
                a.push(Action::InitRequested { header: h });
            } else {
                ignored(a);
            }
        }
        State::Activating => match k {
            Kind::Irs => {
                let expected = h
                    .reference
                    .is_some_and(|r| mine(cfg, &r) && irq_known(s, r.number));
                if expected {
                    s.state = State::Active;
                    s.irs_deadline = None;
                    s.irq_retries = 0;
                    s.last_rx = now;
                } else {
                    a.push(Action::Discard {
                        reason: Discard::UnexpectedIrs,
                    });
                }
            }
            Kind::Irq => {
                // The partner may have been INACTIVE when our IRQ went out: send another
                // before the IRS so it answers us (3.2.2.4.1.2).
                send_irq(cfg, s, now, a, false);
                let number = take_number(s);
                a.push(Action::SendIrs {
                    number,
                    reference: h.id,
                });
            }
            Kind::Trq => {
                let number = take_number(s);
                a.push(Action::SendTrs {
                    number,
                    reference: h.id,
                });
            }
            _ => ignored(a),
        },
        State::Active => {
            s.last_rx = now;
            // Only messages the session processes count for sequence checking (B.2.1).
            let seq = seq_of(s.rx_last_number, h.id.number);
            s.rx_last_number = Some(h.id.number);
            match k {
                Kind::Asm => {
                    let number = take_number(s);
                    a.push(Action::SendLam {
                        number,
                        reference: h.id,
                    });
                }
                Kind::Irq => {
                    let number = take_number(s);
                    a.push(Action::SendIrs {
                        number,
                        reference: h.id,
                    });
                }
                Kind::Trq => {
                    let number = take_number(s);
                    a.push(Action::SendTrs {
                        number,
                        reference: h.id,
                    });
                    // Unanswered messages stay pending and time out normally: no document
                    // says a re-initialisation abandons them.
                    enter_activating(cfg, s, now, a);
                }
                // Received in ACTIVE with no effect (Figure 3-3: Receive IRS, TRS, ASM-LAM).
                Kind::Irs | Kind::Trs => {}
                Kind::Lam | Kind::Lrm | Kind::Rla | Kind::Pla => on_response(cfg, s, h, a),
                _ => a.push(Action::Deliver { header: h, seq }),
            }
        }
        State::Deactivating => match k {
            Kind::Trs => enter_inactive(s, a),
            Kind::Trq => {
                let number = take_number(s);
                a.push(Action::SendTrs {
                    number,
                    reference: h.id,
                });
                enter_inactive(s, a);
            }
            Kind::Lam | Kind::Lrm | Kind::Rla | Kind::Pla => on_response(cfg, s, h, a),
            _ => ignored(a),
        },
    }
}

fn on_tick(cfg: &Config, s: &mut Session, now: Millis, a: &mut Actions) {
    match s.state {
        State::Inactive => {}
        State::Activating => {
            if s.irs_deadline.is_some_and(|d| now >= d) {
                match cfg.max_irq_retries {
                    Some(max) if s.irq_retries >= max => {
                        let number = take_number(s);
                        a.push(Action::SendTrq { number });
                        a.push(Action::ActivationFailed);
                        enter_inactive(s, a);
                    }
                    _ => {
                        s.irq_retries = s.irq_retries.saturating_add(1);
                        send_irq(cfg, s, now, a, true);
                    }
                }
            }
            if s.state == State::Activating {
                expire_pending(cfg, s, now, a, false);
            }
        }
        State::Active => {
            match s.asm {
                Some((_, deadline)) if now >= deadline => {
                    a.push(Action::PeerSilent);
                    enter_activating(cfg, s, now, a);
                    return;
                }
                Some(_) => {}
                None => {
                    if now >= s.last_rx.saturating_add(cfg.asm_idle_ms) {
                        let number = take_number(s);
                        s.asm = Some((number, now.saturating_add(cfg.asm_response_ms)));
                        a.push(Action::SendAsm { number });
                    }
                }
            }
            expire_pending(cfg, s, now, a, true);
        }
        State::Deactivating => {
            if s.trs_deadline.is_some_and(|d| now >= d) {
                enter_inactive(s, a);
            } else {
                expire_pending(cfg, s, now, a, false);
            }
        }
    }
}

/// Resends or reports messages whose response is overdue, at most as many as there is room for
/// in `a`; the rest are handled on a later tick.
fn expire_pending(cfg: &Config, s: &mut Session, now: Millis, a: &mut Actions, may_resend: bool) {
    let mut i = 0;
    while i < usize::from(s.pending_len) && a.room() > 0 {
        let Some(p) = s.pending.get(i).copied() else {
            break;
        };
        if now < p.deadline {
            i += 1;
            continue;
        }
        if may_resend && p.resends_left > 0 {
            if let Some(slot) = s.pending.get_mut(i) {
                slot.resends_left -= 1;
                slot.deadline = now.saturating_add(cfg.response_timeout_ms);
            }
            a.push(Action::ResendAtm {
                kind: p.kind,
                number: p.number,
            });
            i += 1;
        } else {
            a.push(Action::ResponseTimeout {
                kind: p.kind,
                number: p.number,
            });
            remove_pending(s, i);
        }
    }
}

/// One step of the session: the new session and the actions to perform, in order.
pub fn step(cfg: &Config, s: Session, ev: Event, now: Millis) -> (Session, Actions) {
    let mut s = s;
    let mut a = Actions::new();
    match ev {
        Event::Activate => {
            if s.state == State::Inactive {
                enter_activating(cfg, &mut s, now, &mut a);
            }
        }
        Event::Deactivate => match s.state {
            State::Activating => {
                let number = take_number(&mut s);
                a.push(Action::SendTrq { number });
                enter_inactive(&mut s, &mut a);
            }
            State::Active => {
                let number = take_number(&mut s);
                a.push(Action::SendTrq { number });
                // An ASM awaiting its LAM stays: the LAM is still processed (IRD 3.2.2.4.1.4).
                s.state = State::Deactivating;
                s.trs_deadline = Some(now.saturating_add(cfg.trs_timeout_ms));
            }
            State::Inactive | State::Deactivating => {}
        },
        Event::ConnectionLost => {
            if s.state != State::Inactive {
                enter_inactive(&mut s, &mut a);
            }
        }
        Event::Tick => on_tick(cfg, &mut s, now, &mut a),
        Event::Received(h) => on_received(cfg, &mut s, h, now, &mut a),
        Event::SendAtm(kind) => {
            if is_session_kind(kind) || is_response_kind(kind) {
                a.push(Action::Refuse {
                    reason: Refusal::NotAnAtmMessage,
                });
            } else if s.state != State::Active {
                a.push(Action::Refuse {
                    reason: Refusal::NotActive,
                });
            } else if expects_response_in(cfg.profile, kind)
                && usize::from(s.pending_len) == MAX_PENDING
            {
                a.push(Action::Refuse {
                    reason: Refusal::PendingFull,
                });
            } else if expects_response_in(cfg.profile, kind)
                && s.pending()
                    .iter()
                    .any(|p| p.number == s.next_number % NUMBER_MODULUS)
            {
                // A response identifies its request by the 000..999 number. Do not reuse a
                // still-pending number at wrap: that would make two requests indistinguishable.
                a.push(Action::Refuse {
                    reason: Refusal::MessageNumberInUse,
                });
            } else {
                let number = take_number(&mut s);
                if expects_response_in(cfg.profile, kind) {
                    let i = usize::from(s.pending_len);
                    if let Some(slot) = s.pending.get_mut(i) {
                        *slot = Pending {
                            kind,
                            number,
                            deadline: now.saturating_add(cfg.response_timeout_ms),
                            resends_left: cfg.max_resends,
                        };
                        s.pending_len += 1;
                    }
                }
                a.push(Action::SendAtm { kind, number });
            }
        }
        Event::SendAtmWithoutResponse(kind) => {
            if !matches!(kind, Kind::Poa | Kind::Poj) {
                a.push(Action::Refuse {
                    reason: Refusal::NotAnAtmMessage,
                });
            } else if s.state != State::Active {
                a.push(Action::Refuse {
                    reason: Refusal::NotActive,
                });
            } else if s
                .pending()
                .iter()
                .any(|pending| pending.number == s.next_number % NUMBER_MODULUS)
            {
                a.push(Action::Refuse {
                    reason: Refusal::MessageNumberInUse,
                });
            } else {
                let number = take_number(&mut s);
                a.push(Action::SendAtm { kind, number });
            }
        }
        Event::SendResponse { kind, reference } => {
            if !is_response_kind(kind) {
                a.push(Action::Refuse {
                    reason: Refusal::NotAResponse,
                });
            } else if !matches!(s.state, State::Active | State::Deactivating) {
                a.push(Action::Refuse {
                    reason: Refusal::ResponseNotAllowed,
                });
            } else {
                let number = take_number(&mut s);
                a.push(Action::SendResponse {
                    kind,
                    number,
                    reference,
                });
            }
        }
    }
    count_reasons(&mut s, &a);
    (s, a)
}

/// Counts every discard and refusal the step returned under its reason (E6), saturating.
fn count_reasons(s: &mut Session, a: &Actions) {
    for act in a.as_slice() {
        let slot = match act {
            Action::Discard { reason } => s.discards.get_mut(discard_index(*reason)),
            Action::Refuse { reason } => s.refusals.get_mut(refusal_index(*reason)),
            _ => None,
        };
        if let Some(c) = slot {
            *c = c.saturating_add(1);
        }
    }
}

/// The earliest time a [`Event::Tick`] would do something, or `None` if no timer is running.
pub fn next_deadline(cfg: &Config, s: &Session) -> Option<Millis> {
    let mut best: Option<Millis> = None;
    let mut take = |t: Millis| best = Some(best.map_or(t, |b| b.min(t)));
    match s.state {
        State::Inactive => {}
        State::Activating => {
            if let Some(d) = s.irs_deadline {
                take(d);
            }
            for p in s.pending() {
                take(p.deadline);
            }
        }
        State::Active => {
            match s.asm {
                Some((_, d)) => take(d),
                None => take(s.last_rx.saturating_add(cfg.asm_idle_ms)),
            }
            for p in s.pending() {
                take(p.deadline);
            }
        }
        State::Deactivating => {
            if let Some(d) = s.trs_deadline {
                take(d);
            }
            for p in s.pending() {
                take(p.deadline);
            }
        }
    }
    best
}

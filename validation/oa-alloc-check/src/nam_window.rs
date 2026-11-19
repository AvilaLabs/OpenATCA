//! Allocation window for the NAM session core (`oa_nam::session::step`). Shell (B3): a bounded
//! scripted fixture and its evidence. C5: after `new_session`, a run through activation with an
//! IRQ retry, message traffic, resends and response timeouts, a silent partner, a full pending
//! table, refusals and deactivation must not call the global allocator.
#![forbid(unsafe_code)]
use super::allocator::{self, Counts};
use oa_nam::session::{
    default_config, discard_count, new_session, refusal_count, step, Action, Actions, Config,
    Discard, Event, Refusal, Session, State, MAX_PENDING,
};
use oa_nam::{Facility, Header, Kind, MsgId};

const US: Facility = Facility(*b"KZMP");
const THEM: Facility = Facility(*b"CZWG");

/// A7.2: cold evidence for the NAM session window.
pub(super) struct NamEvidence {
    pub counts: Counts,
    pub irq_sent: u64,
    pub atm_sent: u64,
    pub resent: u64,
    pub timed_out: u64,
    pub delivered: u64,
    pub responses: u64,
    pub asm_sent: u64,
    pub peer_silent: u64,
    pub lrm_sent: u64,
    pub trq_sent: u64,
    pub abandoned: u64,
    pub peak_pending: usize,
    pub pending_full: u32,
    pub discards: u32,
    pub dropped: u64,
    pub script_errors: u64,
    pub final_inactive: bool,
    pub injected: bool,
}

// A7.2: cold tally, one per window.
#[derive(Default)]
struct Tally {
    irq_sent: u64,
    atm_sent: u64,
    resent: u64,
    timed_out: u64,
    delivered: u64,
    responses: u64,
    asm_sent: u64,
    peer_silent: u64,
    lrm_sent: u64,
    trq_sent: u64,
    abandoned: u64,
    peak_pending: usize,
    dropped: u64,
    script_errors: u64,
}

const fn rx(kind: Kind, number: u16, reference: Option<u16>, sender: Facility) -> Header {
    let reference = match reference {
        Some(n) => Some(MsgId {
            sender: US,
            receiver: THEM,
            number: n,
        }),
        None => None,
    };
    Header {
        kind,
        id: MsgId {
            sender,
            receiver: US,
            number,
        },
        reference,
    }
}

fn run(cfg: &Config, s: &mut Session, ev: Event, now: u64, t: &mut Tally) -> Actions {
    let (ns, a) = step(cfg, *s, ev, now);
    *s = ns;
    for act in a.as_slice() {
        match act {
            Action::SendIrq { .. } => t.irq_sent += 1,
            Action::SendAtm { .. } => t.atm_sent += 1,
            Action::ResendAtm { .. } => t.resent += 1,
            Action::ResponseTimeout { .. } => t.timed_out += 1,
            Action::Deliver { .. } => t.delivered += 1,
            Action::Response { .. } => t.responses += 1,
            Action::SendAsm { .. } => t.asm_sent += 1,
            Action::PeerSilent => t.peer_silent += 1,
            Action::SendLrm { .. } => t.lrm_sent += 1,
            Action::SendTrq { .. } => t.trq_sent += 1,
            Action::AbandonPending { count } => t.abandoned += u64::from(*count),
            _ => {}
        }
    }
    t.dropped += u64::from(a.dropped);
    t.peak_pending = t.peak_pending.max(s.pending().len());
    a
}

fn sent_number(a: &Actions) -> Option<u16> {
    a.as_slice().iter().find_map(|x| match x {
        Action::SendAtm { number, .. } => Some(*number),
        _ => None,
    })
}

fn irq_number(a: &Actions) -> Option<u16> {
    a.as_slice().iter().find_map(|x| match x {
        Action::SendIrq { number } => Some(*number),
        _ => None,
    })
}

/// The scripted run. Every event is built on the stack; nothing here may allocate.
fn script(s: &mut Session, t: &mut Tally) {
    // Long ASM idle for the traffic phases, so the keep-alive does not interleave with them.
    let quiet = Config {
        asm_idle_ms: 1_000_000_000,
        max_resends: 1,
        max_irq_retries: Some(3),
        ..default_config(US, THEM)
    };
    let lively = Config {
        max_irq_retries: Some(3),
        ..default_config(US, THEM)
    };

    // Activation with one IRQ retry (IRS timeout), then the partner's IRS.
    run(&quiet, s, Event::Activate, 0, t);
    let retry = run(&quiet, s, Event::Tick, 10_000, t);
    let Some(irq) = irq_number(&retry) else {
        t.script_errors += 1;
        return;
    };
    run(
        &quiet,
        s,
        Event::Received(rx(Kind::Irs, 0, Some(irq), THEM)),
        10_001,
        t,
    );
    if s.state != State::Active {
        t.script_errors += 1;
        return;
    }

    // Traffic: deliveries, an ASM answered, our messages answered by LAM, a response sent.
    run(
        &quiet,
        s,
        Event::Received(rx(Kind::Fpl, 1, None, THEM)),
        11_000,
        t,
    );
    run(
        &quiet,
        s,
        Event::Received(rx(Kind::Asm, 2, None, THEM)),
        11_001,
        t,
    );
    let mut numbers = [0u16; 4];
    for slot in numbers.iter_mut() {
        let a = run(&quiet, s, Event::SendAtm(Kind::Fpl), 12_000, t);
        match sent_number(&a) {
            Some(n) => *slot = n,
            None => t.script_errors += 1,
        }
    }
    for n in numbers.iter().take(2).copied() {
        run(
            &quiet,
            s,
            Event::Received(rx(Kind::Lam, 3, Some(n), THEM)),
            12_500,
            t,
        );
    }
    let reference = rx(Kind::Fpl, 4, None, THEM).id;
    run(
        &quiet,
        s,
        Event::SendResponse {
            kind: Kind::Lam,
            reference,
        },
        12_600,
        t,
    );
    // An LRM that answers nothing of ours, and a response to nothing.
    run(
        &quiet,
        s,
        Event::Received(rx(Kind::Lrm, 5, Some(999), THEM)),
        12_700,
        t,
    );
    run(
        &quiet,
        s,
        Event::Received(rx(Kind::Lam, 6, Some(998), THEM)),
        12_800,
        t,
    );

    // Retries: the two unanswered messages are resent once, then reported timed out.
    run(&quiet, s, Event::Tick, 12_000 + 60_000, t);
    run(&quiet, s, Event::Tick, 12_000 + 120_001, t);

    // A silent partner: ASM sent, no LAM, the session re-initialises; an IRS restores it.
    let idle = 200_000;
    run(
        &lively,
        s,
        Event::Received(rx(Kind::Fpl, 7, None, THEM)),
        idle,
        t,
    );
    run(&lively, s, Event::Tick, idle + 10_000, t);
    let silent = run(&lively, s, Event::Tick, idle + 20_000, t);
    let Some(irq) = irq_number(&silent) else {
        t.script_errors += 1;
        return;
    };
    run(
        &lively,
        s,
        Event::Received(rx(Kind::Irs, 8, Some(irq), THEM)),
        idle + 20_100,
        t,
    );
    if s.state != State::Active {
        t.script_errors += 1;
        return;
    }

    // A full pending table: MAX_PENDING sends, one more refused.
    let full = idle + 21_000;
    for _ in 0..MAX_PENDING {
        run(&quiet, s, Event::SendAtm(Kind::Fpl), full, t);
    }
    run(&quiet, s, Event::SendAtm(Kind::Fpl), full, t);
    // Refusals and discards: a message for another unit, a send of a response kind as ATM, a
    // response to nothing.
    run(
        &quiet,
        s,
        Event::Received(rx(Kind::Fpl, 9, None, Facility(*b"XXXX"))),
        full + 1,
        t,
    );
    run(&quiet, s, Event::SendAtm(Kind::Lam), full + 2, t);
    run(
        &quiet,
        s,
        Event::Received(rx(Kind::Lam, 10, None, THEM)),
        full + 3,
        t,
    );

    // Deactivation with the table full, then the TRS timeout: everything pending is abandoned.
    run(&quiet, s, Event::Deactivate, full + 10, t);
    run(&quiet, s, Event::Tick, full + 10 + 10_000, t);
    // Further activation and a lost connection.
    run(&quiet, s, Event::Activate, full + 30_000, t);
    run(&quiet, s, Event::ConnectionLost, full + 30_001, t);
}

/// Runs the scripted session under the counting allocator.
pub(super) fn nam_session_window(inject: bool) -> Result<NamEvidence, &'static str> {
    let mut session = new_session();
    let mut tally = Tally::default();
    if !allocator::start() {
        return Err("NAM session window did not arm cleanly");
    }
    script(&mut session, &mut tally);
    let injected = !inject || allocator::inject_core_allocation();
    let Some(counts) = allocator::stop() else {
        return Err("NAM session window did not disarm cleanly");
    };
    let discards = [
        Discard::WrongSender,
        Discard::WrongReceiver,
        Discard::IgnoredInState,
        Discard::UnexpectedIrs,
        Discard::UnmatchedResponse,
        Discard::MissingReference,
        Discard::ForeignReference,
        Discard::WrongUnitLrm,
    ]
    .into_iter()
    .fold(0u32, |n, r| n.saturating_add(discard_count(&session, r)));
    Ok(NamEvidence {
        counts,
        irq_sent: tally.irq_sent,
        atm_sent: tally.atm_sent,
        resent: tally.resent,
        timed_out: tally.timed_out,
        delivered: tally.delivered,
        responses: tally.responses,
        asm_sent: tally.asm_sent,
        peer_silent: tally.peer_silent,
        lrm_sent: tally.lrm_sent,
        trq_sent: tally.trq_sent,
        abandoned: tally.abandoned,
        peak_pending: tally.peak_pending,
        pending_full: refusal_count(&session, Refusal::PendingFull),
        discards,
        dropped: tally.dropped,
        script_errors: tally.script_errors,
        final_inactive: session.state == State::Inactive,
        injected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ: OA-NAM-035
    #[test]
    fn the_script_reaches_every_state_and_bound_it_claims() {
        // The zero-allocation claim is the harness run (libtest allocates in the background);
        // this checks that the script drives what the run reports.
        let mut s = new_session();
        let mut t = Tally::default();
        script(&mut s, &mut t);
        assert_eq!(t.script_errors, 0);
        assert_eq!(t.dropped, 0);
        assert_eq!(t.peak_pending, MAX_PENDING);
        assert_eq!(t.abandoned, MAX_PENDING as u64);
        assert_eq!(refusal_count(&s, Refusal::PendingFull), 1);
        assert_eq!(s.state, State::Inactive);
        assert!(t.resent > 0 && t.timed_out > 0 && t.peer_silent == 1 && t.lrm_sent == 1);
    }
}

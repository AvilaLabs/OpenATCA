//! The session state machine of NAS-IC-82422100 Figure 3-3, transition by transition, plus the
//! timeouts, message numbering, response bookkeeping and bounds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_nam::lrm::{lrm_entry, LRM_TABLE};
use oa_nam::session::*;
use oa_nam::workload::{bounds, PEAK_RATES, PER_FLIGHT};
use oa_nam::{Facility, Header, Kind, MsgId};

const US: Facility = Facility(*b"KZMP");
const THEM: Facility = Facility(*b"CZWG");

fn cfg() -> Config {
    default_config(US, THEM)
}

fn rx(kind: Kind, number: u16, reference: Option<u16>) -> Header {
    Header {
        kind,
        id: MsgId {
            sender: THEM,
            receiver: US,
            number,
        },
        reference: reference.map(|n| MsgId {
            sender: US,
            receiver: THEM,
            number: n,
        }),
    }
}

fn run(c: &Config, s: Session, ev: Event, now: Millis) -> (Session, Vec<Action>) {
    let (s, a) = step(c, s, ev, now);
    assert_eq!(a.dropped, 0);
    (s, a.as_slice().to_vec())
}

fn active(c: &Config) -> Session {
    let (s, a) = run(c, new_session(), Event::Activate, 0);
    assert_eq!(a, [Action::SendIrq { number: 0 }]);
    let (s, a) = run(c, s, Event::Received(rx(Kind::Irs, 0, Some(0))), 5);
    assert!(a.is_empty());
    assert_eq!(s.state, State::Active);
    s
}

// REQ: OA-NAM-010
#[test]
fn inactive_activate_sends_irq_and_goes_activating() {
    let (s, a) = run(&cfg(), new_session(), Event::Activate, 100);
    assert_eq!(s.state, State::Activating);
    assert_eq!(a, [Action::SendIrq { number: 0 }]);
    assert_eq!(s.irs_deadline, Some(100 + 10_000));
}

// REQ: OA-NAM-010
#[test]
fn inactive_ignores_everything_else() {
    let c = cfg();
    for ev in [
        Event::Deactivate,
        Event::Tick,
        Event::ConnectionLost,
        Event::Received(rx(Kind::Irs, 0, Some(0))),
        Event::Received(rx(Kind::Trq, 0, None)),
        Event::Received(rx(Kind::Fpl, 0, None)),
    ] {
        let (s, a) = run(&c, new_session(), ev, 0);
        assert_eq!(s.state, State::Inactive, "{ev:?}");
        assert!(
            a.iter().all(|x| matches!(x, Action::Discard { .. })),
            "{ev:?}: {a:?}"
        );
    }
    let (_, a) = run(&c, new_session(), Event::SendAtm(Kind::Fpl), 0);
    assert_eq!(
        a,
        [Action::Refuse {
            reason: Refusal::NotActive
        }]
    );
}

// REQ: OA-NAM-011
#[test]
fn activating_irs_for_our_irq_goes_active() {
    let c = cfg();
    let (s, _) = run(&c, new_session(), Event::Activate, 0);
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Irs, 0, Some(0))), 50);
    assert_eq!(s.state, State::Active);
    assert!(a.is_empty());
    assert_eq!(s.last_rx, 50);
    assert_eq!(s.irs_deadline, None);
}

// REQ: OA-NAM-011
#[test]
fn activating_irs_for_an_unknown_irq_is_discarded() {
    let c = cfg();
    let (s, _) = run(&c, new_session(), Event::Activate, 0);
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Irs, 0, Some(7))), 50);
    assert_eq!(s.state, State::Activating);
    assert_eq!(
        a,
        [Action::Discard {
            reason: Discard::UnexpectedIrs
        }]
    );
    let (_, a) = run(&c, s, Event::Received(rx(Kind::Irs, 1, None)), 50);
    assert_eq!(
        a,
        [Action::Discard {
            reason: Discard::UnexpectedIrs
        }]
    );
}

// REQ: OA-NAM-011
#[test]
fn activating_irs_timeout_resends_irq_with_a_new_number() {
    let c = cfg();
    let (s, _) = run(&c, new_session(), Event::Activate, 0);
    let (s, a) = run(&c, s, Event::Tick, 9_999);
    assert!(a.is_empty());
    let (s, a) = run(&c, s, Event::Tick, 10_000);
    assert_eq!(a, [Action::SendIrq { number: 1 }]);
    assert_eq!(s.irq_retries, 1);
    assert_eq!(s.irs_deadline, Some(20_000));
    // An IRS to the first IRQ is still accepted after the retry.
    let (s, _) = run(&c, s, Event::Received(rx(Kind::Irs, 0, Some(0))), 10_500);
    assert_eq!(s.state, State::Active);
}

// REQ: OA-NAM-011
#[test]
fn activating_irq_retry_can_reuse_the_number() {
    let mut c = cfg();
    c.irq_retry_same_number = true;
    let (s, _) = run(&c, new_session(), Event::Activate, 0);
    let (_, a) = run(&c, s, Event::Tick, 10_000);
    assert_eq!(a, [Action::SendIrq { number: 0 }]);
}

// REQ: OA-NAM-011
#[test]
fn activating_max_retries_sends_trq_and_goes_inactive() {
    let mut c = cfg();
    c.max_irq_retries = Some(2);
    let (mut s, _) = run(&c, new_session(), Event::Activate, 0);
    for i in 1..=2u64 {
        let (n, a) = run(&c, s, Event::Tick, i * 10_000);
        assert!(matches!(a[0], Action::SendIrq { .. }), "{a:?}");
        s = n;
    }
    let (s, a) = run(&c, s, Event::Tick, 30_000);
    assert_eq!(s.state, State::Inactive);
    assert_eq!(a, [Action::SendTrq { number: 3 }, Action::ActivationFailed]);
}

// REQ: OA-NAM-011
#[test]
fn activating_irq_received_answers_with_irq_then_irs() {
    let c = cfg();
    let (s, _) = run(&c, new_session(), Event::Activate, 0);
    let h = rx(Kind::Irq, 40, None);
    let (s, a) = run(&c, s, Event::Received(h), 10);
    assert_eq!(s.state, State::Activating);
    assert_eq!(
        a,
        [
            Action::SendIrq { number: 1 },
            Action::SendIrs {
                number: 2,
                reference: h.id
            }
        ]
    );
}

// REQ: OA-NAM-011
#[test]
fn activating_trq_received_answers_trs_and_stays() {
    let c = cfg();
    let (s, _) = run(&c, new_session(), Event::Activate, 0);
    let h = rx(Kind::Trq, 3, None);
    let (s, a) = run(&c, s, Event::Received(h), 10);
    assert_eq!(s.state, State::Activating);
    assert_eq!(
        a,
        [Action::SendTrs {
            number: 1,
            reference: h.id
        }]
    );
}

// REQ: OA-NAM-011
#[test]
fn activating_deactivate_sends_trq_and_goes_inactive() {
    let c = cfg();
    let (s, _) = run(&c, new_session(), Event::Activate, 0);
    let (s, a) = run(&c, s, Event::Deactivate, 5);
    assert_eq!(s.state, State::Inactive);
    assert_eq!(a, [Action::SendTrq { number: 1 }]);
    assert_eq!(s.irs_deadline, None);
}

// REQ: OA-NAM-011
#[test]
fn activating_other_messages_are_ignored() {
    let c = cfg();
    let (s, _) = run(&c, new_session(), Event::Activate, 0);
    for k in [Kind::Fpl, Kind::Asm, Kind::Lam, Kind::Trs] {
        let (n, a) = run(&c, s, Event::Received(rx(k, 1, Some(0))), 5);
        assert_eq!(n.state, State::Activating);
        assert_eq!(
            a,
            [Action::Discard {
                reason: Discard::IgnoredInState
            }],
            "{k:?}"
        );
    }
    let (_, a) = run(&c, s, Event::SendAtm(Kind::Fpl), 5);
    assert_eq!(
        a,
        [Action::Refuse {
            reason: Refusal::NotActive
        }]
    );
}

// REQ: OA-NAM-012
#[test]
fn active_idle_sends_asm_and_a_lam_clears_it() {
    let c = cfg();
    let s = active(&c);
    let (s, a) = run(&c, s, Event::Tick, 5 + 9_999);
    assert!(a.is_empty());
    let (s, a) = run(&c, s, Event::Tick, 5 + 10_000);
    assert_eq!(a, [Action::SendAsm { number: 1 }]);
    assert_eq!(s.asm, Some((1, 5 + 10_000 + 10_000)));
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Lam, 1, Some(1))), 10_100);
    assert_eq!(a, [Action::AsmAnswered]);
    assert_eq!(s.asm, None);
    assert_eq!(s.state, State::Active);
}

// REQ: OA-NAM-012
#[test]
fn active_asm_unanswered_sends_irq_and_goes_activating() {
    let c = cfg();
    let s = active(&c);
    let (s, _) = run(&c, s, Event::Tick, 10_005);
    let (s, a) = run(&c, s, Event::Tick, 20_005);
    assert_eq!(s.state, State::Activating);
    assert_eq!(a, [Action::PeerSilent, Action::SendIrq { number: 2 }]);
    assert_eq!(detection_time_ms(&c), 20_000);
    assert!(detection_time_ms(&c) <= u64::from(bounds::RECOVERY_S) * 1000);
}

// REQ: OA-NAM-012
#[test]
fn active_asm_received_answers_with_lam() {
    let c = cfg();
    let s = active(&c);
    let h = rx(Kind::Asm, 9, None);
    let (s, a) = run(&c, s, Event::Received(h), 100);
    assert_eq!(
        a,
        [Action::SendLam {
            number: 1,
            reference: h.id
        }]
    );
    assert_eq!(s.last_rx, 100);
    assert_eq!(s.state, State::Active);
}

// REQ: OA-NAM-012
#[test]
fn active_irq_received_answers_with_irs_and_stays() {
    let c = cfg();
    let s = active(&c);
    let h = rx(Kind::Irq, 9, None);
    let (s, a) = run(&c, s, Event::Received(h), 100);
    assert_eq!(
        a,
        [Action::SendIrs {
            number: 1,
            reference: h.id
        }]
    );
    assert_eq!(s.state, State::Active);
}

// REQ: OA-NAM-012
#[test]
fn active_irs_and_trs_received_have_no_effect() {
    let c = cfg();
    let s = active(&c);
    for k in [Kind::Irs, Kind::Trs] {
        let (n, a) = run(&c, s, Event::Received(rx(k, 9, Some(0))), 100);
        assert!(a.is_empty());
        assert_eq!(n.state, State::Active);
    }
}

// REQ: OA-NAM-012
#[test]
fn active_trq_received_answers_trs_then_irq_and_goes_activating() {
    let c = cfg();
    let s = active(&c);
    let (s, a) = run(&c, s, Event::SendAtm(Kind::Fpl), 10);
    assert_eq!(
        a,
        [Action::SendAtm {
            kind: Kind::Fpl,
            number: 1
        }]
    );
    let h = rx(Kind::Trq, 9, None);
    let (s, a) = run(&c, s, Event::Received(h), 100);
    assert_eq!(s.state, State::Activating);
    assert_eq!(
        a,
        [
            Action::SendTrs {
                number: 2,
                reference: h.id
            },
            Action::SendIrq { number: 3 }
        ]
    );
    // The unanswered FPL stays pending and times out normally (OA-NAM-023).
    assert_eq!(s.pending().len(), 1);
}

// REQ: OA-NAM-012
#[test]
fn active_deactivate_sends_trq_and_goes_deactivating() {
    let c = cfg();
    let s = active(&c);
    let (s, a) = run(&c, s, Event::Deactivate, 100);
    assert_eq!(s.state, State::Deactivating);
    assert_eq!(a, [Action::SendTrq { number: 1 }]);
    assert_eq!(s.trs_deadline, Some(10_100));
}

// REQ: OA-NAM-012
#[test]
fn active_connection_lost_goes_inactive() {
    let c = cfg();
    let s = active(&c);
    let (s, _) = run(&c, s, Event::SendAtm(Kind::Cpl), 10);
    let (s, a) = run(&c, s, Event::ConnectionLost, 20);
    assert_eq!(s.state, State::Inactive);
    assert_eq!(a, [Action::AbandonPending { count: 1 }]);
}

// REQ: OA-NAM-026
#[test]
fn reconnect_starts_a_fresh_inbound_sequence_and_abandons_old_requests() {
    let c = cfg();
    let (mut s, _) = run(&c, active(&c), Event::Received(rx(Kind::Fpl, 88, None)), 10);
    assert_eq!(s.rx_last_number, Some(88));
    let (next, _) = run(&c, s, Event::SendAtm(Kind::Cpl), 11);
    s = next;

    let (s, a) = run(&c, s, Event::ConnectionLost, 12);
    assert_eq!(s.state, State::Inactive);
    assert_eq!(s.rx_last_number, None);
    assert!(s.pending().is_empty());
    assert_eq!(a, [Action::AbandonPending { count: 1 }]);

    let (s, a) = run(&c, s, Event::Activate, 20);
    assert_eq!(a, [Action::SendIrq { number: 2 }]);
    let (s, _) = run(&c, s, Event::Received(rx(Kind::Irs, 90, Some(2))), 21);
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Fpl, 88, None)), 22);
    assert_eq!(s.rx_last_number, Some(88));
    assert!(matches!(
        a.as_slice(),
        [Action::Deliver {
            seq: Seq::First,
            ..
        }]
    ));
}

// REQ: OA-NAM-013
#[test]
fn deactivating_trs_goes_inactive() {
    let c = cfg();
    let (s, _) = run(&c, active(&c), Event::Deactivate, 100);
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Trs, 5, Some(1))), 200);
    assert_eq!(s.state, State::Inactive);
    assert!(a.is_empty());
}

// REQ: OA-NAM-013
#[test]
fn deactivating_trs_timeout_goes_inactive() {
    let c = cfg();
    let (s, _) = run(&c, active(&c), Event::Deactivate, 100);
    let (s, a) = run(&c, s, Event::Tick, 10_099);
    assert!(a.is_empty());
    assert_eq!(s.state, State::Deactivating);
    let (s, _) = run(&c, s, Event::Tick, 10_100);
    assert_eq!(s.state, State::Inactive);
}

// REQ: OA-NAM-013
#[test]
fn deactivating_trq_received_answers_trs_and_goes_inactive() {
    let c = cfg();
    let (s, _) = run(&c, active(&c), Event::Deactivate, 100);
    let h = rx(Kind::Trq, 5, None);
    let (s, a) = run(&c, s, Event::Received(h), 200);
    assert_eq!(s.state, State::Inactive);
    assert_eq!(
        a,
        [Action::SendTrs {
            number: 2,
            reference: h.id
        }]
    );
}

// REQ: OA-NAM-013
#[test]
fn deactivating_still_processes_responses_and_allows_sending_them() {
    let c = cfg();
    let s = active(&c);
    let (s, _) = run(&c, s, Event::SendAtm(Kind::Cpl), 10);
    let (s, _) = run(&c, s, Event::Deactivate, 20);
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Lam, 5, Some(1))), 30);
    assert!(matches!(
        a[0],
        Action::Response {
            answers: Kind::Cpl,
            ..
        }
    ));
    assert_eq!(s.state, State::Deactivating);
    let r = rx(Kind::Fpl, 6, None).id;
    let (_, a) = run(
        &c,
        s,
        Event::SendResponse {
            kind: Kind::Lam,
            reference: r,
        },
        40,
    );
    assert!(matches!(
        a[0],
        Action::SendResponse {
            kind: Kind::Lam,
            ..
        }
    ));
    let (_, a) = run(&c, s, Event::SendAtm(Kind::Fpl), 40);
    assert_eq!(
        a,
        [Action::Refuse {
            reason: Refusal::NotActive
        }]
    );
    let (_, a) = run(&c, s, Event::Received(rx(Kind::Fpl, 7, None)), 40);
    assert_eq!(
        a,
        [Action::Discard {
            reason: Discard::IgnoredInState
        }]
    );
}

// REQ: OA-NAM-013
#[test]
fn deactivating_abandons_pending_when_it_ends() {
    let c = cfg();
    let s = active(&c);
    let (s, _) = run(&c, s, Event::SendAtm(Kind::Cpl), 10);
    let (s, _) = run(&c, s, Event::Deactivate, 20);
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Trs, 5, Some(2))), 30);
    assert_eq!(a, [Action::AbandonPending { count: 1 }]);
    assert_eq!(s.state, State::Inactive);
}

// REQ: OA-NAM-014
#[test]
fn message_numbers_run_000_to_999_and_wrap() {
    let c = cfg();
    let mut s = active(&c);
    s.next_number = 998;
    let mut seen = vec![];
    for _ in 0..4 {
        let (n, a) = run(&c, s, Event::SendAtm(Kind::Rtu), 10);
        s = n;
        if let Action::SendAtm { number, .. } = a[0] {
            seen.push(number);
        }
    }
    assert_eq!(seen, [998, 999, 0, 1]);
}

// REQ: OA-NAM-014
#[test]
fn wrapped_number_still_pending_is_refused_without_advancing_sequence() {
    let c = cfg();
    let mut s = active(&c);
    let (next, a) = run(&c, s, Event::SendAtm(Kind::Cpl), 10);
    s = next;
    assert_eq!(
        a,
        [Action::SendAtm {
            kind: Kind::Cpl,
            number: 1
        }]
    );

    // Keep number 1 pending while the other 999 values make a full sequence turn.
    for i in 0..999 {
        let (next, a) = run(&c, s, Event::SendAtm(Kind::Rtu), 11 + i);
        assert_eq!(
            a,
            [Action::SendAtm {
                kind: Kind::Rtu,
                number: ((i + 2) % 1000) as u16
            }]
        );
        s = next;
    }
    assert_eq!(s.next_number, 1);
    assert_eq!(s.pending().len(), 1);

    let (s, a) = run(&c, s, Event::SendAtm(Kind::Cpl), 2_000);
    assert_eq!(
        a,
        [Action::Refuse {
            reason: Refusal::MessageNumberInUse
        }]
    );
    assert_eq!(s.next_number, 1, "refused work consumes no message number");
    assert_eq!(s.pending().len(), 1);
    assert_eq!(refusal_count(&s, Refusal::MessageNumberInUse), 1);
}

// REQ: OA-NAM-014
#[test]
fn inbound_sequence_is_classified() {
    let c = cfg();
    let mut s = active(&c);
    let mut seqs = vec![];
    for n in [10, 11, 11, 14, 0, 999, 0] {
        let (ns, a) = run(&c, s, Event::Received(rx(Kind::Fpl, n, None)), 100);
        s = ns;
        if let Action::Deliver { seq, .. } = a[0] {
            seqs.push(seq);
        }
    }
    assert_eq!(
        seqs,
        [
            Seq::First,
            Seq::InOrder,
            Seq::Duplicate,
            Seq::Gap(2),
            Seq::Gap(985),
            Seq::Gap(998),
            Seq::InOrder
        ]
    );
}

// REQ: OA-NAM-014, OA-NAM-024
#[test]
fn a_message_from_or_to_the_wrong_unit_is_refused_with_an_lrm() {
    let c = cfg();
    let s = active(&c);
    let mut h = rx(Kind::Fpl, 1, None);
    h.id.sender = Facility(*b"KZBW");
    let (s2, a) = run(&c, s, Event::Received(h), 100);
    assert_eq!(
        a,
        [
            Action::Discard {
                reason: Discard::WrongSender
            },
            Action::SendLrm {
                number: 1,
                reference: h.id,
                code: 1,
                field: 3
            }
        ]
    );
    assert_eq!(s2.rejected, 1);
    let mut h = rx(Kind::Fpl, 1, None);
    h.id.receiver = Facility(*b"KZBW");
    let (s3, a) = run(&c, s2, Event::Received(h), 100);
    assert_eq!(
        a,
        [
            Action::Discard {
                reason: Discard::WrongReceiver
            },
            Action::SendLrm {
                number: 2,
                reference: h.id,
                code: 2,
                field: 3
            }
        ]
    );
    assert_eq!(s3.rejected, 2);
    // An LRM from a wrong unit is counted and dropped, never answered with an LRM.
    let mut h = rx(Kind::Lrm, 1, Some(0));
    h.id.sender = Facility(*b"KZBW");
    let (s4, a) = run(&c, s3, Event::Received(h), 100);
    assert_eq!(
        a,
        [Action::Discard {
            reason: Discard::WrongUnitLrm
        }]
    );
    assert_eq!(s4.rejected, 3);
    // In INACTIVE it is ignored and still counted.
    let mut h = rx(Kind::Fpl, 1, None);
    h.id.sender = Facility(*b"KZBW");
    let (s5, a) = run(&c, new_session(), Event::Received(h), 100);
    assert_eq!(
        a,
        [Action::Discard {
            reason: Discard::WrongSender
        }]
    );
    assert_eq!(s5.rejected, 1);
}

// REQ: OA-NAM-015
#[test]
fn responses_match_the_message_they_answer() {
    let c = cfg();
    let s = active(&c);
    let (s, _) = run(&c, s, Event::SendAtm(Kind::Rti), 10);
    let (s, _) = run(&c, s, Event::SendAtm(Kind::Poi), 10);
    let (s, _) = run(&c, s, Event::SendAtm(Kind::Cpl), 10);
    let (s, _) = run(&c, s, Event::SendAtm(Kind::Rtu), 10);
    assert_eq!(s.pending().len(), 3, "RTU expects no response");
    // A LAM does not answer an RTI.
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Lam, 2, Some(1))), 20);
    assert_eq!(
        a,
        [Action::Discard {
            reason: Discard::UnmatchedResponse
        }]
    );
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Rla, 3, Some(1))), 20);
    assert!(matches!(
        a[0],
        Action::Response {
            answers: Kind::Rti,
            ..
        }
    ));
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Pla, 4, Some(2))), 20);
    assert!(matches!(
        a[0],
        Action::Response {
            answers: Kind::Poi,
            ..
        }
    ));
    // An LRM answers anything.
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Lrm, 5, Some(3))), 20);
    assert!(matches!(
        a[0],
        Action::Response {
            answers: Kind::Cpl,
            ..
        }
    ));
    assert!(s.pending().is_empty());
    // A response to nothing, and one without a reference, are discarded.
    let (_, a) = run(&c, s, Event::Received(rx(Kind::Lam, 6, Some(9))), 20);
    assert_eq!(
        a,
        [Action::Discard {
            reason: Discard::UnmatchedResponse
        }]
    );
    let (_, a) = run(&c, s, Event::Received(rx(Kind::Lam, 6, None)), 20);
    assert_eq!(
        a,
        [Action::Discard {
            reason: Discard::MissingReference
        }]
    );
}

// REQ: OA-NAM-015
#[test]
fn an_unanswered_message_times_out_and_is_reported() {
    let c = cfg();
    let s = active(&c);
    let (mut s, _) = run(&c, s, Event::SendAtm(Kind::Cpl), 0);
    // Keep the session alive with traffic so only the response timer fires.
    s.last_rx = 59_000;
    let (s, a) = run(&c, s, Event::Tick, 59_999);
    assert!(a.is_empty());
    let (s, a) = run(&c, s, Event::Tick, 60_000);
    assert_eq!(
        a,
        [Action::ResponseTimeout {
            kind: Kind::Cpl,
            number: 1
        }]
    );
    assert!(s.pending().is_empty());
}

// REQ: OA-NAM-015
#[test]
fn an_unanswered_message_is_resent_with_its_number_when_configured() {
    let mut c = cfg();
    c.max_resends = 1;
    let s = active(&c);
    let (mut s, _) = run(&c, s, Event::SendAtm(Kind::Cpl), 0);
    s.last_rx = 59_000;
    let (mut s, a) = run(&c, s, Event::Tick, 60_000);
    assert_eq!(
        a,
        [Action::ResendAtm {
            kind: Kind::Cpl,
            number: 1
        }]
    );
    s.last_rx = 119_000;
    let (_, a) = run(&c, s, Event::Tick, 120_000);
    assert_eq!(
        a,
        [Action::ResponseTimeout {
            kind: Kind::Cpl,
            number: 1
        }]
    );
}

// REQ: OA-NAM-015
#[test]
fn pending_table_is_bounded_and_refuses_when_full() {
    let c = cfg();
    let mut s = active(&c);
    for _ in 0..MAX_PENDING {
        let (n, a) = run(&c, s, Event::SendAtm(Kind::Cpl), 10);
        assert!(matches!(a[0], Action::SendAtm { .. }));
        s = n;
    }
    let before = s.next_number;
    let (s, a) = run(&c, s, Event::SendAtm(Kind::Cpl), 10);
    assert_eq!(
        a,
        [Action::Refuse {
            reason: Refusal::PendingFull
        }]
    );
    assert_eq!(s.next_number, before, "nothing was numbered");
    // RTU needs no slot and still goes.
    let (_, a) = run(&c, s, Event::SendAtm(Kind::Rtu), 10);
    assert!(matches!(
        a[0],
        Action::SendAtm {
            kind: Kind::Rtu,
            ..
        }
    ));
}

// REQ: OA-NAM-015
#[test]
fn many_timeouts_fit_the_action_list_and_the_rest_wait() {
    let c = cfg();
    let mut s = active(&c);
    for _ in 0..20 {
        s = run(&c, s, Event::SendAtm(Kind::Cpl), 0).0;
    }
    s.last_rx = 59_000;
    let (s, a) = run(&c, s, Event::Tick, 60_000);
    assert_eq!(a.len(), MAX_ACTIONS);
    assert_eq!(s.pending().len(), 20 - MAX_ACTIONS);
}

// REQ: OA-NAM-016
#[test]
fn send_requests_are_checked() {
    let c = cfg();
    let s = active(&c);
    for k in [Kind::Irq, Kind::Asm, Kind::Lam, Kind::Lrm] {
        let (_, a) = run(&c, s, Event::SendAtm(k), 0);
        assert_eq!(
            a,
            [Action::Refuse {
                reason: Refusal::NotAnAtmMessage
            }],
            "{k:?}"
        );
    }
    let r = rx(Kind::Fpl, 1, None).id;
    let (_, a) = run(
        &c,
        s,
        Event::SendResponse {
            kind: Kind::Fpl,
            reference: r,
        },
        0,
    );
    assert_eq!(
        a,
        [Action::Refuse {
            reason: Refusal::NotAResponse
        }]
    );
    let (_, a) = run(
        &c,
        new_session(),
        Event::SendResponse {
            kind: Kind::Lam,
            reference: r,
        },
        0,
    );
    assert_eq!(
        a,
        [Action::Refuse {
            reason: Refusal::ResponseNotAllowed
        }]
    );
}

// REQ: OA-NAM-016
#[test]
fn next_deadline_follows_the_running_timers() {
    let c = cfg();
    assert_eq!(next_deadline(&c, &new_session()), None);
    let (s, _) = run(&c, new_session(), Event::Activate, 0);
    assert_eq!(next_deadline(&c, &s), Some(10_000));
    let s = active(&c);
    assert_eq!(next_deadline(&c, &s), Some(5 + 10_000));
    let (s, _) = run(&c, s, Event::SendAtm(Kind::Cpl), 6);
    assert_eq!(next_deadline(&c, &s), Some(5 + 10_000));
    let (s, _) = run(&c, s, Event::Deactivate, 7);
    assert_eq!(next_deadline(&c, &s), Some(10_007));
}

// REQ: OA-NAM-016
#[test]
fn time_is_an_input_and_the_step_is_deterministic() {
    let c = cfg();
    let a = run(&c, active(&c), Event::Tick, 12_345);
    let b = run(&c, active(&c), Event::Tick, 12_345);
    assert_eq!(a.0, b.0);
    assert_eq!(a.1, b.1);
    // Saturating time arithmetic: no overflow near the end of the clock.
    let (_, _) = run(&c, new_session(), Event::Activate, u64::MAX);
}

// REQ: OA-NAM-017
#[test]
fn lrm_table_matches_the_icd() {
    // Table A-1 has codes 1 to 91, Attachment 3 adds 92 to 94, Attachment 2 adds 99 to 116.
    assert_eq!(LRM_TABLE.len(), 91 + 3 + 18);
    for w in LRM_TABLE.windows(2) {
        assert!(w[0].code < w[1].code);
    }
    assert_eq!(lrm_entry(6).unwrap().text, "INVALID ACID");
    assert_eq!(lrm_entry(60).unwrap().field, "03");
    assert_eq!(lrm_entry(116).unwrap().text, "FLIGHT NOT BEACON EQUIPPED");
    assert!(lrm_entry(95).is_none());
    // Every code the codec can produce is in the table.
    for k in [
        oa_nam::ErrorKind::Acid,
        oa_nam::ErrorKind::Mnemonic,
        oa_nam::ErrorKind::Length,
        oa_nam::ErrorKind::Parens,
        oa_nam::ErrorKind::Eet,
    ] {
        let (code, _) = oa_nam::error::lrm_code(k);
        assert!(lrm_entry(code).is_some(), "{k:?} -> {code}");
    }
}

// REQ: OA-NAM-018
#[test]
fn workload_table_is_consistent_with_the_icd_arithmetic() {
    // Table 3-IV: the RTU peak is the handoff rate times 13.2 per handoff.
    let rtu = PEAK_RATES.iter().find(|p| p.kind == Kind::Rtu).unwrap();
    let rti = PEAK_RATES.iter().find(|p| p.kind == Kind::Rti).unwrap();
    assert!((rti.artcc_per_min * 13.2 - rtu.artcc_per_min).abs() < 1e-9);
    assert!((rti.en_route_per_min * 13.2 - rtu.en_route_per_min).abs() < 1e-9);
    // Footnote: every 10 s for 2.2 min.
    assert!((CAATS_PER_HANDOFF - 13.2).abs() < 1e-9);
    // Table 29: 5 RTUs per flight is one every 6 s for 30 s.
    let rtu = PER_FLIGHT.iter().find(|p| p.kind == Kind::Rtu).unwrap();
    assert_eq!(rtu.per_flight, 5.0);
    assert_eq!(
        oa_nam::workload::TABLE29_RTU_DURATION_S / oa_nam::workload::TABLE29_RTU_INTERVAL_S,
        5
    );
    // The smallest message is the IRQ, 17 bytes.
    assert_eq!(bounds::MIN_MESSAGE_BYTES as usize, oa_nam::MIN_MESSAGE_LEN);
    assert_eq!(bounds::MAX_MESSAGE_BYTES as usize, oa_nam::MAX_MESSAGE_LEN);
    // The pending table covers a full minute of answered traffic at the all-En-Route peak.
    let answered: f64 = PEAK_RATES
        .iter()
        .filter(|p| expects_response(p.kind))
        .map(|p| p.en_route_per_min)
        .sum();
    assert!(answered < MAX_PENDING as f64 / 2.0, "{answered}");
}

const CAATS_PER_HANDOFF: f64 = 6.0 * 2.2;

// REQ: OA-NAM-022
#[test]
fn irq_in_inactive_follows_b16_and_the_ird_figure_is_an_option() {
    let c = cfg();
    let h = rx(Kind::Irq, 5, None);
    let (s, a) = run(&c, new_session(), Event::Received(h), 10);
    assert_eq!(a, [Action::InitRequested { header: h }]);
    assert_eq!(s.state, State::Inactive);
    let mut c2 = cfg();
    c2.inactive_ignores_irq = true;
    let (s, a) = run(&c2, new_session(), Event::Received(h), 10);
    assert_eq!(
        a,
        [Action::Discard {
            reason: Discard::IgnoredInState
        }]
    );
    assert_eq!(s.state, State::Inactive);
}

// REQ: OA-NAM-023
#[test]
fn pending_survive_reactivation_and_time_out_normally() {
    let c = cfg();
    let s = active(&c);
    let (s, _) = run(&c, s, Event::SendAtm(Kind::Cpl), 10);
    // Silence: ASM, then no LAM, then ACTIVATING with the CPL still pending.
    let (s, _) = run(&c, s, Event::Tick, 10_005);
    let (s, a) = run(&c, s, Event::Tick, 20_005);
    assert_eq!(s.state, State::Activating);
    assert_eq!(a, [Action::PeerSilent, Action::SendIrq { number: 3 }]);
    assert_eq!(s.pending().len(), 1);
    // It times out while ACTIVATING, and the pending deadline is a timer there.
    assert_eq!(next_deadline(&c, &s), Some(30_005));
    let (s, a) = run(&c, s, Event::Tick, 60_010);
    assert!(
        a.contains(&Action::ResponseTimeout {
            kind: Kind::Cpl,
            number: 1
        }),
        "{a:?}"
    );
    assert!(s.pending().is_empty());
}

// REQ: OA-NAM-023
#[test]
fn a_response_after_reactivation_still_matches() {
    let c = cfg();
    let s = active(&c);
    let (s, _) = run(&c, s, Event::SendAtm(Kind::Cpl), 10);
    let (s, _) = run(&c, s, Event::Received(rx(Kind::Trq, 9, None)), 100);
    assert_eq!(s.state, State::Activating);
    let (s, _) = run(&c, s, Event::Received(rx(Kind::Irs, 10, Some(3))), 120);
    assert_eq!(s.state, State::Active);
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Lam, 11, Some(1))), 130);
    assert!(matches!(
        a[0],
        Action::Response {
            answers: Kind::Cpl,
            ..
        }
    ));
    assert!(s.pending().is_empty());
}

// REQ: OA-NAM-025
#[test]
fn an_lrm_that_answers_no_pending_message_reaches_the_application() {
    let c = cfg();
    let s = active(&c);
    // An RTU expects no response, but the peer may reject it with an LRM.
    let (s, _) = run(&c, s, Event::SendAtm(Kind::Rtu), 10);
    let h = rx(Kind::Lrm, 3, Some(1));
    let (s, a) = run(&c, s, Event::Received(h), 20);
    assert_eq!(a, [Action::UnmatchedLrm { header: h }]);
    assert_eq!(h.reference.unwrap().number, 1);
    // Also one for a message that already timed out or never existed.
    let h = rx(Kind::Lrm, 4, Some(77));
    let (_, a) = run(&c, s, Event::Received(h), 20);
    assert_eq!(a, [Action::UnmatchedLrm { header: h }]);
    // A LAM for nothing is still discarded.
    let (_, a) = run(&c, s, Event::Received(rx(Kind::Lam, 5, Some(77))), 20);
    assert_eq!(
        a,
        [Action::Discard {
            reason: Discard::UnmatchedResponse
        }]
    );
}

// REQ: OA-NAM-025
#[test]
fn an_lrm_for_our_asm_clears_it_and_is_delivered() {
    let c = cfg();
    let s = active(&c);
    let (s, _) = run(&c, s, Event::Tick, 10_005);
    assert!(s.asm.is_some());
    let h = rx(Kind::Lrm, 3, Some(1));
    let (s, a) = run(&c, s, Event::Received(h), 10_100);
    assert_eq!(a, [Action::UnmatchedLrm { header: h }]);
    assert_eq!(s.asm, None);
}

// REQ: OA-NAM-026
#[test]
fn a_foreign_reference_is_named_as_such() {
    let c = cfg();
    let s = active(&c);
    let mut h = rx(Kind::Lam, 3, Some(1));
    h.reference = Some(MsgId {
        sender: Facility(*b"KZBW"),
        receiver: THEM,
        number: 1,
    });
    let (_, a) = run(&c, s, Event::Received(h), 20);
    assert_eq!(
        a,
        [Action::Discard {
            reason: Discard::ForeignReference
        }]
    );
}

// REQ: OA-NAM-026
#[test]
fn only_processed_messages_count_for_sequence_checking() {
    let c = cfg();
    let (s, _) = run(&c, new_session(), Event::Activate, 0);
    // Ignored in ACTIVATING: does not set the counter.
    let (s, _) = run(&c, s, Event::Received(rx(Kind::Fpl, 500, None)), 5);
    assert_eq!(s.rx_last_number, None);
    let (s, _) = run(&c, s, Event::Received(rx(Kind::Irs, 7, Some(0))), 6);
    assert_eq!(s.state, State::Active);
    assert_eq!(s.rx_last_number, None);
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Fpl, 8, None)), 7);
    assert!(matches!(
        a[0],
        Action::Deliver {
            seq: Seq::First,
            ..
        }
    ));
    assert_eq!(s.rx_last_number, Some(8));
}

// REQ: OA-NAM-026
#[test]
fn an_asm_lam_is_processed_in_deactivating() {
    let c = cfg();
    let s = active(&c);
    let (s, _) = run(&c, s, Event::Tick, 10_005);
    let (s, _) = run(&c, s, Event::Deactivate, 10_100);
    assert_eq!(s.state, State::Deactivating);
    let (s, a) = run(&c, s, Event::Received(rx(Kind::Lam, 3, Some(1))), 10_200);
    assert_eq!(a, [Action::AsmAnswered]);
    assert_eq!(s.asm, None);
}

// REQ: OA-NAM-027
#[test]
fn profile_sets_the_irq_retry_number_rule() {
    use oa_nam::Profile;
    let a = Config::for_profile(Profile::EramAttachment2, US, THEM);
    let b = Config::for_profile(Profile::Core, US, THEM);
    assert!(
        !a.irq_retry_same_number,
        "Attachment 2 2.10: the number increments"
    );
    assert!(b.irq_retry_same_number, "Rev G B.1.6: the same number");
    assert_eq!(default_config(US, THEM), a);
    let (s, _) = run(&b, new_session(), Event::Activate, 0);
    let (_, act) = run(&b, s, Event::Tick, 10_000);
    assert_eq!(act, [Action::SendIrq { number: 0 }]);
}

// REQ: OA-NAM-033
#[test]
fn message_number_arithmetic_cannot_overflow_from_any_field_value() {
    let c = cfg();
    // A hand-built session with the largest next_number: reduced modulo 1000 first.
    let mut s = active(&c);
    s.next_number = u16::MAX;
    let (s, a) = run(
        &c,
        s,
        Event::SendResponse {
            kind: Kind::Lam,
            reference: rx(Kind::Fpl, 1, None).id,
        },
        10,
    );
    assert_eq!(a.len(), 1);
    assert!(s.next_number < NUMBER_MODULUS);
    // A received message with the largest number, after one with the largest number.
    let mut s = active(&c);
    for _ in 0..3 {
        let (ns, a) = run(&c, s, Event::Received(rx(Kind::Fpl, u16::MAX, None)), 20);
        s = ns;
        assert_eq!(a.len(), 1);
    }
    let (_, a) = run(&c, s, Event::Received(rx(Kind::Fpl, 0, None)), 30);
    assert!(matches!(a[0], Action::Deliver { .. }));
}

// REQ: OA-NAM-034
#[test]
fn every_discard_and_refusal_is_counted_under_its_reason() {
    let c = cfg();
    let s = new_session();
    // INACTIVE: an FPL is ignored, a send is refused.
    let (s, _) = run(&c, s, Event::Received(rx(Kind::Fpl, 1, None)), 1);
    assert_eq!(discard_count(&s, Discard::IgnoredInState), 1);
    let (s, _) = run(&c, s, Event::SendAtm(Kind::Fpl), 2);
    assert_eq!(refusal_count(&s, Refusal::NotActive), 1);
    let (s, _) = run(&c, s, Event::Received(rx(Kind::Fpl, 2, None)), 3);
    assert_eq!(discard_count(&s, Discard::IgnoredInState), 2);
    // ACTIVE: an unmatched response and a response with no reference.
    let mut s = active(&c);
    let (n, _) = run(&c, s, Event::Received(rx(Kind::Lam, 5, Some(77))), 10);
    s = n;
    assert_eq!(discard_count(&s, Discard::UnmatchedResponse), 1);
    let (n, _) = run(&c, s, Event::Received(rx(Kind::Lam, 6, None)), 11);
    s = n;
    assert_eq!(discard_count(&s, Discard::MissingReference), 1);
    let (s, _) = run(&c, s, Event::SendAtm(Kind::Lam), 12);
    assert_eq!(refusal_count(&s, Refusal::NotAnAtmMessage), 1);
    // Reasons that did not occur stay at zero; every other counter is untouched.
    assert_eq!(discard_count(&s, Discard::WrongSender), 0);
    assert_eq!(refusal_count(&s, Refusal::PendingFull), 0);
}

// REQ: OA-NAM-034
#[test]
fn a_full_pending_table_is_counted_and_the_counters_saturate() {
    let c = cfg();
    let mut s = active(&c);
    for i in 0..MAX_PENDING {
        let (n, a) = run(&c, s, Event::SendAtm(Kind::Fpl), 10 + i as u64);
        s = n;
        assert!(matches!(a[0], Action::SendAtm { .. }));
    }
    let (s, a) = run(&c, s, Event::SendAtm(Kind::Fpl), 500);
    assert_eq!(
        a[0],
        Action::Refuse {
            reason: Refusal::PendingFull
        }
    );
    assert_eq!(refusal_count(&s, Refusal::PendingFull), 1);
}

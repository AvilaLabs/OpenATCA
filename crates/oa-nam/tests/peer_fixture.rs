//! A deterministic offline peer: canonical wire messages, the existing framing, and `step`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_nam::frame::{frame, unframe};
use oa_nam::session::*;
use oa_nam::{bare, encode, parse, Facility, Header, Kind, Message, MsgId};

const LOCAL: Facility = Facility(*b"KZMP");
const PEER: Facility = Facility(*b"CZWG");
const FPL_TEMPLATE: &str = "(FPLCZUL/KZBW043-N12345-IG-C172/L-SD/C-CYST2055-N0120A070 CYSC DCT YSC V447 MPV DCT KMPV-KMPV0053-EET/KZBW0003)";

fn config() -> Config {
    let mut c = default_config(LOCAL, PEER);
    c.asm_idle_ms = 1_000_000;
    c.asm_response_ms = 100;
    c.irs_timeout_ms = 100;
    c.trs_timeout_ms = 100;
    c.response_timeout_ms = 100;
    c
}

/// Encode, frame, unframe, parse and re-encode one canonical simulated wire message.
fn wire(message: Message) -> Header {
    let payload = encode(&message).unwrap();
    let bytes = frame(&payload).unwrap();
    let (unframed, consumed) = unframe(&bytes).unwrap().unwrap();
    assert_eq!(consumed, bytes.len());
    let parsed = parse(unframed).unwrap();
    assert_eq!(encode(&parsed).unwrap(), payload);
    parsed.header
}

fn wire_header(header: Header) -> Header {
    wire(bare(header))
}

fn wire_fpl(sender: Facility, receiver: Facility, number: u16) -> Header {
    let mut message = parse(FPL_TEMPLATE.as_bytes()).unwrap();
    message.header.id = MsgId {
        sender,
        receiver,
        number,
    };
    wire(message)
}

fn received_fpl(c: &Config, s: Session, number: u16, now: Millis) -> (Session, Seq) {
    let header = wire_fpl(c.peer, c.local, number);
    let (s, actions) = step(c, s, Event::Received(header), now);
    assert_eq!(actions.dropped, 0);
    match actions.as_slice() {
        [Action::Deliver {
            header: delivered,
            seq,
        }] => {
            assert_eq!(*delivered, header);
            (s, *seq)
        }
        other => panic!("expected one delivered FPL, got {other:?}"),
    }
}

fn active(c: &Config) -> Session {
    let (s, actions) = step(c, new_session(), Event::Activate, 0);
    assert_eq!(actions.as_slice(), [Action::SendIrq { number: 0 }]);
    let irq = wire_header(Header {
        kind: Kind::Irq,
        id: MsgId {
            sender: c.local,
            receiver: c.peer,
            number: 0,
        },
        reference: None,
    });
    assert_eq!(irq.kind, Kind::Irq);

    // Declared peer response: IRS references the exact IRQ identifier we just framed.
    let irs = wire_header(Header {
        kind: Kind::Irs,
        id: MsgId {
            sender: c.peer,
            receiver: c.local,
            number: 0,
        },
        reference: Some(irq.id),
    });
    let (s, actions) = step(c, s, Event::Received(irs), 1);
    assert!(actions.as_slice().is_empty());
    assert_eq!(s.state, State::Active);
    s
}

#[test]
fn stalled_peer_times_out_in_bounded_batches_then_reconnects() {
    let c = config();
    let mut s = active(&c);

    // Ten FPL requests reach the simulated peer over canonical framed messages. The peer stalls
    // and sends no LAM/LRM; each request remains pending until its stated response deadline.
    for offset in 0..10u64 {
        let now = 10 + offset;
        let (next, actions) = step(&c, s, Event::SendAtm(Kind::Fpl), now);
        let Action::SendAtm {
            kind: Kind::Fpl,
            number,
        } = actions.as_slice()[0]
        else {
            panic!("expected FPL send action, got {:?}", actions.as_slice());
        };
        assert_eq!(number, offset as u16 + 1);
        let sent = wire_fpl(c.local, c.peer, number);
        assert_eq!((sent.kind, sent.id.number), (Kind::Fpl, number));
        assert_eq!(actions.dropped, 0);
        s = next;
    }
    assert_eq!(s.pending().len(), 10);
    assert_eq!(next_deadline(&c, &s), Some(110));

    let (next, actions) = step(&c, s, Event::Tick, 200);
    assert_eq!(actions.dropped, 0);
    assert_eq!(actions.as_slice().len(), MAX_ACTIONS);
    assert_eq!(
        actions
            .as_slice()
            .iter()
            .filter_map(|action| match action {
                Action::ResponseTimeout { number, .. } => Some(*number),
                _ => None,
            })
            .collect::<Vec<_>>(),
        (1..=8).collect::<Vec<u16>>()
    );
    assert!(actions.as_slice().iter().all(|action| matches!(
        action,
        Action::ResponseTimeout {
            kind: Kind::Fpl,
            ..
        }
    )));
    assert_eq!(next.pending().len(), 2);

    let (s, actions) = step(&c, next, Event::Tick, 200);
    assert_eq!(actions.dropped, 0);
    assert_eq!(actions.as_slice().len(), 2);
    assert_eq!(
        actions
            .as_slice()
            .iter()
            .filter_map(|action| match action {
                Action::ResponseTimeout { number, .. } => Some(*number),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [9, 10]
    );
    assert!(actions.as_slice().iter().all(|action| matches!(
        action,
        Action::ResponseTimeout {
            kind: Kind::Fpl,
            ..
        }
    )));
    assert!(s.pending().is_empty());

    // A further request is abandoned on transport loss. Activation restarts with a fresh inbound
    // sequence baseline; a late logical response to the abandoned request is explicitly discarded.
    let (next, actions) = step(&c, s, Event::SendAtm(Kind::Fpl), 300);
    let Action::SendAtm {
        number: old_number, ..
    } = actions.as_slice()[0]
    else {
        panic!("expected final FPL request");
    };
    let sent = wire_fpl(c.local, c.peer, old_number);
    assert_eq!(sent.id.number, old_number);
    s = next;

    let (next, actions) = step(&c, s, Event::ConnectionLost, 301);
    assert_eq!(next.state, State::Inactive);
    assert_eq!(actions.as_slice(), [Action::AbandonPending { count: 1 }]);
    assert!(next.pending().is_empty());
    assert_eq!(next.rx_last_number, None);

    let (next, actions) = step(&c, next, Event::Activate, 310);
    assert_eq!(
        actions.as_slice(),
        [Action::SendIrq {
            number: old_number + 1
        }]
    );
    let irq = wire_header(Header {
        kind: Kind::Irq,
        id: MsgId {
            sender: c.local,
            receiver: c.peer,
            number: old_number + 1,
        },
        reference: None,
    });
    let irs = wire_header(Header {
        kind: Kind::Irs,
        id: MsgId {
            sender: c.peer,
            receiver: c.local,
            number: 77,
        },
        reference: Some(irq.id),
    });
    let (s, actions) = step(&c, next, Event::Received(irs), 311);
    assert!(actions.as_slice().is_empty());
    assert_eq!(s.state, State::Active);

    let late_lam = wire_header(Header {
        kind: Kind::Lam,
        id: MsgId {
            sender: c.peer,
            receiver: c.local,
            number: 78,
        },
        reference: Some(MsgId {
            sender: c.local,
            receiver: c.peer,
            number: old_number,
        }),
    });
    let (s, actions) = step(&c, s, Event::Received(late_lam), 312);
    assert_eq!(
        actions.as_slice(),
        [Action::Discard {
            reason: Discard::UnmatchedResponse
        }]
    );
    assert_eq!(discard_count(&s, Discard::UnmatchedResponse), 1);

    // The restarted peer sends one FPL, repeats it, skips one number, then sends a lower number.
    // The current modulo classifier labels that final message Gap(998), while preserving delivery.
    let (s, seq) = received_fpl(&c, s, 40, 320);
    assert_eq!(seq, Seq::First);
    let (s, seq) = received_fpl(&c, s, 40, 321);
    assert_eq!(seq, Seq::Duplicate);
    let (s, seq) = received_fpl(&c, s, 42, 322);
    assert_eq!(seq, Seq::Gap(1));
    let (s, seq) = received_fpl(&c, s, 41, 323);
    assert_eq!(seq, Seq::Gap(998));
    assert_eq!(s.rx_last_number, Some(41));
}

//! Synthetic local connection-generation ingress checks; no sockets or peer.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_nam::session::{default_config, new_session, step, Action, Event, State};
use oa_nam::shell::transport_epoch::{
    step_tagged, IngressRefusal, TaggedTransportEvent, TransportEvent,
};
use oa_nam::{Facility, Header, Kind, MsgId};

const LOCAL: Facility = Facility(*b"KZMP");
const PEER: Facility = Facility(*b"CZWG");
const OLD_GENERATION: u64 = 40;
const CURRENT_GENERATION: u64 = 41;

fn id(sender: Facility, receiver: Facility, number: u16) -> MsgId {
    MsgId {
        sender,
        receiver,
        number,
    }
}

fn received(kind: Kind, number: u16, reference: Option<MsgId>) -> Header {
    Header {
        kind,
        id: id(PEER, LOCAL, number),
        reference,
    }
}

fn activate(
    c: &oa_nam::session::Config,
    s: oa_nam::session::Session,
    generation: u64,
) -> oa_nam::session::Session {
    let (s, actions) = step(c, s, Event::Activate, 0);
    assert_eq!(actions.as_slice(), [Action::SendIrq { number: 0 }]);
    let irs = received(Kind::Irs, 0, Some(id(LOCAL, PEER, 0)));
    let (s, actions) = step_tagged(
        c,
        s,
        generation,
        TaggedTransportEvent {
            generation,
            event: TransportEvent::Received(irs),
        },
        1,
    )
    .unwrap();
    assert!(actions.as_slice().is_empty());
    assert_eq!(s.state, State::Active);
    s
}

#[test]
fn old_connection_response_and_disconnect_cannot_affect_fresh_session() {
    let c = default_config(LOCAL, PEER);

    // Old process: it issues FPL 001, then stops while a response is still queued locally.
    let old = activate(&c, new_session(), OLD_GENERATION);
    let (_old, actions) = step(&c, old, Event::SendAtm(Kind::Fpl), 2);
    assert_eq!(
        actions.as_slice(),
        [Action::SendAtm {
            kind: Kind::Fpl,
            number: 1
        }]
    );
    let old_lam = received(Kind::Lam, 1, Some(id(LOCAL, PEER, 1)));

    // A fresh session restarts its wire numbering. The caller has separately advanced its local
    // generation, so current FPL 001 is not confused with the queued old LAM for FPL 001.
    let current = activate(&c, new_session(), CURRENT_GENERATION);
    let (current, actions) = step(&c, current, Event::SendAtm(Kind::Fpl), 2);
    assert_eq!(
        actions.as_slice(),
        [Action::SendAtm {
            kind: Kind::Fpl,
            number: 1
        }]
    );
    assert_eq!(current.pending().len(), 1);
    let before = current;

    let stale_response = TaggedTransportEvent {
        generation: OLD_GENERATION,
        event: TransportEvent::Received(old_lam),
    };
    assert_eq!(
        step_tagged(&c, current, CURRENT_GENERATION, stale_response, 3),
        Err(IngressRefusal::StaleGeneration {
            current: CURRENT_GENERATION,
            event: OLD_GENERATION,
        })
    );
    assert_eq!(
        current, before,
        "refusal leaves the caller's session untouched"
    );
    assert_eq!(current.pending().len(), 1);

    // A late loss callback from the old socket cannot abandon the current request either.
    assert_eq!(
        step_tagged(
            &c,
            current,
            CURRENT_GENERATION,
            TaggedTransportEvent {
                generation: OLD_GENERATION,
                event: TransportEvent::ConnectionLost,
            },
            3,
        ),
        Err(IngressRefusal::StaleGeneration {
            current: CURRENT_GENERATION,
            event: OLD_GENERATION,
        })
    );
    assert_eq!(current.state, State::Active);
    assert_eq!(current.pending().len(), 1);

    // The same wire response tagged by the current connection passes the fence and resolves FPL.
    let (current, actions) = step_tagged(
        &c,
        current,
        CURRENT_GENERATION,
        TaggedTransportEvent {
            generation: CURRENT_GENERATION,
            event: TransportEvent::Received(old_lam),
        },
        4,
    )
    .unwrap();
    assert!(matches!(
        actions.as_slice(),
        [Action::Response {
            answers: Kind::Fpl,
            ..
        }]
    ));
    assert!(current.pending().is_empty());
}

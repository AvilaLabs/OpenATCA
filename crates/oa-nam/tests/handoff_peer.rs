//! Two-facility handoff and flight-plan exchanges over encoded, framed NAM bytes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_nam::frame::{frame, unframe};
use oa_nam::session::*;
use oa_nam::{bare, encode, parse, Facility, Field31, Header, Kind, MsgId};

const KZMP: Facility = Facility(*b"KZMP");
const CZWG: Facility = Facility(*b"CZWG");
const RTI: &str = "(RTIKZMP/CZWG812KZMP/CZWG801-DLH499/A3407-KMSP-CYOW-CZWG-13242934462034N0780521WN043327629F349)";
const RLA: &str = "(RLAKZBW/CZUL202CZUL/KZBW445-KBTV1A)";
const RTA: &str = "(RTACZWG/KZMP438KZMP/CZWG812-DLH499/A4222-KZMP-CYOW-CZWGAA)";
const LAM: &str = "(LAMKZHU/MMTY035MMTY/KZHU021)";
const LRM: &str = "(LRMKZLC/CZWG035CZWG/KZLC021-RMK/06/07/AAL98295)";
const FPL: &str = "(FPLCZUL/KZBW043-N12345-IG-C172/L-SD/C-CYST2055-N0120A070 CYSC DCT YSC V447 MPV DCT KMPV-KMPV0053-EET/KZBW0003)";
const EST: &str = "(ESTKZMP/CZWG992KZMP/CZWG991-DAL122/A4322-KFAR-HML/2042F350-CYOW)";
const CHG_AFTER_EST: &str = "(CHGKZMP/CZWG776KZMP/CZWG603-UAL1021-KSEA-CYOW-07/AAL73)";

fn config(local: Facility, peer: Facility) -> Config {
    let mut c = default_config(local, peer);
    c.asm_idle_ms = 1_000_000;
    c.asm_response_ms = 100;
    c.irs_timeout_ms = 100;
    c.trs_timeout_ms = 100;
    c
}

fn id(sender: Facility, receiver: Facility, number: u16) -> MsgId {
    MsgId {
        sender,
        receiver,
        number,
    }
}

fn header(
    kind: Kind,
    sender: Facility,
    receiver: Facility,
    number: u16,
    reference: Option<MsgId>,
) -> Header {
    Header {
        kind,
        id: id(sender, receiver, number),
        reference,
    }
}

/// The endpoint receives only a payload that was encoded, framed, unframed and parsed again.
fn wire(template: &str, header: Header) -> Header {
    let mut message = parse(template.as_bytes()).unwrap();
    message.header = header;
    let payload = encode(&message).unwrap();
    let framed = frame(&payload).unwrap();
    let (unframed, consumed) = unframe(&framed).unwrap().unwrap();
    assert_eq!(consumed, framed.len());
    let decoded = parse(unframed).unwrap();
    assert_eq!(encode(&decoded).unwrap(), payload);
    decoded.header
}

fn wire_rla(header: Header) -> Header {
    let mut message = parse(RLA.as_bytes()).unwrap();
    message.header = header;
    // Part II §3.6.2.3: the receiving center reports the sector to which it routed the transfer.
    // The alphanumeric sector is synthetic test data; the format is the Rev G field definition.
    message.f31 = Some(Field31 {
        facility: CZWG,
        sector: Some(*b"1A"),
        text: None,
    });
    let payload = encode(&message).unwrap();
    let framed = frame(&payload).unwrap();
    let (unframed, consumed) = unframe(&framed).unwrap().unwrap();
    assert_eq!(consumed, framed.len());
    let decoded = parse(unframed).unwrap();
    assert_eq!(encode(&decoded).unwrap(), payload);
    decoded.header
}

fn wire_rta_retract(header: Header) -> Header {
    let mut message = parse(RTA.as_bytes()).unwrap();
    message.header = header;
    // Part II §3.6.4.3: the initiating center retracts its handoff with RTA; the transferor's
    // sector is represented by this synthetic fixture value.
    message.f31 = Some(Field31 {
        facility: KZMP,
        sector: Some(*b"42"),
        text: None,
    });
    let payload = encode(&message).unwrap();
    let framed = frame(&payload).unwrap();
    let (unframed, consumed) = unframe(&framed).unwrap().unwrap();
    assert_eq!(consumed, framed.len());
    let decoded = parse(unframed).unwrap();
    assert_eq!(encode(&decoded).unwrap(), payload);
    decoded.header
}

fn wire_header(header: Header) -> Header {
    let payload = encode(&bare(header)).unwrap();
    let framed = frame(&payload).unwrap();
    let (unframed, consumed) = unframe(&framed).unwrap().unwrap();
    assert_eq!(consumed, framed.len());
    parse(unframed).unwrap().header
}

fn active(c: &Config) -> Session {
    let (s, actions) = step(c, new_session(), Event::Activate, 0);
    let [Action::SendIrq { number }] = actions.as_slice() else {
        panic!("activation must send one IRQ");
    };
    let irq = wire_header(header(Kind::Irq, c.local, c.peer, *number, None));
    let irs = wire_header(header(Kind::Irs, c.peer, c.local, 0, Some(irq.id)));
    let (s, actions) = step(c, s, Event::Received(irs), 1);
    assert!(actions.as_slice().is_empty());
    assert_eq!(s.state, State::Active);
    s
}

fn active_pair() -> (Config, Session, Config, Session) {
    // Bring both named facilities active using actual encoded/framed IRQ and IRS messages.
    let a_config = config(KZMP, CZWG);
    let b_config = config(CZWG, KZMP);
    let (mut a, actions) = step(&a_config, new_session(), Event::Activate, 0);
    let [Action::SendIrq {
        number: a_irq_number,
    }] = actions.as_slice()
    else {
        panic!("facility A activation must send IRQ");
    };
    let a_irq = wire_header(header(Kind::Irq, KZMP, CZWG, *a_irq_number, None));
    let (mut b, actions) = step(&b_config, new_session(), Event::Received(a_irq), 1);
    assert!(matches!(actions.as_slice(), [Action::InitRequested { header }] if *header == a_irq));

    let (next, actions) = step(&b_config, b, Event::Activate, 2);
    b = next;
    let [Action::SendIrq {
        number: b_irq_number,
    }] = actions.as_slice()
    else {
        panic!("facility B activation must send IRQ");
    };
    let b_irq = wire_header(header(Kind::Irq, CZWG, KZMP, *b_irq_number, None));

    let (next, actions) = step(&a_config, a, Event::Received(b_irq), 3);
    a = next;
    let [Action::SendIrq {
        number: a_retry_number,
    }, Action::SendIrs {
        number: a_irs_number,
        reference,
    }] = actions.as_slice()
    else {
        panic!("activating facility A must retry IRQ and answer peer IRQ");
    };
    assert_eq!(*reference, b_irq.id);
    let a_irs = wire_header(header(
        Kind::Irs,
        KZMP,
        CZWG,
        *a_irs_number,
        Some(*reference),
    ));
    let (next, actions) = step(&b_config, b, Event::Received(a_irs), 4);
    b = next;
    assert!(actions.as_slice().is_empty());
    assert_eq!(b.state, State::Active);

    let a_retry = wire_header(header(Kind::Irq, KZMP, CZWG, *a_retry_number, None));
    let (next, actions) = step(&b_config, b, Event::Received(a_retry), 5);
    b = next;
    let [Action::SendIrs {
        number: b_irs_number,
        reference,
    }] = actions.as_slice()
    else {
        panic!("active facility B must answer the retry IRQ");
    };
    assert_eq!(*reference, a_retry.id);
    let b_irs = wire_header(header(
        Kind::Irs,
        CZWG,
        KZMP,
        *b_irs_number,
        Some(*reference),
    ));
    let (next, actions) = step(&a_config, a, Event::Received(b_irs), 6);
    a = next;
    assert!(actions.as_slice().is_empty());
    assert_eq!(a.state, State::Active);
    (a_config, a, b_config, b)
}

// REQ: OA-A7-001
#[test]
fn radar_handoff_messages_cross_two_facilities_as_encoded_bytes() {
    // Rev G Part II §§2.2.5.1, 3.6.1–3.6.4 and Table B-1: RTI is acknowledged by RLA;
    // RTA is a distinct application message and receives its own LAM/LRM response.
    let (a_config, a, b_config, b) = active_pair();

    let (a, send) = step(&a_config, a, Event::SendAtm(Kind::Rti), 10);
    let [Action::SendAtm {
        kind: Kind::Rti,
        number: rti_number,
    }] = send.as_slice()
    else {
        panic!("expected RTI send");
    };
    let rti = wire(
        RTI,
        header(
            Kind::Rti,
            KZMP,
            CZWG,
            *rti_number,
            Some(id(KZMP, CZWG, 801)),
        ),
    );
    let (b, received) = step(&b_config, b, Event::Received(rti), 11);
    assert!(
        matches!(received.as_slice(), [Action::Deliver { header, seq: Seq::Gap(1) }] if *header == rti)
    );

    let (b, response) = step(
        &b_config,
        b,
        Event::SendResponse {
            kind: Kind::Rla,
            reference: rti.id,
        },
        12,
    );
    let [Action::SendResponse {
        kind: Kind::Rla,
        number: rla_number,
        reference,
    }] = response.as_slice()
    else {
        panic!("expected RLA response");
    };
    let rla = wire_rla(header(Kind::Rla, CZWG, KZMP, *rla_number, Some(*reference)));
    let (a, received) = step(&a_config, a, Event::Received(rla), 13);
    assert!(
        matches!(received.as_slice(), [Action::Response { header, answers: Kind::Rti }] if *header == rla)
    );

    let (b, send) = step(&b_config, b, Event::SendAtm(Kind::Rta), 14);
    let [Action::SendAtm {
        kind: Kind::Rta,
        number: rta_number,
    }] = send.as_slice()
    else {
        panic!("expected RTA send");
    };
    let rta = wire(
        RTA,
        header(Kind::Rta, CZWG, KZMP, *rta_number, Some(rti.id)),
    );
    let (a, received) = step(&a_config, a, Event::Received(rta), 15);
    assert!(
        matches!(received.as_slice(), [Action::Deliver { header, seq: Seq::InOrder }] if *header == rta)
    );
    let (_, response) = step(
        &a_config,
        a,
        Event::SendResponse {
            kind: Kind::Lam,
            reference: rta.id,
        },
        16,
    );
    let [Action::SendResponse {
        kind: Kind::Lam,
        number: lam_number,
        reference,
    }] = response.as_slice()
    else {
        panic!("expected LAM response");
    };
    let lam = wire(
        LAM,
        header(Kind::Lam, KZMP, CZWG, *lam_number, Some(*reference)),
    );
    let (_, received) = step(&b_config, b, Event::Received(lam), 17);
    assert!(
        matches!(received.as_slice(), [Action::Response { header, answers: Kind::Rta }] if *header == lam)
    );
}

// REQ: OA-A7-001
#[test]
fn initiating_facility_retracts_an_rti_with_rta_bytes() {
    // Rev G Part II §3.6.4.1 and B.1.8.3: the receiving center sends RTA for acceptance; the
    // initiating center sends RTA to retract. A returned LAM confirms computer receipt only.
    let (a_config, a, b_config, b) = active_pair();

    let (a, send) = step(&a_config, a, Event::SendAtm(Kind::Rti), 10);
    let [Action::SendAtm { number, .. }] = send.as_slice() else {
        panic!("expected RTI send");
    };
    let rti = wire(
        RTI,
        header(Kind::Rti, KZMP, CZWG, *number, Some(id(KZMP, CZWG, 801))),
    );
    let (b, received) = step(&b_config, b, Event::Received(rti), 11);
    assert!(
        matches!(received.as_slice(), [Action::Deliver { header, seq: Seq::Gap(1) }] if *header == rti)
    );

    let (a, send) = step(&a_config, a, Event::SendAtm(Kind::Rta), 12);
    let [Action::SendAtm {
        kind: Kind::Rta,
        number: rta_number,
    }] = send.as_slice()
    else {
        panic!("expected initiating-center RTA");
    };
    let rta = wire_rta_retract(header(Kind::Rta, KZMP, CZWG, *rta_number, Some(rti.id)));
    let (b, received) = step(&b_config, b, Event::Received(rta), 13);
    assert!(
        matches!(received.as_slice(), [Action::Deliver { header, seq: Seq::InOrder }] if *header == rta)
    );

    let (_, response) = step(
        &b_config,
        b,
        Event::SendResponse {
            kind: Kind::Lam,
            reference: rta.id,
        },
        14,
    );
    let [Action::SendResponse {
        kind: Kind::Lam,
        number,
        reference,
    }] = response.as_slice()
    else {
        panic!("expected LAM for syntactically accepted RTA");
    };
    let lam = wire(
        LAM,
        header(Kind::Lam, CZWG, KZMP, *number, Some(*reference)),
    );
    let (_, response) = step(&a_config, a, Event::Received(lam), 15);
    assert!(
        matches!(response.as_slice(), [Action::Response { header, answers: Kind::Rta }] if *header == lam)
    );
}

// REQ: OA-A7-002, OA-A7-003
#[test]
fn rejection_recoordination_and_duplicate_or_out_of_order_rti_are_visible() {
    // Rev G Part II §§3.6.1–3.6.2 and Table B-1: LRM rejects the RTI. A later RTI here is
    // only a new numbered protocol attempt; the ICD does not define an automatic recoordination
    // algorithm or authorize inferring a sector/ownership change from that attempt.
    let (a_config, a, b_config, b) = active_pair();

    let (a, send) = step(&a_config, a, Event::SendAtm(Kind::Rti), 10);
    let [Action::SendAtm { number, .. }] = send.as_slice() else {
        panic!("expected RTI send");
    };
    let first = wire(
        RTI,
        header(Kind::Rti, KZMP, CZWG, *number, Some(id(KZMP, CZWG, 801))),
    );
    let (b, received) = step(&b_config, b, Event::Received(first), 11);
    assert!(matches!(
        received.as_slice(),
        [Action::Deliver {
            seq: Seq::Gap(1),
            ..
        }]
    ));

    let (_, response) = step(
        &b_config,
        b,
        Event::SendResponse {
            kind: Kind::Lrm,
            reference: first.id,
        },
        12,
    );
    let [Action::SendResponse {
        kind: Kind::Lrm,
        number: lrm_number,
        reference,
    }] = response.as_slice()
    else {
        panic!("expected LRM response");
    };
    let rejected = wire(
        LRM,
        header(Kind::Lrm, CZWG, KZMP, *lrm_number, Some(*reference)),
    );
    let (a, response) = step(&a_config, a, Event::Received(rejected), 13);
    assert!(
        matches!(response.as_slice(), [Action::Response { header, answers: Kind::Rti }] if *header == rejected)
    );

    let (_, send) = step(&a_config, a, Event::SendAtm(Kind::Rti), 14);
    let [Action::SendAtm {
        number: next_number,
        ..
    }] = send.as_slice()
    else {
        panic!("expected a fresh RTI attempt after the terminal rejection");
    };
    assert_eq!(*next_number, *number + 1);
    let next = wire(
        RTI,
        header(
            Kind::Rti,
            KZMP,
            CZWG,
            *next_number,
            Some(id(KZMP, CZWG, 801)),
        ),
    );
    let (b, received) = step(&b_config, b, Event::Received(next), 15);
    assert!(matches!(
        received.as_slice(),
        [Action::Deliver {
            seq: Seq::InOrder,
            ..
        }]
    ));

    let (b, received) = step(&b_config, b, Event::Received(next), 16);
    assert!(matches!(
        received.as_slice(),
        [Action::Deliver {
            seq: Seq::Duplicate,
            ..
        }]
    ));

    let gap = wire(
        RTI,
        header(
            Kind::Rti,
            KZMP,
            CZWG,
            *next_number + 2,
            Some(id(KZMP, CZWG, 801)),
        ),
    );
    let (b, received) = step(&b_config, b, Event::Received(gap), 17);
    assert!(matches!(
        received.as_slice(),
        [Action::Deliver {
            seq: Seq::Gap(1),
            ..
        }]
    ));

    let lower = wire(
        RTI,
        header(
            Kind::Rti,
            KZMP,
            CZWG,
            *next_number + 1,
            Some(id(KZMP, CZWG, 801)),
        ),
    );
    let (_, received) = step(&b_config, b, Event::Received(lower), 18);
    assert!(matches!(
        received.as_slice(),
        [Action::Deliver {
            seq: Seq::Gap(998),
            ..
        }]
    ));
}

// REQ: OA-A7-004, OA-A7-005
#[test]
fn flight_plan_timeout_and_est_then_chg_are_wire_ordered() {
    // Rev G Part III §6.1 gives the flight-planning LAM/LRM response performance of under 60 s
    // for at least 99% of normal operations. It gives RTI/RTA/RTU a <6 s interface communication
    // maximum instead; this test does not turn that transit bound into an RTI decision timeout.
    let (a_config, mut a, b_config, b) = active_pair();

    let (next, send) = step(&a_config, a, Event::SendAtm(Kind::Fpl), 10);
    a = next;
    let [Action::SendAtm {
        kind: Kind::Fpl,
        number,
    }] = send.as_slice()
    else {
        panic!("expected FPL send");
    };
    let fpl = wire(FPL, header(Kind::Fpl, KZMP, CZWG, *number, None));
    let (_, received) = step(&b_config, b, Event::Received(fpl), 11);
    assert!(matches!(
        received.as_slice(),
        [Action::Deliver {
            seq: Seq::Gap(1),
            ..
        }]
    ));
    let (a, before_deadline) = step(&a_config, a, Event::Tick, 60_009);
    assert!(before_deadline.as_slice().is_empty());
    let (a, at_deadline) = step(&a_config, a, Event::Tick, 60_010);
    assert!(
        matches!(at_deadline.as_slice(), [Action::ResponseTimeout { kind: Kind::Fpl, number: n }] if *n == *number)
    );
    assert!(a.pending().is_empty());

    // Rev G Part II §§3.1.3 and 3.1.2: a boundary EST may be followed by a CHG. These frames
    // verify transport order and parsing only; applying the amendment to oa-flight is a separate
    // caller integration and is not asserted here.
    let est = wire(
        EST,
        header(Kind::Est, CZWG, KZMP, 20, Some(id(KZMP, CZWG, 19))),
    );
    let (a, received) = step(&a_config, a, Event::Received(est), 60_011);
    assert!(
        matches!(received.as_slice(), [Action::Deliver { header, seq: Seq::First }] if *header == est)
    );
    let chg = wire(
        CHG_AFTER_EST,
        header(Kind::Chg, CZWG, KZMP, 21, Some(id(CZWG, KZMP, 20))),
    );
    let (_, received) = step(&a_config, a, Event::Received(chg), 60_012);
    assert!(
        matches!(received.as_slice(), [Action::Deliver { header, seq: Seq::InOrder }] if *header == chg)
    );
}

// REQ: OA-A7-004
#[test]
fn rti_no_response_uses_only_the_explicit_local_timer_policy() {
    // Rev G Part II B.1.8.1 says an RTI with no RLA/LRM after a specified retry count is marked
    // failed; it does not give a numeric interval. Part III §6.1 sets <6 s for interface transit,
    // not an RTI decision timeout. The 100 ms value below is test configuration only.
    let mut c = config(KZMP, CZWG);
    c.response_timeout_ms = 100;
    c.max_resends = 0;
    let s = active(&c);
    let (s, send) = step(&c, s, Event::SendAtm(Kind::Rti), 10);
    let [Action::SendAtm {
        kind: Kind::Rti,
        number,
    }] = send.as_slice()
    else {
        panic!("expected RTI send");
    };
    let (s, before) = step(&c, s, Event::Tick, 109);
    assert!(before.as_slice().is_empty());
    let (s, at_deadline) = step(&c, s, Event::Tick, 110);
    assert!(
        matches!(at_deadline.as_slice(), [Action::ResponseTimeout { kind: Kind::Rti, number: n }] if *n == *number)
    );
    assert!(s.pending().is_empty());
}

//! Local transport-generation ingress fence (shell, B1/B3/B7).
//!
//! A transport adapter tags each received or deferred event with the local nonzero connection
//! generation before enqueueing it. Immediately before applying a queued event, the adapter calls
//! [`step_tagged`]. A stale receive or disconnect is refused before [`crate::session::step`] can
//! advance session state. This value is local metadata; it is not carried on the NAM wire and is
//! not peer authentication or authority.

use crate::core::identity::Header;
use crate::core::session::{self, Actions, Config, Event, Session};

/// An inbound transport event that can affect the NAM session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportEvent {
    /// A parsed peer message received on one local connection generation.
    Received(Header),
    /// The transport reports that one local connection generation was lost.
    ConnectionLost,
}

/// Queue item tagged with the local connection generation that produced it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaggedTransportEvent {
    pub generation: u64,
    pub event: TransportEvent,
}

/// Why an ingress item was refused before it reached the session state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngressRefusal {
    /// Generation zero is reserved to mean uninitialized.
    ZeroCurrentGeneration,
    /// Generation zero cannot identify a transport event.
    ZeroEventGeneration,
    /// The item belongs to a connection other than the caller's current one.
    StaleGeneration { current: u64, event: u64 },
}

/// Apply one tagged receive or disconnect only when its local generation is current.
///
/// The caller owns generation allocation and must advance it when replacing a transport. The
/// check is intentionally before `session::step`; on refusal no new session or actions are
/// produced. A trusted durable source outside this seam must preserve generation uniqueness
/// across any process restart that can leave old queued events available. This seam does not
/// prevent a delayed response on the *same* connection from matching after message-number wrap,
/// and it cannot distinguish peers that reuse the NAM message identifiers. Those cases need
/// protocol/application policy beyond this local fence.
pub fn step_tagged(
    cfg: &Config,
    session: Session,
    current_generation: u64,
    tagged: TaggedTransportEvent,
    now: session::Millis,
) -> Result<(Session, Actions), IngressRefusal> {
    if current_generation == 0 {
        return Err(IngressRefusal::ZeroCurrentGeneration);
    }
    if tagged.generation == 0 {
        return Err(IngressRefusal::ZeroEventGeneration);
    }
    if tagged.generation != current_generation {
        return Err(IngressRefusal::StaleGeneration {
            current: current_generation,
            event: tagged.generation,
        });
    }
    let event = match tagged.event {
        TransportEvent::Received(header) => Event::Received(header),
        TransportEvent::ConnectionLost => Event::ConnectionLost,
    };
    Ok(session::step(cfg, session, event, now))
}

//! Shell (B1, B7): codecs, input adapters, and the optional local transport-generation ingress
//! fence. Codec functions allocate per message (`Vec`, `String`), so this module is not real-time
//! core. The transport seam adds no network I/O and does not change wire messages.

pub mod error;
pub mod field;
pub mod frame;
pub mod item15;
pub mod lrm;
pub mod message;
pub mod model;
pub mod schema;
pub mod transport_epoch;
pub mod workload;

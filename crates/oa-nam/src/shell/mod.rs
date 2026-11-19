//! Shell (B1, B7): the codec, an input-format adapter under the shell and offline rules (D1,
//! C6). A pure function of bytes and values, but it allocates per message (`Vec`, `String`),
//! so it is not real-time core.

pub mod error;
pub mod field;
pub mod frame;
pub mod lrm;
pub mod message;
pub mod model;
pub mod schema;
pub mod workload;

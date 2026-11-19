//! The per-leg local frame, now shared with the tracker: see `oa_geo::aeqd`. Core (pure, B7).

pub(crate) use oa_geo::aeqd::{
    frame_angle, frame_new, project, try_frame_velocity, unproject, Frame,
};

//! Safe fixed-layout projection over the existing bounded ASTERIX decoder.
#![forbid(unsafe_code)]

use oa_asterix::cat021::Cat021;
use oa_asterix::cat048::Cat048;
use oa_asterix::cat062::Cat062;
use oa_asterix::common::{RawItem, SelectedAltitude};
use oa_asterix::uap::{self, Cat021Edition, Slot, Uap};
use oa_asterix::{AsterixError, Block};

pub const MAX_INPUT_BYTES: usize = oa_asterix::MAX_BLOCK_LEN;
pub const MAX_RECORDS: usize = oa_asterix::MAX_RECORDS_PER_BLOCK;
pub const FIELD_SOURCE: u64 = 1 << 0;
pub const FIELD_ADDRESS: u64 = 1 << 1;
pub const FIELD_IDENT: u64 = 1 << 2;
pub const FIELD_TRACK: u64 = 1 << 3;
pub const FIELD_TIME: u64 = 1 << 4;
pub const FIELD_POSITION_0: u64 = 1 << 5;
pub const FIELD_POSITION_1: u64 = 1 << 6;
pub const FIELD_VELOCITY: u64 = 1 << 7;
pub const FIELD_MODE3A: u64 = 1 << 8;
pub const FIELD_VERTICAL: u64 = 1 << 9;

pub const POSITION_GEODETIC: u8 = 1;
pub const POSITION_POLAR: u8 = 2;
pub const POSITION_CARTESIAN: u8 = 3;
pub const SCALE_CAT021_POS: u8 = 1;
pub const SCALE_CAT021_POS_HIGH: u8 = 2;
pub const SCALE_CAT062_POS: u8 = 3;
pub const SCALE_CAT048_POLAR: u8 = 4;
pub const SCALE_CAT048_CART: u8 = 5;
pub const SCALE_CAT062_CART: u8 = 6;
pub const VELOCITY_CAT021_GROUND: u8 = 1;
pub const VELOCITY_CAT062_CART: u8 = 2;

pub const VERTICAL_GEOMETRIC_HEIGHT: u8 = 1;
pub const VERTICAL_FLIGHT_LEVEL: u8 = 2;
pub const VERTICAL_SELECTED_ALTITUDE: u8 = 3;
pub const VERTICAL_FINAL_SELECTED_ALTITUDE: u8 = 4;
pub const VERTICAL_BARO_RATE: u8 = 5;
pub const VERTICAL_GEOMETRIC_RATE: u8 = 6;
pub const VERTICAL_SCALE_6_25_FT: u8 = 1;
pub const VERTICAL_SCALE_0_25_FL: u8 = 2;
pub const VERTICAL_SCALE_25_FT: u8 = 3;
pub const VERTICAL_SCALE_6_25_FPM: u8 = 4;

pub const VERTICAL_RANGE_EXCEEDED: u8 = 1 << 0;
pub const VERTICAL_NOT_VALIDATED: u8 = 1 << 1;
pub const VERTICAL_GARBLED: u8 = 1 << 2;
pub const VERTICAL_QNH: u8 = 1 << 3;
pub const VERTICAL_SOURCE_PROVIDED: u8 = 1 << 4;
pub const VERTICAL_MANAGE: u8 = 1 << 5;
pub const VERTICAL_HOLD: u8 = 1 << 6;
pub const VERTICAL_APPROACH: u8 = 1 << 7;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaAsterixPosition {
    pub item: u16,
    pub kind: u8,
    pub scale: u8,
    pub a_raw: i32,
    pub b_raw: i32,
}
const _: () = assert!(core::mem::size_of::<OaAsterixPosition>() == 12);
const _: () = assert!(core::mem::align_of::<OaAsterixPosition>() == 4);
const _: () = assert!(core::mem::offset_of!(OaAsterixPosition, item) == 0);
const _: () = assert!(core::mem::offset_of!(OaAsterixPosition, kind) == 2);
const _: () = assert!(core::mem::offset_of!(OaAsterixPosition, scale) == 3);
const _: () = assert!(core::mem::offset_of!(OaAsterixPosition, a_raw) == 4);
const _: () = assert!(core::mem::offset_of!(OaAsterixPosition, b_raw) == 8);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaAsterixVertical {
    pub item: u16,
    pub subitem: u8,
    pub kind: u8,
    pub scale: u8,
    pub flags: u8,
    pub auxiliary: u8,
    pub reserved: u8,
    pub raw: i32,
}
const _: () = assert!(core::mem::size_of::<OaAsterixVertical>() == 12);
const _: () = assert!(core::mem::align_of::<OaAsterixVertical>() == 4);
const _: () = assert!(core::mem::offset_of!(OaAsterixVertical, item) == 0);
const _: () = assert!(core::mem::offset_of!(OaAsterixVertical, subitem) == 2);
const _: () = assert!(core::mem::offset_of!(OaAsterixVertical, kind) == 3);
const _: () = assert!(core::mem::offset_of!(OaAsterixVertical, scale) == 4);
const _: () = assert!(core::mem::offset_of!(OaAsterixVertical, flags) == 5);
const _: () = assert!(core::mem::offset_of!(OaAsterixVertical, auxiliary) == 6);
const _: () = assert!(core::mem::offset_of!(OaAsterixVertical, reserved) == 7);
const _: () = assert!(core::mem::offset_of!(OaAsterixVertical, raw) == 8);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaAsterixRecord {
    pub category: u8,
    pub cat021_edition: u8,
    pub source_sac: u8,
    pub source_sic: u8,
    pub reserved0: u32,
    pub present_frn_mask: u64,
    pub raw_untyped_frn_mask: u64,
    pub typed_unprojected_frn_mask: u64,
    pub present_field_mask: u64,
    pub target_address: u32,
    pub target_ident: [u8; 8],
    pub track_number: u16,
    pub time_item: u16,
    pub time_raw: u32,
    pub position_count: u8,
    pub velocity_kind: u8,
    pub velocity_flags: u8,
    pub mode3a_flags: u8,
    pub positions: [OaAsterixPosition; 2],
    pub velocity_a_raw: i32,
    pub velocity_b_raw: i32,
    pub mode3a_code: u16,
    pub reserved3: u16,
    pub raw_untyped_subitem_mask: u32,
    pub reserved4: u32,
    pub reserved5: u32,
    pub vertical_count: u8,
    pub reserved6: [u8; 7],
    pub vertical: [OaAsterixVertical; 6],
}
const _: () = assert!(core::mem::size_of::<OaAsterixRecord>() == 192);
const _: () = assert!(core::mem::align_of::<OaAsterixRecord>() == 8);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, category) == 0);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, cat021_edition) == 1);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, source_sac) == 2);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, source_sic) == 3);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, reserved0) == 4);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, present_frn_mask) == 8);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, raw_untyped_frn_mask) == 16);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, typed_unprojected_frn_mask) == 24);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, present_field_mask) == 32);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, target_address) == 40);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, target_ident) == 44);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, track_number) == 52);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, time_item) == 54);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, time_raw) == 56);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, position_count) == 60);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, velocity_kind) == 61);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, velocity_flags) == 62);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, mode3a_flags) == 63);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, positions) == 64);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, velocity_a_raw) == 88);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, velocity_b_raw) == 92);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, mode3a_code) == 96);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, reserved3) == 98);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, raw_untyped_subitem_mask) == 100);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, reserved4) == 104);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, reserved5) == 108);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, vertical_count) == 112);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, reserved6) == 113);
const _: () = assert!(core::mem::offset_of!(OaAsterixRecord, vertical) == 120);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaAsterixCounts {
    pub category: u8,
    pub edition: u8,
    pub reserved: u16,
    pub reserved_bytes: [u8; 4],
    pub consumed_bytes: u64,
    pub required_record_count: u64,
    pub written_record_count: u64,
}
const _: () = assert!(core::mem::size_of::<OaAsterixCounts>() == 32);
const _: () = assert!(core::mem::align_of::<OaAsterixCounts>() == 8);
const _: () = assert!(core::mem::offset_of!(OaAsterixCounts, category) == 0);
const _: () = assert!(core::mem::offset_of!(OaAsterixCounts, edition) == 1);
const _: () = assert!(core::mem::offset_of!(OaAsterixCounts, reserved) == 2);
const _: () = assert!(core::mem::offset_of!(OaAsterixCounts, reserved_bytes) == 4);
const _: () = assert!(core::mem::offset_of!(OaAsterixCounts, consumed_bytes) == 8);
const _: () = assert!(core::mem::offset_of!(OaAsterixCounts, required_record_count) == 16);
const _: () = assert!(core::mem::offset_of!(OaAsterixCounts, written_record_count) == 24);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaAsterixError {
    pub status: u32,
    pub kind: u32,
    pub category: u8,
    pub subitem: u8,
    pub item: u16,
    pub record: u32,
    pub reserved_bytes: [u8; 8],
    pub offset: u64,
}
const _: () = assert!(core::mem::size_of::<OaAsterixError>() == 32);
const _: () = assert!(core::mem::align_of::<OaAsterixError>() == 8);
const _: () = assert!(core::mem::offset_of!(OaAsterixError, status) == 0);
const _: () = assert!(core::mem::offset_of!(OaAsterixError, kind) == 4);
const _: () = assert!(core::mem::offset_of!(OaAsterixError, category) == 8);
const _: () = assert!(core::mem::offset_of!(OaAsterixError, subitem) == 9);
const _: () = assert!(core::mem::offset_of!(OaAsterixError, item) == 10);
const _: () = assert!(core::mem::offset_of!(OaAsterixError, record) == 12);
const _: () = assert!(core::mem::offset_of!(OaAsterixError, reserved_bytes) == 16);
const _: () = assert!(core::mem::offset_of!(OaAsterixError, offset) == 24);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsterixFlat {
    pub counts: OaAsterixCounts,
    pub records: Vec<OaAsterixRecord>,
}

fn bit(index: u8) -> u64 {
    1u64.checked_shl(u32::from(index)).unwrap_or(0)
}

fn mark(mask: &mut u64, frn: u8) {
    if let Some(index) = frn.checked_sub(1) {
        *mask |= bit(index);
    }
}

fn frn_for_item(uap: &Uap, item: u16) -> Option<u8> {
    for (index, slot) in uap.slots.iter().enumerate() {
        if matches!(slot, Slot::Item { id, .. } if *id == item) {
            if let Ok(frn) = u8::try_from(index + 1) {
                return Some(frn);
            }
        }
    }
    None
}

fn mark_item(mask: &mut u64, uap: &Uap, item: u16) {
    if let Some(frn) = frn_for_item(uap, item) {
        mark(mask, frn);
    }
}

fn set_address(out: &mut OaAsterixRecord, value: u32) {
    out.target_address = value;
    out.present_field_mask |= FIELD_ADDRESS;
}

fn set_ident(out: &mut OaAsterixRecord, value: [u8; 8]) {
    out.target_ident = value;
    out.present_field_mask |= FIELD_IDENT;
}

fn set_track(out: &mut OaAsterixRecord, value: u16) {
    out.track_number = value;
    out.present_field_mask |= FIELD_TRACK;
}

fn set_time(out: &mut OaAsterixRecord, item: u16, value: u32) {
    out.time_item = item;
    out.time_raw = value;
    out.present_field_mask |= FIELD_TIME;
}

fn add_position(out: &mut OaAsterixRecord, value: OaAsterixPosition) {
    let index = usize::from(out.position_count);
    if let Some(slot) = out.positions.get_mut(index) {
        *slot = value;
        out.position_count = out.position_count.saturating_add(1);
        out.present_field_mask |= if index == 0 {
            FIELD_POSITION_0
        } else {
            FIELD_POSITION_1
        };
    }
}

// Keep the native item/subitem, scale, raw value, and source flags explicit at each projection.
#[allow(clippy::too_many_arguments)]
fn add_vertical(
    out: &mut OaAsterixRecord,
    item: u16,
    subitem: u8,
    kind: u8,
    scale: u8,
    raw: i32,
    flags: u8,
    auxiliary: u8,
) {
    let index = usize::from(out.vertical_count);
    if let Some(slot) = out.vertical.get_mut(index) {
        *slot = OaAsterixVertical {
            item,
            subitem,
            kind,
            scale,
            flags,
            auxiliary,
            reserved: 0,
            raw,
        };
        out.vertical_count = out.vertical_count.saturating_add(1);
        out.present_field_mask |= FIELD_VERTICAL;
    }
}

fn selected_altitude(out: &mut OaAsterixRecord, item: u16, subitem: u8, value: SelectedAltitude) {
    let flags = if value.source_provided {
        VERTICAL_SOURCE_PROVIDED
    } else {
        0
    };
    add_vertical(
        out,
        item,
        subitem,
        VERTICAL_SELECTED_ALTITUDE,
        VERTICAL_SCALE_25_FT,
        i32::from(value.altitude),
        flags,
        value.source,
    );
}

fn raw_subitems(out: &mut OaAsterixRecord, rec: &Cat062) {
    if let Some(derived) = &rec.derived {
        for sub in &derived.other {
            if let Some(index) = sub.number.checked_sub(1) {
                if let Some(flag) = (1u32).checked_shl(u32::from(index)) {
                    out.raw_untyped_subitem_mask |= flag;
                }
            }
        }
    }
}

fn add_present(out: &mut OaAsterixRecord, uap: &Uap, item: u16) {
    mark_item(&mut out.present_frn_mask, uap, item);
}

fn add_raw_untyped(out: &mut OaAsterixRecord, raw: &[RawItem]) {
    for item in raw {
        mark(&mut out.present_frn_mask, item.frn);
        mark(&mut out.raw_untyped_frn_mask, item.frn);
    }
}

fn finish_masks(out: &mut OaAsterixRecord, uap: &Uap, projected: &mut u64) {
    for frn in 1..=64u8 {
        let frn_bit = bit(frn - 1);
        if out.present_frn_mask & frn_bit != 0
            && out.raw_untyped_frn_mask & frn_bit == 0
            && *projected & frn_bit == 0
        {
            let is_cat062_derived = out.category == 62
                && frn_for_item(uap, 380).is_some_and(|derived_frn| derived_frn == frn);
            if !is_cat062_derived {
                out.typed_unprojected_frn_mask |= frn_bit;
            }
        }
    }
    out.present_field_mask |= FIELD_SOURCE;
}

fn add_mode3a(out: &mut OaAsterixRecord, code: u16, flags: u8) {
    out.mode3a_code = code;
    out.mode3a_flags = flags;
    out.present_field_mask |= FIELD_MODE3A;
}

fn project_021(rec: &Cat021, out: &mut OaAsterixRecord, uap: &Uap, projected: &mut u64) {
    out.source_sac = rec.source.sac;
    out.source_sic = rec.source.sic;
    add_present(out, uap, 10);
    mark(projected, frn_for_item(uap, 10).unwrap_or(0));
    if let Some(value) = rec.track_number {
        add_present(out, uap, 161);
        set_track(out, value);
        mark(projected, frn_for_item(uap, 161).unwrap_or(0));
    }
    if let Some(value) = rec.toa_position {
        add_present(out, uap, 71);
        set_time(out, 71, value);
        mark(projected, frn_for_item(uap, 71).unwrap_or(0));
    }
    if let Some(value) = rec.position {
        add_present(out, uap, 130);
        add_position(
            out,
            OaAsterixPosition {
                item: 130,
                kind: POSITION_GEODETIC,
                scale: SCALE_CAT021_POS,
                a_raw: value.lat,
                b_raw: value.lon,
            },
        );
        mark(projected, frn_for_item(uap, 130).unwrap_or(0));
    }
    if let Some(value) = rec.position_hr {
        add_present(out, uap, 131);
        add_position(
            out,
            OaAsterixPosition {
                item: 131,
                kind: POSITION_GEODETIC,
                scale: SCALE_CAT021_POS_HIGH,
                a_raw: value.lat,
                b_raw: value.lon,
            },
        );
        mark(projected, frn_for_item(uap, 131).unwrap_or(0));
    }
    if let Some(value) = rec.address {
        add_present(out, uap, 80);
        set_address(out, value);
        mark(projected, frn_for_item(uap, 80).unwrap_or(0));
    }
    if let Some(value) = rec.target_id {
        add_present(out, uap, 170);
        set_ident(out, value);
        mark(projected, frn_for_item(uap, 170).unwrap_or(0));
    }
    if let Some(value) = rec.ground_vector {
        add_present(out, uap, 160);
        out.velocity_kind = VELOCITY_CAT021_GROUND;
        out.velocity_flags = u8::from(value.range_exceeded);
        out.velocity_a_raw = i32::from(value.speed);
        out.velocity_b_raw = i32::from(value.track);
        out.present_field_mask |= FIELD_VELOCITY;
        mark(projected, frn_for_item(uap, 160).unwrap_or(0));
    }
    if let Some(value) = rec.mode3a {
        add_present(out, uap, 70);
        add_mode3a(out, value, 0);
        mark(projected, frn_for_item(uap, 70).unwrap_or(0));
    }
    if let Some(value) = rec.geometric_height {
        add_present(out, uap, 140);
        add_vertical(
            out,
            140,
            0,
            VERTICAL_GEOMETRIC_HEIGHT,
            VERTICAL_SCALE_6_25_FT,
            i32::from(value),
            0,
            0,
        );
        mark(projected, frn_for_item(uap, 140).unwrap_or(0));
    }
    if let Some(value) = rec.flight_level {
        add_present(out, uap, 145);
        add_vertical(
            out,
            145,
            0,
            VERTICAL_FLIGHT_LEVEL,
            VERTICAL_SCALE_0_25_FL,
            i32::from(value),
            0,
            0,
        );
        mark(projected, frn_for_item(uap, 145).unwrap_or(0));
    }
    if let Some(value) = rec.selected_altitude {
        add_present(out, uap, 146);
        selected_altitude(out, 146, 0, value);
        mark(projected, frn_for_item(uap, 146).unwrap_or(0));
    }
    if let Some(value) = rec.final_state {
        add_present(out, uap, 148);
        let mut flags = 0;
        if value.manage_vertical {
            flags |= VERTICAL_MANAGE;
        }
        if value.altitude_hold {
            flags |= VERTICAL_HOLD;
        }
        if value.approach {
            flags |= VERTICAL_APPROACH;
        }
        add_vertical(
            out,
            148,
            0,
            VERTICAL_FINAL_SELECTED_ALTITUDE,
            VERTICAL_SCALE_25_FT,
            i32::from(value.altitude),
            flags,
            0,
        );
        mark(projected, frn_for_item(uap, 148).unwrap_or(0));
    }
    if let Some(value) = rec.baro_rate {
        add_present(out, uap, 155);
        let flags = if value.range_exceeded {
            VERTICAL_RANGE_EXCEEDED
        } else {
            0
        };
        add_vertical(
            out,
            155,
            0,
            VERTICAL_BARO_RATE,
            VERTICAL_SCALE_6_25_FPM,
            i32::from(value.raw),
            flags,
            0,
        );
        mark(projected, frn_for_item(uap, 155).unwrap_or(0));
    }
    if let Some(value) = rec.geometric_rate {
        add_present(out, uap, 157);
        let flags = if value.range_exceeded {
            VERTICAL_RANGE_EXCEEDED
        } else {
            0
        };
        add_vertical(
            out,
            157,
            0,
            VERTICAL_GEOMETRIC_RATE,
            VERTICAL_SCALE_6_25_FPM,
            i32::from(value.raw),
            flags,
            0,
        );
        mark(projected, frn_for_item(uap, 157).unwrap_or(0));
    }
    for (item, present) in [
        (40, rec.descriptor.is_some()),
        (72, rec.toa_velocity.is_some()),
        (150, rec.air_speed.is_some()),
        (151, rec.true_airspeed.is_some()),
        (73, rec.tomr_position.is_some()),
        (74, rec.tomr_position_hp.is_some()),
        (75, rec.tomr_velocity.is_some()),
        (76, rec.tomr_velocity_hp.is_some()),
        (90, rec.quality.is_some()),
        (210, rec.mops.is_some()),
        (230, rec.roll_angle.is_some()),
        (152, rec.magnetic_heading.is_some()),
        (200, rec.target_status.is_some()),
        (165, rec.track_angle_rate.is_some()),
        (77, rec.time_transmission.is_some()),
        (20, rec.emitter_category.is_some()),
    ] {
        if present {
            add_present(out, uap, item);
        }
    }
    add_raw_untyped(out, &rec.raw);
}

fn project_048(rec: &Cat048, out: &mut OaAsterixRecord, uap: &Uap, projected: &mut u64) {
    out.source_sac = rec.source.sac;
    out.source_sic = rec.source.sic;
    add_present(out, uap, 10);
    mark(projected, frn_for_item(uap, 10).unwrap_or(0));
    if let Some(value) = rec.time_of_day {
        add_present(out, uap, 140);
        set_time(out, 140, value);
        mark(projected, frn_for_item(uap, 140).unwrap_or(0));
    }
    if let Some(value) = rec.polar {
        add_present(out, uap, 40);
        add_position(
            out,
            OaAsterixPosition {
                item: 40,
                kind: POSITION_POLAR,
                scale: SCALE_CAT048_POLAR,
                a_raw: i32::from(value.rho),
                b_raw: i32::from(value.theta),
            },
        );
        mark(projected, frn_for_item(uap, 40).unwrap_or(0));
    }
    if let Some(value) = rec.cartesian {
        add_present(out, uap, 42);
        add_position(
            out,
            OaAsterixPosition {
                item: 42,
                kind: POSITION_CARTESIAN,
                scale: SCALE_CAT048_CART,
                a_raw: i32::from(value.x),
                b_raw: i32::from(value.y),
            },
        );
        mark(projected, frn_for_item(uap, 42).unwrap_or(0));
    }
    if let Some(value) = rec.address {
        add_present(out, uap, 220);
        set_address(out, value);
        mark(projected, frn_for_item(uap, 220).unwrap_or(0));
    }
    if let Some(value) = rec.ident {
        add_present(out, uap, 240);
        set_ident(out, value);
        mark(projected, frn_for_item(uap, 240).unwrap_or(0));
    }
    if let Some(value) = rec.track_number {
        add_present(out, uap, 161);
        set_track(out, value);
        mark(projected, frn_for_item(uap, 161).unwrap_or(0));
    }
    if let Some(value) = rec.mode3a {
        add_present(out, uap, 70);
        let mut flags = 0;
        if value.not_validated {
            flags |= 1;
        }
        if value.garbled {
            flags |= 2;
        }
        if value.l_or_change {
            flags |= 4;
        }
        add_mode3a(out, value.code, flags);
        mark(projected, frn_for_item(uap, 70).unwrap_or(0));
    }
    if let Some(value) = rec.flight_level {
        add_present(out, uap, 90);
        let mut flags = 0;
        if value.not_validated {
            flags |= VERTICAL_NOT_VALIDATED;
        }
        if value.garbled {
            flags |= VERTICAL_GARBLED;
        }
        add_vertical(
            out,
            90,
            0,
            VERTICAL_FLIGHT_LEVEL,
            VERTICAL_SCALE_0_25_FL,
            i32::from(value.raw),
            flags,
            0,
        );
        mark(projected, frn_for_item(uap, 90).unwrap_or(0));
    }
    for (item, present) in [
        (20, rec.descriptor.is_some()),
        (130, rec.plot.is_some()),
        (250, rec.mb_data.is_some()),
        (170, rec.track_status.is_some()),
    ] {
        if present {
            add_present(out, uap, item);
        }
    }
    add_raw_untyped(out, &rec.raw);
}

fn project_062(rec: &Cat062, out: &mut OaAsterixRecord, uap: &Uap, projected: &mut u64) {
    out.source_sac = rec.source.sac;
    out.source_sic = rec.source.sic;
    add_present(out, uap, 10);
    mark(projected, frn_for_item(uap, 10).unwrap_or(0));
    if let Some(value) = rec.time {
        add_present(out, uap, 70);
        set_time(out, 70, value);
        mark(projected, frn_for_item(uap, 70).unwrap_or(0));
    }
    if let Some(value) = rec.position {
        add_present(out, uap, 105);
        add_position(
            out,
            OaAsterixPosition {
                item: 105,
                kind: POSITION_GEODETIC,
                scale: SCALE_CAT062_POS,
                a_raw: value.lat,
                b_raw: value.lon,
            },
        );
        mark(projected, frn_for_item(uap, 105).unwrap_or(0));
    }
    if let Some(value) = rec.cartesian {
        add_present(out, uap, 100);
        add_position(
            out,
            OaAsterixPosition {
                item: 100,
                kind: POSITION_CARTESIAN,
                scale: SCALE_CAT062_CART,
                a_raw: value.x,
                b_raw: value.y,
            },
        );
        mark(projected, frn_for_item(uap, 100).unwrap_or(0));
    }
    if let Some(value) = rec.velocity {
        add_present(out, uap, 185);
        out.velocity_kind = VELOCITY_CAT062_CART;
        out.velocity_a_raw = i32::from(value.vx);
        out.velocity_b_raw = i32::from(value.vy);
        out.present_field_mask |= FIELD_VELOCITY;
        mark(projected, frn_for_item(uap, 185).unwrap_or(0));
    }
    if let Some(value) = rec.mode3a {
        add_present(out, uap, 60);
        let mut flags = 0;
        if value.not_validated {
            flags |= 1;
        }
        if value.garbled {
            flags |= 2;
        }
        if value.l_or_change {
            flags |= 4;
        }
        add_mode3a(out, value.code, flags);
        mark(projected, frn_for_item(uap, 60).unwrap_or(0));
    }
    if let Some(value) = rec.track_number {
        add_present(out, uap, 40);
        set_track(out, value);
        mark(projected, frn_for_item(uap, 40).unwrap_or(0));
    }
    if let Some(value) = &rec.derived {
        add_present(out, uap, 380);
        let mut has_projected = false;
        if let Some(address) = value.address {
            set_address(out, address);
            has_projected = true;
        }
        if let Some(ident) = value.ident {
            set_ident(out, ident);
            has_projected = true;
        }
        if let Some(selected) = value.selected_altitude {
            selected_altitude(out, 380, 6, selected);
            has_projected = true;
        }
        raw_subitems(out, rec);
        if has_projected {
            mark(projected, frn_for_item(uap, 380).unwrap_or(0));
        }
    }
    if let Some(value) = rec.measured_fl {
        add_present(out, uap, 136);
        add_vertical(
            out,
            136,
            0,
            VERTICAL_FLIGHT_LEVEL,
            VERTICAL_SCALE_0_25_FL,
            i32::from(value),
            0,
            0,
        );
        mark(projected, frn_for_item(uap, 136).unwrap_or(0));
    }
    if let Some(value) = rec.geometric_altitude {
        add_present(out, uap, 130);
        add_vertical(
            out,
            130,
            0,
            VERTICAL_GEOMETRIC_HEIGHT,
            VERTICAL_SCALE_6_25_FT,
            i32::from(value),
            0,
            0,
        );
        mark(projected, frn_for_item(uap, 130).unwrap_or(0));
    }
    if let Some(value) = rec.baro_altitude {
        add_present(out, uap, 135);
        let flags = if value.qnh { VERTICAL_QNH } else { 0 };
        add_vertical(
            out,
            135,
            0,
            VERTICAL_FLIGHT_LEVEL,
            VERTICAL_SCALE_0_25_FL,
            i32::from(value.raw),
            flags,
            0,
        );
        mark(projected, frn_for_item(uap, 135).unwrap_or(0));
    }
    if let Some(value) = rec.climb_rate {
        add_present(out, uap, 220);
        add_vertical(
            out,
            220,
            0,
            VERTICAL_BARO_RATE,
            VERTICAL_SCALE_6_25_FPM,
            i32::from(value),
            0,
            0,
        );
        mark(projected, frn_for_item(uap, 220).unwrap_or(0));
    }
    for (item, present) in [
        (80, rec.track_status.is_some()),
        (290, rec.update_ages.is_some()),
    ] {
        if present {
            add_present(out, uap, item);
        }
    }
    add_raw_untyped(out, &rec.raw);
}

fn project_record(block: &Block, index: usize, edition: Cat021Edition) -> Option<OaAsterixRecord> {
    let (category, uap) = match block {
        Block::Cat021(_) => (21, uap::uap_021(edition)),
        Block::Cat048(_) => (48, &uap::UAP_048),
        Block::Cat062(_) => (62, &uap::UAP_062),
    };
    let mut out = OaAsterixRecord {
        category,
        cat021_edition: if category == 21 {
            match edition {
                Cat021Edition::Ed24 => 24,
                Cat021Edition::Ed27 => 27,
            }
        } else {
            0
        },
        ..OaAsterixRecord::default()
    };
    let mut projected = 0u64;
    match block {
        Block::Cat021(records) => project_021(records.get(index)?, &mut out, uap, &mut projected),
        Block::Cat048(records) => project_048(records.get(index)?, &mut out, uap, &mut projected),
        Block::Cat062(records) => project_062(records.get(index)?, &mut out, uap, &mut projected),
    }
    // The source identifier is mandatory in every accepted record.
    finish_masks(&mut out, uap, &mut projected);
    Some(out)
}

pub fn decode(input: &[u8], edition: Cat021Edition) -> Result<AsterixFlat, ProjectionFailure> {
    let (block, consumed) =
        oa_asterix::decode_block_with(input, edition).map_err(ProjectionFailure::Decode)?;
    let records_len = match &block {
        Block::Cat021(records) => records.len(),
        Block::Cat048(records) => records.len(),
        Block::Cat062(records) => records.len(),
    };
    let mut records = Vec::new();
    records
        .try_reserve_exact(records_len)
        .map_err(|_| ProjectionFailure::Allocation)?;
    for index in 0..records_len {
        let Some(record) = project_record(&block, index, edition) else {
            return Err(ProjectionFailure::Allocation);
        };
        records.push(record);
    }
    let category = match &block {
        Block::Cat021(_) => 21,
        Block::Cat048(_) => 48,
        Block::Cat062(_) => 62,
    };
    Ok(AsterixFlat {
        counts: OaAsterixCounts {
            category,
            edition: if category == 21 {
                match edition {
                    Cat021Edition::Ed24 => 24,
                    Cat021Edition::Ed27 => 27,
                }
            } else {
                0
            },
            reserved: 0,
            reserved_bytes: [0; 4],
            consumed_bytes: u64::try_from(consumed).unwrap_or(u64::MAX),
            required_record_count: u64::try_from(records.len()).unwrap_or(u64::MAX),
            written_record_count: 0,
        },
        records,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionFailure {
    Decode(AsterixError),
    Allocation,
}

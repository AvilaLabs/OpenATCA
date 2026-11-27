//! Safe, fixed-layout projection over the existing bounded GRIB2 message decoder.
#![forbid(unsafe_code)]

use oa_grib2::grid::{grid_dims, Grid};
use oa_grib2::message::{Field, FieldInfo};
use oa_grib2::product::{Parameter, RefTime};
use oa_grib2::{decode_message, inspect_stream, message_length, work_budget, Grib2Error};

pub const MAX_INPUT_BYTES: u64 = oa_grib2::MAX_MESSAGE_BYTES;
pub const MAX_FIELDS: usize = oa_grib2::MAX_FIELDS_PER_MESSAGE;
pub const MAX_VALUES: u64 = oa_grib2::MAX_MESSAGE_POINTS;

pub const PARAMETER_OTHER: u8 = 0;
pub const PARAMETER_TMP: u8 = 1;
pub const PARAMETER_RH: u8 = 2;
pub const PARAMETER_UGRD: u8 = 3;
pub const PARAMETER_VGRD: u8 = 4;
pub const PARAMETER_VVEL: u8 = 5;
pub const PARAMETER_HGT: u8 = 6;
pub const PARAMETER_PRES: u8 = 7;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaGrib2Time {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub reserved: u8,
}
const _: () = assert!(core::mem::size_of::<OaGrib2Time>() == 8);
const _: () = assert!(core::mem::align_of::<OaGrib2Time>() == 2);

/// One decoded field's metadata and its span in the call's contiguous `f32` output.
///
/// `grid_template` is 0 for regular latitude/longitude and 30 for Lambert conformal. Grid
/// parameters not used by the selected template are zero; that is a layout convention, not a
/// missing-data substitute. A bitmap-missing value in the separate value buffer is NaN.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaGrib2Field {
    pub discipline: u8,
    pub category: u8,
    pub number: u8,
    pub named_parameter: u8,
    pub grid_template: u16,
    pub product_template: u16,
    pub packing_template: u16,
    pub level_kind: u8,
    pub forecast_time_unit: u8,
    pub reference_time: OaGrib2Time,
    pub forecast_time: u32,
    pub level_value: f32,
    pub interval_present: u8,
    pub interval_ranges: u8,
    pub interval_statistic: u8,
    pub reserved0: u8,
    pub interval_end: OaGrib2Time,
    pub nx: u32,
    pub ny: u32,
    pub grid_flags: u8,
    pub scan_mode: u8,
    /// For template 3.0, +1 means row zero is southernmost and -1 northernmost; otherwise 0.
    pub latitude_row_direction: i8,
    /// 1 only when a template 3.0 grid covers the full circle.
    pub global_longitude: u8,
    pub first_latitude_deg: f64,
    pub first_longitude_deg: f64,
    pub i_increment_deg: f64,
    pub j_increment_deg: f64,
    pub lambert_lad_deg: f64,
    pub lambert_lov_deg: f64,
    pub lambert_dx_m: f64,
    pub lambert_dy_m: f64,
    pub lambert_latin1_deg: f64,
    pub lambert_latin2_deg: f64,
    pub earth_radius_m: f64,
    pub value_offset: u64,
    pub value_count: u64,
    pub missing_count: u64,
}
const _: () = assert!(core::mem::size_of::<OaGrib2Field>() == 168);
const _: () = assert!(core::mem::align_of::<OaGrib2Field>() == 8);
const _: () = assert!(core::mem::offset_of!(OaGrib2Field, reference_time) == 12);
const _: () = assert!(core::mem::offset_of!(OaGrib2Field, first_latitude_deg) == 56);
const _: () = assert!(core::mem::offset_of!(OaGrib2Field, value_offset) == 144);

#[derive(Debug)]
pub(crate) enum AdapterError {
    Decode(Grib2Error),
    Capacity,
    Inconsistent,
}

impl From<Grib2Error> for AdapterError {
    fn from(error: Grib2Error) -> Self {
        Self::Decode(error)
    }
}

pub(crate) struct Plan {
    fields: Vec<FieldInfo>,
    value_count: usize,
}

impl Plan {
    pub(crate) fn field_count(&self) -> usize {
        self.fields.len()
    }

    pub(crate) fn value_count(&self) -> usize {
        self.value_count
    }
}

/// Inspects one complete, bounded message and verifies the decoder's existing field/work caps.
pub(crate) fn inspect(input: &[u8]) -> Result<Plan, AdapterError> {
    if input.len() as u64 > MAX_INPUT_BYTES {
        return Err(AdapterError::Decode(Grib2Error::BadMessageLength(
            input.len() as u64,
        )));
    }
    let length = message_length(input, 0)?;
    if length != input.len() {
        return Err(AdapterError::Decode(Grib2Error::BadMessageLength(
            input.len() as u64,
        )));
    }
    let inspected = inspect_stream(input);
    if let Some(stopped) = inspected.stopped {
        return Err(AdapterError::Decode(stopped.error));
    }
    if let Some(skipped) = inspected.skipped.into_iter().next() {
        return Err(AdapterError::Decode(skipped.error));
    }
    if inspected.messages != 1 || inspected.fields.is_empty() {
        return Err(AdapterError::Inconsistent);
    }

    let mut remaining = work_budget(input.len());
    let mut total = 0u64;
    for info in &inspected.fields {
        let (nx, ny) = grid_dims(&info.grid);
        let points = u64::from(nx) * u64::from(ny);
        total = total
            .checked_add(points)
            .ok_or(AdapterError::Inconsistent)?;
        if points > remaining {
            return Err(AdapterError::Decode(Grib2Error::WorkBudget {
                points,
                budget: remaining,
            }));
        }
        remaining -= points;
    }
    if total > MAX_VALUES {
        return Err(AdapterError::Decode(Grib2Error::GridPoints(total)));
    }
    let value_count = usize::try_from(total).map_err(|_| AdapterError::Inconsistent)?;
    Ok(Plan {
        fields: inspected.fields,
        value_count,
    })
}

/// Decodes after the caller's metadata and value capacities have been checked.
///
/// The existing decoder retains field values in separate vectors. Once the entire message
/// succeeds, this adapter copies them directly into the caller's flat buffer without building a
/// second full-sized Rust value vector.
pub(crate) fn decode_into(
    input: &[u8],
    plan: &Plan,
    fields_out: &mut [OaGrib2Field],
    values_out: &mut [f32],
) -> Result<(), AdapterError> {
    if fields_out.len() < plan.field_count() || values_out.len() < plan.value_count() {
        return Err(AdapterError::Capacity);
    }
    let decoded = decode_message(input)?;
    if decoded.len() != plan.fields.len() {
        return Err(AdapterError::Inconsistent);
    }

    let mut descriptors = [OaGrib2Field::default(); MAX_FIELDS];
    let mut offset = 0usize;
    for (index, field) in decoded.iter().enumerate() {
        let count = field.values.len();
        let end = offset
            .checked_add(count)
            .ok_or(AdapterError::Inconsistent)?;
        if end > plan.value_count || index >= descriptors.len() {
            return Err(AdapterError::Inconsistent);
        }
        let descriptor = descriptors
            .get_mut(index)
            .ok_or(AdapterError::Inconsistent)?;
        *descriptor = describe_field(field, offset)?;
        offset = end;
    }
    if offset != plan.value_count {
        return Err(AdapterError::Inconsistent);
    }

    offset = 0;
    for field in &decoded {
        let end = offset
            .checked_add(field.values.len())
            .ok_or(AdapterError::Inconsistent)?;
        let destination = values_out
            .get_mut(offset..end)
            .ok_or(AdapterError::Inconsistent)?;
        destination.copy_from_slice(&field.values);
        offset = end;
    }
    let output = fields_out
        .get_mut(..decoded.len())
        .ok_or(AdapterError::Inconsistent)?;
    let source = descriptors
        .get(..decoded.len())
        .ok_or(AdapterError::Inconsistent)?;
    output.copy_from_slice(source);
    Ok(())
}

fn describe_field(field: &Field, offset: usize) -> Result<OaGrib2Field, AdapterError> {
    let (grid_template, nx, ny, grid_flags, scan_mode, row_direction, global, grid_params) =
        match field.grid {
            Grid::LatLon(grid) => (
                0,
                grid.ni,
                grid.nj,
                grid.flags,
                grid.scan,
                grid.j_dir,
                u8::from(grid.global),
                [
                    grid.la1, grid.lo1, grid.di, grid.dj, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
                ],
            ),
            Grid::Lambert(grid) => (
                30,
                grid.nx,
                grid.ny,
                grid.flags,
                grid.scan,
                0,
                0,
                [
                    grid.la1,
                    grid.lo1,
                    0.0,
                    0.0,
                    grid.lad,
                    grid.lov,
                    grid.dx,
                    grid.dy,
                    grid.latin1,
                    grid.latin2,
                    grid.radius,
                ],
            ),
        };
    let (interval_present, interval_ranges, interval_statistic, interval_end) = match field.interval
    {
        Some(interval) => (
            1,
            interval.ranges,
            interval.statistic,
            time_from_ref(interval.end),
        ),
        None => (0, 0, 0, OaGrib2Time::default()),
    };
    let value_count = u64::try_from(field.values.len()).map_err(|_| AdapterError::Inconsistent)?;
    let missing_count = u64::from(field.missing_count);
    let value_offset = u64::try_from(offset).map_err(|_| AdapterError::Inconsistent)?;
    Ok(OaGrib2Field {
        discipline: field.discipline,
        category: field.category,
        number: field.number,
        named_parameter: parameter_code(field.parameter),
        grid_template,
        product_template: field.product_template,
        packing_template: field.packing_template,
        level_kind: field.level.kind,
        forecast_time_unit: field.time_unit,
        reference_time: time_from_ref(field.ref_time),
        forecast_time: field.forecast_time,
        level_value: field.level.value,
        interval_present,
        interval_ranges,
        interval_statistic,
        reserved0: 0,
        interval_end,
        nx,
        ny,
        grid_flags,
        scan_mode,
        latitude_row_direction: row_direction,
        global_longitude: global,
        first_latitude_deg: grid_params[0],
        first_longitude_deg: grid_params[1],
        i_increment_deg: grid_params[2],
        j_increment_deg: grid_params[3],
        lambert_lad_deg: grid_params[4],
        lambert_lov_deg: grid_params[5],
        lambert_dx_m: grid_params[6],
        lambert_dy_m: grid_params[7],
        lambert_latin1_deg: grid_params[8],
        lambert_latin2_deg: grid_params[9],
        earth_radius_m: grid_params[10],
        value_offset,
        value_count,
        missing_count,
    })
}

fn time_from_ref(time: RefTime) -> OaGrib2Time {
    OaGrib2Time {
        year: time.year,
        month: time.month,
        day: time.day,
        hour: time.hour,
        minute: time.minute,
        second: time.second,
        reserved: 0,
    }
}

fn parameter_code(parameter: Parameter) -> u8 {
    match parameter {
        Parameter::Tmp => PARAMETER_TMP,
        Parameter::Rh => PARAMETER_RH,
        Parameter::Ugrd => PARAMETER_UGRD,
        Parameter::Vgrd => PARAMETER_VGRD,
        Parameter::Vvel => PARAMETER_VVEL,
        Parameter::Hgt => PARAMETER_HGT,
        Parameter::Pres => PARAMETER_PRES,
        Parameter::Other => PARAMETER_OTHER,
    }
}

//! Bounded deterministic IWXXM METAR/SPECI encoding for the normalized decoder model.
//!
//! This is a shell operation: it allocates one output buffer, capped at the same 256 KiB
//! document limit as the decoder. It serializes only the model's explicit fields. It does not
//! emit TAF, translation-failure reports, collections, or fields that the decoder does not
//! retain.

use crate::{
    Cloud, CloudAmount, CloudLayer, Measure, NilValue, NilValueObservation, Observation,
    PermissibleUsage, QuantityUnit, RelationalOperator, Report, ReportKind, ReportStatus,
    RunwayVisualRange, TrendChange, TrendForecast, TrendTimeIndicator, WeatherCode, WindShear,
    MAX_DOCUMENT_BYTES, MAX_PRESENT_WEATHER, MAX_RECENT_WEATHER, MAX_RVR_ITEMS,
    MAX_TREND_FORECASTS,
};
use core::fmt;

const IWXXM_NS: &str = "http://icao.int/iwxxm/2025-2";
const AIXM_NS: &str = "http://www.aixm.aero/schema/5.1.1";
const GML_NS: &str = "http://www.opengis.net/gml/3.2";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    UnsupportedReport,
    MissingRequiredField,
    InvalidValue,
    OutputTooLarge,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IWXXM encode refused: {self:?}")
    }
}

impl std::error::Error for EncodeError {}

/// Encode one normalized METAR or SPECI report to a deterministic, bounded XML document.
///
/// TAF and unrepresentable model states return a refusal; they are never silently omitted or
/// substituted. The returned document is no larger than [`MAX_DOCUMENT_BYTES`].
pub fn encode_report(report: &Report) -> Result<Vec<u8>, EncodeError> {
    validate_report(report)?;
    let mut writer = XmlWriter {
        text: String::with_capacity(8192),
    };
    let root = match report.kind {
        ReportKind::Metar => "METAR",
        ReportKind::Speci => "SPECI",
        ReportKind::Taf => return Err(EncodeError::UnsupportedReport),
    };
    writer.push("<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
    writer.push("<iwxxm:")?;
    writer.push(root)?;
    writer.push(" xmlns:iwxxm=\"")?;
    writer.attribute(IWXXM_NS)?;
    writer.push("\" xmlns:aixm=\"")?;
    writer.attribute(AIXM_NS)?;
    writer.push("\" xmlns:gml=\"")?;
    writer.attribute(GML_NS)?;
    writer.push("\" xmlns:xlink=\"")?;
    writer.attribute(XLINK_NS)?;
    writer.push("\" gml:id=\"uuid.00000000-0000-4000-8000-000000000001\" reportStatus=\"")?;
    writer.attribute(match report.status {
        ReportStatus::Normal => "NORMAL",
        ReportStatus::Amendment => "AMENDMENT",
        ReportStatus::Correction => "CORRECTION",
    })?;
    writer.push("\" permissibleUsage=\"")?;
    writer.attribute(match report.permissible_usage {
        PermissibleUsage::Operational => "OPERATIONAL",
        PermissibleUsage::NonOperational => "NON-OPERATIONAL",
    })?;
    writer.push("\"")?;
    if let Some(automated) = report.automated_station {
        writer.push(" automatedStation=\"")?;
        writer.push(if automated { "true\"" } else { "false\"" })?;
    }
    writer.push(">")?;
    write_time_reference(&mut writer, "issueTime", report.issue_time_ms, 2)?;
    write_aerodrome(&mut writer, &report.station)?;
    let observation_time = report
        .observation_time_ms
        .ok_or(EncodeError::MissingRequiredField)?;
    write_time_reference(&mut writer, "observationTime", observation_time, 6)?;
    match &report.observation {
        NilValueObservation::Missing => {}
        NilValueObservation::Nil { reason_uri } => {
            let reason = reason_uri
                .as_deref()
                .filter(|reason| !reason.is_empty())
                .ok_or(EncodeError::InvalidValue)?;
            writer.push("<iwxxm:observation nilReason=\"")?;
            writer.attribute(reason)?;
            writer.push("\"/>")?;
        }
        NilValueObservation::Observed(observation) => write_observation(&mut writer, observation)?,
    }
    if report.trends.len() > MAX_TREND_FORECASTS {
        return Err(EncodeError::InvalidValue);
    }
    for (index, trend) in report.trends.iter().enumerate() {
        write_trend(&mut writer, trend, index)?;
    }
    writer.push("</iwxxm:")?;
    writer.push(root)?;
    writer.push(">")?;
    Ok(writer.text.into_bytes())
}

fn validate_report(report: &Report) -> Result<(), EncodeError> {
    if report.kind == ReportKind::Taf || report.taf.is_some() {
        return Err(EncodeError::UnsupportedReport);
    }
    // AirportHeliport.designator is CodeAirportHeliportDesignatorBaseType in the pinned
    // AIXM 5.1.1 data types schema: 3..=6 uppercase ASCII letters/digits.
    if report.station.len() < 3
        || report.station.len() > 6
        || !report
            .station
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        || report.observation_time_ms.is_none()
        || report.issue_time_ms < 0
        || report.observation_time_ms.is_some_and(|value| value < 0)
    {
        return Err(EncodeError::InvalidValue);
    }
    if report.trends.len() > MAX_TREND_FORECASTS {
        return Err(EncodeError::InvalidValue);
    }
    if let NilValueObservation::Observed(observation) = &report.observation {
        validate_observation(observation)?;
    }
    Ok(())
}

fn validate_observation(observation: &Observation) -> Result<(), EncodeError> {
    let air_temperature = observation
        .air_temperature
        .as_ref()
        .ok_or(EncodeError::MissingRequiredField)?;
    let dewpoint_temperature = observation
        .dewpoint_temperature
        .as_ref()
        .ok_or(EncodeError::MissingRequiredField)?;
    let qnh = observation
        .qnh
        .as_ref()
        .ok_or(EncodeError::MissingRequiredField)?;
    validate_nil_value(air_temperature, Some(QuantityUnit::Celsius))?;
    validate_nil_value(dewpoint_temperature, Some(QuantityUnit::Celsius))?;
    validate_nil_value(qnh, Some(QuantityUnit::Hectopascal))?;
    if observation.surface_wind_nil_reason_uri.is_none() {
        validate_optional_value(&observation.wind_direction, Some(QuantityUnit::Degree))?;
        validate_optional_value(&observation.wind_speed, None)?;
        validate_optional_value(&observation.wind_gust, None)?;
        if observation.wind_speed.is_none() {
            return Err(EncodeError::MissingRequiredField);
        }
    } else if observation.wind_direction.is_some()
        || observation.wind_speed.is_some()
        || observation.wind_gust.is_some()
        || observation.variable_wind_direction.is_some()
    {
        return Err(EncodeError::InvalidValue);
    }
    if observation.present_weather.len() > MAX_PRESENT_WEATHER
        || observation.recent_weather.len() > MAX_RECENT_WEATHER
        || observation.runway_visual_ranges.len() > MAX_RVR_ITEMS
        || observation.cloud_and_visibility_ok
            && (observation.prevailing_visibility.is_some()
                || observation.minimum_visibility.is_some()
                || observation.minimum_visibility_direction.is_some()
                || !observation.runway_visual_ranges.is_empty()
                || !observation.present_weather.is_empty()
                || observation.cloud.is_some()
                || !observation.recent_weather.is_empty()
                || observation.wind_shear.is_some()
                || observation.wind_shear_nil_reason_uri.is_some())
    {
        return Err(EncodeError::InvalidValue);
    }
    validate_optional_value(
        &observation.prevailing_visibility,
        Some(QuantityUnit::Meter),
    )?;
    validate_optional_value(&observation.minimum_visibility, Some(QuantityUnit::Meter))?;
    validate_optional_value(
        &observation.minimum_visibility_direction,
        Some(QuantityUnit::Degree),
    )?;
    validate_weather(&observation.present_weather)?;
    validate_weather(&observation.recent_weather)?;
    if let Some(cloud) = observation.cloud.as_ref() {
        validate_cloud(cloud)?;
    }
    if let Some(shear) = observation.wind_shear.as_ref() {
        validate_wind_shear(shear)?;
    }
    if let Some(reason) = observation.wind_shear_nil_reason_uri.as_deref() {
        if reason.is_empty() {
            return Err(EncodeError::InvalidValue);
        }
        validate_scalar(reason)?;
        if observation.wind_shear.is_some() {
            return Err(EncodeError::InvalidValue);
        }
    }
    for rvr in &observation.runway_visual_ranges {
        validate_rvr(rvr)?;
    }
    Ok(())
}

fn validate_optional_value(
    value: &Option<NilValue>,
    expected_unit: Option<QuantityUnit>,
) -> Result<(), EncodeError> {
    if let Some(value) = value {
        validate_nil_value(value, expected_unit)?;
    }
    Ok(())
}

fn validate_nil_value(
    value: &NilValue,
    expected_unit: Option<QuantityUnit>,
) -> Result<(), EncodeError> {
    match value {
        NilValue::Nil { reason_uri } => {
            if reason_uri.as_deref().is_some_and(str::is_empty) {
                return Err(EncodeError::InvalidValue);
            }
            if let Some(reason) = reason_uri.as_deref() {
                validate_scalar(reason)?;
            }
        }
        NilValue::Measure { measure, .. } => {
            if expected_unit.is_some_and(|unit| unit != measure.unit) || measure.scale > 6 {
                return Err(EncodeError::InvalidValue);
            }
            let mantissa = i128::from(measure.mantissa);
            let denominator = 10i128.pow(u32::from(measure.scale));
            let outside_physical_domain = match measure.unit {
                QuantityUnit::Celsius => mantissa * 100 < -27_315 * denominator,
                QuantityUnit::Hectopascal => mantissa <= 0,
                QuantityUnit::Degree => mantissa < 0 || mantissa > 360 * denominator,
                QuantityUnit::Meter
                | QuantityUnit::Foot
                | QuantityUnit::MeterPerSecond
                | QuantityUnit::Knot => mantissa < 0,
            };
            if outside_physical_domain {
                return Err(EncodeError::InvalidValue);
            }
        }
    }
    Ok(())
}

fn validate_weather(values: &[WeatherCode]) -> Result<(), EncodeError> {
    for value in values {
        match value {
            WeatherCode::CodeUri(uri) if !uri.is_empty() => validate_scalar(uri)?,
            WeatherCode::Nil { reason_uri } => {
                let reason = reason_uri
                    .as_deref()
                    .filter(|reason| !reason.is_empty())
                    .ok_or(EncodeError::InvalidValue)?;
                validate_scalar(reason)?;
            }
            _ => return Err(EncodeError::InvalidValue),
        }
    }
    Ok(())
}

fn validate_cloud(cloud: &Cloud) -> Result<(), EncodeError> {
    match cloud {
        Cloud::Nil { reason_uri } => {
            let reason = reason_uri
                .as_deref()
                .filter(|reason| !reason.is_empty())
                .ok_or(EncodeError::InvalidValue)?;
            validate_scalar(reason)?;
        }
        Cloud::Observed {
            vertical_visibility,
            layers,
        } => {
            if layers.len() > crate::MAX_CLOUD_LAYERS
                || vertical_visibility.is_some() && !layers.is_empty()
            {
                return Err(EncodeError::InvalidValue);
            }
            validate_optional_value(vertical_visibility, None)?;
            if vertical_visibility.as_ref().is_some_and(|value| {
                matches!(value, NilValue::Measure { measure, .. } if !matches!(measure.unit, QuantityUnit::Foot | QuantityUnit::Meter))
            }) {
                return Err(EncodeError::InvalidValue);
            }
            for layer in layers {
                validate_cloud_layer(layer)?;
            }
        }
    }
    Ok(())
}

fn validate_cloud_layer(layer: &CloudLayer) -> Result<(), EncodeError> {
    match &layer.amount {
        CloudAmount::CodeUri(uri) if !uri.is_empty() => validate_scalar(uri)?,
        CloudAmount::Nil { reason_uri } => {
            let reason = reason_uri
                .as_deref()
                .filter(|reason| !reason.is_empty())
                .ok_or(EncodeError::InvalidValue)?;
            validate_scalar(reason)?;
        }
        _ => return Err(EncodeError::InvalidValue),
    }
    validate_nil_value(&layer.base, None)?;
    if matches!(&layer.base, NilValue::Measure { measure, .. } if !matches!(measure.unit, QuantityUnit::Foot | QuantityUnit::Meter))
    {
        return Err(EncodeError::InvalidValue);
    }
    if let Some(uri) = layer.cloud_type_uri.as_deref() {
        if uri.is_empty() || layer.cloud_type_nil_reason_uri.is_some() {
            return Err(EncodeError::InvalidValue);
        }
        validate_scalar(uri)?;
    }
    if let Some(reason) = layer.cloud_type_nil_reason_uri.as_deref() {
        if reason.is_empty() || layer.cloud_type_uri.is_some() {
            return Err(EncodeError::InvalidValue);
        }
        validate_scalar(reason)?;
    }
    Ok(())
}

fn validate_wind_shear(shear: &WindShear) -> Result<(), EncodeError> {
    if shear.runway_designators.len() > 4
        || !shear.all_runways && shear.runway_designators.is_empty()
    {
        return Err(EncodeError::InvalidValue);
    }
    for runway in &shear.runway_designators {
        validate_runway_designator(runway)?;
    }
    Ok(())
}

fn validate_rvr(rvr: &RunwayVisualRange) -> Result<(), EncodeError> {
    validate_nil_value(&rvr.mean_range, Some(QuantityUnit::Meter))?;
    match (
        rvr.runway_designator.as_deref(),
        rvr.runway_nil_reason_uri.as_deref(),
    ) {
        (Some(designator), None) => validate_runway_designator(designator)?,
        (None, Some(reason)) if !reason.is_empty() => validate_scalar(reason)?,
        _ => return Err(EncodeError::InvalidValue),
    }
    if let Some(tendency) = rvr.past_tendency.as_deref() {
        if !matches!(
            tendency,
            "UPWARD" | "DOWNWARD" | "NO_CHANGE" | "MISSING_VALUE"
        ) {
            return Err(EncodeError::InvalidValue);
        }
    }
    if rvr.mean_range_operator.is_some() && matches!(rvr.mean_range, NilValue::Nil { .. }) {
        return Err(EncodeError::InvalidValue);
    }
    Ok(())
}

fn validate_runway_designator(value: &str) -> Result<(), EncodeError> {
    // AIXM TextDesignatorBaseType restricts the value to 1..=16 characters of Character3Type.
    // Character3Type's pinned pattern permits uppercase letters, digits and this ASCII set;
    // CR/LF and lowercase letters are excluded.
    const PUNCTUATION: &[u8] = b", !\"#$%&'()*+-./:;<=>?@[\\]^_{|}";
    if !(1..=16).contains(&value.len())
        || !value.bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || PUNCTUATION.contains(&byte)
        })
    {
        return Err(EncodeError::InvalidValue);
    }
    Ok(())
}

fn validate_trend(trend: &TrendForecast) -> Result<(), EncodeError> {
    if trend.change == TrendChange::From
        || trend.time_start_ms < 0
        || trend
            .time_end_ms
            .is_some_and(|end| end < trend.time_start_ms)
    {
        return Err(EncodeError::InvalidValue);
    }
    // The emitted trend field is gml:LengthType, not IWXXM's nillable distance type.
    if matches!(
        trend.prevailing_visibility.as_ref(),
        Some(NilValue::Nil { .. })
    ) {
        return Err(EncodeError::InvalidValue);
    }
    validate_optional_value(&trend.prevailing_visibility, Some(QuantityUnit::Meter))?;
    validate_weather(&trend.weather)?;
    if trend.weather.len() > MAX_PRESENT_WEATHER {
        return Err(EncodeError::InvalidValue);
    }
    if let Some(cloud) = trend.cloud.as_ref() {
        validate_cloud(cloud)?;
    }
    Ok(())
}

fn validate_scalar(value: &str) -> Result<(), EncodeError> {
    if value.len() > crate::MAX_SCALAR_BYTES
        || value.chars().any(|ch| {
            !(ch == '\t'
                || ch == '\n'
                || ch == '\r'
                || ('\u{20}'..='\u{d7ff}').contains(&ch)
                || ('\u{e000}'..='\u{fffd}').contains(&ch)
                || ('\u{10000}'..='\u{10ffff}').contains(&ch))
        })
    {
        return Err(EncodeError::InvalidValue);
    }
    Ok(())
}

struct XmlWriter {
    text: String,
}

impl XmlWriter {
    fn push(&mut self, value: &str) -> Result<(), EncodeError> {
        let next = self
            .text
            .len()
            .checked_add(value.len())
            .ok_or(EncodeError::OutputTooLarge)?;
        if next > MAX_DOCUMENT_BYTES {
            return Err(EncodeError::OutputTooLarge);
        }
        self.text.push_str(value);
        Ok(())
    }

    fn text(&mut self, value: &str) -> Result<(), EncodeError> {
        validate_scalar(value)?;
        self.escaped(value, false)
    }

    fn attribute(&mut self, value: &str) -> Result<(), EncodeError> {
        validate_scalar(value)?;
        self.escaped(value, true)
    }

    fn escaped(&mut self, value: &str, attribute: bool) -> Result<(), EncodeError> {
        for ch in value.chars() {
            match ch {
                '&' => self.push("&amp;")?,
                '<' => self.push("&lt;")?,
                '>' => self.push("&gt;")?,
                '"' if attribute => self.push("&quot;")?,
                '\'' if attribute => self.push("&apos;")?,
                '\r' => self.push("&#xD;")?,
                '\n' if attribute => self.push("&#xA;")?,
                '\t' if attribute => self.push("&#x9;")?,
                _ => {
                    let mut encoded = [0u8; 4];
                    self.push(ch.encode_utf8(&mut encoded))?;
                }
            }
        }
        Ok(())
    }

    fn text_element(&mut self, name: &str, value: &str) -> Result<(), EncodeError> {
        self.push("<iwxxm:")?;
        self.push(name)?;
        self.push(">")?;
        self.text(value)?;
        self.push("</iwxxm:")?;
        self.push(name)?;
        self.push(">")
    }

    fn quantity(
        &mut self,
        name: &str,
        value: &NilValue,
        expected: Option<QuantityUnit>,
    ) -> Result<(), EncodeError> {
        validate_nil_value(value, expected)?;
        match value {
            NilValue::Measure { measure, operator } => {
                if operator.is_some() {
                    return Err(EncodeError::InvalidValue);
                }
                self.push("<iwxxm:")?;
                self.push(name)?;
                self.push(" uom=\"")?;
                self.push(unit_name(measure.unit)?)?;
                self.push("\">")?;
                self.text(&format_measure(*measure)?)?;
                self.push("</iwxxm:")?;
                self.push(name)?;
                self.push(">")
            }
            NilValue::Nil { reason_uri } => {
                let reason = reason_uri
                    .as_deref()
                    .filter(|reason| !reason.is_empty())
                    .ok_or(EncodeError::InvalidValue)?;
                self.push("<iwxxm:")?;
                self.push(name)?;
                self.push(" nilReason=\"")?;
                self.attribute(reason)?;
                self.push("\"/>")
            }
        }
    }

    fn coded(&mut self, name: &str, uri: &str) -> Result<(), EncodeError> {
        self.push("<iwxxm:")?;
        self.push(name)?;
        self.push(" xlink:href=\"")?;
        self.attribute(uri)?;
        self.push("\"/>")
    }

    fn coded_nil(&mut self, name: &str, reason: &str) -> Result<(), EncodeError> {
        self.push("<iwxxm:")?;
        self.push(name)?;
        self.push(" nilReason=\"")?;
        self.attribute(reason)?;
        self.push("\"/>")
    }
}

fn write_time_reference(
    writer: &mut XmlWriter,
    name: &str,
    time_ms: i64,
    id: u32,
) -> Result<(), EncodeError> {
    writer.push("<iwxxm:")?;
    writer.push(name)?;
    writer.push("><gml:TimeInstant gml:id=\"")?;
    writer.push(&gml_id(id))?;
    writer.push("\"><gml:timePosition>")?;
    writer.text(&format_time(time_ms)?)?;
    writer.push("</gml:timePosition></gml:TimeInstant></iwxxm:")?;
    writer.push(name)?;
    writer.push(">")
}

fn write_aerodrome(writer: &mut XmlWriter, station: &str) -> Result<(), EncodeError> {
    writer.push("<iwxxm:aerodrome><aixm:AirportHeliport gml:id=\"")?;
    writer.push(&gml_id(3))?;
    writer.push("\"><aixm:timeSlice><aixm:AirportHeliportTimeSlice gml:id=\"")?;
    writer.push(&gml_id(4))?;
    writer.push(
        "\"><gml:validTime/><aixm:interpretation>SNAPSHOT</aixm:interpretation><aixm:designator>",
    )?;
    writer.text(station)?;
    writer.push("</aixm:designator></aixm:AirportHeliportTimeSlice></aixm:timeSlice></aixm:AirportHeliport></iwxxm:aerodrome>")
}

fn write_observation(writer: &mut XmlWriter, observation: &Observation) -> Result<(), EncodeError> {
    writer.push("<iwxxm:observation><iwxxm:MeteorologicalAerodromeObservation gml:id=\"")?;
    writer.push(&gml_id(7))?;
    writer.push("\" cloudAndVisibilityOK=\"")?;
    writer.push(if observation.cloud_and_visibility_ok {
        "true"
    } else {
        "false"
    })?;
    writer.push("\">")?;
    writer.quantity(
        "airTemperature",
        observation
            .air_temperature
            .as_ref()
            .ok_or(EncodeError::MissingRequiredField)?,
        Some(QuantityUnit::Celsius),
    )?;
    writer.quantity(
        "dewpointTemperature",
        observation
            .dewpoint_temperature
            .as_ref()
            .ok_or(EncodeError::MissingRequiredField)?,
        Some(QuantityUnit::Celsius),
    )?;
    writer.quantity(
        "qnh",
        observation
            .qnh
            .as_ref()
            .ok_or(EncodeError::MissingRequiredField)?,
        Some(QuantityUnit::Hectopascal),
    )?;
    write_surface_wind(writer, observation)?;
    write_visibility(writer, observation)?;
    for (index, rvr) in observation.runway_visual_ranges.iter().enumerate() {
        write_rvr(writer, rvr, index)?;
    }
    for weather in &observation.present_weather {
        write_weather(writer, "presentWeather", weather)?;
    }
    if let Some(cloud) = observation.cloud.as_ref() {
        write_cloud(writer, cloud)?;
    }
    for weather in &observation.recent_weather {
        write_weather(writer, "recentWeather", weather)?;
    }
    write_wind_shear(writer, observation)?;
    writer.push("</iwxxm:MeteorologicalAerodromeObservation></iwxxm:observation>")
}

fn write_surface_wind(
    writer: &mut XmlWriter,
    observation: &Observation,
) -> Result<(), EncodeError> {
    if let Some(reason) = observation.surface_wind_nil_reason_uri.as_deref() {
        if reason.is_empty() {
            return Err(EncodeError::InvalidValue);
        }
        writer.push("<iwxxm:surfaceWind nilReason=\"")?;
        writer.attribute(reason)?;
        return writer.push("\"/>");
    }
    writer.push("<iwxxm:surfaceWind><iwxxm:AerodromeSurfaceWind gml:id=\"")?;
    writer.push(&gml_id(8))?;
    if let Some(variable) = observation.variable_wind_direction {
        writer.push("\" variableWindDirection=\"")?;
        writer.push(if variable { "true" } else { "false" })?;
    }
    writer.push("\">")?;
    if let Some(direction) = observation.wind_direction.as_ref() {
        writer.quantity("meanWindDirection", direction, Some(QuantityUnit::Degree))?;
    }
    let speed = observation
        .wind_speed
        .as_ref()
        .ok_or(EncodeError::MissingRequiredField)?;
    write_wind_measure(writer, "meanWindSpeed", "meanWindSpeedOperator", speed)?;
    if let Some(gust) = observation.wind_gust.as_ref() {
        write_wind_measure(writer, "windGustSpeed", "windGustSpeedOperator", gust)?;
    }
    writer.push("</iwxxm:AerodromeSurfaceWind></iwxxm:surfaceWind>")
}

fn write_wind_measure(
    writer: &mut XmlWriter,
    name: &str,
    operator_name: &str,
    value: &NilValue,
) -> Result<(), EncodeError> {
    validate_nil_value(value, None)?;
    let operator = match value {
        NilValue::Measure { operator, .. } => *operator,
        NilValue::Nil { .. } => None,
    };
    if let NilValue::Measure { measure, .. } = value {
        if !matches!(
            measure.unit,
            QuantityUnit::MeterPerSecond | QuantityUnit::Knot
        ) {
            return Err(EncodeError::InvalidValue);
        }
        if measure.mantissa < 0 {
            return Err(EncodeError::InvalidValue);
        }
    }
    let clean = match value {
        NilValue::Measure { measure, .. } => NilValue::Measure {
            measure: *measure,
            operator: None,
        },
        NilValue::Nil { reason_uri } => NilValue::Nil {
            reason_uri: reason_uri.clone(),
        },
    };
    writer.quantity(name, &clean, None)?;
    if let Some(operator) = operator {
        write_operator(writer, operator_name, operator)?;
    }
    Ok(())
}

fn write_visibility(writer: &mut XmlWriter, observation: &Observation) -> Result<(), EncodeError> {
    let any = observation.prevailing_visibility.is_some()
        || observation.minimum_visibility.is_some()
        || observation.minimum_visibility_direction.is_some();
    if !any {
        if observation.prevailing_visibility_operator.is_some() {
            return Err(EncodeError::InvalidValue);
        }
        return Ok(());
    }
    if let Some(NilValue::Nil { reason_uri }) = observation.prevailing_visibility.as_ref() {
        if observation.minimum_visibility.is_some()
            || observation.minimum_visibility_direction.is_some()
            || observation.prevailing_visibility_operator.is_some()
        {
            return Err(EncodeError::InvalidValue);
        }
        let reason = reason_uri
            .as_deref()
            .filter(|reason| !reason.is_empty())
            .ok_or(EncodeError::InvalidValue)?;
        writer.push("<iwxxm:visibility nilReason=\"")?;
        writer.attribute(reason)?;
        return writer.push("\"/>");
    }
    let prevailing = observation
        .prevailing_visibility
        .as_ref()
        .ok_or(EncodeError::MissingRequiredField)?;
    writer.push("<iwxxm:visibility><iwxxm:AerodromeHorizontalVisibility>")?;
    writer.quantity(
        "prevailingVisibility",
        prevailing,
        Some(QuantityUnit::Meter),
    )?;
    if let Some(operator) = observation.prevailing_visibility_operator {
        write_operator(writer, "prevailingVisibilityOperator", operator)?;
    }
    if let Some(minimum) = observation.minimum_visibility.as_ref() {
        writer.quantity("minimumVisibility", minimum, Some(QuantityUnit::Meter))?;
    }
    if let Some(direction) = observation.minimum_visibility_direction.as_ref() {
        writer.quantity(
            "minimumVisibilityDirection",
            direction,
            Some(QuantityUnit::Degree),
        )?;
    }
    writer.push("</iwxxm:AerodromeHorizontalVisibility></iwxxm:visibility>")
}

fn write_operator(
    writer: &mut XmlWriter,
    name: &str,
    operator: RelationalOperator,
) -> Result<(), EncodeError> {
    writer.text_element(
        name,
        match operator {
            RelationalOperator::Above => "ABOVE",
            RelationalOperator::Below => "BELOW",
        },
    )
}

fn write_rvr(
    writer: &mut XmlWriter,
    rvr: &RunwayVisualRange,
    index: usize,
) -> Result<(), EncodeError> {
    writer.push("<iwxxm:rvr><iwxxm:AerodromeRunwayVisualRange")?;
    if let Some(tendency) = rvr.past_tendency.as_deref() {
        writer.push(" pastTendency=\"")?;
        writer.attribute(tendency)?;
        writer.push("\"")?;
    }
    writer.push(">")?;
    if let Some(reason) = rvr.runway_nil_reason_uri.as_deref() {
        writer.push("<iwxxm:runway nilReason=\"")?;
        writer.attribute(reason)?;
        writer.push("\"/>")?;
    } else {
        let designator = rvr
            .runway_designator
            .as_deref()
            .ok_or(EncodeError::MissingRequiredField)?;
        writer.push("<iwxxm:runway><aixm:RunwayDirection gml:id=\"")?;
        let id = 20 + index as u32 * 2;
        writer.push(&gml_id(id))?;
        writer.push("\"><aixm:timeSlice><aixm:RunwayDirectionTimeSlice gml:id=\"")?;
        writer.push(&gml_id(id + 1))?;
        writer.push("\"><gml:validTime/><aixm:interpretation>SNAPSHOT</aixm:interpretation><aixm:designator>")?;
        writer.text(designator)?;
        writer.push("</aixm:designator></aixm:RunwayDirectionTimeSlice></aixm:timeSlice></aixm:RunwayDirection></iwxxm:runway>")?;
    }
    writer.quantity("meanRVR", &rvr.mean_range, Some(QuantityUnit::Meter))?;
    if let Some(operator) = rvr.mean_range_operator {
        write_operator(writer, "meanRVROperator", operator)?;
    }
    writer.push("</iwxxm:AerodromeRunwayVisualRange></iwxxm:rvr>")
}

fn write_weather(
    writer: &mut XmlWriter,
    name: &str,
    weather: &WeatherCode,
) -> Result<(), EncodeError> {
    match weather {
        WeatherCode::CodeUri(uri) => writer.coded(name, uri),
        WeatherCode::Nil { reason_uri } => {
            let reason = reason_uri
                .as_deref()
                .filter(|reason| !reason.is_empty())
                .ok_or(EncodeError::InvalidValue)?;
            writer.coded_nil(name, reason)
        }
    }
}

fn write_cloud(writer: &mut XmlWriter, cloud: &Cloud) -> Result<(), EncodeError> {
    match cloud {
        Cloud::Nil { reason_uri } => {
            let reason = reason_uri
                .as_deref()
                .filter(|reason| !reason.is_empty())
                .ok_or(EncodeError::InvalidValue)?;
            writer.push("<iwxxm:cloud nilReason=\"")?;
            writer.attribute(reason)?;
            writer.push("\"/>")
        }
        Cloud::Observed {
            vertical_visibility,
            layers,
        } => {
            writer.push("<iwxxm:cloud><iwxxm:AerodromeCloud>")?;
            if let Some(value) = vertical_visibility {
                writer.quantity("verticalVisibility", value, None)?;
            }
            for layer in layers {
                write_cloud_layer(writer, layer)?;
            }
            writer.push("</iwxxm:AerodromeCloud></iwxxm:cloud>")
        }
    }
}

fn write_cloud_layer(writer: &mut XmlWriter, layer: &CloudLayer) -> Result<(), EncodeError> {
    writer.push("<iwxxm:layer><iwxxm:CloudLayer>")?;
    match &layer.amount {
        CloudAmount::CodeUri(uri) => writer.coded("amount", uri)?,
        CloudAmount::Nil { reason_uri } => {
            let reason = reason_uri
                .as_deref()
                .filter(|reason| !reason.is_empty())
                .ok_or(EncodeError::InvalidValue)?;
            writer.coded_nil("amount", reason)?;
        }
    }
    writer.quantity("base", &layer.base, None)?;
    if let Some(uri) = layer.cloud_type_uri.as_deref() {
        writer.coded("cloudType", uri)?;
    } else if let Some(reason) = layer.cloud_type_nil_reason_uri.as_deref() {
        writer.coded_nil("cloudType", reason)?;
    }
    writer.push("</iwxxm:CloudLayer></iwxxm:layer>")
}

fn write_wind_shear(writer: &mut XmlWriter, observation: &Observation) -> Result<(), EncodeError> {
    if let Some(reason) = observation.wind_shear_nil_reason_uri.as_deref() {
        writer.push("<iwxxm:windShear nilReason=\"")?;
        writer.attribute(reason)?;
        return writer.push("\"/>");
    }
    if let Some(shear) = observation.wind_shear.as_ref() {
        writer.push("<iwxxm:windShear><iwxxm:AerodromeWindShear allRunways=\"")?;
        writer.push(if shear.all_runways { "true" } else { "false" })?;
        writer.push("\">")?;
        for (index, runway) in shear.runway_designators.iter().enumerate() {
            writer.push("<iwxxm:runway><aixm:RunwayDirection gml:id=\"")?;
            writer.push(&gml_id(30 + index as u32 * 2))?;
            writer.push("\"><aixm:timeSlice><aixm:RunwayDirectionTimeSlice gml:id=\"")?;
            writer.push(&gml_id(31 + index as u32 * 2))?;
            writer.push("\"><gml:validTime/><aixm:interpretation>SNAPSHOT</aixm:interpretation><aixm:designator>")?;
            writer.text(runway)?;
            writer.push("</aixm:designator></aixm:RunwayDirectionTimeSlice></aixm:timeSlice></aixm:RunwayDirection></iwxxm:runway>")?;
        }
        writer.push("</iwxxm:AerodromeWindShear></iwxxm:windShear>")?;
    }
    Ok(())
}

fn write_trend(
    writer: &mut XmlWriter,
    trend: &TrendForecast,
    index: usize,
) -> Result<(), EncodeError> {
    validate_trend(trend)?;
    writer.push("<iwxxm:trendForecast><iwxxm:MeteorologicalAerodromeTrendForecast gml:id=\"")?;
    writer.push(&gml_id(40 + index as u32 * 4))?;
    writer.push("\" changeIndicator=\"")?;
    writer.attribute(match trend.change {
        TrendChange::Becoming => "BECOMING",
        TrendChange::TemporaryFluctuations => "TEMPORARY_FLUCTUATIONS",
        TrendChange::From => "FROM",
    })?;
    writer.push("\"><iwxxm:phenomenonTime>")?;
    if let Some(end) = trend.time_end_ms {
        writer.push("<gml:TimePeriod gml:id=\"")?;
        writer.push(&gml_id(41 + index as u32 * 4))?;
        writer.push("\"><gml:beginPosition>")?;
        writer.text(&format_time(trend.time_start_ms)?)?;
        writer.push("</gml:beginPosition><gml:endPosition>")?;
        writer.text(&format_time(end)?)?;
        writer.push("</gml:endPosition></gml:TimePeriod>")?;
    } else {
        writer.push("<gml:TimeInstant gml:id=\"")?;
        writer.push(&gml_id(41 + index as u32 * 4))?;
        writer.push("\"><gml:timePosition>")?;
        writer.text(&format_time(trend.time_start_ms)?)?;
        writer.push("</gml:timePosition></gml:TimeInstant>")?;
    }
    writer.push("</iwxxm:phenomenonTime>")?;
    if let Some(indicator) = trend.time_indicator {
        writer.text_element(
            "timeIndicator",
            match indicator {
                TrendTimeIndicator::At => "AT",
                TrendTimeIndicator::Until => "UNTIL",
            },
        )?;
    }
    if let Some(visibility) = trend.prevailing_visibility.as_ref() {
        writer.quantity(
            "prevailingVisibility",
            visibility,
            Some(QuantityUnit::Meter),
        )?;
    }
    if let Some(operator) = trend.prevailing_visibility_operator {
        if trend.prevailing_visibility.is_none() {
            return Err(EncodeError::InvalidValue);
        }
        write_operator(writer, "prevailingVisibilityOperator", operator)?;
    }
    for weather in &trend.weather {
        write_weather(writer, "weather", weather)?;
    }
    if let Some(cloud) = trend.cloud.as_ref() {
        write_cloud(writer, cloud)?;
    }
    writer.push("</iwxxm:MeteorologicalAerodromeTrendForecast></iwxxm:trendForecast>")
}

fn unit_name(unit: QuantityUnit) -> Result<&'static str, EncodeError> {
    match unit {
        QuantityUnit::Celsius => Ok("Cel"),
        QuantityUnit::Hectopascal => Ok("hPa"),
        QuantityUnit::Meter => Ok("m"),
        QuantityUnit::Foot => Ok("[ft_i]"),
        QuantityUnit::MeterPerSecond => Ok("m/s"),
        QuantityUnit::Knot => Ok("[kn_i]"),
        QuantityUnit::Degree => Ok("deg"),
    }
}

fn format_measure(measure: Measure) -> Result<String, EncodeError> {
    if measure.scale > 6 {
        return Err(EncodeError::InvalidValue);
    }
    let negative = measure.mantissa < 0;
    let magnitude = measure.mantissa.unsigned_abs();
    if measure.scale == 0 {
        return Ok(if negative {
            format!("-{magnitude}")
        } else {
            magnitude.to_string()
        });
    }
    let divisor = 10u64
        .checked_pow(u32::from(measure.scale))
        .ok_or(EncodeError::InvalidValue)?;
    let integer = magnitude / divisor;
    let fraction = magnitude % divisor;
    let width = usize::from(measure.scale);
    Ok(if negative {
        format!("-{integer}.{fraction:0width$}")
    } else {
        format!("{integer}.{fraction:0width$}")
    })
}

fn gml_id(number: u32) -> String {
    format!("uuid.00000000-0000-4000-8000-{number:012}")
}

fn format_time(value: i64) -> Result<String, EncodeError> {
    const DAY_MS: i64 = 86_400_000;
    if !(0..=253_402_300_799_999).contains(&value) {
        return Err(EncodeError::InvalidValue);
    }
    let days = value.div_euclid(DAY_MS);
    let within = value.rem_euclid(DAY_MS);
    let z = days.checked_add(719_468).ok_or(EncodeError::InvalidValue)?;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    if !(1..=9999).contains(&year) {
        return Err(EncodeError::InvalidValue);
    }
    let hour = within / 3_600_000;
    let minute = (within / 60_000) % 60;
    let second = (within / 1_000) % 60;
    let millis = within % 1_000;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z"
    ))
}

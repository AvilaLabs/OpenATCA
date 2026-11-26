//! IWXXM 2025-2 METAR/SPECI subset reader. Allocation and XML parsing stay in this shell.

use crate::{
    ForecastChange, ForecastGroup, Measure, NilValue, NilValueObservation, Observation,
    PermissibleUsage, QuantityUnit, RelationalOperator, Report, ReportKind, ReportStatus,
    RunwayVisualRange, TafForecast, TrendChange, TrendForecast, TrendTimeIndicator, WeatherCode,
    WindShear, MAX_ATTRIBUTES, MAX_DECODE_WORK, MAX_DOCUMENT_BYTES, MAX_ELEMENTS,
    MAX_NAMESPACE_BINDINGS, MAX_PRESENT_WEATHER, MAX_RECENT_WEATHER, MAX_RVR_ITEMS,
    MAX_SCALAR_BYTES, MAX_TEXT_NODES, MAX_TREND_FORECASTS,
};
use oa_xml::{
    attr_needs_decode, decode_into, resolve_attr_ns, resolve_elem_ns, span_bytes, Limits, Ns,
    Parser, SpanKind,
};
use std::fmt;

const IWXXM: &[u8] = b"http://icao.int/iwxxm/2025-2";
const AIXM: &[u8] = b"http://www.aixm.aero/schema/5.1.1";
const GML: &[u8] = b"http://www.opengis.net/gml/3.2";
const XSI: &[u8] = b"http://www.w3.org/2001/XMLSchema-instance";
const XLINK: &[u8] = b"http://www.w3.org/1999/xlink";
const COLLECT: &[u8] = b"http://def.wmo.int/collect/2014";

/// Non-sensitive outcome counters for a single document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecodeStats {
    pub elements_seen: u32,
    pub attributes_seen: u32,
    pub unsupported_constructs: u32,
    pub refused_constructs: u32,
    pub work_units: u32,
}

/// A successful decode or a bounded, visible refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeResult {
    pub report: Report,
    pub stats: DecodeStats,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeFailure {
    InputTooLarge,
    Xml,
    UnsupportedRoot,
    UnsupportedNamespace,
    UnsupportedConstruct,
    MissingRequiredField,
    InvalidValue,
    Limit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError {
    pub failure: DecodeFailure,
    pub stats: DecodeStats,
    /// Narrow parser phase label for diagnosing bounded refusals.
    pub context: &'static str,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IWXXM decode refused: {:?}", self.failure)
    }
}

impl std::error::Error for DecodeError {}

fn err(failure: DecodeFailure, stats: DecodeStats) -> DecodeError {
    DecodeError {
        failure,
        stats,
        context: "document",
    }
}

fn in_context<T>(result: Result<T, DecodeError>, context: &'static str) -> Result<T, DecodeError> {
    result.map_err(|mut error| {
        if error.context == "document" {
            error.context = context;
        }
        error
    })
}

/// Decode one bounded IWXXM 2025-2 METAR or SPECI report.
///
/// A present field outside the supported subset refuses the whole report. This prevents a
/// successful-looking normalized row from concealing hazards or operational fields.
pub fn decode_report(input: &[u8]) -> Result<DecodeResult, DecodeError> {
    let mut stats = DecodeStats::default();
    if input.len() > MAX_DOCUMENT_BYTES {
        return Err(err(DecodeFailure::InputTooLarge, stats));
    }
    let mut parser = Parser::new(Limits {
        max_input_bytes: MAX_DOCUMENT_BYTES,
        max_depth: 64,
        max_elements: MAX_ELEMENTS,
        max_attributes: MAX_ATTRIBUTES,
        max_attrs_per_elem: 64,
        max_name_len: 256,
        max_text_nodes: MAX_TEXT_NODES,
        max_ns_bindings: MAX_NAMESPACE_BINDINGS,
    })
    .map_err(|_| err(DecodeFailure::Limit, stats))?;
    let doc = parser
        .parse(input)
        .map_err(|_| err(DecodeFailure::Xml, stats))?;
    stats.elements_seen = u32::try_from(doc.elem_name.len()).unwrap_or(u32::MAX);
    stats.attributes_seen = u32::try_from(doc.attr_name.len()).unwrap_or(u32::MAX);
    if doc.elem_name.is_empty() {
        return Err(err(DecodeFailure::UnsupportedRoot, stats));
    }
    let root = 0usize;
    no_significant_text(&doc, root, input, &mut stats)?;
    let root_name = name(&doc, root, input)?;
    if !elem_is(&doc, root, input, IWXXM)? {
        if root_name == b"MeteorologicalBulletin" && elem_is(&doc, root, input, COLLECT)? {
            refuse(&mut stats);
            return Err(DecodeError {
                failure: DecodeFailure::UnsupportedRoot,
                stats,
                context: "collect.MeteorologicalBulletin",
            });
        }
        return Err(err(DecodeFailure::UnsupportedNamespace, stats));
    }
    require_gml_id(&doc, root, input, &mut stats)?;
    let kind = match root_name {
        b"METAR" => ReportKind::Metar,
        b"SPECI" => ReportKind::Speci,
        b"TAF" => return decode_taf_report(&doc, root, input, stats),
        _ => return Err(err(DecodeFailure::UnsupportedRoot, stats)),
    };
    if attr_value(&doc, root, input, b"translationFailedTAC", &mut stats)?.is_some() {
        refuse(&mut stats);
        return Err(DecodeError {
            failure: DecodeFailure::UnsupportedConstruct,
            stats,
            context: "root.translationFailedTAC",
        });
    }
    in_context(
        check_attributes(
            &doc,
            root,
            input,
            &[b"reportStatus", b"permissibleUsage", b"automatedStation"],
            &mut stats,
        ),
        "root-attributes",
    )?;
    let status = match attr_value(&doc, root, input, b"reportStatus", &mut stats)? {
        Some(b"NORMAL") => ReportStatus::Normal,
        Some(b"AMENDMENT") => ReportStatus::Amendment,
        Some(b"CORRECTION") => ReportStatus::Correction,
        _ => return Err(err(DecodeFailure::InvalidValue, stats)),
    };
    let permissible_usage = match attr_value(&doc, root, input, b"permissibleUsage", &mut stats)? {
        Some(b"OPERATIONAL") => PermissibleUsage::Operational,
        Some(b"NON-OPERATIONAL") => PermissibleUsage::NonOperational,
        _ => return Err(err(DecodeFailure::InvalidValue, stats)),
    };
    let automated_station = match attr_value(&doc, root, input, b"automatedStation", &mut stats)? {
        Some(b"true") | Some(b"1") => Some(true),
        Some(b"false") | Some(b"0") => Some(false),
        None => None,
        _ => return Err(err(DecodeFailure::InvalidValue, stats)),
    };

    let mut issue_time = None;
    let mut observation_time = None;
    let mut station = None;
    let mut observation_ix = None;
    let mut trend_ix = Vec::with_capacity(MAX_TREND_FORECASTS);
    let mut next_root_child = 0u8;
    let mut child = first_child(&doc, root);
    while let Some(ix) = child {
        work(&mut stats)?;
        let local = name(&doc, ix, input)?;
        if elem_is(&doc, ix, input, IWXXM)? && local == b"issueTime" {
            if next_root_child != 0 {
                return Err(err(DecodeFailure::InvalidValue, stats));
            }
            set_once(
                &mut issue_time,
                in_context(time_reference(&doc, ix, input, &mut stats), "issue-time")?,
                &stats,
            )?;
            next_root_child = 1;
        } else if elem_is(&doc, ix, input, IWXXM)? && local == b"observationTime" {
            if next_root_child != 2 {
                return Err(err(DecodeFailure::InvalidValue, stats));
            }
            set_once(
                &mut observation_time,
                in_context(
                    time_reference(&doc, ix, input, &mut stats),
                    "observation-time",
                )?,
                &stats,
            )?;
            next_root_child = 3;
        } else if elem_is(&doc, ix, input, IWXXM)? && local == b"aerodrome" {
            if next_root_child != 1 {
                return Err(err(DecodeFailure::InvalidValue, stats));
            }
            set_once(
                &mut station,
                in_context(aerodrome(&doc, ix, input, &mut stats), "aerodrome")?,
                &stats,
            )?;
            next_root_child = 2;
        } else if elem_is(&doc, ix, input, IWXXM)? && local == b"observation" {
            if next_root_child != 3 {
                return Err(err(DecodeFailure::InvalidValue, stats));
            }
            set_once(&mut observation_ix, ix, &stats)?;
            next_root_child = 4;
        } else if elem_is(&doc, ix, input, IWXXM)? && local == b"trendForecast" {
            if next_root_child != 4 || trend_ix.len() >= MAX_TREND_FORECASTS {
                refuse(&mut stats);
                return Err(DecodeError {
                    failure: DecodeFailure::UnsupportedConstruct,
                    stats,
                    context: "trendForecast",
                });
            }
            trend_ix.push(ix);
        } else {
            refuse(&mut stats);
            let context = if elem_is(&doc, ix, input, IWXXM)? {
                "unexpected-iwxxm-root-child"
            } else {
                "non-iwxxm-root-child"
            };
            return Err(DecodeError {
                failure: DecodeFailure::UnsupportedConstruct,
                stats,
                context,
            });
        }
        child = next_sibling(&doc, ix);
    }
    let issue_time_ms =
        issue_time.ok_or_else(|| err(DecodeFailure::MissingRequiredField, stats))?;
    let observation_time_ms =
        observation_time.ok_or_else(|| err(DecodeFailure::MissingRequiredField, stats))?;
    let station = station.ok_or_else(|| err(DecodeFailure::MissingRequiredField, stats))?;

    let observation = match observation_ix {
        Some(ix) => in_context(
            decode_observation(&doc, ix, input, &mut stats),
            "observation",
        )?,
        None => NilValueObservation::Missing,
    };
    let mut trends = Vec::with_capacity(trend_ix.len());
    for ix in trend_ix {
        trends.push(in_context(
            decode_trend(&doc, ix, input, &mut stats),
            "trendForecast",
        )?);
    }
    Ok(DecodeResult {
        report: Report {
            kind,
            status,
            permissible_usage,
            station,
            issue_time_ms,
            observation_time_ms: Some(observation_time_ms),
            automated_station,
            observation,
            trends,
            taf: None,
        },
        stats,
    })
}

fn decode_taf_report(
    doc: &oa_xml::Doc<'_>,
    root: usize,
    input: &[u8],
    mut stats: DecodeStats,
) -> Result<DecodeResult, DecodeError> {
    if attr_value(doc, root, input, b"translationFailedTAC", &mut stats)?.is_some() {
        refuse(&mut stats);
        return Err(DecodeError {
            failure: DecodeFailure::UnsupportedConstruct,
            stats,
            context: "root.translationFailedTAC",
        });
    }
    check_attributes(
        doc,
        root,
        input,
        &[b"reportStatus", b"permissibleUsage", b"isCancelReport"],
        &mut stats,
    )?;
    match attr_value(doc, root, input, b"isCancelReport", &mut stats)? {
        Some(b"true") | Some(b"1") => {
            refuse(&mut stats);
            return Err(DecodeError {
                failure: DecodeFailure::UnsupportedConstruct,
                stats,
                context: "taf.cancelledReportValidPeriod",
            });
        }
        Some(b"false") | Some(b"0") | None => {}
        _ => return Err(err(DecodeFailure::InvalidValue, stats)),
    }
    let status = match attr_value(doc, root, input, b"reportStatus", &mut stats)? {
        Some(b"NORMAL") => ReportStatus::Normal,
        Some(b"AMENDMENT") => ReportStatus::Amendment,
        Some(b"CORRECTION") => ReportStatus::Correction,
        _ => return Err(err(DecodeFailure::InvalidValue, stats)),
    };
    let permissible = match attr_value(doc, root, input, b"permissibleUsage", &mut stats)? {
        Some(b"OPERATIONAL") => PermissibleUsage::Operational,
        Some(b"NON-OPERATIONAL") => PermissibleUsage::NonOperational,
        _ => return Err(err(DecodeFailure::InvalidValue, stats)),
    };
    let mut issue = None;
    let mut station = None;
    let mut valid = None;
    let mut base = None;
    let mut changes = Vec::with_capacity(crate::MAX_TAF_CHANGE_GROUPS);
    let mut stage = 0u8;
    let mut child = first_child(doc, root);
    while let Some(ix) = child {
        work(&mut stats)?;
        if !elem_is(doc, ix, input, IWXXM)? {
            refuse(&mut stats);
            return Err(DecodeError {
                failure: DecodeFailure::UnsupportedConstruct,
                stats,
                context: "taf.non-iwxxm-child",
            });
        }
        match name(doc, ix, input)? {
            b"issueTime" if stage == 0 => {
                issue = Some(time_reference(doc, ix, input, &mut stats)?);
                stage = 1;
            }
            b"aerodrome" if stage == 1 => {
                station = Some(aerodrome(doc, ix, input, &mut stats)?);
                stage = 2;
            }
            b"validPeriod" if stage == 2 => {
                valid = Some(decode_period_reference(doc, ix, input, &mut stats)?);
                stage = 3;
            }
            b"baseForecast" if stage == 3 => {
                base = Some(ix);
                stage = 4;
            }
            b"changeForecast" if stage >= 4 => {
                if changes.len() >= crate::MAX_TAF_CHANGE_GROUPS {
                    refuse(&mut stats);
                    return Err(DecodeError {
                        failure: DecodeFailure::Limit,
                        stats,
                        context: "taf.changeForecast.cardinality",
                    });
                }
                changes.push(ix);
            }
            b"cancelledReportValidPeriod" => {
                refuse(&mut stats);
                return Err(DecodeError {
                    failure: DecodeFailure::UnsupportedConstruct,
                    stats,
                    context: "taf.cancelledReportValidPeriod",
                });
            }
            _ => {
                refuse(&mut stats);
                let context = match name(doc, ix, input)? {
                    b"extension" => "taf.extension",
                    b"baseForecast" => "taf.baseForecast.order",
                    b"changeForecast" => "taf.changeForecast.order",
                    _ => "taf.unknown-child",
                };
                return Err(DecodeError {
                    failure: DecodeFailure::UnsupportedConstruct,
                    stats,
                    context,
                });
            }
        }
        child = next_sibling(doc, ix);
    }
    let issue_time = issue.ok_or_else(|| err(DecodeFailure::MissingRequiredField, stats))?;
    let station = station.ok_or_else(|| err(DecodeFailure::MissingRequiredField, stats))?;
    let valid_period = valid.ok_or_else(|| err(DecodeFailure::MissingRequiredField, stats))?;
    let base_ix = base.ok_or(DecodeError {
        failure: DecodeFailure::MissingRequiredField,
        stats,
        context: "taf.baseForecast",
    })?;
    let base = in_context(
        decode_forecast_group(
            doc,
            base_ix,
            input,
            &mut stats,
            valid_period.clone(),
            ForecastChange::Base,
            true,
        ),
        "taf.baseForecast",
    )?;
    let mut decoded_changes = Vec::with_capacity(changes.len());
    for ix in changes {
        decoded_changes.push(in_context(
            decode_forecast_group(
                doc,
                ix,
                input,
                &mut stats,
                valid_period.clone(),
                ForecastChange::Becoming,
                false,
            ),
            "taf.changeForecast",
        )?);
    }
    Ok(DecodeResult {
        report: Report {
            kind: ReportKind::Taf,
            status,
            permissible_usage: permissible,
            station,
            issue_time_ms: issue_time,
            observation_time_ms: None,
            automated_station: None,
            observation: NilValueObservation::Missing,
            trends: Vec::new(),
            taf: Some(TafForecast {
                valid_start_ms: valid_period.0,
                valid_end_ms: valid_period.1,
                base,
                changes: decoded_changes,
            }),
        },
        stats,
    })
}

fn decode_period_reference(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(i64, i64, String), DecodeError> {
    check_attributes(doc, wrapper, input, &[], stats)?;
    let period = only_child(doc, wrapper, GML, b"TimePeriod", input, stats)?;
    require_gml_id(doc, period, input, stats)?;
    check_attributes(doc, period, input, &[], stats)?;
    let id = gml_id(doc, period, input, stats)?;
    let (a, b) = exact_children2(
        doc,
        period,
        GML,
        b"beginPosition",
        b"endPosition",
        input,
        stats,
    )?;
    check_attributes(doc, a, input, &[], stats)?;
    check_attributes(doc, b, input, &[], stats)?;
    let start = parse_time(&scalar_text(doc, a, input, stats)?)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    let end = parse_time(&scalar_text(doc, b, input, stats)?)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    if end <= start {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    Ok((start, end, id))
}

fn gml_id(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<String, DecodeError> {
    let first = *doc
        .elem_first_attr
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    let count = *doc
        .elem_attr_count
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))? as usize;
    for i in first.0 as usize..first.0 as usize + count {
        work(stats)?;
        let local = span_bytes(
            input,
            *doc.attr_name
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        if local == b"id"
            && matches!(resolve_attr_ns(doc,oa_xml::ElemIx(elem as u32),oa_xml::AttrIx(i as u32)),Ns::Uri(s)if oa_xml::ns_uri_bytes(doc,s)==Some(GML))
        {
            let raw = span_bytes(
                input,
                *doc.attr_value
                    .get(i)
                    .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
            )
            .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
            if raw.is_empty()
                || raw.len() > MAX_SCALAR_BYTES
                || attr_needs_decode(doc, oa_xml::AttrIx(i as u32))
            {
                return Err(err(DecodeFailure::InvalidValue, *stats));
            }
            return String::from_utf8(raw.to_vec())
                .map_err(|_| err(DecodeFailure::InvalidValue, *stats));
        }
    }
    Err(err(DecodeFailure::MissingRequiredField, *stats))
}

fn decode_forecast_group(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
    valid: (i64, i64, String),
    default: ForecastChange,
    base: bool,
) -> Result<ForecastGroup, DecodeError> {
    check_attributes(doc, wrapper, input, &[], stats)?;
    let body = only_child(
        doc,
        wrapper,
        IWXXM,
        b"MeteorologicalAerodromeForecast",
        input,
        stats,
    )?;
    require_gml_id(doc, body, input, stats)?;
    no_significant_text(doc, body, input, stats)?;
    check_attributes(
        doc,
        body,
        input,
        &[b"changeIndicator", b"cloudAndVisibilityOK"],
        stats,
    )?;
    let change = if base {
        if attr_value(doc, body, input, b"changeIndicator", stats)?.is_some() {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        default
    } else {
        match attr_value(doc, body, input, b"changeIndicator", stats)? {
            Some(b"BECOMING") => ForecastChange::Becoming,
            Some(b"TEMPORARY_FLUCTUATIONS") => ForecastChange::TemporaryFluctuations,
            Some(b"FROM") => ForecastChange::From,
            Some(b"PROBABILITY_30") => ForecastChange::Probability30,
            Some(b"PROBABILITY_30_TEMPORARY_FLUCTUATIONS") => {
                ForecastChange::Probability30TemporaryFluctuations
            }
            Some(b"PROBABILITY_40") => ForecastChange::Probability40,
            Some(b"PROBABILITY_40_TEMPORARY_FLUCTUATIONS") => {
                ForecastChange::Probability40TemporaryFluctuations
            }
            _ => return Err(err(DecodeFailure::InvalidValue, *stats)),
        }
    };
    let cavok = match attr_value(doc, body, input, b"cloudAndVisibilityOK", stats)? {
        Some(b"true") | Some(b"1") => true,
        Some(b"false") | Some(b"0") | None => false,
        _ => return Err(err(DecodeFailure::InvalidValue, *stats)),
    };
    let mut time = None;
    let mut visibility = None;
    let mut operator = None;
    let mut wind = None;
    let mut weather = Vec::with_capacity(MAX_PRESENT_WEATHER);
    let mut cloud = None;
    let mut rank = 0u8;
    let mut child = first_child(doc, body);
    while let Some(ix) = child {
        work(stats)?;
        if !elem_is(doc, ix, input, IWXXM)? {
            refuse(stats);
            return Err(DecodeError {
                failure: DecodeFailure::UnsupportedConstruct,
                stats: *stats,
                context: "taf.forecast.non-iwxxm-child",
            });
        }
        match name(doc, ix, input)? {
            b"phenomenonTime" if rank == 0 => {
                time = Some(decode_taf_group_time(doc, ix, input, stats, &valid, base)?);
                rank = 1;
            }
            b"prevailingVisibility" if rank <= 1 => {
                visibility = Some(measure_value(
                    doc,
                    ix,
                    input,
                    QuantityUnit::Meter,
                    b"m",
                    stats,
                )?);
                rank = 2;
            }
            b"prevailingVisibilityOperator" if rank <= 2 => {
                operator = Some(read_operator(doc, ix, input, stats)?);
                rank = 3;
            }
            b"surfaceWind" if rank <= 3 => {
                wind = Some(decode_forecast_wind(doc, ix, input, stats)?);
                rank = 4;
            }
            b"weather" if rank <= 4 => {
                if weather.len() >= MAX_PRESENT_WEATHER {
                    return Err(err(DecodeFailure::Limit, *stats));
                }
                weather.push(decode_weather(doc, ix, input, stats)?);
                rank = 5;
            }
            b"cloud" if rank <= 5 && cloud.is_none() => {
                cloud = Some(decode_forecast_cloud(doc, ix, input, stats)?);
                rank = 6;
            }
            b"temperature" => {
                refuse(stats);
                return Err(DecodeError {
                    failure: DecodeFailure::UnsupportedConstruct,
                    stats: *stats,
                    context: "taf.forecast.temperature",
                });
            }
            _ => {
                refuse(stats);
                let context = match name(doc, ix, input)? {
                    b"extension" => "taf.forecast.extension",
                    b"weather" => "taf.forecast.weather.cardinality",
                    b"cloud" => "taf.forecast.cloud.duplicate",
                    _ => "taf.forecast.unknown-child",
                };
                return Err(DecodeError {
                    failure: DecodeFailure::UnsupportedConstruct,
                    stats: *stats,
                    context,
                });
            }
        }
        child = next_sibling(doc, ix);
    }
    let (start, end) = time.ok_or(DecodeError {
        failure: DecodeFailure::MissingRequiredField,
        stats: *stats,
        context: "taf.forecast.phenomenonTime",
    })?;
    if start < valid.0 || end > valid.1 || end < start {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    if cavok && (visibility.is_some() || !weather.is_empty() || cloud.is_some()) {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    Ok(ForecastGroup {
        change,
        valid_start_ms: start,
        valid_end_ms: end,
        prevailing_visibility: visibility,
        prevailing_visibility_operator: operator,
        wind_direction: wind.as_ref().and_then(|x| x.direction.clone()),
        wind_speed: wind.as_ref().and_then(|x| x.speed.clone()),
        wind_gust: wind.as_ref().and_then(|x| x.gust.clone()),
        variable_wind_direction: wind.and_then(|x| x.variable_direction),
        weather,
        cloud,
    })
}

fn decode_taf_group_time(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
    valid: &(i64, i64, String),
    base: bool,
) -> Result<(i64, i64), DecodeError> {
    check_xlink_attributes(doc, wrapper, input, stats)?;
    if let Some(href) = xlink_href(doc, wrapper, input, stats)? {
        if !base || href.strip_prefix('#') != Some(valid.2.as_str()) {
            refuse(stats);
            return Err(DecodeError {
                failure: DecodeFailure::UnsupportedConstruct,
                stats: *stats,
                context: "taf.forecast.phenomenonTime.reference",
            });
        }
        return Ok((valid.0, valid.1));
    }
    if base {
        return Err(DecodeError {
            failure: DecodeFailure::MissingRequiredField,
            stats: *stats,
            context: "taf.forecast.phenomenonTime.reference",
        });
    }
    let (start, end, _) = decode_period_reference(doc, wrapper, input, stats)?;
    Ok((start, end))
}

fn check_xlink_attributes(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(), DecodeError> {
    let first = *doc
        .elem_first_attr
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    let count = *doc
        .elem_attr_count
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))? as usize;
    for i in first.0 as usize..first.0 as usize + count {
        work(stats)?;
        let prefix = span_bytes(
            input,
            *doc.attr_prefix
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        let local = span_bytes(
            input,
            *doc.attr_name
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        if prefix == b"xmlns" || prefix.is_empty() && local == b"xmlns" {
            continue;
        }
        if !(local == b"href"
            && matches!(resolve_attr_ns(doc,oa_xml::ElemIx(elem as u32),oa_xml::AttrIx(i as u32)),Ns::Uri(s)if oa_xml::ns_uri_bytes(doc,s)==Some(XLINK)))
        {
            refuse(stats);
            return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
        }
    }
    Ok(())
}

fn decode_forecast_wind(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<DecodedWind, DecodeError> {
    check_attributes(doc, wrapper, input, &[], stats)?;
    let body = only_child(
        doc,
        wrapper,
        IWXXM,
        b"AerodromeSurfaceWindForecast",
        input,
        stats,
    )?;
    decode_surface_wind_body(doc, body, input, stats, b"AerodromeSurfaceWindForecast")
}

fn decode_forecast_cloud(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<crate::Cloud, DecodeError> {
    check_attributes(doc, wrapper, input, &[b"nilReason"], stats)?;
    if let Some(reason) = attr_value(doc, wrapper, input, b"nilReason", stats)? {
        if reason.is_empty()
            || reason.len() > MAX_SCALAR_BYTES
            || first_child(doc, wrapper).is_some()
        {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        return Ok(crate::Cloud::Nil {
            reason_uri: Some(
                String::from_utf8(reason.to_vec())
                    .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?,
            ),
        });
    }
    let body = only_child(doc, wrapper, IWXXM, b"AerodromeCloudForecast", input, stats)?;
    decode_cloud_body(doc, body, input, stats)
}

fn refuse(stats: &mut DecodeStats) {
    stats.unsupported_constructs = stats.unsupported_constructs.saturating_add(1);
    stats.refused_constructs = stats.refused_constructs.saturating_add(1);
}

fn work(stats: &mut DecodeStats) -> Result<(), DecodeError> {
    stats.work_units = stats.work_units.saturating_add(1);
    if stats.work_units as usize > MAX_DECODE_WORK {
        return Err(err(DecodeFailure::Limit, *stats));
    }
    Ok(())
}

fn elem_is(
    doc: &oa_xml::Doc<'_>,
    ix: usize,
    input: &[u8],
    want: &[u8],
) -> Result<bool, DecodeError> {
    match resolve_elem_ns(doc, oa_xml::ElemIx(ix as u32)) {
        Ns::Uri(span) => Ok(oa_xml::ns_uri_bytes(doc, span) == Some(want)),
        _ => {
            let _ = input;
            Ok(false)
        }
    }
}

fn name<'a>(doc: &oa_xml::Doc<'a>, ix: usize, input: &'a [u8]) -> Result<&'a [u8], DecodeError> {
    let s = *doc
        .elem_name
        .get(ix)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, DecodeStats::default()))?;
    span_bytes(input, s).map_err(|_| err(DecodeFailure::InvalidValue, DecodeStats::default()))
}

fn first_child(doc: &oa_xml::Doc<'_>, parent: usize) -> Option<usize> {
    let ix = *doc.elem_first_child.get(parent)?;
    (ix != oa_xml::ElemIx::NONE).then_some(ix.0 as usize)
}

fn next_sibling(doc: &oa_xml::Doc<'_>, ix: usize) -> Option<usize> {
    let next = *doc.elem_next_sibling.get(ix)?;
    (next != oa_xml::ElemIx::NONE).then_some(next.0 as usize)
}

fn attr_value<'a>(
    doc: &oa_xml::Doc<'a>,
    elem: usize,
    input: &'a [u8],
    want: &[u8],
    stats: &mut DecodeStats,
) -> Result<Option<&'a [u8]>, DecodeError> {
    let first = *doc
        .elem_first_attr
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, DecodeStats::default()))?;
    let count = *doc
        .elem_attr_count
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, DecodeStats::default()))?
        as usize;
    for i in first.0 as usize..first.0 as usize + count {
        work(stats)?;
        let n = span_bytes(
            input,
            *doc.attr_name
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, DecodeStats::default()))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, DecodeStats::default()))?;
        if n == want {
            if !matches!(doc.attr_prefix.get(i), Some(p) if p.len == 0) {
                return Err(err(DecodeFailure::InvalidValue, *stats));
            }
            let s = *doc
                .attr_value
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, DecodeStats::default()))?;
            if attr_needs_decode(doc, oa_xml::AttrIx(i as u32)) {
                return Err(err(DecodeFailure::InvalidValue, *stats));
            }
            return span_bytes(input, s)
                .map(Some)
                .map_err(|_| err(DecodeFailure::InvalidValue, *stats));
        }
    }
    Ok(None)
}

fn check_attributes(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    allowed_unqualified: &[&[u8]],
    stats: &mut DecodeStats,
) -> Result<(), DecodeError> {
    let first = *doc
        .elem_first_attr
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    let count = *doc
        .elem_attr_count
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))? as usize;
    for i in first.0 as usize..first.0 as usize + count {
        work(stats)?;
        let prefix = span_bytes(
            input,
            *doc.attr_prefix
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        let local = span_bytes(
            input,
            *doc.attr_name
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        // Namespace declarations are retained as XML attributes by oa-xml, but they are not
        // application attributes. Their values and reserved-prefix rules were validated by the
        // parser; don't treat `xmlns:iwxxm` as an unqualified IWXXM field here.
        if prefix == b"xmlns" || prefix.is_empty() && local == b"xmlns" {
            continue;
        }
        let accepted =
            match resolve_attr_ns(doc, oa_xml::ElemIx(elem as u32), oa_xml::AttrIx(i as u32)) {
                Ns::None => allowed_unqualified.contains(&local),
                Ns::Uri(ns) => {
                    let uri = oa_xml::ns_uri_bytes(doc, ns)
                        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
                    // This profile is inline-only; GML ownership controls are not consumed.
                    uri == GML && local == b"id" || uri == XSI && local == b"schemaLocation"
                }
                Ns::Xml => local == b"lang",
            };
        if !accepted {
            refuse(stats);
            return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
        }
    }
    Ok(())
}

fn require_gml_id(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(), DecodeError> {
    let first = *doc
        .elem_first_attr
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    let count = *doc
        .elem_attr_count
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))? as usize;
    for i in first.0 as usize..first.0 as usize + count {
        work(stats)?;
        let local = span_bytes(
            input,
            *doc.attr_name
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        if local == b"id" {
            if let Ns::Uri(ns) =
                resolve_attr_ns(doc, oa_xml::ElemIx(elem as u32), oa_xml::AttrIx(i as u32))
            {
                if oa_xml::ns_uri_bytes(doc, ns) == Some(GML) {
                    let value = *doc
                        .attr_value
                        .get(i)
                        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
                    if value.len == 0
                        || value.len as usize > MAX_SCALAR_BYTES
                        || attr_needs_decode(doc, oa_xml::AttrIx(i as u32))
                    {
                        return Err(err(DecodeFailure::InvalidValue, *stats));
                    }
                    return Ok(());
                }
            }
        }
    }
    Err(err(DecodeFailure::MissingRequiredField, *stats))
}

fn scalar_text(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<Vec<u8>, DecodeError> {
    if first_child(doc, elem).is_some() {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    work(stats)?;
    let mut out = Vec::new();
    let mut found = false;
    for (i, owner) in doc.text_owner.iter().enumerate() {
        work(stats)?;
        if owner.0 as usize == elem {
            if out.len() >= MAX_SCALAR_BYTES {
                return Err(err(DecodeFailure::Limit, *stats));
            }
            let span = *doc
                .text_span
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
            let mut buf = [0u8; MAX_SCALAR_BYTES];
            let len = decode_into(span, input, &mut buf, SpanKind::Text)
                .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
            if out
                .len()
                .checked_add(len)
                .filter(|n| *n <= MAX_SCALAR_BYTES)
                .is_none()
            {
                return Err(err(DecodeFailure::Limit, *stats));
            }
            let decoded = buf
                .get(..len)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
            out.extend_from_slice(decoded);
            found = true;
        }
    }
    if !found {
        return Err(err(DecodeFailure::MissingRequiredField, *stats));
    }
    let trimmed = trim_xml_ws(&out).ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    Ok(trimmed.to_vec())
}

fn trim_xml_ws(v: &[u8]) -> Option<&[u8]> {
    let mut a = 0;
    let mut b = v.len();
    while a < b {
        let byte = *v.get(a)?;
        if !matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
            break;
        }
        a = a.checked_add(1)?;
    }
    while b > a {
        let end = b.checked_sub(1)?;
        let byte = *v.get(end)?;
        if !matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
            break;
        }
        b = end;
    }
    v.get(a..b)
}

fn time_reference(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<i64, DecodeError> {
    check_attributes(doc, wrapper, input, &[], stats)?;
    let time = only_child(doc, wrapper, GML, b"TimeInstant", input, stats)?;
    require_gml_id(doc, time, input, stats)?;
    check_attributes(doc, time, input, &[], stats)?;
    let pos = only_child(doc, time, GML, b"timePosition", input, stats)?;
    check_attributes(doc, pos, input, &[], stats)?;
    let text = scalar_text(doc, pos, input, stats)?;
    parse_time(&text).ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))
}

fn aerodrome(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<String, DecodeError> {
    check_attributes(doc, wrapper, input, &[], stats)?;
    let aerodrome = only_child(doc, wrapper, AIXM, b"AirportHeliport", input, stats)?;
    require_gml_id(doc, aerodrome, input, stats)?;
    check_attributes(doc, aerodrome, input, &[], stats)?;
    // AirportHeliport has required sibling metadata such as gml:identifier; extract exactly one
    // inline timeSlice while leaving the other AIXM feature properties outside this projection.
    let time_slice = unique_child(doc, aerodrome, AIXM, b"timeSlice", input, stats)?;
    check_attributes(doc, time_slice, input, &[], stats)?;
    let slice = only_child(
        doc,
        time_slice,
        AIXM,
        b"AirportHeliportTimeSlice",
        input,
        stats,
    )?;
    require_gml_id(doc, slice, input, stats)?;
    check_attributes(doc, slice, input, &[], stats)?;
    let interpretation = unique_child(doc, slice, AIXM, b"interpretation", input, stats)?;
    check_attributes(doc, interpretation, input, &[], stats)?;
    match scalar_text(doc, interpretation, input, stats)?.as_slice() {
        b"BASELINE" => validate_aixm_slice_time(doc, slice, input, stats)?,
        b"SNAPSHOT" => validate_aixm_snapshot_time(doc, slice, input, stats)?,
        _ => {
            refuse(stats);
            return Err(DecodeError {
                failure: DecodeFailure::UnsupportedConstruct,
                stats: *stats,
                context: "aixm-interpretation",
            });
        }
    }
    let designator = unique_child(doc, slice, AIXM, b"designator", input, stats)?;
    check_attributes(doc, designator, input, &[], stats)?;
    let raw = scalar_text(doc, designator, input, stats)?;
    if raw.len() < 3 || raw.len() > 8 || !raw.iter().all(u8::is_ascii_alphanumeric) {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    String::from_utf8(raw).map_err(|_| err(DecodeFailure::InvalidValue, *stats))
}

fn validate_aixm_snapshot_time(
    doc: &oa_xml::Doc<'_>,
    slice: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(), DecodeError> {
    let valid = unique_child(doc, slice, GML, b"validTime", input, stats)?;
    check_attributes(doc, valid, input, &[], stats)?;
    if first_child(doc, valid).is_some() {
        refuse(stats);
        return Err(DecodeError {
            failure: DecodeFailure::UnsupportedConstruct,
            stats: *stats,
            context: "snapshot-validTime",
        });
    }
    no_significant_text(doc, valid, input, stats)
}

fn validate_aixm_slice_time(
    doc: &oa_xml::Doc<'_>,
    slice: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(), DecodeError> {
    // AirportHeliportTimeSlice contains other required siblings such as interpretation and
    // designator; select its one GML validTime property without rejecting those AIXM fields.
    let valid_time = unique_child(doc, slice, GML, b"validTime", input, stats)?;
    check_attributes(doc, valid_time, input, &[], stats)?;
    let period = only_child(doc, valid_time, GML, b"TimePeriod", input, stats)?;
    require_gml_id(doc, period, input, stats)?;
    let (begin, end) = exact_children2(
        doc,
        period,
        GML,
        b"beginPosition",
        b"endPosition",
        input,
        stats,
    )?;
    check_attributes(doc, begin, input, &[], stats)?;
    let begin_ms = parse_time(&scalar_text(doc, begin, input, stats)?)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    check_attributes(doc, end, input, &[b"indeterminatePosition"], stats)?;
    match attr_value(doc, end, input, b"indeterminatePosition", stats)? {
        Some(b"unknown") => {
            if first_child(doc, end).is_some() {
                return Err(err(DecodeFailure::InvalidValue, *stats));
            }
            no_significant_text(doc, end, input, stats)?;
            Ok(())
        }
        Some(_) => Err(err(DecodeFailure::UnsupportedConstruct, *stats)),
        None => {
            let end_ms = parse_time(&scalar_text(doc, end, input, stats)?)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
            if end_ms < begin_ms {
                return Err(err(DecodeFailure::InvalidValue, *stats));
            }
            Ok(())
        }
    }
}

fn unique_child(
    doc: &oa_xml::Doc<'_>,
    parent: usize,
    uri: &[u8],
    local: &[u8],
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<usize, DecodeError> {
    no_significant_text(doc, parent, input, stats)?;
    let mut found = None;
    let mut child = first_child(doc, parent);
    while let Some(ix) = child {
        work(stats)?;
        if elem_is(doc, ix, input, uri)?
            && name(doc, ix, input)? == local
            && found.replace(ix).is_some()
        {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        child = next_sibling(doc, ix);
    }
    found.ok_or_else(|| err(DecodeFailure::MissingRequiredField, *stats))
}

/// Require a property wrapper to contain precisely one inline element of the expected expanded
/// name. This is intentionally narrower than the schema's remote-reference alternatives.
fn only_child(
    doc: &oa_xml::Doc<'_>,
    parent: usize,
    uri: &[u8],
    local: &[u8],
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<usize, DecodeError> {
    no_significant_text(doc, parent, input, stats)?;
    let Some(ix) = first_child(doc, parent) else {
        return Err(err(DecodeFailure::MissingRequiredField, *stats));
    };
    work(stats)?;
    if !elem_is(doc, ix, input, uri)? || name(doc, ix, input)? != local {
        refuse(stats);
        return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
    }
    if next_sibling(doc, ix).is_some() {
        work(stats)?;
        refuse(stats);
        return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
    }
    Ok(ix)
}

/// Require the two explicit temporal endpoints accepted by this subset, in schema order.
fn exact_children2(
    doc: &oa_xml::Doc<'_>,
    parent: usize,
    uri: &[u8],
    first: &[u8],
    second: &[u8],
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(usize, usize), DecodeError> {
    no_significant_text(doc, parent, input, stats)?;
    let Some(a) = first_child(doc, parent) else {
        return Err(err(DecodeFailure::MissingRequiredField, *stats));
    };
    work(stats)?;
    let Some(b) = next_sibling(doc, a) else {
        return Err(err(DecodeFailure::MissingRequiredField, *stats));
    };
    work(stats)?;
    if next_sibling(doc, b).is_some() {
        work(stats)?;
        refuse(stats);
        return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
    }
    if !elem_is(doc, a, input, uri)?
        || name(doc, a, input)? != first
        || !elem_is(doc, b, input, uri)?
        || name(doc, b, input)? != second
    {
        refuse(stats);
        return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
    }
    Ok((a, b))
}

/// Element-only property wrappers may contain formatting whitespace, but no hidden text payload.
/// Charge every text-table visit so malformed structures cannot evade the per-call work ceiling.
fn no_significant_text(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(), DecodeError> {
    for (i, owner) in doc.text_owner.iter().enumerate() {
        work(stats)?;
        if owner.0 as usize != elem {
            continue;
        }
        let span = *doc
            .text_span
            .get(i)
            .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
        let raw = span_bytes(input, span).map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        if trim_xml_ws(raw)
            .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?
            .is_empty()
        {
            continue;
        }
        if raw.len() > MAX_SCALAR_BYTES {
            return Err(err(DecodeFailure::Limit, *stats));
        }
        let mut buf = [0u8; MAX_SCALAR_BYTES];
        let len = decode_into(span, input, &mut buf, SpanKind::Text)
            .map_err(|_| err(DecodeFailure::Limit, *stats))?;
        let decoded = buf
            .get(..len)
            .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
        if !trim_xml_ws(decoded)
            .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?
            .is_empty()
        {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
    }
    Ok(())
}

pub(crate) fn parse_time(v: &[u8]) -> Option<i64> {
    // Supported XML Schema dateTime subset: positive four-digit years, UTC `Z` only, and at most
    // millisecond precision. Offsets are deliberately refused until XML Schema's ±14:00 bound
    // and canonicalization rules are implemented here.
    if v.len() < 20 || v.last() != Some(&b'Z') {
        return None;
    }
    let year = dec(v.get(0..4)?)? as i32;
    let month = dec(v.get(5..7)?)? as u32;
    let day = dec(v.get(8..10)?)? as u32;
    if v.get(4) != Some(&b'-')
        || v.get(7) != Some(&b'-')
        || v.get(10) != Some(&b'T')
        || v.get(13) != Some(&b':')
        || v.get(16) != Some(&b':')
    {
        return None;
    }
    let hour = dec(v.get(11..13)?)? as i64;
    let minute = dec(v.get(14..16)?)? as i64;
    let second = dec(v.get(17..19)?)? as i64;
    if year == 0
        || month == 0
        || month > 12
        || day == 0
        || day > month_days(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let mut millis = 0i64;
    let mut end = 19usize;
    if v.get(end) == Some(&b'.') {
        end += 1;
        let start = end;
        while v.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        let digits = end - start;
        if !(1..=3).contains(&digits) {
            return None;
        }
        let n = dec(v.get(start..end)?)? as i64;
        millis = match digits {
            1 => n * 100,
            2 => n * 10,
            _ => n,
        };
    }
    if v.get(end..) != Some(b"Z") {
        return None;
    }
    let days = days_from_civil(year, month, day);
    days.checked_mul(86_400_000)?
        .checked_add(hour * 3_600_000 + minute * 60_000 + second * 1_000 + millis)
}

fn dec(v: &[u8]) -> Option<u32> {
    let mut n = 0u32;
    if v.is_empty() {
        return None;
    }
    for b in v {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((*b - b'0') as u32)?;
    }
    Some(n)
}
fn month_days(y: i32, m: u32) -> u32 {
    match m {
        2 if leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
fn leap(y: i32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = m as i32 + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64
}

fn decode_observation(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<NilValueObservation, DecodeError> {
    check_attributes(doc, wrapper, input, &[b"nilReason"], stats)?;
    if let Some(reason) = attr_value(doc, wrapper, input, b"nilReason", stats)? {
        if first_child(doc, wrapper).is_some() {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        no_significant_text(doc, wrapper, input, stats)?;
        if reason.is_empty() {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        if reason.len() > MAX_SCALAR_BYTES {
            return Err(err(DecodeFailure::Limit, *stats));
        }
        let reason_uri = Some(
            String::from_utf8(reason.to_vec())
                .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?,
        );
        return Ok(NilValueObservation::Nil { reason_uri });
    }
    let ix = only_child(
        doc,
        wrapper,
        IWXXM,
        b"MeteorologicalAerodromeObservation",
        input,
        stats,
    )?;
    require_gml_id(doc, ix, input, stats)?;
    no_significant_text(doc, ix, input, stats)?;
    check_attributes(doc, ix, input, &[b"cloudAndVisibilityOK"], stats)?;
    let cavok = match attr_value(doc, ix, input, b"cloudAndVisibilityOK", stats)? {
        Some(b"true") | Some(b"1") => true,
        Some(b"false") | Some(b"0") => false,
        _ => return Err(err(DecodeFailure::InvalidValue, *stats)),
    };
    let mut air_temperature = None;
    let mut dewpoint_temperature = None;
    let mut qnh = None;
    let mut surface_wind = None;
    let mut visibility = None;
    let mut rvr = Vec::with_capacity(MAX_RVR_ITEMS);
    let mut present_weather = Vec::with_capacity(MAX_PRESENT_WEATHER);
    let mut cloud = None;
    let mut recent_weather = Vec::with_capacity(MAX_RECENT_WEATHER);
    let mut wind_shear = None;
    let mut wind_shear_nil_reason_uri = None;
    let mut next_field = 0u8;
    let mut child = first_child(doc, ix);
    while let Some(field) = child {
        work(stats)?;
        if !elem_is(doc, field, input, IWXXM)? {
            refuse(stats);
            return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
        }
        match name(doc, field, input)? {
            b"airTemperature" if next_field == 0 => {
                air_temperature = Some(measure_value(
                    doc,
                    field,
                    input,
                    QuantityUnit::Celsius,
                    b"Cel",
                    stats,
                )?);
                next_field = 1;
            }
            b"dewpointTemperature" if next_field == 1 => {
                dewpoint_temperature = Some(measure_value(
                    doc,
                    field,
                    input,
                    QuantityUnit::Celsius,
                    b"Cel",
                    stats,
                )?);
                next_field = 2;
            }
            b"qnh" if next_field == 2 => {
                qnh = Some(measure_value(
                    doc,
                    field,
                    input,
                    QuantityUnit::Hectopascal,
                    b"hPa",
                    stats,
                )?);
                next_field = 3;
            }
            b"surfaceWind" if next_field == 3 => {
                surface_wind = Some(decode_surface_wind(doc, field, input, stats)?);
                next_field = 4;
            }
            b"visibility" if (4..=5).contains(&next_field) => {
                if visibility.is_some() {
                    return Err(err(DecodeFailure::InvalidValue, *stats));
                }
                visibility = Some(decode_visibility(doc, field, input, stats)?);
                next_field = 5;
            }
            b"rvr" if (4..=6).contains(&next_field) => {
                if rvr.len() >= MAX_RVR_ITEMS {
                    refuse(stats);
                    return Err(err(DecodeFailure::Limit, *stats));
                }
                rvr.push(decode_rvr(doc, field, input, stats)?);
                next_field = 6;
            }
            b"presentWeather" if (4..=7).contains(&next_field) => {
                if present_weather.len() >= MAX_PRESENT_WEATHER {
                    refuse(stats);
                    return Err(err(DecodeFailure::Limit, *stats));
                }
                present_weather.push(decode_weather(doc, field, input, stats)?);
                next_field = 7;
            }
            b"cloud" if (4..=8).contains(&next_field) => {
                if cloud.is_some() {
                    return Err(err(DecodeFailure::InvalidValue, *stats));
                }
                cloud = Some(decode_cloud(doc, field, input, stats)?);
                next_field = 8;
            }
            b"recentWeather" if (4..=9).contains(&next_field) => {
                if recent_weather.len() >= MAX_RECENT_WEATHER {
                    refuse(stats);
                    return Err(err(DecodeFailure::Limit, *stats));
                }
                recent_weather.push(decode_weather(doc, field, input, stats)?);
                next_field = 9;
            }
            b"windShear" if next_field <= 9 => {
                if wind_shear.is_some() {
                    return Err(err(DecodeFailure::InvalidValue, *stats));
                }
                let (value, nil_reason) = decode_wind_shear(doc, field, input, stats)?;
                wind_shear = value;
                wind_shear_nil_reason_uri = nil_reason;
                next_field = 10;
            }
            _ => {
                refuse(stats);
                let context = match name(doc, field, input)? {
                    b"seaCondition" => "observation.seaCondition",
                    b"recentWeather" => "observation.recentWeather",
                    b"windShear" => "observation.windShear",
                    b"extension" => "observation.extension",
                    b"visibility" => "observation.visibility",
                    b"rvr" => "observation.rvr",
                    b"presentWeather" => "observation.presentWeather",
                    b"cloud" => "observation.cloud",
                    _ => "observation.unknown-child",
                };
                return Err(DecodeError {
                    failure: DecodeFailure::UnsupportedConstruct,
                    stats: *stats,
                    context,
                });
            }
        }
        child = next_sibling(doc, field);
    }
    if air_temperature.is_none()
        || dewpoint_temperature.is_none()
        || qnh.is_none()
        || surface_wind.is_none()
    {
        return Err(err(DecodeFailure::MissingRequiredField, *stats));
    }
    if cavok
        && (visibility.is_some()
            || !rvr.is_empty()
            || !present_weather.is_empty()
            || cloud.is_some()
            || !recent_weather.is_empty()
            || wind_shear.is_some()
            || wind_shear_nil_reason_uri.is_some())
    {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    let wind = surface_wind.ok_or_else(|| err(DecodeFailure::MissingRequiredField, *stats))?;
    Ok(NilValueObservation::Observed(Observation {
        cloud_and_visibility_ok: cavok,
        surface_wind_nil_reason_uri: wind.nil_reason,
        wind_direction: wind.direction,
        wind_speed: wind.speed,
        wind_gust: wind.gust,
        variable_wind_direction: wind.variable_direction,
        prevailing_visibility: visibility.as_ref().and_then(|v| v.0.clone()),
        prevailing_visibility_operator: visibility.as_ref().and_then(|v| v.1),
        minimum_visibility: visibility.as_ref().and_then(|v| v.2.clone()),
        minimum_visibility_direction: visibility.and_then(|v| v.3),
        air_temperature,
        dewpoint_temperature,
        qnh,
        present_weather,
        recent_weather,
        runway_visual_ranges: rvr,
        wind_shear,
        wind_shear_nil_reason_uri,
        cloud,
    }))
}

type VisibilityFields = (
    Option<NilValue>,
    Option<RelationalOperator>,
    Option<NilValue>,
    Option<NilValue>,
);

fn decode_visibility(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<VisibilityFields, DecodeError> {
    check_attributes(doc, wrapper, input, &[b"nilReason"], stats)?;
    if let Some(reason) = attr_value(doc, wrapper, input, b"nilReason", stats)? {
        if first_child(doc, wrapper).is_some()
            || reason.is_empty()
            || reason.len() > MAX_SCALAR_BYTES
        {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        no_significant_text(doc, wrapper, input, stats)?;
        return Ok((
            Some(NilValue::Nil {
                reason_uri: Some(
                    String::from_utf8(reason.to_vec())
                        .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?,
                ),
            }),
            None,
            None,
            None,
        ));
    }
    let value = only_child(
        doc,
        wrapper,
        IWXXM,
        b"AerodromeHorizontalVisibility",
        input,
        stats,
    )?;
    check_attributes(doc, value, input, &[], stats)?;
    no_significant_text(doc, value, input, stats)?;
    let mut previous = 0u8;
    let mut prevailing = None;
    let mut operator = None;
    let mut minimum = None;
    let mut direction = None;
    let mut child = first_child(doc, value);
    while let Some(ix) = child {
        work(stats)?;
        if !elem_is(doc, ix, input, IWXXM)? {
            refuse(stats);
            return Err(DecodeError {
                failure: DecodeFailure::UnsupportedConstruct,
                stats: *stats,
                context: "visibility-child",
            });
        }
        match name(doc, ix, input)? {
            b"prevailingVisibility" if previous == 0 => {
                prevailing = Some(measure_value(
                    doc,
                    ix,
                    input,
                    QuantityUnit::Meter,
                    b"m",
                    stats,
                )?);
                previous = 1;
            }
            b"prevailingVisibilityOperator" if previous == 1 => {
                if matches!(prevailing.as_ref(), Some(NilValue::Nil { .. })) {
                    return Err(err(DecodeFailure::InvalidValue, *stats));
                }
                operator = Some(read_operator(doc, ix, input, stats)?);
                previous = 2;
            }
            b"minimumVisibility" if previous <= 2 => {
                minimum = Some(measure_value(
                    doc,
                    ix,
                    input,
                    QuantityUnit::Meter,
                    b"m",
                    stats,
                )?);
                previous = 3;
            }
            b"minimumVisibilityDirection" if previous <= 3 => {
                direction = Some(measure_value(
                    doc,
                    ix,
                    input,
                    QuantityUnit::Degree,
                    b"deg",
                    stats,
                )?);
                previous = 4;
            }
            _ => {
                refuse(stats);
                return Err(DecodeError {
                    failure: DecodeFailure::UnsupportedConstruct,
                    stats: *stats,
                    context: "visibility-child",
                });
            }
        }
        child = next_sibling(doc, ix);
    }
    if prevailing.is_none() {
        return Err(DecodeError {
            failure: DecodeFailure::MissingRequiredField,
            stats: *stats,
            context: "observation.visibility.prevailingVisibility",
        });
    }
    Ok((prevailing, operator, minimum, direction))
}

fn decode_weather(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<WeatherCode, DecodeError> {
    check_code_attributes(doc, elem, input, stats)?;
    if let Some(reason) = attr_value(doc, elem, input, b"nilReason", stats)? {
        if reason.is_empty()
            || reason.len() > MAX_SCALAR_BYTES
            || first_child(doc, elem).is_some()
            || xlink_href(doc, elem, input, stats)?.is_some()
        {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        no_significant_text(doc, elem, input, stats)?;
        return Ok(WeatherCode::Nil {
            reason_uri: Some(
                String::from_utf8(reason.to_vec())
                    .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?,
            ),
        });
    }
    let href = xlink_href(doc, elem, input, stats)?
        .ok_or_else(|| err(DecodeFailure::MissingRequiredField, *stats))?;
    if first_child(doc, elem).is_some() {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    no_significant_text(doc, elem, input, stats)?;
    Ok(WeatherCode::CodeUri(href))
}

fn check_code_attributes(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(), DecodeError> {
    let first = *doc
        .elem_first_attr
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    let count = *doc
        .elem_attr_count
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))? as usize;
    for i in first.0 as usize..first.0 as usize + count {
        work(stats)?;
        let prefix = span_bytes(
            input,
            *doc.attr_prefix
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        let local = span_bytes(
            input,
            *doc.attr_name
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        if prefix == b"xmlns" || prefix.is_empty() && local == b"xmlns" {
            continue;
        }
        let ns = resolve_attr_ns(doc, oa_xml::ElemIx(elem as u32), oa_xml::AttrIx(i as u32));
        let accepted = matches!(ns, Ns::None) && local == b"nilReason"
            || matches!(ns, Ns::Uri(s) if oa_xml::ns_uri_bytes(doc, s) == Some(XLINK) && local == b"href");
        if !accepted {
            refuse(stats);
            return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
        }
        if attr_needs_decode(doc, oa_xml::AttrIx(i as u32)) {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
    }
    Ok(())
}

fn xlink_href(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<Option<String>, DecodeError> {
    let first = *doc
        .elem_first_attr
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    let count = *doc
        .elem_attr_count
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))? as usize;
    let mut href = None;
    for i in first.0 as usize..first.0 as usize + count {
        work(stats)?;
        let local = span_bytes(
            input,
            *doc.attr_name
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        if local == b"href"
            && matches!(resolve_attr_ns(doc, oa_xml::ElemIx(elem as u32), oa_xml::AttrIx(i as u32)), Ns::Uri(s) if oa_xml::ns_uri_bytes(doc, s) == Some(XLINK))
        {
            if href.is_some() || attr_needs_decode(doc, oa_xml::AttrIx(i as u32)) {
                return Err(err(DecodeFailure::InvalidValue, *stats));
            }
            let span = *doc
                .attr_value
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
            let raw =
                span_bytes(input, span).map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
            if raw.is_empty() || raw.len() > MAX_SCALAR_BYTES {
                return Err(err(DecodeFailure::Limit, *stats));
            }
            href = Some(
                String::from_utf8(raw.to_vec())
                    .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?,
            );
        }
    }
    Ok(href)
}

fn decode_cloud(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<crate::Cloud, DecodeError> {
    check_attributes(doc, wrapper, input, &[b"nilReason"], stats)?;
    if let Some(reason) = attr_value(doc, wrapper, input, b"nilReason", stats)? {
        if first_child(doc, wrapper).is_some()
            || reason.is_empty()
            || reason.len() > MAX_SCALAR_BYTES
        {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        no_significant_text(doc, wrapper, input, stats)?;
        return Ok(crate::Cloud::Nil {
            reason_uri: Some(
                String::from_utf8(reason.to_vec())
                    .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?,
            ),
        });
    }
    let body = only_child(doc, wrapper, IWXXM, b"AerodromeCloud", input, stats)?;
    decode_cloud_body(doc, body, input, stats)
}

fn decode_cloud_body(
    doc: &oa_xml::Doc<'_>,
    body: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<crate::Cloud, DecodeError> {
    check_attributes(doc, body, input, &[], stats)?;
    no_significant_text(doc, body, input, stats)?;
    let mut layers = Vec::with_capacity(crate::MAX_CLOUD_LAYERS);
    let mut vertical_visibility = None;
    let mut child = first_child(doc, body);
    while let Some(ix) = child {
        work(stats)?;
        if !elem_is(doc, ix, input, IWXXM)? {
            refuse(stats);
            return Err(DecodeError {
                failure: DecodeFailure::UnsupportedConstruct,
                stats: *stats,
                context: "cloud-child",
            });
        }
        match name(doc, ix, input)? {
            b"layer" if layers.len() < crate::MAX_CLOUD_LAYERS => {
                let layer = only_child(doc, ix, IWXXM, b"CloudLayer", input, stats)?;
                check_attributes(doc, layer, input, &[], stats)?;
                no_significant_text(doc, layer, input, stats)?;
                let mut amount = None;
                let mut base = None;
                let mut cloud_type_uri = None;
                let mut cloud_type_nil_reason_uri = None;
                let mut field = first_child(doc, layer);
                let mut rank = 0;
                while let Some(item) = field {
                    if !elem_is(doc, item, input, IWXXM)? {
                        refuse(stats);
                        return Err(DecodeError {
                            failure: DecodeFailure::UnsupportedConstruct,
                            stats: *stats,
                            context: "cloud-layer-child",
                        });
                    }
                    match name(doc, item, input)? {
                        b"amount" if rank == 0 => {
                            check_code_attributes(doc, item, input, stats)?;
                            amount = Some(match decode_code_uri(doc, item, input, stats)? {
                                Some(uri) => crate::CloudAmount::CodeUri(uri),
                                None => crate::CloudAmount::Nil {
                                    reason_uri: attr_value(doc, item, input, b"nilReason", stats)?
                                        .map(|v| {
                                            String::from_utf8(v.to_vec()).map_err(|_| {
                                                err(DecodeFailure::InvalidValue, *stats)
                                            })
                                        })
                                        .transpose()?,
                                },
                            });
                            rank = 1;
                        }
                        b"base" if rank <= 1 => {
                            base = Some(measure_value_units(
                                doc,
                                item,
                                input,
                                &[(b"[ft_i]", QuantityUnit::Foot), (b"m", QuantityUnit::Meter)],
                                stats,
                            )?);
                            rank = 2;
                        }
                        b"cloudType" if rank <= 2 => {
                            check_code_attributes(doc, item, input, stats)?;
                            cloud_type_uri = decode_code_uri(doc, item, input, stats)?;
                            cloud_type_nil_reason_uri =
                                attr_value(doc, item, input, b"nilReason", stats)?
                                    .map(|value| {
                                        String::from_utf8(value.to_vec())
                                            .map_err(|_| err(DecodeFailure::InvalidValue, *stats))
                                    })
                                    .transpose()?;
                            rank = 3;
                        }
                        _ => {
                            refuse(stats);
                            return Err(DecodeError {
                                failure: DecodeFailure::UnsupportedConstruct,
                                stats: *stats,
                                context: "cloud-layer-child",
                            });
                        }
                    }
                    field = next_sibling(doc, item);
                }
                layers.push(crate::CloudLayer {
                    amount: amount
                        .ok_or_else(|| err(DecodeFailure::MissingRequiredField, *stats))?,
                    base: base.ok_or_else(|| err(DecodeFailure::MissingRequiredField, *stats))?,
                    cloud_type_uri,
                    cloud_type_nil_reason_uri,
                });
            }
            b"verticalVisibility" if vertical_visibility.is_none() && layers.is_empty() => {
                vertical_visibility = Some(measure_value_units(
                    doc,
                    ix,
                    input,
                    &[(b"[ft_i]", QuantityUnit::Foot), (b"m", QuantityUnit::Meter)],
                    stats,
                )?);
            }
            _ => {
                refuse(stats);
                return Err(DecodeError {
                    failure: DecodeFailure::UnsupportedConstruct,
                    stats: *stats,
                    context: "cloud-child",
                });
            }
        }
        child = next_sibling(doc, ix);
    }
    Ok(crate::Cloud::Observed {
        vertical_visibility,
        layers,
    })
}

fn decode_code_uri(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<Option<String>, DecodeError> {
    if let Some(reason) = attr_value(doc, elem, input, b"nilReason", stats)? {
        if reason.is_empty()
            || reason.len() > MAX_SCALAR_BYTES
            || xlink_href(doc, elem, input, stats)?.is_some()
            || first_child(doc, elem).is_some()
        {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        no_significant_text(doc, elem, input, stats)?;
        return Ok(None);
    }
    if first_child(doc, elem).is_some() {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    no_significant_text(doc, elem, input, stats)?;
    xlink_href(doc, elem, input, stats)?
        .map(Some)
        .ok_or_else(|| err(DecodeFailure::MissingRequiredField, *stats))
}

fn decode_rvr(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<RunwayVisualRange, DecodeError> {
    check_attributes(doc, wrapper, input, &[], stats)?;
    let body = only_child(
        doc,
        wrapper,
        IWXXM,
        b"AerodromeRunwayVisualRange",
        input,
        stats,
    )?;
    check_attributes(doc, body, input, &[b"pastTendency"], stats)?;
    no_significant_text(doc, body, input, stats)?;
    let tendency = attr_value(doc, body, input, b"pastTendency", stats)?
        .map(|v| match v {
            b"UPWARD" | b"DOWNWARD" | b"NO_CHANGE" | b"MISSING_VALUE" => {
                String::from_utf8(v.to_vec()).map_err(|_| err(DecodeFailure::InvalidValue, *stats))
            }
            _ => Err(err(DecodeFailure::InvalidValue, *stats)),
        })
        .transpose()?;
    let mut runway = None;
    let mut mean = None;
    let mut mean_operator = None;
    let mut rank = 0u8;
    let mut field = first_child(doc, body);
    while let Some(ix) = field {
        match name(doc, ix, input)? {
            b"runway" if runway.is_none() && rank == 0 => {
                runway = Some(decode_runway(doc, ix, input, stats)?);
                rank = 1;
            }
            b"meanRVR" if mean.is_none() && rank <= 1 => {
                mean = Some(measure_value(
                    doc,
                    ix,
                    input,
                    QuantityUnit::Meter,
                    b"m",
                    stats,
                )?);
                rank = 2;
            }
            b"meanRVROperator" if mean.is_some() && mean_operator.is_none() && rank <= 2 => {
                mean_operator = Some(read_operator(doc, ix, input, stats)?);
                rank = 3;
            }
            _ => {
                refuse(stats);
                return Err(DecodeError {
                    failure: DecodeFailure::UnsupportedConstruct,
                    stats: *stats,
                    context: "rvr-child",
                });
            }
        }
        field = next_sibling(doc, ix);
    }
    let mean_range = mean.ok_or_else(|| err(DecodeFailure::MissingRequiredField, *stats))?;
    if mean_operator.is_some() && matches!(mean_range, NilValue::Nil { .. }) {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    let (runway_designator, runway_nil_reason_uri) =
        runway.ok_or_else(|| err(DecodeFailure::MissingRequiredField, *stats))?;
    Ok(RunwayVisualRange {
        runway_designator,
        runway_nil_reason_uri,
        mean_range,
        mean_range_operator: mean_operator,
        past_tendency: tendency,
    })
}

fn decode_runway(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(Option<String>, Option<String>), DecodeError> {
    check_attributes(doc, wrapper, input, &[b"nilReason"], stats)?;
    if let Some(reason) = attr_value(doc, wrapper, input, b"nilReason", stats)? {
        if reason.is_empty()
            || reason.len() > MAX_SCALAR_BYTES
            || first_child(doc, wrapper).is_some()
        {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        no_significant_text(doc, wrapper, input, stats)?;
        return Ok((
            None,
            Some(
                String::from_utf8(reason.to_vec())
                    .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?,
            ),
        ));
    }
    let feature = only_child(doc, wrapper, AIXM, b"RunwayDirection", input, stats)?;
    let slice = only_child(
        doc,
        unique_child(doc, feature, AIXM, b"timeSlice", input, stats)?,
        AIXM,
        b"RunwayDirectionTimeSlice",
        input,
        stats,
    )?;
    let valid = unique_child(doc, slice, GML, b"validTime", input, stats)?;
    no_significant_text(doc, valid, input, stats)?;
    if first_child(doc, valid).is_some() {
        refuse(stats);
        return Err(DecodeError {
            failure: DecodeFailure::UnsupportedConstruct,
            stats: *stats,
            context: "runway-validTime",
        });
    }
    let interpretation = unique_child(doc, slice, AIXM, b"interpretation", input, stats)?;
    if scalar_text(doc, interpretation, input, stats)?.as_slice() != b"SNAPSHOT" {
        return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
    }
    let d = unique_child(doc, slice, AIXM, b"designator", input, stats)?;
    let raw = scalar_text(doc, d, input, stats)?;
    if raw.len() > MAX_SCALAR_BYTES {
        return Err(err(DecodeFailure::Limit, *stats));
    }
    Ok((
        Some(String::from_utf8(raw).map_err(|_| err(DecodeFailure::InvalidValue, *stats))?),
        None,
    ))
}

fn decode_wind_shear(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(Option<WindShear>, Option<String>), DecodeError> {
    check_attributes(doc, wrapper, input, &[b"nilReason"], stats)?;
    if let Some(reason) = attr_value(doc, wrapper, input, b"nilReason", stats)? {
        if reason.is_empty()
            || reason.len() > MAX_SCALAR_BYTES
            || first_child(doc, wrapper).is_some()
        {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        no_significant_text(doc, wrapper, input, stats)?;
        return Ok((
            None,
            Some(
                String::from_utf8(reason.to_vec())
                    .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?,
            ),
        ));
    }
    let body = only_child(doc, wrapper, IWXXM, b"AerodromeWindShear", input, stats)?;
    check_attributes(doc, body, input, &[b"allRunways"], stats)?;
    no_significant_text(doc, body, input, stats)?;
    let all = match attr_value(doc, body, input, b"allRunways", stats)? {
        Some(b"true") | Some(b"1") => true,
        Some(b"false") | Some(b"0") | None => false,
        _ => return Err(err(DecodeFailure::InvalidValue, *stats)),
    };
    let mut runways = Vec::with_capacity(4);
    let mut child = first_child(doc, body);
    while let Some(ix) = child {
        if name(doc, ix, input)? != b"runway"
            || !elem_is(doc, ix, input, IWXXM)?
            || runways.len() >= 4
        {
            refuse(stats);
            return Err(DecodeError {
                failure: DecodeFailure::UnsupportedConstruct,
                stats: *stats,
                context: "windShear-runway",
            });
        }
        let (designation, nil_reason) = decode_runway(doc, ix, input, stats)?;
        if nil_reason.is_some() {
            refuse(stats);
            return Err(DecodeError {
                failure: DecodeFailure::UnsupportedConstruct,
                stats: *stats,
                context: "windShear.runway.nil",
            });
        }
        if let Some(s) = designation {
            runways.push(s);
        }
        child = next_sibling(doc, ix);
    }
    if !all && runways.is_empty() {
        return Err(err(DecodeFailure::MissingRequiredField, *stats));
    }
    Ok((
        Some(WindShear {
            all_runways: all,
            runway_designators: runways,
        }),
        None,
    ))
}

fn decode_trend(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<TrendForecast, DecodeError> {
    check_attributes(doc, wrapper, input, &[], stats)?;
    let body = only_child(
        doc,
        wrapper,
        IWXXM,
        b"MeteorologicalAerodromeTrendForecast",
        input,
        stats,
    )?;
    require_gml_id(doc, body, input, stats)?;
    check_attributes(
        doc,
        body,
        input,
        &[b"changeIndicator", b"cloudAndVisibilityOK"],
        stats,
    )?;
    no_significant_text(doc, body, input, stats)?;
    let change = match attr_value(doc, body, input, b"changeIndicator", stats)? {
        Some(b"BECOMING") => TrendChange::Becoming,
        Some(b"TEMPORARY_FLUCTUATIONS") => TrendChange::TemporaryFluctuations,
        Some(b"FROM") => TrendChange::From,
        _ => return Err(err(DecodeFailure::InvalidValue, *stats)),
    };
    let mut time = None;
    let mut indicator = None;
    let mut visibility = None;
    let mut operator = None;
    let mut weather = Vec::with_capacity(MAX_PRESENT_WEATHER);
    let mut cloud = None;
    let mut child = first_child(doc, body);
    let mut stage = 0;
    while let Some(ix) = child {
        if !elem_is(doc, ix, input, IWXXM)? {
            refuse(stats);
            return Err(DecodeError {
                failure: DecodeFailure::UnsupportedConstruct,
                stats: *stats,
                context: "trend-child",
            });
        }
        match name(doc, ix, input)? {
            b"phenomenonTime" if stage == 0 => {
                time = Some(time_period_or_instant(doc, ix, input, stats)?);
                stage = 1;
            }
            b"timeIndicator" if stage <= 1 => {
                indicator = Some(match scalar_text(doc, ix, input, stats)?.as_slice() {
                    b"AT" => TrendTimeIndicator::At,
                    b"UNTIL" => TrendTimeIndicator::Until,
                    _ => return Err(err(DecodeFailure::InvalidValue, *stats)),
                });
                stage = 2;
            }
            b"prevailingVisibility" if stage <= 2 => {
                visibility = Some(measure_value(
                    doc,
                    ix,
                    input,
                    QuantityUnit::Meter,
                    b"m",
                    stats,
                )?);
                stage = 3;
            }
            b"prevailingVisibilityOperator" if stage <= 3 => {
                operator = Some(read_operator(doc, ix, input, stats)?);
                stage = 4;
            }
            b"weather" if stage <= 4 => {
                if weather.len() >= MAX_PRESENT_WEATHER {
                    return Err(err(DecodeFailure::Limit, *stats));
                }
                weather.push(decode_weather(doc, ix, input, stats)?);
                stage = 5;
            }
            b"cloud" if stage <= 5 && cloud.is_none() => {
                cloud = Some(decode_cloud(doc, ix, input, stats)?);
                stage = 6;
            }
            _ => {
                refuse(stats);
                return Err(DecodeError {
                    failure: DecodeFailure::UnsupportedConstruct,
                    stats: *stats,
                    context: "trend-child",
                });
            }
        }
        child = next_sibling(doc, ix);
    }
    let (start, end) = time.ok_or_else(|| err(DecodeFailure::MissingRequiredField, *stats))?;
    Ok(TrendForecast {
        change,
        time_start_ms: start,
        time_end_ms: end,
        time_indicator: indicator,
        prevailing_visibility: visibility,
        prevailing_visibility_operator: operator,
        weather,
        cloud,
    })
}

fn time_period_or_instant(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(i64, Option<i64>), DecodeError> {
    check_attributes(doc, wrapper, input, &[], stats)?;
    let Some(node) = first_child(doc, wrapper) else {
        return Err(err(DecodeFailure::MissingRequiredField, *stats));
    };
    if next_sibling(doc, node).is_some() {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    if elem_is(doc, node, input, GML)? && name(doc, node, input)? == b"TimeInstant" {
        require_gml_id(doc, node, input, stats)?;
        let p = unique_child(doc, node, GML, b"timePosition", input, stats)?;
        return Ok((
            parse_time(&scalar_text(doc, p, input, stats)?)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
            None,
        ));
    }
    if elem_is(doc, node, input, GML)? && name(doc, node, input)? == b"TimePeriod" {
        require_gml_id(doc, node, input, stats)?;
        let (a, b) = exact_children2(
            doc,
            node,
            GML,
            b"beginPosition",
            b"endPosition",
            input,
            stats,
        )?;
        let start = parse_time(&scalar_text(doc, a, input, stats)?)
            .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
        let end = parse_time(&scalar_text(doc, b, input, stats)?)
            .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
        if end < start {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        return Ok((start, Some(end)));
    }
    refuse(stats);
    Err(DecodeError {
        failure: DecodeFailure::UnsupportedConstruct,
        stats: *stats,
        context: "phenomenonTime",
    })
}

struct DecodedWind {
    nil_reason: Option<String>,
    direction: Option<NilValue>,
    speed: Option<NilValue>,
    gust: Option<NilValue>,
    variable_direction: Option<bool>,
}

fn decode_surface_wind(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<DecodedWind, DecodeError> {
    check_attributes(doc, wrapper, input, &[b"nilReason"], stats)?;
    if let Some(reason) = attr_value(doc, wrapper, input, b"nilReason", stats)? {
        if first_child(doc, wrapper).is_some() || reason.is_empty() {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        no_significant_text(doc, wrapper, input, stats)?;
        if reason.len() > MAX_SCALAR_BYTES {
            return Err(err(DecodeFailure::Limit, *stats));
        }
        let nil_reason = String::from_utf8(reason.to_vec())
            .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        return Ok(DecodedWind {
            nil_reason: Some(nil_reason),
            direction: None,
            speed: None,
            gust: None,
            variable_direction: None,
        });
    }
    let ix = only_child(doc, wrapper, IWXXM, b"AerodromeSurfaceWind", input, stats)?;
    decode_surface_wind_body(doc, ix, input, stats, b"AerodromeSurfaceWind")
}

fn decode_surface_wind_body(
    doc: &oa_xml::Doc<'_>,
    ix: usize,
    input: &[u8],
    stats: &mut DecodeStats,
    body_name: &[u8],
) -> Result<DecodedWind, DecodeError> {
    if !elem_is(doc, ix, input, IWXXM)? || name(doc, ix, input)? != body_name {
        return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
    }
    no_significant_text(doc, ix, input, stats)?;
    check_attributes(doc, ix, input, &[b"variableWindDirection"], stats)?;
    let variable_direction = match attr_value(doc, ix, input, b"variableWindDirection", stats)? {
        Some(b"true") | Some(b"1") => Some(true),
        Some(b"false") | Some(b"0") => Some(false),
        None => None,
        _ => return Err(err(DecodeFailure::InvalidValue, *stats)),
    };
    let mut direction = None;
    let mut speed = None;
    let mut speed_operator = None;
    let mut gust = None;
    let mut gust_operator = None;
    let mut next = 0u8;
    let mut child = first_child(doc, ix);
    while let Some(field) = child {
        work(stats)?;
        if !elem_is(doc, field, input, IWXXM)? {
            refuse(stats);
            return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
        }
        match name(doc, field, input)? {
            b"meanWindDirection" if next == 0 => {
                direction = Some(measure_value_units(
                    doc,
                    field,
                    input,
                    &[(b"deg", QuantityUnit::Degree)],
                    stats,
                )?);
                if let Some(NilValue::Measure { measure, .. }) = direction.as_ref() {
                    if measure.scale > 3
                        || measure.mantissa < 0
                        || measure.mantissa > 360 * 10i64.pow(measure.scale as u32)
                    {
                        return Err(err(DecodeFailure::InvalidValue, *stats));
                    }
                }
                next = 1;
            }
            b"meanWindSpeed" if next <= 1 => {
                speed = Some(measure_value_units(
                    doc,
                    field,
                    input,
                    &[
                        (b"m/s", QuantityUnit::MeterPerSecond),
                        (b"[kn_i]", QuantityUnit::Knot),
                    ],
                    stats,
                )?);
                if matches!(speed.as_ref(), Some(NilValue::Measure { measure: Measure { mantissa, .. }, .. }) if *mantissa < 0)
                {
                    return Err(err(DecodeFailure::InvalidValue, *stats));
                }
                next = 2;
            }
            b"meanWindSpeedOperator" if next == 2 => {
                speed_operator = Some(read_operator(doc, field, input, stats)?);
                next = 3;
            }
            b"windGustSpeed" if next <= 3 => {
                gust = Some(measure_value_units(
                    doc,
                    field,
                    input,
                    &[
                        (b"m/s", QuantityUnit::MeterPerSecond),
                        (b"[kn_i]", QuantityUnit::Knot),
                    ],
                    stats,
                )?);
                if matches!(gust.as_ref(), Some(NilValue::Measure { measure: Measure { mantissa, .. }, .. }) if *mantissa < 0)
                {
                    return Err(err(DecodeFailure::InvalidValue, *stats));
                }
                next = 4;
            }
            b"windGustSpeedOperator" if next == 4 => {
                gust_operator = Some(read_operator(doc, field, input, stats)?);
                next = 5;
            }
            _ => {
                refuse(stats);
                return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
            }
        }
        child = next_sibling(doc, field);
    }
    let mut speed = speed.ok_or_else(|| err(DecodeFailure::MissingRequiredField, *stats))?;
    if let Some(operator) = speed_operator {
        if let NilValue::Measure { operator: slot, .. } = &mut speed {
            *slot = Some(operator);
        } else {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
    }
    if let Some(operator) = gust_operator {
        if let Some(NilValue::Measure { operator: slot, .. }) = gust.as_mut() {
            *slot = Some(operator);
        } else {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
    }
    Ok(DecodedWind {
        nil_reason: None,
        direction,
        speed: Some(speed),
        gust,
        variable_direction,
    })
}

fn read_operator(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<RelationalOperator, DecodeError> {
    check_attributes(doc, elem, input, &[], stats)?;
    match scalar_text(doc, elem, input, stats)?.as_slice() {
        b"ABOVE" => Ok(RelationalOperator::Above),
        b"BELOW" => Ok(RelationalOperator::Below),
        _ => Err(err(DecodeFailure::InvalidValue, *stats)),
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, stats: &DecodeStats) -> Result<(), DecodeError> {
    if slot.replace(value).is_some() {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    Ok(())
}

fn measure_value(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    unit: QuantityUnit,
    uom: &[u8],
    stats: &mut DecodeStats,
) -> Result<NilValue, DecodeError> {
    measure_value_units(doc, elem, input, &[(uom, unit)], stats)
}

fn measure_value_units(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    units: &[(&[u8], QuantityUnit)],
    stats: &mut DecodeStats,
) -> Result<NilValue, DecodeError> {
    check_attributes(doc, elem, input, &[b"nilReason", b"uom"], stats)?;
    let nil_reason = attr_value(doc, elem, input, b"nilReason", stats)?;
    let unit_text = attr_value(doc, elem, input, b"uom", stats)?;
    if let Some(reason) = nil_reason {
        if reason.is_empty() {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        if unit_text.is_some() || first_child(doc, elem).is_some() {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        no_significant_text(doc, elem, input, stats)?;
        let reason = if reason.len() > MAX_SCALAR_BYTES {
            return Err(err(DecodeFailure::Limit, *stats));
        } else {
            Some(
                String::from_utf8(reason.to_vec())
                    .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?,
            )
        };
        return Ok(NilValue::Nil { reason_uri: reason });
    }
    let unit = units
        .iter()
        .find_map(|(name, unit)| (unit_text == Some(*name)).then_some(*unit))
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    let raw = scalar_text(doc, elem, input, stats)?;
    let raw = core::str::from_utf8(&raw).map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
    let value = parse_decimal(raw).ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    let decimal_scale = 10i128.pow(value.1 as u32);
    let scaled = value.0 as i128;
    match unit {
        QuantityUnit::Celsius if scaled * 100 < -27_315 * decimal_scale => {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        QuantityUnit::Hectopascal if value.0 <= 0 => {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        QuantityUnit::Degree if scaled < 0 || scaled > 360 * decimal_scale => {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        QuantityUnit::Meter | QuantityUnit::Foot if scaled < 0 => {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        _ => {}
    }
    Ok(NilValue::Measure {
        measure: Measure {
            mantissa: value.0,
            scale: value.1,
            unit,
        },
        operator: None,
    })
}

pub(crate) fn parse_decimal(s: &str) -> Option<(i64, u8)> {
    let (negative, body) = if let Some(body) = s.strip_prefix('-') {
        (true, body)
    } else if let Some(body) = s.strip_prefix('+') {
        (false, body)
    } else {
        (false, s)
    };
    let mut magnitude = 0u64;
    let mut scale = 0u8;
    let mut dot = false;
    let mut digits = 0usize;
    for b in body.bytes() {
        if b == b'.' && !dot {
            dot = true;
            continue;
        }
        if !b.is_ascii_digit() || dot && scale >= 6 {
            return None;
        }
        magnitude = magnitude.checked_mul(10)?.checked_add((b - b'0') as u64)?;
        if dot {
            scale += 1;
        }
        digits += 1;
    }
    if digits == 0 {
        return None;
    }
    let mantissa = if negative && magnitude == (i64::MAX as u64) + 1 {
        i64::MIN
    } else {
        let positive = i64::try_from(magnitude).ok()?;
        if negative {
            positive.checked_neg()?
        } else {
            positive
        }
    };
    Some((mantissa, scale))
}

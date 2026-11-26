#![allow(clippy::indexing_slicing, clippy::panic)]

use crate::{
    decode_report, DecodeFailure, DecodeResult, NilValue, QuantityUnit, ReportKind, ReportStatus,
};

const METAR: &[u8] = br#"<iwxxm:METAR xmlns:iwxxm="http://icao.int/iwxxm/2025-2"
 xmlns:aixm="http://www.aixm.aero/schema/5.1.1" xmlns:gml="http://www.opengis.net/gml/3.2"
 reportStatus="NORMAL" permissibleUsage="OPERATIONAL" automatedStation="true" gml:id="uuid.00000000-0000-4000-8000-000000000001">
 <iwxxm:issueTime><gml:TimeInstant gml:id="uuid.00000000-0000-4000-8000-000000000002"><gml:timePosition>2026-11-24T12:00:00Z</gml:timePosition></gml:TimeInstant></iwxxm:issueTime>
 <iwxxm:aerodrome><aixm:AirportHeliport gml:id="uuid.00000000-0000-4000-8000-000000000003">
  <gml:identifier codeSpace="urn:uuid">urn:uuid:00000000-0000-4000-8000-000000000001</gml:identifier>
  <aixm:timeSlice><aixm:AirportHeliportTimeSlice gml:id="uuid.00000000-0000-4000-8000-000000000004">
   <gml:validTime><gml:TimePeriod gml:id="uuid.00000000-0000-4000-8000-000000000005"><gml:beginPosition>2026-11-24T00:00:00Z</gml:beginPosition><gml:endPosition indeterminatePosition="unknown"/></gml:TimePeriod></gml:validTime>
   <aixm:interpretation>BASELINE</aixm:interpretation>
   <aixm:designator>KJFK</aixm:designator>
  </aixm:AirportHeliportTimeSlice></aixm:timeSlice>
 </aixm:AirportHeliport></iwxxm:aerodrome>
 <iwxxm:observationTime><gml:TimeInstant gml:id="uuid.00000000-0000-4000-8000-000000000006"><gml:timePosition>2026-11-24T11:55:00Z</gml:timePosition></gml:TimeInstant></iwxxm:observationTime>
 <iwxxm:observation><iwxxm:MeteorologicalAerodromeObservation gml:id="uuid.00000000-0000-4000-8000-000000000007" cloudAndVisibilityOK="true">
  <iwxxm:airTemperature uom="Cel">12.3</iwxxm:airTemperature>
  <iwxxm:dewpointTemperature uom="Cel">8.1</iwxxm:dewpointTemperature>
  <iwxxm:qnh uom="hPa">1013.2</iwxxm:qnh>
  <iwxxm:surfaceWind><iwxxm:AerodromeSurfaceWind gml:id="uuid.00000000-0000-4000-8000-000000000008" variableWindDirection="false">
   <iwxxm:meanWindDirection uom="deg">180</iwxxm:meanWindDirection>
   <iwxxm:meanWindSpeed uom="m/s">5.2</iwxxm:meanWindSpeed>
   <iwxxm:windGustSpeed uom="m/s">8.1</iwxxm:windGustSpeed>
  </iwxxm:AerodromeSurfaceWind></iwxxm:surfaceWind>
 </iwxxm:MeteorologicalAerodromeObservation></iwxxm:observation>
</iwxxm:METAR>"#;

const WMO_METAR: &[u8] = include_bytes!("../testdata/official-wmo-v2025-2/metar-A3-1.xml");
const WMO_SPECI: &[u8] = include_bytes!("../testdata/official-wmo-v2025-2/speci-A3-2.xml");
const WMO_TAF: &[u8] = include_bytes!("../testdata/official-wmo-v2025-2/taf-A5-1.xml");
const WMO_TAF_CANCEL: &[u8] = include_bytes!("../testdata/official-wmo-v2025-2/taf-A5-2.xml");
const WMO_METAR_NIL_COLLECTION: &[u8] =
    include_bytes!("../testdata/official-wmo-v2025-2/metar-NIL-collect.xml");
const WMO_METAR_TRANSLATION_FAILED: &[u8] =
    include_bytes!("../testdata/official-wmo-v2025-2/metar-translation-failed.xml");
const WMO_TAF_NIL_COLLECTION: &[u8] =
    include_bytes!("../testdata/official-wmo-v2025-2/taf-NIL-collect.xml");
const WMO_TAF_TRANSLATION_FAILED: &[u8] =
    include_bytes!("../testdata/official-wmo-v2025-2/taf-translation-failed.xml");

fn decode_supported(input: &[u8], context: &str) -> DecodeResult {
    match decode_report(input) {
        Ok(result) => result,
        Err(error) => panic!("{context}: decode refused: {error:?}"),
    }
}

fn required_time(text: &[u8]) -> i64 {
    match crate::decode::parse_time(text) {
        Some(value) => value,
        None => panic!("valid test timestamp expected"),
    }
}

fn required_taf(result: &mut crate::Report) -> crate::TafForecast {
    match result.taf.take() {
        Some(value) => value,
        None => panic!("TAF forecast projection expected"),
    }
}

// REQ: OA-IWXXM-005
#[test]
fn official_wmo_metar_speci_and_taf_values_are_retained() {
    let metar = decode_supported(WMO_METAR, "WMO 2025-2 METAR A3-1");
    let crate::NilValueObservation::Observed(obs) = metar.report.observation else {
        panic!("observation expected")
    };
    assert_eq!(
        obs.prevailing_visibility,
        Some(NilValue::Measure {
            measure: crate::Measure {
                mantissa: 600,
                scale: 0,
                unit: QuantityUnit::Meter
            },
            operator: None
        })
    );
    assert_eq!(obs.runway_visual_ranges.len(), 1);
    assert_eq!(
        obs.runway_visual_ranges[0].runway_designator.as_deref(),
        Some("12")
    );
    assert_eq!(obs.runway_visual_ranges[0].runway_nil_reason_uri, None);
    assert_eq!(obs.runway_visual_ranges[0].mean_range_operator, None);
    assert_eq!(obs.present_weather.len(), 2);
    let metar_xml = core::str::from_utf8(WMO_METAR).unwrap_or("");
    let runway_start = metar_xml.find("<iwxxm:runway>").unwrap_or(0);
    let runway_end = metar_xml.find("</iwxxm:runway>").unwrap_or(0) + "</iwxxm:runway>".len();
    let nil_runway = format!(
        "{}<iwxxm:runway nilReason=\"http://codes.wmo.int/common/nil/inapplicable\"/>{}",
        &metar_xml[..runway_start],
        &metar_xml[runway_end..]
    );
    let nil_runway = decode_supported(nil_runway.as_bytes(), "nil RVR runway fixture");
    let crate::NilValueObservation::Observed(nil_runway_obs) = nil_runway.report.observation else {
        panic!("observation expected")
    };
    assert_eq!(
        nil_runway_obs.runway_visual_ranges[0].runway_designator,
        None
    );
    assert_eq!(
        nil_runway_obs.runway_visual_ranges[0]
            .runway_nil_reason_uri
            .as_deref(),
        Some("http://codes.wmo.int/common/nil/inapplicable")
    );
    assert_eq!(
        obs.cloud.as_ref().map(|c| match c {
            crate::Cloud::Observed { layers, .. } => layers.len(),
            _ => 0,
        }),
        Some(2)
    );
    assert_eq!(metar.report.trends.len(), 2);
    let speci = decode_supported(WMO_SPECI, "WMO 2025-2 SPECI A3-2");
    let crate::NilValueObservation::Observed(obs) = speci.report.observation else {
        panic!("observation expected")
    };
    assert_eq!(
        obs.minimum_visibility_direction,
        Some(NilValue::Measure {
            measure: crate::Measure {
                mantissa: 45,
                scale: 0,
                unit: QuantityUnit::Degree
            },
            operator: None
        })
    );
    assert!(
        matches!(speci.report.trends.get(1).and_then(|trend| trend.weather.first()), Some(crate::WeatherCode::Nil { reason_uri: Some(reason) }) if reason == "http://codes.wmo.int/common/nil/nothingOfOperationalSignificance")
    );
    assert!(
        matches!(speci.report.trends.get(1).and_then(|trend| trend.cloud.as_ref()), Some(crate::Cloud::Nil { reason_uri: Some(reason) }) if reason == "http://codes.wmo.int/common/nil/nothingOfOperationalSignificance")
    );
    let taf = decode_supported(WMO_TAF, "WMO 2025-2 TAF A5-1");
    assert_eq!(taf.report.kind, ReportKind::Taf);
    assert_eq!(taf.report.observation_time_ms, None);
    assert!(matches!(
        taf.report.observation,
        crate::NilValueObservation::Missing
    ));
    let mut taf_report = taf.report;
    let forecast = required_taf(&mut taf_report);
    assert_eq!(
        forecast.valid_start_ms,
        required_time(b"2012-08-16T00:00:00Z")
    );
    assert_eq!(
        forecast.valid_end_ms,
        required_time(b"2012-08-16T18:00:00Z")
    );
    assert_eq!(forecast.base.change, crate::ForecastChange::Base);
    assert_eq!(
        forecast.base.prevailing_visibility,
        Some(NilValue::Measure {
            measure: crate::Measure {
                mantissa: 9000,
                scale: 0,
                unit: QuantityUnit::Meter
            },
            operator: None
        })
    );
    assert_eq!(forecast.changes.len(), 3);
    assert_eq!(forecast.changes[0].change, crate::ForecastChange::Becoming);
    assert_eq!(
        forecast.changes[1].change,
        crate::ForecastChange::TemporaryFluctuations
    );
    assert_eq!(forecast.changes[2].change, crate::ForecastChange::From);
    assert_eq!(
        forecast.changes[2].valid_start_ms,
        required_time(b"2012-08-16T12:30:00Z")
    );
    let probability = core::str::from_utf8(WMO_TAF)
        .unwrap_or("")
        .replace(
            "changeIndicator=\"BECOMING\"",
            "changeIndicator=\"PROBABILITY_30\"",
        )
        .replace(
            "changeIndicator=\"TEMPORARY_FLUCTUATIONS\"",
            "changeIndicator=\"PROBABILITY_40_TEMPORARY_FLUCTUATIONS\"",
        );
    let probability = decode_supported(
        probability.as_bytes(),
        "synthetic WMO-schema probability groups",
    );
    let mut probability_report = probability.report;
    let groups = required_taf(&mut probability_report).changes;
    assert_eq!(groups[0].change, crate::ForecastChange::Probability30);
    assert_eq!(
        groups[1].change,
        crate::ForecastChange::Probability40TemporaryFluctuations
    );
    let wrong_reference = core::str::from_utf8(WMO_TAF).unwrap_or("").replace(
        "href=\"#uuid.ed5d465f-51fb-4ecc-834a-2ee26bb1196f\"",
        "href=\"#unrelated-period\"",
    );
    assert!(
        matches!(decode_report(wrong_reference.as_bytes()), Err(e) if e.failure == DecodeFailure::UnsupportedConstruct && e.context == "taf.forecast.phenomenonTime.reference")
    );
    let unsupported_temperature = core::str::from_utf8(WMO_TAF).unwrap_or("").replace(
        "</iwxxm:MeteorologicalAerodromeForecast>",
        "<iwxxm:temperature/></iwxxm:MeteorologicalAerodromeForecast>",
    );
    assert!(
        matches!(decode_report(unsupported_temperature.as_bytes()), Err(e) if e.failure == DecodeFailure::UnsupportedConstruct && e.context == "taf.forecast.temperature")
    );
    let cancelled = match decode_report(WMO_TAF_CANCEL) {
        Err(error) => error,
        Ok(_) => panic!("cancelled TAF is outside the supported non-cancellation profile"),
    };
    assert_eq!(cancelled.context, "taf.cancelledReportValidPeriod");
}

// REQ: OA-IWXXM-005
#[test]
fn official_wmo_full_report_set_decodes_or_refuses_by_named_construct() {
    for (xml, label) in [
        (WMO_METAR_NIL_COLLECTION, "METAR NIL collection wrapper"),
        (WMO_TAF_NIL_COLLECTION, "TAF NIL collection wrapper"),
    ] {
        let error = match decode_report(xml) {
            Err(error) => error,
            Ok(_) => panic!("{label}: collection wrapper must not be treated as one report"),
        };
        assert_eq!(error.failure, DecodeFailure::UnsupportedRoot, "{label}");
        assert_eq!(error.context, "collect.MeteorologicalBulletin", "{label}");
    }

    for (xml, label) in [
        (WMO_METAR_TRANSLATION_FAILED, "METAR translation failed"),
        (WMO_TAF_TRANSLATION_FAILED, "TAF translation failed"),
    ] {
        let error = match decode_report(xml) {
            Err(error) => error,
            Ok(_) => panic!("{label}: failed translation must not be projected"),
        };
        assert_eq!(
            error.failure,
            DecodeFailure::UnsupportedConstruct,
            "{label}"
        );
        assert_eq!(error.context, "root.translationFailedTAC", "{label}");
    }
}

// REQ: OA-IWXXM-005
#[test]
fn retains_rvr_operator_and_refuses_duplicate_rvr_cardinality() {
    let xml = core::str::from_utf8(WMO_METAR).unwrap_or("");
    let with_operator = xml.replace(
        "<iwxxm:meanRVR uom=\"m\">1000</iwxxm:meanRVR>",
        "<iwxxm:meanRVR uom=\"m\">2000</iwxxm:meanRVR><iwxxm:meanRVROperator>ABOVE</iwxxm:meanRVROperator>",
    );
    let report = decode_supported(with_operator.as_bytes(), "RVR operator fixture");
    let crate::NilValueObservation::Observed(obs) = report.report.observation else {
        panic!("observation expected")
    };
    assert_eq!(
        obs.runway_visual_ranges[0].mean_range_operator,
        Some(crate::RelationalOperator::Above)
    );
    let mut too_many = xml.to_owned();
    let extra_rvr = "<iwxxm:rvr><iwxxm:AerodromeRunwayVisualRange><iwxxm:runway nilReason=\"http://codes.wmo.int/common/nil/inapplicable\"/><iwxxm:meanRVR uom=\"m\">50</iwxxm:meanRVR></iwxxm:AerodromeRunwayVisualRange></iwxxm:rvr>";
    for _ in 0..crate::MAX_RVR_ITEMS {
        too_many = too_many.replace("</iwxxm:rvr>", &format!("</iwxxm:rvr>{extra_rvr}"));
    }
    assert!(
        matches!(decode_report(too_many.as_bytes()), Err(e) if e.failure == DecodeFailure::Limit)
    );
}

// REQ: OA-IWXXM-001
#[test]
fn decodes_supported_metar_subset_with_units_status_and_times() {
    let result = decode_supported(METAR, "synthetic supported METAR fixture");
    assert_eq!(result.report.kind, ReportKind::Metar);
    assert_eq!(result.report.status, ReportStatus::Normal);
    assert_eq!(result.report.station, "KJFK");
    assert_eq!(result.report.issue_time_ms, 1_795_521_600_000);
    assert_eq!(result.report.observation_time_ms, Some(1_795_521_300_000));
    assert_eq!(result.report.automated_station, Some(true));
    let crate::NilValueObservation::Observed(observation) = result.report.observation else {
        panic!("observation expected")
    };
    assert!(observation.cloud_and_visibility_ok);
    assert_eq!(
        observation.wind_direction,
        Some(NilValue::Measure {
            measure: crate::Measure {
                mantissa: 180,
                scale: 0,
                unit: QuantityUnit::Degree
            },
            operator: None,
        })
    );
    assert_eq!(
        observation.wind_speed,
        Some(NilValue::Measure {
            measure: crate::Measure {
                mantissa: 52,
                scale: 1,
                unit: QuantityUnit::MeterPerSecond
            },
            operator: None,
        })
    );
    assert_eq!(
        observation.wind_gust,
        Some(NilValue::Measure {
            measure: crate::Measure {
                mantissa: 81,
                scale: 1,
                unit: QuantityUnit::MeterPerSecond
            },
            operator: None,
        })
    );
    assert_eq!(
        observation.air_temperature,
        Some(NilValue::Measure {
            measure: crate::Measure {
                mantissa: 123,
                scale: 1,
                unit: QuantityUnit::Celsius
            },
            operator: None,
        })
    );
}

// REQ: OA-IWXXM-002
#[test]
fn refuses_hazard_fields_instead_of_silently_dropping_them() {
    let xml = core::str::from_utf8(METAR).unwrap_or("");
    let hazard = xml.replace(
        "</iwxxm:MeteorologicalAerodromeObservation>",
        "<iwxxm:seaCondition/></iwxxm:MeteorologicalAerodromeObservation>",
    );
    let result = decode_report(hazard.as_bytes());
    assert!(
        matches!(result, Err(e) if e.failure == DecodeFailure::UnsupportedConstruct && e.stats.unsupported_constructs == 1 && e.context == "observation.seaCondition")
    );
}

// REQ: OA-IWXXM-005
#[test]
fn retains_recent_weather_and_all_runways_wind_shear() {
    let xml = core::str::from_utf8(METAR).unwrap_or("").replace(
        "xmlns:gml=\"http://www.opengis.net/gml/3.2\"",
        "xmlns:gml=\"http://www.opengis.net/gml/3.2\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"",
    ).replace("cloudAndVisibilityOK=\"true\"", "cloudAndVisibilityOK=\"false\"").replace(
        "</iwxxm:MeteorologicalAerodromeObservation>",
        "<iwxxm:recentWeather xlink:href=\"http://codes.wmo.int/49-2/AerodromeRecentWeather/TSRA\"/><iwxxm:windShear><iwxxm:AerodromeWindShear allRunways=\"true\"/></iwxxm:windShear></iwxxm:MeteorologicalAerodromeObservation>",
    );
    let result = decode_supported(xml.as_bytes(), "recent weather and wind shear fixture");
    let crate::NilValueObservation::Observed(obs) = result.report.observation else {
        panic!("observation expected")
    };
    assert_eq!(
        obs.recent_weather,
        vec![crate::WeatherCode::CodeUri(
            "http://codes.wmo.int/49-2/AerodromeRecentWeather/TSRA".to_owned()
        )]
    );
    assert_eq!(
        obs.wind_shear,
        Some(crate::WindShear {
            all_runways: true,
            runway_designators: Vec::new()
        })
    );
}

// REQ: OA-IWXXM-005
#[test]
fn refuses_present_but_empty_visibility_instead_of_collapsing_it_to_absent() {
    let xml = core::str::from_utf8(WMO_METAR).unwrap_or("").replace(
        "<iwxxm:prevailingVisibility uom=\"m\">600</iwxxm:prevailingVisibility>",
        "",
    );
    assert!(
        matches!(decode_report(xml.as_bytes()), Err(error) if error.failure == DecodeFailure::MissingRequiredField && error.context == "observation.visibility.prevailingVisibility")
    );
}

// REQ: OA-IWXXM-003
#[test]
fn rejects_wrong_units_malformed_time_and_oversized_document() {
    let xml = core::str::from_utf8(METAR).unwrap_or("");
    let missing_gml_id = xml.replace(" gml:id=\"uuid.00000000-0000-4000-8000-000000000001\"", "");
    assert!(
        matches!(decode_report(missing_gml_id.as_bytes()), Err(e) if e.failure == DecodeFailure::MissingRequiredField)
    );
    let wrong_unit = xml.replace("uom=\"Cel\"", "uom=\"K\"");
    assert!(
        matches!(decode_report(wrong_unit.as_bytes()), Err(e) if e.failure == DecodeFailure::InvalidValue)
    );
    let bad_time = xml.replace("2026-11-24T12:00:00Z", "2026-02-30T12:00:00Z");
    assert!(
        matches!(decode_report(bad_time.as_bytes()), Err(e) if e.failure == DecodeFailure::InvalidValue)
    );
    let negative_qnh = xml.replace("1013.2</iwxxm:qnh>", "-1</iwxxm:qnh>");
    assert!(
        matches!(decode_report(negative_qnh.as_bytes()), Err(e) if e.failure == DecodeFailure::InvalidValue)
    );
    let below_absolute_zero = xml.replace(
        "12.3</iwxxm:airTemperature>",
        "-273.16</iwxxm:airTemperature>",
    );
    assert!(
        matches!(decode_report(below_absolute_zero.as_bytes()), Err(e) if e.failure == DecodeFailure::InvalidValue)
    );
    let xsi_nil = xml.replace("xmlns:gml=\"http://www.opengis.net/gml/3.2\"", "xmlns:gml=\"http://www.opengis.net/gml/3.2\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"")
        .replace("<iwxxm:airTemperature uom=\"Cel\">12.3</iwxxm:airTemperature>", "<iwxxm:airTemperature xsi:nil=\"true\"/>");
    assert!(
        matches!(decode_report(xsi_nil.as_bytes()), Err(e) if e.failure == DecodeFailure::UnsupportedConstruct)
    );
    let huge = vec![b' '; crate::MAX_DOCUMENT_BYTES + 1];
    assert!(matches!(decode_report(&huge), Err(e) if e.failure == DecodeFailure::InputTooLarge));
}

// REQ: OA-IWXXM-003
#[test]
fn decimal_lexical_parser_is_exact_and_checked() {
    assert_eq!(crate::decode::parse_decimal("+12.30"), Some((1230, 2)));
    assert_eq!(crate::decode::parse_decimal("-0"), Some((0, 0)));
    assert_eq!(crate::decode::parse_decimal(""), None);
    assert_eq!(crate::decode::parse_decimal("."), None);
    assert_eq!(crate::decode::parse_decimal("1e3"), None);
    assert_eq!(crate::decode::parse_decimal("1.0000001"), None);
    assert_eq!(
        crate::decode::parse_decimal("9223372036854775807"),
        Some((i64::MAX, 0))
    );
    assert_eq!(crate::decode::parse_decimal("9223372036854775808"), None);
    assert_eq!(
        crate::decode::parse_decimal("-9223372036854775808"),
        Some((i64::MIN, 0))
    );
    assert_eq!(crate::decode::parse_decimal("-9223372036854775809"), None);
}

// REQ: OA-IWXXM-004
#[test]
fn accepts_supported_utc_z_timestamps_and_refuses_offsets() {
    assert_eq!(
        crate::decode::parse_time(b"2026-11-24T12:00:00.125Z"),
        Some(1_795_521_600_125)
    );
    assert_eq!(
        crate::decode::parse_time(b"2026-11-24T12:00:00.125+01:30"),
        None
    );
    assert_eq!(
        crate::decode::parse_time(b"2026-11-24T12:00:00+23:59"),
        None
    );
    assert_eq!(
        crate::decode::parse_time(b"2026-11-24T12:00:00.1234Z"),
        None
    );
    assert_eq!(crate::decode::parse_time(b"2026-02-29T12:00:00Z"), None);
    assert_eq!(crate::decode::parse_time(b"0000-01-01T00:00:00Z"), None);
    assert_eq!(crate::decode::parse_time(b"2026-11-24T24:00:00Z"), None);
    assert_eq!(crate::decode::parse_time(b"2026-11-24T12:00:60Z"), None);
}

// REQ: OA-IWXXM-001
#[test]
fn supports_knot_wind_operators_variable_flag_and_nil_group() {
    let xml = core::str::from_utf8(METAR).unwrap_or("");
    let knots = xml
        .replace("variableWindDirection=\"false\"", "variableWindDirection=\"true\"")
        .replace(
            "<iwxxm:meanWindSpeed uom=\"m/s\">5.2</iwxxm:meanWindSpeed>",
            "<iwxxm:meanWindSpeed uom=\"[kn_i]\">10</iwxxm:meanWindSpeed><iwxxm:meanWindSpeedOperator>ABOVE</iwxxm:meanWindSpeedOperator>",
        )
        .replace(
            "<iwxxm:windGustSpeed uom=\"m/s\">8.1</iwxxm:windGustSpeed>",
            "<iwxxm:windGustSpeed uom=\"[kn_i]\">18</iwxxm:windGustSpeed><iwxxm:windGustSpeedOperator>BELOW</iwxxm:windGustSpeedOperator>",
        );
    let result = decode_supported(knots.as_bytes(), "supported knot-wind fixture");
    let crate::NilValueObservation::Observed(observation) = result.report.observation else {
        panic!("observation expected")
    };
    assert_eq!(observation.variable_wind_direction, Some(true));
    assert_eq!(
        observation.wind_speed,
        Some(NilValue::Measure {
            measure: crate::Measure {
                mantissa: 10,
                scale: 0,
                unit: QuantityUnit::Knot
            },
            operator: Some(crate::RelationalOperator::Above),
        })
    );
    assert_eq!(
        observation.wind_gust,
        Some(NilValue::Measure {
            measure: crate::Measure {
                mantissa: 18,
                scale: 0,
                unit: QuantityUnit::Knot
            },
            operator: Some(crate::RelationalOperator::Below),
        })
    );
    let variable_without_direction = knots.replace(
        "<iwxxm:meanWindDirection uom=\"deg\">180</iwxxm:meanWindDirection>",
        "",
    );
    let variable_result = decode_supported(
        variable_without_direction.as_bytes(),
        "variable wind without direction",
    );
    let crate::NilValueObservation::Observed(variable_observation) =
        variable_result.report.observation
    else {
        panic!("observation expected")
    };
    assert_eq!(variable_observation.variable_wind_direction, Some(true));
    assert_eq!(variable_observation.wind_direction, None);

    let start = "<iwxxm:surfaceWind>";
    let end = "</iwxxm:surfaceWind>";
    let Some(from) = knots.find(start) else {
        panic!("fixture wind start")
    };
    let Some(to) = knots.find(end) else {
        panic!("fixture wind end")
    };
    let nil_group = format!(
        "{}<iwxxm:surfaceWind nilReason=\"inapplicable\"> \n </iwxxm:surfaceWind>{}",
        &knots[..from],
        &knots[to + end.len()..]
    );
    let nil_result = decode_supported(nil_group.as_bytes(), "nillable wind property");
    let crate::NilValueObservation::Observed(nil_observation) = nil_result.report.observation
    else {
        panic!("observation expected")
    };
    assert_eq!(
        nil_observation.surface_wind_nil_reason_uri.as_deref(),
        Some("inapplicable")
    );
    assert_eq!(nil_observation.wind_speed, None);

    let nil_speed = knots
        .replace(
            "<iwxxm:meanWindSpeed uom=\"[kn_i]\">10</iwxxm:meanWindSpeed>",
            "<iwxxm:meanWindSpeed nilReason=\"notObservable\"/>",
        )
        .replace(
            "<iwxxm:meanWindSpeedOperator>ABOVE</iwxxm:meanWindSpeedOperator>",
            "",
        );
    let nil_speed_result = decode_supported(nil_speed.as_bytes(), "nillable wind speed");
    let crate::NilValueObservation::Observed(nil_speed_observation) =
        nil_speed_result.report.observation
    else {
        panic!("observation expected")
    };
    assert_eq!(
        nil_speed_observation.wind_speed,
        Some(NilValue::Nil {
            reason_uri: Some(String::from("notObservable"))
        })
    );
}

// REQ: OA-IWXXM-001
#[test]
fn enforces_required_temperature_wind_and_cavok_subset() {
    let xml = core::str::from_utf8(METAR).unwrap_or("");
    let start = "<iwxxm:surfaceWind>";
    let end = "</iwxxm:surfaceWind>";
    let Some(from) = xml.find(start) else {
        panic!("fixture wind start")
    };
    let Some(to) = xml.find(end) else {
        panic!("fixture wind end")
    };
    let without_wind = format!("{}{}", &xml[..from], &xml[to + end.len()..]);
    assert!(
        matches!(decode_report(without_wind.as_bytes()), Err(e) if e.failure == DecodeFailure::MissingRequiredField)
    );
    let without_temperature = xml.replace(
        "<iwxxm:airTemperature uom=\"Cel\">12.3</iwxxm:airTemperature>",
        "",
    );
    assert!(
        matches!(decode_report(without_temperature.as_bytes()), Err(e) if e.failure == DecodeFailure::MissingRequiredField || e.failure == DecodeFailure::InvalidValue || e.failure == DecodeFailure::UnsupportedConstruct)
    );
    let false_cavok = xml.replace(
        "cloudAndVisibilityOK=\"true\"",
        "cloudAndVisibilityOK=\"false\"",
    );
    let false_cavok_result = decode_supported(
        false_cavok.as_bytes(),
        "non-CAVOK report without optional visibility",
    );
    let crate::NilValueObservation::Observed(false_cavok_observation) =
        false_cavok_result.report.observation
    else {
        panic!("observation expected")
    };
    assert!(!false_cavok_observation.cloud_and_visibility_ok);
    let bad_direction = xml.replace(
        ">180</iwxxm:meanWindDirection>",
        ">361</iwxxm:meanWindDirection>",
    );
    assert!(
        matches!(decode_report(bad_direction.as_bytes()), Err(e) if e.failure == DecodeFailure::InvalidValue)
    );
    let bad_speed = xml.replace(">5.2</iwxxm:meanWindSpeed>", ">-1</iwxxm:meanWindSpeed>");
    assert!(
        matches!(decode_report(bad_speed.as_bytes()), Err(e) if e.failure == DecodeFailure::InvalidValue)
    );
    let wrong_order = xml
        .replace("<iwxxm:airTemperature", "<iwxxm:dewpointTemperature")
        .replace("</iwxxm:airTemperature>", "</iwxxm:dewpointTemperature>");
    assert!(
        matches!(decode_report(wrong_order.as_bytes()), Err(e) if e.failure == DecodeFailure::UnsupportedConstruct || e.failure == DecodeFailure::InvalidValue)
    );
}

// REQ: OA-IWXXM-004
#[test]
fn preserves_absent_observation_and_refuses_unknown_report_status() {
    let xml = core::str::from_utf8(METAR).unwrap_or("");
    let start = " <iwxxm:observation><iwxxm:MeteorologicalAerodromeObservation";
    let end = "</iwxxm:MeteorologicalAerodromeObservation></iwxxm:observation>";
    let Some(from) = xml.find(start) else {
        panic!("fixture observation start")
    };
    let Some(to) = xml.find(end) else {
        panic!("fixture observation end")
    };
    let absent = format!("{}{}", &xml[..from], &xml[to + end.len()..]);
    let result = decode_supported(absent.as_bytes(), "schema-optional observation");
    assert!(matches!(
        result.report.observation,
        crate::NilValueObservation::Missing
    ));
    let nil_observation = format!(
        "{}<iwxxm:observation nilReason=\"inapplicable\"/>{}",
        &xml[..from],
        &xml[to + end.len()..]
    );
    let nil_result = decode_supported(nil_observation.as_bytes(), "schema-nillable observation");
    assert!(
        matches!(nil_result.report.observation, crate::NilValueObservation::Nil { reason_uri: Some(ref r) } if r == "inapplicable")
    );
    let cancelled = xml.replace("reportStatus=\"NORMAL\"", "reportStatus=\"CANCELLED\"");
    assert!(
        matches!(decode_report(cancelled.as_bytes()), Err(e) if e.failure == DecodeFailure::InvalidValue)
    );
    let no_status = xml.replace(" reportStatus=\"NORMAL\"", "");
    assert!(
        matches!(decode_report(no_status.as_bytes()), Err(e) if e.failure == DecodeFailure::InvalidValue)
    );
}

// REQ: OA-IWXXM-002
#[test]
fn refuses_wrong_expanded_names_and_extra_content_in_property_wrappers() {
    let xml = core::str::from_utf8(METAR).unwrap_or("");
    let wrong_namespace = xml
        .replace(
            "<gml:TimeInstant gml:id=\"uuid.00000000-0000-4000-8000-000000000002\">",
            "<TimeInstant xmlns=\"urn:wrong\" gml:id=\"uuid.00000000-0000-4000-8000-000000000002\">",
        )
        .replacen("</gml:TimeInstant>", "</TimeInstant>", 1);
    let wrong_namespace_result = decode_report(wrong_namespace.as_bytes());
    assert!(matches!(&wrong_namespace_result, Err(e)
        if e.failure == DecodeFailure::UnsupportedConstruct || e.failure == DecodeFailure::UnsupportedNamespace),
        "wrong-namespace time wrapper was not refused as a namespace/construct: {wrong_namespace_result:?}");

    let extra_child = xml.replace(
        "</iwxxm:issueTime>",
        "<iwxxm:unexpected/> </iwxxm:issueTime>",
    );
    assert!(matches!(decode_report(extra_child.as_bytes()), Err(e)
        if e.failure == DecodeFailure::UnsupportedConstruct));

    let non_whitespace_payload =
        xml.replace("</iwxxm:issueTime>", "not-a-property</iwxxm:issueTime>");
    assert!(
        matches!(decode_report(non_whitespace_payload.as_bytes()), Err(e)
        if e.failure == DecodeFailure::InvalidValue)
    );

    let observation_payload = xml.replace(
        "</iwxxm:MeteorologicalAerodromeObservation></iwxxm:observation>",
        "hidden</iwxxm:MeteorologicalAerodromeObservation></iwxxm:observation>",
    );
    assert!(
        matches!(decode_report(observation_payload.as_bytes()), Err(e)
        if e.failure == DecodeFailure::InvalidValue)
    );

    let wind_payload = xml.replace(
        "</iwxxm:AerodromeSurfaceWind></iwxxm:surfaceWind>",
        "hidden</iwxxm:AerodromeSurfaceWind></iwxxm:surfaceWind>",
    );
    assert!(matches!(decode_report(wind_payload.as_bytes()), Err(e)
        if e.failure == DecodeFailure::InvalidValue));

    let ownership_control = xml.replace("<iwxxm:issueTime>", "<iwxxm:issueTime gml:owns=\"true\">");
    assert!(matches!(decode_report(ownership_control.as_bytes()), Err(e)
        if e.failure == DecodeFailure::UnsupportedConstruct));

    let extra_time_endpoint = xml.replace(
        "</gml:TimePeriod>",
        "<gml:duration>P1D</gml:duration></gml:TimePeriod>",
    );
    assert!(
        matches!(decode_report(extra_time_endpoint.as_bytes()), Err(e)
        if e.failure == DecodeFailure::UnsupportedConstruct)
    );

    let whitespace_end = xml.replace(
        "<gml:endPosition indeterminatePosition=\"unknown\"/>",
        "<gml:endPosition indeterminatePosition=\"unknown\"> \n </gml:endPosition>",
    );
    assert!(decode_report(whitespace_end.as_bytes()).is_ok());

    let nested_end = xml.replace(
        "<gml:endPosition indeterminatePosition=\"unknown\"/>",
        "<gml:endPosition indeterminatePosition=\"unknown\"><gml:timePosition/></gml:endPosition>",
    );
    assert!(matches!(decode_report(nested_end.as_bytes()), Err(e)
        if e.failure == DecodeFailure::InvalidValue));
}

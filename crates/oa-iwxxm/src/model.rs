//! Plain normalized report values. Units stay attached to exact decimal values.

/// Maximum schema cardinality represented for cloud layers in one current observation.
pub const MAX_CLOUD_LAYERS: usize = 4;
/// Maximum schema cardinality represented for present weather in one observation.
pub const MAX_PRESENT_WEATHER: usize = 3;

/// The root report type supported by this decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportKind {
    Metar,
    Speci,
}

/// IWXXM issuance status, retained independently from data usability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportStatus {
    Normal,
    Amendment,
    Correction,
}

/// IWXXM permitted use. A non-operational report is still decoded as non-operational.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissibleUsage {
    Operational,
    NonOperational,
}

/// A supported decimal unit. Values are not silently converted or rounded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuantityUnit {
    Celsius,
    Hectopascal,
    Meter,
    Foot,
    MeterPerSecond,
    Knot,
    Degree,
}

/// An exact decimal value with its schema unit; `value = mantissa * 10^-scale`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measure {
    pub mantissa: i64,
    pub scale: u8,
    pub unit: QuantityUnit,
}

/// A relational limit on a measurement. Absence means the value is exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationalOperator {
    Above,
    Below,
}

/// A nillable measured quantity. Unknown nil-reason URIs are retained verbatim within the
/// per-scalar bound; `None` means the source element was nil without a reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NilValue {
    Measure {
        measure: Measure,
        operator: Option<RelationalOperator>,
    },
    Nil {
        reason_uri: Option<String>,
    },
}

/// Present-weather code-list value, kept as its exact URI rather than guessed into a local enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WeatherCode {
    CodeUri(String),
    Nil { reason_uri: Option<String> },
}

/// Cloud amount code-list value, kept as its exact URI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloudAmount {
    CodeUri(String),
    Nil { reason_uri: Option<String> },
}

/// One layer of cloud reported in the observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudLayer {
    pub amount: CloudAmount,
    pub base: NilValue,
    pub cloud_type_uri: Option<String>,
}

/// The current cloud group. Nil cloud and CAVOK are distinct from an empty layer list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cloud {
    Nil {
        reason_uri: Option<String>,
    },
    Observed {
        vertical_visibility: Option<NilValue>,
        layers: Vec<CloudLayer>,
    },
}

/// Current observed surface conditions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub cloud_and_visibility_ok: bool,
    /// Nil reason on the nillable surfaceWind property itself; distinct from nil members.
    pub surface_wind_nil_reason_uri: Option<String>,
    pub wind_direction: Option<NilValue>,
    pub wind_speed: Option<NilValue>,
    pub wind_gust: Option<NilValue>,
    pub variable_wind_direction: Option<bool>,
    pub prevailing_visibility: Option<NilValue>,
    pub prevailing_visibility_operator: Option<RelationalOperator>,
    pub minimum_visibility: Option<NilValue>,
    pub minimum_visibility_direction: Option<NilValue>,
    pub air_temperature: Option<NilValue>,
    pub dewpoint_temperature: Option<NilValue>,
    pub qnh: Option<NilValue>,
    pub present_weather: Vec<WeatherCode>,
    pub cloud: Option<Cloud>,
}

/// A decoded supported report. Times are Unix milliseconds UTC; observation time and issue time
/// are not conflated. `station` is the AIXM designator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub kind: ReportKind,
    pub status: ReportStatus,
    pub permissible_usage: PermissibleUsage,
    pub station: String,
    pub issue_time_ms: i64,
    pub observation_time_ms: i64,
    pub automated_station: Option<bool>,
    pub observation: NilValueObservation,
}

/// The report-level current observation, distinct from a nil individual field.
// A7.2: cold one-report output; size guard waived. Keep the owned value inline without per-report boxing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NilValueObservation {
    Observed(Observation),
    Nil {
        reason_uri: Option<String>,
    },
    /// The report schema permits the observation property to be absent.
    Missing,
}

//! The plain values the comparison works on. Times are Unix milliseconds, positions 1e-7 degree;
//! an absent altitude is `None`.

/// One OpenATCA one-second output row (A7.2: cold).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OaSample {
    pub t_ms: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub alt_ft: Option<f32>,
    pub vel_east_mps: f32,
    pub vel_north_mps: f32,
    pub coast: bool,
}

/// One ERAM published track position (A7.2: cold).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EramSample {
    pub t_ms: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub alt_ft: Option<i32>,
    pub coast: bool,
}

/// One ERAM flight (one GUFI) with its samples in time order.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct EramFlight {
    pub gufi: u128,
    /// 24-bit Mode S address, when a message carried one.
    pub addr: Option<u32>,
    /// Last beacon code a message gave, four octal digits read as a decimal number.
    pub beacon: Option<u16>,
    /// Flight identification, zero or space padded.
    pub callsign: Option<[u8; 8]>,
    pub samples: Vec<EramSample>,
}

/// The identity the OpenATCA adapter was offered, at one time: a change point of the squawk or
/// callsign of the aircraft's ADS-B reports.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentPoint {
    pub t_ms: i64,
    pub squawk: Option<u16>,
    pub callsign: Option<[u8; 8]>,
}

/// One OpenATCA aircraft: its output rows and the identity points, both in time order.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct OaAircraft {
    pub addr: u32,
    pub samples: Vec<OaSample>,
    pub idents: Vec<IdentPoint>,
}

impl OaAircraft {
    /// The identity points that apply in `[from_ms, to_ms]`: those inside it and the last one
    /// before it (still in force at its start).
    pub fn idents_in(&self, from_ms: i64, to_ms: i64) -> Vec<&IdentPoint> {
        let i = self.idents.partition_point(|p| p.t_ms < from_ms);
        let j = self.idents.partition_point(|p| p.t_ms <= to_ms);
        let start = i.saturating_sub(1);
        self.idents.get(start..j).unwrap_or(&[]).iter().collect()
    }
}

/// A callsign as text for comparison: trailing zeros and spaces removed, upper case.
pub fn callsign_text(c: &[u8; 8]) -> String {
    let n = c
        .iter()
        .rposition(|&b| b != 0 && b != b' ')
        .map_or(0, |p| p + 1);
    c.get(..n)
        .unwrap_or(&[])
        .iter()
        .map(|&b| char::from(b.to_ascii_uppercase()))
        .collect()
}

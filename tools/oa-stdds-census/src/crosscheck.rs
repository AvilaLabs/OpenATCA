//! SECOND MODE (stub): the cross-check of adsb=false tracks that carry a Mode S address against
//! an ADS-B recording of the same day. TODO: no ADS-B clip for 2026-11-20 exists yet, so the
//! command line does not reach this code; only the counting function exists, and it is tested
//! on synthetic samples alone. When a clip exists: read its (address, time) samples with the
//! ADS-B reader, collect the adsb=false (address, time) samples with a non-zero address during the census (the
//! field reads zero on most adsb=false updates, see the census), and call
//! [`crosscheck`]. Collecting real addresses and times stays in memory only and is never printed.

/// One (24-bit address, Unix milliseconds) sample.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub addr: u32,
    pub t_ms: i64,
}

/// Counts of the cross-check; no address or time is kept.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CrossCheck {
    /// Distinct addresses among the adsb=false samples.
    pub addresses: u64,
    /// Of those, the addresses with at least one adsb=false sample that has an ADS-B sample of
    /// the same address within the window.
    pub addresses_matched: u64,
    /// adsb=false samples, and of those the ones with an ADS-B sample of the same address
    /// within the window.
    pub samples: u64,
    pub samples_matched: u64,
}

/// Matches `radar_only` (adsb=false samples with an address) against `adsb` within
/// `window_ms` either side (inclusive).
pub fn crosscheck(radar_only: &[Sample], adsb: &[Sample], window_ms: i64) -> CrossCheck {
    let mut a: Vec<Sample> = adsb.to_vec();
    a.sort_by_key(|s| (s.addr, s.t_ms));
    let mut out = CrossCheck::default();
    let mut matched_addr: std::collections::HashMap<u32, bool> = std::collections::HashMap::new();
    for s in radar_only {
        let lo = (s.addr, s.t_ms.saturating_sub(window_ms));
        let i = a.partition_point(|x| (x.addr, x.t_ms) < lo);
        let hit = a
            .get(i)
            .is_some_and(|x| x.addr == s.addr && x.t_ms <= s.t_ms.saturating_add(window_ms));
        out.samples += 1;
        out.samples_matched += u64::from(hit);
        *matched_addr.entry(s.addr).or_insert(false) |= hit;
    }
    out.addresses = matched_addr.len() as u64;
    out.addresses_matched = matched_addr.values().filter(|&&m| m).count() as u64;
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn s(addr: u32, t_ms: i64) -> Sample {
        Sample { addr, t_ms }
    }

    // REQ: OA-CEN-006
    #[test]
    fn matches_within_ten_seconds_inclusive() {
        let radar = [
            s(1, 100_000),
            s(1, 200_000),
            s(2, 100_000),
            s(3, 100_000),
            s(4, 100_000),
        ];
        let adsb = [
            s(1, 110_000), // exactly +10 s: match
            s(2, 110_001), // just outside
            s(3, 89_999),  // just outside
            s(4, 95_000),  // match
            s(5, 100_000), // an address that is not in the radar set
        ];
        let c = crosscheck(&radar, &adsb, 10_000);
        assert_eq!(
            c,
            CrossCheck {
                addresses: 4,
                addresses_matched: 2,
                samples: 5,
                samples_matched: 2,
            }
        );
    }

    // REQ: OA-CEN-006
    #[test]
    fn empty_inputs_give_zero_counts() {
        assert_eq!(crosscheck(&[], &[s(1, 0)], 10_000), CrossCheck::default());
        let c = crosscheck(&[s(1, 0)], &[], 10_000);
        assert_eq!((c.addresses, c.addresses_matched, c.samples), (1, 0, 1));
    }

    // REQ: OA-CEN-006
    #[test]
    fn an_address_matches_if_any_one_of_its_samples_does() {
        let radar = [s(1, 0), s(1, 500_000)];
        let adsb = [s(1, 500_005)];
        let c = crosscheck(&radar, &adsb, 10_000);
        assert_eq!((c.addresses, c.addresses_matched), (1, 1));
        assert_eq!((c.samples, c.samples_matched), (2, 1));
    }
}

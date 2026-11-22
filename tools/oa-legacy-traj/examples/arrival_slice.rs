//! Public, fully synthetic Chicago arrival example. Run with
//! `cargo run -p oa-legacy-traj --example arrival_slice`.
//! No recordings, feeds, network, or private data are read.

use oa_legacy_traj::core::arrival::{
    IssueSnapshot, LegacyEta, RouteFix, TrackState, TruthPoint, forecast, score,
};
use oa_legacy_traj::core::types::Place;

fn point_on_route(route: [Place; 4], distance_m: f64) -> Place {
    let [first, second, third, last] = route;
    let mut remaining = distance_m;
    for (a, b) in [(first, second), (second, third), (third, last)] {
        let north = (b.lat_deg - a.lat_deg) * 111_195.0;
        let east = (b.lon_deg - a.lon_deg) * 83_000.0;
        let leg = (north * north + east * east).sqrt();
        if remaining <= leg {
            let fraction = remaining / leg;
            return Place {
                lat_deg: a.lat_deg + (b.lat_deg - a.lat_deg) * fraction,
                lon_deg: a.lon_deg + (b.lon_deg - a.lon_deg) * fraction,
            };
        }
        remaining -= leg;
    }
    last
}

fn main() -> Result<(), String> {
    let fix = RouteFix {
        name: *b"ORDMTR  ",
        place: Place {
            lat_deg: 41.70,
            lon_deg: -87.95,
        },
        altitude_constraint: None,
    };
    let jot = RouteFix {
        name: *b"JOT     ",
        place: Place {
            lat_deg: 41.52,
            lon_deg: -88.34,
        },
        altitude_constraint: None,
    };
    let ord = RouteFix {
        name: *b"ORD     ",
        place: Place {
            lat_deg: 41.98,
            lon_deg: -87.90,
        },
        altitude_constraint: None,
    };
    let snapshot = IssueSnapshot {
        cutoff_ms: 1_000_000,
        issued_ms: 1_000_000,
        track: TrackState {
            time_ms: 995_000,
            place: Place {
                lat_deg: 41.35,
                lon_deg: -88.45,
            },
            altitude_ft: Some(12_000.0),
            speed_mps: Some(60.0),
            vertical_rate_fpm: Some(-900.0),
        },
        assigned_altitude_ft: Some(5_000.0),
        assigned_altitude_provenance: Some(crate::core::arrival::ConstraintProvenance::Synthetic),
        route: vec![jot, fix, ord],
        meter_fix: Some(fix),
    };
    let forecast =
        forecast(&snapshot).map_err(|reason| format!("synthetic issue refused: {reason:?}"))?;
    let synthetic_route = [snapshot.track.place, jot.place, fix.place, ord.place];
    let truth = (0..=40)
        .map(|minute| TruthPoint {
            time_ms: snapshot.issued_ms + i64::from(minute) * 30_000,
            place: point_on_route(synthetic_route, 60.0 * 30.0 * f64::from(minute)),
            altitude_ft: Some(if 12_000.0 - 450.0 * f64::from(minute) > 5_000.0 {
                12_000.0 - 450.0 * f64::from(minute)
            } else {
                5_000.0
            }),
            gap_before: false,
        })
        .collect::<Vec<_>>();
    let result = score(
        &forecast,
        &truth,
        Some(fix),
        Some(LegacyEta {
            issued_ms: 990_000,
            eta_ms: 1_600_000,
        }),
        Some(LegacyEta {
            issued_ms: 999_000,
            eta_ms: 1_570_000,
        }),
        1_000.0,
    )
    .map_err(|reason| format!("synthetic score refused: {reason:?}"))?;

    println!(
        "synthetic arrival issue_ms={} coverage={}/{}",
        forecast.issued_ms, result.forecast_covered, result.forecast_possible
    );
    for ((minutes, point), error) in [5, 10, 20]
        .into_iter()
        .zip(forecast.positions)
        .zip(result.forecast)
    {
        println!(
            "{minutes}m lat={:?} lon={:?} alt_ft={:?} horizontal_error_m={:?} vertical_error_ft={:?}",
            point.place.map(|p| p.lat_deg),
            point.place.map(|p| p.lon_deg),
            point.altitude_ft,
            error.horizontal_error_m,
            error.vertical_error_ft
        );
    }
    println!(
        "meter_fix={} forecast_eta_ms={:?} actual_ms={:?} ERAM_error_s={:?} TBFM_error_s={:?}",
        String::from_utf8_lossy(&fix.name),
        forecast.meter_eta.map(|e| e.time_ms),
        result.actual_meter_ms,
        result.eram_eta_error_s,
        result.tb_fm_eta_error_s
    );
    Ok(())
}

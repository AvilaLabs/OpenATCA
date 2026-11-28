//! Snapshot-backed map inventory evidence (**shell**, B1/B3/B7).
//!
//! The replay renderer draws the model's actual scene through the same `View` and projection as
//! traffic. This module records a compact source/type inventory bound to the accepted snapshot;
//! it neither builds nor supplies geometry.

use oa_airspace::core::model::{self, boundary_source, point_kind, stratum, Model};
use std::fmt::Write as FmtWrite;

const POINT_KIND_SLOTS: usize = point_kind::MAX as usize + 1;
const BOUNDARY_SOURCE_SLOTS: usize = boundary_source::MAX as usize + 1;
const STRATUM_SLOTS: usize = stratum::MAX as usize + 1;

fn point_source_name(source: usize) -> &'static str {
    match source {
        0 => "NASR",
        1 => "CIFP",
        _ => "UNATTRIBUTED",
    }
}

fn boundary_source_name(source: u8) -> &'static str {
    match source {
        boundary_source::NASR_ARB => "NASR_ARB",
        boundary_source::CIFP_UC => "CIFP_UC",
        boundary_source::CIFP_UR => "CIFP_UR",
        _ => "UNKNOWN",
    }
}

/// Render source and geometry-kind counts bound to the model snapshot used by the runtime.
/// Counts are inventory rows, not claims that every row falls inside the current viewport.
pub(crate) fn render_inventory(
    model: &Model,
    snapshot_sha256: &[u8; 32],
) -> Result<String, String> {
    model::validate_model(model).map_err(|error| format!("map model validation: {error:?}"))?;

    let mut point_counts = [[0_usize; 3]; POINT_KIND_SLOTS];
    for (row, kind) in model.points.kind.iter().copied().enumerate() {
        let nasr = model::bit_get(&model.points.src_nasr, row);
        let cifp = model::bit_get(&model.points.src_cifp, row);
        let source_counts = point_counts
            .get_mut(usize::from(kind))
            .ok_or("map point kind outside validated range")?;
        if nasr {
            let count = source_counts
                .get_mut(0)
                .ok_or("map point source outside validated range")?;
            *count = count.checked_add(1).ok_or("map point inventory overflow")?;
        }
        if cifp {
            let count = source_counts
                .get_mut(1)
                .ok_or("map point source outside validated range")?;
            *count = count.checked_add(1).ok_or("map point inventory overflow")?;
        }
        if !nasr && !cifp {
            let count = source_counts
                .get_mut(2)
                .ok_or("map point source outside validated range")?;
            *count = count.checked_add(1).ok_or("map point inventory overflow")?;
        }
    }

    let mut boundary_counts = [[0_usize; STRATUM_SLOTS]; BOUNDARY_SOURCE_SLOTS];
    for (source, stratum) in model
        .boundaries
        .source
        .iter()
        .zip(&model.boundaries.stratum)
    {
        let source_ix = usize::from(*source);
        let stratum_ix = usize::from(*stratum);
        let count = boundary_counts
            .get_mut(source_ix)
            .and_then(|strata| strata.get_mut(stratum_ix))
            .ok_or("map boundary inventory code outside validated range")?;
        *count = count
            .checked_add(1)
            .ok_or("map boundary inventory overflow")?;
    }

    let mut evidence = String::from("OA_REGIONAL_MAP_EVIDENCE\t1\n");
    writeln!(
        evidence,
        "snapshot_sha256\t{}",
        oa_sha256::to_hex(snapshot_sha256)
    )
    .map_err(|_| "map evidence formatting failed")?;
    for (kind, sources) in point_counts.iter().enumerate().skip(1) {
        for (source, count) in sources.iter().copied().enumerate() {
            if count > 0 {
                writeln!(
                    evidence,
                    "point\t{}\t{}\t{}",
                    point_kind::name(kind as u8),
                    point_source_name(source),
                    count
                )
                .map_err(|_| "map evidence formatting failed")?;
            }
        }
    }
    for (source, strata) in boundary_counts.iter().enumerate().skip(1) {
        for (boundary_stratum, count) in strata.iter().copied().enumerate().skip(1) {
            if count > 0 {
                writeln!(
                    evidence,
                    "boundary\t{}\t{}\t{}",
                    boundary_source_name(source as u8),
                    stratum::name(boundary_stratum as u8),
                    count
                )
                .map_err(|_| "map evidence formatting failed")?;
            }
        }
    }
    Ok(evidence)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    // REQ: OA-REG-008; kills dropping source or boundary classification from the evidence.
    #[test]
    fn map_inventory_binds_source_geometry_counts_to_snapshot_identity() {
        let mut model = crate::cli::synthetic_model().expect("public synthetic adaptation");
        model::bit_set(&mut model.points.src_nasr, 0, true);
        model.vertices.extend([
            model::PosE7 {
                lat: 410_000_000,
                lon: -870_000_000,
            },
            model::PosE7 {
                lat: 410_000_000,
                lon: -869_000_000,
            },
            model::PosE7 {
                lat: 411_000_000,
                lon: -869_000_000,
            },
            model::PosE7 {
                lat: 411_000_000,
                lon: -870_000_000,
            },
        ]);
        let facility = *model.points.ident.first().expect("synthetic fix identity");
        let boundaries = &mut model.boundaries;
        boundaries.source.push(boundary_source::NASR_ARB);
        boundaries.stratum.push(stratum::ARTCC_HIGH);
        boundaries.edge_interp.push(model::edge_interp::UNKNOWN);
        boundaries.multiple.push(0);
        boundaries.facility.push(facility);
        boundaries.name.push(facility);
        boundaries.floor_ft.push(model::ALT_NONE);
        boundaries.ceiling_ft.push(model::ALT_NONE);
        boundaries.vertices.push(model::Span {
            start: model::VertexIx::new(0),
            len: 4,
        });
        boundaries.segs.push(model::Span {
            start: model::SegIx::new(0),
            len: 0,
        });
        model::bit_push(&mut boundaries.floor_agl, false);
        model::bit_push(&mut boundaries.ceiling_agl, false);
        boundaries.time_code.push(model::time_code::UNKNOWN);
        boundaries.level.push(model::level::UNKNOWN);
        model::bit_push(&mut boundaries.incomplete, false);
        let snapshot = oa_airspace::core::snapshot::encode(&model).expect("valid snapshot");
        let snapshot_sha256 = oa_sha256::sha256(&snapshot);

        let evidence = render_inventory(&model, &snapshot_sha256).expect("map inventory");

        assert!(evidence.contains(&format!(
            "snapshot_sha256\t{}\n",
            oa_sha256::to_hex(&snapshot_sha256)
        )));
        assert!(evidence.contains("point\tfix\tNASR\t1\n"));
        assert!(evidence.contains("point\tfix\tUNATTRIBUTED\t1\n"));
        assert!(evidence.contains("boundary\tNASR_ARB\tARTCC HIGH\t1\n"));
    }
}

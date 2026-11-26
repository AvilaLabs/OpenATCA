//! Minimal R-side demonstration composition (**core**, B1/B2).
//!
//! This is an OpenATCA difference view, not an ERAM replica. It composes the existing traffic and
//! typed-interaction display cores and adds a visible warning. The attached fidelity rows explain
//! which real-world task or display requirement motivated each group; they do not establish field
//! layout, command grammar, colors, or response parity.

use crate::core::interaction::{present_interaction, InteractionState};
use crate::core::list::{push_rect, push_text, DisplayError, DisplayList, ListCaps, ALIGN_LEFT};
use crate::core::palette;
use crate::core::project::Projection;
use crate::core::traffic::{build_traffic, TargetSet, TrafficStats, TrafficStyle};
use oa_geo::Ellipsoid;
use oa_workstation::{EntryOutcome, EntryReceipt, Position, R_SIDE};

const BANNER_TEXT: &str = "OPENATCA R-SIDE PROTOTYPE - NOT ERAM";
const BANNER_LAYER: u8 = 13;
const BANNER_HEIGHT_PX: i32 = 24;
const BANNER_TEXT_BYTES: usize = 48;

/// Provenance class for the evidence referenced by a visible demonstration group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceClass {
    PrimaryFaa,
    Community,
    Mixed,
    OpenAtcaOnly,
}

/// This implementation's status against its referenced fidelity row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DemoStatus {
    OpenAtcaDifference,
}

/// Default presentation and availability of one item in the R-side preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementAvailability {
    Visible,
    AvailableOpenAtcaInteraction,
    Unavailable,
    NotPresented,
}

/// Provenance attached to one visible group or interaction outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: static source metadata, not stored in a hot collection.
pub struct ElementTrace {
    pub trace_id: &'static str,
    pub element: &'static str,
    pub row_ids: &'static [&'static str],
    pub evidence: EvidenceClass,
    pub status: DemoStatus,
    pub availability: ElementAvailability,
    pub note: &'static str,
}

/// Rows the app should expose in its in-app differences list for this minimal screen.
pub const R_SIDE_TRACES: [ElementTrace; 20] = [
    ElementTrace {
        trace_id: "OA-F2-RSIDE-BANNER",
        element: "role-label",
        row_ids: &["ERAM-R-01"],
        evidence: EvidenceClass::PrimaryFaa,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "R-side is a documented position; this OpenATCA label and layout are not an ERAM screen spec.",
    },
    ElementTrace {
        trace_id: "OA-F2-SURVEILLANCE-PICTURE",
        element: "surveillance-picture",
        row_ids: &["ERAM-R-02"],
        evidence: EvidenceClass::PrimaryFaa,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "Uses the OpenATCA PVD-style display core; symbol and layout parity is unverified.",
    },
    ElementTrace {
        trace_id: "OA-F2-TRACK-SYMBOL",
        element: "tracking-target-symbol",
        row_ids: &[],
        evidence: EvidenceClass::OpenAtcaOnly,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "The square outline and centre dot are OpenATCA symbols; no deployed ERAM symbol is claimed.",
    },
    ElementTrace {
        trace_id: "OA-F2-COAST-MARKER",
        element: "coasting-marker-and-tag",
        row_ids: &[],
        evidence: EvidenceClass::OpenAtcaOnly,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "The X marker and CST label are OpenATCA encodings, not claimed ERAM symbology.",
    },
    ElementTrace {
        trace_id: "OA-F2-UNCERTAINTY-MARKER",
        element: "uncertainty-marker-radius-and-tags",
        row_ids: &[],
        evidence: EvidenceClass::OpenAtcaOnly,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "The diamond, radius circle, UNC and R95? labels are OpenATCA encodings.",
    },
    ElementTrace {
        trace_id: "OA-F2-REACQUISITION-MARKER",
        element: "reacquisition-ring-and-tag",
        row_ids: &[],
        evidence: EvidenceClass::OpenAtcaOnly,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "The ring and REACQ label are OpenATCA encodings; no current ERAM display is claimed.",
    },
    ElementTrace {
        trace_id: "OA-F2-CALLSIGN-CONFLICT",
        element: "identity-conflict-marker-and-tag",
        row_ids: &["ERAM-R-02"],
        evidence: EvidenceClass::PrimaryFaa,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "Identity responsibility is relevant context; the address substitution and CS? tag are OpenATCA-only.",
    },
    ElementTrace {
        trace_id: "OA-F2-TRAFFIC-IDENTITY-FIELD",
        element: "synthetic-callsign-or-address",
        row_ids: &["ERAM-R-02", "ERAM-R-07"],
        evidence: EvidenceClass::PrimaryFaa,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "Identity values and generation are synthetic; this is not an ERAM identity-entry workflow.",
    },
    ElementTrace {
        trace_id: "OA-F2-ALTITUDE-FIELD",
        element: "altitude-label",
        row_ids: &["ERAM-R-03", "ERAM-R-04", "ERAM-R-05"],
        evidence: EvidenceClass::PrimaryFaa,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "Altitude is shown in OpenATCA format; block field order and state encoding are not asserted.",
    },
    ElementTrace {
        trace_id: "OA-F2-GROUNDSPEED-FIELD",
        element: "ground-speed-label",
        row_ids: &["ERAM-R-03", "ERAM-R-04", "ERAM-R-06"],
        evidence: EvidenceClass::PrimaryFaa,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "Ground speed is in the OpenATCA three-line label; ERAM fourth-line layout is absent.",
    },
    ElementTrace {
        trace_id: "OA-F2-THREE-LINE-LABEL",
        element: "three-line-traffic-label-and-state-tags",
        row_ids: &["ERAM-R-03", "ERAM-R-04", "ERAM-R-05", "ERAM-R-06", "ERAM-R-11"],
        evidence: EvidenceClass::PrimaryFaa,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "The prototype label has three lines; the cited FAA requirements do not establish this layout.",
    },
    ElementTrace {
        trace_id: "OA-F2-TARGET-SELECTION",
        element: "pointer-target-selection-and-label",
        row_ids: &["ERAM-R-07"],
        evidence: EvidenceClass::PrimaryFaa,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::AvailableOpenAtcaInteraction,
        note: "Nearest-target hit testing and SELECT RAW/FLT labels are OpenATCA interactions, not verified ERAM controls.",
    },
    ElementTrace {
        trace_id: "OA-F2-OPENATCA-PALETTE",
        element: "openatca-palette-and-alert-color",
        row_ids: &["ERAM-R-20"],
        evidence: EvidenceClass::PrimaryFaa,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "FAA color policy is context only; the prototype palette is not claimed to match installed ERAM colors.",
    },
    ElementTrace {
        trace_id: "OA-F2-FOURTH-LINE-OMISSION",
        element: "fourth-line-content",
        row_ids: &["ERAM-R-06"],
        evidence: EvidenceClass::PrimaryFaa,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::NotPresented,
        note: "The preview's three-line label omits fourth-line content; no replacement field is invented.",
    },
    ElementTrace {
        trace_id: "OA-F2-CA-MCI-UNAVAILABLE",
        element: "conflict-alert-and-mode-c-intruder",
        row_ids: &["ERAM-R-13", "STARS-04"],
        evidence: EvidenceClass::PrimaryFaa,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Unavailable,
        note: "Scenario 1 has no adapted CA/MCI inputs; no trigger or presentation behavior is claimed.",
    },
    ElementTrace {
        trace_id: "OA-F2-MSAW-UNAVAILABLE",
        element: "minimum-safe-altitude-warning",
        row_ids: &["ERAM-R-14", "STARS-05"],
        evidence: EvidenceClass::PrimaryFaa,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Unavailable,
        note: "Scenario 1 has no adapted MSAW inputs; no trigger or presentation behavior is claimed.",
    },
    ElementTrace {
        trace_id: "OA-F2-SECTORS-ABSENT",
        element: "facility-sectors-and-frequencies",
        row_ids: &[],
        evidence: EvidenceClass::OpenAtcaOnly,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::NotPresented,
        note: "No real ZAU/C90 sector names, frequencies or facility adaptation are presented.",
    },
    ElementTrace {
        trace_id: "OA-F2-COMMANDS-UNAVAILABLE",
        element: "workstation-command-input",
        row_ids: &["ERAM-R-08", "ERAM-R-17", "ERAM-R-18"],
        evidence: EvidenceClass::Mixed,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Unavailable,
        note: "No command is enabled by default until exact input, operation and user-visible outcome are reviewed.",
    },
    ElementTrace {
        trace_id: "OA-F2-ENTRY-RECEIPT",
        element: "openatca-entry-outcome",
        row_ids: &["ERAM-R-09"],
        evidence: EvidenceClass::OpenAtcaOnly,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "OPENATCA ENTRY APPLIED/UNCHANGED/REFUSED is OpenATCA output, not ERAM response wording.",
    },
    ElementTrace {
        trace_id: "OA-F2-INTERACTION-STATUS",
        element: "typed-command-status-and-refusal-labels",
        row_ids: &["ERAM-R-09"],
        evidence: EvidenceClass::OpenAtcaOnly,
        status: DemoStatus::OpenAtcaDifference,
        availability: ElementAvailability::Visible,
        note: "CMD states and refusal labels are OpenATCA outputs, not ERAM text, color, or sound.",
    },
];

/// Additional capacities required by [`build_r_side`], to add to the map/traffic list caps.
pub const fn r_side_overlay_caps() -> ListCaps {
    ListCaps {
        lines: 0,
        rects: 1,
        circles: 0,
        polylines: 0,
        poly_vertices: 0,
        texts: 2,
        text_bytes: BANNER_TEXT_BYTES + 32,
    }
}

/// Current inputs from the shared demo engine. All scenario and flight rules remain outside this
/// display adapter; the target set and interaction state come from the existing core interfaces.
// A7.2: transient borrowed view passed once per frame.
pub struct RSideFrame<'a> {
    pub projection: &'a Projection,
    pub ellipsoid: &'a Ellipsoid,
    pub position: &'a Position,
    pub targets: &'a TargetSet,
    pub traffic_style: &'a TrafficStyle,
    pub interaction: &'a InteractionState,
    pub entry_receipt: Option<&'a EntryReceipt>,
}

/// Summary of what this demonstration frame appended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: one cold per-frame summary.
pub struct RSideFrameReport {
    pub traffic: TrafficStats,
    pub trace_count: u8,
}

/// Appends the existing OpenATCA traffic and command-status views plus an explicit fidelity banner.
/// The caller must also surface [`R_SIDE_TRACES`] in the in-app differences list.
pub fn build_r_side(
    list: &mut DisplayList,
    frame: &RSideFrame<'_>,
) -> Result<RSideFrameReport, DisplayError> {
    if frame.position.role != R_SIDE {
        return Err(DisplayError::BadView(
            "R-side demonstration requires an R-side position",
        ));
    }
    if frame.projection.height < 40 {
        return Err(DisplayError::BadView(
            "R-side prototype banner needs at least 40 pixels of height",
        ));
    }
    let traffic = build_traffic(
        list,
        frame.projection,
        frame.ellipsoid,
        frame.targets,
        frame.traffic_style,
    )?;
    let width = i32::try_from(frame.projection.width)
        .ok()
        .and_then(|w| w.checked_mul(16))
        .ok_or(DisplayError::BadView("R-side banner width overflows"))?;
    push_rect(
        list,
        (0, 0),
        (width, BANNER_HEIGHT_PX * 16),
        palette::BACKGROUND,
        12,
    )?;
    push_text(
        list,
        (8 * 16, 5 * 16),
        12,
        ALIGN_LEFT,
        palette::DIM_TEXT,
        BANNER_LAYER,
        BANNER_TEXT,
    )?;
    if let Some(receipt) = frame.entry_receipt {
        let (text, colour) = match receipt.outcome {
            EntryOutcome::Applied => ("OPENATCA ENTRY APPLIED", palette::TEXT),
            EntryOutcome::Unchanged => ("OPENATCA ENTRY UNCHANGED", palette::DIM_TEXT),
            EntryOutcome::Refused => ("OPENATCA ENTRY REFUSED", palette::ALERT),
        };
        push_text(
            list,
            (8 * 16, 27 * 16),
            11,
            ALIGN_LEFT,
            colour,
            BANNER_LAYER,
            text,
        )?;
    }
    present_interaction(frame.interaction, list, frame.projection)?;
    let trace_count = u8::try_from(R_SIDE_TRACES.len())
        .map_err(|_| DisplayError::BadView("R-side trace count exceeds its report field"))?;
    Ok(RSideFrameReport {
        traffic,
        trace_count,
    })
}

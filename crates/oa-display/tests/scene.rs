//! The static map over a small hand-built model near Chicago: what is drawn, in which colours and
//! layers, what is left out, overflow, determinism, and the command line end to end.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_airspace::model::*;
use oa_airspace::snapshot::{decode, encode};
use oa_display::cli;
use oa_display::list::*;
use oa_display::palette as pal;
use oa_display::project::{project, ring_radius16, NM_M};
use oa_display::raster::{frame_digest, frame_len, rasterize, render, FrameSpec, RenderError};
use oa_display::scene::*;
use oa_sha256::{sha256, to_hex};

const ORD: (i32, i32) = (419_786_000, -879_048_000);

fn e7(lat: f64, lon: f64) -> (i32, i32) {
    ((lat * 1e7).round() as i32, (lon * 1e7).round() as i32)
}

struct Strings(StrTable);

impl Strings {
    fn add(&mut self, s: &str) -> StrIx {
        self.0.text.push_str(s);
        self.0.offsets.push(self.0.text.len() as u32);
        StrIx::new((self.0.offsets.len() - 2) as u32)
    }
}

/// A boundary ring: source, stratum, facility and vertices.
type Ring = (u8, u8, &'static str, Vec<(i32, i32)>);

/// How the hand-built model's last airway leg ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tail {
    /// Two legs only.
    None,
    /// A third leg whose end is a published border crossing.
    Border,
    /// A third leg whose end is a reference that could not be found.
    Unresolved,
}

/// A valid model: six points (a non-IFR airport, two fixes, a VOR, an airport with a coded
/// approach, an NDB), own HIGH ring left open, a neighbour's HIGH ring, the own LOW ring already
/// closed, a class B ring that the map does not draw, a high airway (J80) of one leg and a low
/// one (V9) of one or two.
fn model(tail: Tail) -> Model {
    let mut m = empty_model(Meta {
        nasr_effective: 20_261_029,
        cifp_cycle: 2611,
        artcc: "ZAU".into(),
        buffer_m: 0,
        nasr_sha256: [1; 32],
        cifp_sha256: [2; 32],
    });
    let mut s = Strings(StrTable::default());
    let pts = [
        ("1C5", point_kind::AIRPORT, e7(41.85, -87.75)),
        ("AAAAA", point_kind::FIX, e7(42.30, -88.30)),
        ("BBBBB", point_kind::FIX, e7(41.60, -87.40)),
        ("DPA", point_kind::VOR, e7(41.90, -88.26)),
        ("KORD", point_kind::AIRPORT, e7(41.9786, -87.9048)),
        ("ZNB", point_kind::NDB, e7(42.10, -87.70)),
    ];
    let region = s.add("K5");
    let empty = s.add("");
    for (ident, kind, (lat, lon)) in pts {
        let ix = s.add(ident);
        let p = &mut m.points;
        p.kind.push(kind);
        p.lat.push(lat);
        p.lon.push(lon);
        p.elev_ft.push(ELEV_NONE);
        p.ident.push(ix);
        p.alt_ident.push(StrIx::NONE);
        p.region.push(region);
        p.scope.push(empty);
        p.name.push(StrIx::NONE);
    }
    let n = pts.len();
    m.points.src_nasr = bit_zeros(n);
    m.points.src_cifp = bit_zeros(n);
    m.points.in_area = bit_zeros(n);
    m.points.cls_b = bit_zeros(n);
    m.points.cls_c = bit_zeros(n);
    m.points.cls_d = bit_zeros(n);
    m.points.cls_e = bit_zeros(n);

    // KORD has a coded approach; 1C5 has none.
    m.procs.kind.push(proc_kind::APPROACH);
    m.procs.airport.push(PointIx::new(4));
    m.procs.ident.push(s.add("I28R"));
    m.procs.name.push(StrIx::NONE);
    m.procs.routes.push(Span {
        start: RouteIx::new(0),
        len: 0,
    });
    m.procs.src_nasr = bit_zeros(1);
    m.procs.src_cifp = bit_zeros(1);
    m.procs.coded = bit_zeros(1);
    bit_set(&mut m.procs.coded, 0, true);

    let square = |lat: f64, lon: f64, d: f64| {
        vec![
            e7(lat + d, lon - d),
            e7(lat + d, lon + d),
            e7(lat - d, lon + d),
            e7(lat - d, lon - d),
        ]
    };
    let mut low = square(41.9, -87.9, 0.5);
    low.push(low[0]);
    let rings: Vec<Ring> = vec![
        (
            boundary_source::NASR_ARB,
            stratum::ARTCC_HIGH,
            "ZAU",
            square(41.9, -87.9, 0.8),
        ),
        (
            boundary_source::NASR_ARB,
            stratum::ARTCC_HIGH,
            "ZMP",
            square(43.2, -87.9, 0.4),
        ),
        (boundary_source::NASR_ARB, stratum::ARTCC_LOW, "ZAU", low),
        (
            boundary_source::CIFP_UC,
            stratum::CLASS_B,
            "KORD",
            square(41.98, -87.9, 0.2),
        ),
    ];
    let b = &mut m.boundaries;
    let mut next = 0u32;
    for (source, strat, fac, ring) in &rings {
        b.source.push(*source);
        b.stratum.push(*strat);
        b.edge_interp.push(edge_interp::PER_SEGMENT);
        b.multiple.push(0);
        b.facility.push(s.add(fac));
        b.name.push(StrIx::NONE);
        b.floor_ft.push(ALT_NONE);
        b.ceiling_ft.push(ALT_NONE);
        b.time_code.push(time_code::UNKNOWN);
        b.level.push(level::UNKNOWN);
        b.vertices.push(Span {
            start: VertexIx::new(next),
            len: ring.len() as u32,
        });
        b.segs.push(Span {
            start: SegIx::new(0),
            len: 0,
        });
        next += ring.len() as u32;
        for (lat, lon) in ring {
            m.vertices.push(PosE7 {
                lat: *lat,
                lon: *lon,
            });
        }
    }
    b.floor_agl = bit_zeros(rings.len());
    b.ceiling_agl = bit_zeros(rings.len());

    let p = PointIx::new;
    let mut legs = vec![(p(1), p(3)), (p(3), p(2))];
    if tail != Tail::None {
        legs.push((p(2), PointIx::NONE));
    }
    m.airways.designator.push(s.add("J80"));
    m.airways.designator.push(s.add("V9"));
    m.airways.location.push(b'J');
    m.airways.location.push(b'V');
    m.airways.legs.push(Span {
        start: AirwayLegIx::new(0),
        len: 1,
    });
    m.airways.legs.push(Span {
        start: AirwayLegIx::new(1),
        len: legs.len() as u32 - 1,
    });
    for (f, t) in &legs {
        let l = &mut m.airway_legs;
        l.from.push(*f);
        l.to.push(*t);
        l.mea_ft.push(ALT_NONE);
        l.mea_opp_ft.push(ALT_NONE);
        l.gps_mea_ft.push(ALT_NONE);
        l.mea_dir.push(0);
        l.gps_mea_dir.push(0);
    }
    m.airway_legs.gap = bit_zeros(legs.len());
    m.airway_legs.signal_gap = bit_zeros(legs.len());
    m.airway_legs.dogleg = bit_zeros(legs.len());
    if tail != Tail::None {
        let u = &mut m.unresolved;
        u.role.push(if tail == Tail::Border {
            unresolved_role::BORDER_CROSSING
        } else {
            unresolved_role::AIRWAY_TO
        });
        u.owner_kind.push(owner_kind::AIRWAY);
        u.owner.push(s.add("V9"));
        u.scope.push(s.add("V"));
        // Segment 2 of V9: the end of its second leg.
        u.ordinal.push(2);
        u.name.push(s.add(if tail == Tail::Border {
            "U.S. CANADIAN BORDER-1"
        } else {
            "NOWHERE"
        }));
        u.region.push(empty);
        u.wanted.push(empty);
    }
    m.strings = s.0;
    m
}

fn spec(w: u32, h: u32) -> FrameSpec {
    FrameSpec {
        width: w,
        height: h,
        stride: w * 4,
    }
}

fn texts(l: &DisplayList) -> Vec<&str> {
    l.texts
        .start
        .iter()
        .zip(&l.texts.len)
        .map(|(s, n)| &l.text[*s as usize..(*s + *n) as usize])
        .collect()
}

/// A built list, its stats and the view, for the hand-built model.
fn build(m: &Model, range: f64, tweak: impl Fn(&mut View)) -> (DisplayList, SceneStats, View) {
    let mut view = view_new(ORD.0, ORD.1, range, 800, 600).unwrap();
    tweak(&mut view);
    let mut l = list_new(scene_caps(m));
    let st = build_scene(
        &mut l,
        m,
        &scene_index(m),
        &view,
        &mut scratch_for(m, &view),
    )
    .unwrap();
    assert!(!l.faulted);
    (l, st, view)
}

fn render_buf(l: &DisplayList, w: u32, h: u32) -> Vec<u8> {
    let sp = spec(w, h);
    let mut buf = vec![0u8; frame_len(&sp)];
    rasterize(l, &sp, &mut buf).unwrap();
    buf
}

fn pixel(buf: &[u8], w: u32, x: usize, y: usize) -> u32 {
    let o = (y * w as usize + x) * 4;
    u32::from_le_bytes(buf[o..o + 4].try_into().unwrap())
}

/// How many pixels of the frame have this palette colour.
fn count_colour(buf: &[u8], colour: u8) -> usize {
    let want = pal::PALETTE[usize::from(colour)];
    buf.as_chunks::<4>()
        .0
        .iter()
        .filter(|p| u32::from_le_bytes(**p) == want)
        .count()
}

/// Every range threshold wide open: all that the model has is shown.
fn all_open(v: &mut View) {
    v.fix_max_nm = 1e9;
    v.terminal_wpt_max_nm = 1e9;
    v.runway_max_nm = 1e9;
    v.non_ifr_airport_max_nm = 1e9;
    v.minor_navaid_max_nm = 1e9;
    v.label_max_nm = 1e9;
    v.major_label_max_nm = 1e9;
}

// REQ: OA-DSP-017
#[test]
fn the_map_draws_boundaries_airways_symbols_rings_and_legend_in_their_colours() {
    let m = model(Tail::None);
    validate_model(&m).unwrap();
    let (l, st, view) = build(&m, 50.0, all_open);
    // With every threshold open everything is drawn.
    assert_eq!((st.own_boundaries, st.neighbour_boundaries), (2, 1));
    assert_eq!((st.fixes, st.navaids, st.airports), (2, 2, 2));
    assert_eq!(st.airway_legs, 2);
    assert_eq!(
        (
            st.points_unplaced,
            st.legs_unplaced,
            st.edges_unplaced,
            st.legs_at_border
        ),
        (0, 0, 0, 0)
    );

    // Three polylines (class B is not drawn): own HIGH, neighbour HIGH, own LOW.
    assert_eq!(
        l.polylines.colour,
        vec![pal::ARTCC_HIGH, pal::NEIGHBOUR_HIGH, pal::ARTCC_LOW]
    );
    assert_eq!(
        l.polylines.layer,
        vec![LAYER_ARTCC_HIGH, LAYER_NEIGHBOUR, LAYER_ARTCC_LOW]
    );
    assert_eq!(l.polylines.width, vec![2, 1, 2]);
    // The open own HIGH ring is closed back to its first vertex; the closed LOW ring is not doubled.
    let ring = |i: usize| {
        let (s, n) = (l.polylines.start[i] as usize, l.polylines.len[i] as usize);
        (l.poly_x[s..s + n].to_vec(), l.poly_y[s..s + n].to_vec())
    };
    let (hx, hy) = ring(0);
    assert_eq!((hx[0], hy[0]), (hx[hx.len() - 1], hy[hy.len() - 1]));
    assert!(hx.len() >= 5);
    let (lx, ly) = ring(2);
    assert_eq!((lx[0], ly[0]), (lx[lx.len() - 1], ly[ly.len() - 1]));
    assert_eq!(
        project(&view.proj, m.vertices[0].lat, m.vertices[0].lon),
        Some((hx[0], hy[0]))
    );

    // Airway legs are lines in the airway colour; fixes have triangles; navaids and airports
    // rings with a dot.
    assert!(l.lines.colour.iter().filter(|&&c| c == pal::AIRWAY).count() >= 3);
    assert_eq!(
        l.lines.colour.iter().filter(|&&c| c == pal::FIX).count(),
        3 * 2 + 2,
        "two triangles and two navaid dots"
    );
    assert_eq!(
        l.circles
            .colour
            .iter()
            .filter(|&&c| c == pal::AIRPORT)
            .count(),
        2
    );
    assert_eq!(
        l.circles.colour.iter().filter(|&&c| c == pal::FIX).count(),
        2
    );

    // Rings: one per step out to the corner, each at its true radius, labelled with its range.
    assert_eq!(ring_step_nm(&view.proj), 20.0);
    let rings: Vec<i32> = l
        .circles
        .r
        .iter()
        .zip(&l.circles.colour)
        .filter(|(_, &c)| c == pal::RING)
        .map(|(r, _)| *r)
        .collect();
    assert_eq!(rings.len() as u32, st.rings);
    assert_eq!(rings[0], ring_radius16(&view.proj, 20.0 * NM_M).unwrap());
    let t = texts(&l);
    for label in ["20", "40"] {
        assert!(t.contains(&label), "no ring label {label} in {t:?}");
    }

    // Legend: facility, cycles, range, ring spacing; the boundary key in its own colours.
    assert!(t.contains(&"ZAU NASR 20261029 CIFP 2611  NORTH UP  RANGE 50 NM  RINGS 20 NM"));
    let named = |name: &str, colour: u8| {
        t.iter()
            .zip(&l.texts.colour)
            .any(|(s, &c)| *s == name && c == colour)
    };
    assert!(named("ZAU HIGH", pal::ARTCC_HIGH) && named("ZAU LOW", pal::ARTCC_LOW));
    assert!(named("NEIGHBOUR HIGH", pal::NEIGHBOUR_HIGH));
    assert!(named("NEIGHBOUR LOW", pal::NEIGHBOUR_LOW));
    assert!(t.contains(&"HIDDEN: NONE"));

    // The picture: the south-east own HIGH vertex is a HIGH pixel, and the own boundary is two
    // pixels wide there (a vertical edge: two columns).
    let buf = render_buf(&l, 800, 600);
    let (sx, sy) = project(&view.proj, m.vertices[2].lat, m.vertices[2].lon).unwrap();
    let (px, py) = ((sx >> 4) as usize, (sy >> 4) as usize);
    let high = pal::PALETTE[usize::from(pal::ARTCC_HIGH)];
    assert_eq!(pixel(&buf, 800, px, py - 20), high);
    let beside = [
        pixel(&buf, 800, px - 1, py - 20),
        pixel(&buf, 800, px + 1, py - 20),
    ];
    assert!(
        beside.contains(&high),
        "no second pixel beside the edge: {beside:x?}"
    );
}

// REQ: OA-DSP-025
#[test]
fn detail_depends_on_range_with_configurable_thresholds_and_the_legend_says_what_is_hidden() {
    let m = model(Tail::None);
    let (l, st, _) = build(&m, 80.0, |_| {});
    // Above 40 NM: no fixes, no airport without an instrument procedure, no NDB, and labels only
    // for the VOR and the airport with a coded approach, which remain drawn.
    assert_eq!((st.fixes, st.navaids, st.airports, st.labels), (0, 1, 1, 2));
    let t = texts(&l);
    assert!(t.contains(&"HIDDEN: FIXES>40NM, TERMINAL WPTS>20NM, RWY/LOC>15NM, NON-IFR APTS>40NM, NDB/DME/TACAN>40NM, LABELS>40NM"));
    assert!(t.contains(&"DPA") && t.contains(&"KORD"));
    // Beyond the wider label range those two go as well, and the line says so.
    let (l, st, _) = build(&m, 120.0, |_| {});
    assert_eq!(st.labels, 0);
    assert!(texts(&l)
        .iter()
        .any(|t| t.ends_with("LABELS>40NM, VOR/IFR APT LABELS>100NM")));
    // Labels are on at 50 NM with the thresholds open, one per drawn symbol.
    let (l, st, _) = build(&m, 50.0, all_open);
    assert_eq!(st.labels, 6);
    let t = texts(&l);
    for name in ["1C5", "AAAAA", "BBBBB", "DPA", "KORD", "ZNB"] {
        assert!(t.contains(&name), "no label {name}");
    }
    // Every threshold is configurable, and the line follows them.
    let (l, st, _) = build(&m, 80.0, |v| {
        v.fix_max_nm = 100.0;
        v.non_ifr_airport_max_nm = 100.0;
        v.minor_navaid_max_nm = 100.0;
        v.label_max_nm = 70.0;
    });
    assert_eq!((st.fixes, st.navaids, st.airports, st.labels), (2, 2, 2, 2));
    assert!(texts(&l)
        .iter()
        .any(|t| t.ends_with("RWY/LOC>15NM, LABELS>70NM")));
    let (_, st, _) = build(&m, 80.0, |v| v.fix_max_nm = 79.0);
    assert_eq!(st.fixes, 0);
    let (_, st, _) = build(&m, 80.0, |v| v.fix_max_nm = 80.0);
    assert_eq!(st.fixes, 2, "a threshold equal to the range still shows");
}

// REQ: OA-DSP-025
#[test]
fn every_layer_can_be_switched_off_and_the_legend_says_so() {
    let m = model(Tail::None);
    let (_, base, _) = build(&m, 50.0, all_open);
    for name in LAYER_NAMES {
        let (l, st, _) = build(&m, 50.0, |v| {
            all_open(v);
            assert!(layer_set(&mut v.layers, name, false));
        });
        let want = format!("HIDDEN: {} OFF", name.to_ascii_uppercase());
        assert!(
            texts(&l).contains(&want.as_str()),
            "{want} not in {:?}",
            texts(&l)
        );
        match name {
            "own-boundary" => assert_eq!(st.own_boundaries, 0),
            "neighbour-boundaries" => assert_eq!(st.neighbour_boundaries, 0),
            "high-airways" => assert_eq!(st.airway_legs, base.airway_legs - 1),
            "low-airways" => assert_eq!(st.airway_legs, base.airway_legs - 1),
            "vors" => assert_eq!(st.navaids, 0),
            "airports" => assert_eq!(st.airports, 0),
            "fixes" => assert_eq!(st.fixes, 0),
            "labels" => assert_eq!(st.labels, 0),
            "rings" => assert_eq!(st.rings, 0),
            other => panic!("{other}"),
        }
    }
    let mut ls = layers_all();
    assert!(!layer_set(&mut ls, "bogus", false));
    assert_eq!(layers_flags(&ls), [true; 9]);
    // Several at once are all named.
    let (l, _, _) = build(&m, 50.0, |v| {
        all_open(v);
        layer_set(&mut v.layers, "fixes", false);
        layer_set(&mut v.layers, "rings", false);
    });
    assert!(texts(&l).contains(&"HIDDEN: FIXES OFF, RINGS OFF"));
}

// REQ: OA-DSP-026
#[test]
fn the_own_boundary_is_two_pixels_in_full_colour_and_neighbours_one_pixel_and_dimmer() {
    let m = model(Tail::None);
    let (l, _, _) = build(&m, 50.0, |_| {});
    // ZAU's HIGH and LOW: width 2, strata colours; ZMP's HIGH: width 1, the dim colour, under it.
    let by_colour = |c: u8| l.polylines.colour.iter().position(|&x| x == c).unwrap();
    assert_eq!(l.polylines.width[by_colour(pal::ARTCC_HIGH)], 2);
    assert_eq!(l.polylines.width[by_colour(pal::ARTCC_LOW)], 2);
    assert_eq!(l.polylines.width[by_colour(pal::NEIGHBOUR_HIGH)], 1);
    assert!(
        l.polylines.layer[by_colour(pal::NEIGHBOUR_HIGH)]
            < l.polylines.layer[by_colour(pal::ARTCC_LOW)]
    );
    // Only the neighbour: the own boundary off leaves no width-2 polyline.
    let (l, _, _) = build(&m, 50.0, |v| {
        layer_set(&mut v.layers, "own-boundary", false);
    });
    assert_eq!(l.polylines.colour, vec![pal::NEIGHBOUR_HIGH]);
    // The facility is the model's: a model built for another ARTCC draws the other one thick.
    let mut other = model(Tail::None);
    other.meta.artcc = "ZMP".into();
    let (l, st, _) = build(&other, 50.0, |_| {});
    assert_eq!((st.own_boundaries, st.neighbour_boundaries), (1, 2));
    assert!(l.polylines.colour.contains(&pal::NEIGHBOUR_LOW));
}

// REQ: OA-DSP-023
#[test]
fn border_crossings_are_reported_in_the_normal_colour_and_never_raise_the_alert() {
    let m = model(Tail::Border);
    validate_model(&m).unwrap();
    let (l, st, _) = build(&m, 50.0, |_| {});
    // The third leg ends at a published border crossing: counted, not drawn, not an alert.
    assert_eq!(
        (
            st.legs_at_border,
            st.legs_unplaced,
            st.points_unplaced,
            st.edges_unplaced
        ),
        (1, 0, 0, 0)
    );
    assert_eq!(st.airway_legs, 2, "the border leg is counted, not drawn");
    let t = texts(&l);
    let i = t
        .iter()
        .position(|s| *s == "1 LEGS END AT BORDER CROSSINGS")
        .unwrap();
    assert_eq!(l.texts.colour[i], pal::TEXT);
    assert!(!t.iter().any(|s| s.starts_with("NOT PLACED")));
    // Without a border row the same leg is a real unplaced leg.
    let m = model(Tail::Unresolved);
    validate_model(&m).unwrap();
    let (l, st, _) = build(&m, 50.0, |_| {});
    assert_eq!((st.legs_at_border, st.legs_unplaced), (0, 1));
    let t = texts(&l);
    let i = t
        .iter()
        .position(|s| *s == "NOT PLACED 0 PTS 1 LEGS 0 EDGES")
        .unwrap();
    assert_eq!(l.texts.colour[i], pal::ALERT);
}

// REQ: OA-DSP-027
#[test]
fn the_alert_colour_is_used_only_when_something_needs_attention() {
    let alerts = |l: &DisplayList| {
        l.lines.colour.iter().filter(|&&c| c == pal::ALERT).count()
            + l.rects.colour.iter().filter(|&&c| c == pal::ALERT).count()
            + l.circles
                .colour
                .iter()
                .filter(|&&c| c == pal::ALERT)
                .count()
            + l.polylines
                .colour
                .iter()
                .filter(|&&c| c == pal::ALERT)
                .count()
            + l.texts.colour.iter().filter(|&&c| c == pal::ALERT).count()
    };
    for (tail, needs) in [
        (Tail::None, false),
        (Tail::Border, false),
        (Tail::Unresolved, true),
    ] {
        let m = model(tail);
        for range in [3.0, 50.0, 80.0, 400.0, 3000.0] {
            let (l, st, _) = build(&m, range, |_| {});
            let unplaced = st.points_unplaced + st.legs_unplaced + st.edges_unplaced;
            assert_eq!(unplaced > 0, needs, "range {range}");
            assert_eq!(alerts(&l) > 0, unplaced > 0, "range {range}");
            let buf = render_buf(&l, 800, 600);
            assert_eq!(count_colour(&buf, pal::ALERT) > 0, needs, "range {range}");
        }
    }
}

/// Text boxes of a list in pixels: (x0, y0, x1, y1, colour, layer, text), from the font metrics.
fn boxes(l: &DisplayList) -> Vec<(i64, i64, i64, i64, u8, u8, String)> {
    let t = texts(l);
    (0..t.len())
        .map(|i| {
            let size = i64::from(l.texts.size[i]);
            let units = (t[i].chars().count() as i64) * 6 - 2;
            let (x, y) = (i64::from(l.texts.x[i]) / 16, i64::from(l.texts.y[i]) / 16);
            (
                x,
                y,
                x + units * size / 6,
                y + size,
                l.texts.colour[i],
                l.texts.layer[i],
                t[i].to_string(),
            )
        })
        .collect()
}

fn overlap(
    a: &(i64, i64, i64, i64, u8, u8, String),
    b: &(i64, i64, i64, i64, u8, u8, String),
) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

// REQ: OA-DSP-028
#[test]
fn the_legend_strip_hides_no_map_data_and_its_rows_never_overlap() {
    let m = model(Tail::Border);
    for range in [20.0, 50.0, 250.0] {
        let view = view_new(ORD.0, ORD.1, range, 1920, 1080).unwrap();
        let mut l = list_new(scene_caps(&m));
        build_scene(
            &mut l,
            &m,
            &scene_index(&m),
            &view,
            &mut scratch_for(&m, &view),
        )
        .unwrap();
        // The strip: everything of the map within the range is below it.
        let strip = LEGEND_STRIP_PX as i64;
        let (_, y) = project(&view.proj, ORD.0 + (range / 60.5 * 1e7) as i32, ORD.1).unwrap();
        assert!(
            i64::from(y) / 16 >= strip - 2,
            "range {range} reaches row {}",
            y / 16
        );
        // Pixels below the strip are the same with and without the legend.
        let with = render_buf(&l, 1920, 1080);
        let mut bare = l.clone();
        bare.layers_used &= !((1 << LAYER_LEGEND_BG) | (1 << LAYER_LEGEND));
        let without = render_buf(&bare, 1920, 1080);
        let below = strip as usize * 1920 * 4;
        assert_eq!(
            with[below..],
            without[below..],
            "range {range}: the legend covers map pixels"
        );
        // Above it, only legend colours: background, text colours, the rule.
        let legend_colours = [
            pal::BACKGROUND,
            pal::TEXT,
            pal::ARTCC_HIGH,
            pal::ARTCC_LOW,
            pal::NEIGHBOUR_HIGH,
            pal::NEIGHBOUR_LOW,
            pal::ALERT,
            pal::RING,
        ];
        for px in with[..below].as_chunks::<4>().0 {
            let v = u32::from_le_bytes(*px);
            assert!(legend_colours
                .iter()
                .any(|&c| pal::PALETTE[usize::from(c)] == v));
        }
        // The legend rows sit inside the strip and no two overlap.
        let rows: Vec<_> = boxes(&l)
            .into_iter()
            .filter(|b| b.5 == LAYER_LEGEND)
            .collect();
        assert!(rows.len() >= 6);
        for (i, a) in rows.iter().enumerate() {
            assert!(
                a.3 < strip - 2 && a.0 >= 0 && a.2 < 1920,
                "{a:?} leaves the strip"
            );
            for b in &rows[i + 1..] {
                assert!(!overlap(a, b), "{a:?} overlaps {b:?}");
            }
        }
    }
}

// REQ: OA-DSP-029
#[test]
fn ring_labels_sit_on_the_diagonal_and_never_overlap_at_any_spacing() {
    let m = model(Tail::None);
    let mut checked = 0;
    for (w, h) in [(800, 600), (1920, 1080), (640, 480), (1200, 900)] {
        for range in [
            0.5, 1.0, 3.0, 7.0, 10.0, 20.0, 35.0, 60.0, 120.0, 250.0, 400.0, 1000.0, 5000.0,
            20000.0,
        ] {
            let mut view = view_new(ORD.0, ORD.1, range, w, h).unwrap();
            view.label_max_nm = 0.0;
            let mut l = list_new(scene_caps(&m));
            let st = build_scene(
                &mut l,
                &m,
                &scene_index(&m),
                &view,
                &mut scratch_for(&m, &view),
            )
            .unwrap();
            let labels: Vec<_> = boxes(&l)
                .into_iter()
                .filter(|b| b.4 == pal::DIM_TEXT && b.5 == LAYER_TEXT)
                .collect();
            assert!(labels.len() as u32 <= st.rings);
            for (i, a) in labels.iter().enumerate() {
                // On the ray 45 degrees east of north from the centre, just beside the ring.
                let (cx, cy) = (view.proj.cx16 / 16.0, view.proj.cy16 / 16.0);
                let (dx, dy) = (a.0 as f64 - 5.0 - cx, cy - (a.1 as f64 + 12.0));
                assert!(
                    (dx - dy).abs() < 1.5,
                    "{a:?} is off the diagonal ({dx}, {dy})"
                );
                for b in &labels[i + 1..] {
                    assert!(
                        !overlap(a, b),
                        "{w}x{h} range {range}: {a:?} overlaps {b:?}"
                    );
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 100, "only {checked} labels checked");
}

/// A valid model of bare points `(kind, ident, lat_e7, lon_e7)` only.
fn points_model(pts: &[(u8, String, i32, i32)]) -> Model {
    let mut m = empty_model(Meta {
        nasr_effective: 20_261_029,
        cifp_cycle: 2611,
        artcc: "ZAU".into(),
        buffer_m: 0,
        nasr_sha256: [1; 32],
        cifp_sha256: [2; 32],
    });
    let mut s = Strings(StrTable::default());
    let region = s.add("K5");
    let empty = s.add("");
    for (kind, ident, lat, lon) in pts {
        let ix = s.add(ident);
        let p = &mut m.points;
        p.kind.push(*kind);
        p.lat.push(*lat);
        p.lon.push(*lon);
        p.elev_ft.push(ELEV_NONE);
        p.ident.push(ix);
        p.alt_ident.push(StrIx::NONE);
        p.region.push(region);
        p.scope.push(empty);
        p.name.push(StrIx::NONE);
    }
    let n = pts.len();
    m.points.src_nasr = bit_zeros(n);
    m.points.src_cifp = bit_zeros(n);
    m.points.in_area = bit_zeros(n);
    m.points.cls_b = bit_zeros(n);
    m.points.cls_c = bit_zeros(n);
    m.points.cls_d = bit_zeros(n);
    m.points.cls_e = bit_zeros(n);
    m.procs.src_nasr = bit_zeros(0);
    m.procs.src_cifp = bit_zeros(0);
    m.procs.coded = bit_zeros(0);
    m.boundaries.floor_agl = bit_zeros(0);
    m.boundaries.ceiling_agl = bit_zeros(0);
    m.airway_legs.gap = bit_zeros(0);
    m.airway_legs.signal_gap = bit_zeros(0);
    m.airway_legs.dogleg = bit_zeros(0);
    m.strings = s.0;
    m
}

fn lcg(seed: &mut u64) -> u32 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (*seed >> 33) as u32
}

/// Detail by zoom: each kind appears at its own range and not before, whatever its distance from
/// the centre.
// REQ: OA-DSP-025
#[test]
fn terminal_waypoints_runways_and_localizers_follow_the_display_range_not_the_distance() {
    let at = |d: f64| e7(41.9786 + d, -87.9048);
    // The same three symbols near the centre and 0.1 degree (6 NM) north of it.
    let mut pts = Vec::new();
    for (i, d) in [0.0, 0.1].into_iter().enumerate() {
        pts.push((
            point_kind::TERMINAL_WAYPOINT,
            format!("TW{i}"),
            at(d).0,
            at(d).1,
        ));
        pts.push((
            point_kind::THRESHOLD,
            format!("RW{i}"),
            at(d + 0.001).0,
            at(d + 0.001).1,
        ));
        pts.push((
            point_kind::LOCALIZER,
            format!("IL{i}"),
            at(d + 0.002).0,
            at(d + 0.002).1,
        ));
        pts.push((
            point_kind::FIX,
            format!("FX{i}"),
            at(d + 0.003).0,
            at(d + 0.003).1,
        ));
    }
    pts.sort_by(|a, b| a.1.cmp(&b.1));
    let m = points_model(&pts);
    validate_model(&m).unwrap();
    // (range, terminal waypoints, runway symbols, fixes) with the default thresholds.
    for (range, tw, rw, fx) in [
        (10.0, 2, 4, 2),
        (15.0, 2, 4, 2),
        (18.0, 2, 0, 2),
        (20.0, 2, 0, 2),
        (30.0, 0, 0, 2),
        (40.0, 0, 0, 2),
        (41.0, 0, 0, 0),
    ] {
        let (l, st, _) = build(&m, range, |_| {});
        // Both of each kind, or none: the position on screen does not matter.
        let _ = l;
        assert_eq!(
            (st.terminal_waypoints, st.runway_symbols, st.fixes),
            (tw, rw, fx),
            "range {range}"
        );
    }
    // The line says what a range hides, and the thresholds are configurable.
    let (l, _, _) = build(&m, 30.0, |_| {});
    assert!(texts(&l)
        .iter()
        .any(|t| t.starts_with("HIDDEN: TERMINAL WPTS>20NM, RWY/LOC>15NM")));
    let (_, st, _) = build(&m, 30.0, |v| {
        v.terminal_wpt_max_nm = 30.0;
        v.runway_max_nm = 30.0;
    });
    assert_eq!((st.terminal_waypoints, st.runway_symbols), (2, 4));
}

/// Label boxes of the list in pixels, text layer only (point labels and ring labels).
fn label_boxes(l: &DisplayList) -> Vec<(i64, i64, i64, i64, u8, u8, String)> {
    let size = i64::from(LABEL_SIZE_PX);
    let t = texts(l);
    (0..t.len())
        .filter(|&i| l.texts.layer[i] == LAYER_TEXT)
        .map(|i| {
            let w = ((t[i].chars().count() as i64) * 6 - 2) * size / 8;
            let (x, y) = (i64::from(l.texts.x[i]) / 16, i64::from(l.texts.y[i]) / 16);
            let x0 = match l.texts.align[i] {
                ALIGN_LEFT => x,
                _ => x - w,
            };
            (
                x0,
                y,
                x0 + w,
                y + size,
                l.texts.colour[i],
                l.texts.layer[i],
                t[i].to_string(),
            )
        })
        .collect()
}

const LABEL_SIZE_PX: u8 = 12;

// REQ: OA-DSP-032
#[test]
fn labels_in_a_dense_scene_never_overlap_keep_clear_of_the_legend_and_are_counted_when_dropped() {
    let mut seed = 7;
    let mut pts = Vec::new();
    for i in 0..600 {
        let kind = [
            point_kind::FIX,
            point_kind::TERMINAL_WAYPOINT,
            point_kind::AIRPORT,
            point_kind::VOR,
            point_kind::NDB,
        ][lcg(&mut seed) as usize % 5];
        let lat = 41.9786 + (f64::from(lcg(&mut seed) % 1000) - 500.0) / 2500.0;
        let lon = -87.9048 + (f64::from(lcg(&mut seed) % 1000) - 500.0) / 1800.0;
        let (la, lo) = e7(lat, lon);
        pts.push((kind, format!("P{i:03}"), la, lo));
    }
    let m = points_model(&pts);
    validate_model(&m).unwrap();
    for (w, h, range) in [
        (1920u32, 1080u32, 20.0),
        (1920, 1080, 12.0),
        (800, 600, 15.0),
        (1200, 900, 30.0),
    ] {
        let mut view = view_new(ORD.0, ORD.1, range, w, h).unwrap();
        all_open(&mut view);
        let mut l = list_new(scene_caps(&m));
        let st = build_scene(
            &mut l,
            &m,
            &scene_index(&m),
            &view,
            &mut scratch_for(&m, &view),
        )
        .unwrap();
        let shown = st.fixes + st.terminal_waypoints + st.navaids + st.airports;
        assert!(shown > 100 && st.labels_dropped > 0, "{st:?}");
        assert_eq!(
            st.labels + st.labels_dropped,
            shown,
            "every shown symbol had a candidate"
        );
        // Every label, the ring labels included, is clear of every other, of the strip and of
        // the screen edge.
        let bx = label_boxes(&l);
        assert!(bx.len() as u32 >= st.labels);
        for (i, a) in bx.iter().enumerate() {
            assert!(
                a.0 >= 0 && a.2 <= i64::from(w) && a.3 <= i64::from(h),
                "{a:?} leaves the screen"
            );
            if a.4 != pal::DIM_TEXT {
                assert!(a.1 >= i64::from(LEGEND_STRIP_PX), "{a:?} is in the strip");
            }
            for b in &bx[i + 1..] {
                assert!(!overlap(a, b), "{w}x{h} {range}: {a:?} overlaps {b:?}");
            }
        }
        // The dropped count is on the legend, in the normal colour.
        let t = texts(&l);
        let want = format!("LABELS DROPPED: {}", st.labels_dropped);
        let i = t.iter().position(|s| *s == want).unwrap();
        assert_eq!(l.texts.colour[i], pal::TEXT);
    }
}

// REQ: OA-DSP-032
#[test]
fn a_navaid_label_is_drawn_ahead_of_every_lower_priority_label_it_overlaps() {
    // A VOR with 40 fixes and 10 airports piled on it, away from the centre.
    let (la, lo) = e7(41.99, -87.80);
    let mut pts = vec![(point_kind::VOR, "ZZVOR".to_string(), la, lo)];
    for i in 0..40 {
        pts.push((
            point_kind::FIX,
            format!("AAF{i:02}"),
            la + i * 20,
            lo + (i % 5) * 20,
        ));
    }
    for i in 0..10 {
        pts.push((
            point_kind::AIRPORT,
            format!("AAA{i}"),
            la - i * 30,
            lo - 100,
        ));
    }
    let m = points_model(&pts);
    let (l, st, _) = build(&m, 10.0, all_open);
    let t = texts(&l);
    // Named last by ident and with the lowest number priority: placed first, so it is drawn.
    assert!(t.contains(&"ZZVOR"), "the VOR label was dropped: {t:?}");
    assert!(st.labels_dropped > 20, "{st:?}");
    // Every drawn label is clear of the VOR's, which is first in the list.
    let bx = label_boxes(&l);
    let v = bx.iter().find(|b| b.6 == "ZZVOR").unwrap();
    for b in bx.iter().filter(|b| b.6 != "ZZVOR") {
        assert!(!overlap(v, b), "{b:?} overlaps the VOR label");
    }
    // The labels are in priority order in the list: the VOR before any airport before any fix.
    let pos = |name: &str| t.iter().position(|s| *s == name).unwrap();
    let first_fix = t.iter().position(|s| s.starts_with("AAF"));
    let first_apt = t.iter().position(|s| s.starts_with("AAA"));
    assert!(pos("ZZVOR") < first_apt.unwrap_or(usize::MAX));
    if let (Some(a), Some(f)) = (first_apt, first_fix) {
        assert!(a < f);
    }
}

/// Padded overlap, as the placer sees it.
fn padded_overlap(a: &[i32; 4], b: &[i32; 4]) -> bool {
    let p = LABEL_PAD16;
    a[0] - p < b[2] + p && b[0] - p < a[2] + p && a[1] - p < b[3] + p && b[1] - p < a[3] + p
}

/// Random candidates in a 1920 by 1080 map area, a scratch holding them, placed.
fn random_scratch(seed: &mut u64, n: usize) -> Scratch {
    let pts: Vec<_> = (0..n)
        .map(|i| (point_kind::FIX, format!("P{i}"), 0, 0))
        .collect();
    let m = points_model(&pts);
    let view = view_new(ORD.0, ORD.1, 50.0, 1920, 1080).unwrap();
    let mut s = scratch_for(&m, &view);
    s.bounds = [0, LEGEND_STRIP_PX as i32 * 16, 1920 * 16, 1080 * 16];
    for i in 0..n {
        let chars = 2 + lcg(seed) as usize % 7;
        s.prio.push(1 + (lcg(seed) % 5) as u8);
        s.x.push((lcg(seed) % (1920 * 16)) as i32);
        s.y.push((lcg(seed) % (1080 * 16)) as i32);
        s.gap.push(8 * 16);
        s.w.push(((chars * 6 - 2) * 24) as i32);
        s.point.push(i as u32);
    }
    // Priority, then the candidate number as the stable key.
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_by_key(|&i| (s.prio[i as usize], i));
    s.order = order;
    s
}

// REQ: OA-DSP-032
#[test]
fn placement_property_no_overlap_and_a_drop_means_every_offset_is_blocked_by_an_equal_or_higher_priority_label(
) {
    let mut seed = 99;
    let mut dropped_total = 0;
    for round in 0..60 {
        let n = 20 + round * 9;
        let mut s = random_scratch(&mut seed, n);
        let placed_n = place_labels(&mut s);
        let boxes_of =
            |s: &Scratch, i: usize, k: u8| label_geom(s.gap[i], s.w[i], s.x[i], s.y[i], k).2;
        let placed: Vec<usize> = (0..n).filter(|&i| s.outcome[i] < 4).collect();
        assert_eq!(placed.len() as u32, placed_n);
        // No two placed labels overlap, padding included.
        for (a, &i) in placed.iter().enumerate() {
            let bi = boxes_of(&s, i, s.outcome[i]);
            let bd = s.bounds;
            assert!(bi[0] >= bd[0] && bi[1] >= bd[1] && bi[2] <= bd[2] && bi[3] <= bd[3]);
            for &j in &placed[a + 1..] {
                assert!(
                    !padded_overlap(&bi, &boxes_of(&s, j, s.outcome[j])),
                    "round {round}: {i} and {j}"
                );
            }
        }
        // Every drop (with room in the grid) is justified at all four offsets: out of bounds or
        // overlapping a label placed earlier, which has equal or higher priority.
        let pos: Vec<usize> = {
            let mut p = vec![0; n];
            for (o, &i) in s.order.iter().enumerate() {
                p[i as usize] = o;
            }
            p
        };
        for i in (0..n).filter(|&i| s.outcome[i] >= 4) {
            assert_eq!(
                s.outcome[i], DROPPED_NO_ROOM,
                "cells never fill at this density"
            );
            dropped_total += 1;
            for k in 0..4 {
                let b = boxes_of(&s, i, k);
                let bd = s.bounds;
                let p = LABEL_PAD16;
                let outside =
                    b[0] - p < bd[0] || b[1] - p < bd[1] || b[2] + p > bd[2] || b[3] + p > bd[3];
                // Some label placed earlier, so of equal or higher priority, takes the offset.
                let blocker = placed.iter().find(|&&j| {
                    pos[j] < pos[i] && padded_overlap(&b, &boxes_of(&s, j, s.outcome[j]))
                });
                if let Some(&j) = blocker {
                    assert!(s.prio[j] <= s.prio[i]);
                } else {
                    assert!(outside, "round {round}: {i} dropped with offset {k} free");
                }
            }
        }
    }
    assert!(
        dropped_total > 100,
        "the property was never exercised: {dropped_total}"
    );
}

// REQ: OA-DSP-032
#[test]
fn a_full_grid_cell_drops_the_label_and_counts_it_without_overflowing() {
    // Thin labels in one 64 pixel cell: more fit by space than the cell holds.
    let n = 90;
    let pts: Vec<_> = (0..n)
        .map(|i| (point_kind::FIX, format!("P{i}"), 0, 0))
        .collect();
    let m = points_model(&pts);
    let view = view_new(ORD.0, ORD.1, 50.0, 1920, 1080).unwrap();
    let mut s = scratch_for(&m, &view);
    s.bounds = [0, 104 * 16, 1920 * 16, 1080 * 16];
    for i in 0..n {
        s.prio.push(4);
        s.x.push((i as i32 % 12) * 5 * 16 + 64 * 16 * 3);
        s.y.push((i as i32 / 12) * 16 * 16 + 64 * 16 * 3);
        s.gap.push(0);
        s.w.push(16);
        s.point.push(i as u32);
    }
    s.order = (0..n as u32).collect();
    let placed = place_labels(&mut s);
    let full = s
        .outcome
        .iter()
        .filter(|&&o| o == DROPPED_CELL_FULL)
        .count();
    assert!(full > 0, "no cell filled");
    assert_eq!(
        placed as usize + full + s.outcome.iter().filter(|&&o| o == DROPPED_NO_ROOM).count(),
        n
    );
    for c in &s.cell_n {
        assert!(usize::from(*c) <= CELL_CAP);
    }
    // The cells are all the allocation there was: nothing grew.
    assert_eq!(s.cell_box.len(), s.cols * s.rows * CELL_CAP);
}

// REQ: OA-DSP-032
#[test]
fn label_placement_is_deterministic_and_does_not_depend_on_the_order_of_the_points() {
    let mut seed = 5;
    let mut pts = Vec::new();
    for i in 0..300 {
        let kind =
            [point_kind::FIX, point_kind::AIRPORT, point_kind::VOR][lcg(&mut seed) as usize % 3];
        let (la, lo) = e7(
            41.9786 + (f64::from(lcg(&mut seed) % 600) - 300.0) / 3000.0,
            -87.9048 + (f64::from(lcg(&mut seed) % 600) - 300.0) / 2200.0,
        );
        pts.push((kind, format!("N{i:03}"), la, lo));
    }
    let labels_of = |pts: &[(u8, String, i32, i32)]| {
        let m = points_model(pts);
        let mut view = view_new(ORD.0, ORD.1, 20.0, 1920, 1080).unwrap();
        all_open(&mut view);
        let index = scene_index(&m);
        let mut sc = scratch_for(&m, &view);
        let mut l = list_new(scene_caps(&m));
        let st = build_scene(&mut l, &m, &index, &view, &mut sc).unwrap();
        // The same scratch, a second frame: the same list.
        let mut l2 = list_new(scene_caps(&m));
        build_scene(&mut l2, &m, &index, &view, &mut sc).unwrap();
        assert_eq!(l, l2);
        let mut v: Vec<_> = (0..l.texts.x.len())
            .filter(|&i| l.texts.layer[i] == LAYER_TEXT)
            .map(|i| {
                (
                    l.texts.x[i],
                    l.texts.y[i],
                    l.texts.align[i],
                    l.texts.colour[i],
                    texts(&l)[i].to_string(),
                )
            })
            .collect();
        v.sort();
        (v, st.labels, st.labels_dropped)
    };
    let first = labels_of(&pts);
    assert!(first.1 > 50 && first.2 > 0);
    let mut shuffled = pts.clone();
    for i in (1..shuffled.len()).rev() {
        shuffled.swap(i, lcg(&mut seed) as usize % (i + 1));
    }
    assert_eq!(labels_of(&shuffled), first);
    assert_eq!(labels_of(&pts), first);
}

// REQ: OA-DSP-030
#[test]
fn rejecting_positions_by_bounding_box_changes_nothing_on_screen() {
    let m = model(Tail::Border);
    for (lat, lon) in [
        ORD,
        e7(42.3, -88.3),
        e7(41.6, -87.4),
        e7(30.0, -100.0),
        e7(0.0, 179.9),
    ] {
        for range in [0.5, 5.0, 50.0, 80.0, 300.0, 3000.0] {
            let mut view = view_new(lat, lon, range, 800, 600).unwrap();
            view.fix_max_nm = 1e9;
            view.non_ifr_airport_max_nm = 1e9;
            view.minor_navaid_max_nm = 1e9;
            let index = scene_index(&m);
            let mut a = list_new(scene_caps(&m));
            let mut b = list_new(scene_caps(&m));
            let sa = build_scene(&mut a, &m, &index, &view, &mut scratch_for(&m, &view)).unwrap();
            view.bbox = None;
            let sb = build_scene(&mut b, &m, &index, &view, &mut scratch_for(&m, &view)).unwrap();
            assert_eq!(a, b, "{lat},{lon} range {range}: the box changed the list");
            assert_eq!(
                (sa.fixes, sa.navaids, sa.airports, sa.airway_legs, sa.labels),
                (sb.fixes, sb.navaids, sb.airports, sb.airway_legs, sb.labels)
            );
        }
    }
}

// REQ: OA-DSP-018
#[test]
fn an_overflowing_scene_is_an_error_and_the_list_is_never_drawn() {
    let m = model(Tail::None);
    let (_, _, view) = build(&m, 50.0, |_| {});
    let index = scene_index(&m);
    let full = scene_caps(&m);
    // Each table, one row short of what the scene needs, fails by name.
    let mut l = list_new(full);
    build_scene(&mut l, &m, &index, &view, &mut scratch_for(&m, &view)).unwrap();
    let used = (
        l.lines.x0.len(),
        l.circles.x.len(),
        l.polylines.start.len(),
        l.poly_x.len(),
        l.texts.x.len(),
        l.text.len(),
    );
    for (what, caps) in [
        (
            "lines",
            ListCaps {
                lines: used.0 - 1,
                ..full
            },
        ),
        (
            "circles",
            ListCaps {
                circles: used.1 - 1,
                ..full
            },
        ),
        (
            "polylines",
            ListCaps {
                polylines: used.2 - 1,
                ..full
            },
        ),
        (
            "polyline vertices",
            ListCaps {
                poly_vertices: used.3 - 1,
                ..full
            },
        ),
        (
            "texts",
            ListCaps {
                texts: used.4 - 1,
                ..full
            },
        ),
        (
            "text bytes",
            ListCaps {
                text_bytes: used.5 - 1,
                ..full
            },
        ),
        ("rects", ListCaps { rects: 0, ..full }),
    ] {
        let mut l = list_new(caps);
        match build_scene(&mut l, &m, &index, &view, &mut scratch_for(&m, &view)) {
            Err(DisplayError::Full { table, .. }) => assert_eq!(table, what),
            other => panic!("{what}: {other:?}"),
        }
        assert!(l.faulted);
        let sp = spec(800, 600);
        let mut buf = vec![7u8; frame_len(&sp)];
        assert_eq!(render(&l, &sp, &mut buf), Err(RenderError::FaultedList));
        assert!(buf.iter().all(|&b| b == 7));
        // The next frame, with room, recovers: clearing resets the fault.
        list_clear(&mut l);
        assert!(!l.faulted);
    }
    // The capacities scene_caps gives hold at every range, extreme centres, and a huge zoom, with
    // every threshold wide open so the most is drawn.
    for tail in [Tail::None, Tail::Border, Tail::Unresolved] {
        let m = model(tail);
        let index = scene_index(&m);
        let full = scene_caps(&m);
        for (lat, lon) in [
            ORD,
            (899_000_000, 0),
            (-899_000_000, 1_800_000_000),
            (0, -1_800_000_000),
            (0, 0),
        ] {
            for range in [0.05, 1.0, 60.0, 500.0, 5_000.0, 20_000.0] {
                let mut view = view_new(lat, lon, range, 640, 480).unwrap();
                view.fix_max_nm = 1e9;
                view.non_ifr_airport_max_nm = 1e9;
                view.minor_navaid_max_nm = 1e9;
                view.label_max_nm = 1e9;
                view.terminal_wpt_max_nm = 1e9;
                view.runway_max_nm = 1e9;
                view.major_label_max_nm = 1e9;
                let mut l = list_new(full);
                build_scene(&mut l, &m, &index, &view, &mut scratch_for(&m, &view))
                    .unwrap_or_else(|e| panic!("{lat},{lon} {range}: {e}"));
                let mut buf = vec![0u8; 640 * 480 * 4];
                rasterize(&l, &spec(640, 480), &mut buf).unwrap();
            }
        }
    }
}

// REQ: OA-DSP-019
#[test]
fn frames_are_identical_and_never_change_the_lists_capacity() {
    let m = model(Tail::Border);
    let index = scene_index(&m);
    let mut l = list_new(scene_caps(&m));
    let caps = list_capacities(&l);
    let sp = spec(800, 600);
    let mut buf = vec![0u8; frame_len(&sp)];
    let mut digests = Vec::new();
    for range in [60.0, 250.0, 5.0, 60.0, 1000.0, 60.0] {
        let view = view_new(ORD.0, ORD.1, range, 800, 600).unwrap();
        build_scene(&mut l, &m, &index, &view, &mut scratch_for(&m, &view)).unwrap();
        let d = render(&l, &sp, &mut buf).unwrap();
        assert_eq!(
            list_capacities(&l),
            caps,
            "range {range} changed a capacity"
        );
        digests.push(d);
    }
    // The same view gives the same list and the same bytes, however many frames came between.
    assert_eq!(digests[0], digests[3]);
    assert_eq!(digests[0], digests[5]);
    assert_ne!(digests[0], digests[1]);
    let view = view_new(ORD.0, ORD.1, 60.0, 800, 600).unwrap();
    let mut fresh = list_new(scene_caps(&m));
    build_scene(&mut fresh, &m, &index, &view, &mut scratch_for(&m, &view)).unwrap();
    build_scene(&mut l, &m, &index, &view, &mut scratch_for(&m, &view)).unwrap();
    assert_eq!(fresh, l);
}

// REQ: OA-DSP-012
#[test]
fn the_hand_built_map_renders_to_a_pinned_hash() {
    // The projection uses the platform's trigonometry, which may differ in the last bit between
    // libm versions; this scene is built so no vertex lies near a 1/16 pixel rounding boundary on
    // the machines checked. A mismatch here with the raster goldens (in `raster.rs`) intact means
    // the projection moved, not the rasterizer.
    let m = model(Tail::Border);
    let (l, _, _) = build(&m, 50.0, |_| {});
    let sp = spec(800, 600);
    let mut buf = vec![0u8; frame_len(&sp)];
    let d = render(&l, &sp, &mut buf).unwrap();
    if let Some(dir) = std::env::var_os("OA_DISPLAY_DUMP") {
        let mut out = Vec::new();
        oa_display::ppm::write_ppm(&mut out, &sp, &buf).unwrap();
        std::fs::write(std::path::Path::new(&dir).join("hand-built-map.ppm"), out).unwrap();
    }
    assert_eq!(
        to_hex(&d),
        "c9cbf36a26347be8bcac7541a6a9805516556b6046ebf3555307b1b8b4137003"
    );
}

fn tmp(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

// REQ: OA-DSP-022
#[test]
fn the_command_line_renders_a_snapshot_to_a_ppm_whose_hash_it_reports() {
    let snap = tmp("hand-built.snapshot");
    let ppm = tmp("hand-built.ppm");
    let bytes = encode(&model(Tail::Border)).unwrap();
    std::fs::write(&snap, &bytes).unwrap();
    assert!(decode(&bytes).is_ok());
    let mut out = Vec::new();
    cli::run(
        &args(&[
            "render",
            "--snapshot",
            snap.to_str().unwrap(),
            "--centre",
            "41.9786,-87.9048",
            "--range-nm",
            "60",
            "--size",
            "800x600",
            "--out",
            ppm.to_str().unwrap(),
            "--hash",
        ]),
        &mut out,
    )
    .unwrap();
    let said = String::from_utf8(out).unwrap();
    let file = std::fs::read(&ppm).unwrap();
    let header = b"P6\n800 600\n255\n";
    assert_eq!(&file[..header.len()], header);
    assert_eq!(file.len(), header.len() + 800 * 600 * 3);
    // The printed hash is the SHA-256 of the XRGB frame: rebuild it from the file's RGB.
    let mut xrgb = Vec::new();
    for px in file[header.len()..].as_chunks::<3>().0 {
        xrgb.extend_from_slice(&[px[2], px[1], px[0], 0]);
    }
    let want = to_hex(&sha256(&xrgb));
    assert!(said.contains(&format!("sha256   {want}")), "{said}");
    // And the same picture the library gives for the same model and view.
    let view = view_new(ORD.0, ORD.1, 60.0, 800, 600).unwrap();
    let m = decode(&bytes).unwrap();
    let mut l = list_new(scene_caps(&m));
    build_scene(
        &mut l,
        &m,
        &scene_index(&m),
        &view,
        &mut scratch_for(&m, &view),
    )
    .unwrap();
    let sp = spec(800, 600);
    let mut buf = vec![0u8; frame_len(&sp)];
    rasterize(&l, &sp, &mut buf).unwrap();
    assert_eq!(to_hex(&frame_digest(&sp, &buf).unwrap()), want);
    assert!(said.contains("boundaries") && said.contains("time     build"));
}

// REQ: OA-DSP-022
#[test]
fn bad_command_lines_are_usage_errors_that_name_the_problem() {
    let ok = [
        "render",
        "--snapshot",
        "a",
        "--centre",
        "41.9,-87.9",
        "--range-nm",
        "50",
        "--size",
        "640x480",
        "--out",
        "o",
    ];
    assert!(cli::parse_render_args(&args(&ok[1..])).is_ok());
    let with = |flag: &str, value: &str| {
        let mut v = args(&ok[1..]);
        let i = v.iter().position(|a| a == flag).unwrap();
        v[i + 1] = value.to_string();
        cli::parse_render_args(&v).unwrap_err().to_string()
    };
    assert!(with("--centre", "41.9").contains("--centre"));
    assert!(with("--centre", "91,0").contains("outside"));
    assert!(with("--centre", "x,y").contains("--centre"));
    assert!(with("--size", "640").contains("--size"));
    assert!(with("--size", "10x10").contains("outside"));
    assert!(with("--size", "9000x480").contains("outside"));
    assert!(with("--range-nm", "far").contains("--range-nm"));
    let missing = cli::parse_render_args(&args(&ok[1..ok.len() - 2]))
        .unwrap_err()
        .to_string();
    assert!(missing.contains("missing --out"));
    let twice = cli::parse_render_args(&args(&[&ok[1..], &["--out", "p"]].concat()))
        .unwrap_err()
        .to_string();
    assert!(twice.contains("more than once"));
    let unknown = cli::parse_render_args(&args(&["--bogus"]))
        .unwrap_err()
        .to_string();
    assert!(unknown.contains("unknown argument"));
    assert!(cli::parse_render_args(&args(&["--snapshot"]))
        .unwrap_err()
        .to_string()
        .contains("missing value"));
    // A range outside the projection's limits fails at the view, not in a panic.
    let mut sink = Vec::new();
    let e = cli::run(
        &args(&[
            "render",
            "--snapshot",
            "/nonexistent",
            "--centre",
            "0,0",
            "--range-nm",
            "50",
            "--size",
            "640x480",
            "--out",
            "o",
        ]),
        &mut sink,
    );
    assert!(e.unwrap_err().to_string().contains("read"));
    assert!(cli::run(&args(&[]), &mut sink).is_err());
    assert!(cli::run(&args(&["--help"]), &mut sink).is_ok());
}

// REQ: OA-DSP-031
#[test]
fn the_frame_hash_is_computed_only_on_request_and_layers_come_from_the_command_line() {
    let snap = tmp("hand-built-hash.snapshot");
    std::fs::write(&snap, encode(&model(Tail::Border)).unwrap()).unwrap();
    let run = |extra: &[&str]| {
        let ppm = tmp("hand-built-hash.ppm");
        let mut v = vec![
            "render",
            "--snapshot",
            snap.to_str().unwrap(),
            "--centre",
            "41.9786,-87.9048",
            "--range-nm",
            "50",
            "--size",
            "800x600",
            "--out",
            ppm.to_str().unwrap(),
        ];
        v.extend_from_slice(extra);
        let mut out = Vec::new();
        cli::run(&args(&v), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    };
    let plain = run(&[]);
    assert!(!plain.contains("sha256"), "{plain}");
    assert!(run(&["--hash"]).contains("sha256 "));
    // Hiding layers and moving thresholds go through, and a bad layer name is a usage error.
    let hidden = run(&[
        "--hide",
        "fixes,rings",
        "--fix-max-nm",
        "10",
        "--label-max-nm",
        "5",
    ]);
    assert!(hidden.contains("0 fixes"), "{hidden}");
    let e = cli::run(
        &args(&[
            "render",
            "--snapshot",
            "a",
            "--centre",
            "0,0",
            "--range-nm",
            "5",
            "--size",
            "640x480",
            "--out",
            "o",
            "--hide",
            "bogus",
        ]),
        &mut Vec::new(),
    );
    assert!(e.unwrap_err().to_string().contains("bogus"));
}

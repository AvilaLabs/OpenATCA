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

/// A valid model: four points (two fixes, a VOR, an airport), a HIGH ring left open, a LOW ring
/// already closed, a class B ring that the map does not draw, one airway of two legs, and, when
/// `unresolved` is set, a third leg whose far end is not a point.
fn model(unresolved: bool) -> Model {
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
        ("AAAAA", point_kind::FIX, e7(42.30, -88.30)),
        ("BBBBB", point_kind::FIX, e7(41.60, -87.40)),
        ("DPA", point_kind::VOR, e7(41.90, -88.26)),
        ("KORD", point_kind::AIRPORT, e7(41.9786, -87.9048)),
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

    let square = |lat: f64, lon: f64, d: f64| {
        [
            e7(lat + d, lon - d),
            e7(lat + d, lon + d),
            e7(lat - d, lon + d),
            e7(lat - d, lon - d),
        ]
    };
    let mut rings: Vec<Ring> = Vec::new();
    rings.push((
        boundary_source::NASR_ARB,
        stratum::ARTCC_HIGH,
        "ZAU",
        square(41.9, -87.9, 0.8).to_vec(),
    ));
    let mut low = square(41.9, -87.9, 0.5).to_vec();
    low.push(low[0]);
    rings.push((boundary_source::NASR_ARB, stratum::ARTCC_LOW, "ZAU", low));
    rings.push((
        boundary_source::CIFP_UC,
        stratum::CLASS_B,
        "KORD",
        square(41.98, -87.9, 0.2).to_vec(),
    ));
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

    let legs: Vec<(PointIx, PointIx)> = if unresolved {
        vec![
            (PointIx::new(0), PointIx::new(2)),
            (PointIx::new(2), PointIx::new(1)),
            (PointIx::new(1), PointIx::NONE),
        ]
    } else {
        vec![
            (PointIx::new(0), PointIx::new(2)),
            (PointIx::new(2), PointIx::new(1)),
        ]
    };
    m.airways.designator.push(s.add("V9"));
    m.airways.location.push(b'V');
    m.airways.legs.push(Span {
        start: AirwayLegIx::new(0),
        len: legs.len() as u32,
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

// REQ: OA-DSP-017
#[test]
fn the_map_draws_boundaries_airways_symbols_rings_and_legend_in_their_colours() {
    let m = model(false);
    validate_model(&m).unwrap();
    let view = view_new(ORD.0, ORD.1, 60.0, 800, 600).unwrap();
    let mut l = list_new(scene_caps(&m));
    let st = build_scene(&mut l, &m, &view).unwrap();
    assert!(!l.faulted);
    assert_eq!(
        (st.boundaries, st.fixes, st.navaids, st.airports),
        (2, 2, 1, 1)
    );
    assert_eq!(st.airway_legs, 2);
    assert_eq!(
        (st.points_unplaced, st.legs_unplaced, st.edges_unplaced),
        (0, 0, 0)
    );

    // Two polylines (class B is not drawn): HIGH and LOW in their colours on their layers.
    assert_eq!(l.polylines.colour, vec![pal::ARTCC_HIGH, pal::ARTCC_LOW]);
    assert_eq!(l.polylines.layer, vec![LAYER_ARTCC_HIGH, LAYER_ARTCC_LOW]);
    // The open HIGH ring is closed back to its first vertex; the closed LOW ring is not doubled.
    let ring = |i: usize| {
        let (s, n) = (l.polylines.start[i] as usize, l.polylines.len[i] as usize);
        (l.poly_x[s..s + n].to_vec(), l.poly_y[s..s + n].to_vec())
    };
    let (hx, hy) = ring(0);
    assert_eq!((hx[0], hy[0]), (hx[hx.len() - 1], hy[hy.len() - 1]));
    assert!(hx.len() >= 5);
    let (lx, ly) = ring(1);
    assert_eq!((lx[0], ly[0]), (lx[lx.len() - 1], ly[ly.len() - 1]));
    // The first HIGH vertex is where the projection puts the model's first vertex.
    let (v0, v1) = (m.vertices[0].lat, m.vertices[0].lon);
    assert_eq!(project(&view.proj, v0, v1), Some((hx[0], hy[0])));

    // Airway legs are lines in the airway colour; fixes have triangles; the airport a circle.
    let airway_lines = l.lines.colour.iter().filter(|&&c| c == pal::AIRWAY).count();
    assert!(airway_lines >= 2);
    let fix_lines = l.lines.colour.iter().filter(|&&c| c == pal::FIX).count();
    assert_eq!(fix_lines, 3 * 2 + 1, "two triangles and a navaid dot");
    assert_eq!(
        l.circles
            .colour
            .iter()
            .filter(|&&c| c == pal::AIRPORT)
            .count(),
        1
    );
    assert_eq!(
        l.circles.colour.iter().filter(|&&c| c == pal::FIX).count(),
        1
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
    for label in ["20", "40", "60"] {
        assert!(t.contains(&label), "no ring label {label} in {t:?}");
    }

    // Legend: facility, cycles, range, and both strata in their own colours.
    assert!(t.contains(&"ZAU NASR 20261029 CIFP 2611"));
    assert!(t.contains(&"NORTH UP  RANGE 60 NM  RINGS 20 NM"));
    let named = |name: &str, colour: u8| {
        t.iter()
            .zip(&l.texts.colour)
            .any(|(s, &c)| *s == name && c == colour)
    };
    assert!(named("ARTCC HIGH", pal::ARTCC_HIGH) && named("ARTCC LOW", pal::ARTCC_LOW));
    assert!(!t.iter().any(|s| s.starts_with("NOT PLACED")));

    // The picture: the third HIGH vertex (south east, clear of the legend) is a HIGH pixel.
    let sp = spec(800, 600);
    let mut buf = vec![0u8; frame_len(&sp)];
    render(&l, &sp, &mut buf).unwrap();
    let (sx, sy) = project(&view.proj, m.vertices[2].lat, m.vertices[2].lon).unwrap();
    let (px, py) = ((sx >> 4) as usize, (sy >> 4) as usize);
    let o = py * sp.stride as usize + px * 4;
    assert_eq!(
        u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()),
        pal::PALETTE[usize::from(pal::ARTCC_HIGH)]
    );
}

// REQ: OA-DSP-017
#[test]
fn labels_appear_only_below_their_range_thresholds() {
    let m = model(false);
    let labelled = |range: f64| {
        let view = view_new(ORD.0, ORD.1, range, 800, 600).unwrap();
        let mut l = list_new(scene_caps(&m));
        let st = build_scene(&mut l, &m, &view).unwrap();
        let t: Vec<String> = texts(&l).iter().map(|s| (*s).to_string()).collect();
        (st, t)
    };
    let (st, t) = labelled(50.0);
    assert!(
        t.contains(&"KORD".to_string())
            && t.contains(&"AAAAA".to_string())
            && t.contains(&"DPA".to_string())
    );
    assert_eq!(st.labels, 4);
    let (st, t) = labelled(80.0);
    assert!(!t.contains(&"KORD".to_string()) && !t.contains(&"AAAAA".to_string()));
    assert_eq!(st.labels, 0);
    // The thresholds are configurable and independent.
    let mut view = view_new(ORD.0, ORD.1, 80.0, 800, 600).unwrap();
    view.airport_label_max_nm = 100.0;
    let mut l = list_new(scene_caps(&m));
    build_scene(&mut l, &m, &view).unwrap();
    let t = texts(&l);
    assert!(t.contains(&"KORD") && !t.contains(&"AAAAA"));
}

// REQ: OA-DSP-023
#[test]
fn what_cannot_be_placed_is_counted_and_said_on_the_screen() {
    let m = model(true);
    let view = view_new(ORD.0, ORD.1, 60.0, 800, 600).unwrap();
    let mut l = list_new(scene_caps(&m));
    let st = build_scene(&mut l, &m, &view).unwrap();
    assert_eq!((st.legs_unplaced, st.airway_legs), (1, 2));
    assert!(texts(&l).contains(&"NOT PLACED 0 PTS 1 LEGS 0 EDGES"));
    let i = texts(&l)
        .iter()
        .position(|s| s.starts_with("NOT PLACED"))
        .unwrap();
    assert_eq!(l.texts.colour[i], pal::ALERT);
}

// REQ: OA-DSP-018
#[test]
fn an_overflowing_scene_is_an_error_and_the_list_is_never_drawn() {
    let m = model(false);
    let view = view_new(ORD.0, ORD.1, 60.0, 800, 600).unwrap();
    let full = scene_caps(&m);
    // Each table, one row short of what the scene needs, fails by name.
    let mut l = list_new(full);
    build_scene(&mut l, &m, &view).unwrap();
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
        match build_scene(&mut l, &m, &view) {
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
    // The capacities scene_caps gives hold at every range, extreme centres, and a huge zoom.
    for (lat, lon) in [
        ORD,
        (900_000_000, 0),
        (-900_000_000, 1_800_000_000),
        (0, -1_800_000_000),
        (0, 0),
    ] {
        for range in [0.05, 1.0, 60.0, 500.0, 5_000.0, 20_000.0] {
            let view = view_new(lat, lon, range, 640, 480).unwrap();
            let mut l = list_new(full);
            build_scene(&mut l, &m, &view).unwrap_or_else(|e| panic!("{lat},{lon} {range}: {e}"));
            let mut buf = vec![0u8; 640 * 480 * 4];
            rasterize(&l, &spec(640, 480), &mut buf).unwrap();
        }
    }
}

// REQ: OA-DSP-019
#[test]
fn frames_are_identical_and_never_change_the_lists_capacity() {
    let m = model(false);
    let mut l = list_new(scene_caps(&m));
    let caps = list_capacities(&l);
    let sp = spec(800, 600);
    let mut buf = vec![0u8; frame_len(&sp)];
    let mut digests = Vec::new();
    for range in [60.0, 250.0, 5.0, 60.0, 1000.0, 60.0] {
        let view = view_new(ORD.0, ORD.1, range, 800, 600).unwrap();
        build_scene(&mut l, &m, &view).unwrap();
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
    build_scene(&mut fresh, &m, &view).unwrap();
    build_scene(&mut l, &m, &view).unwrap();
    assert_eq!(fresh, l);
}

// REQ: OA-DSP-012
#[test]
fn the_hand_built_map_renders_to_a_pinned_hash() {
    // The projection uses the platform's trigonometry, which may differ in the last bit between
    // libm versions; this scene is built so no vertex lies near a 1/16 pixel rounding boundary on
    // the machines checked. A mismatch here with the raster goldens (in `raster.rs`) intact means
    // the projection moved, not the rasterizer.
    let m = model(false);
    let view = view_new(ORD.0, ORD.1, 60.0, 800, 600).unwrap();
    let mut l = list_new(scene_caps(&m));
    build_scene(&mut l, &m, &view).unwrap();
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
        "357eaa91a188f93e183ca28e68915423009c4e848e6db54c1f5421baee718874"
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
    let bytes = encode(&model(false)).unwrap();
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
    build_scene(&mut l, &m, &view).unwrap();
    let sp = spec(800, 600);
    let mut buf = vec![0u8; frame_len(&sp)];
    rasterize(&l, &sp, &mut buf).unwrap();
    assert_eq!(to_hex(&frame_digest(&sp, &buf).unwrap()), want);
    assert!(said.contains("2 boundaries") && said.contains("time     build"));
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

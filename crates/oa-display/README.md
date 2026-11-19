# oa-display

The safe, pure foundation of the controller display (SCOPE design principle 7: safety over
flash, data shown with as little abstraction as possible).

Data goes through three plain stages. The model and state become a flat list of drawing commands,
rebuilt from scratch every frame (`scene::build_scene` into a `list::DisplayList`). The list is drawn
into a pixel buffer (`raster::render`). The buffer is presented. Presentation (KMS/DRM page
flipping) is not in this crate: it is the one crate allowed `unsafe` (RULESET A0). Nothing here can
reach the screen, so everything here is testable bit for bit, and every frame carries a SHA-256.

Core versus shell (RULESET B1):

The modules live in `src/core/` and `src/shell/` and are re-exported at the crate root, so
`oa_display::traffic` is `oa_display::core::traffic`. Every public function of a module has its
module's class.

| module    | class | role                                                               |
|-----------|-------|--------------------------------------------------------------------|
| `list`    | core  | the fixed-capacity struct-of-arrays display list and its pushes     |
| `palette` | core  | the fixed colour palette and its colour indices                    |
| `project` | core  | WGS-84 to screen: oblique stereographic projection and its inverse  |
| `font`    | core  | the built-in stroke font, const data                               |
| `raster`  | core  | integer software rasterizer into an XRGB8888 buffer, frame hash    |
| `scene`   | core  | airspace model to display list (boundaries, airways, fixes, rings)  |
| `traffic` | core  | live targets to display list: symbols, leader, trail, data block    |
| `text`    | core  | fixed-size stack text buffer: the data block text, no allocation    |
| `ppm`     | shell | binary PPM (P6) writer                                             |
| `cli`     | shell | the `oa-scope` command line, file reads and writes                 |

## `oa-scope`

```text
oa-scope render --snapshot <file> --centre <lat,lon> --range-nm <n> --size <WxH> --out <file.ppm>
                [--hide <layer,...>] [--fix-max-nm <n>] [--terminal-max-nm <n>]
                [--runway-max-nm <n>] [--airport-max-nm <n>] [--navaid-max-nm <n>]
                [--label-max-nm <n>] [--major-label-max-nm <n>] [--hash]
```

Draws a snapshot built by `oa-adapt` as a static scope picture. `--range-nm` is the range as a
scope sets it: nautical miles from the centre to the nearest edge of the map area. It prints what
was drawn, what could not be placed, and the time of each stage. The frame's SHA-256 is printed
only with `--hash`: hashing costs more than drawing, so it is for tests and checks, not for every
frame.

**Detail follows the display range (the zoom), never a symbol's distance from the centre.** Every
threshold below is compared with `--range-nm`, so at one zoom the same kinds of symbol show over
the whole screen. Defaults, which declutter the picture (all configurable):

| layer name (`--hide`)  | shown by default                                                   |
|------------------------|--------------------------------------------------------------------|
| `own-boundary`         | the facility's HIGH and LOW boundaries, 2 px wide                  |
| `neighbour-boundaries` | other facilities' boundaries, 1 px wide, dimmer                    |
| `high-airways`         | J and Q airways                                                    |
| `low-airways`          | V and T airways                                                    |
| `vors`                 | VOR, VORTAC, VOR-DME always; NDB, DME, TACAN at range <= 40 NM (`--navaid-max-nm`) |
| `airports`             | airports with a coded instrument procedure always; others at range <= 40 NM (`--airport-max-nm`); runway thresholds and localizers at range <= 15 NM (`--runway-max-nm`) |
| `fixes`                | fixes at range <= 40 NM (`--fix-max-nm`); terminal waypoints at range <= 20 NM (`--terminal-max-nm`) |
| `labels`               | labels of fixes, minor navaids, other airports and terminal waypoints at range <= 40 NM (`--label-max-nm`); labels of VORs and instrument airports at range <= 100 NM (`--major-label-max-nm`) |
| `rings`                | range rings, labelled on the 45 degree north-east diagonal         |

A legend line `HIDDEN: ...` names every layer switched off and every threshold that is hiding
something at the current range, so a clean picture never hides that it is clean.

Labels are placed deterministically. Candidates are sorted by priority (VOR-class navaids, then
airports with instrument procedures, other airports, fixes and minor navaids, terminal waypoints),
then ident, then position, then table row. Each takes the first of four fixed offsets (north-east,
south-east, north-west, south-west of its symbol) whose box overlaps no placed label, no ring
label, the legend strip nor the screen edge. A label with no free offset is dropped and counted:
the legend shows `LABELS DROPPED: N` in the normal colour. The placed boxes are kept in a coarse
grid of 64 px cells with a bounded list each, allocated once; a full cell drops the label and
counts it, it never overflows. The same input gives the same labels on every machine.

The legend is a fixed 104 px strip at the top of the screen that the projection leaves out of the
map area. The alternative, a legend without a background drawn over the map, can still cover
data and gives text no contrast; the strip guarantees the legend never hides map data and the
range circle always fits, and costs about a tenth of a 1080 px screen. Legs that end at border
crossings (published points outside the data, expected) are counted in the normal text colour
("N LEGS END AT BORDER CROSSINGS"). The alert colour is used only for conditions that need
attention, never expected data: here, points, legs or edges that could not be placed.

## Traffic

`traffic` draws live targets over the map. The display knows a target only as plain data
(`traffic::Target`: position, ground track and speed, altitude, address, callsign, coasting and
uncertain flags, seconds since reacquisition, 95% radius in metres, up to 16 history positions);
the tracker is not a dependency. `build_traffic` appends to the list `build_scene` built, so a
frame is state, then list, then pixels, as for the map. Targets are drawn in set order.

| state                  | drawing                                                                   |
|------------------------|---------------------------------------------------------------------------|
| tracking               | 8 px square outline and a centre dot                                       |
| coasting               | the square with an X through it, no centre dot; tag `CST`                  |
| uncertain              | 12 px diamond and a centre dot, the 95% radius as a circle of true scale; tag `UNC` |
| coasting and uncertain | the diamond with an X through it, the circle; tags `CST UNC`               |
| reacquired (N s)       | a 9 px circle in the alert colour added to any of the above; tag `REACQ`   |
| callsign conflict      | two callsigns heard for the address at one instant: its hex address is shown, tag `CS?` |
| uncertain, radius unknown | the diamond without the circle; tag `R95?`                              |

All of it is in the target colour (index 12). Each target also has a velocity leader along its track
(default 1 minute of flight, none without a velocity), history dots in the dim text colour (default
the last 5 positions) and a data block 14 px right and 30 px above the symbol: callsign (hex address
without one), altitude in hundreds of feet and ground speed in knots over ten (`---` and `--` when
absent), then the tags (`CST UNC CS? R95? REACQ`). Data blocks do not yet avoid one another (LS-401).
A frame allocates nothing: the data block text is written into a stack buffer, and the list's tables
are sized once (checked by `validation/oa-alloc-check`).

A `TargetSet` has a fixed capacity and private columns: the only way in is `targets_push`. A push past
the capacity is refused, counted and returns `TrafficError::Full`; a target that is not valid (a
position off the globe, a trail over 16, unknown flag bits) returns `TrafficError::Malformed`. The frame
then shows `TARGETS NOT DRAWN: N` and `TARGETS MALFORMED: N` at the bottom left in the alert colour.
Every part of a target that cannot be drawn (a leader, the 95% circle, a history dot, a text) is
counted by reason in `TrafficStats`, and `traffic_loss` sums what is lost. Targets off the map area are
counted in `TrafficStats::off_screen`. Traffic is on layers 7 to
10, above the map and below the legend strip. `oa-scope-replay` (crate `oa-scope-replay`) runs the
tracker over an ADS-B clip and draws it:

```text
oa-scope-replay --snapshot <file> --adsb <clip.oaf> --centre <lat,lon> --range-nm <n> --size <WxH>
                --start <unix-seconds|YYYY-MM-DDTHH:MM:SSZ> --duration-s <n> --step-s <n> --out-dir <dir>
                [--leader-min <n>] [--trail-n <n>] [--trail-step-s <n>] [--reacq-s <n>]
                [--max-targets <n>] [--warmup-s <n>] [--hashes-only] [--overwrite] [--allow-loss]
```

It writes `frame-NNNN.ppm` per step and `hashes.txt` (`frame_index time sha256`) into an empty or new
directory (`--overwrite` deletes the files of an earlier run first). It ends with a report of
everything it excluded or lost, by reason, and ends with exit code 3 if work was lost unless
`--allow-loss` is given (0 success, 1 failure, 2 usage error, a duration that is not a whole number
of steps among them). The tracker is linked in that crate and not here, so the display stays free of it.

Peak memory of a run, from the named capacities: the row store is at most 2^24 rows of 32 bytes
(512 MiB, and the reacquired times, 8 bytes for each reacquired row, a small part of it); the callsign changes at most 2^25 entries of about 17 bytes (about 544 MiB, far less in
practice); one tracker output table, the display tables and the frame buffer (`width * height * 4`,
8 MiB at 1920x1080) are small beside them. Together at most about 1.1 GiB, reached only by a
window larger than any clip this tool is sized for; a typical hour is a few hundred MiB.

## Display list

Primitives: line segment, polyline (a span of a shared vertex array), filled rectangle, circle
outline, and text (a span of a string table, a position, a size, an alignment). Every primitive
carries a palette index and a layer (0 to 15). Coordinates are `i32` in 1/16 pixel.

Capacity is fixed at creation. A push that does not fit returns `DisplayError::Full` naming the
table and marks the list *faulted*; the rasterizer refuses a faulted list, so a partial frame is
never drawn by mistake. Clearing for the next frame keeps all capacity: after start-up, a frame
allocates nothing in the list.

## Projection

Oblique stereographic projection of the WGS-84 ellipsoid (the EPSG 9809 method) about a chosen
centre. It is conformal, true at the centre, and its error against the geodesics of oa-geo is
measured in a test; see the module documentation of `project`. Positions are 1e-7 degree `i32` in
and 1/16 pixel `i32` out. Range rings are circles of true radius about the centre.

## Rendering rules

* A point is drawn in the pixel that contains it; a filled rectangle fills the pixels whose
  centres lie inside it. Pixel (0, 0) is the top left.
* The rasterizer is integer only, allocates nothing, and writes into a caller-owned XRGB8888
  buffer (blue, green, red, unused, as a KMS dumb buffer holds it) of `stride * height` bytes.
  Every pixel write is bounds-checked, and every primitive is clipped analytically first, however
  far outside the buffer its coordinates lie.
* Lines are one or two pixels wide. A one pixel line is an exact integer midpoint (Bresenham) line;
  a two pixel line is that line plus a copy one pixel along its minor axis. Both are drawn with no blending: every
  pixel on screen is a palette colour, never a blend that could read as a different colour, and
  the picture is a pure function of the display list. A line is the same whichever end is first.
* Layers draw in ascending order; within a layer: rectangles, circles, polylines, lines, text.
* The frame hash is the SHA-256 of the visible pixels (each row's `width * 4` bytes in order), so
  it does not depend on the stride.

## Stroke font

Every character is a few line strokes on a 5 by 8 grid, scaled by the text size (the size is the
cap height in pixels). A to Z, 0 to 9, space and `. , / - + : ( ) * # < > ↑ ↓`; lowercase draws as
capitals; any other character draws as a crossed box, so it is seen and never dropped. Confusable
characters look clearly different: zero is slashed, one has a flag and no foot while I has
serifs, and S and 5, B and 8, Z and 2 differ in their corners and curves. A test measures it: each of
these pairs differs in at least 4 of the grid's 40 cells.

## Colour palette

A dark background, so a controller's eyes stay dark-adapted, and line colours from the
Okabe-Ito set, chosen to stay distinguishable under every common form of colour-vision
deficiency. Tests check that every foreground colour reaches at least 4.5:1 contrast against the
background (WCAG AA), and that no two data colours come within 55 (of 255, in RGB) of each other as
seen with protanopia, deuteranopia or tritanopia (Machado et al. 2009 simulation). Information is
never carried by colour alone: ARTCC HIGH and LOW are named in the legend in their own colours,
and symbol shape separates fixes (triangle), navaids (small ring and dot) and airports (ring and
dot).

| index | name       | colour    | use                              |
|-------|------------|-----------|----------------------------------|
| 0     | background | `#0B0F14` | clear colour                     |
| 1     | text       | `#E8EDF2` | legend, labels                   |
| 2     | ARTCC HIGH | `#56B4E9` | sky blue boundary                |
| 3     | ARTCC LOW  | `#E69F00` | orange boundary                  |
| 4     | airway     | `#8A97A6` | thin grey-blue lines             |
| 5     | fix        | `#F0E442` | fix and navaid symbols           |
| 6     | airport    | `#F2F5F8` | white airport symbols and idents |
| 7     | range ring | `#2F3B48` | dim rings (decoration, not data) |
| 8     | dim text   | `#7E8B99` | ring labels                      |
| 9     | alert      | `#FF5A4F` | attention conditions only        |
| 10    | neighbour HIGH | `#2F6F96` | dim neighbour boundary      |
| 11    | neighbour LOW  | `#9A6B00` | dim neighbour boundary      |
| 12    | target     | `#7FE0B0` | live targets: symbol, leader, data block |

## Tests

`cargo test -p oa-display`. The real-data test reads a ZAU snapshot from `OPENATCA_SNAPSHOT`
(build one with `oa-adapt build --nasr <zip> --cifp <zip> --artcc ZAU --out <file>`), skips when
it is unset, and reports the time per frame at 1920 by 1080 at 250 NM and 60 NM. Setting `OA_DISPLAY_DUMP=<dir>` makes
the golden-hash tests write their pictures there as PPM files, for looking at a changed hash.

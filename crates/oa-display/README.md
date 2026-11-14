# oa-display

The safe, pure foundation of the controller display (SCOPE design principle 7: safety over
flash, data shown with as little abstraction as possible).

Data goes through three plain stages. The model and state become a flat list of drawing commands,
rebuilt from scratch every frame (`scene::build_scene` into a `list::DisplayList`). The list is drawn
into a pixel buffer (`raster::render`). The buffer is presented. Presentation (KMS/DRM page
flipping) is not in this crate: it is the one crate allowed `unsafe` (RULESET A0). Nothing here can
reach the screen, so everything here is testable bit for bit, and every frame carries a SHA-256.

Core versus shell (RULESET B1):

| module    | class | role                                                               |
|-----------|-------|--------------------------------------------------------------------|
| `list`    | core  | the fixed-capacity struct-of-arrays display list and its pushes     |
| `palette` | core  | the fixed colour palette and its colour indices                    |
| `project` | core  | WGS-84 to screen: oblique stereographic projection and its inverse  |
| `font`    | core  | the built-in stroke font, const data                               |
| `raster`  | core  | integer software rasterizer into an XRGB8888 buffer, frame hash    |
| `scene`   | core  | airspace model to display list (boundaries, airways, fixes, rings)  |
| `ppm`     | shell | binary PPM (P6) writer                                             |
| `cli`     | shell | the `oa-scope` command line, file reads and writes                 |

## `oa-scope`

```text
oa-scope render --snapshot <file> --centre <lat,lon> --range-nm <n> --size <WxH> --out <file.ppm>
                [--hide <layer,...>] [--fix-max-nm <n>] [--airport-max-nm <n>]
                [--navaid-max-nm <n>] [--label-max-nm <n>] [--hash]
```

Draws a snapshot built by `oa-adapt` as a static scope picture. `--range-nm` is the range as a
scope sets it: nautical miles from the centre to the nearest edge of the map area. It prints what
was drawn, what could not be placed, and the time of each stage. The frame's SHA-256 is printed
only with `--hash`: hashing costs more than drawing, so it is for tests and checks, not for every
frame.

Defaults, which declutter the picture (all configurable):

| layer name (`--hide`)  | shown by default                                                  |
|------------------------|-------------------------------------------------------------------|
| `own-boundary`         | the facility's HIGH and LOW boundaries, 2 px wide                 |
| `neighbour-boundaries` | other facilities' boundaries, 1 px wide, dimmer                   |
| `high-airways`         | J and Q airways                                                   |
| `low-airways`          | V and T airways                                                   |
| `vors`                 | VOR, VORTAC, VOR-DME, always                                      |
| `airports`             | within 60 NM; beyond that only with a coded instrument procedure  |
| `fixes`                | within 60 NM (`--fix-max-nm`)                                     |
| `labels`               | within 60 NM (`--label-max-nm`)                                   |
| `rings`                | range rings, labelled on the 45 degree north-east diagonal        |

NDB, DME and TACAN show within 60 NM (`--navaid-max-nm`); airports without a procedure show
within `--airport-max-nm` (60). A legend line `HIDDEN: ...` names every layer switched off and
every threshold that is hiding something at the current range, so a clean picture never hides
that it is clean.

The legend is a fixed 104 px strip at the top of the screen that the projection leaves out of the
map area. The alternative, a legend without a background drawn over the map, can still cover
data and gives text no contrast; the strip guarantees the legend never hides map data and the
range circle always fits, and costs about a tenth of a 1080 px screen. Legs that end at border
crossings (published points outside the data, expected) are counted in the normal text colour
("N LEGS END AT BORDER CROSSINGS"). The alert colour is used only for conditions that need
attention, never expected data: here, points, legs or edges that could not be placed.

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

## Tests

`cargo test -p oa-display`. The real-data test reads a ZAU snapshot from `OPENATCA_SNAPSHOT`
(build one with `oa-adapt build --nasr <zip> --cifp <zip> --artcc ZAU --out <file>`), skips when
it is unset, and reports the time per frame at 1920 by 1080 at 250 NM and 60 NM. Setting `OA_DISPLAY_DUMP=<dir>` makes
the golden-hash tests write their pictures there as PPM files, for looking at a changed hash.

# Patina — Roadmap

The outstanding work, in the order it is being done (sorted 2026-11-01). Each item is its own branch and commit;
when an item lands, it is marked done here and what it decided goes into
[settled-decisions.md](settled-decisions.md). The order
puts what later work builds on first: formatting before the large rewrites, the theme before the panel and the
viewer that use it.

The UI items come from a seven-lens review (visual system, editing workflow, viewer, keyboard and accessibility,
feedback, a photographer culling a shoot, a contrarian minimalist) consolidated by convergence. Its data-loss
findings were fixed first (`c8a9541`, `df1efe2`).

## Direction

A quiet dark-green darkroom: the photo is the brightest thing on screen, and the patina green only *means*
something — "you changed this", "the default action", "focused". Today green is decoration (every section dot
glows, Save is lit on an untouched image) and the brightest pixels in the editor are twelve identical placeholder
tiles. The viewer keeps its zero-chrome canvas. The editor rail is reorganised around the edit itself.

## Foundations

1. ~~**rustfmt.**~~ Done — see settled-decisions.md.
2. ~~**Logging.**~~ Done — see settled-decisions.md. The help overlay shows the log's path and opens its folder,
   and a second instance appends to the running one's log instead of moving it away.
3. ~~**Windows-app basics.**~~ Done — the title follows navigation (`● IMG_0412.jpg — 12 / 129 — Patina`), one
   keymap table (`src/keymap.rs`) drives the handler and the help, Tab is egui's, G cycles Off → Thirds → Golden.
4. ~~**Theme foundation.**~~ Done — tokens, one button, Inter, pinned dark theme; see settled-decisions.md.

## The editor

5. ~~**Rail rebuilt around the edit.**~~ Done — one toolbar, open slider groups, preset chips that preview on
   hover, one undo step per drag, Scale gone, EXIF on an Info card (I); see settled-decisions.md. Two parts of the
   plan moved: Before/After is item 7's Backslash rather than a toolbar button, and presets stay a pipeline stage
   instead of writing offsets into the sliders (five of them are not slider offsets).

## The viewer

6. ~~**Where am I, what happened.**~~ Done — HUD, toast lane, loading bar, bad-file card, empty state with
   Ctrl+O; see settled-decisions.md.

## Judging an edit

7. ~~**Judging an edit.**~~ Done — Backslash holds up the original, zoom with a pixel-true 100% in both modes, a
   histogram in the rail head; see settled-decisions.md.

## Working through a set

8. ~~**Working through a set.**~~ Done — arrows, 0–5, L and 6–9 in the editor too (an arrow asks to save first),
   Ctrl+C / Ctrl+V of the look between images; see settled-decisions.md.

## Open decisions, after the UI

9. ~~**Preload cache.**~~ Done — measured one more decoded image per step, now the two neighbours only; a Save
   As over a neighbour no longer serves its old pixels; see settled-decisions.md. Its real-app check found each
   arrow held the window ~0.5 s at 24 MP; the Fit reduction now happens where the image is decoded (9–17 ms).
10. ~~**Sidecar name.**~~ Done — `photo.jpg.xmp` and `photo.xmp` are both read and written, `photo.jpg.xmp` is
    created when neither exists; see settled-decisions.md.
11. ~~**Orientation everywhere.**~~ Done — RAW previews are turned by the RAW's Orientation tag, and the first
    window is sized as the photo is shown; see settled-decisions.md. Checked on a real CR2 in both directions.
12. ~~**Icon in the executable.**~~ Done — `build.rs` embeds `patina.ico` and a version block, and the window
    icon is set from the same file; see settled-decisions.md.
13. ~~**Sort order like Explorer.**~~ Done — `StrCmpLogicalW` in `explorer_order.rs`, the one module allowed
    `unsafe`, chosen over ICU4X after measuring both; see settled-decisions.md.
14. ~~**WebP quality.**~~ Done — the option went: WebP is written lossless and the dialog says so; lossy
    (libwebp, +416 KB of C) measured and not taken; see settled-decisions.md.
15. ~~**Metadata a save still loses.**~~ Done — RAW and JPEG XL sources' EXIF, a TIFF's profile, the true pixel
    size, embedded XMP into JPEG and PNG, and the location left out of a web export (EXIF and XMP); WebP/TIFF
    XMP outputs stay open; see settled-decisions.md.

## Considered and dropped

Live preset thumbnails (twelve renders per image for a 108×81 preview the canvas shows better); a filmstrip
(ends the zero-chrome viewer and needs the whole folder read); cull-mode delete changes, two-up compare and
batch export (features, not UI refinements — revisit after 6–8 exist); slideshow polish.

## From sweeps

- **The user-file write layer is split across unrelated modules.** *Where*: `xmp::write_atomic`
  (src/xmp.rs:306), called by save.rs:354 and orientation.rs:74; `rename_no_replace` (src/main.rs:3182), called
  by xmp.rs:139. *Why it is not the obvious thing*: both are the primitives every write to the user's files goes
  through, but one lives in the sidecar module and one in the `App`; moving them to a module of their own
  (`fsio.rs`) is a structure decision, and the delete path (`trash`) might belong there too. *What would
  falsify it*: a third caller set that shows the two are not one layer (e.g. a write that must not be atomic).
  *Origin*: sweep 2026-11-08 step 1, not yet committed to build.
- **eframe/egui 0.27 is behind, and it is where the unmaintained crates come from.** *Where*: Cargo.toml
  `eframe`, `egui`, `egui_extras` = 0.27. `cargo deny check advisories` names paste (via accesskit_windows),
  ttf-parser (ab_glyph/epaint) and webbrowser in the Windows tree, and derivative, instant, zbus_names, zvariant
  and quick-xml in its Linux tree; `cargo tree -i <crate> --target all` traces each to eframe 0.27. *Why it is not
  the obvious thing*: each egui minor breaks API (`Context`, `ViewportBuilder`, `Painter`, the
  `egui::Frame`/`Margin` types the HUD and rail draw with), and the GPU texture path in view.rs and every render
  harness (`painted_text`, `help_at`) would move with it; it is a package with a real-app check, not a bump.
  *What would falsify it*: a newer eframe that still pulls the same unmaintained crates. *Origin*: sweep
  2026-11-08 step 3, not yet committed to build.
- **149 compatible updates are pending, and a blanket `cargo update` adds 13 crates.** *Where*: Cargo.lock;
  `cargo update --dry-run` adds getrandom 0.4, hashbrown 0.17, libm, miniz_oxide 0.9, num-complex, pulp,
  pulp-wasm-simd-flag, raw-cpuid, reborrow, syn 2.0.119 and 3.0.9, utf_types and zlib-rs. *Why it is not the
  obvious thing*: every new crate needs its justification (CLAUDE.md), and pulp/raw-cpuid look like a SIMD
  backend change in a decoder; the sweep took only the four advisory and yanked fixes. *What would falsify it*:
  the 13 additions all being build-time or non-Windows. *Origin*: sweep 2026-11-08 step 3, not yet committed to
  build.

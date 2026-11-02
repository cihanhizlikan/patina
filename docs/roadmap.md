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
2. ~~**Logging.**~~ Done — see settled-decisions.md. **Still owed before a release**: the log needs a more
   elegant way in than a file path, e.g. *Help → Open log folder*, or a *Copy log* button on the error dialog.
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
11. **Orientation everywhere** — the first window's size, and RAW files.
12. **Icon in the executable** — `build.rs` with `winres`, which is already a build-dependency.
13. **Sort order like Explorer** — needs a safe crate or a reasoned `unsafe` exception.
14. **WebP quality** — the encoder is lossless only; lossy needs a crate or the option goes.
15. **Metadata a save still loses** — embedded XMP, RAW/JPEG XL sources, a TIFF's ICC; GPS in the Web preset.

## Considered and dropped

Live preset thumbnails (twelve renders per image for a 108×81 preview the canvas shows better); a filmstrip
(ends the zero-chrome viewer and needs the whole folder read); cull-mode delete changes, two-up compare and
batch export (features, not UI refinements — revisit after 6–8 exist); slideshow polish.

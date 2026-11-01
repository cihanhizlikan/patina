# Patina — Roadmap

The outstanding work, in the order it is being done (sorted 2026-11-01). Each item is its own branch and commit;
when an item lands, its entry moves to [settled-decisions.md](settled-decisions.md) or is deleted here. The order
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

1. **rustfmt.** Default rustfmt, one formatting-only commit, then a /verify gate. *Decided by the maintainer.*
2. **Logging.** The `log` facade (eframe, winit and wgpu already emit through it and their output is lost today)
   with Patina's own small logger: stderr plus a per-user file (`%LOCALAPPDATA%\Patina\patina.log`,
   `$XDG_STATE_HOME/patina/patina.log`), the previous run kept as `.1`. No console window in release builds.
   **Before a release**, the log needs a more elegant way in than a file path — e.g. *Help → Open log folder*.
3. **Windows-app basics.** The title follows navigation (`● IMG_0412.jpg — 12 / 129 — Patina`, ● for unsaved
   edits). One keymap table drives both the handler and the help overlay, so they cannot drift (help omits 0–5 and
   L today); Tab goes back to egui focus, G cycles guides Off → Thirds → Golden.
4. **Theme foundation.** Pin egui's dark base theme instead of following the OS. One green-tinted token set in
   `ui::tok` with every text colour at WCAG AA (today's faint grey `#5E5E63` is 2.6:1), checked by a test. One
   button component instead of three; Save is primary only with unsaved edits. Three-step type scale in Inter
   (bundled, *decided*), monospace numbers, icons rasterised at their drawn size.

| Token | Hex | Use |
|---|---|---|
| CANVAS | `#0D0F0E` | behind the image; HUD and toast backing |
| SURFACE_SUNKEN | `#141716` | text fields, slider tracks |
| SURFACE | `#1A1D1C` | rail, dialogs |
| RAISED | `#232826` | secondary buttons, chips |
| RAISED_HOVER | `#2B312E` | hover fill |
| BORDER | `#2A302D` | hairlines |
| BORDER_STRONG | `#3B423F` | outlines, disabled glyphs |
| TEXT_1 / TEXT_2 / TEXT_3 | `#E6EAE8` / `#9AA39F` / `#8A938F` | 14.0 / 6.6 / 5.4 : 1 on SURFACE |
| ACCENT (+ HOVER, PRESS) | `#9DBFB0` (`#B2D3C4`, `#87A99A`) | primary fill, modified dots, slider fill |
| ACCENT_SOFT / ACCENT_LINE / ON_ACCENT | `#1F2E28` / `#4F6E61` / `#12201A` | selected fill, focus ring, text on accent |
| DANGER (+ SOFT, LINE) | `#E0654A` (`#35221E`, `#5A3028`) | destructive actions, errors |
| STAR | `#E2C062` | rating stars |
| Labels | `#D7604F` `#D9B95A` `#6DB487` `#6A98CF` `#A178C8` | colour-label dots, ~15% desaturated |

## The editor

5. **Rail rebuilt around the edit**, fitting 1200 px without scrolling:
   - one 44 px toolbar — Undo, Redo, Before/After left; Save As and Save right; Reset all under Save's ▾;
   - all eight sliders always open under Light / Color / Detail subheads (no collapsing);
   - presets as text chips that preview on the canvas on hover and write their offsets into the sliders as one
     labelled undo step (the twelve identical placeholder tiles, `preset_thumbs.rs` and `ThumbCache` go);
   - geometry as one row of four icon buttons;
   - Scale leaves the pipeline; output size is Save As's *Long edge*;
   - EXIF leaves the rail for a viewer Info card (I);
   - a painter-drawn slider: fill from zero, typed values, double-click reset, Shift for fine, **one undo step
     per drag** (a drag fills the whole 50-step history today);
   - section dots appear only when a group is modified, and reset it on click.

## The viewer

6. **Where am I, what happened.** A 30 px HUD that fades in on navigation or mouse move (name, `12 / 129`,
   size, stars, label) and out after 1.5 s; one toast lane for real outcomes ("Saved — rotation stored in EXIF",
   "Moved to the Recycle Bin", "Label: Green"); a loading bar after 150 ms; a bad file shows a card instead of a
   modal, and browsing continues; an empty state with *Drop an image here* and Ctrl+O.

## Judging an edit

7. Hold Backslash for the original. Viewer zoom with a pixel-true 100%. A small histogram in the rail head.

## Working through a set

8. Arrows, 0–5 and L in the editor too. Copy and paste edits between images. Direct label keys 6–9.

## Open decisions, after the UI

9. **Preload cache** — said to grow without bound; measure the working set before changing it.
10. **Sidecar name** — read both `photo.xmp` and `photo.jpg.xmp`, write to whichever exists.
11. **Orientation everywhere** — the first window's size, and RAW files.
12. **Icon in the executable** — `build.rs` with `winres`, which is already a build-dependency.
13. **Sort order like Explorer** — needs a safe crate or a reasoned `unsafe` exception.
14. **WebP quality** — the encoder is lossless only; lossy needs a crate or the option goes.
15. **Metadata a save still loses** — embedded XMP, RAW/JPEG XL sources, a TIFF's ICC; GPS in the Web preset.

## Considered and dropped

Live preset thumbnails (twelve renders per image for a 108×81 preview the canvas shows better); a filmstrip
(ends the zero-chrome viewer and needs the whole folder read); cull-mode delete changes, two-up compare and
batch export (features, not UI refinements — revisit after 6–8 exist); slideshow polish.

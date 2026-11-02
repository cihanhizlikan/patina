# Patina — Settled Decisions

What has already been decided and must not be re-argued. **A decision is re-opened by a new fact, never by an
argument.** Each entry says what was decided, why, and where the evidence is. Where the original reason was never
recorded, the entry says so instead of inventing one — the maintainer fills it in.

Patina began as a copy of Devoid Viewer (the first commit ships `devoid.ico`, renamed in `eb31df4`, and
`.gitignore` still says *"Added by Devoid Viewer build process"*). Decisions inherited from it are marked so.

## Rendering is GPU: eframe + wgpu, zero-copy textures
The decoded image is uploaded to a `wgpu::Texture` and registered with egui (`src/view.rs`), so egui never holds
a copy of the pixels. At Fit, downscaling for display is Lanczos on the CPU (`lanczos_if_needed`); zoomed, the
texture is full resolution with a box-filtered mip chain (2026-11-02, below). This is the opposite of Devoid Viewer's CPU blit, and deliberately so: Patina is an editor
with live adjustments, not a minimal viewer. **The reason for wgpu over egui's glow backend was not recorded** —
maintainer to fill in.

## Cross-platform, with no `#[cfg(windows)]` code
The Win32 hooks of the winit build were removed; eframe handles the window on every platform and `rfd` provides the
file dialogs (`src/main.rs` module doc). The installer (`patina_setup.iss`) is Windows-only. Keep platform
behaviour inside cross-platform crates rather than adding per-OS branches.

## Editing is non-destructive and recomputed from the original
`original` is immutable; `display` is recomputed from scratch on every parameter change. `EditParams` is a plain
value, so undo is a `Vec<EditParams>` with no pixel copies. The operation order is fixed: geometry, then tone,
then colour, then preset style, then sharpening (`src/editor.rs` module doc). Animated GIFs get the same parameters
on every frame.

## Images are shown by their EXIF orientation (2026-11-01)
`decode_raster` applies the Orientation tag, as cameras, browsers, Explorer and Lightroom do. Before, a portrait
phone photo (stored landscape with Orientation 6) opened sideways.

## A JPEG rotation is saved as its EXIF orientation, not re-encoded (2026-11-01)
A save whose only edits are rotate and flip changes the Orientation tag and nothing else (`src/orientation.rs`).
It replaced a decode → rotate → re-encode at quality 95 that was logged as *"lossless"* yet lost quality on every
save and dropped the whole EXIF block. **Why not a real lossless DCT transform** (what `jpegtran` does): the
`mozjpeg` crate's safe API exposes no coefficients, and every crate that does is FFI to libjpeg, which needs
`unsafe` or a C toolchain beyond the one we have; the tag gets the same picture with zero change to the image
data. **The trade-off**: software that ignores EXIF orientation shows the old orientation. Everything current
honours it. The new value is found by trying the eight orientations against `editor::apply_rotation_flip_raw`,
so there is one definition of rotation direction, not a second table to keep in step. `mozjpeg` was dropped.

## A re-encoded save keeps the source's EXIF and ICC profile (2026-11-01)
Every save that is not a pure JPEG rotation re-encodes pixels (`src/save.rs`). It used to write pixels only, so
one Ctrl+S after an exposure tweak erased camera, date, exposure and GPS, and the ICC profile — and with it the
colours of any image not in sRGB, since Patina does not colour-manage and the pixels stay in the source's space.
Now the source's EXIF and ICC go into the new file, through `image`'s own `icc_profile`/`exif_metadata` and
`set_icc_profile`/`set_exif_metadata`: no new dependency. JPEG, PNG and WebP take both, TIFF the profile, BMP
neither. **The EXIF is changed in two places**, because the loader already turned the pixels upright:
Orientation becomes 1, or every viewer would turn the image a second time; and IFD1 is cut, because its thumbnail
still shows the image before the edit, which Explorer and camera apps display. An EXIF block that cannot be
parsed for that is dropped, not copied. Save As takes the metadata of the file that was open, including for an
export resized for the web. Saves now go through a temporary file and a rename, as sidecars do, so a failed
encode no longer leaves a truncated original.

## Delete moves the file to the Recycle Bin (2026-11-01)
It was `std::fs::remove_file` — permanent, no undo — with the error discarded, under a dialog that said *"This
cannot be undone"*. It now goes through the `trash` crate, as in Devoid Viewer: a safe API over the shell's
`IFileOperation` with `FOF_ALLOWUNDO` (and the macOS and freedesktop trashes), so no `unsafe` of ours; where a
volume has no Recycle Bin, Windows asks before destroying the file. A failure reaches a dialog and the file stays
in the list. `trash` unwraps a failed COM instantiation internally; unlike Devoid, Patina does not abort on
panic, so `move_to_trash` catches it — otherwise the delete thread would die silently and `deleting` would stay
true, blocking every later delete.

## The keyboard belongs to whatever is open; shortcuts match exactly (2026-11-01)
`process_keyboard` stands down while a dialog, Save As, the help overlay or a text field is open, and the open
thing handles its own keys: in a dialog **Esc is always the way out that changes nothing** and Enter takes the
first button (Save, Delete, Rename, OK; Delete is recoverable, so Enter may trash, as in Explorer). A Ctrl
shortcut matches its modifiers exactly (`consume_shortcut`), because egui's `consume_key` ignores an extra Shift.
Each rule closed a measured data-loss path: Ctrl+Shift+S saved over the original, Ctrl+Shift+Z undid, Esc in
*Save changes?* discarded the edits, a letter typed into the Save As filename ran its shortcut (Q started a quit),
and the arrow keys moved the view under an open delete dialog. **The delete dialog holds the path it names**, and
save-and-quit closes only when the save succeeded. `keyboard_tests` in `src/main.rs` drive real key events through
an egui context and check the files on disk; each fails with its fix reverted.

**Every shortcut is one row of `KEYMAP`** (`src/keymap.rs`, 2026-11-01): the handler dispatches from it and the
help overlay is drawn from it, key labels included, so the two cannot disagree — the hand-written help had left
out 0–5 and L. A test fails if one key does two things in one mode. **Tab is not a shortcut**: it moves keyboard
focus, as on the rest of the desktop; G alone steps the guides Off → Thirds → Golden.

## Nothing is replaced or dropped without asking (2026-11-01)
Three paths did it silently. **Closing the window** (close button, Alt+F4) with unsaved edits now cancels the
close and asks *Save changes?*, like Q; Discard on that question closes, where it used to only leave the editor.
**Save As onto an existing file** shows *"b.jpg already exists. Replace it?"* and a Replace button; only a second
confirm of the same path replaces it, and changing the folder, name or format asks again. **The wrong-extension
rename** refuses a target that exists (`rename_no_replace`), because `std::fs::rename` replaces on Windows and the
offered name, free when the dialog opened, can be taken before Rename is clicked; a failed rename now reaches a
dialog instead of being dropped. The rename's check-then-rename leaves a gap of two system calls: std's only atomic
no-replace is a hard link, which exFAT memory cards lack.

## Ratings and colour labels live in XMP sidecars
Not written into the image file: `<image>.xmp` next to it, holding `xmp:Rating` (0–5) and `xmp:Label`
(`src/xmp.rs`).

## A sidecar Patina did not write is edited, never replaced (2026-11-01)
A sidecar from darktable (same `photo.jpg.xmp` name) or another tool holds edit history, develop settings and
keywords. `write_labels` changes only the `xmp:Rating` and `xmp:Label` values in place, adds them to the first
`rdf:Description` when absent, and keeps every other byte. Clearing the labels deletes the sidecar only when it
is exactly the packet Patina writes. A file it cannot edit is left alone and the error reaches a dialog. Writes
go through a temporary file and a rename, so a crash cannot truncate a sidecar. **No XML parser**: a
read-modify-write through a DOM would reformat the whole file, which is the damage this rule exists to prevent;
on-disk tests in `src/xmp.rs` hold the byte-for-byte guarantee.

## No `unsafe`, build-enforced (2026-11-01)
`unsafe_code = "forbid"` in `Cargo.toml`: no `#[allow]` can re-open it. The one `unsafe` block there was cast
`&self.icon_cache` to a raw pointer so `draw_modal` could take `&mut App` beside it — a shared reference aliasing a
live `&mut`, undefined behaviour whatever the callee did. `draw_modal` now only reads the dialog and icons and
returns the clicked `ModalAction`; `draw_ui` applies it. Borrow-checker friction is answered by restructuring.
Should Windows ever force an FFI call (as `StrCmpLogicalW` did for Devoid Viewer), it goes in one module, `forbid`
becomes `deny` with that module's `allow`, and this entry is revised in the same commit.

## Default rustfmt, gated (2026-11-01)
Decided by the maintainer before the UI rework, which rewrites most of `ui.rs` and `main.rs`: one formatting-only
commit first, so later diffs show only real changes. The hand-aligned columns went with it. `/verify` fails on a
`cargo fmt --check` diff. The formatting commit is listed in `.git-blame-ignore-revs`
(`git config blame.ignoreRevsFile .git-blame-ignore-revs`). Same choice as Devoid Viewer.

## Logging goes through `log` to a per-user file; release builds have no console (2026-11-01)
Every launch on Windows opened a console beside the window, because it was the only place the `eprintln!` log
went. The maintainer chose to hide it and keep the log, with Linux in view. Patina now logs through the `log`
facade — the Rust standard, and what eframe, winit and wgpu already emit through: before, nothing installed a
logger, so their warnings, GPU errors among them, were dropped. The logger is ours (`src/logging.rs`, about 60
lines), not `env_logger` or `tracing`: it needs no configuration, filters (Patina from Info, dependencies from
Warn) and two sinks, which a framework would add code for and not save any. `log` was already in `Cargo.lock`.
Lines go to stderr and to `%LOCALAPPDATA%\Patina\patina.log`, or `$XDG_STATE_HOME/patina/` else
`~/.local/state/patina/` (the XDG home for logs). The previous run is kept as `patina.log.1`, so a crash's log
survives one relaunch and the file never grows across runs. A panic is logged before the default hook runs.
`#![windows_subsystem = "windows"]` applies to release builds only; debug builds and `cargo run` keep the console.
**Not a release yet**: a user cannot find the file without being told the path (docs/roadmap.md).

## One palette, measured; one button; Inter (2026-11-01)
**Colours** live only in `ui::tok` (`src/ui.rs`): green-tinted neutrals, and an accent that marks meaning —
changed, the default action, selected, focused — never decoration. Every text colour reaches WCAG AA (4.5:1) on
each surface it is drawn on, and `tok::tests` computes it from the hex values; the old faint grey measured 2.6:1.
The test overruled the review's own table twice: its DANGER `#E0654A` is 4.4:1 on a raised button, so DANGER is
`#E67458`; its disabled glyph (BORDER_STRONG) was 1.4:1 and vanished, so disabled is `TEXT_DISABLED`, about 3:1
— exempt from AA, but visible. Tokens are opaque except three drawn over the photo (scrim, guides, label ring).
**egui's theme is pinned dark** (`follow_system_theme: false`): following a light OS gave egui's own text fields
and checkboxes light defaults inside dark dialogs.
**One button** (`ui::Button`, kinds Primary / Secondary / Danger) replaces five rail variants, the dialogs' painter
and the Save As chips. It draws a focus ring, since Tab now moves focus; Enter on a focused button is a click, and
a click wins over the dialog's Enter-means-first-button (`enter_on_a_focused_cancel_cancels_the_delete`). **Save is
primary only with unsaved edits.**
**Inter** 4.1 Regular and SemiBold, from the rsms/inter release (SIL OFL, licence beside them in
`src/assets/fonts/`), +857 KB on the exe; egui's fonts stay behind it for missing glyphs. egui has no weights, so
SemiBold is its own family for titles. Three type steps; slider values are monospace so they do not jitter.
**Icons are rasterised at the pixel size they are drawn**, re-rasterised when the window height or display scale
changes it; scaling the SVGs' 24 px down to 12–17 px blurred them.

## The editor rail: one toolbar, open groups, one undo step per drag (2026-11-01)
**Layout** (`ui::draw_panel`): one toolbar — Undo and Redo left; Save As, Save and a ▾ holding *Reset all edits*
right — then preset chips, a row of four geometry buttons, and the sliders always open under Light / Color /
Detail. A group's head shows a dot only when the group is modified; clicking it resets that group.
**Presets stay a pipeline stage**, not offsets written into the sliders: five presets set style flags (B&W, matte,
glow), Auto Enhance depends on the image's pixels, and contrast composes as `c*0.5+k`, so none of them is a slider
position. A chip previews on the canvas while hovered and commits on click; clicking the chosen one clears it.
**One drag is one undo step.** The slider reports its first change as a step and every later frame of the drag as
continuing it (`ui::Change`), and the app replaces the current undo entry instead of pushing
(`one_undo_takes_back_a_whole_drag`). Before this a drag filled the 50-step history. Arrow keys on a focused
slider step once per press; a key repeat continues the step.
**Scale left the pipeline.** Output size is Save As's *Long edge*; a scaled preview could not show 100% pixels.
**EXIF left the rail** for an Info card over the canvas, toggled with I in both modes.
**A save renders from the committed params** (`apply_edits` on the source at save time), never from `es.display`:
that is the last frame shown, which lags the params during the slider debounce and is the hovered preset while one
is previewed (`a_save_writes_the_committed_edit_not_the_last_frame`).

## The viewer says where you are and what happened, then gets out of the way (2026-11-02)
All of it is `src/hud.rs`; timing is plain functions of `Instant`, tested without a window.
**The HUD** is a 30 px strip over the top of the photo — name, `12 / 129`, pixel size, stars, label — shown on
navigation or a mouse move over the canvas, held 1.5 s, faded over 0.25 s. It **replaces the always-on rating
overlay**: a rating is still seen when it changes (HUD and toast) and on any mouse move, and the photo is otherwise
bare. A slideshow's own advance does not raise it.
**One toast lane** for outcomes the user caused: saves (and *how* — "orientation stored in EXIF, pixels untouched"
for a lossless JPEG rotation), Save As, the Recycle Bin, rename, rating, label. A new toast replaces the one shown.
**Failures to write stay modal** — a save, delete, sidecar or rename that failed needs acknowledging; a toast that
fades would let it pass unseen.
**A file that will not decode is a card, not a modal.** It becomes the current file (title, `3 / 5`, Delete and
the arrows all work on it) and its reason shows in place of the image. Before, a modal stopped browsing, and
`loading_path` was never cleared, so the file could not be tried again. A late failure for a file the user has
already moved past is dropped.
**The loading bar** shows only after 150 ms: a quicker load flashing a bar reads as a glitch.
**With nothing open** the canvas offers *Drop an image here*, Ctrl+O and Open…, and outlines itself while files are
dragged over. **The editor needs an image**: with none, E says "Open an image to edit it" rather than opening a
rail of controls with nothing to act on. What would move this: Patina being launched mainly to edit (an
"Edit with Patina" verb), where E — or launch — opening a file picker would earn its place.

## Judging an edit: the original on Backslash, a pixel-true 100%, a histogram (2026-11-02)
**Backslash shows the original while it is held**, in the editor, with an *Original* tag on the canvas; letting
go, losing focus or a dialog opening shows the edit again. The key is read **by position** (egui's
`physical_key`), not by character: on the Turkish Q layout of the maintainer's machine that key types a comma and
`\` needs AltGr. **The original keeps its own texture** (`original_tex`, the viewer's), and the edit gets a second
one: re-uploading on each flick cost 680–743 ms (`patina.log`, 3840×2400 and 6400×4000). Leaving the editor no
longer re-uploads either.
**Zoom works in both modes** and is Fit or a scale in *physical* pixels per image pixel (`src/view.rs`), so 100%
is one image pixel on one screen pixel at any display scale; the image's corner is snapped to the pixel grid, and
from 100% up the texture is sampled nearest, so float error cannot blend two pixels. Z toggles Fit ↔ 100% at the
pointer, Ctrl+scroll / Ctrl+= / Ctrl+- step through 12½–800% with Fit as a stop, Ctrl+1 is 100%, Ctrl+0 Fit; Ctrl+Space
is gone. Zooming keeps the point under the pointer still; a zoomed image is dragged and never leaves an
empty margin. **The zoom is kept across images**, so a burst is checked for focus frame after frame.
**Zoomed textures are full resolution with a mip chain** made on the CPU by 2×2 box averages (9–11 ms for 24 MP,
`mip_chain_cost`), so steps between Fit and 100% do not alias; Fit keeps its Lanczos texture. **`main` asks the GPU
for its own largest texture**: egui-wgpu asks for 8192 px, narrower than a 45 MP frame's 8256. Only an image past
the GPU's limit is shrunk to it and loses pixel-true 100%.
Two egui defaults were wrong for this and are off: **Ctrl+scroll arrives as a `Zoom` event**, not `Scroll`, so the
editor's old Ctrl+scroll zoom never ran; and **egui's keyboard zoom** scaled the whole interface on Ctrl+= / Ctrl+-.
Ctrl on `+` and `=` ignores Shift, which many layouts need to type them.
**The histogram** heads the rail: R, G and B added together (grey where they overlap), on one scale set by the
tallest level between the ends so a clipped spike does not flatten the rest. It is made on the edit thread with
each result, and shows the original's while Backslash is held. The rail with it still ends 322 px above the bottom
of a 1200 px window (`the_rail_with_its_histogram_fits_a_1200_px_window`).

## Paths are used as given — no `canonicalize` (inherited)
`std::fs::canonicalize` on Windows returns a verbatim `\\?\C:\…` path, which breaks paths handed over by
archivers. **Build-enforced**: both `canonicalize` functions are banned in `clippy.toml`, and
`disallowed_methods` is `deny`.

## The directory listing never stats a file (inherited)
`DirNavigator::rebuild` filters with `DirEntry::file_type()`, which `read_dir` already returned, instead of
`Path::is_file()`, which costs one syscall per entry. Keep any new filter on data the directory entry carries.

## The extension is checked against the magic bytes (inherited)
A file that decodes under a different format than its extension says yields `WrongExtension`, and the UI offers a
rename. RAW formats are exempt: they have no single reliable signature, and `rawler` detects them itself.

## GIF frame delays are clamped (inherited)
A zero-denominator delay becomes 100 ms and every delay is at least 20 ms, so 0-delay frames cannot busy-loop the
event loop (`src/loader.rs`).

---

# Open decisions

Found while writing the baseline (2026-11-01). Each is a candidate until measured; none is settled.

## Sidecar name: `photo.jpg.xmp` or `photo.xmp`
Patina uses the full-filename form, as darktable does. Lightroom and Capture One use `photo.xmp`, so ratings
Patina sets on a RAW are invisible to them, and theirs to Patina. Recommendation: read both, write to whichever
exists, and create `photo.jpg.xmp` only when neither does.

## What a re-encoded save still loses
Left over from carrying EXIF and ICC across (above), each measured or read in `image` 0.25.10:
- **XMP embedded in the file** (Lightroom, darktable, keywords) — `image` has no XMP encoder.
- **RAW and JPEG XL sources** give no metadata — `image` does not open them; `rawler` and `jxl-oxide` would.
- **A TIFF source's ICC profile** — `image` writes the tag but reads none back, not even its own.
- **Stale EXIF fields** — PixelXDimension/PixelYDimension keep the source's size after a resize or a 90° turn.
  Readers use the frame's real size, so this is cosmetic.

## GPS in a web export
The *Web* preset keeps the location like every other save. Lightroom and most export tools offer to strip it for
sharing. Recommendation: the Web preset removes GPS (one IFD pointer in IFD0), the others keep it.

## WebP "quality" is ignored
The `image` crate's WebP encoder is lossless only, so the *Archive (WebP 90%)* preset writes lossless WebP and the
quality field does nothing (the code says so; the UI does not).

## Window size and RAW files ignore orientation
`image_size_hint` sizes the first window from the stored dimensions, so a portrait phone photo opens in a
landscape window. RAW files go through `rawler`, whose orientation Patina does not apply.

## The icon is not embedded in the executable
`winres` is a build-dependency and the module doc mentions `build.rs`, but there is no `build.rs`. Explorer and the
installer show the default icon; only the window icon (`load_icon`) is set.


## Sort order differs from Explorer
`human_sort` lowercases and compares code points, so accented names sort after `z`. Devoid Viewer now uses
`StrCmpLogicalW` on Windows, which is FFI. Patina forbids `unsafe`, so matching Explorer means either a
maintained crate that wraps the call safely or a deliberate exception under the `unsafe` decision above.

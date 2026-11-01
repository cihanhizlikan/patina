# Patina — Settled Decisions

What has already been decided and must not be re-argued. **A decision is re-opened by a new fact, never by an
argument.** Each entry says what was decided, why, and where the evidence is. Where the original reason was never
recorded, the entry says so instead of inventing one — the maintainer fills it in.

Patina began as a copy of Devoid Viewer (the first commit ships `devoid.ico`, renamed in `eb31df4`, and
`.gitignore` still says *"Added by Devoid Viewer build process"*). Decisions inherited from it are marked so.

## Rendering is GPU: eframe + wgpu, zero-copy textures
The decoded image is uploaded to a `wgpu::Texture` and registered with egui through `register_native_texture`
(`src/main.rs` module doc), so egui never holds a copy of the pixels. Downscaling for display is Lanczos on the CPU
(`lanczos_if_needed`). This is the opposite of Devoid Viewer's CPU blit, and deliberately so: Patina is an editor
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

## "Lossless" JPEG save is a re-encode at quality 95
`lossless_jpeg_transform` decodes, rotates the pixels and re-encodes at a fixed quality 95, overwriting the file in
place. Its own comment admits this; the log line still says *"lossless JPEG transform applied"*. It also drops the
EXIF block (camera, date, GPS, orientation). Recommendation: either a real DCT-domain transform or an honest
name, and carry the metadata across either way.

## Delete is permanent
`execute_delete` calls `std::fs::remove_file`: no Recycle Bin, no undo, and errors are discarded. Devoid Viewer
moved to the `trash` crate for the same reason (Devoid's settled decision of 2026-10-31).

## `unsafe` that silences the borrow checker
`App::draw_ui` turns `&self.icon_cache` into a raw pointer and dereferences it while `draw_modal` also receives
`&mut App` — a shared reference aliasing a live `&mut`, which is undefined behaviour whatever `draw_modal` does.
Recommendation: restructure so no `unsafe` is needed, then set `unsafe_code = "forbid"`.

## The Windows build opens a console window
There is no `#![windows_subsystem = "windows"]`, so every launch on Windows shows a console beside the window,
which is where the `log!` macro's `eprintln!` lines go. **Whether the console is wanted was not recorded.**

## The icon is not embedded in the executable
`winres` is a build-dependency and the module doc mentions `build.rs`, but there is no `build.rs`. Explorer and the
installer show the default icon; only the window icon (`load_icon`) is set.

## Formatting: hand-aligned columns or rustfmt
The code is aligned by hand in columns. Default rustfmt would rewrite most files. Devoid Viewer adopted default
rustfmt and gated it in CI.

## Sort order differs from Explorer
`human_sort` lowercases and compares code points, so accented names sort after `z`. Devoid Viewer now uses
`StrCmpLogicalW` on Windows; Patina has no `unsafe` module to put that call in, so this one waits on the
`unsafe` decision above.

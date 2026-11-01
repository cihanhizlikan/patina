# CLAUDE.md — Patina

A fast desktop image viewer and editor in Rust: eframe/egui on wgpu for display, `image` + `jxl-oxide` + `rawler`
for decoding, a non-destructive editing pipeline on `rayon`. It began as a copy of Devoid Viewer and kept its
loader, navigator and path rules.

## Layout
- `src/main.rs` — the `App`: window and viewport state, background decode, preload and metadata threads, directory
  watcher, texture upload, canvas, modals (save-as, delete, rename, error, help), save and export.
- `src/editor.rs` — `EditParams`, the undo stack and `apply_edits`, the fixed-order pixel pipeline.
- `src/ui.rs` — the editor side panel: design tokens, icons, preset tiles, sliders.
- `src/loader.rs` — decoding to straight RGBA8, magic-byte sniffing, wrong-extension recovery, RAW.
- `src/nav.rs` — the sibling-file list for one directory, natural sort, wraparound navigation.
- `src/exif.rs` — EXIF fields for the metadata panel. `src/xmp.rs` — rating and colour-label sidecars.
- `src/preset_thumbs.rs`, `src/assets/` — embedded SVGs. `patina_setup.iss` — Inno Setup installer.

## Critical rules
- **Read [docs/settled-decisions.md](docs/settled-decisions.md) before proposing a change to rendering, paths,
  formats, directory listing, the edit pipeline or sidecars.** A decision there is re-opened by a new fact, never
  by an argument. Where a change genuinely moves one, revise the entry in the same commit. Its *Open decisions*
  section is the work queue.
- **Never work on `main`.** Branch off its tip (`feature/<slug>`, `fix/<slug>`, `chore/<slug>`); merging and
  pushing are the maintainer's.
- **Verify with the `/verify` skill before calling anything green.** Report its numbers, not an impression.
- **After every commit, rebuild from clean**: `cargo clean`, then `cargo build --release --locked`, and report the
  exit code and `target\release\Patina.exe` size. The exe on disk is then always a fresh build of the latest
  commit, never a leftover. (`Cargo.toml` and `Cargo.lock` are source, not build output; they are never deleted.)
- **Patina writes to the user's files** — saves in place, sidecars, renames, deletes. A change on any of those paths
  needs a test that shows what is on disk afterwards, not just that the call returned `Ok`.
- **Documentation lives in this repository as Markdown**, changed in the same commit as the code it describes.

## Building
- **Needs the MSVC linker**: Visual Studio's *Desktop development with C++* workload (or the standalone Build
  Tools). Without it every build, `clippy` and `test` fails with `link.exe not found`.
- **Run cargo from PowerShell, not Git Bash.** Git Bash puts coreutils' `/usr/bin/link` on `PATH`, rustc picks it
  up as the linker, and the build fails with `link: extra operand`.
- `cargo` lives in `~/.cargo/bin`, which may not be on the tool shell's `PATH` — prepend it.
- The toolchain is pinned in `rust-toolchain.toml`; architecture bans live in `clippy.toml` at `deny`.

## Working style
Carried over from the maintainer's other projects:
- **Safe Rust.** Maintainer: *"If we're writing unmanaged code why are we writing code in Rust in the first
  place."* Before writing FFI, look for a maintained crate with a safe API. `unsafe` written to get past the borrow
  checker is a design problem to restructure, never a fix.
- **Evidence, not reasoning.** A fix is finished when it is measured, not when the code looks right. A hypothesis
  built from reading code is a candidate, never a conclusion. A measured "nothing to do" is a deliverable.
- **Take a position and argue it.** An open question wants a recommendation and what would change it, not a menu.
- **Quality outranks compatibility.** No shims or legacy fallbacks; fix the shape, not just the symptom.
- **Justify every new dependency unprompted** — its maintenance status and the first-party alternative, and its
  cost in binary size and startup time.
- **Correct a misused technical term** in one clause, then answer the actual question.
- **Hand over decisions, not inventories.** Triage a lint loop before reporting it; lead with what survives.
- **Commit bodies are the archive**: what changed, why it was not the obvious thing, and the evidence. Search them
  with `git log --grep=<term>`.

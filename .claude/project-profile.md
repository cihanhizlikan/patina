# Project profile — Patina

**What this file is.** The four workflow skills — `/sweep`, `/wp-start`, `/wp-verify`, `/wp-finish` — are Patina's
Rust port of the ones in the maintainer's C# repository, Yalbuz (2026-11-08). Everything they need to know that is
true of THIS repository lives here, as named keys, and the skills read it by key (`profile:<key>`). **A Rust
repository adopting them copies the skills and writes its own profile**; a C# one takes Yalbuz's, whose detectors
are .NET's.

**An absent key is not a default.** A step whose key is missing, `none`, or names something that does not exist
reports **UNAVAILABLE (profile: `<key>`)** and does nothing else. It never reports "measured clean": a step that
read nothing has measured nothing.

**Values are facts about this repository, so they decay like any other.** The sweep's context-drift step checks
that every path below still resolves.

## The crate

| Key | Value |
|---|---|
| `crate` | one binary crate, `Cargo.toml` (`patina`, exe `Patina.exe`), plus `build.rs` |
| `production_sources` | `src/*.rs` outside `#[cfg(test)]` modules; `build.rs`; `src/assets/` (reached only through `include_bytes!`/`include_str!` paths) |
| `test_sources` | the `#[cfg(test)] mod tests` (and `keyboard_tests`) inside each `src/*.rs`; there is no `tests/` directory |
| `fixture_excludes` | none |
| `host_types` | a desktop eframe/egui app on wgpu; background work on `std::thread::spawn` threads that report over `crossbeam_channel`; a panic hook installed by `logging::init` that logs the panic before the default hook |
| `co_located_products` | none |

## Conventions the build or suite already enforces

| Key | Value |
|---|---|
| `enforced_guards` | path bans (`clippy.toml` `disallowed_methods`, `deny` in `Cargo.toml`); `unsafe_code = "deny"`, one allowed module (`src/explorer_order.rs`); rustfmt (`cargo fmt --check`, a gate); zero clippy warnings (`/wp-verify`) |
| `dead_code_lint` | rustc `dead_code`, warn by default, and in this binary crate **it sees every item, `pub` ones included, even one only a test calls** — because every module is private (`mod save;`), its `pub` items are crate-private. Measured 2026-11-08 on a scratch binary crate: a `pub fn` in a private module called only from a test is warned; only a `pub` item at the CRATE ROOT is not, and `src/main.rs` has none. So what it cannot see is what is switched off: the 9 `#[allow(dead_code)]` sites, the other platform's `#[cfg]` half, and assets |
| `lint_config` | `Cargo.toml` `[lints.rust]` / `[lints.clippy]`, `clippy.toml`; clippy's default groups only, so every `pedantic`, `nursery` and `restriction` lint is OFF until a probe proves it (`cargo clippy --all-targets -- -W clippy::<lint>`) |
| `toolchain` | pinned, `rust-toolchain.toml` (1.96.0 with clippy and rustfmt) |
| `line_endings` | `core.autocrlf=true`, no `.gitattributes`: LF in the index, CRLF in the checkout. `sed -i`/`perl -pi` leave a touched file LF in the checkout and git normalises it on `add` (the "LF will be replaced by CRLF" warning is that, not drift). Nothing enforces it |

## Documents the steps read

| Key | Value |
|---|---|
| `context_docs` | `CLAUDE.md`, `.claude/skills/*/SKILL.md`, this file, `docs/settled-decisions.md`, `docs/roadmap.md` |
| `structure_doc` | `CLAUDE.md`, section *Layout* — one line per module saying what it owns |
| `naming_rule` | none |
| `reviewer_carried_rules` | `CLAUDE.md`, section *Working style*: safe Rust, evidence, every new dependency justified, quality over compatibility |
| `settled_decisions` | `docs/settled-decisions.md`; its *Open decisions* part is not settled |
| `config_behaviours` | none — Patina has no configuration file; its tunables are `const`s in the source |
| `config_coverage_test` | none |
| `effective_config` | none |

## The plan

| Key | Value |
|---|---|
| `plan_rule` | **the plan is in the repository**: `docs/roadmap.md`, the work queue in order. A handed-over package is a row in it, committed on the step's branch, so it merges with the step — there is no plan file outside the tree and no plan-mode round trip |
| `plan_known_path` | `docs/roadmap.md` |
| `history_paths` | none |
| `offsite_register` | none — a check only the maintainer can make (a real-app look, an Explorer cache) is a roadmap row saying so |

## Verification

| Key | Value |
|---|---|
| `shell` | **PowerShell for every cargo command**, never Git Bash (its `/usr/bin/link` shadows the MSVC linker), with `$env:PATH = "$HOME\.cargo\bin;$env:PATH"` |
| `secrets_file` | none |
| `baseline_ignored` | **3** — `a_file_goes_to_the_recycle_bin` and `deletes_the_named_file_on_disk` (`src/main.rs`, they write to the user's Recycle Bin) and `mip_chain_cost` (`src/view.rs`, a timing probe) |
| `warning_baseline` | **0** clippy warnings, any target |
| `coverage_command` | none — `cargo-llvm-cov` is not installed (2026-11-08); never installed by an unattended run |
| `mutation_command` | none — `cargo-mutants` is not installed (2026-11-08) |
| `audit_command` | `cargo deny check advisories` — cargo-deny 0.20.2, no `deny.toml`, so defaults; fetches the RustSec database, so it needs the network. LIVE 2026-11-08 and failing: unmaintained, unsound and vulnerable advisories in transitive dependencies |
| `update_command` | `cargo update --dry-run` — semver-compatible updates only, nothing installed; LIVE 2026-11-08 |
| `duplicates_command` | `cargo tree -d` — the same crate at two versions (`image` 0.24 under `rawler` beside 0.25 is known and deliberate) |
| `clean_rebuild` | after every commit: `cargo clean`, then `cargo build --release --locked`; report the exit code and the size of `target\release\Patina.exe` |
| `push_policy` | a session pushes `main` only when the maintainer's request says so |

## The surfaces people read

| Key | Value |
|---|---|
| `user_docs` | `README.md`; the help overlay's words (`what` in `src/keymap.rs`'s `KEYMAP` and `MOUSE_HELP`); dialog, toast and HUD strings in `src/main.rs` and `src/hud.rs` |
| `cultures` | `en` only |
| `wording_rules` | none |
| `ui_register` | none — the nearest are `ui::tok` in `src/ui.rs` and the *One palette* entry in `docs/settled-decisions.md`, which are a token set and a decision, not a register of read/build rows |
| `run_command` | `target\release\Patina.exe <image>` — the RELEASE exe: computer-use access to the debug exe was declined by the maintainer (2026-11-03) |
| `render_harnesses` | in-process egui frames in the tests, which return painted text and rects rather than files: `painted_text` and `help_at` in `src/main.rs` |
| `theme_rules` | colours only from `ui::tok` (`src/ui.rs`) |

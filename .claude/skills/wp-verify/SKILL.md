---
name: wp-verify
description: Verify a work package before reporting it — build, clippy warning count, the test suite with failures named, and the formatter, with this machine's known traps handled. Invoke before claiming any change is green. (Build and lint verification; the bundled /verify, which drives the running app, is a different skill.)
---

# Verifying a work package

**Every step is written the way it is because the obvious way measured nothing.** Report one line at the end:
build exit, clippy warnings/errors, tests passed/failed/ignored, fmt exit. Report the numbers, not an impression.
Values marked `profile:<key>` are in `.claude/project-profile.md`.

There is no CI yet. When `.github/workflows/ci.yml` lands it runs this same recipe, and the two change together.

## 0. Shell and toolchain

Run every cargo command through **PowerShell**, never Git Bash (`profile:shell`): Git Bash's `/usr/bin/link`
shadows the MSVC linker and the build dies with `link: extra operand`. Prepend cargo to `PATH` in each call:

```powershell
$env:PATH = "$HOME\.cargo\bin;$env:PATH"
```

`link.exe not found` means the Visual Studio C++ build tools are missing. That is a blocker to report, not a step
to skip — nothing after it measures anything.

## 1. Build, into a file

```powershell
cargo build --all-targets --message-format=short *> "$env:TEMP\pt-build.txt"; "exit=$LASTEXITCODE"
```

**The exit code is the primary signal; read it first.** A non-zero exit has failed however the output looks, and
a failed `build.rs` (the resource compiler) stops the crate before a single `.rs` diagnostic: its silence is not
a clean result.

## 2. Clippy

```powershell
cargo clippy --all-targets --message-format=short *> "$env:TEMP\pt-clippy.txt"; "exit=$LASTEXITCODE"
(Select-String -Path "$env:TEMP\pt-clippy.txt" -Pattern ': warning: ').Count
(Select-String -Path "$env:TEMP\pt-clippy.txt" -Pattern ': error').Count
```

Match `: warning: ` with the leading colon: in short format every diagnostic reads `src\main.rs:12:5: warning: …`,
while cargo's own summary line (`warning: \`patina\` (bin) generated N warnings`) has no colon before it and must
not be counted. Cargo replays cached warnings for unchanged crates, so a no-op rebuild still reports them.

A `disallowed_methods` hit is an **error** by design — see `clippy.toml` and `docs/settled-decisions.md`.
**Zero warnings is the bar** (`profile:warning_baseline`, reached 2026-11-03): any warning is this package's to
explain, fixed in the same commit or allowed at the site with a comment saying why. A remembered non-zero baseline
makes this a comparison against a number in a document; zero is checkable.

## 3. Tests, into a file, failures by NAME

```powershell
cargo test *> "$env:TEMP\pt-test.txt"; "exit=$LASTEXITCODE"
Select-String -Path "$env:TEMP\pt-test.txt" -Pattern '^test result:'
Select-String -Path "$env:TEMP\pt-test.txt" -Pattern '^test .* FAILED$|^---- .* stdout ----'
```

Sum the `test result:` lines — there is one per test binary. **`profile:baseline_ignored` is 3**, named in the
profile; a fourth ignored test is a finding.

**Capture to a file and grep the failure lines; never pipe the run straight to a tail.** The summary says how many
failed and never which, and an intermittent failure you cannot name is one you cannot fix. **Record the failing
test's name before re-running anything**, then run it alone, several times: `cargo test --bin Patina <name>`.

## 4. Formatter (a gate)

```powershell
cargo fmt --check *> "$env:TEMP\pt-fmt.txt"; "exit=$LASTEXITCODE"; (Select-String -Path "$env:TEMP\pt-fmt.txt" -Pattern '^Diff in').Count
```

Default rustfmt, adopted 2026-11-01 (`docs/settled-decisions.md`). A non-zero exit fails verification: run
`cargo fmt` and include the result in the same commit.

## Experiments

**Never `git checkout` or `git restore` a file to undo an experiment** — it discards every uncommitted change in
it, not just the experiment. To show a test fails without a fix (revert-confirm-red), save the file's text, make
the one change, run the one test, and write the saved text back; then `git diff --stat` must show only the
package's own changes.

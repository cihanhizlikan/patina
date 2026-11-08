---
name: wp-finish
description: Close a work package — write its archive entry into the commit body, check the size cap BEFORE committing, commit, rebuild from clean, and merge only on the maintainer's word. Invoke when a package's work is done and /wp-verify is green.
---

# Closing a work package

**A package is not finished when its tests pass.** Run `/wp-verify` first; this is what happens after it is green.
Values marked `profile:<key>` are in `.claude/project-profile.md`.

## 1. Draft the message in a file

The commit body **is** the archive entry — there is no archive file. Subject with a Conventional-Commits tag
(`feat`, `fix`, `refactor`, `test`, `docs`, `chore`), blank line, then:

- **what changed**
- **why it was not the obvious thing** — the alternative considered and the reason it lost
- **the evidence** — the numbers `/wp-verify` produced, and any revert-confirm-red

`git log --grep=<term>` matches subject and body, which is how this is searched later.

```powershell
$msg = @'
<subject>

<body>
'@
Set-Content -Encoding utf8NoBOM "$env:TEMP\patina-msg.txt" $msg
```

## 2. Measure the body BEFORE committing

```powershell
$t = (Get-Content "$env:TEMP\patina-msg.txt" | Select-Object -Skip 2 | Where-Object { $_ -notmatch '^Co-Authored-By' }) -join "`n"
[Text.Encoding]::UTF8.GetByteCount($t)    # must be <= ~1500
```

**Draft to about 1,200 bytes.** The cap (~1,500 bytes, ~25 lines) is Yalbuz's, adopted 2026-11-08: what an entry
owes is **findability**, not the full argument. **Over it, cut argument, never evidence** — a rule or a measured
refusal goes to `docs/settled-decisions.md`, which is read on purpose; an over-long commit body is read by nobody.
**Measure before `git commit`, not after**: committing first and amending down costs a round trip per trim.

## 3. Commit

```powershell
git add -A
git commit -F "$env:TEMP\patina-msg.txt"
git show --stat --format="" HEAD
```

End the message with the attribution line the session's system prompt specifies, verbatim — **never a model name
written from memory or copied from an older commit**.

**`docs/roadmap.md` and `docs/settled-decisions.md` change in the same commit as the work**, never in a separate
`docs:` commit after it. A `docs:` commit is for a documentation change that IS the work.

## 4. Rebuild from clean

`profile:clean_rebuild` — every commit, so the exe on disk is a fresh build of the latest commit (a `/sweep` does it once, after its last merge):

```powershell
$env:PATH = "$HOME\.cargo\bin;$env:PATH"
cargo clean *> $null; cargo build --release --locked *> "$env:TEMP\pt-rel.txt"; "exit=$LASTEXITCODE"; (Get-Item target\release\Patina.exe).Length
```

Run it in the background; report the exit code and the size.

## 5. Merge only on the maintainer's word

- **Merging is the maintainer's**: fast-forward `main` to the branch and delete it only when the request said so
  (*"merge & delete"*). `git status --porcelain` empty first.
- **A merge instruction is per-request, never standing** — a branch that becomes eligible later needs a new one.
- **Push only per `profile:push_policy`.** `git branch -a` afterwards must show no branch left behind.
- **A check only the maintainer can make** (a look at the real app, an Explorer cache) is named in the report and,
  if it will outlive the conversation, a roadmap row — not a promise in prose.

The sweep (`/sweep`) has its own exception to the first two rules, stated there.

# Skill "Source" shows an em dash for skills the session used

Status: implemented on `claude/sources-dash-inconsistency-2a9488`, awaiting manual test.

## Symptom

In the session-analysis "Skills, MCPs and tools" table, the `Source` column is
blank (`—`) for skills the session actually invoked, while unused skills in the
same table resolve to `Bundled` / `User` / `Project` / `Plugin`.

## Cause

`crates/antiburn-local/src/analysis/initial_context.rs` resolves a skill origin
from ranked transcript evidence, and falls back to a filesystem probe when no
evidence exists. Two defects break the used-skill path.

### 1. `invoked_skills` paths are scheme strings, not filesystem paths

The attachment Claude writes when a skill loads carries a `path` such as
`userSettings:discuss` or `bundled:dataviz`. The parser only understands the
`bundled:` prefix; everything else goes to `classify_claude_filesystem_path`,
which matches `/bundled-skills/`, `/.claude/plugins/`, and `/.claude/skills/`
and therefore returns `Unknown` for a scheme string.

Across a local sample of 361 transcripts, `userSettings:` appears 349 times and
`bundled:` 30 times, so the unhandled branch is the common one.

### 2. `Unknown` is recorded as evidence, and evidence blocks the probe

`record_skill_origin` stores the `Unknown` from defect 1 at
`claude_origin_rank::INVOKED_SKILLS` (rank 0, the strongest rank).
`resolve_claude_skill_origin` returns on the first evidence hit, so the stored
`Unknown` suppresses the filesystem probe that would have found
`~/.claude/skills/<name>/SKILL.md` and answered `User`. Invoking a skill
therefore makes its origin *less* accurate than leaving it unused.

### 3. Secondary: a deleted worktree blanks every skill row

`resolve_claude_skill_origin` returns `Unknown` for every skill when the
transcript's `cwd` no longer exists on disk. In the same local sample, 131 of
361 transcripts (36%) point at a git worktree that has since been removed. User
skills live under the home directory and do not depend on the session's `cwd`,
so this fallback is stricter than it needs to be.

## Fix

1. Map the `invoked_skills` scheme prefix to an origin: `bundled:` → `Bundled`,
   `userSettings:` → `User`, `projectSettings:` / `localSettings:` → `Project`,
   `plugin:` → `Plugin`. An absolute path keeps the current filesystem
   classification, and an unrecognised scheme falls through to the probe.
2. Make `record_skill_origin` ignore `SourceOrigin::Unknown`. A caller that
   cannot classify its input then leaves the slot empty, so weaker evidence and
   the filesystem probe still get their turn. This also covers the
   `PREAMBLE_PATH` caller, which can produce `Unknown` the same way.
3. When the `cwd` is absent from disk, skip only the project probe and still
   check `<home>/.claude/skills/<name>/SKILL.md`. Keep the "bare name with no
   hit is `Bundled`" inference gated on an existing `cwd`, because ruling out
   the project directory is what makes that inference sound.

## Tests

Engine (`crates/antiburn-local`):

- An invoked skill with a `userSettings:` path resolves to `User`.
- An invoked skill with an unknown scheme does not block the filesystem probe.
- A user skill resolves to `User` when the `cwd` is gone; a bare skill with no
  hit stays `Unknown` in the same run.
- Existing origin tests keep passing unchanged.

No desktop change: `skillMcpOriginLabel` already renders `null` for `unknown`,
which the table shows as `—` for the rows that are genuinely unresolvable.

## Result

All three fixes are in `crates/antiburn-local/src/analysis/initial_context.rs`.
Engine checks pass: `cargo fmt --check`, `cargo clippy --all-targets -D
warnings`, and `cargo test` (925 tests). Verified against two real local
transcripts with a throwaway test that was removed afterward:

- A session in a live worktree: `discuss`, `proto`, `new` and every other
  invoked user skill now resolve to `User` instead of blank.
- A session whose worktree is deleted: user skills resolve to `User`; bundled
  names stay `Unknown`, because the project directory cannot be ruled out.

## Known residual

A skill that is user-scoped but does not live at
`~/.claude/skills/<name>/SKILL.md` still resolves to `Bundled` while it is
unused, through the bare-name inference. Invoking it now answers `User`
correctly. Fixing the unused case needs origin evidence the skill listing does
not currently carry.

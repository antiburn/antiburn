# install.sh gets the fire banner

`curl | sh` opens with the doom-fire burn. The flames follow the word outline,
die down, and leave the orange **antiburn** wordmark. Then a live-updating
install log runs underneath. Spec comes from playground v9, signed off
2026-09-01.

## Status

| Step | State |
|---|---|
| 1. Fresh branch off origin/main | done |
| 2. Fire sim in install.sh (contour + clearance + circles) | done |
| 3. Terminal gating and fallbacks | done |
| 4. Live-updating log | done |
| 5. Verification matrix | done, except a real `curl \| sh` against a release (needs release assets; Keith tests) |
| 6. PR | open |

Follow-up, not in this change: port the same sim upgrades back to the
tuning harness in antiburn_assets/fire-term, so the two do not drift.

## The locked spec (playground v9)

| Knob | Value |
|---|---|
| glyph | circle ●, two cells per dot (square grid) |
| flame base | word contour, gap 2 cells clear of every letter dot |
| hug pad | 5 |
| frames / settle | 39 / 24 |
| delay | 95 ms |
| decay / base / gust / cap | 0.39 / 1.15 / 0.04 / 0.76 |
| halo | 0 |
| seed | random each run |
| log style | live-updating |
| placement | opening banner, before the download |

Dropped copy: "Now go burn fewer tokens." The log ends on
"antiburn lives in your menu bar, up by the clock."

## Step 1 — Fresh branch, not this worktree

This session's worktree pre-dates the open-source history rewrite and shares
no ancestor with origin/main (now at PR #326). Work starts on a new branch
`feat/fire-installer` cut from current origin/main. This plan file goes into
`docs/plans/` on that branch, and gets updated as steps land.

## Step 2 — Fire sim inside install.sh

The installer must stay one self-contained file, so the sim ships as a
function in `install.sh` (the awk program from
`antiburn_assets/fire-term/antiburn-fire-term.sh` is the base; the asset repo
copy gets the same upgrades so the two don't drift).

New sim work, all proven in the playground port:

- **Contour burner**: precompute the topmost dot per column; inject heat at
  `top - 1 - gap` instead of a flat floor. No heat pinned in the letter
  bodies, so nothing leaks up between letters.
- **Clearance margin**: precompute chebyshev distance from every cell to the
  nearest letter dot (one BFS at startup, plain awk array). Any flame cell
  with distance ≤ 2 renders empty. This is what keeps the word readable.
- **Hug mask**: existing wrap logic, pad 5.
- **Circle rendering**: each sim cell prints `● ` (glyph + space) so the dot
  grid is square. Word dots stay brand orange; flames use the existing ramp.
- Bake the spec table above in as the defaults; keep the existing flags so
  values stay tunable.

## Step 3 — Terminal gating

Existing gates in the asset script carry over: `[ -t 1 ]`, `NO_COLOR`,
`TERM=dumb`, truecolor vs 256-color, fractional-sleep fallback, cursor
hide/restore with a trap so an interrupted install never leaves a hidden
cursor.

New gate: circle mode is 88 columns wide and Terminal.app defaults to 80.

- `tput cols` ≥ 90 → circles (the chosen look)
- 44–89 columns → half-block renderer (44 cols, same sim, chunkier pixels)
- narrower, non-TTY, `NO_COLOR`, dumb, or CI → static orange wordmark banner

## Step 4 — Live-updating log

- One status line rewritten in place: `\r` + `\033[K`, braille spinner
  (`⠋⠙⠹…`), then the line settles to `● done-text`.
- Download runs in the background (`curl … & pid=$!`) while the spinner
  animates until `kill -0 $pid` fails; show percent from the growing file
  size when Content-Length is known.
- The sudo password prompt stays a plain static line. sudo owns the cursor;
  nothing animates while it waits.
- Non-TTY falls back to plain appended lines, same words.

## Step 5 — Verification before the PR

- Terminal.app at stock 80×24 → half-block fallback runs clean
- iTem/wide window → circle mode
- `NO_COLOR=1`, `TERM=dumb`, and piped (non-TTY) runs → static banner, plain log
- Ctrl-C mid-burn → cursor comes back
- `shellcheck` and `sh -n` on install.sh
- A real end-to-end `curl | sh` against the release URL

## Step 6 — PR

Small diff: install.sh plus this plan doc. DCO sign-off on every commit.
Keith uploads the PR image; the capture is a terminal recording of the burn
(I supply the file path and a suggested crop).

## Out of scope

- during-download and finale fire placements (not picked)
- any change to the app itself or the release pipeline

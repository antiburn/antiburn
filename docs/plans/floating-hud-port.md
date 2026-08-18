# Floating HUD: port plan

**Branch**: `claude/floating-hud-antiburn-port-ce91dc` (this worktree)
**Source**: the private repository, branch `claude/vigorous-margulis-ee04d5` at `787ba77a4` — the antiburn visual-prototype demo. Only the HUD comes over; everything else on that branch stays behind.
**Status**: draft, for review.

## What the HUD is

A small transparent always-on-top window showing the provider usage-limit bars
outside the menu. Collapsed it is bare LED bars that read as part of the
desktop; hovered for 250ms it expands into a panel with a label, bar, and reset
time per limit window; dragged it always draws collapsed so you can see where
you are putting it. One rule outranks all of that, learned the hard way on the
demo branch: **the window never moves on its own — the panel flips its open
direction instead.** The behaviour spec (`antiburn-hud-states.md` on the source
branch) documents the states and the five rejected positioning approaches; it
comes over with the port.

## One ledger entry before code

`docs/oss/source-allowlist.toml` is the public ledger of which private-repo
files were copied here and who said yes. The HUD is not in it, and the
ledger's default is deny — so the port starts with one new entry, approved by
the maintainer merging it. The notification window set the pattern: a
maintainer decision (D-023) plus an allowlist rule (`nudge-window-mechanics`).

**Decision (Keith, 2026-08-18): adapt from private source, do not rewrite.**
The port carries the demo branch's code over as directly as possible; the
ledger entry is what makes that honest. Concretely:

1. **A new allowlist rule** — proposed id `hud-window-mechanics`, disposition
   `adapt`, naming the source files on the demo branch and the reviewed commit
   (`787ba77a4` on `claude/vigorous-margulis-ee04d5` — an unmerged branch, so
   the rule records its own branch/commit rather than riding the manifest's
   `source_commit`).
2. **A maintainer decision id** (next free: D-026) recorded in both manifests'
   `governance_decisions` and a comment block, authorizing the port.
3. **`docs/deviations.md` entries** for the judgment calls named below.

## The parity rule

**Port 1:1 wherever the public codebase allows it.** The demo branch spent 67
commits tuning the hover/drag/flip behaviour; that tuning is the asset. A
difference from the source needs one of exactly two justifications: the public
codebase makes the source approach impossible (its data types don't exist
here), or a mechanical boundary check would fail. Style preferences and
repo-idiom migrations are not justifications — they can be follow-ups after
the port lands. The forced differences are enumerated below; anything not
listed comes over as-is, names included.

Draft rule, for the amendment:

```toml
[[rule]]
id = "hud-window-mechanics"
source_paths = [
  "desktop/src-tauri/src/overlay_window.rs",
  "desktop/src-tauri/src/commands.rs",
  "desktop/src/components/overlay/OverlayWindow.tsx",
  "desktop/src/lib/overlayWindow.ts",
  "desktop/src/lib/usageBars.ts",
  "desktop/src/components/ui/LedBar.tsx",
  "docs/plans/antiburn-hud-states.md",
]
path_kind = "file"
disposition = "adapt"
destination = "apps/desktop/src-tauri/src/overlay_window.rs, apps/desktop/src/views/OverlayWindow.tsx, apps/desktop/src/lib/overlayWindow.ts, apps/desktop/src/lib/usageBars.ts, apps/desktop/src/components/ui/LedBar.tsx, docs/hud-states.md"
provenance = "Company-authored on demo branch claude/vigorous-margulis-ee04d5 at 787ba77a4; publish under D-008's authorized-commit rule, authorized by D-026"
current_license = "Proprietary"
intended_license = "MPL-2.0"
notes = [
  "Preserve the window mechanics 1:1: always-on-top transparent frame, hover-intent expand, manual drag, flip-up-when-parked-low, background-cursor watcher, live-session LED blink.",
  "From commands.rs only the three overlay commands: open_overlay_window, set_overlay_hover_region, get_latest_session_activity.",
  "Rebind data to the public engine's live-usage snapshot and session index; the demo's probe types do not come over.",
  "Do not copy the demo branch's rename scaffolding, login bypass, seeded data, share line, analytics calls, or settings-page rework.",
  "Tests are written fresh in the public repository under Git/DCO provenance, not adapted from the earlier prototype branch.",
]
```

## What comes over, and where it lands

Names stay the source's names (parity rule): `overlay_window.rs`,
`OverlayWindow.tsx`, window label `antiburn-overlay`, command names unchanged.

| Source (demo branch) | Destination | Notes |
| --- | --- | --- |
| `overlay_window.rs` (136 lines) | `apps/desktop/src-tauri/src/overlay_window.rs` | Window builder, create-or-show `open()`, hover watcher, hover-region atomics. 1:1. |
| `commands.rs` (3 commands, ~40 lines) | `apps/desktop/src-tauri/src/commands.rs` | `open_overlay_window`, `set_overlay_hover_region`, `get_latest_session_activity` — the last rebound to the public engine's session index. |
| `OverlayWindow.tsx` (486 lines) | `apps/desktop/src/views/OverlayWindow.tsx` | The state machine: collapsed/expanded/dragging, 250ms hover intent, manual drag, flip-up inset logic, live-session blink. 1:1. |
| `overlayWindow.ts` (51 lines) | `apps/desktop/src/lib/overlayWindow.ts` | Open/hide/isVisible wrappers plus the localStorage preference, exactly as the source has it. |
| `usageBars.ts` (124 lines) | `apps/desktop/src/lib/usageBars.ts` | Same derivation shape; input types swapped to `LiveUsageSummaryPayload` — see "Data" below. The one forced rewrite. |
| `LedBar.tsx` (64 lines) | `apps/desktop/src/components/ui/LedBar.tsx` | Segmented LED dots, 1:1. Colors mapped onto antiburn's token names where the demo's tokens don't exist here. |
| `docs/plans/antiburn-hud-states.md` | `docs/hud-states.md` | The behaviour spec, updated only where this plan forces a difference. |

Wiring (small edits to existing files, ordinary Git/DCO provenance):

- `src-tauri/src/lib.rs` — `mod overlay_window;` plus command registration.
- `src-tauri/capabilities/default.json` — add `"antiburn-overlay"` to `windows`.
- `src/App.tsx` + `src/lib/route.ts` — an overlay route rendering `<OverlayWindow/>`.
- `src/lib/ipc.ts` — typed wrappers for the commands.
- Settings pane — the "Show floating HUD" toggle (placement below).
- Popover usage surface — the pop-out button that opens the HUD, as in the demo.
- Popover startup — restore the HUD when the preference is set, as in the demo.

## Data: what the bars show

The demo read its own probe types. antiburn already has the right surface:
`getLiveUsage()` returns per-provider `LiveUsageWindowPayload` rows with
`usedPercent` and `resetsAt` — exactly a HUD row. `usageBars.ts` keeps its
derivation shape but takes that payload as input, reusing
`isUsageWindowVisible` from `presentation/liveUsage.ts` so the HUD and the
popover agree about which windows are worth showing.

Two consequences worth stating:

- **The HUD inherits the live-usage opt-in.** With `liveUsageEnabled` off, the
  figures come from whatever the agent last cached, and may be stale or absent.
  The HUD renders an honest empty/stale state and the settings toggle's caption
  says the HUD shows live usage limits (linking the two preferences in copy,
  not in mechanism).
- **Refresh interval**: the demo's 60s refresh while visible, unchanged. No
  polling while hidden.

The live-session blink comes over too (parity): the first bar's LEDs blink
while the newest transcript write is under 90s old, polled every 5s while
visible. The demo's `get_latest_session_activity` command is rebound to the
public engine's session index, which already knows each session's last
activity timestamp.

## Preference: localStorage, as the source has it

The demo stores "show floating HUD" in localStorage
(`antiburn.showFloatingHud`): Settings writes it, the popover reads it at
startup to restore the HUD, the HUD's ✕ clears it. That comes over 1:1,
including the source's own documented caveat that each webview holds its own
copy so the visible-now question is asked of the window, not the store.
Migrating the flag into the persisted `AppSettings` struct (antiburn's idiom
for every other preference) is a candidate follow-up, not part of this port.
Default remains off — the HUD is opt-in.

## Judgment calls (each lands in `docs/deviations.md`)

1. **macOS-first, with a researched two-stage revisit.** The demo's non-macOS
   path is an opaque decorated window — a 176×500 grey slab, worse than
   nothing, and every behaviour in the states doc was tuned on a Mac. v1
   registers the feature on macOS only (toggle hidden elsewhere). To keep the
   later enable additive, platform gating stays confined to
   `overlay_window.rs` — the view and derivation code stay platform-neutral.
   - **Windows 11 — feasible next.** Tauri 2 supports transparent,
     undecorated, always-on-top windows there, and the cursor watcher's
     `cursor_position()` is cross-platform. The work is behavioral tuning:
     the flip-up math and default parking assume a top menu bar (the taskbar
     is at the bottom), and always-on-top over fullscreen apps has known
     rough edges (tauri#7328). Roughly a day of tuning plus real testing on
     a Windows 11 machine.
   - **Linux — deferred with named blockers.** Always-on-top silently no-ops
     on Wayland (tauri#3117, tao#1134) and window positioning is broken there
     too (tauri#14913), which kills both the manual drag and the flip-up
     window-move pairing; transparency depends on a compositing WM and has
     NVIDIA/WebKitGTK failure modes. X11 works, but desktops are migrating
     to Wayland. Revisit when Tauri gains usable Wayland support for
     always-on-top and positioning, or as an X11-only enable with the caveat
     stated.
2. **The transparent frame swallows clicks.** The frame is ~176×500 so the
   panel can expand inside it; its transparent area eats clicks meant for
   whatever is behind it. The demo tried cursor pass-through
   (`set_ignore_cursor_events`) and it left the HUD unresponsive, so the cost
   is accepted and named. Revisit: if Tauri grows a usable per-region
   pass-through.
3. **Polling while the HUD is visible.** A 100ms native cursor poll (macOS
   delivers no hover to background apps), a 5s session-liveness poll for the
   blink, a 60s data refresh. All bounded to a visible window: hidden or
   never-opened, the HUD costs nothing. This is the resource-boundary case
   the pull request names.

## Boundary compliance checklist

- No prohibited concept strings in any ported file, comments included
  (`scripts/check-boundary.mjs` list). The demo code is antiburn-branded
  already; verify rather than trust.
- MPL-2.0 header on every new file; DCO sign-off on every commit.
- No new network surface: the HUD renders IPC payloads the shell already
  serves. `tests/no-exfiltration.test.ts` must stay green.
- Tests written fresh here (no fixtures from the private repository; any
  usage figures in tests are obviously synthetic).
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`,
  `pnpm` type-check/lint/test all clean.

## Build order

1. **Ledger entry** — allowlist rule, D-026 notes in both manifests,
   deviations entries. Its own commit, first in the PR.
2. **Rust mechanics** — `overlay_window.rs`, the three commands, capability,
   route. App builds; a dev-only invocation shows an empty transparent window.
3. **Frontend** — `OverlayWindow.tsx`, `LedBar`, `usageBars.ts` against live
   usage; states doc ported. HUD renders real bars, hover/drag/flip/blink work.
4. **Wiring** — settings toggle, popover pop-out button, startup restore,
   ✕ clears the preference.
5. **Tests + hardening** — derivation unit tests, hover/drag component tests,
   full check suite, manual run on the Mac against the demo side-by-side.

One PR, reviewed as a whole; the ledger commit leads it. Rough size: ~1,000
lines of implementation plus tests — at the comfort line but not divisible
without shipping a window that shows nothing.

Acceptance for the parity rule: run the demo build and the ported build
side-by-side and walk `docs/hud-states.md` — every state, transition, and
positioning behaviour matches, with the enumerated forced differences
(data source, blink signal plumbing) invisible at the surface.

## Open questions

1. **Settings pane**: the toggle fits the **Usage** pane (it renders the live
   usage that pane governs) or **Appearance**. Proposal: Usage, directly under
   the live-usage opt-in it depends on. (The demo's "General" section doesn't
   map — antiburn's General pane is launch/update plumbing.)
2. **Popover pop-out button placement**: the demo puts it beside the tray
   usage bars; antiburn's popover has a different usage surface. Proposal: the
   usage view's header, nearest equivalent position.

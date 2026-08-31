# antiburn desktop

The antiburn desktop application: a menu-bar / system-tray shell around the
local [`antiburn-local`](../../crates/antiburn-local) engine.

The app discovers the coding-agent sessions already on this machine, analyzes
them with the engine, and shows activity, per-session analysis, and
API-equivalent cost estimates. Everything runs on the device, as you: antiburn
needs no antiburn account, server, or backend of any kind, and nothing about
your sessions is uploaded. It makes two calls to a service of ours, neither of
which it depends on: the updater plugin, registered in release builds only,
asking whether a newer version exists; and the anonymised analytics
channel in [`src-tauri/src/analytics`](src-tauri/src/analytics),
which reports on the application itself in official release builds. The Ready
screen explains it, and Settings → Privacy provides the opt-out. The analytics
client is excluded from default source and development builds.

## Layout

```text
design.md       The design contract: tokens, type scale, motion, platform rules
src/            React 19 + TypeScript frontend (Vite, Tailwind v4)
  components/
    ui/         Shared presentation primitives (no app state, no IPC)
  lib/          Route selection, the typed IPC surface, presentation helpers
  styles/       The design foundation, imported by src/styles.css
  views/        One component per window, plus its panes
tests/          Checks that must not live inside the tree they check
scripts/        Icon generator (see src-tauri/icons/README.md)
src-tauri/      The Tauri 2 shell: windows, tray, store, scan, commands
  capabilities/ Webview permission grants
  icons/        Generated app and tray artwork
```

Read [`design.md`](design.md) before any styling work. Component code uses the
semantic Tailwind utilities it defines (`bg-surface`, `text-label`,
`type-body`, …) rather than raw values.

`src-tauri` is a **standalone Cargo workspace** with its own `Cargo.lock`. It is
deliberately not joined with the engine's workspace: the engine resolves under
its own dependency boundary that keeps it free of any service of ours, and the
shell's app-framework dependencies must not leak into that resolution.

## Prerequisites

- Rust, per [`rust-toolchain.toml`](../../rust-toolchain.toml)
- Node 22+ and pnpm (via Corepack: `corepack enable`)
- Platform dependencies for Tauri 2 — on Debian/Ubuntu:
  `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev`

## Commands

Run from the repository root:

```bash
pnpm install
pnpm --filter @antiburn/desktop dev          # Tauri dev build (tray + popover)
pnpm --filter @antiburn/desktop dev:web      # frontend only, in a browser
pnpm --filter @antiburn/desktop dev:bundle   # bundled debug .app / installer
pnpm --filter @antiburn/desktop lint
pnpm --filter @antiburn/desktop type-check
pnpm --filter @antiburn/desktop test
pnpm --filter @antiburn/desktop build        # frontend bundle only
pnpm --filter @antiburn/desktop icons        # regenerate app and tray icons
```

And for the shell:

```bash
cd apps/desktop/src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

To inspect analytics requests locally, start the print-only loopback collector
in [`docs/analytics.md`](../../docs/analytics.md#verifying-this-yourself), then
run:

```bash
ANTIBURN_ANALYTICS_URL=http://127.0.0.1:8787 \
ANTIBURN_ANALYTICS_OPERATOR="Local development" \
pnpm --filter @antiburn/desktop exec tauri dev \
  --features analytics --config src-tauri/tauri.debug.conf.json
```

Debug builds load the frontend from the Vite dev server, so `cargo` checks do
not need a built bundle. Release packaging embeds `apps/desktop/dist`.

The macOS-only [memory reporting runbook](../../docs/runbooks/memory-reporting.md)
documents the probe-enabled release measurement, Steve setup, and result
interpretation.

`rusqlite` is compiled from bundled sources, so neither CI nor a checkout needs
a system SQLite.

## Debugging

See [`docs/debugging.md`](../../docs/debugging.md) for development modes,
debug-profile isolation, developer tools, logs, onboarding tests, sample
notifications, and the updater simulator.

## What keeps the app local

The Tauri content security policy limits renderer connections to the local app
and IPC. The Rust analytics module tests consent, endpoint injection, and the
payload schema. `cargo-deny` rejects known telemetry dependencies in the local
engine. Release and dependency checks run through the required CI gate.

## Shell behavior

See [Desktop window renderer lifecycle](../../docs/window-renderer-lifecycle.md)
for the shared readiness handshake, onboarding prewarm, popover eviction,
Settings teardown, and the memory rules behind those policies.

- **Tray item.** Primary click toggles the popover. Secondary click opens a
  menu with Pin Window, Settings, and Quit. The Settings sidebar ends in the
  same Quit action — an agent application has no Dock icon and no application
  menu, so those two are the only places a reader can look for the way out.
  Both go through the shell's `exit(0)`, which is what distinguishes a
  deliberate quit from the window closes the shell suppresses. On macOS the
  item stays highlighted for as long as the popover is open: the system's own
  highlight is momentary and lets go on mouse-up, so the shell drives it, and
  clears it again on every path that puts the popover away.
- **Popover.** 380pt wide, frameless, always on top, hidden from the taskbar.
  It is created on demand and anchored under its menu-bar item on each
  open, flipping above the item and clamping to the display when there is no
  room below. It hides when it loses focus, when Escape is pressed, on a
  second click of the menu-bar item, and — on macOS — on a click anywhere
  outside the app, which catches the Finder desktop: clicking it makes no
  window key, so no focus change is reported at all.
- **Pin.** The tray menu's first item suspends all four of those dismissals,
  and reads Unpin Window while it does. The state is in memory only: a pin
  means "keep this on screen while I work", and a relaunch ends that work.
  Pinning also re-shows the popover, because opening the tray menu is what
  took focus away from it in the first place.
- **First run.** A 680×480 decorated window of its own, opened at launch while
  onboarding is unfinished — a fresh install should not have to discover the
  menu-bar glyph before it is told anything. While
  it is unfinished the tray click goes here rather than to the popover, which
  has nothing to show yet, and antiburn is an ordinary Dock application so the
  window can be reached again once something else takes focus. Finishing it
  puts the window away, drops the Dock icon, and posts the one notification
  that says where the app now lives, anchored under that glyph.
- **Settings.** An ordinary decorated window, created on demand and destroyed
  on close. A source list on the left, one pane on the right; every control
  writes through immediately, so there is no Save button and no dirty state.
- **Popover lifetime.** Finishing onboarding starts one hidden renderer before
  the onboarding window retires. After it becomes ready, the handoff renderer
  stays warm for up to 60 seconds. The first reveal consumes that lease; later
  dismissals use the normal 15-second grace period before renderer destruction.
- **Local store.** One SQLite database under the app data directory
  (`ai.antiburn.desktop`, or `ai.antiburn.desktop.debug` for a development
  build — see above) holds preferences, scan roots, and the local session data
  needed for visibility and analysis. That data may include content copied
  from a transcript, but remains on the device; the agent's source transcript
  is never modified or deleted. Migrations are embedded and versioned by the
  `user_version` pragma.
- **Scanning.** A single background task refreshes what the app knows: once at
  launch (after onboarding), whenever the popover is opened, every 60s while it
  stays open, paused entirely while it is hidden, and on demand. Passes never
  overlap and are bounded. CPU, memory, and disk I/O are product constraints:
  background work must be no more frequent or intensive than the visible
  feature requires. See the policy at the top of `src-tauri/src/scan.rs`.
- **Notifications.** Six kinds, all posted by the shell and never by the webview
  (which is granted no notification permission): an automatic update, a failed
  scan, low disk space, a crossed usage milestone, the first-run menu-bar
  location, and the settings pane's own test. The test and first-run location
  bypass the master switch because both follow a direct reader action. Other
  kinds use the master preference and their own gate, and nothing repeats. See
  the policy at the top of
  `src-tauri/src/notifications.rs`. Delivery is antiburn's own always-on-top
  window rather than the platform's notification centre: the `antiburn-nudge`
  crate under `src-tauri/crates/nudge/`, applied at the seam in
  `src-tauri/src/nudges.rs`. Nothing about a notification leaves the machine.
  Usage milestones default to every 10% and compare quota consumed with the
  share of the current limit window that has elapsed. Settings offers every 5%
  step when a reader wants different milestones.
- **Attention.** The popover shows a banner above the activity list when a
  repository cannot be read (which opens Settings at Sources) or when the local
  database rejects a write (which retries with a scan). Both are derived from
  shell-reported signals in `src/lib/attention.ts`; there is no speculative
  banner kind.
- **Theme.** Follows the operating system through `color-scheme` and Tailwind's
  `prefers-color-scheme` dark variant.
- **macOS.** `LSUIElement` in [`src-tauri/Info.plist`](src-tauri/Info.plist)
  makes the bundled app an agent; the shell applies the equivalent accessory
  activation policy at runtime so unbundled development runs match.

Settings and onboarding have dedicated HTML and TypeScript entries. The
resident shell uses URL fragments for the nudge and overlay, with the popover
as its default. Each window owns one surface until the shell releases it.

## Known gaps

These build-level limits affect desktop development:

- The popover is opaque and square-cornered. Rounded, translucent chrome needs
  `macOSPrivateApi` plus transparent-window support, and arrives with the
  design system.
- The updater plugin is registered in release builds only, so development
  performs no update checks or downloads.
- `bundle.createUpdaterArtifacts` is `true`, which means a **bundle** build
  signs its updater artifact and therefore needs `TAURI_SIGNING_PRIVATE_KEY` in
  the environment. Nothing in CI or the everyday `cargo`/`pnpm` checks bundles,
  so this only affects running `tauri build` by hand; `dev:bundle` already turns
  the artifact off through the debug config, and a release-profile bundle
  without a key needs `--config '{"bundle":{"createUpdaterArtifacts":false}}'`.
- Launch at login is applied only by builds carrying the Cargo `distribution`
  feature, which CI sets for packaged releases. macOS 13+ uses the system's
  main-app service, Windows uses the per-user Run key, and Linux writes an
  escaped Desktop Entry. New installs are asked on the Ready step (default on),
  General reflects the same preference, and development runs — including
  `cargo run --release` — never change the machine's login items.
- Agent icons are a single neutral glyph for every agent. Vendor logos are the
  vendors' marks; original per-agent artwork is a later stream, and
  `src/lib/agentIcon.tsx` already carries the icon-name seam it will key off.

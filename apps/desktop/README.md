# antiburn desktop

The antiburn desktop application: a menu-bar / system-tray shell around the
local [`antiburn-local`](../../crates/antiburn-local) engine.

The app discovers the coding-agent sessions already on this machine, analyzes
them with the engine, and shows activity, per-session analytics, and
API-equivalent cost estimates. Everything runs on the device: nothing is
uploaded, and the only network-capable surface in the whole application is the
updater plugin, which is registered in release builds only.

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
its own network-free dependency boundary, and the shell's app-framework
dependencies must not leak into that resolution.

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

Debug builds load the frontend from the Vite dev server, so `cargo` checks do
not need a built bundle. Release packaging embeds `apps/desktop/dist`.

`rusqlite` is compiled from bundled sources, so neither CI nor a checkout needs
a system SQLite.

## What keeps the app offline

Three independent checks, none of which relies on review:

1. The engine's own `tests/boundary.rs` keeps `antiburn-local` free of network
   and socket dependencies.
2. The repository-wide [`scripts/check-boundary.mjs`](../../scripts/check-boundary.mjs)
   scans every file for prohibited concepts, including telemetry SDKs and raw
   socket types.
3. [`tests/offline.test.ts`](tests/offline.test.ts) walks `src/` and fails on
   any browser networking API (`fetch`, `XMLHttpRequest`, `WebSocket`,
   `EventSource`, `sendBeacon`, remote dynamic imports). It lives outside `src/`
   on purpose: it names every API it bans, so a guard inside the tree it checks
   would trip its own check.

## Shell behavior

- **Tray item.** Primary click toggles the popover. Secondary click opens a
  menu with Settings and Quit. The Settings sidebar ends in the same Quit
  action — an agent application has no Dock icon and no application menu, so
  those two are the only places a reader can look for the way out. Both go
  through the shell's `exit(0)`, which is what distinguishes a deliberate quit
  from the window closes the shell suppresses.
- **Popover.** 380pt wide, frameless, always on top, hidden from the taskbar.
  It is created once at startup and anchored under its menu-bar item on each
  open, flipping above the item and clamping to the display when there is no
  room below. It hides when it loses focus.
- **Settings.** An ordinary decorated window, created on first use and reused.
  A source list on the left, one pane on the right; every control writes
  through immediately, so there is no Save button and no dirty state.
- **Local store.** One SQLite database under the app data directory
  (`ai.antiburn.desktop`) holds preferences, scan roots, a session metadata
  cache, and the engine-derived analysis. **It never stores transcript
  content** — see the contract in `src-tauri/src/store/schema.rs`. Migrations
  are embedded and versioned by the `user_version` pragma.
- **Scanning.** A single background task refreshes what the app knows: once at
  launch (after onboarding), whenever the popover is opened, every 60s while it
  stays open, paused entirely while it is hidden, and on demand. Passes never
  overlap and are bounded — see the policy at the top of `src-tauri/src/scan.rs`.
- **Notifications.** Exactly two, both posted by the shell and never by the
  webview (which is granted no notification permission): an automatic update
  check that found a version, and the first scan failure of a run. Each is gated
  by the master preference *and* its own, both default on, and neither repeats —
  see the policy at the top of `src-tauri/src/notifications.rs`. Delivery is the
  platform's own notification centre; nothing about a notification leaves the
  machine.
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

Both windows load the same bundle and select their view from the URL fragment.
There is no router: each window owns one route for its whole lifetime.

## Known gaps

Every deliberate difference from the ratified feature matrix — the deferred
update-install flow, the absent external links, the omitted panes, and the CI
coverage choices — is recorded with its reason and its revisit milestone in
[`docs/deviations.md`](../../docs/deviations.md). The list below is the
build-level subset that a developer working in this directory runs into first:

- The popover is opaque and square-cornered. Rounded, translucent chrome needs
  `macOSPrivateApi` plus transparent-window support, and arrives with the
  design system.
- Updates are configured but inert. `plugins.updater.pubkey` in
  `tauri.conf.json` is empty, so the app reports no update support and the
  release workflow refuses to build until the key pair is minted (see
  [`docs/runbooks/updater-key-recovery.md`](../../docs/runbooks/updater-key-recovery.md)).
  The plugin is registered in release builds only, so development performs no
  network requests at all.
- `bundle.createUpdaterArtifacts` is `true`, which means a **bundle** build
  signs its updater artifact and therefore needs `TAURI_SIGNING_PRIVATE_KEY` in
  the environment. Nothing in CI or the everyday `cargo`/`pnpm` checks bundles,
  so this only affects running `tauri build` by hand; to do that without a key,
  pass `--config '{"bundle":{"createUpdaterArtifacts":false}}'`.
- Launch-at-login is recorded as a preference but not enforced: registering a
  login item needs the autostart plugin, which this build does not carry. The
  General pane says so next to the control rather than showing a switch that
  silently does nothing.
- Agent icons are a single neutral glyph for every agent. Vendor logos are the
  vendors' marks; original per-agent artwork is a later stream, and
  `src/lib/agentIcon.tsx` already carries the icon-name seam it will key off.
- `icon.icns` / `icon.ico` are produced during release packaging rather than
  checked in — nothing in the build or test path needs them yet.

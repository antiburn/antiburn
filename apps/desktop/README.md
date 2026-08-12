# antiburn desktop

The antiburn desktop application: a menu-bar / system-tray shell around the
local [`antiburn-local`](../../crates/antiburn-local) engine.

This is currently a **skeleton**. It builds, runs, shows a tray item, opens an
anchored popover and a settings window, and proves the engine link end to end.
Design system, views, and data land in later streams.

## Layout

```text
src/            React 19 + TypeScript frontend (Vite, Tailwind v4)
  lib/          Route selection and the typed IPC surface
  views/        One component per window
scripts/        Icon generator (see src-tauri/icons/README.md)
src-tauri/      The Tauri 2 shell: windows, tray, commands
  capabilities/ Webview permission grants
  icons/        Generated app and tray artwork
```

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

## Shell behavior

- **Tray item.** Primary click toggles the popover. Secondary click opens a
  menu with Settings and Quit — an agent application has no Dock icon and no
  application menu, so the menu is the only way out.
- **Popover.** 380pt wide, frameless, always on top, hidden from the taskbar.
  It is created once at startup and anchored under its menu-bar item on each
  open, flipping above the item and clamping to the display when there is no
  room below. It hides when it loses focus.
- **Settings.** An ordinary decorated window, created on first use and reused.
- **Theme.** Follows the operating system through `color-scheme` and Tailwind's
  `prefers-color-scheme` dark variant.
- **macOS.** `LSUIElement` in [`src-tauri/Info.plist`](src-tauri/Info.plist)
  makes the bundled app an agent; the shell applies the equivalent accessory
  activation policy at runtime so unbundled development runs match.

Both windows load the same bundle and select their view from the URL fragment.
There is no router: each window owns one route for its whole lifetime.

## Known gaps

These are deliberate, and belong to later milestones:

- The popover is opaque and square-cornered. Rounded, translucent chrome needs
  `macOSPrivateApi` plus transparent-window support, and arrives with the
  design system.
- Updates are configured but inert. `plugins.updater.pubkey` in
  `tauri.conf.json` is empty and `bundle.createUpdaterArtifacts` is `false`;
  release signing sets both. The plugin is registered in release builds only,
  so development performs no network requests at all.
- Launch-at-login is not wired up.
- `icon.icns` / `icon.ico` are produced during release packaging rather than
  checked in — nothing in the build or test path needs them yet.

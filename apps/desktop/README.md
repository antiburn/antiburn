# antiburn desktop

The desktop app combines a main window and menu-bar / system-tray companion
around the local [`antiburn-local`](../../crates/antiburn-local) engine. It
shows coding-agent activity, session analysis, usage limits, and estimated costs.

Use this README to build the app and find the owner of a change. Detailed
behavior belongs in the linked architecture guides and source modules. Update
this file when setup steps or ownership boundaries change.

## Architecture and source map

- [`src/`](src/) contains the React frontend. Shared presentation primitives
  live in `components/ui/`; feature views and their external stores own visible
  state. Native commands and events cross typed boundaries in `lib/`.
- [`src-tauri/`](src-tauri/) contains the Tauri shell: windows, tray, settings,
  scanning, local storage, and feature services. It is a standalone Cargo
  workspace with its own lockfile. This keeps shell dependencies outside the
  engine's dependency boundary.
- [`src/styles/`](src/styles/) and feature stylesheets own exact visual values.
  Read the [design guide](design.md) before styling work.
- [`tests/`](tests/) holds checks that must live outside the source tree they
  inspect. Feature tests otherwise live alongside their owners.

| Changing                                            | Read first                                                                                                            |
| --------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| Navigation, search, or main-window integration      | [Navigation boundaries](../../docs/main-window-navigation.md)                                                         |
| Window creation, readiness, visibility, or teardown | [Renderer lifecycle](../../docs/window-renderer-lifecycle.md)                                                         |
| HUD placement or passive native interaction         | [HUD states](../../docs/hud-states.md)                                                                                |
| Scan publication or live session state              | [Session lifecycle](../../docs/session-lifecycle-events.md), [scan policy](src-tauri/src/scan/mod.rs)                 |
| Agent discovery or parsing                          | [Session coverage](../../docs/session-coverage.md)                                                                    |
| Burn Checks or remediation                          | [Check coverage](../../docs/check-coverage.md), [remediation safety](../../docs/remediation.md)                       |
| Notifications                                       | [Notification policy](src-tauri/src/notifications.rs), [OS permission boundary](../../docs/os-notification-gating.md) |
| Analytics                                           | [Public catalog](../../docs/analytics.md), [measurement and review rules](../../docs/analytics-measurement.md)        |
| Interface size or native geometry                   | [Scaling QA](../../docs/runbooks/interface-scale-qa.md)                                                               |
| Icons                                               | [Icon generation](src-tauri/icons/README.md)                                                                          |

## What keeps the app local

Session discovery and analysis run on the device. The local SQLite database
can contain transcript-derived content; the source transcripts are not modified
or deleted. Provider integrations use credentials issued by those providers.
Public model prices come from models.dev without session data or credentials.

The project-operated network channels are release update checks and optional
anonymised analytics. Neither is required to use local analysis. Analytics is
excluded from default source builds; configured official builds disclose it
and offer an opt-out in Settings → Privacy. See the
[privacy policy](../../docs/privacy-policy.md) and
[analytics disclosure](../../docs/analytics.md) for the maintained boundaries.

The Tauri content security policy restricts renderer networking. The shell owns
provider access, analytics delivery, and reviewed file edits. Auto Fix requires
review and confirmation, revalidates its exact target, and preserves a backup
before replacing an existing configuration file. Follow the
[remediation guide](../../docs/remediation.md) when changing that boundary.

## Prerequisites

- Rust, per [`rust-toolchain.toml`](../../rust-toolchain.toml)
- Node 22+ and pnpm (via Corepack: `corepack enable`)
- Platform dependencies for Tauri 2 — on Debian/Ubuntu:
  `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev`

## Commands

Run from the repository root:

```bash
pnpm install
pnpm --filter @antiburn/desktop dev          # Tauri dev build (main window + tray)
pnpm --filter @antiburn/desktop dev:web      # frontend only, in a browser
pnpm --filter @antiburn/desktop dev:bundle   # bundled debug .app / installer
pnpm --filter @antiburn/desktop lint
pnpm --filter @antiburn/desktop type-check
pnpm --filter @antiburn/desktop test
pnpm --filter @antiburn/desktop build        # frontend bundle only
pnpm --filter @antiburn/desktop icons        # regenerate app and tray icons
```

The `dev`, `dev:bundle`, and `tauri` scripts load `apps/desktop/.env` when it
exists. The shell environment takes precedence. `.env.example` lists the Google
installed-app variables used only for Antigravity 2.0, its IDE, and `agy`.

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
pnpm --filter @antiburn/desktop tauri dev \
  --features analytics --config src-tauri/tauri.debug.conf.json
```

Debug builds load the frontend from the Vite dev server, so `cargo` checks do
not need a built bundle. Release packaging embeds `apps/desktop/dist`.

The macOS-only [memory reporting runbook](../../docs/runbooks/memory-reporting.md)
documents the probe-enabled release measurement, Steve setup, and result
interpretation.

The [main-window validation runbook](../../docs/runbooks/main-window.md) covers
native lifecycle checks, opening latency, and hidden-window resource use. Run
Rust formatting, Clippy, and tests from `src-tauri/crates/main-window` as well
as the shell when changing the main-window mechanism.

For companion-window changes, run the same Rust checks from
`src-tauri/crates/anchored-window`. CI runs its Clippy checks and tests on macOS,
Windows, and Linux.

`rusqlite` is compiled from bundled sources, so neither CI nor a checkout needs
a system SQLite.

## Debugging

See [`docs/debugging.md`](../../docs/debugging.md) for development modes,
debug-profile isolation, developer tools, logs, onboarding tests, sample
notifications, and the updater simulator.

## Known gaps

These limits affect development and native validation:

- Tauri-created macOS webviews retain Wry's creation-time activation limitation,
  even with `focused(false)`. Native hover previews bypass Wry. A passive reveal
  does not prove that cold creation preserves focus; validate both paths using
  the [renderer lifecycle guide](../../docs/window-renderer-lifecycle.md).
- The real updater runs only in release builds. Use the debug simulator for UI
  work and signed artifacts for delivery validation. See
  [debugging](../../docs/debugging.md#test-the-updater-interface).
- Release bundle builds create signed updater artifacts and require
  `TAURI_SIGNING_PRIVATE_KEY`. `dev:bundle` disables these artifacts through the
  debug config. For an unsigned local release-profile bundle, pass
  `--config '{"bundle":{"createUpdaterArtifacts":false}}'` to the Tauri build.
  Ordinary frontend builds and Cargo checks do not bundle the app.
- Launch at login changes OS state only in packaged builds with the Cargo
  `distribution` feature. A release-profile development run does not enable
  that integration. Test packaged behavior through the
  [release runbook](../../docs/runbooks/release.md).

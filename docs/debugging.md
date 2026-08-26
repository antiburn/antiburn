# Debugging the desktop application

This guide covers the focused tools for desktop development. antiburn does not
have a general Debug pane.

## Choose a development mode

Run commands from the repository root.

| Command | Use it for | Limits |
| --- | --- | --- |
| `pnpm --filter @antiburn/desktop dev` | Normal desktop work with the Tauri shell and Vite reload | Runs an unbundled debug build |
| `pnpm --filter @antiburn/desktop dev:web` | Fast frontend layout and interaction work in a browser | Has no native tray, shell, scanner, local database, or updater |
| `pnpm --filter @antiburn/desktop dev:bundle` | Tests that need a bundled debug application or installer | Does not create or sign updater artifacts |

Use the package scripts. Do not use a bare `cargo run`, `cargo build`, or
`pnpm tauri dev` to launch the app. Those commands omit
`src-tauri/tauri.debug.conf.json` and can select the release identity.

## Keep debug data separate

The debug configuration uses the bundle identifier
`ai.antiburn.desktop.debug`. The installed app uses
`ai.antiburn.desktop`. This difference separates the app data, the local
database, and platform privacy grants.

Open **Settings → About → Data folder** to see the exact directory for the
running build. For a fresh-profile test:

1. Open the active data folder from Settings.
2. Quit antiburn.
3. Move that directory aside. Do not delete it.
4. Start the same build again.

Do not edit the SQLite database while antiburn runs. Keep the debug override
when you change package scripts or maintain a fork. On macOS, reset privacy
grants for `ai.antiburn.desktop.debug`, not `ai.antiburn.desktop`.

## Use developer tools and logs

Debug webviews keep reload and developer tools available. Focus the webview,
then use the normal platform shortcuts:

- macOS: `Command+Option+I` for developer tools and `Command+R` to reload.
- Windows and Linux: `Ctrl+Shift+I` for developer tools and `Ctrl+R` to reload.

The shell writes compact logs to the terminal and hourly JSON logs to a debug
directory. It removes file logs after seven days.

| Platform | Debug log directory |
| --- | --- |
| macOS | `~/Library/Logs/antiburn-debug` |
| Windows | `%LOCALAPPDATA%\antiburn-debug\logs` |
| Linux | `${XDG_STATE_HOME:-~/.local/state}/antiburn-debug/logs` |

Set `RUST_LOG` before the development command when you need a different Rust
filter. For example, `RUST_LOG=trace pnpm --filter @antiburn/desktop dev`
enables trace output. Logs can contain local diagnostic values. Check them
before you share them.

## Restart onboarding

Select **Reset Onboarding** in the debug-build tray menu. This action sets
`onboardingCompleted` to `false` and opens setup at Welcome. It keeps indexed
sessions, scan folders, repository choices, and preferences. Closing setup
before completion keeps onboarding pending.

Use the fresh-profile procedure above when you need a true first-install test.
Packaged builds provide **Settings → General → Run setup again** for the same
non-destructive restart.

## Show sample notifications

Run the normal `dev` command, then open **Settings → Notifications → Sample
notifications**. Each button shows one notification kind with fixed sample
values. The action skips notification gates so you can check copy, layout,
position, timing, and sound on the real notification window. The sample row
uses Vite development mode and does not appear in `dev:bundle`.

Use **Test notification** to check the normal reader-facing test action. Sample
notifications do not prove that a real scan, disk, usage, or update event has
the correct trigger conditions.

## Test the updater interface

Open **Settings → About → Updates** in a debug build. Select **Start
simulation**, then use the normal update controls. The fixed flow covers each
important state:

1. **Install** starts indeterminate and determinate download progress.
2. The first attempt fails and offers **Try install again**.
3. The retry downloads again, installs, and offers **Restart to update**.
4. Restart resets the simulator without restarting the process.

To test the notification route, open **Settings → Notifications → Sample
notifications** and select the update sample. Its **Install** button opens About
and starts the same flow. Clicking the notification body opens About but does
not approve the install.

The simulator uses fixed local data. It does not contact the release feed,
download a bundle, verify a signature, replace the application, or restart the
process. The real updater remains unavailable in debug builds.

Simulation does not replace smoke testing a signed release. Before publication,
install the signed artifacts and test the real download, signature check,
installation, restart, and failure paths in
[`docs/runbooks/release.md`](runbooks/release.md).

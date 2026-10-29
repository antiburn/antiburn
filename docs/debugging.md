# Debugging the desktop application

This guide covers the focused tools for desktop development. antiburn does not
have a general Debug pane.

## Choose a development mode

Run commands from the repository root.

| Command                                      | Use it for                                               | Limits                                                         |
| -------------------------------------------- | -------------------------------------------------------- | -------------------------------------------------------------- |
| `pnpm --filter @antiburn/desktop dev`        | Normal desktop work with the Tauri shell and Vite reload | Runs an unbundled debug build                                  |
| `pnpm --filter @antiburn/desktop dev:web`    | Fast frontend layout and interaction work in a browser   | Has no native tray, shell, scanner, local database, or updater |
| `pnpm --filter @antiburn/desktop dev:bundle` | Tests that need a bundled debug application or installer | Does not create or sign updater artifacts                      |

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

Debug webviews keep the context menu, reload, and developer tools available.
Right-click the webview and select **Inspect Element**, or focus the webview
and use the normal platform shortcuts:

- macOS: `Command+Option+I` for developer tools and `Command+R` to reload.
- Windows and Linux: `Ctrl+Shift+I` for developer tools and `Ctrl+R` to reload.

The shell writes compact logs to the terminal and hourly JSON logs to a debug
directory. It removes file logs after seven days.

| Platform | Debug log directory                                     |
| -------- | ------------------------------------------------------- |
| macOS    | `~/Library/Logs/antiburn-debug`                         |
| Windows  | `%LOCALAPPDATA%\antiburn-debug\logs`                    |
| Linux    | `${XDG_STATE_HOME:-~/.local/state}/antiburn-debug/logs` |

Set `RUST_LOG` before the development command when you need a different Rust
filter. For example, `RUST_LOG=trace pnpm --filter @antiburn/desktop dev`
enables trace output. Logs can contain local diagnostic values. Check them
before you share them.

If a session stays on **Running Burn Checks**, compare metadata scans with
evidence publication in the logs. A fresh title or activity timestamp does not
prove that parsing or checks completed. Failed local parse attempts show
**Burn Checks unavailable** during retry backoff when no earlier result exists.
Earlier published results remain visible as stale. Live OpenCode connection
errors are separate from stored-session parsing errors. Do not clear either
database to recover a failed worker; the worker retries local evidence without
resending provider requests whose outcome is unknown.

### Diagnose a saved connection credential read

A successful Keychain prompt does not prove that the app read the saved key.
The local warning `system_one_credential_read_failed` records a fixed `stage`,
a fixed `category`, and an optional numeric macOS `os_status`. A credential read
that fails before Use changes selection has stage `switch_preflight`.

Share only these fields when reporting that failure. Do not share the key,
Keychain contents, raw credential errors, or a full log file. The diagnostic
contains no credential bytes, account metadata, connection IDs, paths, or routes.
It is a local log event, not an analytics event. A storage error is not proof
that the provider rejected the key. Use does not test the provider.

### Follow a YAML check through the runtime

Use a narrow Rust filter when you need check diagnostics without every scan event:

```sh
RUST_LOG="info,\
antiburn_desktop::installed_checks=debug,\
antiburn_desktop::store=debug,\
antiburn_desktop::published_evidence_reader=debug,\
antiburn_desktop::insights_worker=debug,\
antiburn_desktop::insights_report::installed_checks=debug" \
  pnpm --filter @antiburn/desktop dev
```

Compare source admission, committed publication, candidate preparation, evaluation,
and report output. A watched file update is not proof that a candidate exists.
A provider dispatch intent is not proof that the provider returned an answer.
Check saved acceptance and the last status write before interpreting the UI.

| Event                                      | What it proves                                                                                                                   |
| ------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------- |
| `installed_check_admission`                | The current startup gate, including credential restoration.                                                                      |
| `burn_check_startup_recovery`              | Saved work was requeued, restored, or failed without a new send.                                                                 |
| `opencode_v2_capture_attached`             | The host attached to the verified OpenCode source. A history gap is separate.                                                    |
| `opencode_v2_capture_stopped`              | Current capture stopped with a fixed phase and reason. Limit failures include available numeric counts and bounds.               |
| `opencode_v2_evidence_published`           | Verified capture committed public evidence. Cursor ranges connect publication to source-page consumption.                        |
| `opencode_v2_capture_gap`                  | A committed capture gap changed evidence or capture state. A detach reason alone does not prove a current alert.                 |
| `stored_metrics_committed`                 | Stored V2 metrics committed while live evidence remained outside that writer's ownership.                                        |
| `native_evidence_transition_committed`     | Capture or reconciliation committed a publication range and actual invalidation counts.                                          |
| `burn_check_evidence_transition_committed` | Runtime consumption committed accepted-evidence revocations or unfinished-work changes for its consumed range.                   |
| `burn_check_source_page_committed`         | The host committed a source page and its candidates or gaps.                                                                     |
| `burn_check_preparation_committed`         | The host saved preparation before model dispatch. Rejected preparation includes available typed local failure and budget fields. |
| `burn_check_dispatch_intent_committed`     | The host saved send intent. This is not a confirmed response.                                                                    |
| `burn_check_answer_committed`              | The host saved a returned decision answer.                                                                                       |
| `burn_check_acceptance`                    | Acceptance succeeded, deferred, or rejected the saved work.                                                                      |
| `burn_check_delivery_unknown`              | Delivery is unknown; the runtime must not resend automatically.                                                                  |
| `burn_check_store_operation_failed`        | A named store operation failed with a safe category and available SQLite code.                                                   |
| `installed_check_report_state`             | The current result and progress counts used by the report.                                                                       |

These local events use fixed reasons, counts, and timing. They do not include
selected text, credentials, native record IDs, or private paths. Keep source
history limits separate from connection health when reading them.
Use the opaque `check_ref`, `activation_ref`, `candidate_ref`, and `decision_ref`
fields to follow concurrent work across stages. `report_ref` identifies the grouped
UI report; it is not a candidate identifier. These references support local
diagnosis and do not enter product analytics.

`burn_check_blocking_preparation` measures local blocking work, not a model
request or an accepted check. Its fixed `stage` is `work_probe` or
`preparation_start`. `succeeded=true` means the local operation returned `Ok`;
that includes a probe with nothing due and a preparation that no longer needs to
start. Normal timings use TRACE. The same event uses DEBUG when slot wait or
blocking work takes at least one second. Committed preparation, dispatch, answer,
and acceptance events remain visible at DEBUG, and safe caller failures retain
their own diagnostics. Do not count wrapper timings as provider requests.

Publication cursors connect committed capture changes to runtime consumption.
Metrics, capture, and candidate claims have separate fence domains; equal numeric
fences do not identify the same work. Invalidation summaries count actual
accepted-evidence revocations separately from unfinished candidates becoming
stale. Successful transaction summaries appear only after commit. A failed write
or a repeated no-op is not a committed transition.

For a local `NoFit`, compare its typed reason, budget kind, observed estimate,
and maximum. `reductions_exhausted` means permitted shortening ran but could not
fit the complete request. A missing reason stays unspecified, for example when
fixed questions fail before fitting. `PreparationResourceLimit` is a separate
local work or admission limit. These numeric estimates are not billed tokens.
Do not resend a request to obtain a clearer trace.

The local references are deterministic correlation values, not a guarantee of
anonymity. New check traces omit model strings, endpoints, raw errors, omission
paths, and serialized context as well as credentials and native IDs. Keep these
references and measurements out of product analytics.

Capture health and historical completeness are separate. A cold attachment or
recovered claim can leave earlier history partial while capture works now. An old
gap does not produce a capture alert after recovery. A current capture stop still
needs diagnosis for recent activity or unfinished admitted checks. An inactive
detach publication can still need consumption without creating a current alert;
its evidence-validity effects still apply. Stored V2 metrics cannot publish or
revoke live-capture evidence. File-backed agents use bounded suffix reads rather than a live
service connection. Their active delivery loop polls every second; metadata
watchers and the five-second active-file poll also refresh discovery.

Do not change a source cursor, delete local state, or resend a request to diagnose
progress. These actions can lose evidence or duplicate charges. Use synthetic
files for reproduction and keep private session content out of shared logs.

### Usage and CLI diagnostics

Claude usage recovery emits local `claude_credential_reread` and
`claude_refresh_outcome` debug events. Their fixed outcome values distinguish
changed credentials, deferred background recovery, cooldown, missing CLI,
metadata failures, launch failures, verification timeouts, and settled refreshes.
`live_source_failed` also includes the typed error detail. These events contain
no credentials or CLI output. A settled refresh means the credential carrier
changed; the subsequent usage request still determines whether recovery worked.
When Claude Desktop is the only Claude app and no Claude Code or Pi login
exists, the check fails as `authentication` with detail `DesktopOnly`. Claude
Desktop's own sign-in is never read.

To test without Pi's logins, start a debug build with
`ANTIBURN_IGNORE_PI_AUTH=1`. Live usage then ignores `~/.pi/agent/auth.json`
for Claude and Codex, and logs `pi_auth_ignored`. Release builds ignore the
variable.

The `claude_keychain_read` debug event records each Keychain secret read as
`absent`, `unreadable`, `found`, or `found_without_login`, with the `security`
exit code when the process reports one. The `warn` event `claude_keychain_secret_missing`
means that the secret read found no `Claude Code-credentials` item but the
attribute read found it or failed. The meter then shows a Keychain read
failure, unless another credential carrier returns a reading. The debug
event `live_source_absent` records a source that returned no reading and no
error; that provider shows no usage.

At launch, the `info` event `cli_located` records where antiburn finds the
`claude` CLI: `process_path`, `install_dir` (an install directory outside the
process `PATH`), or `none`. It contains no path. Release logs include it.
An app that starts from Finder gets only `/usr/bin:/bin:/usr/sbin:/sbin`;
`open` from a terminal passes the terminal's `PATH` instead. To reproduce a
Finder launch from a terminal, run
`osascript -e 'tell application "Finder" to open (POSIX file "/path/to/antiburn.app" as alias)'`.

## Reset first run

Select **Reset First Run** in the debug-build tray menu. This action clears
locally indexed session data, resets the main window's saved placement, and
sets `onboardingCompleted` and `liveUsageStarted` back to `false`. The next
launch — or the current main window, once it reconciles the new settings —
shows the first-run Overview again, as a fresh install would.

This is destructive: unlike the retired packaged-build restart, it does not
keep indexed sessions. Use the fresh-profile procedure above instead when you
need a true first-install test without touching an existing debug profile's
other data.

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

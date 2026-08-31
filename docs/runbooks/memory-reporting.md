# Desktop memory reporting

The memory report measures the real antiburn menu-bar popover with deterministic
synthetic sessions. It uses normal application startup and tray behavior. The
`memory-probe` Cargo feature adds only fixture rows and the exact popover WebKit
content-process PID diagnostic.

## Requirements

Live reports require:

- macOS 13 or later.
- A logged-in desktop session. SSH-only and headless sessions are unsupported.
- The maintained [mikker/steve](https://github.com/mikker/steve) CLI. The tested
  version is 0.5.1.
- Accessibility permission for Steve in **System Settings > Privacy & Security
  > Accessibility**.
- `/bin/ps` and `/usr/bin/footprint`, which macOS supplies.
- No running antiburn or antiburn memory-probe instance.

Install Steve from its maintained Homebrew tap:

```bash
brew tap mikker/tap
brew install steve
```

Run `steve apps` once. If macOS denies access, add the installed Steve executable
to Accessibility and run the command again.

Linux, Windows, and headless CI cannot run this report because it uses macOS
Accessibility, menu-bar geometry, WebKit process APIs, and `footprint`. CI runs
only the platform-independent parser and report tests.

## Run a report

Run the standard release measurement from the repository root:

```bash
node scripts/mem-report.mjs --release \
  --output /tmp/antiburn-memory.json \
  --summary /tmp/antiburn-memory-summary.json
```

This builds a probe-enabled release `.app`, creates five independent profiles,
and measures five settled samples per profile. The default fixture contains 225
sessions.

Reuse the existing debug or release bundle without rebuilding:

```bash
node scripts/mem-report.mjs --no-build --runs 1 --samples 1
```

Use `node scripts/mem-report.mjs --help` for fixture, timing, metric, format,
profile-retention, and executable options.

## Measurement flow

For each run, the runner:

1. Creates an isolated home, temporary directory, and XDG directories.
2. Completes onboarding through Steve in an unmeasured setup launch.
3. Starts a fresh measured `.app` through macOS Launch Services.
4. Finds the app's menu-bar item through PID-scoped accessibility elements.
5. Clicks the reported item coordinates and waits for the `Sessions` landmark.
6. Reads the exact WebContent PID from the real page-load callback.
7. Samples the shell and WebContent processes after the settle delay.
8. Hides the popover and selects **Quit antiburn** from the native tray menu.

The runner rejects another antiburn instance, ambiguous status items, missing
PIDs, changed process start identities, and incomplete memory output. Failed
profiles can be retained with `--keep-profile failure`.

## Results

RSS comes from `/bin/ps`. Physical footprint comes from `/usr/bin/footprint`.
The aggregate physical footprint is the deduplicated total for the shell and
WebContent process, not the sum of two independently reported shared regions.

Each run reports individual samples and medians. The cross-run summary reports
the distribution of run medians. Use the maximum run median for the issue 237
acceptance gate.

Compare before and after results only when these values are identical:

- Machine and macOS build.
- Release or debug profile.
- Session count and fixture seed.
- Settle delay, sample count, interval, and run count.
- Metric selection.

The isolated profiles contain synthetic session data only. The runner disables
analytics and does not read a normal antiburn profile. Successful profiles are
removed by default. Do not commit reports because they contain local paths,
process IDs, timestamps, and machine-specific measurements.

## Native boundary

The `memory-probe` feature cannot be combined with `distribution`. It provides
bounded deterministic `ActivityEntry` values and reads WKWebView's private
`_webProcessIdentifier` selector after the real popover page loads. Distribution
builds contain neither behavior.

## Troubleshooting

- **Steve reports a permission error:** Enable Steve in **System Settings >
  Privacy & Security > Accessibility**. Restart Steve after changing permission.
- **Another antiburn instance is reported:** Quit all normal, debug, and probe
  instances. The runner intentionally refuses ambiguous ownership.
- **No status item appears:** Make sure the macOS menu bar is visible and has
  room for the antiburn item. The runner waits for one framed `AXMenuBarItem`.
- **The `Sessions` wait times out:** Keep the desktop session unlocked and do
  not click the app while the report runs.
- **The WebContent PID is missing:** Rebuild without `--no-build`. Confirm the
  build uses the `memory-probe` feature and the generated probe `.app`.
- **`footprint` fails:** Run the command from a local interactive administrator
  session. Use `--metric rss` only for diagnosis, not the physical-footprint
  acceptance gate.
- **A profile remains:** The report retained a failure for diagnosis. Use the
  reported profile path only after confirming its marker file and ownership.

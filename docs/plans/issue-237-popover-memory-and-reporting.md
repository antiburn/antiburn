# Issue #237: reduce the popover WebContent footprint

**Issue:** [antiburn#237](https://github.com/antiburn/antiburn/issues/237)

**Status:** Complete

**Acceptance target:** Every independent release run with 225 deterministic
session rows has a settled popover WebContent physical-footprint median below
100 MiB.

This plan replaces the earlier native scenario controller. The controller
changed application startup and window behavior, then stalled while it tried to
create and inspect WKWebViews from control threads. Those changes made the
measurement less representative than normal application use.

The new harness treats the application as a black box. It uses the macOS
Accessibility API through [Steve](https://github.com/mikker/steve) to click the
real menu-bar item and wait for visible content. The feature-gated native code
only supplies deterministic rows and reports the exact WebContent PID.

---

## 1. Scope

### Goals

- [x] Measure shell and exact popover WebContent memory on macOS.
- [x] Use a new isolated profile for each independent run.
- [x] Keep raw samples and per-run medians in machine-readable reports.
- [x] Drive the real menu-bar click, placement, readiness, hide, and quit paths.
- [x] Capture a release baseline before changing `SessionList`.
- [x] Bound mounted session rows to the viewport plus overscan.
- [x] Replace per-row tooltip trees with one shared list tooltip.
- [x] Keep the session list unmounted during native height contraction.
- [x] Add no `useEffect`.
- [x] Document operation, interpretation, privacy, and known limits.

### Result

The valid 225-row release baseline had renderer physical-footprint run medians
from 125,191,008 to 126,649,184 bytes. The optimized report had run medians
from 60,080,896 to 60,670,768 bytes, with no failures. The maximum run median
fell by 52.1% and is below the 100 MiB acceptance target. The aggregate
physical-footprint maximum run median fell from 156,288,920 to 89,573,200
bytes.

### Non-goals

- Do not build a general desktop scenario protocol.
- Do not automate Settings or transcript processing in this issue.
- Do not add memory thresholds to pull-request CI.
- Do not read a developer's normal profile or real transcripts.
- Do not identify WebContent ownership by process name or parent PID.
- Do not add unrestricted JavaScript or native window-control endpoints.
- Do not claim Linux or Windows memory-report support.
- Do not reduce the existing 500-row application input bound.
- Do not redesign the visible session-row content.

---

## 2. Requirements

The live memory report has these hard requirements:

- macOS 13 or later.
- A logged-in graphical session with the menu bar visible.
- The maintained upstream Steve CLI, pinned to version 0.5.1 or a documented
  compatible version.
- Accessibility permission for Steve in System Settings > Privacy & Security >
  Accessibility.
- `/usr/bin/footprint` and `/bin/ps`, which ship with macOS.
- No other running antiburn instance.

Install Steve with:

```bash
brew tap mikker/tap
brew install steve
```

The linked `salah2277/steve` repository is not the implementation dependency.
The maintained MIT-licensed upstream is `mikker/steve`.

CI runs only the pure Node parser and report tests. CI does not run live memory
measurements because hosted runners do not provide a stable interactive macOS
desktop, Accessibility permission, or comparable WebKit memory conditions.

---

## 3. Architecture

### 3.1 External action layer

`scripts/mem-report.mjs` launches the final `.app` through macOS Launch Services,
resolves its exact shell PID, and invokes Steve as a separate command. Launch
Services supplies the bundle privacy descriptions required by macOS.

Steve performs only user-observable actions:

```text
elements --pid PID        Find the antiburn menu-bar item and its frame
click-at X Y              Open or hide through the real tray callback
wait --pid PID --text     Wait for the Sessions accessibility landmark
click-at X Y --right      Open the native tray menu
click --pid PID --title   Select Quit antiburn from that menu
```

`AXPress` does not activate this status item. The runner uses the item's reported
frame with `steve click-at`. It does not call a native popover-opening helper.

### 3.2 Minimal native feature

The Cargo feature remains named `memory-probe`. Distribution builds cannot
enable it.

It has two responsibilities:

1. Return deterministic full-shape `ActivityEntry` rows from
   `list_recent_sessions` when `ANTIBURN_MEMORY_SESSIONS` is set.
2. Emit one prefixed JSON line from the real popover page-load callback with the
   exact `_webProcessIdentifier` and renderer generation.

Example diagnostic line:

```text
@antiburn-mem {"event":"webcontent","window":"popover","generation":1,"pid":48122}
```

The feature does not create, show, hide, close, inspect, or retain windows. It
does not start a protocol thread, read stdin, modify `RunEvent`, skip production
subsystems, or evaluate renderer JavaScript.

### 3.3 Why the PID hook remains

WKWebView content runs outside the application process. macOS does not expose a
reliable public API that maps a WebKit helper process back to its responsible
application. Process names and PPID traversal can select another application's
WebContent process.

The feature-gated hook uses WebKit's private `_webProcessIdentifier` selector.
It is acceptable only in local non-distributed measurement builds. If the
selector is unavailable or returns zero, the run fails instead of guessing.

### 3.4 Isolated profiles

Each run receives a directory with redirected home and application paths:

```text
<profile-root>/popover/run-001-XXXXXX/
  home/
  temp/
  data/
  config/
  state/
```

The runner sets `HOME`, `CFFIXED_USER_HOME`, `TMPDIR`, and the XDG paths. It also
sets `ANTIBURN_ANALYTICS_ENABLED=false` and uses synthetic session rows.

A separate unmeasured launch completes onboarding through Steve. The runner
then stops that setup process after the settings write finishes. The measured
launch reuses the isolated profile and runs normal application initialization.
The runner never opens or deletes the normal release or debug profile.

### 3.5 Measurement model

`--runs` means independent profiles and application launches. `--samples` means
repeated observations of one settled visible popover in one run. Samples from
different runs are never flattened into one population.

The runner records:

- Shell RSS and physical footprint.
- Exact popover WebContent RSS and physical footprint.
- A deduplicated multi-PID application physical footprint.
- Optional accessibility-tree node diagnostics.
- Operating system, hardware, Git revision, dirty state, fixture count, and
  fixture seed.

Before and after every memory command, the runner verifies that the shell and
WebContent PIDs still have the expected process-start identity. A changed or
dead PID invalidates the run.

---

## 4. CLI Contract

The issue acceptance command is:

```bash
node scripts/mem-report.mjs --release
```

Defaults:

```text
scenario: popover
sessions: 225
runs: 5 for --release, 1 otherwise
samples: 5
fixture seed: 237
metric: both
settle: 2s
timeout: 30s
profile retention: failure
```

Supported options:

```text
--release                   Build and measure an optimized .app
--app <path>                Use an existing probe-enabled executable
--no-build                  Require the existing executable
--runs <count>              Independent profiles and launches
--samples <count>           Samples of the settled popover
--sample-interval <time>    Delay between samples
--settle <time>             Delay after accessibility readiness
--timeout <time>            Per-action timeout
--metric <name>             rss, footprint, or both
--sessions <count>          Deterministic rows, from 0 through 500
--fixture-seed <integer>    Deterministic row-shape seed
--steve <path>              Steve executable, default from PATH
--format <name>             table, json, ndjson, or csv
--output <path>             Report destination
--summary <path>            Compact JSON summary destination
--profile-root <path>       Parent for isolated profiles
--keep-profile <policy>     never, failure, or always
--quiet                     Hide successful child diagnostics
--help                      Print usage
```

Durations require units such as `250ms` or `2s`.

---

## 5. Popover Run Contract

Each run executes these observed phases:

```text
profile-created
onboarding-started
onboarding-complete
measured-process-started
shell-idle
popover-open-requested
popover-content-ready
popover-visible-settled
popover-hidden
process-exited
```

The measured samples belong only to `popover-visible-settled`.

The runner must prove:

- Steve can identify one antiburn status item.
- The status-item action opens a window anchored beside that item.
- The application accessibility tree exposes `Sessions`.
- The page-load diagnostic supplies one live WebContent PID.
- The measured process identity remains stable for every sample.
- The second status-item action hides the popover.
- The application exits without a forced kill.

The runner stops the unmeasured onboarding process after its settings write.
For the measured process, it can use a forced kill only during failure cleanup.
A forced measured-process cleanup marks the run failed.

---

## 6. Implementation Phases

### Phase 1: Replace the plan and prove Steve control

- [x] Replace the native-controller plan with this black-box plan.
- [x] Install Steve 0.5.1 from the maintained upstream Homebrew tap.
- [x] Grant and verify Accessibility permission.
- [x] Confirm PID-scoped `steve elements` finds the framed status item.
- [x] Confirm `steve click-at` opens the correctly anchored popover.
- [x] Confirm Steve can wait for the `Sessions` landmark by shell PID.

Exit gate: a manually launched normal popover opens and becomes accessible
without any native probe action.

### Phase 2: Reduce native instrumentation

- [x] Delete the stdin protocol and control thread.
- [x] Remove probe-specific startup, exit, subsystem, Settings, readiness, and
      popover-control behavior.
- [x] Keep deterministic rows with a 500-row bound.
- [x] Add a page-load-only WebContent PID diagnostic.
- [x] Keep `memory-probe` incompatible with `distribution`.
- [x] Add focused Rust tests for fixture determinism and diagnostic encoding.

Exit gate: a probe-enabled build follows the same window lifecycle as a normal
build and emits the exact PID after a real status-item click.

### Phase 3: Simplify the Node runner

- [x] Remove protocol framing, request dispatch, generic scenarios, Settings,
      transcript, DOM-control, worker, and eviction code.
- [x] Make popover with 225 rows the default command.
- [x] Add Steve discovery, permission, status-item, wait, and quit
      adapters with JSON parsing and actionable failures.
- [x] Add unmeasured onboarding preparation through accessible controls.
- [x] Parse the prefixed PID diagnostic from application output.
- [x] Keep safe profiles, memory parsers, summaries, formats, and cleanup.
- [x] Verify process identity before and after every sample.
- [x] Keep pure Node tests in CI.

Exit gate: one debug run completes from profile creation through normal quit and
produces shell, WebContent, and aggregate samples.

### Phase 4: Document operation

Create `docs/runbooks/memory-reporting.md` and update the desktop and contributor
documentation.

The documentation must include:

- [x] macOS 13+ and logged-in GUI requirements.
- [x] Steve upstream, pinned version, Homebrew install, and Accessibility setup.
- [x] Why Linux, Windows, SSH-only, and headless CI are unsupported.
- [x] The exact release and existing-build commands.
- [x] Profile isolation, synthetic data, cleanup, and privacy guarantees.
- [x] The native feature boundary and private WebKit PID selector.
- [x] RSS, physical footprint, aggregate totals, runs, samples, and medians.
- [x] Before-and-after comparison on the same machine and OS version.
- [x] Troubleshooting for permissions, hidden status items, duplicate app
      instances, status-item matching, missing PID diagnostics, and `footprint`.
- [x] Why only pure parser/report tests run in CI.

Exit gate: a maintainer unfamiliar with the code can install Steve, grant the
required macOS permission, run the report, and interpret it from the docs.

### Phase 5: Capture the pre-change baseline

- [ ] Run zero-row diagnostic measurements.
- [x] Run the 225-row release acceptance fixture for five independent runs.
- [ ] Run a 500-row stress measurement.
- [x] Keep canonical JSON reports outside temporary profiles.
- [x] Record machine, macOS, WebKit/build, Git, and variance information.
- [x] Confirm no optimization code changed before the baseline.

Exit gate: the same commands and fixture seed can be repeated after the UI
changes.

### Phase 6: Virtualize `SessionList`

- [x] Use the smallest suitable virtualization implementation or dependency.
- [x] Flatten day headings and rows into one stable keyed virtual sequence.
- [x] Measure variable row and heading heights.
- [x] Use a small documented overscan.
- [x] Preserve the 500-row input bound, grouping, pinned day, fade, scroll
      restoration, click, Enter, Space, focus, and accessibility behavior.
- [x] Add no `useEffect`.

Tests must prove bounded mounting at 225 and 500 rows, first-to-last scrolling,
non-overlapping variable rows, pinned-heading changes, restoration, and keyboard
activation.

### Phase 7: Use one shared list tooltip

- [x] Reuse current status and cost tooltip content.
- [x] Render row triggers without private tooltip roots.
- [x] Delegate pointer and focus handling at the list boundary.
- [x] Render one controlled tooltip surface.
- [x] Preserve accessible names, descriptions, delay, placement, Escape, blur,
      unmount, and surface-change behavior.
- [x] Keep standalone tooltips outside `SessionList` unchanged.
- [x] Add no `useEffect`.

Tests must prove constant tooltip component count and correct content for every
row tooltip kind.

### Phase 8: Isolate the list from height animation

- [x] Keep Usage mounted during the 780-to-700 contraction.
- [x] Mount Activity only after the winning resize reaches its target.
- [x] Ignore stale resize completions.
- [x] Keep reduced-motion resizing immediate.
- [x] Preserve warm renderer reuse and scroll restoration.

Tests must prove that no session rows exist during contraction and that stale
requests cannot mount the wrong surface.

### Phase 9: Final measurement and validation

- [ ] Repeat the exact zero, 225, and 500-row baseline commands.
- [x] Use the same machine, OS, build profile, sample count, run count, and seed.
- [x] Compare raw values, run medians, run peaks, and accessibility diagnostics.
- [x] Confirm every 225-row run median is below 100 MiB.
- [x] Run frontend format, lint, type checks, tests, and build.
- [x] Run shell format, Clippy, and tests with and without `memory-probe`.
- [x] Run Node tests, design drift, notices, secrets, and `aislop scan --changes`.

Exit gate: issue #237 meets its target, or the retained reports identify the
remaining measured blocker.

---

## 7. Report Interpretation

The canonical JSON report contains configuration, platform details, phase
timings, process identities, raw samples, per-run summaries, cross-run
summaries, warnings, and failures.

Per run and process role, report count, minimum, median, p95, maximum, mean, and
standard deviation. Across runs, summarize run medians and run peaks.

RSS is useful for quick diagnostics but includes shared resident pages in a
simple sum. The acceptance value is the WebContent process `phys_footprint`
reported by macOS `footprint`. The multi-PID aggregate is diagnostic and is not
the sum of individual physical footprints.

Results are comparable only on the same machine, macOS version, build profile,
fixture, and measurement configuration. A different WebKit or operating-system
build can change the baseline.

---

## 8. Risks

| Risk                                                | Mitigation                                                                 |
| --------------------------------------------------- | -------------------------------------------------------------------------- |
| Steve lacks Accessibility permission                | Check before launch and print the exact System Settings path.              |
| Steve cannot identify the status item               | Inspect PID-scoped `elements`; require one framed menu-bar item.           |
| `AXPress` does not trigger Tauri                    | Fall back to a real coordinate click at the reported status-item frame.    |
| Another antiburn instance owns the matched item     | Require no other instance and target all app waits by exact shell PID.     |
| WebKit replaces the content process                 | Verify process identity around every sample and fail if it changes.        |
| The private selector changes                        | Fail when no positive PID is emitted; never guess by process name.         |
| `footprint` perturbs memory                         | Settle first, use a small fixed sample count, and compare identical runs.  |
| Virtualization breaks keyboard or screen-reader use | Preserve semantic grouping and test focus and activation across ranges.    |
| A tooltip targets a recycled row                    | Key active tooltip state to row and tooltip identity; close it on unmount. |
| Height completion races                             | Give each resize request ownership and ignore stale completion.            |

---

## 9. Progress

| Phase                   | Status      | Evidence                            |
| ----------------------- | ----------- | ----------------------------------- |
| 1. Plan and Steve spike | Complete    | Live status-item control proved     |
| 2. Minimal native hooks | Complete    | Rust checks and focused tests pass  |
| 3. Steve-based runner   | Complete    | End-to-end debug smoke passes       |
| 4. Documentation        | Complete    | Runbook and contributor links added |
| 5. Baseline             | Not started |                                     |
| 6. Virtualized list     | Not started |                                     |
| 7. Shared tooltip       | Not started |                                     |
| 8. Height isolation     | Not started |                                     |
| 9. Final validation     | Not started |                                     |

Final values:

| Measurement                                     |       Before |   After |                 Target |
| ----------------------------------------------- | -----------: | ------: | ---------------------: |
| 225-row maximum run-median WebContent footprint |      Pending | Pending |              < 100 MiB |
| 225-row mounted session rows                    | 225 expected | Pending | Viewport plus overscan |
| Shared list tooltip roots                       |      Pending | Pending |                      1 |

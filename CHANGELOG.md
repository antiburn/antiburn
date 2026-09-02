# Changelog

Changes to the **antiburn desktop application**, the thing an `antiburn-v*` tag
releases. The engine crate keeps its own changelog in
[`crates/antiburn-local/CHANGELOG.md`](crates/antiburn-local/CHANGELOG.md),
released separately under `antiburn-local-v*`.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
versions follow [semantic versioning](https://semver.org/spec/v2.0.0.html).

**This file is load-bearing.** `.github/workflows/release-app.yml` reads the
section matching the tagged version and refuses the release if there is none:
the section becomes the release notes, the body of the update prompt inside the
app, and the `notes` field of `latest.json`. Write it for the reader who is
about to be asked to install something, not for the commit log.

Entries describe what changed for a person using antiburn. Internal refactors,
CI changes, and documentation that no user acts on stay out — see
`docs/runbooks/release.md`.

## [Unreleased]

### Changed

- Context charts now distinguish provider cache misses from cache rehydration
  after meaningful user inactivity, using provider-specific timing.

## [0.3.2] - 2026-09-02

### Added

- Hovering or focusing a provider limit in the tray popover now opens its
  detailed usage view beside the originating row.
- Native Antigravity IDE and `agy` sessions now receive local analysis with
  token, model, cache, retry, timing, and Insights evidence.
- Settings now exports a bounded diagnostic report that omits transcript
  content, titles, paths, repository names, and account identifiers.
- The terminal installer now opens with an animated antiburn wordmark on
  supported terminals while the release downloads.

### Changed

- Provider usage is now grouped by account for Antigravity, OpenCode, and Pi.
  Antigravity can also show Google plan limits from the current IDE or CLI login.
- Model pricing now refreshes from models.dev at startup and hourly, so new
  models can receive cost estimates without an application update. The last
  valid catalog remains available when the service cannot be reached.
- Background scans now continue while the popover is hidden, avoid rereading
  unchanged session sources, and show the current session list immediately when
  the popover opens.

### Fixed

- Clicking the tray icon now dismisses the current notification nudge before
  opening or closing the popover.
- Long model and thinking-mode lists no longer extend beyond session cards.
- The macOS terminal installer no longer requests administrator access when the
  destination is already writable.

## [0.3.1] - 2026-09-01

### Fixed

- macOS disk images now open with the designed Finder background, window
  layout, and application shortcut.
- Hovering a notification nudge no longer closes the open popover or restores
  focus after the reader changes applications or opens notification settings.

## [0.3.0] - 2026-09-01

### Added

- Settings now offers configurable session-data retention and removes expired
  local transcript-derived data automatically.
- The usage HUD remembers its position on each display.
- Session detail now expands Efficiency and Hygiene into evidence breakdowns,
  and its context chart identifies context rewrites.
- Onboarding now shows agent discovery and analysis progress, previews session
  hygiene, and explains notification and Do Not Disturb behavior before setup
  completes.
- Onboarding repository selection now includes one All repos switch for quickly
  enabling or disabling every discovered repository.
- On macOS, source discovery now includes repositories under `~/Developer`.

### Changed

- Long activity lists now retain less memory while preserving keyboard
  navigation, grouping, pinned headings, and session tooltips.
- The app opens its popover without taking focus from the frontmost application.
- The main popover now stays ready after its first use, so reopening it reuses
  the hidden renderer instead of rebuilding the window after 15 seconds.
- Signed application updates now download, install, and restart antiburn
  automatically. The automatic-update setting remains available as an opt-out.
- Session analysis now persists bounded turn rows and serves the last completed
  result while a newer pass runs. Drilldowns stay available during refreshes
  and use less repeated transcript parsing.
- Insights now recognizes more Claude, Codex, OpenCode, and Pi thread and
  sub-agent relationships. Model, speed, cache, compaction, and repeated-context
  findings use the expanded evidence and fail closed when required facts are
  missing.
- Codex cache-write usage and service tiers now contribute to the correct
  efficiency and model-policy results.

## [0.3.0-rc.1] - 2026-09-01

This is a release-candidate rehearsal build for user acceptance testing.

### Added

- Settings now offers configurable session-data retention and removes expired
  local transcript-derived data automatically.
- The usage HUD remembers its position on each display.
- Session detail now expands Efficiency and Hygiene into evidence breakdowns,
  and its context chart identifies context rewrites.
- On macOS, source discovery now includes repositories under `~/Developer`.

### Changed

- Signed application updates now download, install, and restart antiburn
  automatically. The automatic-update setting remains available as an opt-out.
- Session analysis now persists bounded turn rows and serves the last completed
  result while a newer pass runs. Drilldowns stay available during refreshes
  and use less repeated transcript parsing.
- Insights now recognizes more Claude, Codex, OpenCode, and Pi thread and
  sub-agent relationships. Model, speed, cache, compaction, and repeated-context
  findings use the expanded evidence and fail closed when required facts are
  missing.
- Codex cache-write usage and service tiers now contribute to the correct
  efficiency and model-policy results.

## [0.2.0] - 2026-08-28

### Added

- OpenCode sessions now receive dedicated local analysis and Insights without
  rendering the provider database into an in-memory transcript.
- Pi sessions now receive dedicated local analysis and Insights on supported
  macOS and Linux installations.

### Changed

- **Sessions containing record types antiburn does not recognise are now assessed again.** A new housekeeping record from a coding agent used to make a whole session's evidence incomplete, which held back results and session badges. antiburn now proves a skipped record carries no usage data before assessing the session, names unrecognised types in Insights coverage, and still declines when a skipped record could carry usage data or exceeds a bounded type limit. Insights results and badges refresh as sessions are re-read.
- **An anonymised event now reports when Insights encounters unknown record formats.** When analytics are on, opening Settings → Insights can send `antiburn.unrecognized_records_observed` with a rounded session-count bucket and one of three fixed outcome labels. Unknown type names never leave the device. `docs/analytics.md` contains the full catalog entry.
- Analysing a very large session now uses far less memory. Long charts group
  nearby activity, while crowded detail tables prioritize useful entries and
  summarize overflow.
- Codex session analysis now refreshes when a sub-agent transcript changes,
  even when the parent transcript stays unchanged.
- Delegated and sidechain turns recorded inside a session transcript now count
  as sub-agent tokens instead of main-thread tokens. This can correct the
  reported context-window tier for affected sessions.

## [0.1.0] - 2026-08-27

The first supported public release. The macOS app is Developer ID signed and
notarized. The Windows installer is unsigned, so Windows can show a SmartScreen
warning.

### Added

- Local-first discovery and analysis for supported AI coding-agent sessions,
  with session content kept on the device.
- Provider usage, activity, session insights, and API-equivalent cost estimates
  in a desktop menu-bar or tray application.
- Signed in-app updates and checksum-verified installers for macOS, Linux, and
  Windows.

### Changed

- The source repository and release line are now public under the MIT License.

## [0.1.0-rc.10] - 2026-08-27

A release-candidate rehearsal build, not a supported release. The macOS app is
Developer ID signed and notarized. The Windows installer is **unsigned**, so
Windows can show a SmartScreen warning.

### Added

- **Updates can now be installed from Settings.** Settings → About shows
  download progress, verifies the signed update, and offers a restart when the
  new version is ready. Failed or interrupted updates stay visible and can be
  retried safely.

- **One-command installers are available for macOS, Linux, and Windows.** The
  release includes `install.sh` and `install.ps1`; each selects the correct
  package, verifies its SHA-256 checksum, and upgrades the existing
  installation without leaving a partial replacement on failure.

- **Session detail shows the cost of skills, MCP servers, built-in tools, and
  sub-agents.** The Skills, MCPs and tools table identifies loaded, used,
  unused, and deferred context sources. Expand the sub-agent cost row to see
  each agent's model, tokens, cost, timing, and share of the session total.

- Automated notifications now respect Focus and Do Not Disturb where the
  operating system exposes that state. Suppressed notifications are dropped
  instead of appearing later as a stale backlog.

### Changed

- Provider usage headings now show the detected subscription plan, such as
  Claude Max 5x or Codex Pro.

- Session-analysis cards explain their context, cost, and efficiency measures
  more clearly. Cache misses during a fast tool burst no longer appear as
  avoidable cache rehydration.

### Fixed

- The Context chart appears fully drawn when it first opens while retaining
  transitions for later live updates.

- Collapsed provider-limit rings show their percentage again, or an em dash
  when the provider reports no percentage.

## [0.1.0-rc.9] - 2026-08-27

A release-candidate rehearsal build, not a supported release. The macOS app is
Developer ID signed and notarized. The Windows installer is **unsigned**, so
Windows can show a SmartScreen warning.

### Added

- **Updates can now be installed from Settings.** Settings → About shows
  download progress, verifies the signed update, and offers a restart when the
  new version is ready. Failed or interrupted updates stay visible and can be
  retried safely.

- **One-command installers are available for macOS, Linux, and Windows.** The
  release includes `install.sh` and `install.ps1`; each selects the correct
  package, verifies its SHA-256 checksum, and upgrades the existing
  installation without leaving a partial replacement on failure.

- **Session detail shows the cost of skills, MCP servers, built-in tools, and
  sub-agents.** The Skills, MCPs and tools table identifies loaded, used,
  unused, and deferred context sources. Expand the sub-agent cost row to see
  each agent's model, tokens, cost, timing, and share of the session total.

- Automated notifications now respect Focus and Do Not Disturb where the
  operating system exposes that state. Suppressed notifications are dropped
  instead of appearing later as a stale backlog.

### Changed

- Provider usage headings now show the detected subscription plan, such as
  Claude Max 5x or Codex Pro.

- Session-analysis cards explain their context, cost, and efficiency measures
  more clearly. Cache misses during a fast tool burst no longer appear as
  avoidable cache rehydration.

### Fixed

- The Context chart appears fully drawn when it first opens while retaining
  transitions for later live updates.

- Collapsed provider-limit rings show their percentage again, or an em dash
  when the provider reports no percentage.

## [0.1.0-rc.8] - 2026-08-25

A release-candidate rehearsal build, not a supported release. The macOS and
Windows binaries are **unsigned** — your operating system will warn you, and it
is right to. Install it only if you are testing the release pipeline itself.

On macOS, open the app for the first time with right-click → Open rather than a
double-click. The build is sealed but has no Developer ID signature, so macOS
shows its unidentified-developer warning.

### Added

- **A floating usage HUD keeps provider limits on screen on macOS.** Turn it on
  from Settings → Usage, drag it to an edge, and hover for the detailed limit
  windows. The HUD sizes itself to visible content, stays out of the way of
  desktop clicks, and keeps its Settings control in sync when closed directly.

- **Session detail explains how the work used context and money.** The Context
  chart now combines token flow with cache activity, compactions, sub-agent
  work, model and effort changes, and meaningful labels for gaps between model
  calls. Cost rows show token and spend shares, while the new Efficiency card
  separates spend on new work from cached carry and rewritten input.

- **First-run setup can be run again.** Settings → General now restarts setup
  without deleting indexed sessions, repositories, permissions, or preferences.

### Changed

- **Provider limits take less space in the popover.** Each provider gets a
  compact ring for its fullest live window; expand the bar for every reported
  window, its elapsed-time marker, and its reset time.

- **Recent Activity carries more useful session context.** Rows lead with the
  repository and title, show the models used, and include the current prototype
  hygiene indicators. The detail pane stays current while an active session is
  open instead of requiring a close and reopen.

- **Desktop windows open ready instead of flashing unfinished content.**
  Settings is prepared after setup, and the popover is created on demand behind
  a renderer-readiness gate. The popover footer now shows the running version
  and links the antiburn name to the project.

- Browser-only context menus and navigation shortcuts no longer appear inside
  release webviews. Normal keyboard focus traversal remains available.

### Fixed

- **Codex forks now show their relationship in Recent Activity.** antiburn
  reads Codex's declared parent session during discovery, so both the fork and
  its parent show the correct relationship without an extra navigation step.

- **Codex sessions show their short task names again.** Recent Activity now
  prefers Codex's generated title instead of displaying the full opening
  request when both are present.

- **Provider readings survive temporary failures and stay fresh in the
  background.** A provider that fails on cold start remains visible with a
  useful status, the last good reading remains available during later failures,
  and HUD and popover data no longer wait for the popover to open before they
  refresh. A reported Claude value of 1% is no longer rounded away.

- **Session totals and labels stay consistent after analysis.** Sub-agent spend
  and token series roll into the parent where appropriate, model labels refresh
  after analysis, and Codex resume records no longer count the same usage twice.

- Settings no longer opens as a blank window, recent-session scroll position is
  restored after returning from detail, and the running-session indicator is
  visible in dark mode.

- Notification windows, the macOS popover, and the floating HUD now keep the
  correct shape, position, and visibility across repeated opens and closes.

## [0.1.0-rc.7] - 2026-08-21

A release-candidate rehearsal build, not a supported release. The macOS and
Windows binaries are **unsigned** — your operating system will warn you, and it
is right to. Install it only if you are testing the release pipeline itself.

On macOS, open the app for the first time with right-click → Open rather than a
double-click. The build is sealed but has no Developer ID signature, so macOS
shows its unidentified-developer warning.

### Changed

- **The main popover stays ready after its first use.** Opening antiburn again
  reuses the existing hidden popover instead of rebuilding its webview, so the
  main surface returns immediately.

- **Recent Activity is simpler to scan.** Session rows now focus on the agent,
  repository, recency, and estimated cost, with less visual separation and no
  duplicate duration measure.

- **The macOS installer now has a branded, readable layout.** The disk image
  shows the antiburn app and Applications folder on a light background with
  clear labels.

### Fixed

- **Plan limits no longer hold up the useful part of the popover.** antiburn
  shows its last successful provider reading while it refreshes that reading in
  the background. The activity list and saved limits now appear together,
  without a blank route or a second layout change before the limits arrive.

- **Linux tray interactions now open in the right place.** The tray menu opens
  the popover at the panel instead of the pointer, and notification cards size
  themselves to their content.

## [0.1.0-rc.6] - 2026-08-20

This release candidate supersedes rc.5, which was not published after its
embedded engine source was found not to have a matching component release tag.
It contains the same tested application behavior with the engine correctly
identified as the separately released `antiburn-local` 0.1.3.

A release-candidate rehearsal build, not a supported release. The macOS and
Windows binaries are **unsigned** — your operating system will warn you, and it
is right to. Install it only if you are testing the release pipeline itself.

On macOS, open the app for the first time with right-click → Open rather than a
double-click. The build is sealed but has no Developer ID signature, so macOS
shows its unidentified-developer warning.

### Added

- **Launch antiburn when you sign in.** New installs opt in by default from the
  final setup step so the menu-bar or tray utility is available without being
  opened by hand. The same switch is available later in Settings → General,
  and can be turned off before setup finishes.

- **What "local" means, spelled out.** antiburn needs no antiburn account,
  server, or backend — everything runs on your machine, as you. The
  connections it makes are yours: reading your provider's own current usage
  figures with your own credentials, traffic between this machine and a
  provider you already use. Its one call to a service of ours — checking
  whether a newer version exists — is a convenience it never depends on, and
  it hands your data to no one who doesn't already have it.

### Changed

- **Plan limits now live at the top of the main popover.** Expand the Limits
  section for each provider's windows and reset times, or collapse it to a row
  of compact percentage chips. Each chip's ring follows that provider's
  fullest reported limit, so the most urgent allowance stays visible.

- **A per-model weekly limit stays out of the way until you use that
  model.** Your provider can report a supplemental weekly allowance scoped to
  one model alongside your account-wide limits. Most readers never touch that
  model, so its row used to sit at the bottom of the Usage screen reading 0%
  forever. It is now hidden until you actually use the model it tracks, and
  once it shows real usage it stays on screen for the rest of that week —
  even past a reading that comes back without a percentage — so a limit you
  are genuinely drawing on never looks like it disappeared.

- **Plan limits are now fetched directly, not read from a file your agent
  happened to cache — and that runs by default.** Settings → Usage's switch
  asks each provider's own usage endpoint directly, with the credential your
  coding tool already keeps on this machine — on macOS, that is usually the
  same Keychain item the Claude CLI itself reads and writes, with its
  credentials file as a fallback — instead of running your coding agent in the
  background and reading the file it wrote. If Codex's endpoint can't be
  reached directly, antiburn falls back to asking the local `codex app-server`
  process the same question over its own protocol, rather than showing
  nothing. This is your own connection, made as you, to a provider you already
  use, with a credential you already hold — ordinary traffic, not something
  antiburn asks permission for — so the switch is on by default once first-run
  setup is complete, and no source runs a moment before that. One switch in
  Settings → Usage turns all of it off, for anyone who wants no background
  traffic at all; with it off, antiburn has no plan limits to show, since there
  is no longer a cached file it reads on its own.

- **Hidden utility windows no longer remain resident indefinitely.** The
  popover, Settings, setup, and notification views are released when they are
  no longer needed and recreated on demand, substantially reducing the app's
  idle memory footprint.

### Fixed

- **Recent Activity follows real transcript activity.** Background file
  touches, title changes, permission changes, and other agent housekeeping no
  longer make an idle session look newly active. Subagent work still counts
  toward its parent session.

- **Native and WSL session analytics stay separate.** Sessions with the same
  agent identifier can no longer reuse one another's cached analysis across
  those environments.

- Settings panes return to the top when you move between them, instead of
  keeping the previous pane's scroll position.

- **Session names in recent activity.** antiburn now reads authoritative names
  from each agent's indexed session store when available, while keeping mounted
  WSL sessions isolated from native stores. Renames appear on the next scan,
  and a missing title can no longer leave mismatched title provenance behind.

## [0.1.0-rc.5] - 2026-08-20

A release-candidate rehearsal build, not a supported release. The macOS and
Windows binaries are **unsigned** — your operating system will warn you, and it
is right to. Install it only if you are testing the release pipeline itself.

On macOS, open the app for the first time with right-click → Open rather than a
double-click. The build is sealed but has no Developer ID signature, so macOS
shows its unidentified-developer warning.

### Added

- Anonymised usage analytics about the application itself, on by default with
  the control in Settings → Privacy, and can be turned off for one launch with
  `ANTIBURN_ANALYTICS_ENABLED=false`. Events start from the first launch, so
  first-run setup itself is counted, one event per step. They carry thirteen
  fields — platform, a per-message id, a rotating installation
  identifier, an in-memory identifier for one run of the app, the event name,
  capture and delivery timestamps, architecture, a bucketed count, a
  closed-vocabulary label, a second such label where an event has two things
  worth telling apart, app version, and operating system — and never
  sessions, prompts, titles, file paths, repository names, token counts, costs,
  or credentials. Settings → Privacy enumerates all of them, and
  `docs/analytics.md` carries the full catalog plus the commands to check any of
  it yourself. The automatic default is the same in every locale, and a build
  with no endpoint injected sends nothing at all.
- **Launch antiburn when you sign in.** New installs opt in by default from the
  final setup step so the menu-bar or tray utility is available without being
  opened by hand. The same switch is available later in Settings → General,
  and can be turned off before setup finishes.

- **What "local" means, spelled out.** antiburn needs no antiburn account,
  server, or backend — everything runs on your machine, as you. The
  connections it makes are yours: reading your provider's own current usage
  figures with your own credentials, traffic between this machine and a
  provider you already use. Its one call to a service of ours — checking
  whether a newer version exists — is a convenience it never depends on, and
  it hands your data to no one who doesn't already have it.

### Changed

- **Plan limits now live at the top of the main popover.** Expand the Limits
  section for each provider's windows and reset times, or collapse it to a row
  of compact percentage chips. Each chip's ring follows that provider's
  fullest reported limit, so the most urgent allowance stays visible.

- **A per-model weekly limit stays out of the way until you use that
  model.** Your provider can report a supplemental weekly allowance scoped to
  one model alongside your account-wide limits. Most readers never touch that
  model, so its row used to sit at the bottom of the Usage screen reading 0%
  forever. It is now hidden until you actually use the model it tracks, and
  once it shows real usage it stays on screen for the rest of that week —
  even past a reading that comes back without a percentage — so a limit you
  are genuinely drawing on never looks like it disappeared.

- **Plan limits are now fetched directly, not read from a file your agent
  happened to cache — and that runs by default.** Settings → Usage's switch
  asks each provider's own usage endpoint directly, with the credential your
  coding tool already keeps on this machine — on macOS, that is usually the
  same Keychain item the Claude CLI itself reads and writes, with its
  credentials file as a fallback — instead of running your coding agent in the
  background and reading the file it wrote. If Codex's endpoint can't be
  reached directly, antiburn falls back to asking the local `codex app-server`
  process the same question over its own protocol, rather than showing
  nothing. This is your own connection, made as you, to a provider you already
  use, with a credential you already hold — ordinary traffic, not something
  antiburn asks permission for — so the switch is on by default once first-run
  setup is complete, and no source runs a moment before that. One switch in
  Settings → Usage turns all of it off, for anyone who wants no background
  traffic at all; with it off, antiburn has no plan limits to show, since there
  is no longer a cached file it reads on its own.

- **Hidden utility windows no longer remain resident indefinitely.** The
  popover, Settings, setup, and notification views are released when they are
  no longer needed and recreated on demand, substantially reducing the app's
  idle memory footprint.

### Fixed

- **Recent Activity follows real transcript activity.** Background file
  touches, title changes, permission changes, and other agent housekeeping no
  longer make an idle session look newly active. Subagent work still counts
  toward its parent session.

- **Native and WSL session analytics stay separate.** Sessions with the same
  agent identifier can no longer reuse one another's cached analysis across
  those environments.

- Settings panes return to the top when you move between them, instead of
  keeping the previous pane's scroll position.

- **Session names in recent activity.** antiburn now reads authoritative names
  from each agent's indexed session store when available, while keeping mounted
  WSL sessions isolated from native stores. Renames appear on the next scan,
  and a missing title can no longer leave mismatched title provenance behind.

## [0.1.0-rc.4] - 2026-08-17

A release-candidate rehearsal build, not a supported release. The macOS and
Windows binaries are **unsigned** — your operating system will warn you, and it
is right to. Install it only if you are testing the release pipeline itself.

On macOS, open the app for the first time with right-click → Open rather than a
double-click. The build is sealed but has no Developer ID signature, so macOS
shows its unidentified-developer warning.

### Added

- **Your plan's limits, on the Usage screen.** When one of your agents has
  already fetched its own usage, antiburn now reads that and shows what your
  provider actually said: how much of your 5-hour and weekly allowances is
  gone, any per-model weekly limit, and when each one resets. A second mark on
  each bar shows how far through the period the clock has travelled, so 60%
  used with most of the window left reads differently from 60% used with an
  hour to go.

  antiburn still fetches nothing. These are numbers your agent collected while
  it was online and left on this machine, so each one is shown with the moment
  your provider stated it, and a reading more than an hour old says so rather
  than ageing quietly on screen. A figure your provider did not give is shown
  as unknown, never as zero.

  Your local spend estimates are unchanged and sit directly below, as before.
  If no agent has cached a reading, the limits section is simply not there.

- **How fast, and whether it will last.** Once antiburn has seen a limit move
  more than once, it says what that movement implies: the rate you are
  consuming at, whether the last half hour is faster or slower than the last
  two hours, when the allowance runs out if you carry on, and how much of a
  weekly limit went today.

  These need a series, and the readings only move when your agent refetches
  them — so most of the time antiburn will say "not enough history", and it
  says exactly that rather than showing a confident zero. A window that has
  just reset says so too, because its numbers are fine and simply too new to
  extrapolate from.

- **A ring in the popover footer.** Each provider's chip now carries a ring
  showing your account-wide limit — the weekly one, not the shortest window and
  not whichever happens to be fullest. A per-model limit can be closer to its
  ceiling without describing how you are doing overall, so it keeps its own
  named row instead. Hovering a chip shows that provider's limits and pace
  without leaving the list; clicking pins the panel open.

- **Settings → Usage, with one switch.** Off by default. On, antiburn runs
  your coding agent in the background about every ten minutes so the agent
  refreshes its own usage reading, and then reads the file the agent writes.
  Your agent goes online to do that, exactly as it does when you use it
  yourself — antiburn still opens no connection of its own, and nothing your
  agent prints is read.

  The pane also lists what antiburn can currently see, and turns a failed
  reading into something you can act on rather than a blank.

  Turning the switch on is also what lets usage milestone notifications fire,
  because a milestone is a threshold being _crossed_ and that needs readings
  that keep moving. With it off you still see your limits; antiburn just never
  interrupts you about them. The switch says both of these things.

- **Folder access asks before macOS does.** If sessions point into Documents,
  Desktop, or Downloads, antiburn explains why it needs that folder and waits
  for you to choose before it attempts a read. You can review deferred folders
  and grant them later from Settings → Sources. If access is revoked outside
  antiburn, the stale grant is noticed and the folder returns to the consent
  flow instead of being treated as readable.

- **Recognisable agent marks.** Codex and OpenAI now use their own marks, and
  the agent palette has stronger, more consistent colours wherever agents are
  identified.

- **A popover that can stay put.** The tray menu can pin the popover open while
  you work elsewhere, then unpin it to restore the usual click-away behaviour.

- **A dedicated first-run window.** Setup now opens in a regular window rather
  than borrowing the menu-bar popover. It explains that antiburn lives in the
  menu bar after setup, and clicking the menu-bar item brings setup back while
  it is unfinished.

### Changed

- The menu-bar item remains highlighted for as long as the popover is visible,
  including while it is pinned, so its open state is clear.

- The legal documents in Settings → About now open as their own pages, with a
  back arrow, instead of expanding in place and burying the rest of the pane.
  Each one is reached by an "Open" button on its row, and the attributions for
  bundled third-party material have moved out of "Legal notices" onto a row of
  their own.

### Fixed

- Pinning is unavailable until first-run setup is complete, so setup cannot be
  hidden behind an empty popover and always remains reachable from the menu
  bar.

## [0.1.0-rc.3] - 2026-08-15

A release-candidate rehearsal build, not a supported release. The macOS and
Windows binaries are **unsigned** — your operating system will warn you, and it
is right to. Install it only if you are testing the release pipeline itself.

On macOS, opening it the first time takes a right-click → Open rather than a
double-click: the build is sealed but carries no Developer ID, so the warning
is the system doing its job. If you installed rc.2 and were told the app was
damaged, this is the release that fixes it — remove the old copy first, or
clear its quarantine with
`xattr -dr com.apple.quarantine /Applications/antiburn.app`.

### Fixed

- The macOS app opens after download. Unsigned rehearsal builds are now sealed
  with an ad-hoc signature, so instead of "antiburn is damaged and can't be
  opened", macOS shows the standard unidentified-developer warning and
  right-click → Open works.

### Changed

- Installing no longer shows a licence-agreement screen on the mounted macOS
  disk image or in the Windows installer. The full licence text is readable
  inside the app.

### Added

- Settings → About gains "Legal notices" and "Licence text": the notice and
  attributions, and the complete licence, readable in place.

## [0.1.0-rc.2] - 2026-08-14

A release-candidate rehearsal build, not a supported release. The macOS and
Windows binaries are **unsigned** — your operating system will warn you, and it
is right to. Install it only if you are testing the release pipeline itself.

### Added

- First packaged build of the antiburn desktop application: the menu-bar/tray
  shell over the antiburn-local engine, discovering and analyzing the coding
  agent sessions already on your machine. Everything runs on the device;
  nothing is uploaded.
- Auto-update wiring: this build carries the updater public key and can verify
  and install future signed releases.

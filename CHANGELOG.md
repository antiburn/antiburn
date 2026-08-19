<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

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

### Added

- Anonymised usage analytics about the application itself, on by default with
  the control on the first-run Ready screen and in Settings → Privacy. Events
  carry thirteen fields — platform, a per-message id, a rotating installation
  identifier, an in-memory identifier for one run of the app, the event name,
  capture and delivery timestamps, architecture, a bucketed count, a
  closed-vocabulary label, a second such label where an event has two things
  worth telling apart, app version, and operating system — and never
  sessions, prompts, titles, file paths, repository names, token counts, costs,
  or credentials. Settings → Privacy enumerates all of them, and
  `docs/usage-analytics.md` carries the full catalog plus the commands to check
  any of it yourself. Nothing is sent until the first run completes; in the EU,
  the EEA, and the UK the control starts off rather than on; and a build with no
  endpoint injected sends nothing at all. Recorded as D-027 in
  `docs/oss/source-denylist.toml` and D-28 in `docs/deviations.md`, which also
  resolves D-5.

- **What "local" means, spelled out.** antiburn needs no antiburn account,
  server, or backend — everything runs on your machine, as you. The
  connections it makes are yours: reading your provider's own current usage
  figures with your own credentials, traffic between this machine and a
  provider you already use. Its one call to a service of ours — checking
  whether a newer version exists — is a convenience it never depends on, and
  it hands your data to no one who doesn't already have it.

### Changed

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

### Fixed

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
  because a milestone is a threshold being *crossed* and that needs readings
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

- Installing no longer shows a licence-agreement screen — neither on the
  mounted macOS disk image nor in the Windows installer. The MPL-2.0 requires
  no acceptance, and the full licence text is now readable inside the app.

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

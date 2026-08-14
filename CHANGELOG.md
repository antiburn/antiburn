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

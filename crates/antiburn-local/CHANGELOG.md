<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Changelog — antiburn-local

Changes to the local engine crate, released under `antiburn-local-v*` tags. The
desktop application has its own changelog at the repository root.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
versions follow [semantic versioning](https://semver.org/spec/v2.0.0.html).

The audience here is different from the application's: this file is read by
somebody who depends on the crate from their own code, so it states API and
behaviour changes — including anything that moves the network-free boundary,
the discovery roots, or the persistence and export contracts, each of which is
a compatibility fact rather than a feature note.

`.github/workflows/release-engine.yml` reads the section matching the tagged
version and refuses the release if there is none.

## [Unreleased]

## [0.1.1] - 2026-08-14

### Fixed

- `pricing::normalize_model_key` no longer panics on model IDs containing
  multi-byte UTF-8 characters. Model IDs come from external transcript files;
  the date-suffix check now runs on bytes and only slices at a confirmed ASCII
  hyphen boundary.
- The scan-down arm of `repositories::resolve_granted_repos` reports the
  canonical repository path in `repo_root` and `suspected_path` instead of the
  folded identity key (which lowercases and slash-normalizes on Windows). The
  key now serves only deduplication, matching the session-resolved arm and the
  documented field contract.

## [0.1.0] - 2026-08-13

### Added

- Initial public surface of the local engine, extracted as a self-contained
  crate:
  - `discovery` — local discovery of AI coding-agent sessions from documented
    files, read-only databases, and bounded WSL paths.
  - `analysis` — transcript and session analysis.
  - `repositories` — repository identity.
  - `pricing` — API-equivalent pricing.
  - `model`, `paths`, `platform` — shared local data model, filesystem roots,
    and platform handling.
  - Versioned local persistence and export contracts.
- The network-free boundary as a compatibility contract: no network or socket
  I/O, no private dependencies, and a public API that carries no
  authentication, organization, remote-sharing, enrichment, or telemetry
  concepts. Enforced mechanically by the crate's boundary test suite.

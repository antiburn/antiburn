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

Nothing released yet. The first release adds a `## [X.Y.Z] - YYYY-MM-DD`
section here, directly above this line's section, and moves the entries into it.

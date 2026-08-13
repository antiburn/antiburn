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

Nothing released yet.

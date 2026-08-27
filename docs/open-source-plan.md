---
artifact: temporary_plan
issue: GH-197
title: "Prepare Antiburn for open source"
created_at: "2026-08-27"
---

# Prepare Antiburn for open source

This plan has two passes. The first pass lands through a normal pull request.
The second pass replaces the Git history, removes this file, and force-pushes
`main`.

## Pass 1: Public source pull request

### Phase 1: Move private migration records

- Move `docs/oss/` and `docs/deviations.md` to an appropriate private location
  under `~/dev/cadence`.
- Move instructions about porting, comparing, or migrating code from Cadence to
  the same private repository.
- Remove public references to the moved files.
- Replace manifest-dependent boundary tests with direct checks for Antiburn's
  public runtime and data boundaries.
- Keep historical references to Cadence when they give useful context and do not
  expose private code or internal strategy.

### Phase 2: Clean public documentation

- Rewrite the root README for users and contributors. Remove open-source
  preparation notes and private migration details.
- Simplify `AGENTS.md` for work in the public repository.
- Review plans, runbooks, comments, tests, and changelog entries for private
  architecture, commercial strategy, private repository links, and unavailable
  Cadence implementation details.
- Keep useful Antiburn behavior, support, security, build, and release
  documentation.
- Remove the completed aislop implementation plan. Keep only justified
  suppressions for the two fixed Codex protocol constants.
- Confirm that a clean clone contains all files needed to build and test the
  engine and desktop app.

### Phase 3: Migrate the project to MIT

- Replace the root license with the MIT license.
- Use `Copyright (c) 2026 Cadence AI (Vic) Pty Ltd`.
- Change project package metadata from `MPL-2.0` to `MIT`.
- Replace project-owned MPL headers and update the README, CONTRIBUTING guide,
  About view, legal notices, SBOM generation, workflows, tests, and runbooks.
- Keep license declarations for third-party code under its existing license.
- Confirm that exact searches find no stale project-owned MPL declarations.

### Phase 4: Review dependencies and notices

- Run the Rust and frontend production dependency license inventories.
- Keep compatible dependencies under their own licenses. Do not replace a
  dependency only because the Antiburn project uses MIT.
- Add the Bitcount copyright and OFL-1.1 text to the shipped third-party notices.
- Resolve unknown dependency license metadata where practical.
- Include required notices in the source tree, application bundles, source
  archives, and in-app legal view.
- Add or update automated license checks so a new unreviewed license fails CI.

### Phase 5: Prepare the GitHub project

- Review issues, pull requests, comments, and attachments for private Cadence
  code, internal strategy, commercial plans, credentials, and private endpoints.
- Edit or remove important leaks. Home paths, session examples, and historical
  references to the Cadence name do not require cleanup by themselves.
- Add issue and pull-request templates with a warning not to submit credentials
  or private session content.
- Add a Code of Conduct and CODEOWNERS.
- Organize labels for area, platform, priority, and contributor work.
- Set the repository description, homepage, topics, and organization profile.

#### README badges

Use one compact badge row near the title. This set follows common Tauri app
READMEs and shows project health, availability, adoption, and code quality:

```markdown
[![CI](https://github.com/antiburn/antiburn/actions/workflows/ci.yml/badge.svg)](https://github.com/antiburn/antiburn/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/antiburn/antiburn?include_prereleases&sort=semver)](https://github.com/antiburn/antiburn/releases/latest)
[![License](https://img.shields.io/github/license/antiburn/antiburn)](LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/antiburn/antiburn)](https://github.com/antiburn/antiburn/stargazers)
[![aislop score](https://badges.scanaislop.com/score/antiburn/antiburn.svg)](https://scanaislop.com/antiburn/antiburn)
[![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)](https://tauri.app/)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-informational)](docs/support.md)
```

- Run a public aislop scan after the repository opens so the score badge has a
  current result.
- Consider a total-download badge after publication:
  `https://img.shields.io/github/downloads/antiburn/antiburn/total`. GitHub counts
  every downloaded release asset, including updater and metadata files, so omit
  it if the number suggests installer downloads that did not occur.
- Do not add separate Rust, React, TypeScript, Node, pnpm, or dependency-version
  badges. They add visual noise and package files already provide those versions.
- Use the default flat Shields style for a consistent row.

### Phase 6: Verify the pull request

- Run formatting, lint, type checks, frontend tests, builds, Rust tests, Clippy,
  dependency license checks, and `pnpm run slop:all`.
- Run `pnpm run secrets` on the final tree and inspect release payloads.
- Test the documented setup from a clean clone.
- Confirm that `docs/oss/`, `docs/deviations.md`, and private migration
  instructions are absent.
- Confirm that no private code or important internal strategy is present.
- Require a full-tree 100/100 aislop score on `main`. Pull requests gate their
  changed files.
- Merge the pull request only after all checks pass.

## Pass 2: Git history cleanup

Complete this pass separately after the public-source pull request is merged.
Do not include it in the pull request above.

### Phase 7: Replace the history and publish

- Confirm that `main` contains the merged public-source changes and has no
  uncommitted work.
- Keep a local backup of the old repository state.
- Create one orphan root commit from the final `main` tree.
- Delete this temporary plan file before creating the root commit.
- Sign off the root commit and use a clear initial public commit message.
- Replace `main` with the new root and force-push it.
- Delete obsolete remote branches, tags, and releases that retain the old
  migration records.
- Ask contributors to reclone before they push more work.
- Cut the next release from the new public root.
- Make the repository public and enable branch protection, required CI, secret
  scanning, push protection, Dependabot alerts, and private vulnerability
  reporting.
- Verify the public repository and release from an unauthenticated clean clone.

This file must not exist in the rewritten public root.

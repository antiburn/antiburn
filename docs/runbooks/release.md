<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Cutting a release

How a version of antiburn gets from a commit to something a reader can install.

There are two release trains, tagged separately and released separately:

| Train | Tag | Workflow | What it produces |
| --- | --- | --- | --- |
| Desktop application | `antiburn-v<version>` | [`release-app.yml`](../../.github/workflows/release-app.yml) | Installers, updater bundles, signatures, checksums, inventories, provenance, `latest.json` |
| Engine crate | `antiburn-local-v<version>` | [`release-engine.yml`](../../.github/workflows/release-engine.yml) | A source tarball, checksums, an inventory, provenance |

Both are **draft-first**. The workflow builds, signs, hashes, attests, and
drafts; a person reads the draft and presses Publish. There is no auto-publish
and there will not be one — the review is the point, not a formality on the way
to it.

GitHub Releases is the only place antiburn's artifacts live. There is no
separate download host, no object store, and no content-delivery layer: the
release page is the canonical artifact host and the updater host at once.

---

## Part 1 — One-time repository setup

None of this exists yet. The release workflows are written against it and will
fail loudly (and early, before building anything) if it is missing, which is the
intended behaviour: an unconfigured repository should not be able to produce
something that looks like a signed release.

### 1.1 The `release` environment

Create an environment named exactly **`release`** (Settings → Environments).
Every signing credential lives here rather than in repository secrets, so the
only jobs that can reach them are the ones that ask for the environment by name
— in this repository, the four `build` jobs of `release-app.yml`.

Configure it as:

- **Deployment branches and tags:** *Selected branches and tags* → add the tag
  rule `antiburn-v*`. Nothing else can start a job that touches these secrets.
- **Required reviewers:** optional. The draft-then-publish step is already a
  human gate; add reviewers here as well if you want the pause to happen
  *before* the credentials are used rather than after.
- **Wait timer:** not needed.

Fork pull requests can never reach this: the release workflows have no
`pull_request` trigger at all, and their first job refuses to run unless
`github.repository` is this repository.

### 1.2 Secrets

Add these to the **`release` environment** (not to repository-wide secrets).
Placeholders below show the shape, never a real value.

| Secret | Required | What it is | How to produce it |
| --- | --- | --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | **Always** | The updater's private signing key. Signs every updater bundle; the app verifies against the public half compiled into it. | `pnpm --filter @antiburn/desktop exec tauri signer generate -w ./antiburn.key`, then paste the contents of `antiburn.key`. Placeholder: `dW50cnVzdGVkIGNvbW1lbnQ6…` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | If the key has one | The passphrase for the above. Give the key a passphrase. | Chosen when generating the key. Placeholder: `<passphrase>` |
| `APPLE_CERTIFICATE` | For signed macOS builds | Base64 of the **Developer ID Application** certificate and its private key, exported as `.p12`. | `base64 -i DeveloperID.p12 \| pbcopy`. Placeholder: `MIIM…` |
| `APPLE_CERTIFICATE_PASSWORD` | With the above | The `.p12` export passphrase. | Chosen during export. Placeholder: `<passphrase>` |
| `APPLE_ID` | For notarization | The Apple ID that owns the notarization submission. | Placeholder: `releases@example.org` |
| `APPLE_PASSWORD` | For notarization | An **app-specific password** for that Apple ID — never the account password. | appleid.apple.com → Sign-In and Security → App-Specific Passwords. Placeholder: `abcd-efgh-ijkl-mnop` |
| `APPLE_TEAM_ID` | For notarization | The ten-character Apple Developer team identifier. | Apple Developer → Membership. Placeholder: `ABCDE12345` |
| `WINDOWS_CERTIFICATE` | For signed Windows builds | Base64 of the Authenticode code-signing certificate exported as `.pfx`. | `base64 -w0 codesign.pfx`. Placeholder: `MIIM…` |
| `WINDOWS_CERTIFICATE_PASSWORD` | With the above | The `.pfx` export passphrase. | Chosen during export. Placeholder: `<passphrase>` |

`GITHUB_TOKEN` is provided by Actions; it is not configured and must not be
replaced by a personal access token.

**The updater key is not optional.** `release-app.yml` fails immediately if
`TAURI_SIGNING_PRIVATE_KEY` is absent, and it fails *before that* if
`plugins.updater.pubkey` in `apps/desktop/src-tauri/tauri.conf.json` is still
empty. Both halves have to exist for an update to be verifiable, and an update
that cannot be verified is worse than no updater at all. See
[`updater-key-recovery.md`](updater-key-recovery.md) for custody.

### 1.3 Repository variables

Variables (Settings → Secrets and variables → Actions → Variables), not secrets:

| Variable | Default when unset | Effect |
| --- | --- | --- |
| `ALLOW_UNSIGNED_MACOS` | unset (= build fails without Apple credentials) | `true` builds macOS artifacts **without** Developer ID signing or notarization. Gatekeeper refuses these on a normal machine. Only for a deliberate, clearly-labelled build. |
| `ALLOW_UNSIGNED_WINDOWS` | unset (= build fails without a certificate) | `true` builds the Windows installer **without** an Authenticode signature. SmartScreen warns on download. |
| `WINDOWS_TIMESTAMP_URL` | `http://timestamp.digicert.com` | RFC 3161 timestamp authority used when signing the installer, so signatures outlive the certificate. |

The two `ALLOW_UNSIGNED_*` variables exist because antiburn refuses to fake a
signature. Until the certificates exist, the honest options are "no release" or
"a release that says in as many words that it is unsigned" — not "a release that
looks signed". This is recorded as a deviation (D-16) in
[`docs/deviations.md`](../deviations.md); delete the variables the day the
certificates land.

### 1.4 Tag protection

The tag is what authorizes a signed build, so it needs to be at least as
protected as the default branch. Settings → Rules → Rulesets → **New tag
ruleset**:

- Target tags: `antiburn-v*` and `antiburn-local-v*`
- **Restrict creations** — bypass list: the maintainers who cut releases
- **Restrict updates** and **Restrict deletions** — no bypass. A published tag
  never moves and is never deleted.

Also confirm, under Settings → Actions → General:

- Workflow permissions: **Read repository contents and packages permissions**
  (each workflow escalates per job where it genuinely needs to)
- **Require approval for all external contributors** before running workflows

### 1.5 Branch protection and required checks

Releases are cut from `main`, and `release-app.yml` re-runs the whole CI
workflow against the tagged commit, so a tag that was never on a protected
branch still cannot skip the gate. Protect `main` and require the CI jobs by
name. (There is no aggregate `ci-required` check yet — recorded as D-13 in
[`docs/deviations.md`](../deviations.md).)

### 1.6 Attestations

Build provenance is recorded with `actions/attest-build-provenance`, which needs
`id-token: write` and `attestations: write` — granted in the one job that
produces them and nowhere else. Public repositories get this for free; nothing
else needs enabling.

---

## Part 2 — Cutting an application release

### 2.1 Decide the version

Semantic versioning against what a reader experiences. A pre-release version
(`1.2.0-rc.1`) is allowed everywhere and is the supported way to do a full
rehearsal: it produces a real signed draft that is never marked as the latest
release, and a draft can simply be deleted afterwards.

### 2.2 Bump every manifest, in one commit

Four files state the version and all four must agree, or the tag is refused:

```text
apps/desktop/package.json                  "version"
apps/desktop/src-tauri/tauri.conf.json     "version"
apps/desktop/src-tauri/Cargo.toml          [package] version
apps/desktop/src-tauri/Cargo.lock          the `antiburn` package entry
```

The lockfile is the one people forget. Refresh it after editing `Cargo.toml`:

```bash
cargo update --manifest-path apps/desktop/src-tauri/Cargo.toml --package antiburn
```

Check the whole set locally before pushing anything:

```bash
node scripts/verify-release-version.mjs app antiburn-v<version>
```

### 2.3 Write the release notes

Add a section to [`CHANGELOG.md`](../../CHANGELOG.md):

```markdown
## [1.2.0] - 2026-09-01
```

That section *is* the release notes, the body of the in-app update prompt, and
the `notes` field of `latest.json`. Write it for somebody who is deciding
whether to install this. Internal refactors, CI changes, and documentation
nobody acts on stay out.

### 2.4 Open a pull request, get it reviewed, merge it

The version bump and the changelog entry go through the same review as anything
else. Merge to `main`.

### 2.5 Tag the merged commit and push the tag

```bash
git checkout main && git pull
git tag -a antiburn-v1.2.0 -m "antiburn 1.2.0"
git push origin antiburn-v1.2.0
```

Annotated tags, always: a tag is a claim and it should carry an author. Push the
tag only after the commit is on `main`.

### 2.6 What the workflow then does

1. **verify** — the tag agrees with all four manifests and the lockfile; the
   updater has a public key and produces artifacts; the changelog has a section
   for this version.
2. **checks** — the entire CI workflow, re-run against the tagged commit.
3. **sbom** — CycloneDX inventories of the Rust tree (all targets) and of the
   frontend's production dependencies. No credentials are in scope for this job.
4. **build** — four jobs (macOS ARM64, macOS x64, Windows x64, Linux x64), each
   in the `release` environment, each producing an installer, an updater bundle,
   a detached signature, and a fragment of `latest.json`. These are the only
   jobs that can see a signing credential.
5. **draft** — merges the fragments into `latest.json` with immutable
   tag-specific URLs, checks that every URL it advertises names a file actually
   in the release, computes `SHA256SUMS`, attests provenance over every asset,
   and creates the draft.

Roughly: the whole thing takes as long as four platform builds, and any failure
leaves nothing published.

### 2.7 Review the draft

Open the draft release. Work down this list; it is the reason the release is a
draft.

- [ ] **Every asset is present.** Two macOS `.dmg`, one Windows `-setup.exe`,
      one `.AppImage`, one `.deb`, four updater bundles, four `.sig` files,
      `latest.json`, `SHA256SUMS`, `RELEASE-NOTES.md`, two `.cdx.json`
      inventories, one `.intoto.jsonl` provenance bundle.
- [ ] **No unsigned warning in the run log** — unless it was deliberate, in
      which case say so in the release notes before publishing.
- [ ] **`latest.json` is right.** Its `version` matches the tag. Every
      `platforms[*].url` starts with
      `https://github.com/antiburn/antiburn/releases/download/antiburn-v<version>/`
      — the tag-specific, immutable path, never `releases/latest`. All four
      platform keys are there: `darwin-aarch64`, `darwin-x86_64`,
      `windows-x86_64`, `linux-x86_64`.
- [ ] **Checksums match.** Download an asset and compare against `SHA256SUMS`.
- [ ] **Provenance verifies:**
      `gh attestation verify <asset> --repo antiburn/antiburn`
- [ ] **It installs and runs.** On each platform you can reach: install from the
      downloaded installer, launch it, confirm the tray item appears, open the
      popover, and check that Settings → About shows the new version. On macOS,
      confirm it opens without a Gatekeeper prompt (which is what notarization
      buys); on Windows, note whether SmartScreen warns.
- [ ] **The previous version can update to this one.** The real test of a
      release: install the previous version, point it at the draft only after
      publishing (drafts are not reachable), or rehearse with a pre-release tag.
- [ ] **"Set as the latest release" is checked.** The application's updater
      reads `releases/latest/download/latest.json`; if this release is not the
      latest one, nobody is offered the update. The workflow sets this already —
      confirm it survived.

### 2.8 Publish

Press **Publish release**. That is the whole publication step, and it is
deliberately a person's action.

### 2.9 Verify after publishing

```bash
curl -sSL https://github.com/antiburn/antiburn/releases/latest/download/latest.json | jq .
```

The `version` must be the one just published and the URLs must point at its tag.
Then open an installed copy of the previous version and use Settings → Updates →
Check now: it should report the new version.

If anything here is wrong, **do not fix the assets**. Go to
[`rollback.md`](rollback.md).

---

## Part 3 — Cutting an engine release

The engine is consumed as a Git dependency pinned to a tag; there is no
crates.io publish. The release is a reviewable source snapshot with a checksum
and a provenance record.

1. Bump `version` in `crates/antiburn-local/Cargo.toml`, refresh its lockfile
   (`cargo update --manifest-path crates/antiburn-local/Cargo.toml --package antiburn-local`),
   and add a section to `crates/antiburn-local/CHANGELOG.md`. Check it with
   `node scripts/verify-release-version.mjs engine antiburn-local-v<version>`.
2. Review, merge, then tag `antiburn-local-v<version>` and push the tag.
3. The workflow re-runs CI (which includes the engine's own network-free
   boundary suite, the whole-tree boundary scan, and `cargo deny check bans
   licenses`), packages a deterministic source tarball with `LICENSE` and
   `NOTICE` alongside it, inventories the dependency tree, attests provenance,
   and drafts the release.
4. Review the draft: the tarball extracts, `cargo test` passes inside it, the
   checksum matches, provenance verifies. (While the repository is private, the
   two provenance steps skip themselves — attestation persistence is plan-gated
   on private repositories — and activate automatically once the repository is
   public; a private-phase draft has no provenance bundle to verify.)
5. Publish. **Leave "Set as the latest release" unchecked** — the workflow
   already sets `--latest=false`, and for good reason: "latest" is a property of
   the repository, and the application's updater reads
   `releases/latest/download/latest.json`. An engine release wearing that badge
   would point every installed copy of the app at a release that has no such
   file.

### How consumers pin the engine

A consumer depends on the crate by pinning the full commit SHA of a released
tag — the SHA, not the tag name, is the contract, and restoring the previous
SHA is the supported rollback:

```toml
antiburn-local = { git = "https://github.com/antiburn/antiburn", rev = "<full-sha>" } # antiburn-local-v<version>
```

While this repository is private, downstream consumers authenticate on their
own side; nothing about this repository or its workflows changes, and the
`GITHUB_TOKEN` rule above stands. The recipe, so that the manifest already has
its final public form and going public later needs no consumer-side code
change:

1. A fine-grained access token, read-only on this repository's contents and
   nothing else, stored as a secret in the consumer's CI.
2. A URL rewrite in the consumer's CI, applied only when the secret is
   present, so the same manifest works before and after the repository is
   public:

   ```bash
   git config --global \
     url."https://x-access-token:${TOKEN}@github.com/antiburn/antiburn".insteadOf \
     "https://github.com/antiburn/antiburn"
   ```

3. `git-fetch-with-cli = true` under `[net]` in the consumer's cargo config,
   so cargo fetches through the git CLI — which honors the rewrite in CI and
   the developer's normal credential helper locally.

The lockfile records the manifest URL, never the rewritten one, so the token
cannot end up in a committed lockfile. When the repository becomes public,
the consumer deletes the secret and revokes the token; the rewrite step
becomes a no-op and everything else stays byte-identical.

---

## When a run fails

| Failure | What it means | What to do |
| --- | --- | --- |
| `verify` rejects the tag | A manifest, the lockfile, or the changelog disagrees with the tag | Fix on `main`, delete the *unpublished* tag, re-tag. Deleting a tag that was never published is fine; deleting a published one is not. |
| `checks` fails | The tagged commit does not pass CI | Fix on `main` and cut a new version. Do not re-tag around a failing gate. |
| A `build` job fails | Usually a credential or a platform toolchain | Fix, then re-run the failed jobs. The draft is rebuilt idempotently. |
| `draft` refuses: "already published" | The tag has a published release | Stop. This is the immutability rule doing its job — go to [`rollback.md`](rollback.md). |

Re-running the workflow on an existing **draft** re-uploads every asset with
`--clobber`, which is safe and expected. It refuses outright to touch a
published one.

## What is never done

- Replacing, deleting, or re-uploading an asset on a published release.
- Deleting or moving a published tag.
- Publishing by hand from a local build. Every published artifact comes from a
  tagged run of these workflows, which is what the provenance attestation
  actually attests to.
- Signing with anything but the credentials in the `release` environment.

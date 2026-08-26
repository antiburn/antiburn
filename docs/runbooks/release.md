<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Cutting a release

How a version of antiburn gets from a commit to something a reader can install.

The measured baseline, conservative after-model, output invariants, and
post-merge ratification thresholds live in
[`docs/ci-release-efficiency.md`](../ci-release-efficiency.md).

There are two release trains, tagged separately and released separately:

| Train               | Tag                         | Workflow                                                           | What it produces                                                                           |
| ------------------- | --------------------------- | ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------ |
| Desktop application | `antiburn-v<version>`       | [`release-app.yml`](../../.github/workflows/release-app.yml)       | Installers, bootstrap scripts, updater bundles, signatures, checksums, inventories, provenance, `latest.json` |
| Engine crate        | `antiburn-local-v<version>` | [`release-engine.yml`](../../.github/workflows/release-engine.yml) | A source tarball, checksums, an inventory, provenance                                      |

Both are **draft-first**. The workflow builds, signs, hashes, attests, and
drafts; a person reads the draft and presses Publish. There is no auto-publish
and there will not be one — the review is the point, not a formality on the way
to it.

GitHub Releases is the only place antiburn's artifacts live. There is no
separate download host, no object store, and no content-delivery layer: the
release page is the canonical artifact host and the updater host at once.

---

## Part 1 — One-time repository setup

The `release` environment, updater key, and Apple credentials are configured.
Use this section when credentials rotate or the environment must be recreated.
The workflows fail early if required material is missing, so an unconfigured
repository cannot produce something that looks like a signed release.

### 1.1 The `release` environment

Create an environment named exactly **`release`** (Settings → Environments).
Every signing credential lives here rather than in repository secrets, so the
only jobs that can reach them are the ones that ask for the environment by name
— in this repository, the four `build` jobs of `release-app.yml`.

Configure it as:

- **Deployment branches and tags:** _Selected branches and tags_ → add the tag
  rule `antiburn-v*`. Nothing else can start a job that touches these secrets.
- **Required reviewers:** optional. The draft-then-publish step is already a
  human gate; add reviewers here as well if you want the pause to happen
  _before_ the credentials are used rather than after.
- **Wait timer:** not needed.

Fork pull requests can never reach this: the release workflows have no
`pull_request` trigger at all, and their first job refuses to run unless
`github.repository` is this repository.

### 1.2 Secrets

Add these to the **`release` environment** (not to repository-wide secrets).
Placeholders below show the shape, never a real value.

| Secret                               | Required                  | What it is                                                                                                                | How to produce it                                                                                                                                                                                                                    |
| ------------------------------------ | ------------------------- | ------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `TAURI_SIGNING_PRIVATE_KEY`          | **Always**                | The updater's private signing key. Signs every updater bundle; the app verifies against the public half compiled into it. | `pnpm --filter @antiburn/desktop exec tauri signer generate -w "$HOME/antiburn.key"` (absolute path — a relative one lands in the working tree), then paste the contents of `antiburn.key`. Placeholder: `dW50cnVzdGVkIGNvbW1lbnQ6…` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | If the key has one        | The passphrase for the above. Give the key a passphrase.                                                                  | Chosen when generating the key. Placeholder: `<passphrase>`                                                                                                                                                                          |
| `APPLE_CERTIFICATE`                  | For signed macOS builds   | Base64 of the **Developer ID Application** certificate and its private key, exported as `.p12`.                           | `base64 -i DeveloperID.p12 \| pbcopy`. Placeholder: `MIIM…`                                                                                                                                                                          |
| `APPLE_CERTIFICATE_PASSWORD`         | If the `.p12` has one     | The `.p12` export passphrase.                                                                                             | Chosen during export. Leave this secret unset when the export has no passphrase. Placeholder: `<passphrase>`                                                                                                                         |
| `APPLE_PROVISIONING_PROFILE`         | For signed macOS builds   | Base64 of the Developer ID profile for `ai.antiburn.desktop`, with Communication Notifications enabled.                  | Download `antiburn.provisionprofile` from Apple Developer, then run `base64 -i antiburn.provisionprofile \| pbcopy`.                                                                                                                 |
| `APPLE_ID`                           | For notarization          | The Apple ID that owns the notarization submission.                                                                       | Placeholder: `releases@example.org`                                                                                                                                                                                                  |
| `APPLE_PASSWORD`                     | For notarization          | An **app-specific password** for that Apple ID — never the account password.                                              | appleid.apple.com → Sign-In and Security → App-Specific Passwords. Placeholder: `abcd-efgh-ijkl-mnop`                                                                                                                                |
| `APPLE_TEAM_ID`                      | For notarization          | The ten-character Apple Developer team identifier.                                                                        | Apple Developer → Membership. Placeholder: `ABCDE12345`                                                                                                                                                                              |
| `WINDOWS_CERTIFICATE`                | For signed Windows builds | Base64 of the Authenticode code-signing certificate exported as `.pfx`.                                                   | `base64 -w0 codesign.pfx`. Placeholder: `MIIM…`                                                                                                                                                                                      |
| `WINDOWS_CERTIFICATE_PASSWORD`       | With the above            | The `.pfx` export passphrase.                                                                                             | Chosen during export. Placeholder: `<passphrase>`                                                                                                                                                                                    |

`GITHUB_TOKEN` is provided by Actions; it is not configured and must not be
replaced by a personal access token.

**The updater key is not optional.** `release-app.yml` fails immediately if
`TAURI_SIGNING_PRIVATE_KEY` is absent, and it fails _before that_ if
`plugins.updater.pubkey` in `apps/desktop/src-tauri/tauri.conf.json` is still
empty. Both halves have to exist for an update to be verifiable, and an update
that cannot be verified is worse than no updater at all. See
[`updater-key-recovery.md`](updater-key-recovery.md) for custody.

#### Communication Notifications profile

The Focus-status API needs more than a Developer ID signature. The signed app
must carry a matching Developer ID provisioning profile that authorizes the
restricted `com.apple.developer.usernotifications.communication` entitlement.

1. In Apple Developer, select the explicit App ID `ai.antiburn.desktop`.
2. Enable **Communication Notifications** and save the App ID.
3. Create a **Developer ID** provisioning profile for that App ID. Select the
   same Developer ID Application certificate stored in `APPLE_CERTIFICATE`.
4. Download it as `antiburn.provisionprofile`.
5. Store it in the `release` environment:

   ```bash
   base64 -i antiburn.provisionprofile | gh secret set \
     --env release APPLE_PROVISIONING_PROFILE --repo antiburn/antiburn
   ```

Regenerate the profile after the App ID capability or signing certificate
changes. Never commit it. The release workflow checks its team, application
identifier, capability, distribution type, and expiration before embedding it.

### 1.3 Repository variables

Variables (Settings → Secrets and variables → Actions → Variables), not secrets:

| Variable                 | Default when unset                              | Effect                                                                                                                                                                                                                                                                                                                                           |
| ------------------------ | ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `ALLOW_UNSIGNED_WINDOWS` | unset (= build fails without a certificate)     | `true` builds the Windows installer **without** an Authenticode signature. SmartScreen warns on download.                                                                                                                                                                                                                                        |
| `WINDOWS_TIMESTAMP_URL`  | `http://timestamp.digicert.com`                 | RFC 3161 timestamp authority used when signing the installer, so signatures outlive the certificate.                                                                                                                                                                                                                                             |

macOS releases always require Developer ID signing and notarization.
`ALLOW_UNSIGNED_WINDOWS` remains set under D-16 until an Authenticode certificate
exists. It produces a build that says it is unsigned; it never fakes a platform
signature.

#### Enabling Windows installer signature enforcement

The PowerShell bootstrap installer requires SHA-256 verification today but permits
the unsigned Windows mode recorded in D-16. When an Authenticode certificate is
available:

1. Configure `WINDOWS_CERTIFICATE` and `WINDOWS_CERTIFICATE_PASSWORD` in the
   `release` environment.
2. Remove `ALLOW_UNSIGNED_WINDOWS` and require the `authenticode` signing mode in
   `release-app.yml`.
3. Extend `Assert-InstallerIntegrity` in the root `install.ps1` with
   `Get-AuthenticodeSignature`. Require `Valid` status and the expected antiburn
   publisher identity.
4. Add tests for a missing signature, a wrong publisher, an invalid chain, and an
   expired certificate to `scripts/install-ps1.test.ps1`.
5. Remove the unsigned-installer warning from `install.ps1` and the README only
   after a signed release passes the Windows acceptance check.

Do not add inactive signature code before these credentials exist. The checksum
and the unsigned warning must continue to state the current release behavior.

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

Releases are cut from `main`, and each release workflow accepts only a
successful **push** run of `.github/workflows/ci.yml` whose SHA exactly equals
the tag SHA. A pull-request check, a successful run for a neighboring commit,
or an untested tag is refused before any signing job starts.

`main` requires only the `ci-required` check. It does not require pull requests
or resolved conversations, and it does not prohibit force-pushes or deletion.
See [`branch-rules.md`](branch-rules.md) for the committed rule and its apply,
verify, and rollback commands. The aggregate check is deliberately stable while
its platform jobs remain free to run or skip according to the semantic diff
classifier.

The current solo-maintainer repository requires zero independent approvals.
That is not permission to bypass the pull request: the exact-SHA main run is the
release trust record, and the release jobs query it through the read-only
Actions API.

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
node scripts/verify-app-engine-release.mjs
```

The second check proves that the complete in-tree `antiburn-local` crate is
identical to the annotated `antiburn-local-v<version>` tag named by its own
manifest, that the tag is an ancestor of the application commit, and that the
desktop lockfile records the same engine version. If it fails, cut the engine
release first. Application releases never ship an unreleased engine tree under
an older component version.

### 2.3 Write the release notes

Add a section to [`CHANGELOG.md`](../../CHANGELOG.md):

```markdown
## [1.2.0] - 2026-09-01
```

That section _is_ the release notes, the body of the in-app update prompt, and
the `notes` field of `latest.json`. Write it for somebody who is deciding
whether to install this. Internal refactors, CI changes, and documentation
nobody acts on stay out.

### 2.4 Open a pull request, get it reviewed, merge it

The version bump and the changelog entry go through the same review as anything
else. A pure release bump gets the narrow release-metadata gate only when all
three executable manifests changed **only** their package version, the lockfile
changed only the `antiburn` package entry, and the changelog is the only other
changed file. Any dependency or other content change falls back to the full
platform matrix. Merge to `main`.

The resulting main run compiles all four release targets with `tauri build
--no-bundle`, in parallel with its required metadata and boundary checks. It has
no release environment and no signing secret; its only durable output is a
dependency cache that the tag build can restore. The cache is saved only by a
trusted main push and release jobs are restore-only.

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
2. **trusted-main-ci** — waits for and verifies the successful main push run for
   this exact tag SHA. It never substitutes a PR run and never re-runs the
   matrix.
3. **sbom** — CycloneDX inventories of the Rust tree (all targets) and of the
   frontend's production dependencies. No credentials are in scope for this job.
4. **build** — four jobs (macOS ARM64, macOS x64, Windows x64, Linux x64), each
   restoring the dependency cache prepared by main and then entering the
   `release` environment to package and sign. Each produces an installer, an
   updater bundle, a detached signature, and a fragment of `latest.json`. These
   are the only jobs that can see a signing credential, and none may save a
   cache after doing so.
5. **draft** — adds the root `install.sh` and `install.ps1`, then merges the
   fragments into `latest.json` with immutable
   tag-specific URLs; verifies all four platform keys, asset presence, detached
   signatures, reported signing modes, and `SHA256SUMS`; attests provenance over
   every asset; and creates the draft.

The main run and its cache warming finish before the exact-SHA gate opens, so
the tag's critical path is packaging and signing rather than another test
matrix followed by a cold compile. Any failure still leaves nothing published.

### 2.7 Review the draft

The workflow summary contains the exact main CI run, the signing mode reported
by each target, and the complete checksum table. Before the draft exists, the
workflow has already required the four platform keys, immutable URLs, matching
detached signatures, present assets, and a successful `sha256sum --check`.
Those are machine gates, not boxes for a person to repeat.

Open the draft release and perform the checks that need a real reader or
installed operating system:

The debug updater simulator described in
[`docs/debugging.md`](../debugging.md#test-the-updater-interface) helps with UI
work, but it does not replace these signed-artifact checks.

- [ ] **It installs and runs.** On each platform you can reach: install from the
      downloaded installer, launch it, confirm the tray item appears, open the
      popover, and check that Settings → About shows the new version. On macOS,
      confirm it opens without a Gatekeeper prompt (which is what notarization
      buys); on Windows, note whether SmartScreen warns.
- [ ] **The previous version can update to this one.** The real test of a
      release: install the previous version, point it at the draft only after
      publishing (drafts are not reachable), or rehearse with a pre-release tag.
      On macOS and Linux AppImage, confirm About shows download progress, changes
      to the installed state, and offers Restart to update. Restart and confirm
      About shows the new version. On Windows, confirm the passive NSIS installer
      exits and relaunches the updated application. The Debian package is
      install-only and must report that in-app updates are unavailable.
- [ ] **Updater failures stay safe and visible.** In a release rehearsal, interrupt
      one download and serve one bundle with a bad updater signature. Each attempt
      must end in a visible install failure with a retry action. The bad bundle must
      not replace the installed application.
- [ ] **"Set as the latest release" is checked.** The application's updater
      reads `releases/latest/download/latest.json`; if this release is not the
      latest one, nobody is offered the update. The workflow sets this already —
      confirm it survived.
- [ ] **The release notes and signing modes tell the truth.** Read the notes and
      compare any Gatekeeper or SmartScreen behavior with the workflow summary.
- [ ] **The bootstrap scripts install this release.** Run `install.sh` on macOS
      and Linux and `install.ps1` on Windows. Run each script twice and confirm
      the second run replaces or upgrades the same installation. Interrupt one
      package download and confirm the installed version does not change.
- [ ] **Provenance verifies where available:**
      `gh attestation verify <asset> --repo antiburn/antiburn`.

### 2.8 Publish

Press **Publish release**. That is the whole publication step, and it is
deliberately a person's action.

### 2.9 Verify after publishing

```bash
curl -sSL https://github.com/antiburn/antiburn/releases/latest/download/latest.json | jq .
curl -fsSL https://github.com/antiburn/antiburn/releases/latest/download/install.sh | sh -s -- --help
curl -fsSL https://github.com/antiburn/antiburn/releases/latest/download/install.ps1 > /dev/null
```

The `version` must be the one just published and the URLs must point at its tag.
Then open an installed copy of the previous version and use Settings → About →
Check for updates. It should report the new version and complete the install and
restart flow described above.

If anything here is wrong, **do not fix the assets**. Go to
[`rollback.md`](rollback.md).

---

## Part 3 — Cutting an engine release

The engine is consumed as a Git dependency pinned to a tag; there is no
crates.io publish. The release is a reviewable source snapshot with a checksum
and a provenance record.

The desktop path-depends on the in-tree engine while developing, but an
application release is accepted only when that entire engine subtree exactly
matches an annotated engine release tag. Any engine change since the last tag
therefore makes an engine release the required first half of the next
application release.

1. Bump `version` in `crates/antiburn-local/Cargo.toml`, refresh its lockfile
   (`cargo update --manifest-path crates/antiburn-local/Cargo.toml --package antiburn-local`),
   and add a section to `crates/antiburn-local/CHANGELOG.md`. Check it with
   `node scripts/verify-release-version.mjs engine antiburn-local-v<version>`.
   Refresh the app's lockfile too
   (`cargo update --manifest-path apps/desktop/src-tauri/Cargo.toml --package antiburn-local`):
   the shell path-depends on the engine, so its lockfile records the engine
   version, and the locked desktop CI legs and the license check fail on the
   mismatch otherwise.
2. Review, merge, then tag `antiburn-local-v<version>` and push the tag.
3. The workflow requires the successful main push run for the exact tag SHA,
   then packages a deterministic source tarball with `LICENSE` and `NOTICE`
   alongside it, inventories the dependency tree, verifies its checksums,
   attests provenance, and drafts the release. The engine release does not
   repeat the desktop platform matrix.
4. Review the draft: the tarball extracts, `cargo test` passes inside it, the
   checksum matches, provenance verifies. (While the repository is private, the
   two provenance steps skip themselves — attestation persistence is plan-gated
   on private repositories — and activate automatically once the repository is
   public; a private-phase draft has no provenance bundle to verify.)

   When running `cargo test` inside the extracted tarball, set
   `ANTIBURN_OSS_MANIFEST_DIR` to a repository checkout's `docs/oss/`
   directory. The boundary suite's manifest test reads the governance
   manifests, which deliberately do not ship in the tarball; without the
   variable, that one test fails with a "must ship with the repository"
   error while everything else passes.

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

| Failure                              | What it means                                                     | What to do                                                                                                                                                                                                                                                                                                                                         |
| ------------------------------------ | ----------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `verify` rejects the tag             | A manifest, the lockfile, or the changelog disagrees with the tag | Fix on `main`, delete the _unpublished_ tag, re-tag. Deleting a tag that was never published is fine; deleting a published one is not. Deleting any release tag requires an admin to temporarily disable the tag-immutability ruleset (Settings → Rules → Rulesets), then re-enable it immediately after re-tagging — that friction is deliberate. |
| `trusted-main-ci` fails or times out | The exact tag SHA has no successful main push run                 | For a transient failure, re-run CI for that exact SHA, wait for success, then re-run the release workflow. For a code failure, fix it on `main` and cut a new version and tag; never move the failed tag to the corrected commit. Do not substitute a PR run.                                                                                      |
| A `build` job fails                  | Usually a credential or a platform toolchain                      | Fix, then re-run the failed jobs. The draft is rebuilt idempotently.                                                                                                                                                                                                                                                                               |
| `draft` refuses: "already published" | The tag has a published release                                   | Stop. This is the immutability rule doing its job — go to [`rollback.md`](rollback.md).                                                                                                                                                                                                                                                            |

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
  `ALLOW_UNSIGNED_WINDOWS` uses no signing identity; it does not make an
  identity claim from outside the environment.

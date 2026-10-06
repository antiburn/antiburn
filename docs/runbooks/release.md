# Cutting a release

How a version of antiburn gets from a commit to something a reader can install.

The measured baseline, conservative after-model, output invariants, and
post-merge ratification thresholds live in
[`docs/ci-release-efficiency.md`](../ci-release-efficiency.md).

There are two release trains, tagged separately and released separately:

| Train               | Tag                         | Workflow                                                           | What it produces                                                                                              |
| ------------------- | --------------------------- | ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------- |
| Desktop application | `antiburn-v<version>`       | [`release-app.yml`](../../.github/workflows/release-app.yml)       | Installers, bootstrap scripts, updater bundles, signatures, checksums, inventories, provenance, `latest.json` |
| Engine crate        | `antiburn-local-v<version>` | [`release-engine.yml`](../../.github/workflows/release-engine.yml) | A source tarball, checksums, an inventory, provenance                                                         |

Both are **draft-first**. The workflow builds, signs, hashes, attests, and
drafts; a person reads the draft and presses Publish. There is no auto-publish
and there will not be one — the review is the point, not a formality on the way
to it.

GitHub Releases hosts published artifacts. Manual builds store their assets in
GitHub Actions. There is no
separate download host, no object store, and no content-delivery layer: the
release page is the canonical artifact host and the updater host at once.

---

## Part 1 — One-time repository setup

The `release` environment, updater key, Apple credentials, and Azure Windows
signing configuration are configured.
Use this section when credentials rotate or the environment must be recreated.
The workflows fail early if required material is missing, so an unconfigured
repository cannot produce something that looks like a signed release.

### 1.1 The `release` environment

Create an environment named exactly **`release`** (Settings → Environments).
Every signing credential lives here rather than in repository secrets, so the
only jobs that can reach them are the ones that ask for the environment by name
— in this repository, the six `build` jobs of `release-app.yml`.

Configure it as:

- **Deployment branches and tags:** _Selected branches and tags_ → add the tag
  rule `antiburn-v*` and branch rule `main`. Manual builds run only on `main`.
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
| `APPLE_PROVISIONING_PROFILE`         | For signed macOS builds   | Base64 of the Developer ID profile for `ai.antiburn.desktop`, with Communication Notifications enabled.                   | Download `antiburn.provisionprofile` from Apple Developer, then run `base64 -i antiburn.provisionprofile \| pbcopy`.                                                                                                                 |
| `APPLE_ID`                           | For notarization          | The Apple ID that owns the notarization submission.                                                                       | Placeholder: `releases@example.org`                                                                                                                                                                                                  |
| `APPLE_PASSWORD`                     | For notarization          | An **app-specific password** for that Apple ID — never the account password.                                              | appleid.apple.com → Sign-In and Security → App-Specific Passwords. Placeholder: `abcd-efgh-ijkl-mnop`                                                                                                                                |
| `APPLE_TEAM_ID`                      | For notarization          | The ten-character Apple Developer team identifier.                                                                        | Apple Developer → Membership. Placeholder: `ABCDE12345`                                                                                                                                                                              |
| `ANTIBURN_ANALYTICS_URL`             | For official analytics    | The first-party analytics endpoint compiled into official builds.                                                         | Store the production HTTPS origin. Never commit it.                                                                                                                                                                                  |
| `ANTIBURN_ANALYTICS_OPERATOR`        | With the analytics URL    | The operator name shown in Settings → Privacy.                                                                            | `Cadence AI (Vic) Pty Ltd`                                                                                                                                                                                                           |

`GITHUB_TOKEN` is provided by Actions; it is not configured and must not be
replaced by a personal access token in the build and signing workflows. The
separate Homebrew publication workflow uses a tap-only PAT for its
cross-repository write; see [Homebrew publication](#homebrew-publication).

The workflow compiles official desktop builds with `distribution,analytics`.
Keep both analytics secrets unset until the privacy policy is ready. A missing
or invalid endpoint or operator leaves analytics unavailable.

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

### 1.3 Azure Windows signing and variables

Windows uses **Azure Artifact Signing Basic**, an Organization / Public identity
validation, and a **Public Trust** certificate profile. Microsoft added Australian
organization eligibility on 2026-07-23. The legal publisher is
`Cadence AI (Vic) Pty Ltd`. The East US service endpoint is
`https://eus.codesigning.azure.net`; the company can be Australian even though
the signing service is hosted in another region.

Basic includes 5,000 signatures per month at a published USD 9.99/month before
tax. Each file signed consumes a signature. Tauri signs the app, NSIS plugin
copies, uninstaller, and installer, so each release target uses several
signatures. Check current [pricing](https://azure.microsoft.com/pricing/details/artifact-signing/)
before changing the subscription.

Add these as **environment variables** under Settings → Environments → `release`:

| Variable | Value |
| --- | --- |
| `AZURE_CLIENT_ID` | Application (client) ID of the dedicated Entra signing application |
| `AZURE_TENANT_ID` | Entra tenant ID that owns the signing subscription |
| `AZURE_SUBSCRIPTION_ID` | Subscription ID containing the signing account |
| `AZURE_SIGNING_ENDPOINT` | Region-specific HTTPS signing endpoint |
| `AZURE_SIGNING_ACCOUNT_NAME` | Artifact Signing account name |
| `AZURE_SIGNING_CERTIFICATE_PROFILE_NAME` | Public Trust certificate profile name |

These are configuration identifiers, not private keys. No PFX, certificate
password, or client secret is used. All six variables are required for signed
Windows releases.

#### Azure identity and GitHub federation setup

1. Register `Microsoft.CodeSigning` in the paid Azure subscription. Create a
   Basic signing account in a supported region.
2. Give the human onboarding account `Artifact Signing Identity Verifier`
   access. Submit Organization / Public validation using the legal business
   details and complete the representative identity checks. Wait for
   **Completed**, then create a **Public Trust** certificate profile with
   Program Type **None**.
3. Create a single-tenant Entra app registration for Windows release signing.
   Under Certificates & secrets → Federated credentials, add a GitHub credential
   for organization `antiburn`, repository `antiburn`, entity type **Environment**,
   and environment `release`.
4. Confirm issuer `https://token.actions.githubusercontent.com`, subject
   `repo:antiburn/antiburn:environment:release`, and audience
   `api://AzureADTokenExchange`.
5. Assign the app's service principal `Artifact Signing Certificate Profile Signer`
   at the certificate-profile scope, not subscription Owner. For example:

   ```bash
   az role assignment create \
     --assignee-object-id "<service-principal-object-id>" \
     --assignee-principal-type ServicePrincipal \
     --role "Artifact Signing Certificate Profile Signer" \
     --scope "/subscriptions/<subscription-id>/resourceGroups/<resource-group>/providers/Microsoft.CodeSigning/codeSigningAccounts/<account>/certificateProfiles/<profile>"
   ```

6. Configure the six GitHub environment variables and keep the environment's
   `antiburn-v*` tag and `main` branch restrictions. Azure login uses OIDC through
   the CLI. The
   signing client excludes all credential types except Azure CLI.

The shared release build matrix has `id-token: write`; only the Windows signed
legs execute Azure login. All legs remain behind the same protected environment
and exact-SHA main CI gate. Installer-test CI has no signing identity and does
not make signing requests.

Follow Microsoft's [setup guide](https://learn.microsoft.com/azure/artifact-signing/quickstart)
and [role guide](https://learn.microsoft.com/azure/artifact-signing/tutorial-assign-roles)
when renewing identity validation or recreating the resources. Check the Azure
portal for validation expiry and renew it before it expires. Microsoft rotates
the three-day signing certificates; do not pin a leaf certificate thumbprint.

#### Toolchain and signing order

`scripts/setup-windows-signing.ps1` downloads hash-verified, pinned Windows SDK
BuildTools 10.0.26100.4188, Artifact Signing Client 1.0.128, and the x64 .NET
8.0.31 runtime. The x64 SignTool and client DLL run natively on x64 and through
x64 emulation on the existing `windows-11-arm` runner. The official Artifact
Signing GitHub action does not support ARM runners, so this integration uses
Microsoft's documented SignTool interface instead.

CI exercises x64 runtime and SignTool execution on both Windows architectures
without credentials. This smoke check does not prove live client authentication
or signing: the signed prerelease must prove the full path on both targets before
production acceptance. If ARM64 signing fails, stop the release and investigate;
do not drop that target or publish an unsigned substitute.

The temporary Tauri `bundle.windows.signCommand` overlay calls
`scripts/windows-signing.ps1` with each file as a separate argument. It signs
with SHA-256 and the RFC 3161 service `http://timestamp.acs.microsoft.com`, verifies
the signature, and requires the full expected publisher subject and a timestamp.
Tauri calls this hook for the app and NSIS contents before producing the final
installer and its updater signature. Never sign an installer again after its
detached updater signature, checksum, or provenance is generated.

The build rejects SignTool warnings and errors. Final verification checks the
app executable and NSIS installer. Windows acceptance also checks the installed
uninstaller. A valid timestamp lets signatures remain valid after leaf expiry;
the verifier uses Windows trust validation rather than rejecting every expired
leaf certificate.

If signing fails, check the logged error, OIDC subject, profile-level signer
role, matching endpoint region, and identity validation status. A wrong role or
endpoint commonly produces 403. Do not change the role to Owner or add a secret
fallback. Runtime or DLL-load errors require checking the pinned x64 toolchain
and its `DOTNET_ROOT` configuration, especially on ARM64.

#### Legacy unsigned waiver

`ALLOW_UNSIGNED_WINDOWS` is a repository variable. It permits a clearly labelled
unsigned Windows build only when **all six Azure variables are absent**. With
all six configured, signing is required even if the waiver is still `true`.
A partial configuration fails; a signing failure cannot use the waiver.
Remove the variable after signed release acceptance. macOS releases always
require Developer ID signing and notarization.

#### Enabling Windows installer signature enforcement

The PowerShell bootstrap installer requires SHA-256 verification today but permits
unsigned Windows packages. Activate strict bootstrap verification after the
first signed production release passes Windows acceptance and becomes latest:

1. Rehearse both Windows targets with the configured Azure signing profile.
2. Remove `ALLOW_UNSIGNED_WINDOWS` and confirm both inventory entries use
   `authenticode`.
3. Extend `Assert-InstallerIntegrity` in the root `install.ps1` with
   `Get-AuthenticodeSignature`. Require `Valid` status and the expected antiburn
   publisher identity.
4. Add tests for a missing signature, a wrong publisher, an invalid chain, and an
   expired leaf certificate with and without a valid timestamp to
   `scripts/install-ps1.test.ps1`.
5. Remove the unsigned-installer warning from `install.ps1` and the README only
   after a signed release passes the Windows acceptance check.

Do not merge strict bootstrap enforcement while latest still points at an
unsigned release. That would stop the public install command from working.
Decide how explicit requests for historical unsigned versions are handled, and
document any rejection clearly. Keep current unsigned warnings accurate until
the signed release is available. Authenticode signing does not guarantee that
SmartScreen warnings disappear; Microsoft no longer promises an EV bypass.

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

`main` requires signed commits, a pull request with one approval, approval of
the latest push, and an up-to-date `ci-required` check. It blocks force-pushes
and deletion. Repository rules are managed in GitHub rather than duplicated in
the source tree. The aggregate check is deliberately stable while its platform
jobs remain free to run or skip according to the semantic diff classifier.

The exact-SHA main run is the release trust record, and the release jobs query
it through the read-only Actions API. Administrator bypass is for exceptional
repository maintenance, not routine contributions or releases.

### 1.6 Attestations

Build provenance is recorded with `actions/attest-build-provenance`, which needs
`id-token: write` and `attestations: write`. Only the assemble job has
`attestations: write`; the build job also needs `id-token: write` for Azure login.
Public repositories get attestations for free; nothing else needs enabling.

---

## Part 2 — Cutting an application release

### Manual full-matrix build

Run the same signed packaging pipeline without creating a tag or GitHub Release:

```bash
gh workflow run release-app.yml --ref main -f version=0.9.0
```

The version input is required and can be an existing version or a supported
prerelease such as `0.10.0-rc.1`. The workflow applies it only in runner checkouts
to all application and helper manifests and lockfile entries. It runs all six
desktop targets, both remote helpers, SBOMs, updater signing, platform signature
checks, checksum assembly, and provenance. Artifact and installer names are the
same as tag builds. Download individual platform artifacts or the assembled
`release` artifact from the Actions run.

Manual builds require successful main CI for the dispatched SHA and the existing
release credentials. They do not require a matching engine release tag or a
changelog entry. `BUILD-INFO.json` records the source SHA, version, event, and
engine tree identity. Tag builds retain engine-release and changelog checks.

Only tag pushes can reach the GitHub Releases write job. A manual build cannot
create or update a release or change Latest. The assembled `latest.json` retains
the normal version-based release URLs for packaging validation; the workflow
does not host those newly built files at those URLs. Use the Actions artifacts
for manual acceptance, not the published update endpoint.

Signing and notarization consume the existing service quotas. Installing a
manual build with the same version uses the regular app identity and paths;
perform acceptance on the intended Windows test systems.

### 2.1 Decide the version

Semantic versioning against what a reader experiences. A pre-release version
(`1.2.0-rc.1`) is allowed everywhere and is the supported way to do a full
rehearsal: it produces a real signed draft that is never marked as the latest
release, and a draft can simply be deleted afterwards.

### 2.2 Bump every manifest, in one commit

Six files state the version and all six must agree, or the tag is refused:

```text
apps/desktop/package.json                  "version"
apps/desktop/src-tauri/tauri.conf.json     "version"
apps/desktop/src-tauri/Cargo.toml          [package] version
apps/desktop/src-tauri/Cargo.lock          the `antiburn` package entry
crates/antiburn-remote/Cargo.toml          [package] version
crates/antiburn-remote/Cargo.lock          the `antiburn-remote` package entry
```

The remote helper is in the set because it ships as an asset of this release.
Its archive name, its `--version` output and its `hello` response all come from
its manifest, so a reader who holds a helper can name the release it came from.
The helper bumps with the application even when its code does not change.

The lockfiles are the ones people forget. Refresh them after editing the
manifests. The desktop lockfile records both crates:

```bash
cargo update --manifest-path apps/desktop/src-tauri/Cargo.toml --package antiburn
cargo update --manifest-path apps/desktop/src-tauri/Cargo.toml --package antiburn-remote
cargo update --manifest-path crates/antiburn-remote/Cargo.toml --package antiburn-remote
```

If `cargo update` also moves unrelated entries in a lockfile, edit only the
version line by hand instead. Do not pass `--offline`: it resolves from the
local cache and moves unrelated entries.

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
four executable manifests changed **only** their package version, each lockfile
changed only the package entries those manifests name, and the changelog is the
only other changed file. Any dependency or other content change falls back to the full
platform matrix. Merge to `main`.

The resulting main run compiles all six release targets with `tauri build
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
4. **build** — six jobs (macOS ARM64, macOS x64, Windows x64, Windows ARM64,
   Linux x64, Linux ARM64), each
   restoring the dependency cache prepared by main and then entering the
   `release` environment to package and sign. Each produces an installer, an
   updater bundle, a detached signature, and a fragment of `latest.json`. These
   are the only jobs that can see a signing credential, and none may save a
   cache after doing so.
5. **remote-helper** — builds static Linux x64 and ARM64 helpers on native
   runners, runs their tests, checks static linkage, and verifies archive
   extraction. It has no signing or repository-write credentials.
6. **assemble** — requires both helper archives, adds the root `install.sh` and `install.ps1`, then merges the
   fragments into `latest.json` with immutable
   tag-specific URLs; verifies all six platform keys, asset presence, detached
   signatures, reported signing modes, and `SHA256SUMS`; attests provenance over
    every asset; and uploads the assembled `release` Actions artifact.
7. **draft** — runs only for tag pushes and creates or updates the draft from
   the assembled assets.

The main run and its cache warming finish before the exact-SHA gate opens, so
the tag's critical path is packaging and signing rather than another test
matrix followed by a cold compile. Any failure still leaves nothing published.

### 2.7 Review the draft

The workflow summary contains the exact main CI run, the signing mode reported
by each target, and the complete checksum table. Before the draft exists, the
workflow has already required the six platform keys, immutable URLs, matching
detached signatures, present assets, and a successful `sha256sum --check`.
Those are machine gates, not boxes for a person to repeat.

The release includes `antiburn-remote-<version>-x86_64-unknown-linux-musl.tar.gz`
and `antiburn-remote-<version>-aarch64-unknown-linux-musl.tar.gz`. Both are covered
by `SHA256SUMS`. Build provenance is included when the public-repository
attestation steps run; private-repository releases skip those steps. Their archive version follows the application
release, and §2.2 holds the helper's manifest to it, so `antiburn-remote --version`
on an installed helper reports the release that shipped it. The helper's wire
protocol version is a separate compatibility check.
Follow [remote host setup](../remote-sessions.md) to exercise SSH discovery and
offline cached analysis on a Linux host before publishing.

Windows ships `antiburn_<version>_x64-setup.exe` and
`antiburn_<version>_arm64-setup.exe`. The ARM64 job uses the native
`windows-11-arm` runner and the `aarch64-pc-windows-msvc` Rust target.
`install.ps1` reads `Win32_Processor.Architecture` to select the package even
when PowerShell runs under emulation. The updater uses `windows-x86_64` and
`windows-aarch64` respectively. The NSIS installer runs under x86 emulation
on ARM64; the installed application runs natively.

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
      Check both Windows architectures, including an ARM64 install from
      Windows PowerShell 5.1 and an emulated x64 PowerShell 7 session.
- [ ] **Windows publisher and timestamps verify.** Dot-source
      `scripts/windows-signing.ps1` and call `Assert-WindowsSignature -FilePath`
      for the downloaded installer, installed `antiburn.exe`, and installed
      uninstaller on both architectures. All must have valid Authenticode,
      the expected company subject, and timestamps. Test uninstall as well.
      Download through a browser on clean supported Windows systems to retain
      Mark of the Web, and record SmartScreen behavior separately from signature
      validity.
- [ ] **The previous version can update to this one.** The real test of a
      release: install the previous version, point it at the draft only after
      publishing (drafts are not reachable), or rehearse with a pre-release tag.
      On macOS and Linux AppImage, leave the popover closed and confirm the app
      downloads, installs, and restarts without approval. Confirm About shows the
      new version after restart. On Windows, confirm the passive NSIS installer
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

Confirm that [Update Homebrew tap](../../.github/workflows/update-homebrew-tap.yml)
succeeds for this stable desktop tag and that the tap's own native macOS CI
passes. Engine releases and prereleases do not update the tap. A failed tap bump
does not unpublish the desktop release.

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
   mismatch otherwise. Refresh the remote helper's lockfile as well
   (`cargo update --manifest-path crates/antiburn-remote/Cargo.toml --package antiburn-local`):
   it path-depends on the engine too, and the release's remote-helper jobs
   build with `--locked`. Pull request CI does not run those jobs for a
   release-only change, so the mismatch shows first in the app release run.
   If `cargo update` also moves unrelated entries in either lockfile, edit
   only the `antiburn-local` version line by hand instead.
2. Review, merge, then tag `antiburn-local-v<version>` and push the tag.
3. The workflow requires the successful main push run for the exact tag SHA,
   then packages a deterministic source tarball with `LICENSE`, `NOTICE`,
   `THIRD_PARTY_NOTICES`, and the three coverage-test inputs under `docs/`
   (`check-coverage.md`, `session-coverage.md`, and `support.md`). It extracts
   the archive into an isolated directory and requires `cargo test --locked`
   to pass before it inventories the dependency tree, verifies its checksums,
   attests provenance, and drafts the release. The engine release does not
   repeat the desktop platform matrix.
4. Review the draft: verify its checksum and provenance, inspect its contents,
   and read the release notes. The workflow already tests the extracted archive.
   To reproduce that gate locally, extract into an empty directory, enter
   `antiburn-local-<version>`, and run `cargo test --locked`. Coverage tests read
   bundled `docs/` files in the archive and repository-level docs in a checkout;
   missing inputs fail the tests. (While the repository is private, the
   two provenance steps skip themselves — attestation persistence is plan-gated
   on private repositories — and activate automatically once the repository is
   public; a private-repository draft has no provenance bundle to verify.)

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

## Homebrew publication

The macOS cask lives in the public
[`antiburn/homebrew-tap`](https://github.com/antiburn/homebrew-tap) repository.
Use `brew install --cask antiburn/tap/antiburn`. The tap supports stable desktop
releases only; an upstream `homebrew/cask` submission is not required.

[`update-homebrew-tap.yml`](../../.github/workflows/update-homebrew-tap.yml)
runs after a desktop release is published. It reads that exact release and its
`SHA256SUMS`, requires both macOS DMGs, updates only the cask version and two
hashes, and runs Homebrew style and online strict audit before pushing a
signed-off version-bump commit. The tap's push CI then tests both native macOS
architectures. The build and signing workflow still stops at a draft.

Updates are serialized. An identical rerun is a no-op. Older versions and
changed hashes for an existing version fail. Policy changes to the cask need
normal review; the updater does not rewrite them. Published assets never change.

### Tap credential

Store a fine-grained PAT as repository Actions secret `HOMEBREW_TAP_TOKEN` in
`antiburn/antiburn`. Select resource owner `antiburn`, only repository
`homebrew-tap`, and Contents read/write. Metadata read access is automatic.
Do not grant Workflows or Administration permissions. Complete organization
approval if required. The normal product `GITHUB_TOKEN` reads release data;
the PAT authenticates only the tap checkout and push.

```sh
gh secret set HOMEBREW_TAP_TOKEN --repo antiburn/antiburn
```

Use the interactive prompt. Keep the token owner, expiration, and renewal
reminder in the maintainer's private credential record. On rotation, replace the
secret, verify tap access, and revoke the old token. Never log the value.

Same-repository pull requests touching the updater run a non-writing preview
against the current tap release. The preview checks the cask and performs a
dry-run push to verify the credential; it does not commit a bump. Fork pull
requests run the pure script tests through normal CI without the tap secret.
Publication and manual recovery use updater code from the default branch.

### Retry or refresh the cask

After the workflow is merged, dispatch it with an exact published stable tag:

```sh
gh workflow run update-homebrew-tap.yml \
  --repo antiburn/antiburn --ref main \
  -f tag=antiburn-v<VERSION>
```

Inspect the Actions result, the cask version/hashes, and the tap's CI result.
If access fails, check the token's expiration, approval, permissions, and tap
branch rules. If a maintainer changed the tap concurrently, rerun against its
latest state. Do not force-push or downgrade the tap.

If Actions is unavailable, use a product checkout and the current, clean
Homebrew tap checkout:

```sh
tap=$(brew --repository antiburn/tap)
node scripts/update-homebrew-cask.mjs antiburn-v<VERSION> "$tap/Casks/antiburn.rb"
brew style --cask antiburn/tap/antiburn
brew audit --cask --strict --online antiburn/tap/antiburn
```

Review the diff in that tap checkout. Stage only `Casks/antiburn.rb`, then
commit and push the validated change with Conventional Commits and DCO sign-off
under the tap's rules. Do not validate an older installed tap while pushing a
different checkout.

A failed tap update leaves the desktop release available through curl and
manual downloads. Fix the update and rerun the same tag. Do not rebuild or
unpublish the product release to repair the tap.

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

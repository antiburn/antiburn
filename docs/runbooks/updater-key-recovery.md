<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# The updater signing key

Custody, rotation, and what happens if it is lost.

## What the key is

One asymmetric key pair, generated once:

- The **private half** signs every updater bundle at release time. It lives in
  the `release` environment as `TAURI_SIGNING_PRIVATE_KEY`, and nowhere else
  that a build can reach.
- The **public half** is committed to
  `apps/desktop/src-tauri/tauri.conf.json` as `plugins.updater.pubkey` and is
  compiled into every build. It is public by nature; there is nothing to protect
  about it.

An installed copy of antiburn will install an update **only** if the download's
detached signature verifies against the public half its own binary carries. That
sentence is the whole security model of the update channel, and it has a
consequence people usually meet the hard way: *the public key inside an
installed build cannot be changed by an update*. A build only trusts what it
already trusts.

## Current state

The key pair **was minted 2026-08-14.** The public half is committed as
`plugins.updater.pubkey`; the private half and its passphrase live in the
`release` environment (`TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)`) and in the
maintainer's password manager, which holds the canonical copy. Because the
field is non-empty:

- `apps/desktop/src-tauri/src/updates.rs` reports update support as true, and
  release builds check `latest.json` for verifiable updates;
- `.github/workflows/release-app.yml`'s key gate passes (it still refuses to
  build if the field is ever emptied).

`bundle.createUpdaterArtifacts` is `true`, so the pipeline produces signed
bundles. The two settings are one decision: artifacts without a key would be
unverifiable downloads, and a key without artifacts would have nothing to
check.

(The reason that warning is not a comment next to the field: `tauri.conf.json`
is parsed as strict JSON, so a comment there fails the build.)

## Minting it, once

```bash
pnpm --filter @antiburn/desktop exec tauri signer generate -w "$HOME/antiburn-updater.key"
```

The `-w` path is deliberately absolute: `pnpm --filter` runs the tool inside
`apps/desktop`, so a relative path would drop both halves into the working
tree. `.gitignore` guards `*.key` as a backstop, but the private half should
never touch the repository at all.

Give it a passphrase. Then:

1. Put the contents of `antiburn-updater.key` in the `release` environment as
   `TAURI_SIGNING_PRIVATE_KEY`, and the passphrase as
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
2. Commit the printed public key to `plugins.updater.pubkey` in
   `apps/desktop/src-tauri/tauri.conf.json`, through an ordinary reviewed pull
   request. The reviewer's job is to confirm it is the public half of the key
   that was actually stored — a public key nobody holds the private half of
   produces a release nobody can update from.
3. Move the private key file to offline custody (below) and delete every working
   copy, including the shell history entry that wrote it.

Do this once. Every subsequent release uses the same pair.

## Custody

- **Two offline copies, in separate physical locations**, held by separate
  people. An encrypted volume or a password manager's secure-file storage; not a
  cloud drive, not a chat message, not a repository.
- **The passphrase is stored separately from the key.** A copy of both in one
  place is one copy, not two.
- **A written record** of who holds what and when it was last verified. Verify
  annually by decrypting a copy and checking that its public half still matches
  the committed one.
- **Never on a laptop used for daily work**, and never in a `.env`, a lockfile,
  a CI cache, or an artifact.

The private key is more sensitive than the code signing certificates: those can
be revoked by a third party who is motivated to help you, while this key is
trusted by binaries that are already on other people's machines and cannot be
told otherwise.

## Rotation

Rotate when the private key may be in someone else's hands, or when both custody
copies are lost. Do not rotate as routine hygiene — the cost falls entirely on
readers.

**What rotation costs.** Every installed build verifies against the old public
key. Bundles signed with the new key will not verify, so those installations
cannot update at all: they will report a failed check in Settings → About and
install nothing. They are not broken and they are not unsafe — they are stranded
until somebody reinstalls by hand. The failure is a refusal, not a silent
downgrade, which is the correct direction but is little comfort to the reader.

Procedure:

1. **Decide, and write down why.** Rotation is a decision with an external cost;
   it belongs in the advisory or the release notes.
2. Mint a new key pair exactly as above, into fresh custody. Do not reuse the
   old passphrase.
3. Replace the secret in the `release` environment and commit the new public key
   through a reviewed pull request.
4. Cut a release with the new key, per [`release.md`](release.md).
5. **Say so, loudly and in the places people will actually look:** the release
   notes, the repository README, and a pinned security advisory if the rotation
   was caused by a compromise. The message a reader needs is short and specific:
   *automatic updates cannot carry you across this change; download and install
   the new version by hand, once.*
6. Destroy the old private key only after the advisory is out and the release is
   published — until then it is evidence, and it is also the only thing that can
   sign a last update for the old population if that turns out to be possible.

### What readers have to do

Download the new version from the releases page and install it over the existing
one. Nothing is lost by doing so: the local database lives in the application
data directory and is untouched by a reinstall. There is no migration, no export
first, and no account to re-authenticate — antiburn has none.

## If the key is lost but not exposed

Both custody copies are gone; nobody else has it either. This is a rotation with
one difference: there is no urgency and no advisory, so you can schedule it with
a release people were going to install anyway. Announce it in the release notes
of the *preceding* version if you can — a reader who is told in advance that the
next update must be installed by hand will do it.

## What is never done

- Committing the private key, in any form, to this repository.
- Storing the private key and its passphrase in the same place.
- Signing a release outside the `release` environment.
- Rotating quietly. A rotation that is not announced looks exactly like a broken
  updater to every reader who has one.

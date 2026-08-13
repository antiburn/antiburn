<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Rolling back a published release

A version is out and it is bad: it crashes on launch, it loses the local index,
it ships the wrong architecture, its `latest.json` is wrong. This is how it gets
undone.

**Nothing is deleted and nothing is replaced.** The fix is always a new version
that supersedes the bad one. If the problem is that a signing key or a build may
be in someone else's hands, this is the wrong runbook — go to
[`compromised-release.md`](compromised-release.md) first and come back.

## Why not just delete it

Because deletion does not do what it appears to do:

- Anyone who already installed the bad version keeps it, and deleting the
  release removes the one thing that could have offered them a way out.
- The assets have been mirrored, linked, and cached. The URL going dead breaks
  those without telling anyone why.
- Deleting the release detaches the provenance attestation and the published
  checksums from the bytes people are still running, which is exactly the
  evidence you need while diagnosing.
- Replacing an asset in place is worse still: the published SHA-256 and the
  updater signature both describe the *old* bytes. Every verification a careful
  reader performs would now fail, and they cannot tell your correction from an
  attack.

Pulling readers forward is fast (the app checks every six hours, and on demand).
Pulling them backwards is not possible at all — the updater only moves forward,
and that is by design.

## What actually stops the bleeding

The application asks `releases/latest/download/latest.json` whether there is
something newer. Two things decide what that returns:

1. Which release carries the **Latest** badge.
2. What the `latest.json` on that release says.

So a rollback is: make a good release the latest one.

## Procedure

### 1. Decide what "good" is

Either the previous known-good version, or a new fix on top of the bad one.
Prefer the fix if you have it; prefer speed if you do not — a re-release of the
previous code under a new version number is a legitimate corrective release, and
frequently the right one.

### 2. Cut a corrective release

Follow [`release.md`](release.md) exactly, with a higher version than the bad
one. A revert of the offending commits plus a patch bump is the usual shape:

```text
1.2.0   bad
1.2.1   corrective — "Revert the … introduced in 1.2.0"
```

The changelog entry says plainly what went wrong and what this version does
about it. People who were bitten will read it, and the app shows it to them in
the update prompt.

### 3. Publish it and confirm it is the latest

After publishing, check that the badge moved:

```bash
curl -sSL https://github.com/antiburn/antiburn/releases/latest/download/latest.json | jq .version
```

That must print the corrective version. If it prints the bad one, the Latest
badge is still on the bad release — fix it on the corrective release
(Edit release → "Set as the latest release"), not by deleting anything.

### 4. Mark the bad release, without unpublishing it

On the bad release, edit the notes and put a line at the very top:

```markdown
> **Superseded by [1.2.1](…).** This version <what went wrong>. Update before
> using it. Its assets remain here, unchanged, so existing installations can
> still be verified against their published checksums.
```

Optionally tick **"Set as a pre-release"** on the bad release. This does not
remove it, does not break any link, and does not alter a byte — it just stops it
being offered as the latest download to somebody arriving at the releases page.

### 5. Tell people

- The corrective release's own notes (they are shown in the update prompt).
- The security channel in [`SECURITY.md`](../../SECURITY.md), if the fault has a
  security dimension — and then also
  [`security-releases.md`](security-releases.md).

## If the bad thing is `latest.json` itself

Same procedure. A malformed or wrongly-pointed `latest.json` cannot be edited on
a published release; it is superseded by publishing a new release whose
`latest.json` is correct. The window in which readers see the broken manifest is
bounded by how long the corrective release takes: an update check that fails is
reported in Settings → Updates and retried, and it installs nothing, so a broken
manifest is an outage rather than a hazard.

## What is never done

- Deleting a published release or its tag.
- Uploading a corrected file over a published asset.
- Editing `latest.json` in place.
- Re-tagging a version that has already been published.

If you believe one of these is the only option, the situation is a compromise,
not a rollback. Go to [`compromised-release.md`](compromised-release.md).

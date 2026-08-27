# A compromised release

For when a published artifact, a signing key, or the pipeline that produced them
may be under someone else's control. This is the worst thing that can happen to
a desktop application that ships an updater, because the updater is a channel
for running code on other people's machines.

Assume compromise if any of the following is true and you cannot rule it out
within the hour:

- The updater signing key, the Apple `.p12`, or the Authenticode `.pfx` was
  exposed — pasted somewhere, committed, exfiltrated, or held on a machine that
  was compromised.
- A published asset's SHA-256 does not match what the run produced.
- A release exists that no maintainer cut, or a tag was created by an account
  that should not have been able to.
- `gh attestation verify` fails for a published asset.
- A workflow run shows steps or jobs that are not in the workflow file at that
  commit.

**Speed matters less than not making it worse.** A wrong move here — deleting
evidence, rotating a key before you know what it signed — costs more than the
extra twenty minutes.

## 0. Before anything else: preserve the evidence

Do this first, in this order, and do not skip it.

- Download every asset of every affected release, keep them, and record their
  SHA-256 values. Do **not** delete or replace anything published.
- Export the Actions run logs for the affected releases.
- Record the repository's audit log (Settings → Audit log) for the period.
- Note which key material was where, and since when.

Deleting the release destroys exactly what you need to work out what was
shipped, to whom, and for how long.

## 1. Stop the bleeding

The one lever that matters is the **Latest** badge, because that is what
installed copies ask about. In order:

1. If a *bad* release currently carries the badge, move it — publish a corrective
   release (below), or in the extreme case mark the bad one as a pre-release so
   it stops being offered as the latest download. Editing which release is
   latest changes no bytes and is not a violation of the immutability rule.
2. Revoke the credentials that could produce another one:
   - Delete the affected secrets from the `release` environment.
   - Set the environment's deployment-tag rule to a pattern that matches
     nothing, so no build can start while you work.
   - Revoke or rotate any personal access token, deploy key, or app
     installation implicated.
3. If an account was taken over, remove its access and require the owner to
   re-authenticate everywhere before restoring it.

## 2. Revoke the signing material

| Material | Action | Effect on readers |
| --- | --- | --- |
| Apple Developer ID certificate | Revoke in the Apple Developer portal, then request a new one | Revocation invalidates existing signatures; notarization tickets already stapled to shipped builds continue to validate, and Apple can also revoke the notarization of a specific build if it is malicious. Contact Apple if the build is actively harmful. |
| Authenticode certificate | Contact the issuing certificate authority and request revocation with the correct reason code | Windows checks revocation; a timestamped signature made *before* the revocation date normally stays valid, which is why the revocation reason and date matter. Say so to the CA explicitly. |
| Updater signing key | Rotate it — this is the hard one | See [`updater-key-recovery.md`](updater-key-recovery.md). A rotated key means existing installations cannot verify updates signed with the new key and must be reinstalled by hand. |

Rotating the updater key is a decision with a real cost to every installed copy.
Take it when the key itself may be in someone else's hands. Do not take it
because a *build* was bad — that is a rollback.

## 3. Ship a corrective release

Once you can build safely again — new credentials, a reviewed tree, a pipeline
you trust:

1. Confirm the source is clean. Review the commits in the affected range, the
   workflow files at those commits, and every dependency change.
2. Restore the `release` environment with new credentials, and restore the
   deployment-tag rule.
3. Cut a corrective release with a higher version, per
   [`release.md`](release.md). Its notes state, in plain words, that earlier
   version X may have been tampered with and that readers should update.
4. Publish it and confirm it is the latest release.

## 4. Publish an advisory

Use GitHub Security Advisories on this repository (the same channel
[`SECURITY.md`](../../SECURITY.md) points reporters at). The advisory says:

- which versions and which assets are affected, by filename and SHA-256;
- the window during which the bad assets were reachable;
- how a reader can tell whether they have one (`sha256sum` against the published
  list, `gh attestation verify`, and on macOS `codesign -dv --verbose=4`);
- what to do — which version to move to, and whether a reinstall rather than an
  update is required;
- whether the updater signing key was rotated, and therefore whether an in-app
  update can work at all.

Do not wait for a complete story. An advisory that says "we are still
establishing the scope" on the day it happened is worth more than a perfect one
a week later.

## 5. Afterwards

Write down what let it happen and what changed as a result. The candidates worth
checking every time:

- Where private keys live, who can read them, and whether that is still true.
- Whether the tag ruleset actually restricted tag creation to the people you
  thought.
- Whether the `release` environment's tag rule was as narrow as it should be.
- Whether every third-party action is still pinned by commit SHA, and whether
  any pin moved.
- Whether provenance attestation would have caught it earlier, and if not, why.

## What is never done

- Deleting the compromised release, its tag, or its assets. That is destroying
  evidence, and it does not protect anyone who already installed it.
- Replacing a published asset with a fixed one under the same name. A reader
  cannot distinguish your correction from the attack.
- Rotating the updater key before you know what was signed with it.
- Publishing a "quiet" fix. If the updater channel may have been abused, readers
  are entitled to know before they are asked to install something else.

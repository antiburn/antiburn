# Runbooks

Procedures that are followed under time pressure, written down before the
pressure arrives. Each one states what it is for, what it assumes, and what
"done" looks like.

| Runbook | Read it when |
| --- | --- |
| [`release.md`](release.md) | Cutting a release, or setting the repository up so a release is possible at all |
| [`rollback.md`](rollback.md) | A published release is bad and readers need to stop landing on it |
| [`compromised-release.md`](compromised-release.md) | A signing key, a build, or a published asset may be in someone else's hands |
| [`updater-key-recovery.md`](updater-key-recovery.md) | The updater signing key is lost, rotated, or suspected exposed |
| [`security-releases.md`](security-releases.md) | A vulnerability report has arrived and a fix has to ship |
| [`branch-rules.md`](branch-rules.md) | Changing what `main` requires before a merge, or unblocking a merge the rule refuses |

## The rule these all rest on

**A published tag and its assets are immutable.** Nothing in antiburn ever
replaces a file under a tag that has been published, deletes a published
release, or moves a tag. Every correction is a new version.

This is not fastidiousness. Installers get mirrored, linked, and cached; a
checksum gets pasted into somebody's build script; the updater signs the exact
bytes it published. Silently changing what a URL returns breaks all three and
leaves no trace that it happened — the one failure mode that a reader cannot
detect from the outside. A corrective release is visible, verifiable, and cheap.

Drafts are the opposite: they are not immutable, nobody can reach them, and the
release workflows will happily rebuild one. That asymmetry is the whole reason
releases here are drafted first and published by a person.

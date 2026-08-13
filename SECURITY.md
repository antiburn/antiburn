# Security Policy

## Reporting a vulnerability

Please report vulnerabilities **privately** via GitHub's security advisories:

**[Report a vulnerability](https://github.com/antiburn/antiburn/security/advisories/new)**

Do not open public issues for security reports. We will acknowledge reports
promptly, keep you informed of progress, and credit reporters in the fix's
release notes unless you prefer otherwise.

## Scope

antiburn runs entirely on-device. Reports of particular interest:

- any code path that could cause network egress from `antiburn-local`;
- discovery escaping its documented provider roots, following symlinks out of
  an approved root, or writing to provider-owned stores;
- reads of credential, cookie, token, account, or billing data;
- transcript content leaking into logs, derived storage, or analytics.

## Supported versions

Security fixes target the latest release; corrective releases are published
rather than replacing assets under an existing tag.

## What happens after you report

How a report becomes a shipped fix, and what we do if a release or a signing key
is ever compromised, is written down rather than improvised:

- [`docs/runbooks/security-releases.md`](docs/runbooks/security-releases.md) —
  acknowledgement, the private fix, the release, and when the advisory goes out
- [`docs/runbooks/compromised-release.md`](docs/runbooks/compromised-release.md)
  — a published artifact or signing key in someone else's hands
- [`docs/runbooks/rollback.md`](docs/runbooks/rollback.md) — superseding a bad
  release without deleting or replacing anything published
- [`docs/runbooks/updater-key-recovery.md`](docs/runbooks/updater-key-recovery.md)
  — custody and rotation of the update signing key

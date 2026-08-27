# Security Policy

## Reporting a vulnerability

Please report vulnerabilities **privately** via GitHub's security advisories:

**[Report a vulnerability](https://github.com/antiburn/antiburn/security/advisories/new)**

Do not open public issues for security reports. We will acknowledge reports
promptly, keep you informed of progress, and credit reporters in the fix's
release notes unless you prefer otherwise.

## Scope

antiburn processes session data on-device and needs no account or backend.
Optional analytics sends only the fields documented in
[`docs/analytics.md`](docs/analytics.md). Reports of particular interest:

- any code path that sends session content, credentials, or anything else
  about you to a service of ours, or to any third party — the one hard line
  antiburn holds; returning your own token to its issuing provider is not a
  violation of this, but sending it anywhere else is;
- any code path that could cause network egress from `antiburn-local`, which
  is supposed to have none;
- discovery escaping its documented provider roots, following symlinks out of
  an approved root, or writing to provider-owned stores;
- a credential or token read for one purpose (calling its own issuing
  provider, on your behalf) ending up logged, cached beyond that purpose, or
  sent anywhere else;
- transcript content leaving app-controlled local storage, appearing in logs,
  or being exposed outside a visibility or analysis feature that needs it.

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

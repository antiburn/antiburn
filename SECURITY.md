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

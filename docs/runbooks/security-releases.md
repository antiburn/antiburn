<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Shipping a security fix

A vulnerability report has arrived and a fix has to reach people. This runbook
covers the release side of that; the reporting side belongs to
[`SECURITY.md`](../../SECURITY.md), which is where reporters are sent and which
is the only channel antiburn asks them to use.

If a signing key or a published artifact may be in someone else's hands, this is
not the runbook — start at
[`compromised-release.md`](compromised-release.md).

## Receiving a report

Reports arrive as private GitHub security advisories. On arrival:

1. **Acknowledge within a day**, even if the answer is "we are still reading
   it". A reporter who hears nothing assumes nothing is happening and starts
   thinking about disclosure.
2. **Reproduce it**, and say whether you could. A report you cannot reproduce is
   not a report you can dismiss — ask for the missing detail.
3. **Decide the severity in terms of this application.** antiburn runs
   on-device, as the reader, and needs no connection to any service of ours;
   the one call it makes on its own account is the update check. The classes
   that matter most, from `SECURITY.md`:
   - anything that sends session content, credentials, or anything else about
     the reader to a service of ours, or to any third party;
   - anything that causes network egress from `antiburn-local`, which is
     supposed to have none;
   - discovery escaping its documented provider roots, following a symlink out
     of an approved root, or writing to a provider-owned store;
   - a credential or token read for one purpose ending up logged, cached
     beyond that purpose, or sent anywhere but its issuing provider;
   - transcript content leaving app-controlled local storage, reaching logs,
     or appearing in an export that did not ask;
   - anything reachable through the updater, which is the one channel that can
     cause code to run on someone else's machine.

## Fixing it in private

Use a **private fork** of the advisory (GitHub creates one from the advisory
itself). Develop and review the fix there. Nothing about the vulnerability goes
into a public branch, issue, commit message, or changelog entry before the fix
is published — a public commit that says "bounds-check the transcript path" is a
disclosure with a countdown attached.

Keep the fix as small as the fault. A security release is the worst possible
place to also land a refactor: the reviewers are working under time pressure and
the reader has to take the whole thing.

Write a test that fails without the fix. If the fault crossed one of the
mechanical boundaries this repository already enforces — the engine's
`tests/boundary.rs`, the whole-tree `scripts/check-boundary.mjs`, the frontend's
`tests/no-exfiltration.test.ts` — extend that check as part of the fix. A
boundary that was crossed once had a gap in it, and the test is what closes
the gap rather than the patch.

One distinction to make before you harden anything: antiburn has **one**
sanctioned outbound channel besides the update check — the anonymised
usage-analytics publisher in `src-tauri/src/usage_analytics`, recorded as D-027
and deviations D-28. Traffic from it is not a breach. What *would* be a breach
is that channel carrying a field its event schema does not name, reaching an
endpoint the build did not inject, or sending while the reader's consent is
off. Check the schema and the gate before concluding the boundary held.

## Releasing it

An ordinary release, cut per [`release.md`](release.md), with three differences.

1. **Version.** A patch bump on the supported line. Security fixes target the
   latest release, as `SECURITY.md` states — antiburn does not maintain
   long-term branches, so "backporting" means telling people to move to the
   current version.
2. **Notes.** Say what the fault allowed, in the reader's terms, and what to do.
   Credit the reporter by name unless they asked otherwise. Include the advisory
   identifier once it exists. Do not include the exploit.
3. **Timing.** Merge the fix to `main`, tag, and publish in one sitting. The
   window between a public fix commit and a published release is the window in
   which the fault is known and unfixed for everyone.

## Publishing the advisory

Publish the GitHub advisory once the release is out, not before. It should
carry:

- affected versions and the fixed version;
- what an affected reader is exposed to, stated plainly;
- how to tell whether they are affected, if that is knowable;
- the fix — normally "update to X.Y.Z";
- credit for the reporter.

Then, if the fault reached a shipped release, mention it in the following
release's notes as well. Readers who skipped a version still need to know.

## Ordering, and why it is this way

```text
report → acknowledge → reproduce → private fix → release → advisory
```

The release comes before the advisory because the advisory is what makes the
fault public, and a public fault with no available fix helps only whoever is
looking for one. The acknowledgement comes first because a reporter's patience
is the only thing keeping that order intact.

## What is never done

- A silent fix. If a shipped version was vulnerable, the advisory is published
  even when the fault was found internally and never reported.
- A fix committed publicly before the release that carries it.
- Asking a reporter to sign anything, or offering payment in exchange for
  silence.
- Shipping a security fix in a release that also carries unrelated risk.

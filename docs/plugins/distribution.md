# Plugin distribution

The current repository candidate is **antiburn** 0.2.5 for Claude Code.
It fixes Desktop meter width and AbovePrompt composition while preserving quota
behavior and the embedded 0.2.1 shared core. It is generated locally as `plugins/antiburn`,
with a separate local-only `antiburn-dev` build.
Codex, Cursor, OpenCode and Pi adapters remain unpublished future candidates.
The user-facing name remains antiburn; the internal package is
`antiburn-plugin-core`. For how sources, generated builds and catalogs connect,
see the [plugin architecture](architecture.md).

There are no real users of the old `antiburn-mod` identity. Its obsolete 0.1.4
snapshot is retained in Git history, not in the working tree. A rename map,
legacy cache migration and independent recovery marketplace are unnecessary
for this initial launch. Fresh installs, current-version updates, removal and
public/dev independence still require validation.

The repository remains the source of truth. The [plugin page](../../plugins/README.md)
is its dedicated GitHub folder page. The candidate marketplace catalog lists
public `antiburn` 0.2.5 from `plugins/antiburn`, pinned to an immutable artifact
commit. The catalog on `main` points to an earlier unreleased 0.2.5 candidate.
This revision supersedes that candidate before acceptance; the submitted
0.2.4 artifact and tag remain unchanged. This public version contains exactly 24 files; the
[Claude catalog](../../.claude-plugin/marketplace.json) records the authoritative
artifact SHA. Keep submitted and released bytes immutable. Unreleased candidate
revisions need acceptance against their exact artifact SHA. Use a fresh host
profile when comparing candidate snapshots with the same version. When the catalog pins
a commit introduced by a pull request, merge that pull request with a merge
commit. Do not squash or rebase it: those methods replace the pinned commit in
main history and can leave fresh CI checkouts without the artifact object. Use the permanent
marketplace commands in the [plugin guide](../../plugins/README.md#install).
The local `antiburn-dev` build stays outside the hosted catalog. Its burn
animation and preview controls are temporarily disabled; live checks and
actions remain.

## Public installation route

Public installs add `https://github.com/antiburn/antiburn.git` without a branch
fragment. The catalog is merged into `main` and selects the immutable artifact
SHA. Installation does not depend on retaining the release branch. This route
makes the pinned plugin available without a GitHub Release or tag.

Hosted fresh-install and reinstall checks must verify the exact pinned files;
local transport checks alone are insufficient. Verify these checks through the
permanent main route. Do not treat the catalog merge as evidence that all hosted
acceptance checks have passed.

For 0.2.5, verify an update from the immutable 0.2.4 artifact, as well as fresh
installation, reinstall and removal. Internal testing marketplaces are disposable
test setup; they are not a supported user upgrade route. Keep the historical
0.2.1 pin and bytes available for recovery.

## Later repository separation

A separate generated distribution repository can come later without moving the
source immediately. Export the same self-contained artifact, licenses, source
commit and hashes. Keep plugin identity and command/state namespaces stable;
verify fresh install, updates and removal against the exported bytes. Use one
authoritative generator and one destination per release. Leave a forwarding
README here after the new route is verified. Creating or publishing that
repository requires a separate request.

## Other agents

Keep host-specific catalogs separate: they do not share a schema. Codex remains
skills-only; Cursor uses its canonical `/antiburn` skill; OpenCode separates
its command asset from the optional
runtime; Pi retains its extension package. No listing, registry package,
Desktop compatibility or cross-vendor quota support is established by assembly.
Run the vendor's installation and native acceptance matrix when that vendor is
selected for release. Their certification is deferred from the Claude milestone.

## Vendor rollout documentation checklist

Complete these documentation updates as part of each vendor rollout, before presenting its installation route as available:

- Validate the actual distribution source, package identity, host versions and native surfaces. Development runbook commands are not approved public installation instructions.
- Add the verified public installation, invocation, update and removal procedures to [installation.md](installation.md). Keep each vendor's prerequisites and limitations explicit.
- Update the supported-agent table in the [plugin page](../../plugins/README.md#supported-agents), the [compatibility record](compatibility.md), and the [validation scope](validation.md) with evidence for that vendor.
- Update this distribution guide with the approved channel and promotion requirements.
- Update the [architecture map](architecture.md) when the rollout changes source ownership, assembly inputs/outputs, adapter packaging or catalog boundaries.
- Remove or revise that vendor's unpublished label in [development-adapters.md](development-adapters.md). Keep contributor-only testing procedures separate and link to the public guide.
- Check relative links and examples, and confirm that unshipped vendors still carry an unpublished status. Include these documentation changes in the vendor's rollout PR.

Publishing, tags, uploads, marketplace changes exposed to users, listing
submissions, pushes and release drafts require separate authorization.

## Claude directory submission

The submitted 0.2.4 candidate selects `claude-plugin-v0.2.4` at
`f795e08059e993d7f226c4891c845a9f91e63c68`, with marketplace artifact
`ceb2800b4bb9b25744624b7ee309ef9532f596d0`. The owner reports that its security
scan passed and policy review is underway. Preserve its bytes and tag. The 0.2.5
candidate is local and unsubmitted; no portal setting or tracked reference changes. The
recommended directory release channel is a dedicated `claude-plugin-release`
branch, as described below; it is not configured by this documentation change.
The repository marketplace selects an immutable artifact commit independently.
Compare all installed files and both source trees before submission. Keep
automatic publication **off**. The submission repository and folder remain
[antiburn/antiburn](https://github.com/antiburn/antiburn), `plugins/antiburn`.
The listing homepage is
[antiburn.com/plugin](https://antiburn.com/plugin). The owner validates and
submits manually through the [Claude developer portal](https://claude.ai/directory/manage),
following the [submission workflow](https://claude.com/docs/plugins/submit) and
[pre-submission checklist](https://claude.com/docs/plugins/pre-submission-checklist).

### Directory update channel

The directory tracks the branch or tag entered in the portal. Its tracked
reference is separate from the repository marketplace catalog's artifact SHA.
Changing one does not change the other. See Claude's
[update workflow](https://claude.com/docs/plugins/submit#update-a-published-plugin)
and [tracked-reference settings](https://claude.com/docs/plugins/submit#change-the-tracked-branch-or-tag).

| Tracked reference                               | How a new candidate reaches Claude                                   | Release control                                                     |
| ----------------------------------------------- | -------------------------------------------------------------------- | ------------------------------------------------------------------- |
| `main`                                          | New commits on `main` are eligible for directory checks.             | App development also advances the tracked snapshot.                 |
| `claude-plugin-release` (recommended)           | Promote a tested plugin release commit to this dedicated branch.     | App-only merges to `main` do not advance this branch.               |
| A versioned tag, such as `claude-plugin-v0.2.4` | Create a new tag for each release and select that tag in the portal. | The selected commit stays fixed until the portal reference changes. |

Use the dedicated release branch for ongoing updates without changing the
portal's tracked reference for every release. Before advancing it, raise the
plugin version, regenerate the public package, complete the release checks, and
verify that the plugin tree matches the reviewed immutable artifact selected by
the repository catalog. Promote only the tested commit, not every app merge.
Claude discovers the promoted commit through its scheduled check or configured
GitHub webhook, then scans it. With automatic publication off, complete the
portal's required publish and review steps explicitly.

For a tag-based release, complete the same checks, create a new versioned tag at
the tested commit, and change the portal's tracked reference to that new tag.
Never move or replace an existing versioned tag. Re-validate after changing the
source reference and complete the required scan, review and publication steps.
The last published plugin remains live until a replacement is published. Claude
does not allow changing the tracked reference while the plugin is with a reviewer.
A Git tag does not require a GitHub Release.

People installing the plugin receive only the selected plugin folder. However,
some validation checks cover the wider repository, including archive size and
layout. A release branch or fixed tag holds the repository snapshot steady while
app development continues on `main`; it does not exempt that snapshot from
repository-wide checks. See the
[pre-submission checklist](https://claude.com/docs/plugins/pre-submission-checklist).

Moving the directory to a release branch or tag does not change the permanent
repository marketplace installation commands: that catalog can remain on `main`
and continue selecting an immutable artifact SHA. Creating or pushing a release
reference, changing portal settings and publishing remain separate release actions.

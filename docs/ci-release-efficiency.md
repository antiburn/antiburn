# CI and release efficiency

CI selects checks by stage and changed workspace. Release workflows require a
successful `main` push run for the exact release SHA instead of repeating the
full check matrix. These rules reduce duplicate work without changing release
outputs or publication approval.

## Staged CI

`ci.yml` selects its checks by stage. The `classify` job reads the diff, pull
request labels, and draft state from the API. A rerun after a label change sees
the new label. A push that arrives with "ready for review" runs the ready stage.

| Stage | When | Rust platforms | Frontend platforms | Other checks |
| --- | --- | --- | --- | --- |
| draft | Draft pull request without `ci:full` | Linux | Linux | Changed-code quality scan; no licenses, installers, or remote helper build |
| ready | Other pull request events | Linux, Windows, macOS | Linux, Windows | Changed-code quality scan; licenses, installers, advisories, and remote helper build when affected |
| main | Push to `main` | Linux, Linux ARM64, Windows, Windows ARM64, macOS | Linux, Windows | Full-tree quality scan, affected advisories, release credentials, Clippy without analytics, remote helper build, and release cache warming |
| nightly | Schedule at 03:17 UTC or `workflow_dispatch` | Linux, Linux ARM64, Windows, Windows ARM64, macOS | Linux, Windows | All checks, including `cargo deny check advisories` |
| release | App or engine tag push | Not repeated | Not repeated | Successful `main` push run for the exact tag SHA |

The diff selects a changed Rust workspace and every workspace that depends on
it, except on nightly runs, which select all workspaces. Most documentation
changes select no workspace. A file read by a workspace selects that reader:
for example, `docs/support.md` selects the engine and shell through
`CROSS_WORKSPACE_INPUTS`. Workflow changes and unknown paths select everything.

### Jobs

| Job | Responsibility |
| --- | --- |
| `classify` | Design drift, classifier and workspace-registry tests on every event, then stage and workspace selection |
| `ci-scripts` | CI and release script tests when scripts, workflows, or root package files change |
| `repository` | Pull request DCO checks, main release credentials, secret scan, and changed-code or full-tree quality scan |
| `engine` | Format, Clippy, and Nextest for the engine and remote helper workspaces, with one target directory per platform |
| `desktop-backend` | Shell and path-crate checks in parallel parts per platform; shell uses analytics, with a separate no-analytics Clippy pass on Linux main and nightly runs |
| `desktop-frontend` | Format, lint, type check, Knip, and build on Linux; tests on each frontend platform |
| `remote-helper` | Static musl build and packaging; engine CI owns its checks, while release builds retain their tests |
| `licenses` | Dependency bans, licenses, advisories, and third-party notices |
| `installer-tests` | POSIX and PowerShell installer tests and linters |
| `release-metadata` | Version, tag, and changelog checks for version changes |
| `warm-release-cache` | Unsigned release compilation on main after an app version change |
| `ci-required` | Required aggregate check for classification, repository checks, and every selected job |

Files read outside a workspace, path dependencies, and doctests must remain
visible to workspace selection. An intentionally unselected job may be skipped;
a selected job must succeed.

### The `ci:full` label

Add `ci:full` to a draft pull request to run the ready stage. Then use
**Re-run all jobs** on the latest run. The label itself does not start a run.
**Re-run failed jobs** does not rerun classification and cannot see the label.

Drafts use a reduced Linux-only matrix without license or installer checks.
GitHub does not merge draft pull requests. `ready_for_review` starts a ready-stage
run that replaces the aggregate result.

## Cache ownership

Rust CI caches are saved only on `main` pushes and nightly runs for Linux,
Windows, and macOS. ARM platforms build without a CI cache. The engine group
saves one cache per platform. Desktop shell and path-crate parts share a key;
only the shell saves it, and the crate part restores it. Each key hashes every
workspace lockfile in the job.

Keep caches within the repository's configured storage limit. Cache warming
receives no signing or release-write secrets. Release jobs restore caches
read-only after secrets become available. A cache does not replace required
checks or authorize publication.

## Exact-SHA release gate

`scripts/wait-for-main-ci.mjs` requires a completed, successful `push` run on
`main` for the exact tag SHA. It accepts a matching successful run even if another
matching run failed or was canceled. Without a success, it waits for a matching
run that is still active; missing runs or only completed failures block release.
Non-main, non-push, and wrong-SHA runs cannot satisfy the gate. Waiting does not
reduce required coverage.

Non-PR CI concurrency groups include the SHA. A later main push therefore does
not cancel or queue behind the run needed by an earlier release. Pull request
runs still replace older runs of the same pull request.

## Output and safety invariants

Efficiency changes must preserve the release contract:

- App drafts contain the required installers, updater archives, detached
  signatures, SBOMs, checksums, and `latest.json` entries.
- Engine drafts contain the deterministic source archive, inventory, checksums,
  notes, and eligible provenance bundle.
- Release validation checks asset names, counts, signatures, checksums, URLs,
  and platform coverage.
- Workflows create drafts only. Installation checks, release-note review, update
  testing, and publication remain human decisions.
- Cache changes cannot weaken the exact-SHA gate or expose release secrets.

Use [the release runbook](runbooks/release.md) for release procedures.

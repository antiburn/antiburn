<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Branch rules

This runbook defines the repository rule for `main`. It also gives the commands
to apply, verify, and disable the rule.

## What the rule does

The rule requires the `ci-required` status check on `main`. It pins the check to
the GitHub Actions app. A red or missing check prevents a merge or an unchecked
branch update.

The rule does not require pull requests or resolved conversations. It does not
prevent force-pushes or deletion. GitHub can accept a local merge when the
up-to-date branch already has a passing `ci-required` check.

The condition names `refs/heads/main`. Edit the payload and reapply the ruleset
if the repository renames the branch.

## Sweep the open pull requests

Run this command before an administrator applies the rule. It tests every open
pull request head against `origin/main` plus the current seam head.

Exit 0 authorizes the apply. Exit 1 means that one or more heads failed or had a
merge conflict. Exit 2 means that the sweep did not complete. Record each pull
request number, head SHA, result, and the final exit code.

```bash
set -u
die() { echo "SWEEP ABORTED: $*" >&2; exit 2; }
REPO=$(git rev-parse --show-toplevel) || die "not a git checkout"
SEAM_HEAD=$(git -C "$REPO" rev-parse HEAD) || die "cannot resolve SEAM_HEAD"
git -C "$REPO" fetch -q origin main || die "fetch of origin main failed"
SWEEP=$(mktemp -d) && rm -rf "$SWEEP" || die "cannot allocate a scratch path"
cleanup() {
  git -C "$REPO" worktree remove --force "$SWEEP" >/dev/null 2>&1 || true
  git -C "$REPO" worktree prune >/dev/null 2>&1 || true
  git -C "$REPO" for-each-ref --format='%(refname)' refs/sweep/ 2>/dev/null |
    while read -r r; do git -C "$REPO" update-ref -d "$r"; done
}
trap cleanup EXIT
git -C "$REPO" worktree add -q --detach "$SWEEP" origin/main || die "worktree add failed"
GIT_S="git -C $SWEEP -c user.name=sweep -c user.email=sweep@local"
$GIT_S merge -q --no-ff -m "sweep base" "$SEAM_HEAD" || die "base merge of $SEAM_HEAD failed"
SWEEP_BASE=$(git -C "$SWEEP" rev-parse HEAD) || die "cannot resolve SWEEP_BASE"
"$REPO/node_modules/.bin/aislop" --version || die "the pinned aislop is not runnable"
HEADS=$(gh pr list --state open --json number --jq '.[].number') || die "gh pr list failed"
[ -n "$HEADS" ] || die "gh pr list returned no open head"
FAILED=0
for N in $HEADS; do
  git -C "$REPO" fetch -q --force origin "pull/$N/head:refs/sweep/$N" || die "fetch of PR $N head failed"
  H=$(git -C "$REPO" rev-parse --short "refs/sweep/$N") || die "cannot resolve the PR $N head"
  git -C "$SWEEP" reset -q --hard "$SWEEP_BASE" || die "reset to SWEEP_BASE failed"
  if ! $GIT_S merge -q --no-ff -m "pr$N" "refs/sweep/$N" >/dev/null 2>&1; then
    $GIT_S merge --abort >/dev/null 2>&1 || true
    echo "PR $N head=$H result=CONFLICT"; FAILED=1; continue
  fi
  ( cd "$SWEEP" && "$REPO/node_modules/.bin/aislop" ci --changes --base "$SWEEP_BASE" )
  RC=$?
  echo "PR $N head=$H aislop_exit=$RC"
  [ "$RC" -eq 0 ] || FAILED=1
done
echo "SWEEP_FAILED=$FAILED"
exit "$FAILED"
```

A nonzero result blocks the apply. Fix or close the pull request, add a justified
`aislop-ignore` directive that cites issue #90, or ask the author to rebase a
conflicted head. Run the full sweep again after the route is complete. Only a
new exit 0 authorizes the apply.

## Apply the rule

A repository administrator runs this command only after a clean sweep. It
creates the ruleset and prints its ID. Save that ID as `$RS`.

```bash
gh api --method POST repos/antiburn/antiburn/rulesets \
  --input .github/rulesets/main-required-checks.json --jq .id     # -> $RS
```

## Verify the rule

An agent runs the read-only binding check after the apply. The check binds every
committed parameter. The diff that follows is an advisory drift read.

```bash
gh api "repos/antiburn/antiburn/rulesets/$RS" --jq \
  '(.rules[]|select(.type=="required_status_checks")|.parameters) as $p | [
    .target=="branch", .enforcement=="active",
    ((.bypass_actors // [])|length)==0,
    .conditions.ref_name.include==["refs/heads/main"],
    ((.conditions.ref_name.exclude // [])|length)==0,
    ([.rules[]|select(.type=="required_status_checks")]|length)==1,
    $p.strict_required_status_checks_policy==false,
    $p.do_not_enforce_on_create==false,
    $p.required_status_checks==[{"context":"ci-required","integration_id":15368}]
  ] | all' | grep -qx true

diff <(gh api "repos/antiburn/antiburn/rulesets/$RS" --jq \
        '{name,target,enforcement,bypass_actors:(.bypass_actors // []),conditions,rules}' | jq -S .) \
     <(jq -S '{name,target,enforcement,bypass_actors:(.bypass_actors // []),conditions,rules}' \
        .github/rulesets/main-required-checks.json)
```

## Rehearse the rollback

A repository administrator disables the ruleset. An agent then confirms that
the rule is disabled and no branch rule is effective on `main`. The
administrator re-enables the ruleset.

```bash
gh api --method PUT "repos/antiburn/antiburn/rulesets/$RS" -f enforcement=disabled  # human
gh api "repos/antiburn/antiburn/rulesets/$RS" --jq .enforcement | grep -qx disabled # agent
gh api repos/antiburn/antiburn/rules/branches/main --jq length | grep -qx 0         # agent
gh api --method PUT "repos/antiburn/antiburn/rulesets/$RS" -f enforcement=active    # human
```

After the re-enable, the agent repeats the binding check and advisory drift
read. Record every output in the applied state.

The following command is an unrehearsed destructive fallback. A repository
administrator can use it if disabling the ruleset does not restore merges.

```bash
gh api --method DELETE "repos/antiburn/antiburn/rulesets/$RS"
```

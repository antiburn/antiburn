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

## Check the gate before the apply

Run this read-only preflight before the human repository administrator applies
the rule. It reads every open pull request at that time.

The preflight records one result for each head:

- `CANNOT_MERGE` means GitHub reports a conflict. Record and skip this pull
  request.
- `GATE_LIVE` means a completed `ci-required` run also produced `slop gate`.
  The conclusion does not affect this result. A red result proves that the gate
  works and does not block the apply.
- `GATE_PENDING` means a check run is still in progress.
- `GATE_ABSENT` means no completed `ci-required` exists and no check run is in
  progress.
- `GATE_STALE` means the completed `ci-required` run did not also produce
  `slop gate`.

Exit 0 permits the apply. Exit 1 names each mergeable head that did not prove a
live gate. Wait for in-progress CI or rerun CI, and then run the full preflight
again. Exit 2 means that the pass did not complete. Do not apply the rule after
exit 1 or exit 2. This procedure does not change another contributor's pull
request.

```bash
set -uo pipefail
die() { echo "PREFLIGHT ABORTED: $*" >&2; exit 2; }
SLUG=antiburn/antiburn
LIMIT=200
command -v jq >/dev/null 2>&1 || die "jq is not on PATH"
PRS=$(gh pr list --repo "$SLUG" --state open --limit "$LIMIT" --json number,mergeable,headRefOid) || die "gh pr list failed"
echo "$PRS" | jq -e 'type=="array"' >/dev/null || die "gh pr list did not return a JSON array"
OPEN=$(echo "$PRS" | jq length)
[ "$OPEN" -lt "$LIMIT" ] || die "gh pr list returned $OPEN heads, the --limit value; the list may be truncated"
[ "$OPEN" -gt 0 ] || echo "no open pull request"
BLOCKED=0
for N in $(echo "$PRS" | jq -r '.[].number'); do
  M=$(echo "$PRS" | jq -r --argjson n "$N" '.[]|select(.number==$n)|.mergeable')
  H=$(echo "$PRS" | jq -r --argjson n "$N" '.[]|select(.number==$n)|.headRefOid')
  S=$(echo "$H" | cut -c1-7)
  case "$M" in
    CONFLICTING) echo "PR $N head=$S result=CANNOT_MERGE"; continue ;;
    MERGEABLE) ;;
    *) die "mergeability of PR $N is $M; rerun after GitHub computes it" ;;
  esac
  RAW=$(gh api "repos/$SLUG/commits/$H/check-runs?per_page=100" --paginate) || die "check-runs read for PR $N failed"
  echo "$RAW" | jq -se 'all(has("total_count") and (.check_runs|type=="array"))' >/dev/null || die "check-runs response for PR $N has an unexpected shape"
  TOTAL=$(echo "$RAW" | jq -s '.[0].total_count')
  GOT=$(echo "$RAW" | jq -s '[.[].check_runs[]]|length')
  [ "$GOT" -eq "$TOTAL" ] || die "check-runs for PR $N returned $GOT of $TOTAL entries"
  CR=$(echo "$RAW" | jq -s '[.[].check_runs[]|select(.app.id==15368)|{name,status,conclusion,run:(((.details_url//"")|[scan("/runs/([0-9]+)/")]|flatten|first)//"0")}]') || die "check-runs projection for PR $N failed"
  REQ=$(echo "$CR" | jq -c '[.[]|select(.name=="ci-required" and .status=="completed")]|max_by(.run|tonumber) // empty')
  if [ -z "$REQ" ]; then
    if [ "$(echo "$CR" | jq '[.[]|select(.status!="completed")]|length')" -gt 0 ]
      then echo "PR $N head=$S result=GATE_PENDING"
      else echo "PR $N head=$S result=GATE_ABSENT"; fi
    BLOCKED=1; continue
  fi
  CC=$(echo "$REQ" | jq -r '.conclusion // "none"'); RUN=$(echo "$REQ" | jq -r '.run')
  SLOP=$(echo "$CR" | jq --arg r "$RUN" '[.[]|select(.name=="slop gate" and .run==$r)]|length')
  if [ "$SLOP" -eq 0 ]; then echo "PR $N head=$S result=GATE_STALE run=$RUN"; BLOCKED=1; continue; fi
  echo "PR $N head=$S result=GATE_LIVE ci-required=$CC run=$RUN"
done
echo "PREFLIGHT_BLOCKED=$BLOCKED"
exit "$BLOCKED"
```

## Apply the rule

Before you apply, confirm that the caller has repository-admin permission:

```bash
gh api repos/antiburn/antiburn --jq .permissions.admin
```

The POST requires repository-admin permission on `antiburn/antiburn`. A 404
from the POST means that the caller is not an administrator. It does not mean
that the payload is wrong.

The human repository administrator owns the mutating call as the Tier 3
authorized action. Run it only after the preflight exits 0. The command creates
the ruleset and prints its ID. Save that ID as `$RS`.

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

The human repository administrator owns the disable and re-enable calls as the
Tier 3 authorized actions. An agent confirms that the rule is disabled and no
branch rule is effective on `main` between those calls.

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

## Applied state

The live preflight completed with exit 0 in SEAMS verification run
`0005-live-preflight-20260821T062921Z-2976`.

| Pull request | Head | Result | `ci-required` | Workflow run |
| --- | --- | --- | --- | --- |
| #114 | `63e7efd` | `GATE_LIVE` | `success` | `32451866394` |
| #112 | `6ecd1fe` | `CANNOT_MERGE` | Not read | Not read |

```text
PR 114 head=63e7efd result=GATE_LIVE ci-required=success run=32451866394
PR 112 head=6ecd1fe result=CANNOT_MERGE
PREFLIGHT_BLOCKED=0
```

- Ruleset ID: `21129949`.
- Binding predicate: `true` in SEAMS verification run
  `0005-post-apply-binding-20260821T063251Z-5d2c`.
- Advisory drift diff: exit 0 with no differences in SEAMS verification run
  `0005-post-apply-drift-20260821T063256Z-3d15`.
- Effective rule on `main`: the API returned the following rule in SEAMS
  verification run `0005-post-apply-effective-20260821T063301Z-762e`.

```json
[
  {
    "type": "required_status_checks",
    "parameters": {
      "strict_required_status_checks_policy": false,
      "do_not_enforce_on_create": false,
      "required_status_checks": [
        {
          "context": "ci-required",
          "integration_id": 15368
        }
      ]
    },
    "ruleset_source_type": "Repository",
    "ruleset_source": "antiburn/antiburn",
    "ruleset_id": 21129949
  }
]
```

- Disabled enforcement: the ruleset read returned `disabled` in SEAMS
  verification run `0005-disabled-enforcement-20260821T064455Z-20e1`.
- Disabled effective rule: the effective-rule list length for `main` returned
  `0` in SEAMS verification run
  `0005-disabled-effective-main-rules-20260821T064500Z-39ce`.
- Re-enabled binding predicate: `true` in SEAMS verification run
  `0005-re-enable-binding-20260821T064804Z-28f0`.
- Re-enabled advisory drift diff: `no differences` in SEAMS verification run
  `0005-re-enable-drift-20260821T064812Z-3942`.
- Re-enabled effective rule on `main`: the API returned the following rule in
  SEAMS verification run
  `0005-re-enable-effective-main-rule-20260821T064823Z-013c`.

```json
[
  {
    "type": "required_status_checks",
    "parameters": {
      "strict_required_status_checks_policy": false,
      "do_not_enforce_on_create": false,
      "required_status_checks": [
        {
          "context": "ci-required",
          "integration_id": 15368
        }
      ]
    },
    "ruleset_source_type": "Repository",
    "ruleset_source": "antiburn/antiburn",
    "ruleset_id": 21129949
  }
]
```

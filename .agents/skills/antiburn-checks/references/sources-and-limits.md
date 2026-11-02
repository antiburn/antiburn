# Sources and supported limits

This is a short authoring reference for `v1-alpha`, not a replacement schema or
specification. Verify the draft label and one matching revision before using it.

## Read one coherent revision

The links below use `main` as an entry point. Confirm that the specification,
authoring guide, schema, and this skill describe the same contract. If they
disagree, ask for the intended release or local checkout. Do not combine files
from different revisions.

For remote reads, resolve a user-approved ref to a commit through
`https://api.github.com/repos/antiburn/antiburn/commits/<ref>` or an available
checkout. Replace `/main/` in every raw URL below with the same commit SHA.
Record that SHA in the handoff. Read-only fetching does not authorize executing
the fetched validator, installing dependencies, or making provider requests.

| Need                                         | Canonical source                                                                                                                                                                                                   |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Precise language contract                    | [Specification](https://raw.githubusercontent.com/antiburn/antiburn/main/docs/burn-check/specification.md)                                                                                                         |
| Executable subset and workflow               | [Authoring guide](https://raw.githubusercontent.com/antiburn/antiburn/main/docs/burn-check/authoring.md)                                                                                                           |
| Structural schema                            | [Draft 2020-12 schema](https://raw.githubusercontent.com/antiburn/antiburn/main/docs/burn-check/schemas/burn-check.schema.json)                                                                                    |
| Example overview and setup                   | [Burn Checks guide](https://raw.githubusercontent.com/antiburn/antiburn/main/docs/burn-check/README.md)                                                                                                            |
| Native events, source formats, producer pins | [Session coverage](https://raw.githubusercontent.com/antiburn/antiburn/main/docs/session-coverage.md)                                                                                                              |
| Reachable findings and result limits         | [Check coverage](https://raw.githubusercontent.com/antiburn/antiburn/main/docs/check-coverage.md)                                                                                                                  |
| Connection output and route limits           | [Provider contracts](https://raw.githubusercontent.com/antiburn/antiburn/main/docs/burn-check/provider-contracts.md)                                                                                               |
| YAML and schema validator                    | [Validator script](https://raw.githubusercontent.com/antiburn/antiburn/main/docs/burn-check/scripts/validate-checks.mjs)                                                                                           |
| Validator dependencies                       | [Package manifest](https://raw.githubusercontent.com/antiburn/antiburn/main/docs/burn-check/package.json), [lock file](https://raw.githubusercontent.com/antiburn/antiburn/main/docs/burn-check/package-lock.json) |

In a local Antiburn checkout, these paths start at its root. They are not
relative to the installed skill directory. Resolve relative links inside fetched
documents against that document's full same-revision URL.

## Choose a maintained example

Read and adapt the canonical file at the verified revision. Do not create a
second maintained copy of a repository example inside this skill.

| Authoring problem                  | Canonical definition                                                                                                       | Keep this limit                                                                         |
| ---------------------------------- | -------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| Unrelated read/search work         | [Over-exploring](https://raw.githubusercontent.com/antiburn/antiburn/main/.antiburn/checks/over-exploring.yml)             | Plausible dependencies, tests, and verification remain allowed.                         |
| Work on a separate objective       | [Scope creep](https://raw.githubusercontent.com/antiburn/antiburn/main/.antiburn/checks/scope-creep.yml)                   | Keep user approval separate from tool claims and prove completed work.                  |
| Current rules or prior corrections | [Ignored instructions](https://raw.githubusercontent.com/antiburn/antiburn/main/.antiburn/checks/ignored-instructions.yml) | Current-file mismatch does not prove a past violation; uncertainty can preserve status. |
| Available skill guidance           | [Skill opportunities](https://raw.githubusercontent.com/antiburn/antiburn/main/.antiburn/checks/skill-opportunities.yml)   | Inventory descriptions do not prove past loading or non-use.                            |
| Standard-mode trial advice         | [Fast mode value](https://raw.githubusercontent.com/antiburn/antiburn/main/.antiburn/checks/fast-mode-value.yml)           | Recorded mode hints do not prove served tier, timing, or savings.                       |

## Small supported subset

- One trigger family per definition. Only messages support a rolling window;
  size is at most 40 and minimum is at most size.
- The desktop emits `session_start` reason `attached` only. `session_idle`
  records a persisted 120-second observation boundary, not real idle time or
  model occupancy. Remote, WSL, child, and historical-only sources do not start
  new live checks.
- Omit `decisions` for local status rules. Otherwise, use independent named
  decision blocks with one to eight questions each. Prepare every block before
  inference; actions wait for every valid answer.
- Choice criteria are an ordered label/description mapping with at least two
  entries. The output is one declared label. Optional details are a raw label
  probability map and separate provider confidence.
- Score criteria are an ordered description array with at least two entries.
  The output is a finite number from zero through the last criterion index,
  including fractions. Optional probabilities use an array in criterion order;
  provider confidence is separate.
- Noul has a question without authored criteria. Its output is a finite
  yes-probability in `[0, 1]`, not a Boolean. Its details are an empty record;
  there is no separate confidence or probability distribution.
- Use only status actions in the supported runtime. Message actions remain
  unsupported even behind a false condition. Schema-valid broader context such
  as effective `agent.skills` or `repo.branch` does not establish runtime support.
- Use JSON-compatible YAML 1.2 with one mapping document and string keys.
  Reject duplicate keys, custom tags, anchors, aliases, merge keys, multiple
  documents, and non-finite numbers. Do not use a YAML 1.1 parser that changes
  `on` into a Boolean.
- Use only documented expression roots, operators, and pure functions. Missing
  evidence stays unavailable through transforms; `||` is not a missing-data
  fallback. Operations on `session.requests` membership need complete history;
  copying an available partial history whole can retain its explicit limits.

These limits describe supported forms, not complete source or provider support.
Use the matching authoring guide and compiler to check runtime support. Use the
coverage documents for source limits and the provider contracts for connection
limits.

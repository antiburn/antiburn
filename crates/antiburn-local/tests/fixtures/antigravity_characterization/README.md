# Antigravity characterization

All fixtures use synthetic text and paths. They are not captured user sessions.

## Accepted records and pins

`scope_records.jsonl` uses the brain record shapes demonstrated by
[`nizos/probity` at `f750c1d82d2bcc842d4bfe7f00401758a3584b56`](https://github.com/nizos/probity/blob/f750c1d82d2bcc842d4bfe7f00401758a3584b56/test/fixtures/antigravity-cli/real-session.jsonl):
`USER_INPUT.content` with `source: "USER_EXPLICIT"`,
`PLANNER_RESPONSE.content`, and `tool_calls[].name/args`.
The public records also show `write_to_file` input with `CodeContent`,
`TargetFile`, and `ArtifactMetadata.RequestFeedback/Summary/UserFacing`.
This is a record-shape pin, not an official producer or a release range.
The synthetic Proceed and rejection sentences test ordinary user text. They
do not establish dedicated review-event persistence.

The existing nested `userInput.userResponse` and `userInput.items[].text`
acceptance is tested separately. Their use in a brain record has no reviewed
public producer pin here. Coexisting fields retain all text in response,
scalar-content, then item order. This field order does not prove separate event
times. Nested text has unknown authority even when the step has
`source: "USER_EXPLICIT"`. No typed user answer or plan approval comes from
these compatibility fields. The brain pin does not establish cascade authority.
Cascade user text also has unknown authority.

Only scalar content on an exact brain `type: "USER_INPUT"` record with exact
`source: "USER_EXPLICIT"` has human authority. A conflicting `SYSTEM` or `MODEL`
source, an unknown source, an absent source, or a non-string source leaves the
scalar text's authority unknown. Text and user-role metrics remain available.
Unattributed or empty user records mark `AttributionIncomplete`; this is a
coverage gap, not proof of rejection. Required user history is unavailable when
no proven human content exists. Preserved unknown text cannot satisfy that
requirement. Proven records do not establish complete history when other user
records have unknown authority.

Optional call `id` and result `tool_call_id` preservation is a
compatibility test, not proof that the pinned records emit IDs. The public
brain calls have no IDs. Do not invent positional IDs or result joins.

SQLite is pinned to `agy 1.0.16`, `user_version = 1`, and
[`ccusage` at `90e296efd1bdd25a9db07019854255284588d720`](https://github.com/ccusage/ccusage/blob/90e296efd1bdd25a9db07019854255284588d720/rust/adapters/antigravity/src/proto.rs).
That source supports usage, models, timestamps, retries, and response identity.
It does not establish a review, approval, comment, or artifact-version schema.

## Optional evidence limits

The [implementation-plan documentation](https://antigravity.google/docs/implementation-plan/)
describes Proceed and review comments. The
[artifact-review documentation](https://antigravity.google/docs/artifact-review/)
describes Always Proceed. These are workflow descriptions, not persistence
contracts. Notification success, policy settings, task completion, and task
lists do not establish user approval. Arbitrary tool output stays tool evidence.

Recorded proposals and write inputs remain assistant evidence. Proven scalar
user replies remain ordered user evidence. Other retained user-shaped text
keeps unknown authority. No typed review answer or approved plan is
created without a proven producer shape and version binding.

The only read companion is
`conversations/<owner>.db` ->
`brain/<owner>/.system_generated/logs/transcript.jsonl` within the same root.
The input owner must equal the database stem and must be a single safe path
component. Brain artifacts and `transcript_full.jsonl` are not sessions.
Plan files and metadata are not loaded. A missing transcript supplies no user
history. An unrelated owner's transcript supplies no history for this session.
The combined source fingerprint invalidates a claimed read when the transcript
changes, appears, or disappears before the read.

Current mutable `implementation_plan.md` bytes cannot prove an earlier reviewed
version. No accepted public record here binds comment/Proceed to an immutable
plan digest or revision. Missing or stale plan sidecars therefore supply no
typed approval or historical plan text. Companion retrieval needs a separate
bounded, exact-owner, source-version contract before support can expand.

# Smart Burn Checks

Smart Burn Checks are like deterministic Burn Checks: both look for signs of
known issues in coding sessions. Smart Burn Checks send selected parts
of a session to the configured provider for an assessment. Antiburn combines those assessments
with its local rules. The result describes evidence of an issue; it does not
judge a person's intent.

Smart Burn Checks means all provider-powered Burn Checks. Each check declares its
own evidence selection, questions, and result policy. All four use the shared
framework; Ignored Instructions does not define the other checks' selection.

The four checks are Ignored Instructions, Scope Creep, Over-exploring, and Skill
Opportunities. Jev uses a TypeSafe API key. Ollama, Cloudflare, and Custom
connections use the same check flow. Hosted requests can incur charges; local
usage estimates are not a spending cap or provider invoice.

## Setup, controls, and local data

Open **Settings → Checks**, enable Smart Burn Checks, and configure a connection.
Jev is the default. **Use Ollama or another provider** opens Ollama, Cloudflare,
and Custom setup. Saved connections retain their settings when switched. Model
capabilities govern preparation, packing, transport, validation, and reduction.
A connection test proves protocol acceptance, not check accuracy. Ollama can use
a local or remote base URL serving `/v1/systemone`; an ordinary chat endpoint
does not establish this protocol. Cloudflare accepts Clef or Clef Flash with an
account ID and credential. Custom uses the exact endpoint with direct System One
or Cloudflare-envelope response decoding.
The setting controls automatic checks. You can choose future sessions or start a
7- or 30-day history review there. The history window applies to the requested
review; it does not prove which instructions were active during older actions.

Pause checks to stop new assessments while keeping saved connections. Credentials
use native credential storage or memory, not the session database. Removing a
required credential prevents new requests through that connection. Remove a
session to delete its local check state; already-incurred usage remains in the
local totals. **Clear Local
Data** also removes local sessions, check state, unresolved requests, and usage
totals. See the [privacy policy](privacy-policy.md) for retention and data
handling details.

## Registered checks

The shared worker runs all four checks. Scope Creep, Over-exploring, and Skill
Opportunities accept native Claude Code, Codex, OpenCode SQLite v2, and Pi
sources under the pinned retained-root contracts in
[session coverage](session-coverage.md#retained-root-smart-check-inputs).
They use the same master setting, provider connection, history controls, and
Checks views. Four descriptors use
one worker, including Ignored Instructions.

Scope Creep compares substantial optional performed work with the full latest
recorded scope. It includes recorded user messages, supported answers and plan
evidence, and required proposal context. Every initial and follow-up request
includes the complete compact scope. Later recorded approval withdraws stale
findings and prompt actions. Task context that exceeds the model limit remains
unassessed; the check does not split, summarize, or silently truncate approvals.

Over-exploring reviews completed investigation episodes against recorded task
context. Its three reasons are unrelated files, excessive file breadth, and
excessive within-file reading. Counts and requested ranges do not prove waste.
The provider receives selected user task context, read requests and recorded
results, and supporting activity. Missing history, unfinished episodes, and
unsupported read results remain unassessed.
A finished read alone does not complete an episode. Current episodes need later
completed edit activity; a next authorizing user boundary needs resolved,
unambiguous tool operations. A pending request can resolve through a unique later
completed native result. An explicitly running request remains deferred even if
a result says completed. Unresolved, failed, missing, orphaned, and ambiguous
operations remain deferred. Observed lines do not prove whole-file access or
file versions.

Skill Opportunities compares recorded work with current installed skill
descriptions and typed selected skill-use evidence. It sends selected task/work
context, matching skill names, descriptions, semantic frontmatter, and use/time
limits to the configured provider. OpenCode result proof needs selected
`OtherToolOutput`; input-only projections cannot carry result text. Codex and Pi
document selections and Claude requests/failures remain distinct facts, not
execution proof.
It does not prove past visibility or whole-session absence. Reliable birth time
only excludes skills created after the relevant work; missing time stays explicit.

All four checks preserve exact source citations and exclude private thinking.
Prompts suggest better instructions for future work. They do not repair the
reviewed session, offer Auto Fix, create verification watches, or estimate savings.
Changed source evidence, provider configuration, or skill inputs invalidate stale work.
History progress counts check-session jobs, not distinct sessions.

## Ignored Instructions

The following product flow describes Ignored Instructions. Its instruction
rules, selected fields, analysis passes, and confidence gates are specific
to this check.

### What the check reads

The session reader turns each supported agent's records into the same event
format. This lets the check handle different agents in the same way.

The check selects:

- Instruction text from supported files in their current state. This is a
  current-file comparison, not proof of the text or activation the agent saw
  during the session. An explicit 7- or 30-day history run compares older
  actions with these current files as a best-effort review. Historical text
  is available only from a supported, authoritative session record; a read
  path alone does not provide it.
- Assistant text that describes work.
- Bash command input, which can include inline scripts, heredocs, and patches
  recorded inside the command.
- File-edit paths, read-file paths, and search queries with their scope filters.
- Inputs for other tools that are not recognized as Bash, edit, read, or search
  tools.

- Selected recorded user text and Bash results for bounded context. Human text
  requires a normalized source-bound history proof and exact native text range.
  Bash output requires a unique earlier request in the same source, thread, and
  scope, exact call/name/range/digest bindings, and completed or error status.
  The earlier request must be present in the selected evidence.

The check excludes search results, read output, edit content, other tool output,
typed question/plan fields, and private thinking. Dedicated edit-tool content
stays excluded even though inline Bash content is selected. Selected file paths
can appear in provider requests. Event IDs, line numbers, and citation links
stay on the computer. The provider gets temporary labels; Antiburn uses a private map to
connect its answers to the original session events. Missing historical evidence
stays unavailable and cannot prove historical activation, even if a selected
review shows Clean. A recorded command request
does not establish that the command ran or succeeded.

The selected query excludes thinking and unqualified non-authoritative text before
loading rows. It selects normalized tool fields by the check's field mask and
applies byte limits to selected values. It can still read metadata for excluded
tool categories. Source-wide truncation flags can keep a result unassessed when
they cannot identify the omitted field.

### How instructions are split

The Markdown reader uses headings and main list items to find rules. It keeps a
rule's nested bullets, conditions, exceptions, and examples with that rule. It
also saves the YAML header with the instruction file. A changed current file
produces a different comparison snapshot; it does not change which instructions
governed older actions.

Background and example sections do not become rule targets. The check does not
classify a rule by searching for words such as “must” or “never.” Jev judges the
meaning from the supplied text.

Long text cannot fit in one request. The check cuts long rules, assistant
messages, and tool inputs into small, overlapping text ranges. Each range
repeats a little text from the range before it. This keeps words near a cut
connected. Sampling can leave ranges and range combinations unchecked.

Instruction source also matters. The worker discovers supported instruction
files from the session's worktree and home. It records an observed version and
the session source positions at that observation. A new or changed version
applies to later actions, not to actions already recorded. The first observation
cannot establish when that version became active. A matching path or read
request does not prove historical content or activation. A history review can
compare past actions with current rules, but does not claim those rules were
active then or mark that comparison as historical proof.
The rule reference retains its source, digest, scope, conditions, and local
file location so a finding points to the instruction it used.

### How sessions are split

The check starts at the saved enablement boundary, or at the selected history
window for a history run. It includes supported content after that boundary.
Assistant text and tool input can be actions to check. A candidate action means
one such event that may match an instruction. Accepted user text and bound Bash
results supply context, not candidate work. Completion/error labels do not prove
tests passed; result text must support the exact obligation. Unknown-origin,
synthetic, skill-document, unmatched, conflicting, and truncated context cannot
restore missing human authority or result proof.

Antiburn reads a large session in pages. Each page holds a fixed amount of
content. The first page can have newer events than the second page. The reader
keeps the original order of events and keeps separate conversation branches
apart.

### How sampling works

One rule-text range and one action-text range form a possible comparison. The
default sample is **256 high-priority rule/action pairs per review**. This is
not exhaustive coverage or a cap on possible comparisons, TypeSafe requests,
tokens, elapsed time, or cost. A large session can have many more possible
pairs. The check records possible, sampled, and remaining pair counts. Remaining
pairs are a sampling gap, not known violations.

The local selector scores meaningful word overlap; words found in fewer actions
carry more weight. A tool name or literal action path mentioned by a rule adds
a stronger signal. Prohibitions paired with tool input and recent actions add
smaller signals. These signals rank work; they do not prove that any other pair
is irrelevant. The selector spreads early choices across rules and instruction
sources and includes low-overlap probes when space permits. It fills the rest
of the budget in stable score order. Diversity and exploration reduce, but do
not remove, the chance of missing a conflict. Jev still decides applicability
using the rule and recorded context. Keyword matches alone never publish findings.

On subsequent reviews, new activity receives attention first, then older
eligible pairs not yet sampled. The worker saves sampled pair identities and
compatible typed answers across completed reviews, appends, and restarts.
Unchanged sampled pairs do not consume the next pass because content pages or
request packing changed. With no new work, the remaining gap decreases over
successive reviews; new actions or rules can increase it. Reuse requires the
same identifiable instruction rule and action, selected text, relevant context,
instruction version, model, and check revisions. A matching path or similar
wording alone is insufficient. When a request outcome is unknown, recovery can
make up to three total dispatch attempts. An earlier dispatched attempt may
already have incurred a charge. If the result is still unknown after those
attempts, Antiburn blocks further dispatch of that work.

An **event window** contains one action and up to two sampled rules. This sends
the action once for those rules instead of copying it into a separate window
each time. One rule/action comparison is a **target**. Long actions or rules
can need several windows with overlapping text. Sampling does not promise
every possible range or window is sent.

For example, the instructions might say “Run tests before publishing” and “Get
approval before publishing.” If the session says “Published the release,” the
window has one action and two targets. Jev makes a separate decision for each
target.

An older page may contain a selected earlier step for a newer action. The worker
keeps only comparisons whose prior history is incomplete, up to the candidate
limit. It adds matching earlier events in source order and checks each candidate
again. Completed comparisons do not carry forward. The carried comparison state
is separate from the content cursor and page result. Missing history never proves
that a prerequisite did not happen. Accepted human text can inform a bounded
permission-dependent comparison. Unknown-origin answers, assistant approval
reports, skill selections, and tool permission do not establish human approval.
If the needed interval remains
missing or cut short, that comparison stays unassessed.

### What Jev receives and returns

Each event window contains the selected rule text, the action, and a few
nearby events. The check uses two analysis passes. The first pass asks one
applicability question for each target:

1. Does this instruction apply to the action?

The check skips follow-up questions for a confident “not applicable” answer
(at least 0.90 probability). For other valid applicability answers, it asks
relationship and evidence questions, plus a completion question when relevant:

1. Does the action conflict with or follow the instruction after conditions and
   exceptions are considered?
2. Could missing or truncated evidence change that conclusion?
3. Does the instruction have a completion-bound obligation whose completion
   boundary is not shown?

Weak or uncertain answers do not prove a clean comparison. Source completeness
and unresolved obligations limit conclusions about the reviewed comparisons.

The shared request code combines windows until the request reaches its size or
question limit. Jev returns a choice and probability for each answer. The
temporary labels let the shared worker connect answers to their windows, while
the private map connects those windows back to the original rule and event.
The follow-up also checks excluded-field limits and keeps context
violations separate from the candidate's local citation. The first pass evaluates
chosen targets, not every possible rule/action pair.

### How results become findings

Jev does not decide the session result by itself. Antiburn checks the answers
available for each selected target, then combines those decisions into one
review result.

The local decision rules apply confidence limits. A possible finding needs at
least 0.85 probability that the rule applies and the action conflicts. A likely
finding needs 0.90 plus stronger evidence about the instruction source and the
action.
When evidence is incomplete, Jev must also judge that the supplied evidence is
enough to prove the result. A direct conflict can still qualify when omitted
history cannot change it. A conclusion that depends on missing history stays
unassessed.

Antiburn links each finding to the exact local rule and action. It combines
duplicate results for the same rule and action into one finding. A typed
completion answer can keep a rule pending until the session shows the task's
completion point. A completed sampled review with no findings can show the
ordinary **Clean** label. Clean means **no finding among sampled comparisons**.
It does not mean that all session content is safe or that every instruction
was active at the time. Unsampled pairs remain a coverage gap. Missing
prerequisites, unavailable approval evidence, incomplete source evidence, pending
obligations, and unresolved sampled comparisons cannot prove those comparisons
clean. A provider or response error means the review did not finish; an
evidence error means required source material is unavailable. Neither is a
clean sampled review.

The shared worker saves progress after each request and stores compatible
typed answers separately from current findings. It rebuilds findings against
the current source publication before reusing answers. A changed instruction
file governs future actions; old actions do not get reassessed against the new
file merely because it changed. The worker also handles the TypeSafe connection,
cached responses, usage reservations, retries, response checks, and cancellation.
Check text and answers do not go to product analytics.

## Reusable Jev check contract

The shared engine contract is `JevCheck` in
`crates/antiburn-local/src/analysis/jev.rs`. A check implements it to define:

- Its stable check ID and input, chunking, question, and reducer revisions.
- Its typed session-field selection. The default selection is empty.
- How to project normalized facts into bounded `JevInputWindow` values.
- Which local evidence IDs are candidates, instructions, or supporting context.
- Its typed questions and deterministic reducer.
- Its typed prepared state, without a JSON round trip through the runner.
- Any check-owned reference snapshots, separate from transcript evidence.

`run_jev_check` packs those windows, validates Jev's answers, maps each answer
back to its local window, saves generic progress, resumes unfinished requests,
and calls the check's reducer. Its mechanics do not depend on Ignored
Instructions.

The desktop `JevCheckWorker` trait connects a check to product work. It supplies
the check ID and processes one stored session candidate. The shared
`jev_worker` scheduler finds candidates, and `execute_jev_batch` handles
transport, cache, reservations, retries, cancellation, and safe error reporting.
The scheduler does not inspect instruction rules or define findings.

Ignored Instructions implements `JevCheck` in
`crates/antiburn-local/src/checks/ignored_instructions/assessment.rs`. Its
desktop adapter is `apps/desktop/src-tauri/src/ignored_instructions_worker.rs`.
That adapter owns instruction discovery, source paging, its saved cursor, and
how its page results combine.
The check's instruction discovery, planning, questions, matching, windows, and
reduction live beside its assessment under `checks/ignored_instructions/`.
`analysis::ignored_instructions` remains a compatibility export.

### Inputs and evidence

Each check declares a `JevInputSelection` and matching
`JevEvidenceRequirements`. The default selection is empty. The source boundary
applies selection before a check receives evidence. Its page query excludes
thinking and unqualified non-authoritative text, selects normalized tool fields by
mask, and retains row metadata. The check receives only permitted text fields.

`JevFieldAvailability` reports `excluded`, `unsupported`, `not_observed`, or
`observed` for each field, plus source capability and observed, empty, malformed,
and truncated part counts. A conditional field can be supported by the source
contract but absent from one page. Unsupported source formats remain unavailable.
Malformed known tools do not become other-tool input. Unsupported, malformed,
truncated, or capped selected evidence blocks a clean result. An unobserved
conditional field stays distinct from an unsupported field.

`JevEvidenceStore` indexes selected values by local source ID and field. It caps
the page at 288 source IDs and 1.125 MiB of selected text. The store is tied to the
published content fence and is rebuilt for each page or continuation. IDs stay
local. A check can request an allowed value through `retrieve_evidence`; the
method rejects fields outside the check's declared selection. It cannot read an
unselected field or another publication.

`JevInputWindow` contains only the bounded state sent to Jev. Its evidence
references bind request-local context to local source IDs. Packing sends the
context and questions, not those IDs. Unpacking restores the local bindings and
the runner verifies them before reduction.

### Private question and plan records

`ContentPart::with_scope_evidence` accepts adapter-normalized `JevUserAnswer`
and `JevPlanReference` records. The types live in
`analysis/jev_evidence/scope_metadata.rs`. They retain source format, native
role and record identity, call/question IDs, record-local order, optional producer
acceptance order, provenance,
producer and normalization revisions, source ranges, and truncation. Content
rows supply the session, branch, turn order, and publication binding.

Answer lifecycle and origin are separate. Submitted, skipped, cancelled, timed-out,
pending, and unknown statuses retain selections and free text. User, synthetic,
automatic, and unknown origins remain distinct. Each question retains its full
prompt, optional header, context and comment, ordered options, descriptions, and
multi-select metadata. Pi questionnaire display labels use `header`. Native ranges
carry their own optional record IDs for cross-record call/result bindings. The adapter
must validate the native producer shape before it attaches these records.
A tool name or result string alone cannot create a typed user answer.

Plan records retain proposed/approved/rejected/feedback status, recorded text,
feedback, path, identity, revision, and current and approved content digests.
Recorded, version-matched, mutable, missing, and unresolved content remain
distinct. A plan status is source evidence, not a check's approval decision.
Current mutable bytes do not prove the version that received approval.

These records stay in private `JevOperationMetadata`. Engine row insertion stores
them under `metadata` in `turn_content.normalized_fields_json`, including parts
without normalized tool input. The selected query reads only the arrays requested
by `JevInputField::UserAnswer` and `JevInputField::PlanReference`. Each field is
retrievable as an exact JSON array through `JevCheck::retrieve_evidence`.
Selected records and their revisions contribute to the selected-input digest.
Queries retain the existing session/publication fences, paging, and byte limits.
Oversized metadata is rejected or reported as omitted; it is not summarized.
Thinking cannot supply these records.

Both fields require explicit opt-in. `JevInputSelection::ALL` retains its legacy
text-only mask. Ignored Instructions excludes these typed scope fields; changes
to excluded fields do not change its selected digest at the same publication.
The fields are conditional for `OpenCodeSqliteV2`, `ClaudeJsonl`,
`CodexRolloutJsonl`, and `PiV3Jsonl` under the fixture-backed contracts in
`session-coverage.md`. Other sources remain
unavailable. Claude retains structured answer strings with unknown human origin
and exact recorded plan versions without approved-version bindings. SDK callback
input, plan-mode exit, permission changes, and approval wording do not establish
Claude scope approval. Missing and mutable plan content stays unresolved.
The pinned local OpenCode plan completion retains build-switch approval, but
not the approved plan path, content, or version. Provider-executed results and
synthetic attachment text do not establish approval. Companion-plan retrieval
remains unavailable.

### Reference sources

Reference data is separate from transcript evidence. A worker or check-owned
source adapter loads the policy data it can prove, then supplies a typed
`JevReferenceSnapshot` with a kind, identity, revision, and fields. For Ignored
Instructions this source is the instruction snapshot. Current-file comparisons
remain labeled as current-file evidence; they do not become historical proof.

The check lists required reference kinds in `JevEvidenceRequirements`. The
runner rejects a context that lacks a required snapshot. Reference identity and
revision are included in the check input revision and progress identity.

### Prepared state and revisions

`JevCheckPlan<Prepared>` carries the check's typed preparation and reducer
state. A check defines `Prepared`; the generic runner does not serialize it to
JSON or interpret its fields. The desktop adapter can still persist generic
answers, source bindings, and progress. It rebuilds the prepared state from the
immutable input when it resumes.

The four `JevCheckRevisions` cover projection, chunking, questions, and
reduction. Progress identity also includes the input selection and required
reference kinds. Increase the relevant revision when these semantics change.
The source input revision binds the selected evidence, source generation, and
publication fence. A changed input or revision invalidates prior answers.

### Check and desktop boundaries

Checks own projection, windows, questions, follow-up policy, deterministic
reduction, and clean-result eligibility. Checks do not decode native source
formats, call TypeSafe, reserve usage, or access the desktop Store.

`JevCheckWorker` is a thin desktop adapter. It enrolls and pages product
candidates, obtains source-fenced check input and reference snapshots, restores
check progress, calls `run_jev_check`, and publishes results. A new check reuses
the shared scheduler, executor, cache, and usage reservation.

### Test-only second check

`OutputEnabledCheck` in the `analysis::jev` tests selects Bash output and
retrieves it by its local evidence binding. It runs through the same packing,
response validation, progress, resume, and reduction code as Ignored
Instructions. This test proves the shared preparation contract does not force
checks to exclude output fields. Production output support still depends on the
source field matrix and the check's product enrollment.

### Adding another Jev-powered check

1. Define a stable check ID, field selection, evidence/reference requirements,
   and four revisions. Increase a revision when its
   projection, chunking, questions, or reducer changes.
2. Implement `JevCheck`. Select only the normalized facts the check needs,
   group them into bounded windows, bind local evidence IDs, write typed
   questions, and reduce the answers deterministically.
3. Implement `JevCheckWorker` for candidate selection, source preparation,
   check-specific paging, resume state, and result combination. Load only
   supported source evidence and check-owned references. Preserve availability
   and source-fence limits.
4. Register the worker and its check ID with Settings and candidate enrollment.
   Reuse the shared scheduler and `execute_jev_batch`; do not add another HTTP,
   cache, or usage-reservation path.
5. Add offline tests for field selection, excluded-field isolation, allowed
   follow-up retrieval, window boundaries, request bounds, local citations,
   reference and revision invalidation, partial progress and history, and
   clean-result eligibility.
   Keep billable live tests bounded and based on synthetic data.

The main rule is simple: the check owns what its evidence means. Shared Jev code
owns how requests run and how progress resumes.

## Evaluation scope

The Rust diagnostics are under `apps/desktop/src-tauri/eval/`. Each check target
has one ignored `live` entry point. Shared support owns environment-only provider
selection, suites, exact case IDs, a positive case limit, usage/latency, and
timestamped JSON capture. Use `ANTIBURN_EVAL_PROVIDER`, `ANTIBURN_EVAL_SUITE`,
`ANTIBURN_EVAL_CASES`, `ANTIBURN_EVAL_LIMIT`, and optionally
`ANTIBURN_EVAL_OUTPUT_DIR` or `ANTIBURN_EVAL_RAW`.

```sh
ANTIBURN_EVAL_PROVIDER=ollama-nimble ANTIBURN_EVAL_LIMIT=3 cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test over_exploring live -- --ignored --exact --nocapture --test-threads=1
```

Ignored Instructions maps `development` to `core`; it also accepts `context`,
`limits`, and `all`, with `controls` as an alias for `limits`. The other checks
accept `development` and `controls`. Reports retain weak scores, errors,
abstentions, missing outcomes, citation diagnostics, usage, and latency.
Synthetic-case accuracy is diagnostic, not a population estimate or delivery
gate. No frozen hashes, binding sidecars, previous passing capture, strict
accuracy threshold, or Python tool is required. Production authority and protocol
validators still apply. Live calls are explicit and can incur charges. Do not
run every ignored test. See the [eval guide](../apps/desktop/src-tauri/eval/README.md)
and [provider presets](../apps/desktop/src-tauri/eval/support/README.md).

Bounded diagnostics on 2026-10-07 completed selected cases for all four checks
with Jev and representative Ignored Instructions cases through Ollama,
Cloudflare, and Custom direct/envelope routes. Some expected findings or clean
results abstained. Nimble retained an incomplete Ignored Instructions execution
and a Skill Opportunities advisory false positive. These results do not
establish broad model quality, native parser reachability, or every provider/check
combination. Retain weak scores and failures; rerun affected cases when request
preparation, transport, or reduction changes.

An ordinary assessment in about 60 seconds after worker start is a performance
goal, not a deadline or guarantee. Preparation, provider admission, request
packing, model latency, follow-up questions, checkpoints, and publication all
add time. A pass can use several paid requests; the 256-pair budget does not
cap spending. Large inputs, rate limits, retries, and missing history can take
longer. Measure worker-start-to-result time, sampled coverage, requests and
tokens, reuse, missed findings, and cost on representative sessions before
making a performance or quality claim. Offline synthetic tests cannot establish
live model accuracy or elapsed provider time.

TypeSafe's [API reference](https://docs.typesafe.ai/api) describes typed
questions and answers. Its [model page](https://docs.typesafe.ai/models)
documents token and request limits and notes that limits can change. Its
[re-ranking example](https://docs.typesafe.ai/cookbooks/rerank_typesafe)
shortlists before semantic scoring; a shortlist can miss the correct result.
Its [large-state guidance](https://docs.typesafe.ai/model-jaggedness/jev-1.13)
describes model limits as state grows. These sources motivate bounded requests
and a measured shortlist. They do not validate Ignored Instructions accuracy.

## Ignored Instructions selected-input coverage

Audit date: 2026-10-07.

This matrix records the twelve normalized session fields and their current
field capability and selection for Ignored Instructions. User text and Bash
output are selected, but accepted context needs the exact normalized proofs
above. Field capability alone does not prove context eligibility.
Other Smart Burn Checks can select a
different subset. It complements the exact native-source inventory in
[`session-coverage.md`](session-coverage.md) and the reachable finding contract
in [`check-coverage.md`](check-coverage.md).

`S` means the accepted adapter can supply the field. `C` means the field is
conditional on the native record. `U` means this check does not accept the
source format. `N` means the field is excluded by the current check selection;
it does not mean the source format cannot contain it. Page availability is
reported separately as excluded, unsupported, not observed, or observed, with
observed, empty, malformed, and truncated counts.

The accepted formats below have native fixture-to-store-to-selected-query
coverage. Claims remain limited to the characterized source shapes and producer
pins in the session coverage document. All other formats are unavailable to
this check, even if another local parser reads them. Cursor store and composer
routes retain user and assistant text but mark native tool fields unavailable,
so they cannot produce a clean result from incomplete tool evidence.

| `SourceFormat`                 | UserMessage | AssistantMessage | BashCommandInput | BashCommandOutput | FileEditPath | FileEditContent | ReadFilePath | ReadFileOutput | SearchFilesQuery | SearchFilesOutput | OtherToolInput | OtherToolOutput |
| ------------------------------ | ----------- | ---------------- | ---------------- | ----------------- | ------------ | --------------- | ------------ | -------------- | ---------------- | ----------------- | -------------- | --------------- |
| `ClaudeJsonl` | S | S | C | C | S | N | S | N | C | N | C | N |
| `CodexRolloutJsonl` | S | S | C | C | S | N | S | N | C | N | C | N |
| `PiV3Jsonl` | S | S | C | C | S | N | S | N | C | N | C | N |
| `OpenCodeSqliteV2` | S | S | C | C | S | N | S | N | C | N | C | N |
| `CursorCliAgentJsonl` | S | S | C | C | S | N | S | N | C | N | C | N |
| `CursorCliStoreDb` | S | S | U | U | U | U | U | U | U | U | U | U |
| `CursorChatStoreDb` | S | S | U | U | U | U | U | U | U | U | U | U |
| `CursorIdeComposer` | S | S | U | U | U | U | U | U | U | U | U | U |
| `AntigravityBrainJsonl` | S | S | C | C | S | N | S | N | C | N | C | N |
| `AntigravitySqlite` | S | S | C | C | S | N | S | N | C | N | C | N |
| `OpenCodeJsonl`                | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `OmpV3Jsonl`                   | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `CursorJsonl`                  | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `CursorLegacyChatJson`         | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `AntigravityJson`              | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `AntigravityCascadeJson`       | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `AntigravityWorkspaceChatJson` | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `CopilotCliJsonl`              | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `CopilotIdeChatJson`           | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `ClineSessionJson`             | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `ClineMessagesContractV1`      | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `KiroSessionJson`              | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `KiroChat`                     | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `KiroCliV2Bundle`              | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `KiroCliV3Bundle`              | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `KiroChatSaveExport`           | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `AmpThreadJson`                | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `AmpFileChanges`               | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `WindsurfWorkspaceJson`        | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `WindsurfMirrorJson`           | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `WindsurfCascadeProtobuf`      | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `DevinLocalSqlite`             | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |
| `Uncharacterized`              | U           | U                | U                | U                 | U            | U               | U            | U              | U                | U                 | U              | U               |

The query excludes thinking and unqualified non-authoritative text before loading
rows. It applies part and byte limits to selected fields. SQLite extracts only
selected normalized tool fields but can read excluded row metadata. Each tool
request is normalized once when its source adapter writes the fenced content
row. Raw request content remains local; selected
paths, commands, queries, and other eligible tool input form the assessment
input. Dedicated edit-tool bodies are excluded from Ignored Instructions, while
inline Bash scripts and patches can be selected as command input.
Schema migration 63 adds normalized fields. Parser revision 46 refreshes older
rows for operation metadata, native field bindings, recorded path context, and
truncated-request isolation and exact result joins. Metadata uses the existing
private normalized JSON column.

## Shared normalization contract

Native adapters validate source shapes before shared normalization. Checks
consume generic source-bound facts instead of parsing producer markup. Normalized
skill facts retain request, document-selection, failure, and unknown status,
selected ranges, digests, native identity, and available chronology. A selected
document is not execution proof. Normalized human text and command results retain
exact action/request references, scope, digests, ranges, and lifecycle. A copied
pointer, tool name, result phrase, or current filesystem value cannot supply
missing provenance. Read facts preserve observed extent and unavailable file
versions. Selection removes excluded facts and bindings before check preparation.

Normalization is a reusable source-boundary abstraction. Each Smart Burn Check
selects from its normalized fields; Ignored Instructions does not define which
fields another check can select. `analysis::jev_evidence` owns normalization,
projection, availability, and local citation bindings. The existing Ignored
Instructions evidence exports remain available through that shared module.

The shared normalizer accepts object arguments, JSON-encoded arguments, and up
to four `arguments`/`args`/`params`/`input` wrappers. It bounds each input at
256 KiB, each object or array at 256 entries, the decoded structure at 16 levels
and 4,096 values, and each normalized request at 256 KiB. Unknown tools retain
only bounded recorded input. A malformed known tool does not become other-tool
input. These decoding rules do not prove that a producer emits every wrapper.

Bash input keeps the recorded command, including inline scripts, heredocs, and
patches. When present, its envelope also keeps scalar `cwd`, `workdir`, `shell`,
`login`, `timeout`, and `timeout_ms` execution context. It excludes descriptions,
output-size controls, polling intervals, and output. Command arrays must contain
strings. This is recorded request evidence, not proof of successful execution.

Search input keeps string or string-array query, pattern, glob, include/exclude,
path, and CWD fields, plus recorded boolean constraints. It excludes matches and
arbitrary nested objects. Read paths and edit paths are separate from outputs
and edit bodies. Patch input accepts a recorded `patchText` string, including
OpenCode `apply_patch` arguments. Patch paths include both `Update File` and
`Move to` paths; content-only selection removes those path headers and retains
line prefixes. Empty or malformed patch input supplies no invented path.
Multi-file edits retain every accepted path. Paths remain literal transcript
evidence. The normalizer does not resolve relative paths, platform syntax, or
globs against files on disk.

Dedicated edit content accepts recorded string bodies and arrays of edits with
recorded string bodies. Numbers, nested result objects, and unrecognized edit
items do not become edit text. An explicit empty string body is observed and
counted as empty. It is distinct from a missing or malformed body. Message and
tool-result strings use the same presence distinction in the shared content
decoder; non-text blocks remain outside selectable text.

Projection retains only selected normalized values. Excluded bodies cannot
survive in its normalized-field map. Changes to excluded bodies do not change
the selected digest at the same publication fence. A changed source fence still
invalidates that digest. Unsupported formats supply no projected actions.

### Recorded operation and path facts

`JevOperationMetadata` retains a typed native lifecycle state and selected
`JevNativeFieldRange` bindings. OpenCode native `state.status` maps pending,
running, completed, and error exactly. Missing or unknown labels remain unknown.
Pinned Claude, Codex, and Pi adapters also retain characterized native result
status and exact joins. Uncharacterized shapes do not infer lifecycle from
request presence, adjacency, or assistant reports. Completed is a recorded label; it
does not establish a passing command or a resulting file state.

Field bindings identify a typed native container, JSON pointer, and UTF-8 offsets within its
decoded string. They are not byte offsets into a transcript file, SQLite cell,
or escaped JSON representation. Bindings cover retained direct string fields
of characterized native request objects. The pointer base is the native
OpenCode part, Claude/Pi record, Codex rollout record, Cursor tool block, or
Antigravity step. Raw Bash strings bind only when retained verbatim. Encoded
JSON strings, nested wrappers, arrays, unknown-tool envelopes, patch-derived paths, and synthesized
store/composer calls have no native field-range claim. No range is fabricated
for a field the producer does not record. Edit keys retain old/new/body
ownership through their native pointers; this does not add a native hunk-range
claim. Native IDs and bindings stay local.

Storage preserves lifecycle state and truncation. SQL filters bindings by the
check's field mask before materialization; projection applies the same mask.
Excluded edit-body bindings cannot affect path-only evidence or its digest.
A truncated known request retains category, malformed state, lifecycle state,
and truncation, but supplies no normalized request text or native range. It
cannot become raw Bash or other-tool input. This fail-closed limit also applies
to oversized raw known-tool input. Unknown tools retain their bounded partial
input under their own selector.

Antigravity's characterized `truncated_fields: ["content"]` marker survives
storage and selection on message/result text. It does not mark tool arguments
as truncated. Other marker-to-field mappings remain uncharacterized. Pi's
subagent filtering preserves original block indexes for native field bindings.

`JevRecordedPathFacts` reads only selected request envelopes. Read/edit paths
retain recorded string CWD/workdir context. Facts bound each path, CWD, and glob
at 4,096 bytes and path/glob lists at 256 entries, and report truncation. Glob
facts distinguish pattern, include, and exclude constraints. Conflicting CWD
aliases stay unknown. Session-header CWD is not propagated into requests in
this contract. Platform must come from a characterized recorded source; no
adapter in this batch records it, and the host platform is never substituted.

The shared lexical resolver requires an explicit recorded POSIX platform and,
for relative paths, an absolute recorded CWD. It collapses `.` and repeated
separators without filesystem access. Parent traversal, shell expansions,
Windows paths, symlink identity, and missing context remain unknown. The typed
POSIX segment-glob helper supports `*` and `?` with case-sensitive matching;
wildcards cannot cross `/`. `**`, brackets, braces, and escapes remain unknown.
Callers must establish this glob dialect separately. These helpers do not claim
that an accepted producer records platform or glob-dialect evidence.

### Characterization and limits

`turn_content_privacy::equivalent_native_requests_have_the_same_selected_meaning_for_all_six_formats`
runs synthetic equivalent assistant, command, edit, read, search, and unknown
tool records through each admitted adapter, the sink, storage, and selection.
It asserts exact selected text and observed field counts. The tool names in
this test exercise recorded call envelopes; they do not establish a producer's
built-in tool inventory. Producer pins and version limits remain in session
coverage.

`turn_content_privacy::native_field_sentinels_remain_isolated_in_fenced_queries_and_projection`
uses the synthetic Claude `selection_isolation.jsonl` fixture. Each of the twelve
single-field queries retains its own sentinel and excludes the other eleven and
private thinking. The empty-field perturbation separately checks empty messages,
edit bodies, and results, and rejects an image block with a misleading text key.
These are adapter/store/projection tests, not model-quality evaluations.
The Pi non-text-result test also rejects image blocks with a text key through
its native tool-result message envelope.

Claude's exact `tool_use_id` joins use a source-local, branch-scoped identity map.
It retains at most 4,096 calls and accepts identity strings up to 512 bytes.
The map survives a validated resume. Reused IDs with conflicting names remain
ambiguous; missing IDs, missing branch links, and calls beyond the cap cannot
gain a name from adjacency or another branch. A joined result records tool
ownership, not successful execution or an authoritative completion boundary.

OpenCode's native SQLite lifecycle fixture covers pending, running, completed,
and error requests. Only completed output and error text supply result bodies
in that fixture. Ignored Instructions can use exactly bound selected Bash
results as context. A completed label
does not prove command success or a resulting file state. Exact recorded call
IDs remain local. Native patch parts remain separate metric evidence and do not
supply a second selected edit action.

Historical instruction snapshots, missing call identities, ambiguous output
joins, relative-path resolution, and producer completion boundaries are not
fabricated. The six-format equivalent-event test is narrower than the product
admission list. Cursor store/composer routes retain normalized messages but lack
native tool proof; Antigravity SQLite needs its assessable companion transcript.
Antigravity Cascade is not admitted. These tests do not establish new native
schemas, human authority, or release ranges.

## Shared execution and storage limits

All registered Jev checks use the same desktop transport, request admission,
provider backoff, response cache, and usage settlement. The scheduler takes one
candidate from each check per round. Within a check, history and recent work
alternate. Least recently attempted candidates lead each lane, so continuing
history pages do not always take the next slot.

The runner saves each completed response before waiting for other requests in
the wave. It saves failure state too. Results reduce in stable work-item order;
the reported provider error uses stable batch-ID order. A checkpoint failure
stops further dispatch. Source and credential fences still control admission,
checkpoint writes, and final publication.

Request packing counts JSON bytes and hashes through writers. It measures each
item independently and builds the final request once per batch. The final
request must pass both the 60 KiB request limit and the 30 KiB state-plus-longest-
question limit. These are byte proxies, not measured provider token counts.

The desktop shares sixteen HTTP slots and a 4 MiB in-flight byte allowance.
Each call reserves twice its request and local-binding bytes plus the 64 KiB
response maximum.
The runner also caps a request wave at a 16 MiB queued-byte allowance shared
across runners. These limits account for serialized payloads and response
buffers; they are not a bound on every allocator or check-owned preparation
object. Provider starts are at least 25 ms apart across checks. Retry-After
and bounded exponential backoff delay the shared start time. A rolling one-
second allowance admits at most 100,000 reserved input tokens. Each dispatched
call starts with a 65,536-token reservation. Confirmed input-token usage replaces
that reservation within the window; known rejection releases it. Unknown
outcomes keep the reservation until the window expires. This is shared across
checks. No live throughput claim follows from these local limits.
Each session's compact progress checkpoint is limited to 2 MiB.

Production HTTP uses a cancellable async request. Cancellation cannot undo a
request that the provider already accepted. A timeout, cancellation after
dispatch, or unusable response can leave the billing outcome unknown. The
store keeps hashed work-item identities and a reservation ID for that outcome
and blocks another dispatch of the same semantic work for that session
incarnation and check, including after append or repacking. An unknown outcome
allows up to three total dispatch attempts. Earlier dispatched attempts may
have incurred charges. If the outcome remains unknown, the identity stays
unresolved and further dispatch of that work is blocked. At 1,024 unresolved
identities, the worker stops new dispatch rather than evict an identity and risk
repeat billing. Clear Local Data removes this state.

At startup, recovery records any dispatched reservation without a final usage
settlement as one unknown outcome. It preserves unresolved identities and their
reservations, so a restart does not authorize another attempt. A later confirmed
usage result can settle the reservation and remove its unknown count. An expired
reservation is removed only when no unresolved request identity refers to it.
Deleting one session removes its session-scoped check state, but does not erase
usage totals already incurred. Clear Local Data removes local sessions, check
state, unresolved identities, reservations, and usage totals. These rules do not
show whether TypeSafe billed a request whose outcome is unknown.

Migration 64 moves cached responses into indexed SQLite rows and preserves the
older bounded cache. Cache writes, confirmed usage settlement, and unresolved-
identity removal share one transaction. Exact lookup reads one response rather
than decoding every cached response. Cache retention is seven days, at most
128 rows and 512 KiB of payload and identity bytes. The rolling usage ledger
retains at most 4,096 unsettled or unknown reservations. Confirmed usage moves
to bounded aggregates, and its settled reservation detail is pruned before the
next request is admitted. A full ledger of unresolved reservations stops new
dispatch rather than dropping billing safeguards.

Migration 65 adds source-order and recent-order content indexes. Production
selected-content reads use bounded keyset pages. A cursor stores a revision, a
hash of the session identity and query inputs, and the last source key, turn
index, row ID, and part index. The query inputs include the published fence
scope, source generation, activity cutoff, source positions, parser, analyzer,
and evidence-schema revisions, and selected fields. A change to any of them rejects the
cursor before content is read. The Store checks that the publication still
matches the session generation, verified source fingerprint, parser and analyzer
revisions, evidence schema, and ready status in the same transaction as the page
query.

Pages use stable source/turn/row/part order, with a separate reverse order for
recent activity. The full position distinguishes duplicate source coordinates
and multi-part rows. Pages seek after the saved position; they do not use
ordinal offsets. After a validated resume, the cursor may continue through rows
that remain in the same immutable publication. Newly published or changed input
requires a new cursor. For recent-history checks, each page also loads bounded
context before the activity cutoff and through the current page boundary; that
context does not advance the cursor. Page state and carried comparison state are
saved separately.

The selected-page query caps output at 288 source IDs and 1.125 MiB of selected
text. These bounds apply to selected content, not every allocation in the
application. Cursor serialization stays local and is not sent to TypeSafe.
Selected normalized-field limits count UTF-8 bytes, including JSON envelope
bytes. SQLite character counts do not define the selected-page byte allowance.

Compact checkpoints save each answer's original request binding, so a later
packing layout does not require a paid request for completed work. Checkpoint
serialization borrows cursor data and caches follow-up descriptions within one
immutable assessment. A check can opt into append reuse only when its shared
reuse scope and exact work-item content, questions, and local bindings match.
Changed reference text, selection, model, or evaluator revisions invalidate it.

# File public-event fixtures

These are synthetic accepted-shape fixtures. They are not private captures or a
claim about all historical producer versions.

## Source contracts

- Codex 0.160.1: public producer pin
  `d27764b82f7118f674371e6d6e76271d9d606edb`. The task aliases and typed aborts
  come from `codex-rs/protocol/src/protocol.rs:1406-1418,2153-2197,4257-4285`.
  `rollout/src/policy.rs:94-134` separates persisted paginated items from legacy
  UI echoes. Public item and raw message shapes come from `protocol/src/items.rs`
  and `protocol/src/models.rs`. The existing retained-user contract also requires
  exact message/turn metadata, `user.text` parts, and a complete retained source.
- Pi 0.84.4: producer pin `b79e4cc834970cca69daebffab7df1da7d1e52c4`.
  `packages/coding-agent/src/core/agent-session.ts:667-724,753-766,805-823`
  persists accepted slot values after `message_end` and extension replacement.
  `session-manager.ts:46-56,1052-1067` supplies entry identity and ancestry.
- Claude: the existing accepted CLI 2.1.278 root/UUID/parent contract. The public
  decoder does not prove final public assistant text. Tests reuse
  `../native_tool_facts/claude.jsonl`; they do not invent that finality boundary.

## Accepted public content

`codex.jsonl` binds one exact task ID to retained input, a complete explicit
custom call, its result, a completed `AgentMessage`, the matching raw assistant
projection, and a task terminal. The custom input looks like JSON but remains
`NativeToolInput::Text`. The raw assistant projection matches the completed
item's ID, turn, and text. Private reasoning supplies activity and request proof,
not public text.

`pi.jsonl` separates explicit calls from completed text. Text parts keep native
ordinals 1 and 3; private thinking occupies ordinal 0 and the call occupies 2.
The text projection sorts after all native blocks. Only characterized `stop`
and `toolUse` stops admit assistant text. Error, aborted, length, missing, and
unknown stops do not. `example-fast` is a custom model-ID suffix hint, not a
verified built-in alias or served-tier claim.

Claude admits complete explicit `tool_use` objects and accepted user messages.
Result carriers do not create users. Same-record duplicate call IDs produce
unsafe call evidence, not an eligible first call. Claude publishes neither
assistant text nor response cycles in this subset.

## Cycle and activity limits

Codex cycles require one exact start and terminal, exact initiating and steering
input bindings, complete admitted public members, and no unresolved explicit
calls. Tests cover both task wire aliases, normal and error completion, and all
four typed abort reasons. Error codes come from `error.codex_error_info`, whose
unit variants are strings and whose structured variants have one object key.
The cycle never copies free-form error text.

Missing starts/input, wrong turns, ID-less terminals, overlapping starts, unknown
public records, compaction, rollback, missing or conflicting assistant
projections, and oversized intervals withhold the cycle. Independently admitted
messages remain usable. Legacy UI/control shapes that lack the required interval
proof also withhold cycles. A conflicting later terminal removes the existing
cycle identity; an equivalent terminal alias does not create another cycle.

The collector counts every serialized member copy and the final cycle envelope
against 4 MiB, with at most 4,096 members. Request uniformity includes recorded
generation contexts and private-only generating requests. Model and mode
uniformity remain independent. A user-only cancellation has no generating model.

Native activity uses explicit source work records, independently of authored
filters. Settings, model selections, polling, usage-only records, generated
context notices, and UI state do not supply activity. Explicit Codex settings and
Pi model changes supply separate session-selection facts. Missing selection
stays unavailable; the latest requester is not a replacement.

## Delivery tests

`public_event_characterization.rs` checks standalone/inline equality, exact
positions and ordinals, private-text exclusion, source gaps, terminal statuses,
steering prefixes, every live record split, JSON/postcard cursor restore, partial
assistant records, and bounded cycles. `file_live/tests.rs` checks named V4
conversion, retained joins, pending results, partial framing, and combined output
pages. `resume_parity.rs` checks the new evidence and task state after every
record-aligned snapshot.

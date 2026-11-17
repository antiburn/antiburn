# Capture metadata

Every [native capture](capture.md) has one separate `capture-metadata` stream.
Its compact UTF-8 JSON uses the same `CaptureMetadata` structure for Claude JSONL
and OpenCode SQLite sources. Native file bytes and typed SQLite cells stay in
their own streams. Metadata does not rewrite either source.

The [JSON Schema](capture-metadata.schema.json) uses Draft 2020-12 and integer
`version: 1`. It describes the full metadata object, not the capture manifest or
the SQLite framing header. These contracts have separate roles even when their
numeric versions are equal.

## Common fields

This is a synthetic OpenCode-shaped metadata object with no optional sections:

```json
{
  "version": 1,
  "agent": "opencode",
  "source_format": "OpenCodeSqliteV2",
  "native_session_id": "synthetic-session",
  "native_agent_id": null,
  "source_agent_version": null,
  "version_observations": [],
  "version_scan_complete": false,
  "owned_version_complete": false,
  "relationship": {"kind": "unknown"},
  "history_completeness": "unknown",
  "sections": {}
}
```

Claude uses `agent: "claude-code"` and `source_format: "ClaudeJsonl"`. Its
session ID and optional child agent ID come from the admitted source identity.
OpenCode uses its native session ID; a persona is not `native_agent_id`.
Capture IDs, account IDs, destination IDs, queue state, and automatic capture
timestamps do not belong in this object.

`source_agent_version` describes a historical native runtime only when owned
native records prove it. It is not the installed runtime or a section producer's
version. Claude retains bounded `version_observations` with `owned`, `inherited`,
or `unknown` ownership. Its two completeness flags describe scan limits, not
complete retained history. OpenCode leaves the source version null, the
observations empty, and both flags false. A native creation or import version
does not prove which runtime produced the session's whole history.

`relationship` is `unknown`, or a directly proved `child` or `fork` with
`parent_source_id` and `proof`. An edge grants no access and does not mean that
the parent was captured. `history_completeness` remains `unknown`.

The core object has declared fields. Extend metadata through namespaced
`sections`, rather than adding unrelated fields to each core type. The schema
rejects extra core fields; Rust does not promise to retain them.

## Section envelope

Each section has the same required envelope:

| Field | Rule |
| --- | --- |
| Map key | Namespaced ASCII identifier such as `example.session-billing`, at most 128 bytes. |
| `schema` | Producer-defined data contract identifier with nonempty dot-separated ASCII parts. It is not a URL to fetch. |
| `producer` | Required `name` and `version`. These identify the section producer, not the native runtime. |
| `observation` | `source_position` with a stream ID and expanded offset, or `observed_at` with positive Unix milliseconds. |
| `evidence` | One through eight typed `native_stream` or `local_observation` items. |
| `data` | Any JSON value: object, array, scalar, or null. |

Identifiers use ASCII letters, digits, `_`, and `-`; dotted identifiers also use
`.` between nonempty parts. Each identifier is at most 128 bytes. A section key
needs at least two parts. Producer names and observation methods do not use dots.

Unknown fields round-trip in the section, producer, observation, and evidence
envelopes. Unknown fields inside `data` also remain intact. The same size bounds
apply to all of them. Extension maps must not collide with declared fields,
including an enum's `kind`. This rule also applies to values constructed
directly in Rust. Extra fields are not a way to replace required fields or bypass
their types.

A `source_position` offset is a zero-based byte position strictly inside a native
stream. It requires `native_stream` evidence for that exact stream and its full
expanded SHA-256. This works with a JSONL transcript, a native companion, or the
[SQLite native stream](sqlite-native.md). The offset addresses exported stream
bytes, not a database page or a position inside decoded cell data. Generated
metadata cannot serve as native evidence.

An `observed_at` time requires `local_observation` evidence. Its `data_sha256`
binds the crate's compact JSON encoding of `data`, not pretty-printed JSON or the
whole envelope. Times range from 1 through 253,402,300,799,999 Unix milliseconds.
A digest proves a byte binding, not the truth of a claim.

## Git context example

This synthetic section records a checkout at capture time. It does not prove
the checkout, branch, or worktree used during earlier session execution. The
library does not collect Git state automatically.

```json
{
  "schema": "example.capture-git.v1",
  "producer": {"name": "synthetic-host", "version": "1.0.0"},
  "observation": {"kind": "observed_at", "unix_millis": 1794873600000},
  "evidence": [{
    "kind": "local_observation",
    "method": "synthetic-git-snapshot",
    "data_sha256": "f7c5e56d25c1a96d91e0c7a70b1c651cea500d6854f5d95f055f9f4d3edd2660"
  }],
  "data": {
    "branch": "example",
    "head": "0123456789abcdef0123456789abcdef01234567",
    "historical_execution_context": false,
    "observation_scope": "capture_time",
    "synthetic": true
  }
}
```

Place it under a key such as `example.capture-git`. A current local checkout
cannot fill missing historical context. Only accepted native evidence can
support a claim about the session's earlier execution.

## Session billing evidence example

Billing context belongs to the selected session and its proved observation
boundary. It is not a generic payment-service integration or a current account
snapshot. The plan in effect when a session was created is unknown unless the
source proves it. A later account plan must not fill that gap.

The following synthetic section illustrates the plan at creation, metered cost,
and this session's contribution to a subscription allowance. It is not a universal
billing contract or a claim that Claude or OpenCode currently exposes these fields.
Its native evidence refers only to this invented JSONL record, including its
final LF byte:

```json
{"synthetic":true,"session_id":"synthetic-session","plan_at_creation":"example-plan","metered_cost_usd":"0.012","allowance_tokens":120,"period":"2026-11"}
```

```json
{
  "schema": "example.session-billing.v1",
  "producer": {"name": "synthetic-host", "version": "1.0.0"},
  "observation": {
    "kind": "source_position",
    "stream_id": "transcript",
    "expanded_offset": 0
  },
  "evidence": [{
    "kind": "native_stream",
    "stream_id": "transcript",
    "expanded_sha256": "6a173527812d91d80270128eb0bca9f8447fbc1ae3ade669ea9f64f7c84deb42"
  }],
  "data": {
    "synthetic": true,
    "session_id": "synthetic-session",
    "plan_at_creation": {"status": "source_proven", "id": "example-plan"},
    "metered_cost": {"amount": "0.012", "currency": "USD"},
    "subscription_allowance_contribution": {
      "quantity": "120", "unit": "tokens", "period": "2026-11"
    },
    "provenance": {"kind": "synthetic_native_record", "expanded_offset": 0}
  }
}
```

Place it under a key such as `example.session-billing`. For an unproved plan,
use an explicit unknown value under the producer's data contract or omit that
claim. Amount, currency, unit, period, and provenance fields can evolve inside
`data` without losing unknown values. Prefer decimal strings for exact amounts,
including money, under the producer's namespaced data contract. State their
currency or unit there. This envelope does not define money calculations.

A session contribution must not become an account aggregate or a calculated
paid balance. A projection must not include private child sessions, or totals
derived from them, when the viewer lacks access. Source relationships do not
grant that access. Missing session-specific evidence stays unknown or absent,
not zero.

## Bounds and validation

| Compact JSON value | Maximum |
| --- | --- |
| One section, including extensions | 16 KiB |
| Entire `sections` map | Eight sections and 32 KiB |
| Full generated metadata stream | 64 KiB |
| One section's evidence | Eight items, at least one required |

Rust enforces serialized UTF-8 byte bounds, including JSON syntax and escaping.
JSON Schema string length limits count characters, not UTF-8 bytes. ASCII
identifier limits agree, but a general `maxLength` cannot enforce these payload
budgets. Arrays, nested data, null values, and unknown fields all consume the
same byte budget.

The schema checks required fields, types, supported numeric version, identifier
shapes, evidence kinds, and counts. Rust validation still checks reserved-field
collisions, compact JSON size, local data digests, matching native stream IDs and
digests, unique native descriptors, and offsets strictly inside those streams.
Callers must verify descriptors against actual bytes and assess producer claims
against source evidence. Neither check proves privacy, current permissions,
historical truth, or provider billing semantics.

Flexible JSON does not authorize reading billing accounts, extra files, or URLs.
The host owns consent, source scope, reviewed data contracts, and minimal
collection. Omit credentials, signed URLs, and unneeded paths. The capture library
adds no billing collector, Git collector, network request, or extra source access.

## Fingerprints and retries

`CaptureMetadata::stable_fingerprint()` hashes compact, sorted-key metadata after
removing only `unix_millis` from typed `observed_at` envelopes. Observation kind,
source position, producer version, evidence, unknown fields, and all `data`
remain. A `data` field named `unix_millis` is not removed.

The Rust producer sorts decoded object keys at every depth by their UTF-8 bytes.
For example, U+E000 sorts before U+1F600. JavaScript's default string sort uses
UTF-16 code units and reverses that pair.

Consumers that verify digests need lossless JSON numbers. Preserve the numeric
text in the produced sidecar: `18446744073709551615`, `1.0`, and `-0.0` must not
become a rounded integer, `1`, or `0`. A plain `JSON.parse` and `JSON.stringify`
round-trip through JavaScript `Number` cannot preserve these forms. Use lossless
number handling and compare decoded keys by UTF-8 bytes when rebuilding the
fingerprint input.
The schema does not enforce this byte fidelity, data digests, native proof, or
UTF-8 byte budgets. These rules describe the producer's encoding; they do not
define a general JSON canonicalization format.

A meaningful metadata-only change affects the source fingerprint even when
native bytes stay the same. A change only to the typed observation timestamp
does not. The saved metadata stream and manifest digest still bind the actual
timestamp and exact bytes. Freeze all metadata, observation boundaries, native
bytes, and compressed bodies for a pending retry. New metadata must not replace
that saved capture.

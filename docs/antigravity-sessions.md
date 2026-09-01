# Antigravity Session Data

Antiburn supports Antigravity 2.0, Antigravity IDE, and the `agy` CLI through
one agent identity. Native installations use one of these roots under
`$GEMINI_HOME`, which defaults to `~/.gemini`:

- `antigravity-cli`
- `antigravity-ide`
- `antigravity`

## Native Storage

A current session can use two related sources:

```text
<root>/conversations/<uuid>.db
<root>/brain/<uuid>/.system_generated/logs/transcript.jsonl
```

The SQLite database is the primary native session source. Its `steps` table is
the conversation timeline, and `gen_metadata` records model generations. The
brain transcript is a display trace. It carries user activity and tool details,
but current native traces usually do not carry token usage.

Antiburn streams the brain transcript for activity and tools. It reads the
database in one read transaction for tokens, cache usage, retries, timestamps,
and model names. A database session still reports token metrics when its brain
transcript is absent. Its title and workspace can remain unknown until the
transcript or another metadata source appears.

Older workspace-storage JSON, saved API cascades, and configured mirror files
remain file sources. A matching native database replaces its brain transcript
in discovery so one session cannot appear twice.

## SQLite Reads

Antiburn opens each database read-only and uses a stable transaction. It does
not copy the database, retain a result set, or load all rows into memory.

Database analysis uses three row streams:

1. Read `gen_metadata.data` to build a bounded invocation-to-model map.
2. Read `steps.metadata` to emit timestamped primary, retry, and background use.
3. Read `gen_metadata.data` again to recover a generation pruned from `steps`.

The database is the sole assistant-generation source for paired sessions. The
brain transcript contributes user activity and tool calls without duplicating
assistant turns. Native usage rows provide complete input, output, cache-read,
and cache-write token classes for model and cache checks.

Each blob has a 1 MiB limit. SQL checks the blob length before Rust requests its
bytes. The parser retains one blob at a time. Invocation maps have a fixed entry
limit. Reaching it marks the analysis partial instead of increasing memory
without a bound.

The source fingerprint covers step and generation row state, bounded row
content, and the optional brain transcript version. SQLite WAL and transcript
activity participate in discovery freshness. This lets a live session
invalidate cached analysis when either source changes.

## Protobuf Subset

Google does not publish the session `.proto` files. The supported field subset
comes from protobuf descriptors embedded in official Antigravity binaries and
independent implementations tested against native databases.

`ModelUsageStats` uses these fields:

| Field | Meaning |
| --- | --- |
| `1` | Numeric model enum, not an input-token count |
| `2` | Input tokens |
| `3` | Total output tokens |
| `4` | Cache-write tokens |
| `5` | Cache-read tokens |
| `9`, `10` | Thinking and response output split |
| `7`, `11`, `12` | Message, response, and provider message identities |

The parser uses output field `3` when present. It can reconstruct output from
fields `9` and `10` when the total is absent. When both forms exist, they must
agree. An inconsistent row is not counted.

The parser does not depend on one outer wrapper field. It checks bounded
length-delimited candidates for a valid `ChatModelMetadata` shape. It skips
unknown protobuf fields and supports varint, fixed-width, length-delimited, and
deprecated group wire forms without recursion. Invalid or truncated wire data
affects only that row.

Model field `19` is canonical. Field `21` is a compatibility fallback only when
it is printable text. This check prevents nested protobuf bytes that happen to
be valid UTF-8 from becoming a model name.

## Limits

The format is private and can change. Output consistency checks detect many
field-layout changes, but no stored checksum verifies the meaning of every
input-side field. Unknown models remain measured but unpriced.

Legacy encrypted `.pb` conversations do not expose this SQLite row interface.
Antiburn continues to use available brain or mirror files for those sessions.
Provider quota percentages and reset times do not come from the conversation
database. The separate live-usage integration reads those values from the
provider or a running local Antigravity process.

All committed fixtures are synthetic. Do not add captured databases,
transcripts, home paths, prompts, credentials, or other local machine data.

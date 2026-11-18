# Desktop Cloud capture

Antiburn Cloud is waitlisted. [Sign up for the waiting list](https://antiburn.com/cloud/).
Public desktop builds keep Cloud disabled until launch.

This standalone crate owns bounded capture of Claude files, Codex rollouts, Pi
session files, and OpenCode SQLite. It also owns private disk spooling, a durable
many-session queue, and upload reconciliation through a caller-owned transport.
It contains no network client.

The default build has no capture API. Select the explicit non-default `cloud`
feature for controlled integration. Production upload enablement, consent,
credentials, service endpoints, and scheduling belong to the desktop host.

This crate is separate from [`antiburn-local`](../antiburn-local), which owns
local session discovery, parsing, and analysis. Capture packages native source
and capture metadata, not the desktop's parsed evidence, cost estimates, or
Burn Check results. See [capture and local analysis](../../docs/cloud/capture.md#capture-and-local-analysis)
for the data and consent boundaries.
Here native refers to the provider's original session formats, bytes, and
recorded identities. The application performs desktop capture of those sources.

Read the [capture contract](../../docs/cloud/capture.md) for source ownership,
independent stream chunks with fixed 8 MiB expanded bounds, stable source
fingerprints, typed metadata sections, frozen upload plans, limits, acceptance,
and platform scope. Use `resume_step` to bound each scheduler turn to one saved
chunk. The host owns persistent selections, accepted fingerprints, and fair
scheduling.
After acceptance, save the accepted fingerprint and terminal request durably,
then call `acknowledge_accepted` to remove the retained receipt.

## Host integration

1. Authorize one Claude source root, Codex rollout, Pi session file, or OpenCode
   database. Scope each candidate with the host's stable account and device-local
   source scope.
2. Save request and consent bindings, then capture with a new globally unique ID.
   If capture leaves `Preparing`, cancel explicitly before using a new ID.
3. Schedule one `resume_step` per pending request after checking current consent.
   Retry saved bytes and bindings, not a new source generation.
4. Save the accepted fingerprint and terminal request durably, then acknowledge
   the accepted receipt. Executor stop and logout do not cancel requests.

Use `discover_claude` for an authorized project directory or `discover_opencode`
for an authorized V2 database. `Candidate::source_kind()` identifies the native
contract. `source_path()` is a display locator; OpenCode candidates share a
database path but have independent native IDs and scoped source keys.
See the [OpenCode source guide](../../docs/cloud/opencode.md) and
[SQLite native format](../../docs/cloud/sqlite-native.md) for ownership and cell
preservation rules. A source schema error is not a saved-format migration.
Use `reopen_codex` for one authorized rollout and its expected native session ID.
See [Codex rollout capture](../../docs/cloud/codex.md) for its ownership and
folder permission rules.
Use `reopen_pi` for one authorized Pi session file and its expected native
session ID. See [Pi session capture](../../docs/cloud/pi.md) for its rules.

[`Transport`](src/contract.rs) is the desktop host's service boundary. Its API
documents method ordering, exact body handling, frozen plans, and closed errors.
The host keeps credentials and upload decisions outside renderer code and runs
blocking work on bounded workers, never on UI or async runtime threads. See the contract's
[operator recovery](../../docs/cloud/capture.md#failure-and-operator-recovery)
before handling a spool that cannot open or reducing limits.

## Tests

Tests use public synthetic fixtures and files generated inside temporary directories.

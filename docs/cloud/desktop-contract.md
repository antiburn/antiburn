# Cloud desktop contract

Antiburn Cloud is waitlisted. [Sign up for the waiting list](https://antiburn.com/cloud/).
Standard public builds keep Cloud disabled. This contract covers opt-in
Cloud-enabled development builds, not production availability.

Development builds use a browser-approved desktop connection, not a general
OAuth or OpenID Connect service. Builds without the `cloud` feature include no Cloud commands,
capabilities, callback scheme, account controls, or Cloud onboarding step. Local
sessions and Checks do not need Cloud.

## Local analysis and Cloud capture

[`antiburn-local`](../../crates/antiburn-local) owns local session discovery,
parsing, and analysis. Its parsed evidence and metrics support local session
views, API-equivalent cost estimates, and Burn Checks. It does not own Cloud
consent, capture spools, or uploads.

The separate [`antiburn-cloud-capture`](../../crates/antiburn-cloud-capture)
library prepares bounded native source packages and reconciles uploads through
a host-supplied `Transport`. These packages contain native source streams and
capture metadata, not serialized local evidence, cost estimates, or Check results.
See the [capture contract](capture.md) for source fidelity and transport rules.

On supported Unix platforms, the desktop can capture and upload supported native
sessions after separate, account-bound ongoing content consent. A desktop scheduler
owns this work independently of the onboarding or Settings renderer. Project
discovery alone does not start it. Legacy saved-request commands remain
discovery-only and do not enable ongoing uploads.

Local analysis needs no Cloud account or content consent. Model-based Burn Checks
use their own configured connection and **Run checks** setting. They can send
selected inputs when Cloud is disabled; Cloud content consent does not authorize
these provider requests. See [data and network requests](../../apps/desktop/README.md#data-and-network-requests)
for the separate controls and input limits.

## Desktop build gate

Cloud requires the explicit, non-default Cargo feature `cloud`. Neither an
environment variable nor the `distribution` feature enables it. The standard
desktop debug launcher selects it explicitly. Production build commands do not.

The standard debug launchers use a separate app identifier and data directory.
Settings, source selections, saved requests, auth state, and retry files remain
separate from the release app. Source selections can be reused within one profile;
this does not move credentials between service origins.

Desktop `AppInfo.cloudEnabled` enables the existing Cloud teaching and setup
screens after Checks. Builds without Cloud finish after Checks, including when an
older saved introduction points to a Cloud screen. Service configuration,
offline errors, and unsupported capture platforms do not turn the build feature
off. Cloud setup always permits local completion without source or content
consent.

When Rust debug assertions are enabled (`cfg(debug_assertions)`), the issuer
and resource both default to `http://localhost:3000` when neither
`ANTIBURN_CLOUD_ISSUER` nor `ANTIBURN_CLOUD_RESOURCE` is set. The local service
must be running for sign-in to work. Partial or invalid overrides disable
authentication instead of falling back to localhost.

Both variables are build-time canonical origins without a trailing slash. They
must match the service's exact issuer and resource strings. Debug assertions
also permit canonical loopback HTTP service origins. A custom release build
that explicitly enables debug assertions uses the same debug behavior.

Standard release builds keep Cloud disabled. Manually enabling `cloud` without
debug assertions requires both HTTPS origins; it adds no implicit configuration.
Route URLs still include the slash before the path. The resource origin owns
the desktop API routes. Use an approved development service only.

The explicit feature command also works for a debug build:

```sh
cd apps/desktop
pnpm tauri dev --features cloud --config src-tauri/tauri.debug.conf.json
```

A manual Cloud build does not permit production use. System-browser and OS
credential-storage acceptance remain separate from synthetic checks. Capture and
upload also require current service capabilities and explicit ongoing content
consent; a build feature or successful sign-in is not permission.

## Browser approval

The connection opens in the system browser. It uses S256 PKCE and a random
32-byte state. Before browser launch, the client binds an ephemeral port on
literal `127.0.0.1`. Its exact callback URI is
`http://127.0.0.1:<port>/cloud/callback`.

The listener accepts only loopback peers, GET, HTTP/1.1, the exact callback path,
and one Host matching the bound IP and port. It rejects request bodies, Origin,
duplicate Host headers, absolute request targets, encoded path variants,
fragments, unknown query fields, and duplicate query keys. Reads and header size
are bounded. Both state and issuer must match the pending transaction.

Attempts expire after ten minutes. Cancellation, logout, another connection
attempt, and a process restart invalidate the old attempt. A restarted process
does not restore an ephemeral listener or reuse its verifier.
An OS missing-launcher error before browser handoff clears only that attempt and
permits retry. Other opener errors do not prove that handoff failed.
Browser approval can create a connection before the callback arrives. Canceling,
replacing, or losing that callback therefore preserves unconfirmed revocation.
Without a refresh credential, Settings requires an explicit local reset before
reconnecting and keeps the warning. An explicit browser denial does not need this
reset.

The browser must name the workspace, permitted action, and device before the user
approves the connection. The approved scope covers this first-party desktop
client only. It does not grant general API-client or third-party authorization.
Token responses bind opaque credentials to a grant, user, personal workspace,
fixed grant deadline, `capture:write`, resource, and issuer. The flat session
response supplies those same bindings, nullable display name, workspace name,
device display label, and access deadline. Deadlines use RFC 3339 strings.
Desktop validation compares the token and session bindings before saving a
refresh credential. A changed grant, account, destination, or fixed deadline
cannot restore the saved connection.

Desktop authentication requests use the configured API origin:

| Request                   | Desktop proof                                                                                                |
| ------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `GET /desktop/authorize`  | `redirect_uri`, `state`, `code_challenge`, `code_challenge_method=S256`, and the display label `device_name` |
| `POST /v1/desktop/token`  | JSON `{grant_type:"authorization_code",code,code_verifier}` or `{grant_type:"refresh_token",refresh_token}`  |
| `GET /v1/desktop/session` | Access bearer                                                                                                |
| `POST /v1/desktop/logout` | JSON `{refresh_token}`                                                                                       |

Token, session, and logout requests send no cookies or Origin header. Token and
logout proof does not use a bearer header. Desktop requests do not follow
redirects or retry automatically.

## Credential and logout safety

Opaque access tokens stay in desktop process memory for at most 15 minutes. Only the OS keyring stores rotating refresh
tokens. There is no plaintext or database fallback. The connection has a fixed
one-year deadline. Refresh does not extend that deadline.

A single Cloud blocking-work permit admits synchronous keyring, SQLite, source,
state-file, and short HTTP operations before they enter Tokio's blocking pool.
Ordinary requests fail clearly as busy without starting work. Cleanup waits
asynchronously for the permit. Sign-out, reset, and browser cancellation invalidate
their account or browser generation before waiting. A busy connection attempt
does not invalidate the pending browser attempt. An admitted connection attempt
cannot replace a newer cancellation while it waits for a blocking worker.
An awaiting caller cannot release a permit still owned by running work.
Cloud table initialization runs lazily in that worker before table access, not
during UI startup. Failed initialization remains retryable and does not hide
credential cleanup.

Loopback accept, read, write, cancellation polling, and expiry use Tokio async I/O
outside the controller mutex and blocking-work permit. The existing HTTP transport
remains synchronous with a 15-second timeout per request, only inside admitted
blocking work. This does not make the whole authentication engine asynchronous.

A process-wide mutex serializes credential operations. One lifetime OS lock owns
the vault namespace. A second process cannot write credentials until that owner
exits. The vault namespace includes the application identifier and state
directory. Atomic state-file replacement uses a separate stable lock file. State
contains only a random generation, a closed state, and a revocation flag, never a
credential, callback, or verifier.

Before sending a refresh, the client flushes a reauth-required marker. An
uncertain response, process exit, invalid response, keyring failure, or failed
session verification must not retry the predecessor. A non-timeout connection
setup failure proves that no HTTP proof was sent; only that result can restore
retry eligibility for the current account generation. Successful rotation needs
the same grant ID, account, workspace, resource, scope, issuer, and fixed deadline.
Generation checks before and after storage prevent late results from restoring a
cancelled account.
Rejected refresh responses save the original and returned proofs for logout
before updating local state. These cleanup credentials cannot restore an
authenticated session. Cleanup requires confirmation for both proofs. A failed
secure-storage write keeps an unconfirmed warning and attempts to revoke the
returned proof.
Decoded responses retain bounded logout proof even when token bounds or authority
are invalid. An unreadable authentication state keeps remote uncertainty; cleanup
of a newer connection cannot prove that an older connection was revoked.

Settings keeps the connection action available after onboarding. Desktop sign-out
first flushes a new signed-out generation. Browser cancellation changes only the
pending browser-attempt generation; it does not cancel an established connection
or its refresh. Sign-out revokes only this client's current
grant, not every connection on the account. A failed remote response stays visible
as unconfirmed revocation. The client durably records confirmed remote revocation
before deleting its keyring credential. A restart can therefore finish local
deletion without requiring that deleted credential or repeating remote logout.
A deletion failure remains visible, and the closed marker blocks login
restoration. Cleanup-only keyring records cannot restore an authenticated session.
Only a synchronous HTTP 204 confirms remote logout. HTTP 202 does not.
Status reads can rotate credentials or close access. Desktop state updates reach
every account surface, including before the requesting renderer receives its
reply. Renderers attach their listener before reading and clear content consent
when authority changes.

When remote revocation is unknown and no refresh credential remains, Settings
offers an explicit local reset. The confirmation states that this does not revoke
the old connection. Reset permits a new connection but preserves an
unconfirmed-revocation warning, including after reconnect and restart. The user
can revoke the old connection in browser Settings. Reset does not remove retry
bytes or accept late browser callbacks.

Callback URLs, codes, tokens, PKCE material, and raw provider errors do not enter
renderer events, logs, or analytics. Identity and folder paths do not enter
analytics. Logout, auth failure, and cancellation do not delete, rebuild, or
retarget saved capture bytes.

## Destination and content consent

Only the currently verified account and personal destination can enable sync.
Unknown membership or an explicit team destination blocks it; the desktop does
not silently replace a team with personal. Consent also stays bound to the
configured service origin. Account, destination, or service changes cannot
retarget saved captures.

Sign-in, source access, folder toggles, and the legacy local **Upload sessions**
preference do not grant upload consent. Clicking **Start syncing** confirms
full-content upload for the current selection. The folder screen's short subtitle
states the scope; no separate consent checkbox or footer notice is required. `desktop-project-sync-v1`
binds the account, personal destination, exact provider sources, and approved
project/worktree paths, with full content and ongoing sync enabled and external
attachments disabled. It covers existing sessions and new sessions in those
approved folder trees while uploads remain enabled.

Full native content includes messages, private thinking, tool calls and results,
and session metadata. It can contain secrets or private file text. OpenCode V2
also includes proved session-owned content stored inside the selected database,
including native blobs. External attachment files, arbitrary linked files, and
URL downloads remain excluded. This consent remains separate from local analysis
and model-based Checks.

Changing the account, destination, provider source, or selected path membership
invalidates any pending approval for an earlier selection. Count-only updates do
not change consent. Settings saves folder opt-outs immediately and confirms
additions before saving them.

### Project suggestions and worktrees

The project list uses cached local session metadata from all discovered agents.
It can show projects for agents that cannot upload. Counts describe local or
provider-source discovery, not accepted Cloud sessions. Only qualified Claude and OpenCode
V2 sources can produce upload jobs; a suggested project or checked row does not
add support for another provider.

New onboarding project choices default to selected in renderer memory. Loading a page or
toggling an onboarding row does not save ongoing consent or switch uploads on. Existing
explicit choices and saved opt-outs take precedence over a new default.
**Start syncing** approves only the currently loaded selected groups and their
exact current path members. **Load more folders** does not approve unseen groups.
Until confirmation, an existing saved policy stays unchanged.

After initial consent, Settings switches show saved project permission rather
than draft defaults. Turning a group off removes its approved bindings immediately.
Turning it on requires the Settings confirmation dialog for its full sessions and
current worktrees. The command checks the exact policy revision and group membership,
then changes only that group. Other groups, unseen pages, and the global pause remain
unchanged. The last group can be turned off without deleting saved captures.
There is no Update button. Onboarding keeps its draft-and-Start flow without a dialog.

Confirmation saves the complete `projectChoices` map, including exclusions,
atomically with the upload policy. Status returns the same map for reopen.
These group choices are UI preferences, separate from provider-file preferences.
Only the policy's approved `projects` grant upload permission. Older policies
without a choice map load an empty map; the UI requires review before changing
their selection.

For local paths, current local Git metadata can group a repository and its
worktrees into one displayed project. This is a present-day display association,
not historical session ownership or upload permission. Approval includes the
exact current worktree paths in the selected group, including paths outside the
main repository. A new worktree outside the already approved folder trees needs
confirmation. Git association alone cannot add it to a saved policy. Foreign
environment paths are not probed as local directories.

Recorded project and working-directory (CWD) evidence controls session eligibility.
Any proved source-owned CWD inside an approved folder or its descendants can match.
Matching uses path components, so `/work/app` does not include `/work/app-other`.
CWD changes during a session are valid. Provider storage paths, encoded Claude
directory names, parent/fork links, file-touch paths, and tool CWDs grant no
project permission. Each child and fork remains an independent native source.

A matching session uploads in full. This is an allowlist, not redaction:
messages and tool results from other CWDs or paths in the same session remain
part of the upload.

### Source access and discovery limits

The desktop host finds supported provider sources through existing local metadata
and provider adapters. Settings uses these default discoveries without source pickers.
Source access does not permit every project in that directory or database.
OpenCode support follows the structural [V2 source contract](opencode.md), not
every historical or future release. V1 and unsafe or unsupported stores have no
fallback. Windows has no desktop capture fallback.

Project lookup can read bounded local Claude transcript bytes to collect recorded
CWDs, but does not upload content. The host keeps a bounded metadata cache and
checks source changes. Cached evidence can suggest work; new capture still needs
fresh admission and a full project-policy check after reservation. Unknown or
ambiguous owned CWDs cannot enable capture. A Claude record above 8 MiB makes the
project scan incomplete; large tool results below that bound can retain valid
CWD evidence. Malformed or incomplete records can also block admission.
OpenCode's recorded session directory does not prove a complete historical CWD list.
See [project folder selection](capture.md#project-folder-selection) for the source rules.

Listings are bounded and can report partial results. Successful source listings
contain at most 512 candidates across up to 100 source roots. A saved policy
permits at most 100 source/project path bindings. Missing access, unsupported
schemas, and exceeded bounds do not become a complete empty listing or permission
to upload an excluded source. Restore access or retry discovery in Settings.
Setup failures do not hide account cleanup controls.

Before enabling sync or making a new capture, the desktop host checks current
upload capabilities. The desktop source profile is at most 128 MiB per Claude
transcript or expanded OpenCode native-row stream, and can be lower under the
service's advertised total limit. Companion and metadata space remain included
in the capture budget. Missing, disabled, or incompatible capabilities block new
capture; the host does not clip source or fall back to guessed limits.

### Background sync and saved bytes

**Start syncing** moves onboarding to completion without waiting for capture,
transfer, or integrity acceptance. **Done** remains available while setup or
uploads are pending. Closing onboarding stops renderer observation, not the
saved ongoing policy or desktop scheduler. Local tools setup remains
independent of Cloud sync.

Settings → Sessions shows the account, personal destination, upload switch,
folder choices, and one short upload state without internal dividers. The normal view omits transfer counters.
Folder-read and saved-upload failures use brief messages, with recovery actions
when available. Known blocked or interrupted work remains visible while other
uploads run or the policy is paused.
Errors use an inline alert rather than muted routine status. Jobs excluded by the
current folder policy retain their bytes but do not count as current upload failures.

`storageKnown` is true only when saved-state reads succeed. When false, status
counters and progress are unknown or last-known values, not current results. The desktop
still publishes `storage_unavailable` when it cannot read saved files. Last-known
values must match the current account, destination, and service.
**Upload sessions** pauses or resumes the confirmed policy. For saved-upload errors,
**Retry** retries permitted saved work without expanding the selection.
`recoveryRequiredCount` identifies missing or incomplete local captures that
cannot resume from their saved state. These count as interrupted, not queued.
They do not block healthy sources. When only these failures remain, Settings
shows a local-recovery message without a Retry button that cannot fix them.
Pause stops admission of further sensitive work and saves the paused
state. It does not delete spools, undo requests already sent, or delete remote
content. Sign-out and account changes also stop unauthorized work without
retargeting or deleting saved captures.
Signing in again to the same account and workspace resumes eligible saved uploads
automatically. A new sign-in clears conflict cooldowns but preserves capacity
and offline backoff. It does not replace the capture, change its destination,
expand folder consent, or resume a paused policy.

Temporary request failures and HTTP 429 capacity rejections defer the current
upload until its retry deadline. Valid `Retry-After` hints use a 1 to 300 second
bound. A saved request's delay does not block other ready uploads.
The desktop then retries the same saved capture ID after checking current source
and policy bindings. Saved uploads take priority over new captures. Saved bytes
remain intact. Historical unknown failures before reservation still require explicit **Retry**.

A proved local queue or disk-capacity refusal before publication saves a separate
wait. The same capture ID can retry after fresh source and project checks.
Unknown missing entries cannot use that permission.

Initial registration, part, and finalization conflicts first try renewal of the same reservation,
manifest hash, and plan. Only a successful renewal permits one replay of the
original request. This recovers idle expiry without reading source again or changing
capture identity. Server authority, absolute expiry, writer, and cleanup fences still
apply. Earlier fenced post-reservation uploads receive a bounded automatic recheck;
persistent conflicts wait at least 60 seconds between attempts. Empty interrupted
captures do not gain permission to read source through this recovery path.

After sign-in, process restart, or explicit Retry, the scheduler checks saved
finalization status. If integrity is still pending, it can replay the exact
saved finalization once. This can restart stopped processing without new source
reads or a new capture. Polling alone does not resubmit finalization.

Time expiry is recoverable, including after a weekend offline. The service
rechecks current authority and the saved request before renewing storage work.
Expired incomplete parts can require transfer again from the same local bytes.
This never grants permission to read source again, change the capture, or
overwrite a newer publication. Deletion and cancellation remain final.

Capture API requests use the configured resource origin and desktop-held credentials.
Saved gzip bodies go only to validated, short-lived upload locations supplied
for the bound request. Those body uploads carry no API bearer or cookies.
Upload locations never enter saved state, renderer status, or diagnostics.
Resume diagnostics record closed states, whether a saved plan exists, request
routes, renewal outcomes, timing, and retry delays. They never contain capture
content, source paths, sign-in proof, signed URLs, or raw provider errors.

The scheduler and capture spool serialize bounded work. Pending captures keep
their original account, destination, service, source, capture ID, compressed
bytes, and frozen plan across restarts. Narrowing the approved folder policy
blocks affected saved captures rather than removing part of their content.
Jobs bind the approved tree used for admission, not unrelated folders in the
same provider store. Removing an unrelated folder does not fence the job.
Retry can replace only a present, verified empty capture that has no reservation.
The desktop saves that recovery decision before removing the empty entry.
Missing entries cannot start recovery or authorize recapture. They can only
finish a previously saved empty-recovery decision. Partial captures and
post-reservation interruptions keep their IDs and bytes; they are not replaced
with newer source. Missing or unreadable saved state is not permission to discard
retry files.

HTTP 202, a successful upload request, transfer receipts, progress counters, and
an operation ID do not prove full integrity acceptance. Automatic spool release
requires exact bound status with `integrity_accepted: true` after full stored-byte
verification. The host saves the accepted source fingerprint and terminal job
state before acknowledging the retained library receipt. Explicit cancellation
is the separate release path for an unaccepted capture, not remote deletion.
Saved bytes and operator recovery remain governed by the [capture contract](capture.md).

### Local tools status

Completion reads `get_local_tools_status` and observes `local-tools:changed`.
Both carry `{revision, phase, cliInstalled, cliTested, skillInstalled, reason}`. The phases
are `pending`, `ready`, `unavailable`, and `failed`. Tool status is separate from
Cloud authentication, folder discovery, and upload consent. Pending or failed tool
setup does not block these paths or **Done**.

The renderer attaches the listener before reading. It keeps the latest native
revision in memory for the current app process. It ignores queries and events
with lower revisions. A successful query can replace an intervening event when
its revision is at least as recent. An accepted event supersedes a pending query
error, which has no revision. Closing the observer rejects late replies
and removes its listener. Tool setup can settle from events without a refresh
control or polling.

**Local tools ready** means the CLI passed its capability test and the generated
skill and bound config match their ownership receipts. It does not prove host
skill discovery, vendor callbacks, or native integration delivery. Failed setup
points to **Repair** in Settings → CLI. That existing action retries debug tool
setup while preserving ownership checks. Read-only status calls do not install
or repair tools. See the [CLI contract](../cli-installation.md#debug-cli-and-skill)
for profile and platform limits.

### Legacy saved source requests

`cloud_prepare_sync`, `cloud_start_sync`, and `cloud_reconcile_sync` retain the
discovery-only saved-request contract. They do not create an ongoing policy or
start desktop capture/upload. **Save capture request** on that compatibility path
still reports **Upload unavailable**. Older `native-full-content-v1` and
`native-full-content-v2` confirmations cannot become `desktop-project-sync-v1`
consent.

Legacy Claude and typed-provider requests retain their serialized consent,
source scopes, capture IDs, and locators. Reading or reconciling a completed or
cancelled selection does not rediscover it or convert its records. Unknown saved
provider kinds fail without rewriting data. Cancellation does not remove
unrelated captures or remote content.

## Validation

Desktop checks use synthetic sessions, in-memory keyrings, temporary state files,
separate test processes, and loopback sockets. They must cover PKCE, callback
guards, generation fences, restart, uncertain refresh, keyring failure, fixed
deadlines, logout, cross-process locking, ongoing account/project consent,
multi-CWD matching, worktree grouping without implied permission, policy pause
and retry, negotiated capture bounds, retained retry bytes, integrity acceptance,
stable legacy records, provider dispatch, and independent database sessions.
Source checks use the public synthetic SQL fixtures in private temporary
databases. Tests must not read user provider folders, installed databases, or
real credentials.

Synthetic service-transport HTTP tests run only with debug assertions enabled.
General Cloud tests remain enabled in every test profile. These mocks do not
prove a live service connection.

Run desktop Rust formatting, Clippy, and focused Nextest checks with `--features cloud`.
Also check the ordinary build with `--no-default-features`. Frontend checks cover
Cloud stores, consent, ordinary-build entry-point exclusion, and searchable
Settings controls. System-browser and OS-keyring acceptance on each supported
platform remains separate from synthetic checks.

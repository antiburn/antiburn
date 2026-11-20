# Cloud desktop contract

Antiburn Cloud is waitlisted. [Sign up for the waiting list](https://antiburn.com/cloud/).
Standard public builds keep Cloud disabled. This contract covers opt-in
Cloud-enabled development builds, not production availability.

Development builds use a browser-approved device connection, not a general
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

On supported Unix platforms, Desktop can capture and upload supported native
sessions after you enable upload for a folder. Enabling a folder is the upload
decision. There is no second content prompt. Workspace sign-in and browser
approval are separate and do not enable folders. A desktop scheduler runs
independently of onboarding or Settings. Discovery and legacy saved requests
do not start it.

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
the device API routes. Use an approved development service only.

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
Without a refresh credential, Settings offers a local reset for that exact
workspace. Reset permits a new connection but keeps the warning about remote
revocation. An unresolved team approval does not block Personal sign-in. An
explicit browser denial does not need a reset.

The browser must name the workspace, permitted action, and device before the user
approves the connection. The approved scope covers the first-party device
connection only. It does not grant general API-client or third-party authorization.
Token responses bind opaque credentials to an account grant, user,
fixed grant deadline, scope, resource, and issuer. The session
response supplies those same bindings, a nullable display name,
the device label, and the access deadline. Deadlines use RFC 3339 strings.
Desktop accepts only these exact scope strings:

- New approvals use `capture:write sessions:list sessions:read`. This scope
  permits capture and authorizes search and reads for sessions the signed-in
  user can access. The service still checks the user's current access. Desktop
  capture uses only the capture permission.
- `capture:write` permits capture only. Older account grants keep this scope
  when refreshed.

Desktop requires the token and session bindings to match before it saves a
refresh credential. A changed grant, account, scope, or fixed deadline
cannot restore the saved connection. To receive the broader scope on an older
connection, sign in again instead of refreshing the old grant.

Device authentication requests use the configured API origin:

| Request                          | Proof                                                                                                       |
| -------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `GET /devices/authorize/account` | `redirect_uri`, `state`, `code_challenge`, `code_challenge_method=S256`, and the display label `device_name` |
| `POST /v1/devices/token`         | JSON `{grant_type:"authorization_code",code,code_verifier}` or `{grant_type:"refresh_token",refresh_token}` |
| `GET /v1/devices/session`        | Access bearer                                                                                              |
| `GET /v1/captures/destinations`  | Access bearer                                                                                              |
| `POST /v1/devices/logout`        | JSON `{refresh_token}`                                                                                     |

Desktop uses the short OS hostname as `device_name`. It omits the domain suffix
and falls back to `Antiburn Desktop` if the name is unavailable or unsuitable.
The label is sent when starting browser approval and is saved for that
connection. A rename does not change labels on existing connections.

Token, session, workspace list, and logout requests send no cookies or Origin
header. Token and logout proof does not use a bearer header. Desktop requests do
not follow redirects or retry automatically.

The workspace list returns `{workspaces:[{workspace_id,name,type,membership_generation}]}`.
Each `workspace_id` is a UUID string and `type` is `personal` or `team`.
`membership_generation` is an opaque string for Team and `null` for Personal.
Desktop verifies that the list includes the signed-in account's Personal
workspace. With multiple eligible workspaces, the user chooses a destination
for each folder. With only Personal available, Desktop shows a folder switch.

One account approval covers Personal and current or future Teams where the
account can upload. Approval does not turn on uploads or approve a folder. Each
capture names its exact workspace and sends the saved membership generation for
Team or `null` for Personal. A missing or stale Team permission does not fall
back to Personal. An old workspace grant does not become an account grant.
Sign in again to use the account flow. Old refresh proof can only sign out the
grant it belongs to.

## Credential and logout safety

### Read sessions from the CLI

On macOS and Linux, a Cloud-enabled CLI can reuse an active, read-capable
device connection from a running Antiburn Desktop app. The app owns sign-in
and refresh. It gives the CLI short-lived access proof over a private local
socket that checks the OS user. It does not write the access proof to a file or
give the CLI the refresh credential. This does not isolate processes that run
as the same OS user.

The CLI starts with the Personal workspace. It can request another workspace
by ID. The service checks current workspace and session permissions on every
search and document read. It uses the workspace session API, not a device-only
or CLI-only endpoint. A connection approved only for upload cannot serve
CLI reads. Open Antiburn Desktop and approve a new connection to enable reads
where the service offers them.

| Request | Device scope | Page fields the CLI uses |
| --- | --- | --- |
| `GET /v1/workspaces/{workspace_id}/sessions` with `q` and `page[size]` | `sessions:list` | `items`, `page.next_cursor` |
| `GET /v1/workspaces/{workspace_id}/sessions/{session_id}/document` with optional `page[after]` | `sessions:read` | `workspace_id`, `session_id`, `revision`, `stale`, `messages`, `page.next_cursor` |

Send the access token as `Authorization: Bearer <token>`. A cursor continues
the same document revision; it does not grant access. The service checks
permission again when the CLI reads the next page.

### Device credentials

Opaque access tokens expire after at most 15 minutes. The app and CLI keep
them in memory. Only the OS keyring stores rotating refresh
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
the same grant ID, account, resource, scope, issuer, and fixed deadline.
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

Only a verified account grant and current permission for the chosen workspace
can enable sync. Folder approval stays bound to the account, workspace, issuer,
and resource. Team policies also save the membership generation. If the saved
proof is missing or no longer current, Desktop holds uploads for those folders.
Once Desktop verifies the current membership for the same Team, Settings lists
the exact saved folders. **Save** approves that list and any folders you chose
for the same Team in one update. It checks the current policy revision and
validates the new choices. It does not unlock old jobs, change their saved bytes,
or resume a paused device.

If a session matches enabled folders in more than one workspace, Desktop
uploads the full session independently to each matching workspace where consent
and membership are current. Each upload has its own destination and capture ID.
An unavailable workspace does not block another eligible destination. Existing
jobs keep their original destination, capture ID, and retry conditions.

An older capture with no saved issuer cannot retry. An explicit **Save** can
replace folder consent that has no issuer with the current verified scope.
Desktop does not add an issuer to old captures. Eligible sessions can use new
capture IDs. Desktop keeps the old spool bytes and counts them toward its
storage limit. It also keeps accepted receipts and deletion markers for the
same resource, account, and workspace.

Sign-in, source access, and the legacy local **Upload sessions** preference do
not grant upload consent. Use **Start syncing** during onboarding or **Save**
in Settings to approve selected folders. Each save selects one workspace.
To change a saved folder's destination, turn off the old rule, then
approve the folder for the new destination. This does not move sessions already
in Cloud. The new rule may upload older local sessions. `desktop-project-sync-v1`
binds the account, workspace, exact provider sources, and approved
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
not change consent.

### Project suggestions and worktrees

The project list uses cached local session metadata from all discovered agents.
It can show projects for agents that cannot upload. Counts describe local or
provider-source discovery, not accepted Cloud sessions. Only qualified Claude,
OpenCode V2, Codex, and Pi sources can produce upload jobs; a suggested project
or checked row does not
add support for another provider.

With one eligible workspace, new onboarding choices start selected. With
multiple workspaces, new choices start off. Loading the page or changing an
onboarding row does not save consent or turn on uploads. Existing choices and
saved opt-outs take precedence.
**Start syncing** approves only the currently loaded selected groups and their
exact current path members. **Load more folders** does not approve unseen groups.
Until confirmation, an existing saved policy stays unchanged.

After initial consent, Settings shows the saved folder rules. Turning off a
group removes its approved paths immediately. Adding a folder does not grant
permission until you select **Save**. Save approves full sessions and current
worktrees for the selected groups. The first Save checks current source and
recorded folder evidence, just like later additions. Changes check the current
policy revision and do not change other groups, unseen pages, or the device
pause. Turning off the last group does not delete saved captures.
If a saved folder no longer appears in discovery, its exact saved row still
lets you turn it off without access to the original source.

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

Before enabling sync or making a capture, Desktop reads
`GET /v1/captures/capabilities`. It sends only formats in the response: `ClaudeJsonl`,
`OpenCodeSqliteV2`, `CodexJsonl`, or `PiJsonl`. The source limit is at most 128 MiB
per transcript or expanded OpenCode native-row stream. A lower advertised limit
can apply. Missing support blocks new capture without truncating or dropping
saved bytes.

### Background sync and saved bytes

**Start syncing** moves onboarding to completion without waiting for capture,
transfer, or integrity acceptance. **Done** remains available while setup or
uploads are pending. Closing onboarding stops renderer observation, not the
saved ongoing policy or desktop scheduler. Local tools setup remains
independent of Cloud sync.

Settings → Sessions shows the account, verified workspace count, device upload
switch, folder choices, and upload status. The normal view omits transfer counters.
Folder-read and saved-upload failures use brief messages, with recovery actions
when available. Known blocked or interrupted work remains visible while other
uploads run or the policy is paused.
Errors use an inline alert rather than muted routine status. Jobs excluded by the
current folder policy retain their bytes but do not count as current upload failures.

`storageKnown` is true only when saved-state reads succeed. When false, status
counters and progress are unknown or last-known values, not current results. The desktop
still publishes `storage_unavailable` when it cannot read saved files. Last-known
values must match the current account, destination, and service.
**Upload sessions to Antiburn Cloud** pauses or resumes uploads to every
destination on this device. With no approved folders, the switch opens folder
setup instead. The switch does not change folder approval or start a retry.
Folder changes while the device is paused do not resume uploads. On upgrade,
an old workspace pause starts the device switch off. The old workspace pause
stays in force until that workspace is reviewed, because it can also mean the
saved source choices changed. **Save** can clear a source hold after Desktop
checks the selected folders and verifies that all kept source bindings are still
allowed. Restore access or remove folders with unavailable sources before saving.
**Retry saved uploads** is separate. It does not
resume the device switch, clear a source-change pause, or expand the selection.
It retries only the displayed workspace and saved folder rules. If those rules
change in another window, reload them before retrying.
**Backfill sessions** asks Desktop to offer all available sessions in
the named workspace's saved folders again, including unchanged sessions it
previously uploaded. Cloud can reuse content for the same source. Desktop keeps
deletion markers, existing jobs and bytes, and upload retry delays. Backfill
does not turn on uploads, save draft choices, or retry failed jobs. Starting a
scan does not confirm any uploads. Only an explicit backfill re-offers unchanged
sessions; normal sync still skips them.
Backfill starts at the first retained source key and scans in bounded pages.
The desktop activity date range does not limit this scan. It cannot recover
history the coding tool removed, use files outside approved source roots, or
read sources that exceed the provider limits. Settings shows when the scan is
active. The scan can finish before Cloud accepts every upload.
If Desktop restarts before the scan finishes, request backfill again.
`recoveryRequiredCount` identifies missing or incomplete local captures that
cannot resume from their saved state. These count as interrupted, not queued.
They do not block healthy sources. When only these failures remain, Settings
shows a local-recovery message without a Retry button that cannot fix them.
Pause stops new capture and upload work and saves the device setting across
restarts. It does not sign out, discard retry bytes, undo requests already sent,
or delete remote content. Sign-out and account changes also stop unauthorized
work without retargeting or deleting saved captures.
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
apply. Desktop probes a saved upload at most three times after repeated
conflicts, with at least 60 seconds between probes. A successful step, a new
sign-in, an app restart, or explicit Retry resets that limit. Empty interrupted
captures do not gain permission to read source through this recovery path.

After sign-in, process restart, or explicit Retry, the scheduler checks saved
finalization status. If integrity is still pending, it can replay the exact
saved finalization once. This can restart stopped processing without new source
reads or a new capture. If both status and renewal conflict, Desktop can replay
the exact saved finalization once for a capture with saved finalization intent.
Temporary finalization failures use the service's retry delay and a separate
three-attempt limit. During backfill, Desktop gives discovery a turn between
pending integrity checks even if the next poll is already due. A failed Team
membership lookup keeps the current scan position
and retry limits but does not let the device upload until it verifies access.
It keeps the saved bytes until the service confirms integrity acceptance.

Time expiry is recoverable, including after a weekend offline. The service
rechecks current authority and the saved request before renewing storage work.
Expired incomplete parts can require transfer again from the same local bytes.
This never grants permission to read source again, change the capture, or
overwrite a newer publication. Deletion and cancellation remain final.

Capture API requests use `/v1/captures` on the configured resource origin
and use credentials held by Desktop.
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
The scheduler can finish the saved decision after a crash without reading source.
Missing entries cannot start recovery or authorize recapture. They can only
finish a previously saved empty-recovery decision. Partial captures and
post-reservation interruptions keep their IDs and bytes; they are not replaced
with newer source. Missing or unreadable saved state is not permission to discard
retry files.

Older jobs with no saved issuer cannot resume. They keep their IDs and spool
files but do not use active job slots. Approve the folder again with the current
issuer to allow a new capture. A job with a different known issuer can resume
only under its original binding. It still holds its slot and blocks a second
job for the same source. Retained files still count against local spool limits.

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

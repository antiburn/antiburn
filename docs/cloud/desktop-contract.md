# Cloud desktop contract

Antiburn Cloud is waitlisted. [Sign up for the waiting list](https://antiburn.com/cloud/).
Standard public builds keep Cloud disabled. This contract covers opt-in
Cloud-enabled development builds, not production availability.

Development builds use a browser-approved device connection, not a general
OAuth or OpenID Connect service. Builds without the `cloud` feature omit Cloud
account commands, account permissions, the callback scheme, and account controls.
Their Cloud onboarding step links to the public waitlist. Local sessions and
Checks do not need Cloud.

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
independently of onboarding or Settings. Discovery does not start it.

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
Settings, folder consent, auth state, and retry files remain
separate from the release app. Source selections can be reused within one profile;
this does not move credentials between service origins.

Desktop reads `AppInfo.cloudEnabled` once per onboarding window. Earlier steps
remain available during the lookup. After Checks or when resuming a saved Cloud
step, Desktop waits up to five seconds if the lookup is still pending.
A true result makes the Cloud teaching and setup screens available.
A false result, a failure, or a timeout shows the informational screen. Late
replies do not change that screen. **Learn more** opens the public Cloud page in
the system browser. **Done** opens Sessions. Service configuration, offline
errors, and unsupported capture platforms do not turn the build feature off.
Cloud setup always permits local completion without source or content consent.

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
debug assertions defaults both origins to `https://api.antiburn.com` when
neither variable is set. Set both variables to override that default with
canonical HTTPS origins. The resource origin owns the device API routes.

For the **Open Cloud dashboard** shortcut in Settings, set the build-time
`ANTIBURN_CLOUD_DASHBOARD` to the dashboard's canonical origin without a
trailing slash. It must use HTTPS, or loopback HTTP in a build with debug
assertions.
**Open Cloud dashboard** opens the root of this origin in the system browser.
An invalid or missing dashboard origin hides this shortcut. It does not
change device approval or **Finish setup in browser**. Desktop opens setup
through the configured resource origin.

The explicit feature command also works for a debug build:

```sh
cd apps/desktop
pnpm tauri dev --features cloud --config src-tauri/tauri.debug.conf.json
```

Enabling Cloud does not grant access. The service still controls sign-in.
Capture and upload require the current service capabilities and explicit
ongoing content consent.

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
workspace. Reset permits a new connection without waiting for remote
revocation. An unresolved team approval does not block Personal sign-in. An
explicit browser denial does not need a reset.

The browser must show the account, permitted actions, and device before the user
approves the connection. The approved scope covers the first-party device
connection only. It does not grant general API-client or third-party authorization.
Token responses bind opaque credentials to an account grant, user,
fixed grant deadline, scope, resource, and issuer. The session
response supplies those same bindings, a nullable display name,
the device label, and the access deadline. Deadlines use RFC 3339 strings.
Desktop checks each action in the space-delimited `scope` field:

- `capture:write` permits session uploads to workspaces that allow uploads.
- `sessions:list` permits session search and metadata lists.
- `sessions:read` permits reading complete session documents.

The service still checks current workspace access. Unknown action names do
not enable or disable any known action. Desktop checks that the action set,
including any unknown actions, stays the same across refresh.

Desktop requires the token and session bindings to match before it saves a
refresh credential. A changed grant, account, scope, or fixed deadline
cannot restore the saved connection. To add missing actions, choose
**Update device access**. Desktop asks the same account to approve its current
actions and the missing actions in the browser. An access update cannot overlap
another browser approval. Once the approval expires, a new attempt can start.
You can still cancel an access update if Desktop loses the current session.
Desktop keeps the current grant until it verifies and securely saves the
replacement. It records unconfirmed
revocation with the replacement epoch, then tries to revoke the old grant
before returning from the callback. That request can take up to 15 seconds.
It leaves the detached logout slot free for sign-out. Both proofs share one
keyring entry until Desktop replaces or deletes the entry, for example during
credential rotation or sign-out. Desktop does not queue a revocation retry.
After a confirmed revoke, Desktop restores the previous cleanup state for the
same epoch without clearing earlier warnings. If that write fails, Desktop
keeps revocation uncertainty.
If Desktop cannot confirm rollback to the old epoch, it leaves both proofs
in place. A later refresh selects the proof that matches the active epoch.

Denial or cancellation does not revoke the old grant. If Desktop abandons
an update after the browser may have opened, it records revocation uncertainty
without blocking the current grant. This also applies if a verified callback
fails before Desktop has proof it can use to revoke the replacement. Explicit
denial and confirmed launch failure do not add uncertainty. The update does not
change folder settings.

Device authentication requests use the configured API origin:

| Request | Proof |
| --- | --- |
| `GET /devices/authorize/account` | `redirect_uri`, `state`, `code_challenge`, `code_challenge_method=S256`, and the display label `device_name`. For an access update, also send the requested `scope` and current `account_id`. |
| `POST /v1/devices/token` | JSON `{grant_type:"authorization_code",code,code_verifier}` or `{grant_type:"refresh_token",refresh_token}` |
| `GET /v1/devices/grants/current` | Access bearer |
| `GET /v1/captures/destinations` | Access bearer |
| `POST /v1/devices/grants/current/revoke` | JSON `{refresh_token}` |

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
If workspace discovery is temporarily unavailable, Desktop retries while the
folder pane is open. In Settings, choose **Check again** if Desktop could not
read the account or verify its Personal workspace. This does not enable upload.
Desktop checks for current destinations when the user selects **Continue**
after sign-in. A signed-in user can also select **Refresh workspaces** in
**Settings > Sessions**. Both actions use the same workspace list request.
They do not issue a grant or change consent.
A workspace list response of `401` clears cached access and asks the user to
sign in again. It does not revoke the saved grant. A `403` tells the user that
workspace access is denied without changing the connection. Neither response
grants upload permission.

One account approval covers Personal and current or future Teams where the
account can upload. Approval does not turn on uploads or approve a folder. Each
capture names its exact workspace and sends the saved membership generation for
Team or `null` for Personal. A missing or stale Team permission does not fall
back to Personal. An old workspace grant does not become an account grant.
Sign in again to use the account flow. Old refresh proof can only sign out the
grant it belongs to.

### Browser workspace setup

After the loopback listener validates an initial approval callback, the
browser can select **Continue setup in browser**. Desktop exchanges the
approval code without waiting for this action. If the browser page is closed,
**Finish setup in browser** opens the same route after sign-in. The action is
available in onboarding and in **Settings > Sessions**.

Both actions open `GET /v1/devices/setup` on the configured resource origin.
The service redirects the browser to `/desktop/setup` on its dashboard. The
loopback page does not send its URL as the referrer. Neither action sends
device proof, an email address, an invite token, or a redirect parameter.

The dashboard verifies the work email and offers a matching workspace invite.
Device approval does not wait for email verification or workspace admission.
Desktop does not accept a browser assertion about membership. After the user
returns and selects **Continue**, Desktop checks the current destinations with
its existing device grant before it shows folder choices. This flow does not
approve folders or turn on sync.

## Credential and logout safety

### Read sessions from the CLI

On macOS and Linux, a Cloud-enabled CLI can reuse an active, read-capable
device connection from a running Antiburn Desktop app. The app owns sign-in
and refresh. Desktop requests a read-only token with `POST
/v1/devices/grants/current/read-token`. It sends the current device bearer and
no body, cookies, or `Origin` header. The response is `{access_token}`.
The read token cannot upload or claim membership. It expires with the original
access token, within 15 minutes, and has no refresh token.

Desktop gives the CLI only the read token over a private local socket that
checks the OS user. If Desktop cannot get a read token, it does not pass the
device bearer instead. Desktop does not store the read token or give the CLI a
refresh credential. The socket does not isolate processes that run as the
same OS user.

The CLI starts with the Personal workspace. It can request another workspace
by ID. The service checks current workspace and session permissions on every
search and document read. It uses the workspace session API, not a device-only
or CLI-only endpoint. A connection approved only for upload cannot serve
CLI reads. In Antiburn Desktop, choose **Update device access** to approve
session reads without signing out.

| Request | Device scope | Response |
| --- | --- | --- |
| `GET /v1/workspaces/{workspace_id}/sessions` with `q` and `page[size]` | `sessions:list` | Session metadata with `items` and `page.next_cursor` |
| `GET /v1/workspaces/{workspace_id}/sessions/{session_id}/document` | `sessions:read` | One projected document with `schema_version`, `source`, `workspace_id`, `session_id`, `revision`, and `stale` |

Send the access token as `Authorization: Bearer <token>`. A document read
returns one publication without a cursor. The Cloud response body must fit
within 20 MiB, including the envelope and JSON escaping. The server returns
an error for a document that exceeds the limit. An older publication can be
returned with `stale: true`.

### Device credentials

Opaque access tokens expire after at most 15 minutes. The app and CLI keep
them in memory. Only the OS keyring stores rotating refresh
tokens. There is no plaintext or database fallback. The connection has a fixed
one-year deadline. Refresh does not extend that deadline.

A single Cloud blocking-work permit admits synchronous keyring, SQLite, source,
state-file, and short HTTP operations before they enter Tokio's blocking pool.
Ordinary requests fail clearly as busy without starting work. Cleanup waits
asynchronously for the permit. Sign-out and browser cancellation invalidate
their account or browser generation before waiting. A local reset invalidates
late work only after it succeeds. A busy connection attempt
does not invalidate the pending browser attempt. An admitted connection attempt
cannot replace a newer cancellation while it waits for a blocking worker.
An awaiting caller cannot release a permit still owned by running work.
Cloud table initialization runs lazily in that worker before table access, not
during UI startup. Failed initialization remains retryable and does not hide
credential cleanup.

Loopback accept, read, write, cancellation polling, and expiry use Tokio async I/O
outside the controller mutex and blocking-work permit. The existing HTTP transport
remains synchronous with a 15-second timeout per request. Normal requests use
admitted blocking work, including access update revocation. Best-effort logout
for sign-out and older Team slots uses a separate blocking worker.

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
before updating local state. These cleanup credentials cannot restore a
session. Best-effort logout can try both proofs without waiting on the main
Cloud worker. A failed secure-storage write leaves remote revocation unconfirmed.
Decoded responses retain bounded logout proof even when token bounds or authority
are invalid. An unreadable authentication state keeps remote uncertainty; cleanup
of a newer connection cannot prove that an older connection was revoked.

Settings keeps the connection action available after onboarding. Desktop sign-out
first flushes a new signed-out generation. Browser cancellation changes only the
pending browser-attempt generation; it does not cancel an established connection
or its refresh. Desktop closes local access and tries to remove the keyring
credential before returning from sign-out. It does not wait for Cloud to revoke
the old grant. A deletion failure leaves the closed marker in place and allows
Desktop to retry local deletion. Reconnect can proceed once local cleanup
finishes.

At most one detached logout runs at a time for the account and old Team slots.
An occupied slot skips the new attempt. Desktop does not queue it for later.
The worker sends old proofs only to their original service and cannot change
credentials or account state. If Desktop cannot confirm remote revocation,
the old grant can expire at its existing deadline. Settings does not show a
warning about earlier attempts that no longer block this device.
Status reads can rotate credentials or close access. Desktop state updates reach
every account surface, including before the requesting renderer receives its
reply. Renderers attach their listener before reading and clear content consent
when authority changes.

Settings still offers local reset for older cleanup records with no refresh
credential. A reset does not revoke the old connection, remove retry bytes, or
accept late browser callbacks.

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
the exact saved folders. **Sync now** approves that list and any folders you chose
for the same Team in one update. It checks the current policy revision and
validates the new choices. It does not unlock old jobs, change their saved bytes,
or resume a paused device.

If a session matches enabled folders in more than one workspace, Desktop
uploads the full session independently to each matching workspace where consent
and membership are current. Each upload has its own destination and capture ID.
An unavailable workspace does not block another eligible destination. Existing
jobs keep their original destination, capture ID, and retry conditions.

An older capture with no saved issuer cannot retry. An explicit folder approval can
replace folder consent that has no issuer with the current verified scope.
Desktop does not add an issuer to old captures. Eligible sessions can use new
capture IDs. Desktop keeps the old spool bytes and counts them toward its
storage limit. It also keeps accepted receipts and deletion markers for the
same resource, account, and workspace.

Sign-in, source access, and the legacy local **Upload sessions** preference do
not grant upload consent. Use **Start syncing** to approve the first folder
choices. Use **Sync now** to approve later choices in Settings. Onboarding can
approve folders for more than one workspace in one action. Desktop saves each
workspace separately and checks whether it saved the exact folders. A failed
route stays selected for retry. Settings saves one workspace at a time.
To change a verified saved folder's destination, choose the new workspace and
select **Sync now**. Desktop removes the old rule before it approves the new one.
This does not move sessions already in Cloud. The new rule may upload older local
sessions. If Desktop confirms the removal but cannot save the new rule, the old
rule stays off. Choose a workspace to start syncing the folder again.
`desktop-project-sync-v1` binds the account, workspace, exact provider
sources, and approved folders or repositories. It enables full content and
ongoing sync, but not external attachments. It covers existing and new
sessions that match those rules while uploads remain enabled.

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

On macOS and Linux, approval for a Git checkout with a supported fetch
`origin` covers other local checkouts and linked worktrees with the same
origin. Standard HTTPS and SSH forms can match on supported hosting services.
For other endpoints, Desktop keeps the transport and full path, including
any `.git` suffix. A saved origin label that does not prove the full path
cannot grant upload access. Older opt-outs still apply unless you select
the folder again. Desktop does not match a fork through its `upstream` remote.
This is a local Git configuration check, not proof of remote ownership.

Desktop keeps one rule per repository in each workspace. The rule uses only
its approved provider sources. A non-Git folder or a checkout without a
supported origin keeps exact-folder approval. Desktop can upgrade a legacy
rule when the saved path is still the exact root of a verified checkout.
The upgrade uses its current origin because the old rule did not store one.
Desktop keeps the exact rule if an old exclusion or another workspace could
conflict. The upgrade does not resume uploads or override a source exclusion.

The project list uses cached local session metadata from all discovered agents.
It can show projects for agents that cannot upload. By default, folder rows
show indexed local sessions. For an approved row in one verified workspace,
Settings can instead show `N sessions found` from sync discovery. That count
includes only sessions assigned to the row's saved rules. It is not a total or
a count of new uploads. A session without one verified rule does not add to a
row. Settings does not add row counts across workspaces.
Only qualified Claude, OpenCode V2, Codex, Pi,
and [Cursor](cursor.md) sources can produce upload jobs.
A suggested project or checked row does not add support for another provider.

With one eligible workspace, new onboarding choices start selected. With
multiple workspaces, new choices start off. Loading the page or changing an
onboarding row does not save consent or turn on uploads. Existing choices and
saved opt-outs take precedence.
**Start syncing** approves only the selected groups. **Load more folders**
does not approve unseen groups.
Until confirmation, an existing saved policy stays unchanged.

Settings reads more folder pages in the background. It keeps verified choices
usable while later pages load or retry. A later page failure does not replace
verified choices with an error. Discovery does not save or expand folder approval.

After initial consent, Settings shows the saved folder rules. Turning off a
group removes its folder or repository rule immediately. Adding a folder does not grant
permission until you select **Sync now**. Folder approval covers full sessions
for the selected folders and matching Git checkouts. First-time approval checks
current source and recorded folder evidence, just like later additions.
Changes check the current policy revision and do not change other groups,
unseen pages, or the device pause. Turning off the last group does not delete
saved captures.
Settings lists folders with verified local directories. Use **Manage other
saved folders** to turn off a saved rule that does not appear in the folder
list. You do not need access to the original source to turn the rule off.

Confirmation saves the complete `projectChoices` map, including exclusions,
atomically with the upload policy. Status returns the same map for reopen.
These group choices are UI preferences, separate from provider-file preferences.
Only the policy's folder and repository rules grant upload permission. Older policies
without a choice map load an empty map; the UI requires review before changing
their selection.

Matching Git checkouts share one row. Desktop verifies the nearest checkout
for a recorded directory. It does not need a list of every worktree to apply
the saved repository rule. Foreign environment paths are not probed as local
directories.

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

Folder listing uses a separate local worker from upload approval and sync.
It uses the current signed-in account without refreshing the Cloud session.
Settings can show local suggestions while it checks workspace access. A
background refresh does not block already verified choices in the same scope.

Listings can return partial results. Listing stops starting new provider
reads after its 3-second work budget. An in-progress read can take longer.
If Desktop checked some folder paths before deferring the rest, Settings
can check the same page once in the background. These results do not replace
the fresh checks required for approval.
Folder approval checks current evidence and can inspect up to 4,096
candidates across up to 100 source roots. Each provider page holds at most
512 candidates. Desktop gives each root a first
turn, then uses the remaining budget on later pages. Cursor IDE uses its full
assigned share because it does not report how many IDs it inspected. Codex
and Pi count each inspected ID, even when opening the file fails. These pages
use native ID order, not session dates. Before opening any session file, each
Codex or Pi page indexes at most 4,096 directory entries. It descends at most
three levels for Codex and one for Pi. Later folder pages add cached project
suggestions. They do not scan all provider history.
Claude folder checks share a 64 MiB budget for uncached metadata reads. The
first uncached transcript can use up to 128 MiB on its own. Evidence that does
not fit the remaining budget makes the result partial.

A saved policy permits at most 100 source roots and 100 distinct project
directories. It can store up to 512 exact pairs of source and project directory.
Missing access, unsupported schemas, and exceeded bounds do not become a
complete empty listing or permission to upload an excluded source.
Setup failures do not hide account cleanup controls.

Before enabling sync or making a capture, Desktop reads
`GET /v1/captures/capabilities`. It sends only formats in the response: `ClaudeJsonl`,
`OpenCodeSqliteV2`, `CodexJsonl`, `PiJsonl`, `CursorCliAgentJsonl`, or
`CursorIdeComposer`. The source limit is at most 128 MiB per transcript or
expanded native-row stream. A lower advertised limit
can apply. Missing support blocks new capture without truncating or dropping
saved bytes.

### Background sync and saved bytes

**Start syncing** moves onboarding to completion without waiting for capture,
transfer, or integrity acceptance. **Done** remains available while setup or
uploads are pending. Closing onboarding stops renderer observation, not the
saved ongoing policy or desktop scheduler. Local tools setup remains
independent of Cloud sync.

Settings → Sessions shows the account, verified workspace count, device upload
switch, folder choices, and a short sync status.
Folder-read and saved-upload failures use brief messages, with recovery actions
when available. Known blocked or interrupted work remains visible while other
uploads run or the policy is paused.
Errors use an inline alert rather than muted routine status. Jobs excluded by the
current folder policy retain their bytes but do not count as current upload failures.

While the first account and sync-status reads are pending, Settings shows skeletons
for the content that depends on those reads. It announces one accessible
`Loading Cloud settings` status. Missing results do not imply a signed-out account,
switch values, folder approval, or zero counters. Verified account and folder content
stays visible during background refreshes in the same account, destination, and
service scope. A read failure ends loading and shows a recovery action instead of
an indefinite skeleton.

`storageKnown` is true only when saved-state reads succeed. When false, status
counters and progress are unknown or last-known values, not current results. The desktop
still publishes `storage_unavailable` when it cannot read saved files. Last-known
values must match the current account, destination, and service.
**Upload sessions to Antiburn Cloud** pauses or resumes uploads to every
destination on this device. Before the first folder approval, Settings shows the
folder choices without a device switch. The switch does not change folder
approval or start a retry.
Folder changes while the device is paused do not resume uploads. On upgrade,
an old workspace pause starts the device switch off. The old workspace pause
stays in force until that workspace is reviewed, because it can also mean the
saved source choices changed. **Sync now** can clear a source hold after Desktop
checks the selected folders and verifies that all kept source bindings are still
allowed. Restore access or remove folders with unavailable sources before saving.
**Retry saved uploads** is separate. It does not
resume the device switch, clear a source-change pause, or expand the selection.
It retries only the displayed workspace and saved folder rules. If those rules
change in another window, reload them before retrying.
**Start syncing** approves the first folder choices in Settings. **Sync now**
applies pending folder changes. Both request a full scan of eligible saved
folders after approval succeeds.
The scan skips sessions when the accepted receipt matches their current version.
Desktop keeps deletion markers, existing jobs and bytes, and upload retry delays.
The scan does not resume paused uploads or retry failed jobs. Starting a scan
does not confirm any uploads. While Desktop runs, scheduled sync continues with
Settings closed. It skips matching receipts, reads sources in bounded pages,
and uses one blocking Cloud worker. When it reaches the last discovery page, it
sets the next discovery time 60 seconds ahead. After an
upgrade, an unchanged Claude, Codex, or Pi file can be offered again if its
receipt uses the old observation format. That receipt does not prove a match with
`file-generation-v1`, so the re-offer can require more capture and transfer work.
Desktop lets one scan request wait up to five seconds for its Cloud worker.
If the worker stays busy, the request does not start. Try again when the
worker is free. Before scheduling, Desktop checks the account, upload controls,
and saved folder policy again.
The full scan starts at the first retained source key and reads bounded pages.
The desktop activity date range does not limit this scan. When the Claude
metadata read budget or OpenCode content check budget is reached, the scan
continues from the first unread source on a later turn. An unavailable source
root or approved folder does not stop work in other verified folders. The
scan reports a gap.
The scan cannot recover history the coding tool removed, use files outside
approved source roots, or read sources that exceed the provider limits.
Settings shows when the scan is active. The scan can finish before Cloud
accepts every upload.
If Desktop restarts before the scan finishes, select **Sync now** again.
Each destination can report `uploadProgress` for its last explicit scan in
the current Desktop run and sign-in. `uploaded` counts Cloud acceptance for
source versions found in that scan. `total` counts sources that match the saved
folders and are not marked as deleted. `total` is null while the scan runs. It
stays null if discovery misses sources or reaches the progress counter limit.
If fresh evidence proves that a source moved outside all approved folders,
Desktop removes it from this count.
A receipt for a different version does not count as uploaded. Desktop checks
the source again. If the refresh cannot find it, the scan reports a gap rather
than assuming that the source moved outside the approved folders.
`scanIncomplete` reports known gaps in the scan. `blocked` counts discovered
sources that could not proceed or whose captured bytes did not match the
scanned version. `unsupported` counts discovered sources whose format Cloud
does not currently accept. A failed capability request does not prove that a
format is unsupported. These two counts exclude versions with matching
acceptance receipts. Neither the counts nor `total - uploaded` show the number
of saved upload jobs.
Settings does not show a percentage without a known nonzero total.
`uploadProgress.transfer` reports required compressed bytes across the captures
in that scan. `sentBytes` counts bytes backed by current service receipts.
Desktop keeps accepted contributions after it removes their spool entries.
Matching acceptance receipts from before the scan add no new transfer bytes.
`totalBytes` stays null until discovery finishes without gaps and every source
without a matching receipt has a known upload plan. Zero is valid for an empty
scan or one whose source versions already have matching acceptance receipts.
The counters exclude reused bytes and do not prove session acceptance.
Repeated status reads do not add the same capture twice. Provisional receipt
expiry can reduce `sentBytes`; Desktop does not keep invalidated credit.
Earlier accepted versions retain their transfer work if the same source changes
during the scan. Its new version needs its own plan or matching receipt before
the total is known again. Proven folder exclusions and deletion markers remove
that source's contributions. The tracker bounds both source and capture counts
to 4,096. Reaching either bound can leave totals unknown. These counters share
the session progress lifetime and its account, policy, and sign-in limits.
Desktop can report `backgroundProgress` for a separate background sweep.
It uses the same version and transfer rules as explicit progress. It does not
reuse the last explicit scan or include other sweeps. `backgroundSyncInProgress`
reports active background work, even before the sweep has measurements.
`fullSyncInProgress` stays true after explicit discovery finishes if uploads
or integrity checks can still advance. `backfillInProgress` reports explicit
discovery only. Waiting for the next sweep does not count as work in progress.
When present, the runtime activity flags take precedence over the general
sync status, which can remain `syncing` after the last acceptance.
Session counts describe accepted source versions. Transfer counts describe
confirmed compressed bytes, not raw network traffic or session acceptance.
Combined workspace counts use `session uploads`, such as
`Syncing 3 of 8 session uploads`, because sending one local session to two
workspaces counts as two deliveries. A combined session or byte total is known
only when every included workspace supplies that measurement with a known total.
A sum of known members is not a complete aggregate when another member is unknown.
Settings does not combine an active full scan with background counters or with
an idle workspace's earlier results.

The same status shows background and explicit sync work. With measurements, it
uses a short line such as `Syncing 2 of 5 sessions (12 MB transferred)`. If the
total is unknown, `sessions found` describes only the sessions found so far.
If discovery has no count, Settings shows the accepted count alone when known.
Unavailable measurements are omitted. Settings does not guess totals,
percentages, or time remaining.
Without measurements, an explicit scan can show `Checking folders`. Transfer
and automatic recovery can show `Syncing sessions`.
When idle, Settings marks retained explicit results as `Last full scan` or
`Folder scan incomplete`. These results include the known totals for sessions
and transferred bytes. When active work has no scan measurements, Settings can
show pending saved uploads.
That count is not a scan total. If exactly one pending upload has a known byte
counter, Settings labels it `Current upload` rather than using it as an
aggregate transfer count.
Without measurements, Settings shows the activity alone.
Errors replace the progress line and offer a supported recovery action. Settings
does not claim `Up to date` without evidence that the current scan scope is complete
and has no pending, blocked, unsupported, or undiscovered work.
The same footer button keeps its location and size. It shows `Syncing` and uses the
native `disabled` attribute while a scan, upload, or automatic recovery is active.
Completion of the command response does not enable it while background work
remains active. It returns to `Sync now` only after that activity ends.
While a scan is active, `found` can report how many matching sessions Desktop
has found so far. It is not a final total. If Desktop can identify one current
upload, `uploadTransfer` reports confirmed compressed bytes and the bytes the
service requires for that upload. It does not count reused bytes or prove that
Cloud accepted the session.
`recoveryRequiredCount` identifies missing or incomplete local captures that
cannot resume from their saved state. These count as interrupted, not queued.
They do not block healthy sources. When only these failures remain, Settings
shows a local-recovery message without a Retry button that cannot fix them.
`automaticRecoveryCount` is a subset of `queuedCount`. It counts saved work
the current scheduler can advance without **Retry saved uploads**. This
includes eligible integrity polls, bounded retries of the same upload,
proved capacity waits, and saved decisions to clear an empty local entry.
Ordinary queued uploads, exhausted conflict probes, incomplete captures,
and missing entries without saved recovery proof do not count. Desktop
omits the count when it cannot verify current scheduling authority or storage,
or when uploads are paused. A recovery count does not confirm acceptance.
Pause stops new capture and upload work and saves the device setting across
restarts. It does not sign out, discard retry bytes, undo requests already sent,
or delete remote content. Sign-out and account changes also stop unauthorized
work without retargeting or deleting saved captures.
Signing in again to the same account and workspace resumes eligible saved uploads
automatically. A new sign-in clears conflict cooldowns but preserves capacity
and offline backoff. It does not replace the capture, change its destination,
expand folder consent, or resume a paused policy.

During background sync, an HTTP 403 from the capture API means Cloud denied
uploads to that workspace. Desktop keeps the account signed in and does not
pause other destinations. After each denial, Desktop waits 60 to 300 seconds
before the next automatic capture request for that destination. A new sign-in
or **Retry saved uploads** clears that wait. An HTTP 401 instead invalidates
the cached access token. A denial does not grant permission to repeat an
incomplete reservation or read source again.

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

Desktop sends capture API requests to the configured resource origin with its
own credentials. See the [capture routes](capture.md#desktop-http-routes).
Saved gzip bodies go only to validated, short-lived upload locations supplied
for the bound request. Those body uploads carry no API bearer or cookies.
Upload locations never enter saved state, renderer status, or diagnostics.
Resume diagnostics record closed states, whether a saved plan exists, request
routes, renewal outcomes, timing, and retry delays. They never contain capture
content, source paths, sign-in proof, signed URLs, or raw provider errors.
Backfill summaries record discovery, saved-job decisions, retry reasons, and
receipt matches at info level. Desktop limits progress summaries to one per
minute and records scan and acceptance outcomes separately. Retry counts
describe attempts, not distinct sources. Summaries do not record source,
folder, or capture IDs.

The scheduler and capture spool serialize bounded work. Pending captures keep
their original account, destination, service, source, capture ID, compressed
bytes, and frozen plan across restarts. Narrowing the approved folder policy
blocks affected saved captures rather than removing part of their content.
For Claude, Codex, Pi, and Cursor CLI, the spool checks the file generation in
the saved job against the files it pins before reservation. A changed file cannot earn a
receipt for the previous version. If the spool proves it stopped before
reservation, Desktop retires the new job or clears its verified empty
capacity-retry entry. Desktop offers the new version on a normal scan.
Saved bytes from older builds can still upload. Their old file observation
does not prove a match with the current file generation.
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

### Retired saved source requests

Desktop no longer opens or runs one-time source requests. SQLite upgrades keep
the old request rows and source scopes unchanged. Desktop does not convert
their consent into folder sync permission. Approve folders in the current
workspace to enable uploads.

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
Run `cargo nextest run --release --features cloud -E 'test(cloud_auth)'` in
`apps/desktop/src-tauri` after building the frontend to check release origins.
Also check the ordinary build with `--no-default-features`. Frontend checks cover
Cloud stores, consent, ordinary-build entry-point exclusion, and searchable
Settings controls. System-browser and OS-keyring acceptance on each supported
platform remains separate from synthetic checks.

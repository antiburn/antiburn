# Cloud desktop contract

Cloud is unreleased. It uses a browser-approved desktop connection, not a general
OAuth or OpenID Connect service. Ordinary builds include no Cloud commands,
capabilities, callback scheme, account controls, or Cloud onboarding step. Local
sessions and Checks do not need Cloud.

## Manual build gate

Cloud requires the explicit, non-default Cargo feature `cloud`. Neither an
environment variable nor the `distribution` feature enables it. Existing
production build commands do not select it.

`ANTIBURN_CLOUD_ISSUER` and `ANTIBURN_CLOUD_RESOURCE` are build-time canonical
HTTPS origins with a trailing slash. Both must match the service's fixed values.
The resource origin owns the four native routes. Without valid configuration,
authentication remains unavailable. Use an approved development service only.

```sh
cd apps/desktop
pnpm tauri dev --features cloud --config src-tauri/tauri.debug.conf.json
```

A manual Cloud build does not permit production use. System-browser and native
credential-storage acceptance remain separate from synthetic checks. Native
source requests can be saved, but the build has no capture reservation or upload
transport.

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
Native validation compares the token and session bindings before saving a
refresh credential. A changed grant, account, destination, or fixed deadline
cannot restore the saved connection.

Native requests use the configured API origin:

| Request                   | Native proof                                                                                                 |
| ------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `GET /desktop/authorize`  | `redirect_uri`, `state`, `code_challenge`, `code_challenge_method=S256`, and the display label `device_name` |
| `POST /v1/desktop/token`  | JSON `{grant_type:"authorization_code",code,code_verifier}` or `{grant_type:"refresh_token",refresh_token}`  |
| `GET /v1/desktop/session` | Access bearer                                                                                                |
| `POST /v1/desktop/logout` | JSON `{refresh_token}`                                                                                       |

Token, session, and logout requests send no cookies or Origin header. Token and
logout proof does not use a bearer header. Native requests do not follow
redirects or retry automatically.

## Credential and logout safety

Opaque access tokens stay in native memory for at most 15 minutes. Only the OS keyring stores rotating refresh
tokens. There is no plaintext or database fallback. The connection has a fixed
one-year deadline. Refresh does not extend that deadline.

A process-wide mutex serializes credential operations. One lifetime OS lock owns
the vault namespace. A second process cannot write credentials until that owner
exits. The vault namespace includes the application identifier and state
directory. Atomic state-file replacement uses a separate stable lock file. State
contains only a random generation, a closed state, and a revocation flag, never a
credential, callback, or verifier.

Before sending a refresh, the client flushes a reauth-required marker. An
uncertain response, process exit, invalid response, keyring failure, or failed
session verification must not retry the predecessor. Successful rotation needs
the same grant ID, account, workspace, resource, scope, issuer, and fixed deadline.
Generation checks before and after storage prevent late results from restoring a
cancelled account.
Rejected refresh responses save the original and returned proofs for logout
before updating local state. These cleanup credentials cannot restore an
authenticated session. Cleanup requires confirmation for both proofs. A failed
secure-storage write keeps an unconfirmed warning and attempts to revoke the
returned proof.

Settings keeps the connection action available after onboarding. Native sign-out
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

Unknown current membership data blocks destination selection. The desktop does
not infer membership from local settings.

The initial connection path supports personal workspaces only. It blocks an
explicit team request instead of silently replacing it with personal. The general
destination policy is personal for zero teams, automatic for one team, and
an explicit account-bound saved team default for several teams. A removed default
blocks capture until the user chooses again. Neither a new account nor a changed
destination can reuse a previous content confirmation or retarget a saved
capture. Changing selected sources clears the content confirmation too. Displayed
count updates do not change that selection or its confirmation.

Settings keeps folder choices separate for each account and personal destination.
This scoped path requires explicit native Claude `projects/<project>` source
directories. It does not discover sources from a repository path, an inferred
working directory, or the current directory. Choose the source directories in
the native picker. Path aliases and unsupported platforms fail without a
fallback. A database read or write failure reports an error instead of treating
setup as saved. Folder-load errors do not hide account cleanup controls.

Login, folder selection, and the legacy local Upload sessions preference do not
grant content consent. The UI requires a separate full-content confirmation:
messages, private thinking, tool calls and results, and session metadata can leave
the device and may contain secrets or file text. Attachment files need separate
consent; this scoped UI does not read or upload them.

`syncEnabled` is false. On supported Unix platforms, `captureEnabled` permits
**Save capture request** after explicit source selection and full-content
confirmation. The native host saves the consent version, account, destination,
and exact root selection before listing source names. The capture library lists
bounded native candidates without reading transcript or companion content. The
host freezes their local capture IDs and scoped source IDs. A source scope stays
stable for the same account and local root when an access grant changes.

These requests report **Upload unavailable**. No native content is captured or
uploaded. There is no automatic future upload promise. A retry keeps the saved
request and source IDs and does not rescan a completed selection. Reconciliation
reads saved request state only. Account changes, logout, and grant replacement
do not retarget requests or delete spools. Explicit request cancellation marks
that selection cancelled; it does not delete remote data or unrelated captures.

One native host mutex and the capture library's spool lock serialize access.
This build starts no uploader. It permits up to 128 saved requests, 100 roots per
request, and 512 selected sources per request. Full storage blocks new requests
without removing old records. Source-request listing is not native ownership,
capture-byte, or service-acceptance proof. The library's synthetic exporter and
spool tests establish their separate byte contract.

HTTP 202, upload completion, and a background operation ID do not prove integrity
acceptance. A later upload integration must persist the accepted source
fingerprint and terminal request in one checked transaction before it
acknowledges the library's accepted receipt. That host path is not enabled here.
Saved compressed retry bytes remain governed by the
[capture contract](capture-v1.md).

## Validation

Native checks use synthetic sessions, in-memory keyrings, temporary state files,
separate test processes, and loopback sockets. They must cover PKCE, callback
guards, generation fences, restart, uncertain refresh, keyring failure, fixed
deadlines, logout, and cross-process locking. Tests must not read user provider
folders or real credentials.

Run native formatting, Clippy, and focused Nextest checks with `--features cloud`.
Also check the ordinary build with `--no-default-features`. Frontend checks cover
Cloud stores, consent, ordinary-build entry-point exclusion, and searchable
Settings controls. System-browser and OS-keyring acceptance on each supported
platform remains separate from synthetic checks.

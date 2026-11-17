# Cloud desktop contract

Cloud is unreleased. It uses a browser-approved desktop connection, not a general
OAuth or OpenID Connect service. Ordinary builds include no Cloud commands,
capabilities, callback scheme, account controls, or Cloud onboarding step. Local
sessions and Checks do not need Cloud.

## Manual build gate

Cloud requires the explicit, non-default Cargo feature `cloud`. Neither an
environment variable nor the `distribution` feature enables it. Existing
production build commands do not select it.

```sh
cd apps/desktop
pnpm tauri dev --features cloud --config src-tauri/tauri.debug.conf.json
```

A manual Cloud build does not establish service compatibility or permit
production use. The browser connection and capture transport need separate
acceptance. Native capture transport stays disabled.

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

The browser must name the workspace, permitted action, and device before the user
approves the connection. The approved scope covers this first-party desktop
client only. It does not grant general API-client or third-party authorization.
The exact public request and response schemas need interoperability acceptance;
provisional client helpers do not establish a supported service contract.

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
the same connection ID, account, workspace, audience, action, and fixed deadline.
Generation checks before and after storage prevent late results from restoring a
cancelled account.

Settings keeps the connection action available after onboarding. Native sign-out
first flushes a new signed-out generation. Browser cancellation changes only the
pending browser-attempt generation; it does not cancel an established connection
or its refresh. Sign-out revokes only this client's current
grant, not every connection on the account. A failed remote response stays visible
as unconfirmed revocation. A keyring deletion failure is also visible, but the
durable local marker still blocks login restoration after a restart. Cleanup-only
keyring records may retry revocation; they cannot restore an authenticated
session. Only a synchronous HTTP 204 confirms remote logout. HTTP 202 does not.

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

Login, folder selection, and the legacy local Upload sessions preference do not
grant content consent. The UI requires a separate full-content confirmation:
messages, private thinking, tool calls and results, and session metadata can leave
the device and may contain secrets or file text. Attachment files need separate
consent; this scoped UI does not read or upload them.

`syncEnabled` is false. Direct `cloud_start_sync` requests reject without reading
or queueing native content. HTTP 202, upload completion, and a background
operation ID do not prove integrity acceptance. Saved compressed retry bytes must
remain until the service confirms integrity for that exact capture and manifest,
or the user explicitly cancels the capture.

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

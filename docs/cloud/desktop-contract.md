# Cloud desktop contract

Cloud is optional. This client ships with **Learn more**, opening
`https://antiburn.com/cloud` as an ordinary external link in the system browser.
Learn more adds no query parameters or callback, creates no pending attempt,
accesses no credential storage, and has no waiting or cancellation state. Authentication and uploads
have separate gates. The desktop includes the handoff, verified identity,
folder draft, auth recovery, and completion screens. Sync runs in the background without a dedicated screen. Test fixtures supply
unavailable service states; they never enable production uploads.

## Authentication release gate

The native build enables authentication only when all these build-time values
are present, valid, and the verified gate is explicitly selected:

| Variable                           | Requirement                                                  |
| ---------------------------------- | ------------------------------------------------------------ |
| `ANTIBURN_CLOUD_AUTH_VERIFIED`     | Exactly `enabled`, only after backend integration acceptance |
| `ANTIBURN_CLOUD_AUTHORIZATION_URL` | HTTPS authorization endpoint                                 |
| `ANTIBURN_CLOUD_TOKEN_URL`         | HTTPS token endpoint                                         |
| `ANTIBURN_CLOUD_PROFILE_URL`       | HTTPS authenticated identity endpoint                        |
| `ANTIBURN_CLOUD_REVOCATION_URL`    | HTTPS refresh-token revocation endpoint                      |
| `ANTIBURN_CLOUD_CLIENT_ID`         | Nonempty public desktop client identifier, at most 256 bytes |
| `ANTIBURN_CLOUD_CALLBACK_URI`      | Exactly the registered scheme followed by `://cloud/auth`    |

Endpoints must share an origin and exclude credentials, fragments and query
strings. Redirects are not followed. Requests time out after 15 seconds;
responses are limited to 64 KiB. No client secret ships in the application.

The production scheme is `antiburn`. A review/debug build uses its application
identifier with periods replaced by hyphens, and its bundle configuration must
register that exact scheme. Checked-in debug and memory-probe configurations
register distinct schemes. Never reuse the production scheme for reviews.
The deep-link crate supplies build-time bundle registration only. Its runtime
plugin is deliberately not installed: it broadcasts raw callback URLs to
renderers. macOS open-URL events and Windows/Linux single-instance command-line
arguments go directly to the native Cloud controller. Early callbacks enter a
bounded native queue until initialization finishes.

The native browser URL requests authorization code flow with PKCE S256, random
32-byte state, and `openid profile offline_access`. The backend brokers GitHub
login and must not request repository or upload permission. The registered
callback carries `state` plus a short-lived, single-use `code`; denial carries
`error=access_denied`. Duplicate query keys, additional fields, wrong routes,
fragments, wrong states, expired attempts, and superseded attempts are rejected.
Attempts expire after 10 minutes and are saved in OS credential storage before
browser launch. These attempts and callbacks apply only to enabled authentication.
Learn more leaves the Cloud screen available for local continuation; neither
returning to the app nor an obsolete marketing callback authenticates an account.

The token endpoint accepts form-encoded `grant_type=authorization_code`, `code`,
`code_verifier`, `client_id`, and `redirect_uri`. Refresh uses
`grant_type=refresh_token`, `refresh_token`, and `client_id`. Successful JSON:

```json
{
  "access_token": "synthetic-access",
  "refresh_token": "synthetic-rotation",
  "expires_in": 600,
  "token_type": "Bearer"
}
```

Access lifetime must be 60–86400 seconds. Each refresh returns a rotated refresh
token; the client saves it before verifying the profile so ordinary offline
errors do not lose the rotation. Profile uses Bearer authentication and returns:

```json
{ "id": "synthetic-account", "login": "example" }
```

Initial sign-in verifies the profile before saving a refresh credential; an offline
profile response cannot leave a resumable, unverified login. Only the authenticated
profile establishes display identity. Successful secure
storage and profile verification are both required for the signed-in screen.
Access tokens stay in native memory. Refresh tokens and pending PKCE material
share one atomic, bundle-scoped OS-vault entry. There is no plaintext fallback.
Requests serialize; cancellation generations prevent late results from restoring
an identity. A cancelled verification clears and revokes its new credentials.
Definitive rejection requires fresh login; ordinary offline failures retain the
refresh credential. Local sign-out deletes the vault entry independently of
remote revocation. A vault deletion failure is shown as a failure and retains
native material for retry; the client still attempts remote revocation.

Revocation receives form-encoded `token`, `token_type_hint=refresh_token`, and
`client_id`. Success is any 2xx. The renderer receives only bounded status,
expiry, display identity and its folder draft. URLs, codes, states, verifiers and
tokens never enter renderer events, local database, logs or analytics. Identity
and folder paths do not enter analytics. The separate onboarding window keeps
the existing durable completion and exact Sessions acknowledgment contract.
Legacy `cloudUnavailable` progress now opens the Cloud handoff.

## Sync gate and consent decisions

Production `syncEnabled` is always false. `cloud_start_sync` rejects requests,
including direct IPC calls. Login, folder selection and the existing device-local
Upload sessions preference do not grant upload consent. Found and newly added folders start
selected, are persisted separately from scan settings, and never enqueue work.
No production upload client or automatic resume is enabled in this change.

Before a sync implementation can be enabled, approve all of:

- Eligible sessions and exact payload fields; summary-only versus transcript content.
- Original working-folder identity, independent of inferred repositories.
- Historical versus future scope and remote-session eligibility. Found folders are selected by default; this preference does not authorize uploads.
- Explicit consent text and confirmation, consent version, and account binding.
- Retention, deletion, disconnect, and cancellation behavior.

The renderer port defines the required operation lifecycle without defining an
upload payload. Capability discovery must return a reviewed consent version and
explicit enabled status. `prepareSync` durably records a random request identity
and consented selection **before** transport submission. `start` sends that
identity as an idempotency key. Acceptance returns an operation identity and
bounded completed/total counters. `reconcile` queries the existing request
identity; it must never create an operation. Lost responses are ambiguous and
must reconcile before any resubmission. Partial and failed outcomes preserve the
selection. Cancellation is confirmed by service result. A reopened pending
request reconciles status only; it never restarts uploads. The production port
rejects prepare/start and supplies unavailable status until this contract is
implemented and reviewed. Controlled fixtures exercise the other outcomes.

## Validation and rollout

Run desktop type checking, scoped ESLint, CloudSession and onboarding unit tests,
then `cloud.spec.ts` and the existing onboarding browser journeys. The visual
harness `surface=cloud` is excluded from the product bundle. Native Cloud tests
use in-memory vaults, controlled transport failures and an ephemeral loopback
server; no production endpoint or real credential is used. Native packaged
cold/warm callbacks and OS vault behavior still need acceptance on each supported
platform before the auth gate is selected. Hosted backend deployment and the consent/upload policy remain outside this
client change.

## Background sync onboarding

Start syncing goes directly from folders to completion. Done remains available while the request is prepared, submitted or reconciled. Partial, failed, ambiguous and cancelled results do not open a progress or recovery screen. Closing onboarding detaches its view; it does not cancel an already requested background handoff. Production transport remains gated and must own durable background work independently of the webview.

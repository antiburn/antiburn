# Live usage degraded state

## The problem

When a live-usage source fails with no cached last-good reading (observed:
Anthropic's `https://api.anthropic.com/api/oauth/usage` returning HTTP 429 on a
cold start), the summary arrives with an empty `providers` list and one entry
in `errors`. `UsageLimitsSection`'s `limited` filter and its expanded listing
both key off `providers`, so the provider's section vanishes with no
explanation. A reader sees that as data loss, not as a passing failure.

The in-session case is already covered: `sources/cooldown.rs` keeps the last
good snapshot across a failed retry, so a 429 mid-session shows stale figures
plus an error. The gap is the cold start, where no last-good exists yet
because that cache is in memory only.

## The design

Keep the provider visible with an explicit unavailable row instead of
vanishing. Two layers:

### Rust: name the provider on the error

`LiveUsageSourceError` carried only `source` ("claude-usage-fetch") and
`category`. The frontend cannot map a source id to a provider without
duplicating registry knowledge. So:

1. `LiveUsageSource` gains `fn provider(&self) -> &'static str` — the
   canonical provider id a source answers for. Required method, so a new
   source cannot forget it.
2. `sources::collect` records failures as a `SourceFailure` carrying the
   source id, the provider, and the error.
3. DTO `LiveUsageSourceError` gains `provider` and `displayName` (via
   `providers::display_name`). Both are `#[serde(default)]` so a snapshot
   cached before the fields existed still loads.

Sources: `anthropic_fetch` → `ANTHROPIC`, `codex_fetch` → `OPENAI`.

### Frontend: derive and render the degraded rows

New helpers in `lib/presentation/liveUsage.ts`:

- `liveUnavailableProviders(summary)`: the errors whose provider shows no
  visible windows in `summary.providers`, deduped by provider — exactly the
  providers that would otherwise vanish. When the provider *is* visible
  (cooldown-cached reading), the existing staleness treatment already covers
  it and no extra row appears, so one failure is never reported twice.
- `liveUnavailableReason(category)`: short label — "rate limited",
  "sign-in needed", "unreadable reply", "unreachable".
- `liveErrorNote(category)`: the full sentence, moved from `UsagePane` so the
  wording lives in one place.

`UsageLimitsSection`:

- The render gate becomes `limited.length === 0 && unavailable.length === 0`.
- The expanded listing gains a subsection per unavailable provider: the
  provider name, the short reason, and the full sentence beneath.

`UsagePane` labels its error rows with the provider's display name and imports
the shared `liveErrorNote`.

## Scope

The popover prototype's `UsageLimitsBar` — the horizontal replacement for
`UsageLimitsSection` — carries the same treatment on
`proto/popover-overview`, where that component lives. It is not part of this
change, which targets only what is on `main`.

## Tests

- Rust `live/tests.rs`: a failed source's error carries its provider id and
  display name.
- `liveUsage.test.ts`: `liveUnavailableProviders` returns the vanished
  provider, dedupes two failures for one provider, skips an error with no
  provider id, and stays empty while the provider has visible windows.
- `UsageLimitsSection.test.tsx`: the section keeps a subsection for a
  provider whose source failed with nothing cached, and still renders nothing
  when there are no providers and no errors.

## Grace period

A provider whose source failed keeps showing its last good reading for a
grace period. The grace period is `LIVE_USAGE_GRACE_MS`, ten minutes. The app
measures the reading's age as `generatedAt - observedAt`. It never reads the
wall clock.

Every surface shows a muted grace note next to the reading. The note names
the failure and says how old the reading is.

Past the grace period, the app treats the provider as the cold-start
unavailable case above. The reading no longer shows.

Three helpers in `lib/presentation/liveUsage.ts` carry this rule:

- `liveProviderStatus(summary, provider)`: reports `live`, `grace`, or
  `failed` for one provider.
- `liveDisplayableProviders(summary)`: the providers to show. It drops every
  `failed` provider.
- `liveGraceNote(category, provider, ageMs)`: the note text.

### Rust: a Keychain read must not look like a sign-out

On macOS, `security find-generic-password` can time out or fail to spawn.
That case must not clear a good reading the way a real sign-out does.
`ClaudeDirectFetch` now tells the Keychain's `Absent` (item not found) apart
from `Unreadable` (a timeout or another failure). Only `Absent` falls back
to the credentials file. `Unreadable` is a real failure, so `Cooldown` keeps
the last good snapshot.

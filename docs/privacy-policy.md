# Privacy policy

Effective: 15 October 2026

This policy explains first-party analytics and optional model-based Burn Check assessment
requests sent by the antiburn desktop application.
Antiburn is operated by **Cadence AI (Vic) Pty Ltd** ("we", "us"). Contact us
at [support@teamcadence.ai](mailto:support@teamcadence.ai).

## What stays on your computer

Antiburn reads coding-agent session files already on your computer and stores
its session index locally. Ordinary session analysis does not upload sessions,
transcripts, prompts, messages, titles, source code, filenames, paths,
repository or branch names, working directories, token counts, or costs.

If you enable **Model-based Checks** in **Settings → Checks**, authored YAML
definitions can review new local session activity through Jev. **Decision model**
exposes Jev only. Requests go to TypeSafe and contain the definition's selected
tool arguments, results, and available user-slot text, not full transcripts.
These fields can contain private work content, source code, paths, or credentials.
Private thinking is excluded. User-slot text can include API, extension, or
synthetic input; it does not prove human approval. Partial history, truncated
text, and missing fields are identified as quality limits. Missing images are
not analyzed. See the [supported subset](burn-check/authoring.md#supported-subset)
and [session coverage](session-coverage.md#burn-check-source-facts).

TypeSafe processes these requests under its own terms, and usage charges can
apply. Provider requests and authentication credentials are separate from
first-party analytics. Credentials use native credential storage or memory,
not the session database. Turning off Model-based Checks stops new discovery
and activation while retaining saved connections and local results. Removing a
credential is a separate action. Past requests cannot be withdrawn from their
recipient.

Built-in Ignored Instructions, Scope Creep, Over-exploring, and Skill Opportunities
execution and historical enrollment are suspended. Their saved results,
preferences, and compatible evidence remain stored. Saved Ollama, Cloudflare,
and Custom profiles and retained provider code do not make these routes
selectable for YAML checks. The native past-session control does not replay
YAML definitions. Developer replay is an explicit, isolated diagnostic.

Antiburn keeps local assessment progress, frozen selected input, answers, and
status provenance for recovery. Compatible accepted answers can be reused without
another provider call. Changed definitions or selected evidence can require new
paid work. A timeout or cancellation after sending can have an unknown delivery
and billing outcome; unknown delivery does not permit automatic resend. An
earlier attempt may have incurred a charge. Request bounds and usage reservations
limit local admission, not the total cost of a long session or repeated revisions.
Settings shows estimated usage, not a spending cap or provider invoice.

Explicit success covers the assessed work, not a repaired or clean whole session.
Current instruction files do not prove historical activation, and current skill
inventory does not prove historical availability or whole-session non-use. Saved
built-in findings retain their original evidence limits; see the
[retained built-in contract](smart-burn-checks.md).

Antiburn keeps bounded local totals for confirmed TypeSafe token usage, calls,
cache reuse, and unknown request outcomes. The totals include the model and price
version used to estimate cost. They do not include session identifiers, request
text, response text, credentials, or paths. The estimated cost is not a provider
invoice. Deleting one session does not change these totals because the usage has
already occurred. Clear Local Data removes the totals and rolling usage records.

If you confirm an Auto Fix, Antiburn can change one existing supported coding
agent setting on your computer. It shows the setting, scope, current value, and
new value before the change. It does not modify a source transcript. Burn Check
attempts, safe display facts, and verified savings stay in the local database.

Antiburn does not require an account. It does not use a third-party analytics,
telemetry, crash-reporting, or session-replay SDK.

If you add a remote host, Antiburn uses your configured SSH connection to copy
supported sessions and companion files into a private cache on this computer.
Analysis happens here. The transfer does not send those files to Antiburn's
operator or an unrelated third party. Removing a host deletes the local copies
and their analysis; it leaves the remote originals unchanged. SSH aliases,
host names, remote paths, and connection diagnostics are not analytics data.

## Analytics we collect

Official release builds send limited first-party events about how the application works,
which features are used, and coarse hourly ranges for the application's own
resource use. This includes application launch and progress through the
first-run steps shown in the Overview.

The event schema contains thirty-one fields:

- the constant product surface `desktop`;
- random message, installation, and analytics-session identifiers;
- the event name and capture and delivery times;
- the processor architecture, operating-system family, and app version;
- an optional count rounded to a range;
- optional labels selected from fixed lists in the application;
- whether a visible state followed a user or automatic exposure, or whether a
  Burn Check watch started passively or from an action;
- a range for Claude's current five-hour usage;
- whether Claude returned reset data, null, or a malformed value;
- Claude's reset eligibility and experiment-membership states;
- an allowlisted reason when Claude reports ineligibility;
- Claude's reset experiment arm and availability state;
- the weekly reset count rounded to zero, one, or two-plus;
- whether Claude supplied a next-reset date, never the date itself;
- a learned session-limit factor's plan, mapped to a fixed list, never the
  provider's own plan string;
- that factor's dollars-per-percent value, reduced to a coarse band;
- how far the factor's estimate and the provider's own meter disagree,
  reduced to a coarse band;
- for one finished usage window, whether a dollars-only estimate landed above
  or below the provider's own meter and by how much, how much of the meter's
  rise no local session could explain, and whether a reading covered the
  window's end, each reduced to a coarse band;
- a nested hourly summary containing fixed bands for antiburn's own shell CPU,
  memory, process read and write I/O, local database size, and database log
  size, plus `none`, `partial`, or `full` coverage for each measurement; and
- up to 16 sanitized unknown transcript record type names, each checked
  against a fixed character set and length before it is sent, with a
  rejected name replaced by a fixed placeholder rather than sent verbatim.

Resource summaries can reveal coarse application work intensity and local data
volume. They contain no work content, paths, credentials, renderer-process
counts, whole-machine measurements, exact byte counts, or exact percentages.
CPU and memory describe the antiburn shell process, not renderer processes or
whole-app totals. Memory means physical footprint on macOS, resident set size on
Linux, and working set size on Windows. Linux I/O can include I/O inherited from
child processes after the shell waits for them. Windows I/O covers all shell-process
I/O transfer bytes, not physical disk traffic. An unavailable measurement is not
reported as zero, and the highest memory band is a sampled maximum rather than a
true peak.

The retained four-check event schema can report the fixed check
category, published finding/clean/abstained/failed outcome, visible finding,
evidence outcome, and prompt preparation/copy outcome. Provider setup and explicit
tests report only `jev`, `ollama`, `cloudflare`, or `custom` and fixed outcomes.
Built-in execution is suspended, so schema acceptance does not prove new
assessment events. Saved-result interactions remain separate. YAML definitions
do not map arbitrary check identities into these events. These events contain
fixed status words only. They contain no endpoint, model,
instruction text,
session excerpt, finding details, prompt, API key, path, or session identifier.
The separate provider assessment request is not a first-party analytics event.

The installation identifier is random and changes every 30 days. The live
analytics-session identifier is generated in memory and changes when the app
restarts, when the installation identifier rotates, or after 30 minutes without
a captured analytics event. Each queued event contains the identifier, so that
serialized value remains in the local analytics queue on disk until the event
is sent or removed. Background events can keep an analytics session active, and
one app run can contain more than one; it does not measure a user visit or time
spent. Neither identifier comes from your hardware, account, name, or email
address.

The complete field list, event catalog, and verification steps are in
[Anonymised analytics](analytics.md).

## First-party analytics network information

The analytics endpoint also stores the IP address and user-agent attached to
the request. These values can reveal your approximate location, network, device
type, and app runtime. We store them with the raw event.

## Why we use analytics

We use these events to understand whether first-run setup works, which product
features are useful, which operations fail, whether Ignored Instructions
findings and prompts are used, when Claude makes its session-limit reset
available, and whether antiburn has resource regressions. We do not use them
for advertising, user profiling, or decisions about a person.

We process this data for our legitimate interest in maintaining and improving
Antiburn. You can object at any time by turning analytics off.

## When analytics starts

Analytics starts automatically in official release builds. Launch and
first-run-step events can be recorded before the first run finishes. The
Overview's own first-run welcome text explains the channel. Settings → Privacy
provides the permanent opt-out.

Default source and development builds exclude the analytics client. A builder
must select the `analytics` Cargo feature and provide an endpoint and operator
name to include it.

## Retention

Raw events, including their request IP address and user-agent, have no automatic
deletion schedule. We retain them until we delete them.

Turning analytics off deletes the installation identifier and unsent events on
your computer. It does not delete events that have already reached us. Contact
us to request deletion of received data. Because Antiburn has no account and the
installation identifier is random, we might not be able to identify which
events belong to you.

## Who receives the data

Cadence AI (Vic) Pty Ltd operates the first-party analytics endpoint. Service
providers that host or maintain our infrastructure can process the data for us
under their service terms. We do not sell analytics data.

Data can be processed outside your country. Local privacy protections can differ
from those in your country. Contact us for current information about the service
providers and processing locations used for this endpoint.

## Your choices and rights

Turn analytics off in **Settings → Privacy**. This takes effect immediately and
clears the local analytics queue and installation identifier. You can also set
`ANTIBURN_ANALYTICS_ENABLED=false` in the application's launch environment.

Depending on your location, you can ask us to access, correct, delete, or
restrict personal information, or object to its processing. You can also
complain to your local privacy regulator. Send requests to
[support@teamcadence.ai](mailto:support@teamcadence.ai).

## Changes

We will update this page when the analytics data or its use changes. A change
that adds data requires a matching update to the closed event schema and public
event catalog before release.

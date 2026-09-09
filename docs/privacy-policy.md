# Privacy policy

Effective: 8 September 2026

This policy explains the analytics sent by the antiburn desktop application.
Antiburn is operated by **Cadence AI (Vic) Pty Ltd** ("we", "us"). Contact us
at [support@teamcadence.ai](mailto:support@teamcadence.ai).

## What stays on your computer

Antiburn reads coding-agent session files already on your computer and analyzes
them locally. It does not upload your sessions, transcripts, prompts, messages,
titles, source code, file contents, filenames, paths, repository or branch
names, working directories, token counts, costs, or credentials.

Antiburn does not require an account. It does not use a third-party analytics,
telemetry, crash-reporting, or session-replay SDK.

## Analytics we collect

Official release builds send limited events about how the application works,
which features are used, and coarse hourly ranges for the application's own
resource use. This includes application launch and progress through the fixed
onboarding steps.

The event schema contains twenty-seven fields:

- the constant product surface `desktop`;
- random message, installation, and analytics-session identifiers;
- the event name and capture and delivery times;
- the processor architecture, operating-system family, and app version;
- an optional count rounded to a range;
- optional labels selected from fixed lists in the application;
- whether a visible state followed a user or automatic exposure;
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
  reduced to a coarse band; and
- a nested hourly summary containing fixed bands for antiburn's own shell CPU,
  memory, process read and write I/O, local database size, and database log
  size, plus `none`, `partial`, or `full` coverage for each measurement.

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

## Network information

The analytics endpoint also stores the IP address and user-agent attached to
the request. These values can reveal your approximate location, network, device
type, and app runtime. We store them with the raw event.

## Why we use analytics

We use these events to understand whether onboarding works, which product
features are useful, which operations fail, when Claude makes its session
limit-reset feature available, and whether antiburn has resource regressions.
We do not use them for
advertising, user profiling, or decisions about a person.

We process this data for our legitimate interest in maintaining and improving
Antiburn. You can object at any time by turning analytics off.

## When analytics starts

Analytics starts automatically in official release builds. Launch and
onboarding-step events can be recorded before onboarding is complete. The Ready
screen explains the channel. Settings → Privacy provides the permanent opt-out.

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

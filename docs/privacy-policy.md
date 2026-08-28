# Privacy policy

Effective: 28 August 2026

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

Official release builds send limited events about how the application works and
which features are used. This includes application launch and progress through
the fixed onboarding steps.

Each event contains thirteen fields:

- the constant product surface `desktop`;
- random message, installation, and application-run identifiers;
- the event name and capture and delivery times;
- the processor architecture, operating-system family, and app version;
- an optional count rounded to a range; and
- optional labels selected from a fixed list in the application.

The installation identifier is random and changes every 30 days. The run
identifier exists only in memory and changes when the app restarts or after 30
minutes of inactivity. Neither identifier comes from your hardware, account,
name, or email address.

The complete field list, event catalog, and verification steps are in
[Anonymised analytics](analytics.md).

## Network information

The analytics endpoint also stores the IP address and user-agent attached to
the request. These values can reveal your approximate location, network, device
type, and app runtime. We store them with the raw event.

## Why we use analytics

We use these events to understand whether onboarding works, which product
features are useful, and which operations fail. We do not use them for
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

# Privacy policy

> **Status: draft. Not in force, and not yet linked from the application.**
>
> Every section marked **`[TO BE COMPLETED]`** describes a decision that
> belongs to whoever operates the analytics endpoint, not to the application,
> and cannot be answered from this repository. This file exists so that those
> questions are written down in one place rather than discovered after a
> reader asks one of them.
>
> Nothing in the app links here until the placeholders are resolved. Publishing
> a policy with gaps in it is worse than having none, because a gap reads as an
> answer.

## What this policy covers

antiburn is a desktop application that reads coding-agent session files already
on your machine and analyses them locally. That analysis — sessions,
transcripts, prompts, file paths, repository names, token counts, costs — stays
on your machine. It is not sent anywhere, and this policy has nothing to say
about it, because there is no processing by anyone but you.

This policy covers one thing only: the anonymised usage events antiburn sends
when the analytics control is switched on.

For the complete, field-by-field account of what those events contain — and the
commands you can run to verify it yourself — see
[docs/usage-analytics.md](usage-analytics.md). The list there is the whole list,
enforced by a closed type in the source rather than by a promise in a document.

## What is sent, in summary

Each event carries thirteen fields: a constant naming the surface class, a
random per-event id, a random installation identifier, a random identifier for
one run of the app, the event name, when it happened, when it was delivered,
your processor architecture, a count rounded into a range where the event has
one, a short label from a fixed vocabulary, a second such label where an event
has two things worth telling apart, the application version, and your
operating-system family.

Two identifiers appear, and they are not equally durable:

- **The installation identifier** is random, is not derived from anything about
  your machine, and is replaced every 30 days. Switching the control off
  deletes it.
- **The run identifier** exists only in memory. Quitting antiburn ends it, and
  it is replaced after 30 minutes of inactivity. It cannot connect one run of
  the application to another.

Neither is derived from your name, your email address, your account with anyone,
or any hardware or network identifier. antiburn has no account system, so there
is nothing for these to be joined to on our side.

## Legal basis and consent

The control is presented on the first-run **Ready** screen, already switched on,
before any event is sent. Nothing is transmitted until first-run setup
completes, so switching it off there means no event ever leaves your machine.
Settings → Privacy carries the permanent switch.

In the **EU, the EEA, and the UK** the control starts switched **off**: analytics
there are something you opt into rather than out of. antiburn determines this
from the locale and time zone your machine already reports. Nothing is looked
up, and neither the locale nor the time zone is ever transmitted.

An installation that completed first-run setup before this feature existed
defaults to **off**, because it was set up under copy stating that analytics did
not exist.

**`[TO BE COMPLETED]`** — The lawful basis relied on outside the EU/EEA/UK, and
confirmation that the EU/EEA/UK default-off behaviour is the basis relied on
there. This needs a lawyer's sign-off rather than an engineer's reasoning.

## Who receives these events

**`[TO BE COMPLETED]`** — The legal entity operating the endpoint, its
registered address, and the name shown to readers in the Privacy pane. The
application displays this name from a value injected at build time; it is not
present in the public source.

## How long events are kept

**`[TO BE COMPLETED]`** — The retention period for raw events, whether they are
aggregated and the raw rows discarded after some interval, and what happens to
data already collected if the endpoint is retired.

The application cannot make any statement about this. It knows only what it
sent.

## IP addresses

**`[TO BE COMPLETED]`** — Whether the receiving server records the IP address
that every internet request necessarily carries, and if so for how long and for
what purpose.

This is the single most consequential unanswered question in this document. An
IP address recorded alongside an installation identifier is capable of making
otherwise-anonymous events identifiable, which would change how this data must
be described and handled. It is stated plainly here rather than left implicit.

## Processors and sub-processors

**`[TO BE COMPLETED]`** — The hosting provider, any analytics or data-warehouse
service the events are forwarded to, and any other third party with access.

## International transfers

**`[TO BE COMPLETED]`** — Where the receiving infrastructure is located, and the
transfer mechanism relied on for readers in the EU, the EEA, and the UK.

## Your rights

**`[TO BE COMPLETED]`** — How to make an access, erasure, or objection request,
the response time, and the supervisory authority a reader may complain to.

One practical note that is settled and worth stating regardless of how the above
is answered: because the events carry no account, no email address, and no
stable identifier tied to a person, it may not be possible to locate a
particular reader's events in order to act on such a request. That limitation
should be stated honestly in the completed version rather than promising a
capability that does not exist.

## Turning it off

Settings → Privacy, the **Anonymised analytics** switch.

Switching it off is a withdrawal rather than a pause. Anything still queued on
your machine is discarded and the installation identifier is destroyed, so
switching it back on later starts a new identifier that cannot be linked to the
old one. You can confirm both by reading the application's own database
directly; the commands are in
[docs/usage-analytics.md](usage-analytics.md#verifying-this-yourself).

Every build made from a clean checkout of this repository has no endpoint
configured and transmits nothing at all, whatever the switch says.

## Changes to this policy

**`[TO BE COMPLETED]`** — How readers are notified of a material change, and
whether a change that widens what is collected re-prompts for consent.

## Contact

**`[TO BE COMPLETED]`** — The contact address for privacy enquiries, and the
data-protection representative in the EU and the UK if one is required.

<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# OS notification gating

How antiburn decides not to show an automated notification while the operating
system says interruptions are unwelcome — Focus and Do Not Disturb on macOS,
fullscreen and presentation states on Windows, Do Not Disturb on GNOME and
Plasma. Written for whoever next touches notification policy, because the two
rules that matter most — *drop, never queue* and *fail open* — are invisible in
any single function signature.

The policy home is `apps/desktop/src-tauri/src/notifications.rs`; the OS probes
live in the `antiburn-nudge` crate's gate module
(`apps/desktop/src-tauri/crates/nudge/src/gate.rs`).

## The problem this exists to solve

antiburn's notification window is its own — not the system notification center —
so Focus and Do Not Disturb do not apply to it automatically. Without a gate, a
disk warning or a usage milestone can float over a presentation, a screen share,
or a Focus session the reader set up precisely to avoid being interrupted.

The obvious fix — defer suppressed notifications and release them when Focus
ends — creates a second problem: a pile of stale notifications arriving the
moment the reader turns Focus off, describing moments that have passed.

So the two rules the whole design serves:

1. **The OS gets the last word, and a suppressed notification is dropped.** The
   gate runs last, after every preference and once-per-run claim has already
   passed, so the trigger's bookkeeping stays consumed. Ending Focus releases
   nothing.
2. **Unknown state fails open.** A missing desktop service, a denied
   authorization, or an API failure must never silently disable notifications
   forever. When the OS cannot answer, antiburn delivers, and logs the fact at
   most three times per run.

## Who is gated

Automated kinds — update available, scan failure, low disk space, usage
milestone — ask the gate. Preview kinds bypass it for the same reason they
bypass the master notification switch: they are the direct result of a button
the reader pressed a moment earlier. That is the settings pane's test
notification, the debug-only sample row, and the one-shot first-run
menu-bar pointer.

Consequences of drop semantics, per kind:

- **Low disk space** — the episode is consumed. The warning returns only after
  free space recovers past the re-arm threshold and then crosses below the
  limit again (`disk_monitor.rs` hysteresis).
- **Usage milestone** — the crossing stays recorded as delivered. The next
  milestone step notifies as usual.
- **Scan failure** — the once-per-run claim stays spent; the next run of the
  app reports again if scans still fail.
- **Update available** — the once-per-version claim stays spent; a *newer*
  version still notifies.

## Platform matrix

| Platform | Suppresses when | Fails open when |
|---|---|---|
| macOS 13+ | `INFocusStatusCenter` is authorized and reports that Focus applies to antiburn | Authorization denied, restricted, or not determined; a `nil` Focus value; an unbundled dev binary (see below) |
| Windows | `SHQueryUserNotificationState` reports anything other than `QUNS_ACCEPTS_NOTIFICATIONS`, or the `NOC_GLOBAL_SETTING_TOASTS_ENABLED` registry value is exactly zero | Missing registry value; API failure. Windows 11's own DND switch is not exposed by the supported API and is not detected |
| GNOME | `org.gnome.desktop.notifications show-banners` is false | Missing schema or key |
| Plasma | `org.freedesktop.Notifications` reports `Inhibited` = true | Missing service, malformed reply, or a 150 ms timeout on the bounded, no-auto-start D-Bus call |
| Other desktops | never | always |

If the reader adds antiburn to a Focus's Allowed Apps list, macOS reports that
Focus does not apply to antiburn, and notifications behave as if Focus were
off. That is the OS's answer, not a gap.

## Idle cost: none

Every probe runs on demand, immediately before one automated notification would
show, and retains nothing afterward. There is no polling loop, no Focus-change
observer, no cached D-Bus connection, and no worker thread at idle (the Plasma
probe's bounded worker exists only for the duration of one query, at most one
in flight). Do not "improve" this into a subscription model: antiburn idles in
the menu bar, and its resting footprint must not grow.

## Authorization (macOS)

Reading the Focus status needs two authorizations — User Notifications, then
Focus status — and the second prompt only means something after the first.
antiburn requests them once per process, chained so macOS never shows two
sheets at once, and only when setup is complete and the master notification
preference is on. First runs never see the prompt over the onboarding window;
`apply_settings_transition` asks again when onboarding finishes or the master
switch turns on, and the gate's once-flag makes repeat calls free.

A denied prompt is a fail-open state, not an error: notifications keep
working, they just stop yielding to Focus. The reader can change their mind in
System Settings → Focus.

## The signed-bundle caveat

Focus status is only available to a bundle with a stable identity, the
communication-notifications entitlement (`Entitlements.plist`, wired through
`bundle.macOS.entitlements`), and a signature macOS recognizes. Two
consequences:

- The unbundled binary `tauri dev` produces cannot even ask: UserNotifications
  raises an Objective-C exception (which Rust cannot catch) for unbundled
  callers, so the gate detects the layout and skips the query entirely.
- A local ad-hoc-signed `.app` may show the permission dialog, but its answers
  are not reliable evidence. **Test Focus behavior only with the signed CI
  release artifact**, and use an automated trigger (low disk, a milestone) —
  the test button bypasses the gate by design and proves nothing about
  suppression.

## Manual test recipe (signed build)

1. Enable notifications, complete onboarding, and accept both authorization
   prompts. Confirm the prompts appear once and never again.
2. Turn on Do Not Disturb. Fire an automated trigger (lower the disk threshold
   above current free space, or wait for a milestone). Nothing appears, and
   nothing appears later when DND ends.
3. Press the test button during DND. The test notification appears — previews
   bypass the gate.
4. Add antiburn to the Focus's Allowed Apps. Automated notifications appear
   during that Focus.
5. Deny the Focus authorization on a fresh install. Automated notifications
   keep working (fail open).

# Main-window navigation and local search

The retained main renderer owns a bounded history of 100 destinations. A destination is
Overview, Limits, Sessions with its contextual filters and optional session identity, or Checks with an
optional check ID. Explicit navigation appends; Back and Forward restore; automatic initial
selection replaces. Selecting the same destination can reveal it again without appending.
Opening one of the Agents, Sessions, or Checks progress steps' settings modals on top of
Overview is not a separate history entry.
New navigation after Back removes the forward branch. Deleted session targets are pruned.
History lives for the renderer lifetime. The sidebar stays visible above the 720 CSS pixel
navigation breakpoint and becomes a modal drawer below it;
previously saved collapse preferences are ignored without being deleted.

Explicit session targets and Back/Forward reveal the selected detail in compact layouts,
including a repeated target after the collection's Back action. Ordinary row selection and
filter changes do not force detail open. The navigation owner sends a reveal intent to the
session owner; the generic collection pane receives only its monotonically increasing revision.

Session destinations retain the selected agents, check result, and spend facet. Agent order
does not create a distinct history entry. Returning through the Sessions sidebar item keeps
these facets and the retained session selection. Native session requests also keep the
current facets. Back and Forward restore the full facet state with the session identity.

The shell sends one revisioned `main:navigation-target` request containing a destination.
Live events and generation-scoped peek/acknowledgement recovery use the same request.
A session opened from another surface cannot create a separate intermediate section entry.
Settings is a separate window and does not enter main-window history.

| Action         | macOS                 | Windows / Linux      |
| -------------- | --------------------- | -------------------- |
| Back / Forward | Command+[ / Command+] | Alt+Left / Alt+Right |
| Search         | Command+K             | Control+K            |

Editable controls, composition, repeated keydown events, and open modals retain ownership.

## Static search

### Shared view registry

`apps/desktop/src/lib/navigation/mainViews.ts` owns the ordered top-level view IDs,
visible labels, and aliases. Both the sidebar and search derive their metadata
from this list. To add a main-window view, register its descriptor and supply its
renderer binding in `MainWindowView.tsx`. Type checking requires every binding;
search includes the new view automatically. Exact queries can reach views beyond
the empty-query group's five-result limit.

The registry contains pure metadata and imports no other modules. Icons, React
components, session instances, and event handlers belong to the renderer. History
uses the registry-derived `MainViewId`; native cross-window requests retain their
narrower DTO and an explicit mapping at the renderer boundary. A new local view
does not extend native routes or permissions. Adding a native route still requires
an explicit TypeScript/Rust contract change.

Settings host session counts use the revisioned native navigation route, including
pending-request recovery when the main window opens. Only Settings can send an
Activity destination with a validated remote host ID. The renderer opens that
host’s list with other facets cleared and no selected session. The link explicitly
retains the user’s Sessions date range; the count represents all cached sessions.

The same native route also carries an optional Overview step: the popover's
attention banners open the main window on Overview and straight into the
Agents, Sessions, or Checks step's settings modal, rather than a Settings pane
that no longer exists. The Rust `NavigationDestination` gained an
`overview_step: Option<OverviewStep>` field (`OverviewStep` is `Agents`,
`Sessions`, or `Checks`), valid only alongside the Overview section; the
`open_main_window_section` command and its caller-scoping gate it the same
way. `mainWindowIpc.ts` exposes it as `openMainWindowSection(section,
overviewStep?)`, and `MainWindowNavigationSession.ts` opens the named step
(calling the Overview store's `openProgressStep`) once it applies a request
that carries one. This is the one native route a new local view does not
need: adding a native route still requires an explicit TypeScript/Rust
contract change, as above.

### Feature-owned search metadata

Settings panes, Settings controls, agent session filters, and checks own their
descriptors. Search combines these descriptors without maintaining independent
label or alias lists. Settings panes share sidebar order and require exhaustive
icon and renderer bindings. Check descriptors use the existing detector IDs.

The Agents, Sessions, and Checks progress steps own a parallel set of search
descriptors in `stepSettingsTargets.ts`, for the controls that moved out of
Settings into their step's modal. These targets carry a
`step` rather than a `pane`, reuse the same label, alias, and platform shape,
and resolve through `resolveStepSettingsSearchTarget` to the step and control
to open. Choosing one navigates to Overview and opens that step's modal with
the control revealed and focused, never changing its value — the same
reveal mechanism Settings controls use (`SettingsTargetFocus`), attached to
the step modal instead of a Settings pane. Limits has no modal of its own, so
its settings stay searchable through `settingsSearchTargets.ts` under Usage
instead. During the first run, before the step modals are reachable, search
excludes every `stepSetting` result; `AppSearch`'s `stepSettingsAvailable`
prop carries that gate, and it does not apply to any other result kind.

The contextual filter menu owns result and spend choices, while live session state
supplies counts. Fixed filters are excluded from search. The top-level Sessions
search result explicitly clears all facets and opens the full list. Agent search
results select only that agent and clear result and spend facets. Agent filter
labels share one formatter; search includes registered agents, while the contextual
menu shows agents present in loaded sessions, including unfamiliar slugs.
This distinction does not cause additional reads or background discovery.

Settings control IDs determine their owning pane. The internal search target
contains either a pane or a control, never a separately supplied pair. The
renderer resolves control targets into the existing `pane#control` wire format.
Session filters can only accompany the Sessions view in search targets.

Settings-specific row adapters apply control IDs, labels, and focus attributes
to generic UI primitives. The adapters add no layout wrappers. The same label
resolver supplies search and Settings with platform-specific tray terminology.
Unavailable-build explanations can retain their contextual wording and remain
focusable. Delayed controls still reveal themselves when they mount.

Settings remains a separate window. Individual session search is not included.
Agent filter, Settings, check, and top-level view destinations remain available.
`Checks` accepts its former `Burn checks` name as an alias.

When adding a destination, update its owning descriptor and renderer. Verify that
the rendered destination is searchable and that each configured target reaches
its control or panel without changing values. Metadata import rules prevent UI
and controller dependencies; generic rows cannot import Settings metadata.

### Search behavior

`appSearch.ts` ranks a local catalog of views, agent filters, Settings targets, step-settings
targets, and checks. Names rank before aliases; category groups are bounded. Settings
controls use stable IDs from `settingsSearchTargets.ts`, shared with their visible labels;
step-settings controls use the parallel stable IDs in `stepSettingsTargets.ts`. The existing
Settings request accepts `pane#control`; old pane-only requests keep working, and a request
naming a pane that no longer exists — `sources` or `checks` — falls back to General rather
than failing. The Settings renderer reveals an arriving control once and can wait for a
delayed row without a React effect; the Overview step modal reveals a step-settings control
the same way.

Search only navigates. It does not toggle controls, scan, fix, export, or delete data.
Queries are transient and local. Analytics carries only explicit actions and fixed result
categories, described in `docs/analytics-measurement.md`.

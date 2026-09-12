# Session filter presets in the main window sidebar

## Scope

Add sub-items under "Sessions" in the main window sidebar. Each sub-item
filters the sessions list and shows a count of the sessions it selects.

Order, with separators between groups:

1. Notable Sessions, Material Sessions
2. One item for each harness present in the loaded list
3. Failing Sessions, Passing Sessions
4. All Sessions

Decisions confirmed by the maintainer on 2026-09-12:

- The list keeps the current activity window and row cap. Counts use the
  loaded list only.
- "Sessions" stays selectable. A click on it selects "All Sessions".
- Notable uses the existing high-cost flame rule. The rule can change later.
- Material means a priced cost of at least $1. Unpriced sessions are not
  Material.
- Failing means at least one finding. Passing means at least one assessed
  check and zero findings. A session with no assessed check is neither.
- Harness items appear only for harnesses with at least one session. All other
  items always appear, also with a count of zero.
- The label suffix "Sessions" stays until UAT.
- The selected filter persists in `AppSettings`.
- A new analytics event records the selected filter with a closed vocabulary.

## Design

- Filtering and counts are client-side. The activity controller already holds
  every session in the window, and hygiene is already fetched for the whole
  list.
- The hygiene fetch moves from `SessionList` into `MainActivitySession`. The
  request stays pinned to the unfiltered list. Counts fold over the full list.
- `SidebarNav` gains nested items and a count badge. The count uses the muted
  mono pill already used for the model count on session cards.

| Step | Status |
| --- | --- |
| Sidebar nesting, counts, and `design.md` update | Pending |
| Filter model, counts, hygiene lift, persistence, analytics | Pending |
| Wire the nav to the filter model; view tests | Pending |
| PR and CI | Pending |

<!--
  This Source Code Form is subject to the terms of the Mozilla Public
  License, v. 2.0. If a copy of the MPL was not distributed with this
  file, You can obtain one at https://mozilla.org/MPL/2.0/.
-->

# antiburn HUD: states and positioning

_Behavior reference for the floating HUD. Source-governance D-026 authorizes
the port, and deviations-register D-30 records its platform and resource costs._

The HUD is a small always-on-top window that shows usage bars outside the menu.
It has three visible states. Hovering the HUD must never move its visible panel.

## The states

```mermaid
stateDiagram-v2
    [*] --> Hidden
    Hidden --> Collapsed: Usage pop-out button<br/>or Settings toggle
    Collapsed --> Hidden: ✕ in the HUD<br/>or Usage pop-out button

    Collapsed --> Expanded: pointer rests on it 250ms
    Expanded --> Collapsed: pointer leaves

    Collapsed --> Dragging: mouse down
    Expanded --> Dragging: mouse down
    Dragging --> Collapsed: mouse up (pointer away)
    Dragging --> Expanded: mouse up (pointer still on it)

    note right of Collapsed
        Bars only. No panel, no
        background, no chrome.
    end note
    note right of Expanded
        Panel: wordmark, ✕, and for
        each limit a label, a bar and
        its reset time.
    end note
    note right of Dragging
        Always drawn collapsed, so you
        can see the place you are
        putting it.
    end note
```

| State         | What you see                                                 | Purpose                                                    |
| ------------- | ------------------------------------------------------------ | ---------------------------------------------------------- |
| **Hidden**    | Nothing                                                      | The HUD is opt-in.                                         |
| **Collapsed** | Bare LED bars on a transparent background                    | It stays ambient.                                          |
| **Expanded**  | Labels, percentages, reset times, wordmark, and close button | It shows detail on request.                                |
| **Dragging**  | The collapsed bars                                           | It does not cover the place where the reader positions it. |

### Transition details

- Expansion waits 250ms. Collapse is immediate.
- DOM mouse edges provide the focused path. The Rust crate polls the global
  cursor every 100ms for the background path and emits `overlay_hover`.
- Dragging starts on the panel except on its two buttons. Only mouse release or
  window blur ends the drag.
- The drag moves the window manually at most once per animation frame.
- The HUD always renders collapsed for the complete drag.
- Renderer transitions use 150ms. Native frame resizing uses the popover's
  140ms ease and stops immediately when a drag starts.

## Positioning

The native frame is 176 logical pixels wide and exactly as tall as the rendered
panel, up to the original 500px ceiling. The default position is centered under
the primary macOS menu bar, with a 24px menu-bar allowance and an 8px gap.
Reopening a live window keeps the reader's position and measured height.

The renderer measures the collapsed panel before the native window first
appears. Turning the HUD off cancels that pending reveal, even if the measurement
arrives later. Reopening uses the completed measurement. Later measurements
resize the visible window. The panel normally keeps its top edge and opens down.
A HUD that is too low keeps its bottom edge and opens up. Collapse uses the same
anchor, so the collapsed HUD returns to the position the reader chose.

The direction calculation keeps an 8px screen margin. Before the expanded panel
has a measured height, it estimates 48px of chrome plus 50px for each bar. It
uses the largest measured expanded height after the first expansion. It decides
again after a drag and before each expansion. Content changes during one hover
recalculate the direction with the new bar count before the frame resizes.

| Event                     | Result                                                   |
| ------------------------- | -------------------------------------------------------- |
| The reader drops the HUD  | The panel stays at the drop position.                    |
| The reader hovers the HUD | The panel opens down or up without visible movement.     |
| The reader leaves the HUD | The panel collapses without visible movement.            |
| A new limit appears       | The frame recalculates its anchor and follows the panel. |

The native frame no longer reserves transparent expansion space. Desktop clicks
outside the visible HUD reach the application underneath it.

## Data and timing

- Each LED bar has 20 segments.
- Only the first bar blinks during a live session.
- A transcript write stays live for 90 seconds.
- The renderer polls session liveness every 5 seconds.
- The shell memoizes session discovery for 60 seconds.
- The renderer polls usage every 60 seconds.
- Reset labels update every 30 seconds.
- The native hover watcher polls every 100ms while the window is visible.
- The HUD uses the Bitcount Prop Single Variable face for captions, numbers,
  and its wordmark.

## Preference and entry points

The preference key is `antiburn.showFloatingHud` in localStorage. Settings →
Usage writes it. The Usage header pop-out button writes it. The popover session
restores the HUD at startup when it reads `1`. The HUD close button writes `0`
before it hides the window.

Each webview can hold a different localStorage copy. The pop-out button asks the
native window for current visibility when it mounts and whenever the popover
receives focus. This known drift remains part of the port.

## Platform boundary

v1 is macOS-only. The native crate returns without creating a window on other
platforms, and the frontend hides both entry points there. Windows needs tuning
for its taskbar position. Linux waits for reliable Wayland positioning and
always-on-top behavior.

## Rejected positioning designs

| Design                                      | Failure                                                  |
| ------------------------------------------- | -------------------------------------------------------- |
| Keep a fixed 500px transparent frame        | Invisible space blocks clicks in other applications.     |
| Make the complete window ignore mouse input | The visible HUD cannot expand, drag, or answer controls. |
| Never move and never flip                   | The expanded panel clips at the bottom.                  |
| Move the collapsed anchor to make room      | The HUD leaves the position the reader chose.            |
| Resize before collapsing for a drag         | The first drag movement uses the expanded origin.        |

The panel direction is the correct lever. The visible HUD position is not.

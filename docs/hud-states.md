# antiburn HUD: states and positioning

_Behavior and ownership reference for the floating HUD._

The HUD is an opt-in, always-on-top usage meter on macOS. Its small native frame
fits the visible bars so transparent space does not block clicks in other apps.
Hover detail appears in a separate passive window; the HUD stays where the
reader placed it. The Settings → Usage toggle hides or shows it. The close
control is currently disabled.

## The states

| State        | What the reader sees                                                                             | How it changes                                                                                 |
| ------------ | ------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------- |
| Hidden       | No HUD or detail card.                                                                           | The Settings toggle shows the saved placement.                                                 |
| Floating     | Usage bars, with an optional token map above them.                                               | Hover opens detail after a short intent delay; a drag moves or docks the HUD.                  |
| Detail shown | The floating bars and a separate card with labels, values, reset times, and live-session detail. | Pointer exit or drag hides the card immediately.                                               |
| Dragging     | The HUD without its detail card.                                                                 | Releasing it saves the placement; a drop at an available display edge docks it.                |
| Docked       | A narrow tab at a display edge.                                                                  | Hover peeks the full HUD; dragging tears it off; activity, high spend, or a reset can wake it. |
| Island       | On a notched Mac display, a black row blends into the notch.                                     | Hover expands the usage content below the notch; a drag can tear it off.                       |

The floating frame never expands to hold the detail card. A mouse down cancels
pending hover detail, and a drag hides any open card. A fresh hover is required
after the drag. The card is display-only, ignores input, and never takes focus.
The island carries labels and reset times in its expanded body, so it does not
open that card. Reduced motion stops the live pulse and appearance animation.

### The token map

“Show what live sessions are doing” in Settings → Usage enables the experimental
map; it is off by default. The map summarizes tokens written during a five-minute
window. It appears when at least two agents, including sub-agents, contribute.
One agent leaves the bars alone and remains available in the detail card. A
session with a low rounded rate retains a dim dot, and a sub-agent uses smaller
dots inside its parent session's frame. Dot colors represent work modes; the
card names their rates and current dot value. The map changes scale only as
needed to fit and resists shrinking immediately after a burst. A recently
hidden map waits one poll before returning, to avoid flashing near the
two-agent boundary.

The map's five-minute accounting window and its recent-turn highlight are
separate from **working** liveness. The working indicator comes from the
session lifecycle registry's exact working and anonymous counts. Named work
becomes quiet after 30 seconds without a write; anonymous work clears when the
registry resolves it or its own deadline expires. Quiet sessions can remain in
session lists but do not keep the HUD's working indicator on. The HUD does not
infer this state from its own transcript timer or from the map's dots. See the
[session lifecycle contract](session-lifecycle-events.md) for the evidence and
provider-route rules.

### When there are no bars

An empty track preserves the HUD's presence and size. The detail card
distinguishes the reader's choice, “No meter selected,” from a selected meter
that has no provider reading yet, “No usage limits detected yet.” A Settings
change reaches the HUD through a shell push; the regular usage refresh is a
fallback.

### Docked

Dropping the HUD near a free display edge parks most of its frame off screen and
leaves a tab to find. A shared edge between displays is not a dock edge. Hover
peeks the full HUD at the edge; it parks again after the pointer leaves. A drag
on the tab or peeked HUD tears it off, and a later edge drop can dock it again.
The detail card hides before docking.

A new spell of activity after a long quiet interval, sustained high spend, or a
usage reset can wake a docked HUD temporarily. The HUD's renderer and its
visible-feature updates remain available while docked, so a peek is immediate.
Hiding the HUD retains its dock preference for the next open.

### Island

On a Mac with a notch, dropping over that notch or using Settings → Usage →
Docking → Move to Notch selects the island. The collapsed row sits beside the
notch with a live mark. Hovering its hotspot expands the bars, labels, reset
times, and any token-map legend below it. Clicking the row alone does not tear
it off; dragging beyond the movement threshold does. The drag previews the
island shape over a valid notch, then returns to the floating shape away from
it.

The island belongs to the notched display. If that display disappears, the HUD
falls back to a top dock while retaining the island preference, and returns
when a usable notch reappears. An explicit tear-off or Settings action clears
that preference. The webview draws no content under the physical notch.

## Positioning and native boundary

The shell owns placement and native window geometry. It stores recent
per-display positions relative to each display, so rearranging monitors does
not turn a saved offset into a global desktop coordinate. A drag makes that
display preferred. Disconnecting it uses another connected placement without
rewriting the preference; reconnecting it restores the preferred location.
The shell measures the rendered HUD before its first reveal and updates the
native frame when content height changes. It parks display checks while hidden.

The HUD and detail are passive, nonactivating macOS panels: neither becomes key
or main, and the detail passes clicks through. Their native policy joins Spaces
and fullscreen Spaces on the HUD's chosen display. The shell reapplies that
policy before reveal and leaves the window hidden if panel conversion fails.
Stacking level alone did not make an ordinary window visible in another app's
fullscreen Space. There is one HUD on one display, not a copy per display;
fullscreen and Space behavior need live macOS QA after native changes. See
[window renderer lifecycle](window-renderer-lifecycle.md#macos-overlay-presentation)
for the shared presentation boundary.

The HUD and detail windows are macOS-only. Windows needs taskbar-aware placement;
Linux needs reliable Wayland positioning and always-on-top behavior before the
entry point can be exposed there.

## Ownership and resource limits

The Settings preference controls startup restoration. Native visibility
changes are broadcast because each webview can hold a different localStorage
copy; Settings uses live visibility rather than treating its cached preference
as the current window state. The shell stores dock and island placement
separately from that visibility preference.

The HUD renderer owns its displayed bars, map, hover intent, and card content.
The shell owns the native frame, placement, docking, island geometry, passive
presentation, and the detail window's reveal and conceal. The first detail
hover creates its renderer hidden; later hovers reuse it. Before hiding, the
detail renderer clears its card, with a native fallback if it cannot respond.
The shell sizes the card from measured content before showing it, placing it
below the HUD when possible and above it near the bottom of the display.

While visible or docked, the HUD refreshes usage and the optional token map;
the native hover watcher also runs. Hiding the HUD parks those polls and
renderer timers. The HUD's map and spend calculation use bounded recent
samples. The only live motion on its bars is the first bar's rate-sensitive
blink. Its fastest rate is capped for flash safety. When no model has a known
price, the blink uses allowance consumption if available; an unknown spend
must not imply that work is quiet. Reduced motion stops the blink and the
detail card describes the spend in words. Provider and model sweeps belong to
popover meters and session rows, not the HUD. Scoped sweep eligibility and its
attribution limits are documented in
the [session lifecycle contract](session-lifecycle-events.md#scoped-sweep-evidence).

The separate detail window preserves the reader's HUD placement, and the
content-sized frame preserves click-through outside the visible meter. Those
constraints rule out expanding the HUD in place or reserving a large
transparent native frame.

Implementation owners: [HUD renderer](../apps/desktop/src/views/overlay/OverlaySession.ts),
[native HUD window](../apps/desktop/src-tauri/crates/hud/src/lib.rs),
[dock policy](../apps/desktop/src-tauri/crates/hud/src/dock.rs),
[island policy](../apps/desktop/src-tauri/crates/hud/src/island.rs), and
[placement store](../apps/desktop/src-tauri/src/hud.rs).

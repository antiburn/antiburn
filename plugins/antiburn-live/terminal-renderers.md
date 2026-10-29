# Terminal brand rendering decision

Date: 2026-10-16. Status: accepted.

## Decision

Use the standard monospace `antiburn` wordmark in bright antiburn orange on a
muted orange background. Reserve exactly two terminal rows for both inline and
stacked layouts. The lower row carries the bright wordmark and inline meters;
stacked limit rows use the same two-row space. Keep the upper row blank as padding
while idle; during activity, let shaded-block flames rise through it. Animate a
heat ripple behind the bright lower-row letters so they stay legible. Reserve
11 columns for the wordmark slot: one inset column plus a 10-column grid (the
eight letters and one background-padding cell on each side). Keep the one-column
gap to the meter outside that slot, for 12 columns total before the meter.
Desktop keeps the branded dot mark and its existing motion.

The terminal block animation is experimental. Candidate shading glyphs include
`█`, `▓`, `▒`, `░`, `▄`, and `▀`; visual comparison across supported fonts should
guide the final glyph set. The old braille mark remains an inactive code reference.

Remove image rendering and its cell-size input, probes, diagnostic controls,
image state, and mount lifecycle. Pace-marker hover stays confined to the marker.
Reset text stays beside its window label and may paint over the pace marker.

The flame trigger is shared by desktop and terminal surfaces. See the
[cross-surface burn activity heuristic](burn-activity.md) for signal inputs,
sampling, thresholds, expiry, and limitations. The two-row wordmark grid above
describes only the terminal presentation.

## Image experiment evidence

The user tested the Image experiment in an iTerm/Claude Code session. The host
check reported `Image evidence: denied`, the test image was blank, and the strip
retained its fallback. The rejection reason was not retained, so this does not
establish that iTerm itself lacks image support. The experiment required manual
cell-pixel measurements and had no verified visual benefit. It was removed to
keep the terminal renderer simpler and testable.

Revisit images if Claude exposes reliable graphics capability and cell geometry
detection and a native rendering path passes visual checks in supported terminal
configurations. Revisit braille only if a compact pattern stays legible across
terminal fonts and both layouts. No terminal image support is currently claimed.

### Usage warning dots

Reached pace markers use bright antiburn orange without dimming. Red filled dots
swap `●` and `•` every 800 ms for six 1.6-second cycles, then settle to `●`.
Orange and unused dots keep their glyphs. Reduced motion keeps all dots static.
The existing warning examples remain previews; this change does not enable a
new warning policy for live readings. Empty readings, hidden strips, explicit layout
selection, and session teardown cancel the pulse. A completed revision does not
restart on an ordinary redraw.

### Website link

The terminal wordmark uses a plain native Button. An explicit click or keyboard
activation opens the fixed `https://antiburn.com` URL through the host process
API, with bounded opener attempts (`open`, then `xdg-open`; Windows uses
`cmd.exe /c start`). A failed opener leaves the address in the host status line.
Rendering and focus alone do not launch a browser or reveal reset details.

The wordmark text cells reuse the raster heat colors, and the upper flame row
remains a raster. During burn, each 100 ms frame paints the upper row and
requests a redraw for the letters. The two-row allocation and letter positions
stay fixed. The logo has no hover hint or tooltip; click opens the website.

Terminal window labels and single-cell pace markers each have a bounded Client
pointer region. Entering a label reveals its reset detail next to that label.
Entering a pace marker reveals pace detail near the marker. Neighboring dots do
not trigger pace detail. Pointer leave clears only the transient reveal and
restores any explicitly activated detail. Click/tap and Enter remain fallbacks;
they toggle a persistent detail. Hover support depends on host mouse reporting.
The Client targets add no rows or timers, and the flame raster remains outside
them. Desktop rendering and interaction stay unchanged.

Native evidence: Claude Code 2.1.301 in Claude desktop accepted the explicit
host opener and opened antiburn.com after keyboard activation. The user reported
that the earlier OSC 8 wordmark worked with Cmd-click in iTerm but failed with
both Cmd-click and Ctrl-click in Claude's embedded terminal. This does not prove
support for all terminal, multiplexer, or operating-system configurations.

On desktop, a native text link overlays the logo in both resting and burning
states. The host rejects SVG or Raster children inside a Link, and the tested
SVG sandbox does not navigate external anchors. Logo tooltips are removed;
flame playback stays unchanged. The reset and pace interaction overlay starts
at the meters so it does not cover the logo link.

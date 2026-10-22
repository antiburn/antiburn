> The local development artifact is **antiburn-dev**, separate from the public
> **antiburn** candidate. Public output excludes CLI discovery, live checks and
> previews. This guide documents the development feature set; see
> [the plugin page](../README.md) for current distribution status.

# antiburn development source

Burn animation and its preview controls are temporarily disabled until refined.
App-backed predefined checks, development previews, static branding, quota
controls and loading indicators remain available. Animation algorithms remain in the source for later work.

This folder contains the maintained development implementation, including
the app-backed Checks pane and debug tools. Selected shared usage files and the
public-specific inputs in `release/` also feed the generated
[`antiburn`](../antiburn/README.md) artifact. The public entry points come from
`integrations/antiburn/claude`; the development entry is `hooks/register.tsx`.
The public artifact excludes development checks and debug controls. See the
[plugin architecture](../../docs/plugins/architecture.md) for the complete
source-to-artifact map. The cross-vendor core keeps the `antiburn` name.

The command examples below describe this source's `/antiburn-dev` registration.
Assembly rewrites commands and state owners to `antiburn-dev` in the
local development artifact, so it can coexist with the release and core.

A Claude Code mod that shows account usage above the prompt. Usage is the
default mode. Set `ANTIBURN_LIVE_MODE=checks` before launching Claude Code to
add the app-backed predefined Checks pane. Other values select usage.
Changing the flag requires a reload or restart; `/clear` retains the selected
mode. The mod makes no network calls.

In this README, "desktop" means the Claude desktop app. The development mod reads saved antiburn app results through the CLI;
"desktop" does not refer to that evaluator. See "Privacy and resources" below.

## Install and run

Generate the local-only development artifact from the repository root:

```bash
node scripts/assemble-claude-mod.mjs --dev
claude --settings '{"tui":"fullscreen"}' \
  --plugin-dir .agent-artifacts/claude-mod/dev/plugins/antiburn-dev
```

The development launch above selects fullscreen for this process so panes dock
on the right when the terminal is at least 110 columns wide. It does not change
the user settings. Narrower terminals use the inline fallback.

Regenerate after changing source files. The host can reload changes in the
emitted folder; it does not run the assembler for you. Use a separate test
session and normally enable only one usage strip per session.

The generated marketplace at `.agent-artifacts/claude-mod/dev` is named
`antiburn-local-dev`; it lists only `antiburn-dev`. Keep it local.
The repository marketplace lists only public `antiburn` 0.2.3 at its
immutable artifact commit. It does not publish this development build. See the
[plugin page](../README.md) for current distribution status and remaining gates.

The source manifest retains its development name. Do not publish this folder.
Its hooks module is `hooks/register.tsx`.

## Surfaces

### Persistent usage

The small `a` mark accompanies the “5-hour” and “Weekly” meters. Usage stays
closest to the composer, below optional checks, including while Claude works.
A host survey temporarily owns the band. Claude Code scrolls a tree taller
than its `maxRows`; the mod does not override that height limit.

The Usage card in `/antiburn-dev debug` holds usage details and a Refresh button
for the native snapshot. Details include the exact percentage, reset timestamp
and recognized additional windows. Values come from `session.usage()` and `session.measure`, as last
reported by Claude Code. There is no independent provider fetch or freshness
claim. Desktop engine 2.1.295 was observed returning only a weekly window
from `session.usage()` after the five-hour reset; the mod preserves that missing
percentage as unknown and uses “Starts when a message is sent” for its reset
text when a valid weekly reading is present. This copy is a presentation
fallback; the SDK does not expose an inactive-window flag. Empty readings say “Usage not available yet”; missing windows use an
unknown value, not zero. Failed refreshes label retained values “Last known” in the debug pane and accessible meter descriptions. Desktop keeps the same strip height across ready, empty, and failed readings.
An exceeded percentage stays exact while its meter saturates at 100%.

The pace marker uses the elapsed fraction of the known five-hour or weekly
period. It is a continuous neutral line before usage reaches that unrounded fraction,
then a continuous orange line. High usage remains static orange.

Desktop draws the labels, exact percentages and whole resting meter dots in SVG.
The filled count rounds to the nearest dot, with one lit for nonzero usage and
all dots lit only at 100%. Desktop fill transitions can pass through partial dots.
Terminal uses whole filled or empty glyphs. Red warning dots stay fully to the
right of a reached pace marker; the marker never splits a dot's color.
Bundled Bitcount glyphs provide dot typography without loading a font at runtime.
Desktop primary captions use 14px lettering; dim secondary copy stays at 12px.
The usage-strip mark is 13px wide, including activity previews.
The small mark centers in its unchanged 14px slot against the complete meter block. A 5px leading inset
balances the host padding against the 13px gap before the meters. A matching
5px trailing inset gives the last dot the same band-edge clearance as the mark.
Reset details occupy reserved caption space: hover reveals them after a 300ms dwell; focus reveals them immediately without
adding rows. Click away or move focus to dismiss. Reset text centers vertically
against the label. The 18px pace marker centers on the dots and uses one continuous stroke.
Pace hover contrast uses the same 300ms dwell; focus and tap stay immediate.
Missing percentage readings display `—`; internal availability and read-error states remain distinct.
Desktop draws no native tooltip; the Usage card in `/antiburn-dev debug` keeps full
reset and pace text available, and accessible labels retain the same information.

Desktop presentation includes the once-per-session entrance, 600–960ms fill
sweep, changed-digit rolls and reset hover transitions. SVG CSS handles motion;
host events release expired presentation bookkeeping without scheduling timers. New readings interrupt entrance and
settle to current values. Resize, surveys, clear and teardown cancel presentation
work. Same-session reload does not grant another entrance. CSS reduced-motion
rules remove movement and delayed fill and use 80ms opacity for entrance/details.
Live reduced-motion preference changes were verified on desktop 2.37337.1.
Full native boundary-frame validation is tracked separately;
these implementation rules are not a claim of pixel-identical native output.

Terminal retains an explicitly accepted static fallback: native text captions,
whole/half-dot glyph approximations, exact numeric values and a two-cell brand
gap. It schedules no presentation timers and does not assume pixel Image
support, pixel cell dimensions, or an unavailable reduced-motion signal. Both
adapters reduce dot counts as space shrinks. The desktop's 80-column threshold
uses the tested 8px-per-column scale; other host scales still need validation.

### Optional checks (`ANTIBURN_LIVE_MODE=checks`)

Antiburn's desktop app evaluates installed YAML checks. The development mod
reads saved results through `antiburn checks SESSION --delivery-v1` and displays
them in the existing checks strip, footer indicator and expandable pane. It
never detects problems from prompts, tools, token counts or turn events.

Build the new CLI from `apps/cli` before testing this mode. The app must run and
be configured to produce new assessments. A CLI read can return saved results
while the app is closed; it does not prove freshness or recent evaluation.
See [session checks](../../docs/plugins/session-checks.md) for the protocol,
coverage and native context-delivery adapters.

### Checks pane and indicator

Open the pane with `/antiburn-dev checks`, the footer indicator or the strip's
View control. The existing expandable cards, section navigation and keyboard
controls remain. Cards use dynamic identities from Antiburn, so adding a YAML
check requires no matching local detector or fixed catalog entry.

Each real card displays the authored assessment message, assessment time and
checking activity. It offers local dismissal only. It provides no fix prompt,
verification action, repair claim or savings estimate. A saved eligible finding
can remain visible while another assessment runs.

Connection labels distinguish these states:

- `Connected · freshness not evaluated` means the CLI protocol returned results.
- `Waiting for Antiburn to discover this session` retains prior warnings while
  the app has not produced an authoritative discovery snapshot.
- `Antiburn disconnected · showing last known checks` retains previous cards
  after a failed process call.
- `Antiburn could not read the latest checks · showing last known checks`
  retains previous cards for unavailable or incompatible saved results.
- `Antiburn checks are inactive for this session` removes cards when checks are
  disabled, unconfigured or unsupported.

An explicit `cleared` update moves a prior warning into the resolved group.
It means the saved assessment no longer reports the warning; it does not prove
repair. An `invalidated` update removes advice whose eligibility is no longer
confirmed. Connection failures never simulate either transition.

`Dismiss this check` hides a card locally. The pane's `claude-code:ui` consumer
is independent from `claude-code:context`; dismissing or viewing a card cannot
consume the model's advice. Usage meters remain available when the CLI or app
is unavailable.

The terminal docks the pane on the right in fullscreen layout (`/tui fullscreen`
or `"tui": "fullscreen"`). Classic layout shows the pane inline. Use the
isolated fullscreen development launch above for pointer testing. Desktop,
terminal and narrow surfaces still need separate visual acceptance.

## Commands

The mod registers `/antiburn-dev`. The separate usage pane is currently disabled;
usage readings, developer examples and animation inspection are in the Usage
card of `/antiburn-dev debug`. The Checks card groups mode controls and check demos. Only checks mode advertises and accepts the
`checks` and `demo` arguments. Usage mode does not collect checks, discover
skills, publish check status, open check panes or run check actions.

| Command                   | Action                                                                                                                                                                                                                                 |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/antiburn-dev`           | Opens live checks in checks mode. In usage mode, confirms that limits are shown above the prompt without opening a pane.                                                                                                               |
| `/antiburn-dev checks`    | Opens the checks pane in checks mode.                                                                                                                                                                                                  |
| `/antiburn-dev demo`      | Shows a sample view with all eight checks and opens the pane. Five are active (E1, D, T, K1, K2), one is watched (X1) and two are resolved (O by its fix, C1 by itself). The sample stays until `demo off`. `demo states` is an alias. |
| `/antiburn-dev demo tour` | Starts a tour of 4 steps on an empty, separate set of checks, and opens the pane.                                                                                                                                                      |
| `/antiburn-dev demo next` | Plays the next tour step as one turn. The `Next step` button in the pane does the same.                                                                                                                                                |
| `/antiburn-dev demo off`  | Ends the sample view or the tour, and shows the live checks again.                                                                                                                                                                     |

Any unrecognized argument replies with the usage line.

The tour steps are:

1. A retry loop is the first warning (X1).
2. Three more checks arrive (T, E1, K2). The tour dismisses the strip first,
   so the strip comes back with `(N new)`.
3. The context gets heavy, and the retry loop stops (D with its cause, X1
   resolves).
4. A skill goes unused, and edits leave the plan (K1, C1).

The tour does not show O, because O fires only on the first turn. The tour
uses its own set of checks. The app-result consumer remains separate from the synthetic demo tracker.

**Caution:** a fix in a demo or a tour acts for real. It sends the prompt to
Claude, fills the prompt box, or compacts the session.

## Runtime checks and development samples

Real-session checks come only from Antiburn's installed YAML definitions. The
old `LiveChecks` rules receive no real prompts, tool results, steps, usage,
completion or compaction events. There is no local detector fallback.

The eight fixed catalog samples remain in the development demo and tour. They
exercise the existing visual states and keyboard navigation; they do not
establish app evaluation or check coverage. Sample verification and savings
labels are not claims about the current session. See `hooks/catalog.ts` and
`hooks/checks.ts` for the retained sample implementation.

## Privacy and resources

- **Evaluation.** The app is the sole evaluator. The mod starts no provider
  evaluation, network request, model turn or idle polling loop for checks.
- **CLI reads.** At session start and turn completion, the mod uses the host's
  sandboxed process API with an argument vector and bounded execution/output.
  It reads the versioned CLI response, applies UI state, then acknowledges the
  UI receipt. It never runs a shell or launches the desktop interface.
- **Data.** The pane receives authored messages and safe result metadata, not
  YAML definitions, provider credentials or raw evidence. It validates session
  identity, main-session scope and protocol before applying results.
- **Persistence.** UI and dismissal state belong to the local session. The CLI
  keeps bounded consumer receipt/fingerprint state separately from its
  read-only product database. It does not store message text in that state.
- **Independent usage.** Native account-usage readings and usage interaction
  state retain their existing behavior. A check-connection failure does not
  stop those readings.
- **Failure.** Bounded read errors retain last-known cards. They do not block a
  coding action, run a local detector or claim that a warning cleared.

## Development

From the repository root:

```bash
node scripts/assemble-claude-mod.mjs --dev-test
claude plugin test .agent-artifacts/claude-mod/dev-test/plugins/antiburn-dev
claude plugin validate .agent-artifacts/claude-mod/dev-test/plugins/antiburn-dev
```

The test stage excludes release-source tests and runs development tests against
the generated development identity. Do not run `claude plugin test` directly
on this source folder: the host recursively discovers the separate release
entry's tests, which belong to the public artifact's test stage.

For a source type check, load `plugins/antiburn-live` directly in a separate
interactive test session to generate its local `.claude-plugin/types`, then run:

```bash
npx -y -p typescript@5.6.3 tsc -p plugins/antiburn-live --noEmit
```

Generated declarations reflect the installed host and can include account MCP
types. Keep them ignored and out of both artifacts. CI validates capabilities
and tests both emitted identities; the separate SDK type check remains a local
release gate.

The `policy` part of `hooks/catalog.ts` must agree with the engine catalog
(`ReportCatalogs`): the revisions, the depth cap (D), the effort cap (T), the
premium models (S) and the model replacements (O). A test compares each one,
and it fails when `policy` holds a field that has no test. The `live`
thresholds and the `checks` text exist only in the mod, because the engine has
no copy of them. From the repository root, run:

```bash
cargo nextest run --manifest-path crates/antiburn-local/Cargo.toml --test live_catalog_drift
```

The [antiburn Claude plugin workflow](../../.github/workflows/plugin.yml) uses
Node 24 and Claude Code 2.1.303. It checks deterministic packaging, strictly
validates the public marketplace/plugin, tests the exact public artifact and
separate development build, and runs an isolated installation lifecycle check.
The public capability policy permits only the approved account-usage and website
actions; development capabilities are separate. This workflow does not publish
a release or replace native acceptance. See the
[architecture checks](../../docs/plugins/architecture.md#checks-and-their-limits).

### File layout

| Path                          | Contents                                                                                                                                                            |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `.claude-plugin/plugin.json`  | The manifest: name, version, description, and the state types.                                                                                                      |
| `hooks/hooks.json`            | Loads `register.tsx`.                                                                                                                                               |
| `hooks/register.tsx`          | The hooks, the `/antiburn-dev` command, the strip, the chip, the pane, the app-result bridge and the demo.                                                          |
| `hooks/checks.ts`             | `LiveChecks`, retained rules and stages for development samples only. It takes no `$` and does no I/O.                                                              |
| `hooks/catalog.ts`            | The check catalog: ids, names, sentences, fixes, thresholds and model replacements. Keep the text between the `catalog:begin` and `catalog:end` markers valid JSON. |
| `hooks/tour.ts`               | The 4 demo tour steps.                                                                                                                                              |
| `hooks/*.test.ts`             | Tests for the checks, the surfaces and the tour.                                                                                                                    |
| `hooks/usage.ts`              | Pure usage normalization, pace calculations and read ordering.                                                                                                      |
| `hooks/usage-graphics.ts`     | SVG typography, meter drawing and bounded CSS motion.                                                                                                               |
| `hooks/usage-presentation.ts` | Shared geometry, timeline calculations and entrance ownership.                                                                                                      |
| `hooks/usage-font.ts`         | Bundled Bitcount glyph layout; license in `assets/bitcount`.                                                                                                        |
| `types/index.d.ts`            | The check view and the state types.                                                                                                                                 |
| `types/claude-code.d.ts`      | Names the `claude-code` module for tools that read only the committed source.                                                                                       |
| `tsconfig.json`               | Extends the generated `.claude-plugin/types/tsconfig.json`.                                                                                                         |
| `.gitignore`                  | Ignores `.claude-plugin/types/`.                                                                                                                                    |

## Known interaction issues

### The first click in the pane only moves the keyboard

Seen in Claude Code 2.1.296 in the desktop Code tab.

When the prompt holds the keyboard, a click on a pane button moves the
keyboard to the pane but does not press the button. You must click again.
While the pane holds the keyboard, later clicks press at once.

The mod cannot work around this. The host does not raise `ui.press` for that
click, and the `ui.focus` event that it raises has no `element`, so the mod
cannot tell which button you clicked. The mod handles a press that it
receives in a few milliseconds, so the delay is the wait for the second click.

A fix needs a change in Claude Code: the click should press the button, or
`ui.focus` should name the clicked element.

### Developer usage previews

Open `/antiburn-dev debug` to show the separate developer debug pane. The `debug`
argument is unadvertised: it is omitted from the normal command hint and help
response. The host can hide whole commands but has no per-argument hiding setting.
No environment variable or restart is required. Usage defaults to **Label reveal**:
label, adaptive dot track, and percentage share a row per window. Narrow views
stack complete rows. The Usage card's layout buttons select **Current** (captions
above the tracks) for comparison with the same live or synthetic readings.
Desktop dots are 7px wide with a fixed 4px gap (11px center to center) and 85% fill opacity in both layouts; available width changes the count, never the spacing. Layout changes cancel pending motion and reset on plugin reload. Hover a label
for reset text over its track; focus or tap reveals it immediately. Pace has its
own control. Terminals retain the static glyph fallback with label and pace
controls, without graphical animation. The debug pane alone shows the
effective mode: **Mode 0 · Usage limits** or **Mode 1 · Usage limits + live checks**.
The mode button switches between usage-only and live checks for the current
session. Checks start fresh after each change; switching to Mode 0 closes the
checks pane and clears its status. **View live checks** opens the existing checks
pane in Mode 1. Reload restores `ANTIBURN_LIVE_MODE` (`usage` or unset for Mode 0,
`checks` for Mode 1); the button does not change that environment setting. Normal checks panes omit developer controls and mode labels. The separate usage
pane is disabled; current readings and Refresh are in the debug Usage card. Select inactive, low, mid, beyond-pace, high, near-limit,
missing, or read-error examples. Extra 9%, 10%, 99%, and 100% examples
exercise digit rolls. Exceeded and fractional examples are not in the menu. Synthetic data is labeled
in the Usage card and above the meters. Native snapshots continue updating separately;
**Return to live data** restores the latest reading.

The Usage card also offers **Idle**, **Checking**, **Retry**, **Success**, and
**Stale** previews for the account-usage states. These selectors change only the
synthetic preview; they do not read native usage, authorize, request account
data, or submit a model prompt. **Refresh** moves Idle or Retry to Checking in
the preview. The usual Return to live control restores the latest native reading.

The animation inspector lists timings and easing and replays entrance or reset
reveal/hide. Example fill transitions run for 360ms, including decreases and
rapid retargeting. Beyond-pace and high previews color only the used portion to the right of a
reached pace marker red. Dots before it stay orange. Near-limit makes all used
dots red and adds a
subtle glow and six 1600ms opacity pulses, then settles. Reduced motion settles
fill, digits, and warning pulses. Terminal examples retain matching values and
static red glyphs; graphical replay requires desktop SVG support. These warning
styles are synthetic demonstrations, not a production warning policy. Reload or
clear may discard preview selection with host state; native limits never change.

Desktop meters draw their data in SVG images and place reset/pace interactions
in a separate transparent frame. Updating values therefore does not reload the
whole visible meter frame. Identical readings that do not change reset or pace
presentation retain their existing graphic source.

The Rooted flame signals recent generated-output activity. It does not report
subscription quota use or exact instantaneous token throughput. See the
[cross-surface burn activity heuristic](burn-activity.md) for its inputs,
thresholds, expiry, and limitations. The debug card offers separate 15-second
active and reduced-motion previews. Desktop uses a finite Ember catch animation;
terminal layout and rendering are documented in the
[terminal brand decision](terminal-renderers.md).
The desktop logo and terminal wordmark link to the antiburn website, including
during activity. The logo has no activity tooltip; terminal wordmark hover shows
a dim modifier-click hint.

The debug pane's **Play all usage examples** button runs once through entrance,
usage fixtures except numeric-roll examples, reset and pace reveal/hide, then Ember catch on the high
usage fixture (88% / 82%). Each example holds for 1.8 seconds; near limit holds
for 10 seconds to show all six pulse cycles. The Ember finale lasts 4 seconds.
Only this explicit sequence schedules one pending `clock.after` at a time.
Stop, a manual fixture, returning to live data, layout changes, restart, and
session changes cancel the sequence. Terminal shows the same sequence of static
states. Native readings continue in the background. The last example remains
selected when playback finishes; **Return to live data** restores native values.

Synthetic usage examples share fixed pace markers (5-hour 55%, Weekly 50%). Only usage changes between examples; native live pace remains time-based. Inactive/missing windows still omit markers when no reset time exists.

## Terminal brand mark

The terminal uses a monospace `antiburn` wordmark in bright antiburn orange on a
muted orange background beside its meters. Both inline and stacked layouts use
exactly two terminal rows. The lower row carries the bright wordmark and inline
meters; stacked limit rows use the same two-row space. The upper row is blank
padding while idle and carries the shaded-block flames upward when active. The
wordmark slot is 11 columns: a one-cell inset plus ten columns for the eight
linked letters and one background-padding cell on each side. The upper flame
row uses a raster, while linked text retains the heat colors below. The meter gap adds
one more column outside the slot. Image rendering and its experimental debug
controls are removed. See the
[terminal rendering decision](terminal-renderers.md).

Desktop usage percentages show whole numbers, rounded down. The source reading
remains exact; readings below 100% never display as exhausted. At 100% and above,
filled dots and the pace marker use steady red with the existing softened fill.
Pace markers include a 1px right-edge shadow at 22% opacity for dot separation.

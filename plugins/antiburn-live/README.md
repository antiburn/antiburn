# antiburn-live

A Claude Code mod that shows live token-burn checks for the current session.
Each check names one pattern that costs tokens, such as a retry loop or a very
large context, and offers one fix. The mod draws a strip above the prompt, a
footer chip and a checks pane, in the terminal and in the Code tab of the Claude
desktop app. It runs locally and makes no network calls.

In this README, "desktop" means the Claude desktop app. The mod does not
connect to the antiburn app (`apps/desktop`). See
"Privacy and resources" below.

## Install and run

To run the mod from this repository for one session, start Claude Code from
the repository root:

```bash
claude --plugin-dir plugins/antiburn-live
```

An interactive session reloads the mod when you save a file in that folder.
For a session that the desktop app starts, name the folder as an absolute path
in `CLAUDE_CODE_PLUGIN_DIRS` (in the process environment or in the `env` block
of `~/.claude/settings.json`).

The repository marketplace (`.claude-plugin/marketplace.json`) lists the mod
at the git tag `antiburn-live-v0.1.0`. This install line works only when that
tag exists on GitHub:

```text
/plugin install antiburn-live --marketplace antiburn/antiburn
```

The manifest (`.claude-plugin/plugin.json`) declares version 0.1.0. Its hooks
module is `hooks/register.tsx`.

## Surfaces

### Strip above the prompt

The strip shows when at least one check is active. It names the top active
check (the first in catalog order: X1, D, O, T, E1, K1, C1, K2) and counts the
active checks as `N to fix`. While Claude works, or while a survey shows, the
strip gives its place back to Claude Code.

| Control | Label | Terminal hotkey | Action |
| --- | --- | --- | --- |
| Fix | The fix label of the top check, such as `Ask Claude to fix` | `1` | Runs the fix of the top check. |
| View | `View checks` (`View` below 80 columns) | `2` | Opens the checks pane and tries to move the focus to its first row. |
| Dismiss | `×`: a close glyph on the desktop, a dim `×` in the terminal | none | Hides the strip until a new check arrives. |

In the terminal, the labels start with the digit (`1 Ask Claude to fix`,
`2 View checks`). The desktop prompt sends a digit to Claude, so the desktop
strip has no hotkeys and no digits.

The strip has four forms. Each form keeps the antiburn name and the top check
name, because the fix button acts on that check.

| Width (columns) | Form |
| --- | --- |
| 100 and more | Name, count with `(N new)`, check name and its metric. |
| 80 to 99 | The metric goes. `(N new)` stays. |
| 56 to 79 | The `View` label is shorter. The desktop shows the antiburn mark in place of the wordmark and drops `(N new)`. The terminal keeps `(N new)` while it fits. |
| Less than 56 | The buttons move to a second row. |

The terminal also moves the buttons to a second row when the check name does
not fit beside them. It shows the count in a box only while the check name
still fits.

After a dismiss, the strip stays hidden until a check becomes new: a check
appears for the first time, comes back after it stopped, or reaches a higher
retry step. The strip then shows `(N new)` beside the count. The same check
in a later turn does not bring the strip back.

When a check resolves and no check is active or watched, the strip shows
`✓ <Check name> resolved` for one turn (`+N` counts the other checks that
resolved). A watched check does not show in the strip.

### Footer chip

The chip is in the session mode footer, after any mode labels. Press it to
open the checks pane.

| Chip | Meaning |
| --- | --- |
| `antiburn · N ●` | N checks are active. |
| `antiburn · N ⧗` | No check is active. N checks are watched after a fix. |
| `antiburn · N` | No check is active or watched. N checks still build evidence. |
| `antiburn ✓` | No open check. |

The chip does not count a check that you ignore. On a VS Code or mobile
surface, which draws no strip, the mod puts the same text in the status line
while a check is active or watched. The terminal gets no status line text.

### Checks pane

The pane opens with the title `antiburn` and asks for 48 columns and 24 rows.
These actions open it: `/antiburn`, the demo commands, the chip, the View
button, and the first press of a send fix in the strip when it needs a
confirm.

A new active check also opens the pane, without the keyboard, so the next
prompt still goes to the composer. The mod does not open it when it is already
open, or after the person closes it by hand in this session (`/clear` resets
this). The host draws a pane that opens this way only from 144 terminal columns
(110 if the person opened it before). Below that, the pane waits undrawn, and
the strip and the chip still show the check.

In the terminal, the host docks the pane on the right only in the fullscreen
layout (`/tui fullscreen`, or `"tui": "fullscreen"` in the user settings). In
the classic layout, the pane shows inline below the output. The launch flag
`--setting-sources project,local` skips the user settings, so it also skips that
setting.

- **Heading.** `Checks` with the count of active checks.
- **Groups.** Active checks show first. `Awaiting verification` holds the
  watched checks. `✓ Resolved` (`Resolved` on the desktop) is folded. To open
  it, press its heading in the terminal or its chevron on the desktop. The pane leaves out checks that build
  evidence, checks that are quiet, and checks that you ignore.
- **Cards.** Each card shows the check name, its metric and the first sentence
  of its reason. Only one card is open at a time. An open card shows the full
  reason, a `Cause` line when the check has one, the fix block, and the
  actions. A watched card shows `Checking the next turns.` in place of the
  actions. A resolved card shows `✓ Fixed` or `✓ Stopped by itself`.
- **Fix block.** A fill fix that is one command shows one line, such as
  `Fills /model claude-opus-5. You send it.` The K1 advice shows
  `Next time, start with /<skill>. It may take fewer steps.` Other fixes show
  their text under `Fix prompt`, `Prompt` or `Compact instructions`. Text
  longer than about 180 characters shows `Show all` and `Show less`.
- **Empty pane.** Before the first check: `No checks yet`. After a check showed
  and none is active: `Nothing to fix right now.`
- **Footer.** A link: `Get more from every session with antiburn app ↗`, to
  https://antiburn.com. While the demo tour runs, the footer also shows
  `Demo tour · step N of 4` and a `Next step` or `End tour` button.

| | Terminal | Desktop |
| --- | --- | --- |
| Rows | A coloured `●` and the check name, a metric line, the reason. | A card with a tinted icon tile. Metrics and skill names draw as SVG. |
| Open a card | The `▸` button at the right (`▾` when open). The first row has the focus when the pane opens, so Enter opens it. The pane scrolls the open card to the top. | The chevron at the top right of the card (`Show more`, `Show less`). |
| Card actions | `f <fix label>` and `h Ignore this session`. The `f` and `h` keys work on the open card. | The fix button. `Ignore this session` shows on the open card. Cards have no hotkeys. |
| Counts | Dim numbers. | Round badges beside each section heading. |
| Background | none | The antiburn mark, behind an empty pane or under the cards. |

## Commands

The mod registers `/antiburn` with the hint `[demo | demo tour | demo off]`.

| Command | Action |
| --- | --- |
| `/antiburn` | Opens the checks pane. |
| `/antiburn demo` | Shows a sample view with all eight checks and opens the pane. Five are active (E1, D, T, K1, K2), one is watched (X1) and two are resolved (O by its fix, C1 by itself). The sample stays until `demo off`. `demo states` is an alias. |
| `/antiburn demo tour` | Starts a tour of 4 steps on an empty, separate set of checks, and opens the pane. |
| `/antiburn demo next` | Plays the next tour step as one turn. The `Next step` button in the pane does the same. |
| `/antiburn demo off` | Ends the sample view or the tour, and shows the live checks again. |

Any other argument replies with the usage line.

The tour steps are:

1. A retry loop is the first warning (X1).
2. Three more checks arrive (T, E1, K2). The tour dismisses the strip first,
   so the strip comes back with `(N new)`.
3. The context gets heavy, and the retry loop stops (D with its cause, X1
   resolves).
4. A skill goes unused, and edits leave the plan (K1, C1).

The tour does not show O, because O fires only on the first turn. The tour
uses its own set of checks. The real hooks keep feeding the live checks while
a demo shows.

**Caution:** a fix in a demo or a tour acts for real. It sends the prompt to
Claude, fills the prompt box, or compacts the session.

## Checks

The thresholds come from `CATALOG.live` and `CATALOG.policy` in
`hooks/catalog.ts`. Only the main thread counts, except for X1. A check is
tested at the end of each main turn.

| ID | Name | Fires when | Fix |
| --- | --- | --- | --- |
| X1 | Retry loop | The same tool call fails 3 or more times in one turn of the main thread or of a subagent. A success of the same call resets its count. The call matches on its input, without the `description`, `tool_use_id`, `agentId` and `consent` fields. Metric: `3 tries · Bash`, with `· subagent` for a subagent. The check comes back as new at 5, 10 and 20 tries. | `Ask Claude to fix`: sends a prompt that asks Claude to stop the retry, name the cause and change the approach. |
| D | Context heavy | One request in the turn carries more than 400,000 tokens (input, cache read and cache creation), or two context readings in a row are at 90% or more. A context reading between turns also updates it. Metric: `Nk tokens per request` or `N% of context used`. When file reads are 50% or more of the context (an estimate at 4 characters per token, since the last compaction), the card adds `Cause · File reads are about N% of it (Nk tokens).` | `Compact now`: compacts the session with instructions to keep the task, the decisions, the open questions and the files in progress. With a file-read cause, the instructions also keep the findings and file paths and drop the raw file contents. |
| O | Obsolete model | At the end of the first main turn, the main model has a newer replacement in `policy.modelReplacements` (for example `claude-opus-4-8` to `claude-opus-5`). The model key ignores a provider prefix, a date suffix and case. After the first turn, an open O goes quiet. Metric: `opus-4-8 → opus-5`. | `Switch model`: fills `/model <replacement>` in the prompt box. |
| T | Overthinking | After 3 or more main turns, at least 5 main requests use an effort above the cap (`max`, `ultra` or `xhigh`), these are at least 50% of the requests that name an effort, and the latest request is still above the cap. A numeric effort does not count. Until then, T builds evidence. Metric: `6 of 8 requests at max`. | `Lower effort`: fills `/effort high`. |
| E1 | Reading spiral | In one main turn, Claude makes 20 or more successful read calls over 10 or more files or folders, and no successful edit. Reads are `Read`, `Grep`, `Glob`, `LS`, `NotebookRead`, and a `Bash` call that Claude Code marks read-only or whose every part runs a read program (`cat`, `head`, `tail`, `less`, `nl`, `wc`, `ls`, `tree`, `find`, `fd`, `rg`, `grep`, `ag`, or `sed -n` without `-i`) with no redirect into a file. Metric: `24 reads · 0 edits · 17 files`. | `Ask Claude to focus`: sends a prompt that asks Claude to state its findings, its hypothesis and the files it will change, then change them or ask one question. |
| K1 | Skill not used | The first main turn after your request takes 10 or more steps, the request does not start with `/`, and no skill or slash command ran for it. The mod then reads the list of installed commands (without the built-in commands and `/antiburn`) and looks for a match: the request names the skill as `/name`, names a dashed skill name in words, or shares 3 or more keywords with the skill description. Metric: `/name fits · N steps`. When the request names `/name` after its first word, the metric is `/name named · not run`, and the reason says that a skill name runs only at the start of a message. | `Got it`: advice only. It sends nothing and sets the check quiet. The card says `Next time, start with /name.` |
| C1 | Outside the plan | After you approve a plan (`ExitPlanMode`), Claude edits 3 or more files whose names are not in the plan text or in your requests from that point. Test files do not count. Metric: `N files outside the plan`. | `Ask about extras`: fills a prompt that asks Claude to list each change outside the plan with its reason, and to keep only the ones you approve. |
| K2 | Repeated workflow | The same 3 or more shell commands, in the same order, run in 3 or more main turns. A command is named by its program and first word, such as `cargo test` or `npm run build`. `cd`, `echo` and read programs do not count. Metric: `same 3 commands × 3`. | `Draft a skill`: fills a prompt that asks Claude to write `.claude/skills/<name>/SKILL.md` for these commands and to show it before it saves it. |

Before K1 matches, the mod removes the context blocks that Claude Code adds to
a message, such as `<system-reminder>`. A machine prompt, such as a loop
wake-up, is not a request.

## Check lifecycle

| Stage | Meaning | Shown in |
| --- | --- | --- |
| building | The check collects evidence (T only). | Nowhere. |
| active | The check fired and offers its fix. | Strip, chip, pane. |
| watching | The fix ran. The mod checks the next turns. | Chip (`⧗`), `Awaiting verification`. |
| resolved | The pattern stopped. | `✓ Resolved` group, and the resolved strip for one turn. |
| quiet | The check no longer applies to the current task. A later match shows it again. | Nowhere. |

A fix moves a check from active to watching:

- **Send and compact fixes** (X1, E1, D) move to watching when the fix runs.
- **Fill fixes** move to watching only when a later request shows the change.
  O moves when a request uses a model with no replacement. T moves when a
  request uses an effort at or below the cap. C1 moves when you send the
  filled prompt unchanged. K2 moves when you send its filled prompt unchanged.

A watched check resolves when its rule clears. Its card then shows
`✓ Fixed`. A check that clears without a fix shows `✓ Stopped by itself`.
If the rule still fires for 3 more turns, the check goes back to active.

Each check clears on its own rule:

| ID | Resolves when |
| --- | --- |
| X1 | A main turn ends with no call that failed 3 times. |
| D | The main session compacts, or the context falls below both limits. |
| O | A request uses a model that has no replacement. |
| T | The latest request uses an effort at or below the cap. |
| E1 | A main turn edits a file. |
| K1 | A later request runs the skill. |
| C1 | 3 turns in a row add no file outside the plan. |
| K2 | Claude writes a `skills/<name>/SKILL.md` file. |

Some checks go quiet: O after the first turn, K1 after `Got it` or a new
request, and C1 after a new request.

## Fix actions and safety

| Kind | Checks | What the press does |
| --- | --- | --- |
| Send | X1, E1 | Sends the fix prompt to Claude as your message. |
| Compact | D | Compacts the session with the compact instructions. |
| Fill | O, T, C1, K2 | Replaces the text in the prompt box. You read it and send it. |
| Advice | K1 | Sends nothing. |

The mod sends to Claude only the fix text from the catalog. It fills in the
replacement model (O), the skill name (K1) or the command names (K2).

**Two-press confirm.**

- A send fix needs two presses when the permission mode is `auto` or
  `bypassPermissions`. It also needs two presses before the mod has seen the
  first permission mode. On a host that an organization manages, Claude Code
  does not run the classic hooks for this mod, so the mod never sees the
  permission mode and every send fix needs two presses. The first press opens the card on its prompt and
  changes the label to `Confirm send`. A first press in the strip also opens
  the pane. The second press sends.
- A compact fix on a card always needs two presses, because a compaction
  cannot be undone. The label changes to `Confirm compact`. The
  `Compact now` button in the strip compacts on one press.
- The armed button takes the primary style. A new prompt, a press on
  `Ignore this session`, or a press that opens or closes a card or a group
  cancels an armed fix.

**Compact.** If the session has too few messages to compact, the mod logs
`Compaction did not run. This session has too few messages to compact yet.`
and the check stays active.

**Ignore this session.** This button on an open active card hides the check
for the rest of the session. The check leaves the strip, the chip count and
the pane. `/clear` or a new session shows all checks again.

**Dismiss.** The `×` button hides the strip only. The chip and the pane still
show the checks.

**Fail open.** Each hook that can block an action has a `.catch` that runs the
rest of the chain. An error in the mod does not block a tool call, a command
or a turn.

## Privacy and resources

- **No network, processes, model calls or timers.** The hooks module does not
  call `$.http`, `$.process`, `$.model` or `$.clock`. CI checks the call list
  that `claude plugin validate` prints and fails if one of these four appears.
  The footer link to https://antiburn.com is a plain link. The mod does not
  fetch it.
- **The host runs the model, and only after a press.** A fix with a prompt
  calls `$.prompt.submit`, and a compact fix calls `$.session.compact`. Both
  ask Claude Code to act. The mod never starts a model call by itself.
- **No files.** The mod writes no file and does not use `$.store`. A test
  checks that a check, its fix and its resolve make no `fs.write`. CI does not
  check `$.store` or `$.fs`. All state ends with the session.
- **No link to the antiburn app.** The mod and the antiburn app share no
  socket, file or call. The app reads the session transcripts that Claude Code
  writes to disk. It does not read the mod state. The two share one thing: the
  policy values in `hooks/catalog.ts` and in `ReportCatalogs` (see
  [Development](#development)).
- **What it keeps.** The check counts live in memory in `LiveChecks`
  (`hooks/checks.ts`). The pane, strip and view state are three session state
  values: `antiburn-live.view`, `antiburn-live.strip` and `antiburn-live.pane`.
  For K1, the mod keeps the first 2,000 characters of your latest request in
  memory. For C1, it keeps the approved plan and the requests after it.
- **What it reads.** Tool call inputs and the size of each result (not its
  text), the model, the effort, token usage, context readings, your prompts,
  the permission mode, and the approved plan. It reads the installed commands
  only after a main turn of 10 or more steps that runs no skill.
- **When it writes state.** At the end of a main turn when a check changed,
  at a prompt or a context reading that changes a check, after a compaction,
  and after a press. The per-step hook makes no engine call and writes no
  state. The `LiveChecks` methods are synchronous and do no I/O.
- **Bounds.** The mod keeps at most 1,000 read targets per turn and 200
  command sets per session. A call key keeps the first 200 characters of a
  long input value.

## Development

From `plugins/antiburn-live`:

```bash
claude plugin validate .
claude plugin test .
npx -y -p typescript@5.6.3 tsc -p . --noEmit
```

- `claude plugin validate .` lists the hooks, the engine calls and the state
  that the mod reads and writes.
- `claude plugin test .` runs `hooks/*.test.ts`.
- The type check needs the declarations in `.claude-plugin/types/`. Claude
  Code writes them each time a session loads the mod from this folder, for
  example with `claude --plugin-dir`. Git ignores them. HEAD has 5 known type
  errors. CI does not run the type check.

The `policy` part of `hooks/catalog.ts` must agree with the engine catalog
(`ReportCatalogs`): the revisions, the depth cap (D), the effort cap (T), the
premium models (S) and the model replacements (O). A test compares each one,
and it fails when `policy` holds a field that has no test. The `live`
thresholds and the `checks` text exist only in the mod, because the engine has
no copy of them. From the repository root, run:

```bash
cargo test --manifest-path crates/antiburn-local/Cargo.toml --test live_catalog_drift
```

The CI job `antiburn-live mod` validates the marketplace and the mod, refuses
network, process, model and timer calls, and runs the tests on Claude Code
2.1.292.

### File layout

| Path | Contents |
| --- | --- |
| `.claude-plugin/plugin.json` | The manifest: name, version, description, and the state types. |
| `hooks/hooks.json` | Loads `register.tsx`. |
| `hooks/register.tsx` | The hooks, the `/antiburn` command, the strip, the chip, the pane, the fix actions and the demo. |
| `hooks/checks.ts` | `LiveChecks`, the check rules and stages. It takes no `$` and does no I/O. |
| `hooks/catalog.ts` | The check catalog: ids, names, sentences, fixes, thresholds and model replacements. Keep the text between the `catalog:begin` and `catalog:end` markers valid JSON. |
| `hooks/tour.ts` | The 4 demo tour steps. |
| `hooks/*.test.ts` | Tests for the checks, the surfaces and the tour. |
| `types/index.d.ts` | The check view and the state types. |
| `types/claude-code.d.ts` | Names the `claude-code` module for tools that read only the committed source. |
| `tsconfig.json` | Extends the generated `.claude-plugin/types/tsconfig.json`. |
| `.gitignore` | Ignores `.claude-plugin/types/`. |

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

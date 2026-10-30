# Bundled desktop promotions

Promotions are bundled content rendered by shared announcement and setup panels.
They are not fetched from a server. An optional YAML `sidebar` block defines a mini panel that opens its enclosing
promotion through the same shared renderer.

## YAML schema version 1

Place each definition at `src/promotions/<folder>/promo.yaml`, with its assets in
that folder. This is the current Claude Mod definition:

```yaml
schema_version: 1
id: claude-mod
revision: 1
analytics:
  campaign: claude_mod
title: antiburn Claude Mod

sidebar:
  title: Claude Mod
  description: Usage, resets, and pace above your prompt.
  link_label: Find out more
  badge: NEW
  icon: claude-code
  priority: 0
  audience:
    type: detected_agents
    agents: [claude-code]

screens:
  announcement:
    eyebrow: Now available
    title: antiburn Claude Mod
    description: Pace your Claude Code usage across sessions.
    media:
      kind: video
      src: ./assets/claude-mod-desktop-dark-no-label-800.mp4
      poster: ./assets/claude-mod-desktop-dark-no-label-poster.png
      alt: Claude Mod showing usage and reset times.
      light:
        kind: video
        src: ./assets/claude-mod-desktop-light-no-label-800.mp4
        poster: ./assets/claude-mod-desktop-light-no-label-poster.png
    features:
      - icon: gauge
        title: Your limits, in view
        description: Five-hour and weekly usage, right above your prompt.
      - icon: clock
        title: Know when to pace yourself
        description: See reset times at a glance and pace your usage when you need to.
    primary_action:
      label: Set up
      type: screen
      target: setup
    secondary_action:
      label: Learn more
      type: external
      url: https://github.com/antiburn/antiburn/blob/main/plugins/antiburn/README.md
  setup:
    title: Set up Claude Mod
    description: Run these two commands in your terminal.
    steps:
      - title: Add the antiburn marketplace
        command: >-
          claude plugin marketplace add https://github.com/antiburn/antiburn.git
      - title: Install Claude Mod
        command: >-
          claude plugin install antiburn@antiburn --scope user
      - title: Open Claude Code
        description: >-
          Run claude in terminal or desktop – antiburn should now appear above
          the prompt.
    secondary_action:
      label: Learn more
      type: external
      url: https://github.com/antiburn/antiburn/blob/main/plugins/antiburn/README.md
```

### Fields and constraints

Unknown keys and duplicate YAML mapping keys are rejected. All text fields must
contain non-whitespace text. Content is plain text, not Markdown or HTML.

| Field                  | Contract                                                                                                                         |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `schema_version`       | Required; exactly `1`.                                                                                                           |
| `id`                   | Required; lowercase kebab-case, unique across the catalog.                                                                       |
| `revision`             | Required; positive safe integer. Together with `id`, identifies a seen announcement.                                             |
| `title`                | Required; catalog title, separate from each screen's title.                                                                      |
| `development_only`     | Optional boolean. Production builds exclude definitions marked `true`.                                                           |
| `analytics`            | Optional mapping containing only `campaign`; currently only `claude_mod` is registered. Omit it for no promotion events.         |
| `sidebar`              | Optional sidebar mini panel. Requires `title`, `description`, and `audience`; see the sidebar schema below.                      |
| `screens.announcement` | Required; requires `title` and `description`. Optional `eyebrow`, `media`, `features`, `primary_action`, and `secondary_action`. |
| `screens.setup`        | Optional; requires `title`, `description`, and a `steps` list. Allows `secondary_action`.                                        |
| `features[]`           | Requires `icon`, `title`, and `description`. Icons are `gauge` or `clock`.                                                       |
| `steps[]`              | Requires `title`; optionally provides either `command` or `description`, never both. Commands are copied, never executed.        |

The schema permits empty feature and step lists. Keep content short and check the
fixed panel at small window sizes; validation does not enforce a copy-length budget.

### Sidebar mini-panel schema

Omit `sidebar` for a promotion with no sidebar entry. Its announcement remains
available to the automatic display flow. When present, `sidebar` accepts only:

| Field         | Contract                                                                                                                                                                                                                                |
| ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `title`       | Required nonempty plain text for the card heading.                                                                                                                                                                                      |
| `description` | Required nonempty plain text below the heading.                                                                                                                                                                                         |
| `link_label`  | Optional nonempty plain text appended inline with an arrow. Omission hides both.                                                                                                                                                        |
| `badge`       | Optional nonempty plain text, such as `NEW`. Omission hides the badge. Viewing, clicking, and restarting do not remove it.                                                                                                              |
| `icon`        | One optional string: an existing agent slug, such as `claude-code` or `codex`, or the generic value `megaphone`. Defaults to `megaphone`; unknown values are rejected. Icon choice does not restrict the audience.                      |
| `priority`    | Optional safe integer, including negative values. Defaults to `0`; higher values win.                                                                                                                                                   |
| `audience`    | Required mapping: `{ type: all }` or `{ type: detected_agents, agents: [claude-code, codex] }`. Detected-agent lists must be nonempty and use known agent slugs; any matching agent qualifies. The `all` variant accepts no agent list. |

Agent slugs come from `AGENT_SLUGS` in `src/lib/presentation/agents.ts`.
`icon` is a single scalar, not separate agent/generic fields or an asset path.
A vendor-neutral card can omit its icon and badge:

```yaml
sidebar:
  title: A new feature
  description: Discover what is available.
  link_label: Find out more
  priority: 10
  audience:
    type: all
```

This card uses the generic megaphone and qualifies without agent detection.
A vendor icon is also allowed with `type: all`; targeting is independent of artwork.

### Actions

Both announcement action slots and the setup secondary action accept these shapes:

```yaml
# Navigate within this promotion. A setup target requires a setup definition.
type: screen
label: Set up
target: setup # announcement is also supported
```

```yaml
# Open a repository page in the external browser.
type: external
label: Learn more
url: https://github.com/antiburn/antiburn/blob/main/plugins/antiburn/README.md
```

```yaml
# Open the supported app destination.
type: app
label: View usage
target: settings
pane: usage
```

External links must use credential-free HTTPS on `github.com`, under
`/antiburn/antiburn/`. Adding another destination requires updating the shared URL
validator and native opener scope together. Arbitrary app routes are not supported.

### Media

`media` requires `kind`, `src`, and `alt`. `kind` is `image` or `video`; videos
also require `poster`. An optional `light` mapping has its own `kind`, `src`, and
optional `poster` (required for video), and shares the base `alt` text.

- Images: `.png`, `.jpg`, `.jpeg`, `.webp`, `.gif`, `.svg`.
- Videos: `.mp4`, `.webm`, `.mov`. These are accepted extensions, not a guarantee
  that every codec works in each platform's webview.
- Paths start with `./` and stay inside the definition folder. Remote URLs,
  parent traversal, backslashes, query strings, fragments, missing files, and
  symlinks escaping the folder are rejected.

The base media is the dark-mode and legacy fallback. Light mode selects `light`
when present. Theme changes replace the source and poster together and clear any
previous playback failure. Videos are muted and loop inline. Window focus changes
do not reset playback. Reduced motion pauses video at the start and shows its
poster. Images with a poster also use it for reduced motion. Media load failures
fall back to the supplied poster.

Claude Mod uses label-free H.264 MP4s and matching PNG posters for both themes:
800 × 416, 50 fps, 12.8 seconds, no audio, with fast-start metadata.

## Build and runtime flow

1. The Vite catalog reads immediate subfolders in `src/promotions`, sorted by
   folder name, and parses each `promo.yaml`.
2. The validator checks the schema, supported actions, and unique IDs. The
   catalog resolves included assets and imports them as separate hashed files.
3. `virtual:promotions` exports the bundled definitions. The shared host renders
   their content; there is no runtime YAML fetch or command execution.
4. The session selects the first unseen, non-development promotion when the
   main window is ready, visible, and in the foreground. It selects at most one
   automatic promotion per session instance.
5. Actual presentation marks the ID/revision pair as seen. Closing the panel
   does not immediately advance to another promotion.

For new users, completing the main-window first-run experience starts a local
24-hour delay. Automatic display also requires a later app session. Foreground
return rechecks eligibility. Existing users without a timing record are eligible
immediately after readiness checks. Resetting onboarding starts the delay again.
A revision bump does not bypass this gate.

Local state belongs to the app profile:

| Key                                      | Purpose                                                                     |
| ---------------------------------------- | --------------------------------------------------------------------------- |
| `antiburn.promotions.seen.v1`            | JSON list of seen `id@revision` pairs. App version changes do not reset it. |
| `antiburn.promotions.ftue.v1`            | First-run completion time used by the automatic delay.                      |
| `antiburn.promotions.launcher-opened.v1` | Launcher activation history; does not control the NEW badge.                |

### Sidebar selection and opening

The sidebar shows one eligible card above Settings. A definition qualifies when it
has `sidebar` content, is not development-only, and its audience matches. The
highest `sidebar.priority` wins; ties use ascending promotion ID, independent of
folder order. A missing or ineligible higher-priority card lets the next eligible
card appear. If none qualify, no card appears. Selection updates from existing
session-count and live-detection subscriptions, without additional scans or
provider refreshes. It does not depend on seen or launcher activation history.

The card opens its enclosing definition's `screens.announcement` by passing that
definition's top-level `id` to `openFromLauncher`. No separate target or React
mapping is needed. Adding the next promo with a higher-priority eligible `sidebar`
block replaces the current card; removing that block reveals the next eligible
card. Keep titles, badge text, and inline labels short and check narrow windows.

The shipped Claude Mod definition targets `claude-code`. It uses the same shared
Agents detection: sessions, installation/sign-in, or Claude Desktop. Pi-only
sign-in does not qualify. Agent visibility switches do not hide it. Unknown or
failed detection without other evidence keeps it hidden. For agents without a
provider meter, discovered sessions establish detection.

The card is hidden during onboarding and becomes available immediately after
completion, bypassing the automatic delay for deliberate opens. It keeps the
selected app view. Sidebar priority and audience apply only to the card; automatic
announcement selection, display timing, and seen-state persistence are unchanged.

Pointer opening reveals the panel from the bottom-left corner over 180 ms.
Keyboard opening is immediate; reduced motion uses a 100 ms fade. Manual dismissal
restores focus to the launcher. Navigating between screens does not replay the reveal.

## Analytics

`analytics: { campaign: claude_mod }` opts into the existing consent-gated
`antiburn.promotion_interaction` event. Development-only definitions never emit it.

| Label                  | Successful trigger                  |
| ---------------------- | ----------------------------------- |
| `announcement_viewed`  | Announcement is actually presented. |
| `setup_viewed`         | Setup is actually presented.        |
| `primary_activated`    | Primary action completes.           |
| `learn_more_activated` | Secondary action completes.         |
| `command_copied`       | Clipboard write succeeds.           |
| `dismissed`            | Reader explicitly closes the panel. |

Each label is emitted at most once per opening: up to two screen views and four
action labels. `origin` remains `automatic` or `user` throughout that opening,
including asynchronous actions. Hidden or blocked screens and failed actions do
not report success. These events measure exposure and setup intent; copying a
command does not prove installation or later use.

YAML cannot supply arbitrary event names or properties. The payload excludes
promotion IDs, revisions, content, commands, URLs, asset paths, and work data.
Register new campaigns in the schema/type, IPC type, Rust enum and fixed wire
mapping, tests, and measurement documentation together.
See the [measurement contract](../../../docs/analytics-measurement.md#claude-mod-promotion-implemented)
and [analytics architecture](../../../docs/analytics.md).

## Authoring and verification

1. Add a definition and local assets. Use synthetic content for test fixtures.
2. Choose a stable ID. Increment `revision` only when the promotion should
   announce again to users who have already seen it.
3. Run the catalog, session, timing, panel, and media tests from `apps/desktop`:

   ```sh
   node node_modules/vitest/vitest.mjs run tests/promotionCatalog.test.ts src/lib/promotions/PromotionSession.test.ts src/lib/promotions/PromotionTiming.test.ts src/components/promotions/PromotionPanel.test.tsx src/components/promotions/PromotionMedia.test.tsx src/components/promotions/PromotionLauncher.test.tsx
   node node_modules/@playwright/test/cli.js test --config tests/visual/promotions.config.ts
   ```

4. Run the [desktop development checks](../README.md#commands) and inspect
   both themes, hover/focus states, small windows, reduced motion, media errors,
   copy feedback, and external/app destinations.
5. Test-mode Vite configuration generates the dummy via
   `tests/promotionFixture.ts` under ignored `.agent-artifacts/promotions/`.
   Its YAML and assets stay uncommitted and outside production imports.

To repeat automatic display in a debug app, open main-window developer tools
(`Command+Option+I` on macOS), clear only the seen history, then reload or restart:

```js
localStorage.removeItem("antiburn.promotions.seen.v1")
```

This does not bypass onboarding or the 24-hour delay. The persistent NEW badge
needs no reset. Use the synthetic timing fixtures to test delay boundaries rather
than changing real-user onboarding state.

The implementation contracts live in [types](../src/promotions/types.ts),
[validation](../src/promotions/validate.ts), the
[catalog plugin](../scripts/promotionCatalog.ts), and
[session lifecycle](../src/lib/promotions/PromotionSession.ts).

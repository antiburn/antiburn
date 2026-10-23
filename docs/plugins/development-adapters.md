# Unpublished adapters — contributor testing only

Codex, Cursor, OpenCode and Pi have local development candidates, not public releases. The procedures below support isolated contributor testing. They do not establish public availability, supported native surfaces or parity with the Claude usage mod. Host versions record the candidate procedures; revalidate them before each vendor rollout.

For the current Claude rollout, use the [installation guide](installation.md). For the required documentation updates when another vendor ships, follow the [vendor rollout checklist](distribution.md#vendor-rollout-documentation-checklist).

## Prepare local candidates

Build with `node scripts/assemble-antiburn-core-plugin.mjs` and review package contents and hashes before enabling executable adapters. The build scripts and default `.agent-artifacts/antiburn-core-plugin/` output use the internal core plugin name; the product name is antiburn plugin.

Examples use `/absolute/path/to/generated-plugins` as a placeholder for that generated output directory. Replace it with the actual absolute path. Choose one source and scope; use temporary profiles and disposable projects. Preserve unrelated installations, settings, skills and credentials.

Help does not require the antiburn desktop app, executable or account. Automatic
predefined checks require the new CLI delivery mode and app-produced saved
results. Reads do not launch the app or evaluate checks. See
[session checks](session-checks.md) for protocol and acceptance coverage. Model-mediated commands still need normal host model access and may consume capacity. Check help/status, plugin version, `App connection: Not checked`, reload and removal. A missing executable is an expected state for discovery adapters.

## Codex

The local package includes a `UserPromptSubmit` command hook for automatic
check delivery, plus a skill for explicit help. Review and trust the executable
hooks in the isolated profile. CLI 0.163.0 requires separate native hook
registration: run `node /absolute/path/to/generated-plugins/codex/hooks/session-check-config.mjs --host codex`,
then review and merge the printed fragment into that profile’s `hooks.json`.
Package enablement alone did not activate its bundled hooks in native acceptance.
No startup turn is created.

```sh
codex plugin marketplace add /absolute/path/to/generated-plugins/catalogs/codex
```

On Codex CLI 0.163.0, install with `codex plugin add antiburn@<catalog-name>` and inspect with `codex plugin list`. Use a dedicated temporary `CODEX_HOME` during testing. In the supported desktop client, find the catalog entry in Plugins, inspect it, and install/enable it; desktop installation is a separate acceptance gate. Invoke through `/skills` or its installed `$` name. Record the actual qualified spelling; a native `/antiburn` slash command is not promised.

Repository catalogs use `.agents/plugins/marketplace.json`, with source paths relative to the marketplace root. A trusted project's `.codex/config.toml` can control a matching local plugin key. Merge only owned entries; preserve other config. Workspace/admin policies can restrict installation.

On CLI 0.163.0, remove with `codex plugin remove antiburn@<catalog-name>`. Disable/remove through the desktop client's package controls when installed there. For a manually owned catalog, remove only its entry and unchanged artifact after disabling. Do not install the test package into this running chat's shared profile. Loose-skill installation needs its own client-specific location and precedence validation.

## Cursor

The UI’s From Local Repository importer requires the generated catalog root to be a real Git repository with its own commit. Importing an ignored catalog subdirectory from another repository can appear successful before a Git-fetch error appears. Use a disposable Git snapshot for acceptance and verify the loaded detail page after installation. Use a distinct catalog identity when changing sources; reimporting the same identity can retain its previous source.

For local testing, copy the complete assembled Cursor artifact into the isolated profile's `plugins/local/antiburn` directory. The normal user location is `~/.cursor/plugins/local/antiburn`; do not modify that live location during acceptance. Include the hidden `.cursor-plugin` directory. External symlinks may be skipped.

Restart the isolated client or run Developer: Reload Window. Inspect Customize and select `/antiburn` in Agent chat. Local imports need organizational permission; do not bypass policy. A same-name marketplace install can take precedence over the local copy.

Once a verified listing exists, use Customize → listing → Install → user or project scope. A team admin can import the generated catalog through Dashboard → Plugins & MCPs → Team Marketplaces, review it, and expose it as Default Off. This implementation creates no listing or team import.

Update only the owned local artifact; remove only that directory and reload. Marketplace installs use Customize controls, subject to policy. The local candidate includes a `postToolUse` command hook. It can deliver saved
check updates after a tool result; it does not wake an idle conversation. Actual
native-route acceptance is recorded separately from explicit command use.
For Cursor Agent 2026.05.24-dda726e, run
`node /absolute/path/to/generated-plugins/cursor/hooks/session-check-config.mjs --host cursor`
and review/merge the fragment into the disposable project’s `.cursor/hooks.json`.
Plugin discovery alone did not register bundled hooks in that CLI build.
Preserve existing hooks and use normal host trust controls.

## OpenCode

Install the explicit command asset separately from the runtime candidate. Copy `opencode/commands/antiburn.md` into one scope:

- User: `~/.config/opencode/commands/antiburn.md`.
- Project: `.opencode/commands/antiburn.md`.

Use a disposable project/profile first. Refuse a conflicting file. Start a new TUI session and invoke `/antiburn`. A prompt-based response can consume model capacity.

For the opt-in runtime candidate, merge this entry into the selected scope's existing `opencode.json` `plugin` array, using your actual absolute file URL:

```json
{
  "plugin": ["file:///absolute/path/to/generated-plugins/opencode/runtime/antiburn.mjs"]
}
```

Preserve the adjacent `session-checks.mjs` and `lib/` files. Keep the runtime
outside auto-discovered plugin directories so helper modules are not loaded as
plugin factories. The candidate targets `@opencode-ai/plugin` SDK 1.19.2 and uses the primary-session
`experimental.chat.messages.transform` callback on loopback servers. Runtime
acceptance uses the host build recorded in [session checks](session-checks.md);
SDK validation and TUI visibility remain separate gates. Use a dedicated `HOME` and XDG config/data/cache/state directories for acceptance. Remove only this array entry to disable the candidate. For npm installation, merge only the verified pinned package into the existing `plugin` array; no published package is assumed here. An npm plugin does not automatically install command assets.

If startup notice behavior is unverified, use the explicit command and report that limit. Update/remove owned command/runtime files and config entries only; preserve unrelated JSON, commands, and the shared cache. Registry publication and name ownership are separate gates.

## Pi

Load the complete artifact through its entry for one-session testing:

```sh
pi --extension /absolute/path/to/generated-plugins/pi/extensions/antiburn.mjs
```

Keep adjacent shared files. Alternatively install the local package:

```sh
pi install /absolute/path/to/generated-plugins/pi
pi list
```

Set `PI_CODING_AGENT_DIR` to a temporary profile and use a disposable working directory during testing. Personal scope is the default. Explicit `--local` installation writes project declarations and needs project trust. Review source before granting trust.

Invoke `/antiburn` for help/status and verify the notice without a model request. Startup belongs at `session_start`, not factory discovery. Automatic check
context uses `before_agent_start` for user messages only. Reload must not duplicate the same session's notice. See the artifact for presentation overrides.

Remove with `pi remove <exact-installed-source>` in the matching scope, restart, and verify command removal. One-session users stop supplying the flag. On Pi 0.80.3, `pi update --all` reconciles installed packages; a pinned version/ref needs an explicit new selection to upgrade.

## Updates, rollback, and evidence

Record artifact hash/version, source, scope, host build, OS, surface, activation, result, and removal. For executable availability discovery, use a synthetic executable that creates
a marker and verify it stays absent. Session-check hooks deliberately invoke
the CLI; use a separate protocol fixture to verify read and acknowledgement
arguments, context delivery and bounded failures. Native color needs visual evidence on light/dark surfaces; a palette function alone does not prove presentation.

Rollback selects a retained verified artifact through the same host install path. Do not mutate a published version. These instructions authorize no tags, submission, publish, or release draft. Plugin removal does not uninstall the desktop app or an existing executable. Session checks use the versioned CLI protocol; executable discovery alone still
does not establish a connection or current assessment.

Sources: [Claude plugins](https://code.claude.com/docs/en/discover-plugins), [Codex packaging](https://developers.openai.com/plugins/build/plugins), [Cursor plugins](https://cursor.com/docs/plugins), [OpenCode commands](https://opencode.ai/docs/commands/), [OpenCode plugins](https://opencode.ai/docs/plugins/), [Pi packages](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/packages.md).

## Cross-vendor visual audit launcher

Run `node scripts/audit-antiburn-core-plugin.mjs prepare` from the source checkout after assembly. It verifies the source inventory and creates a unique temporary audit directory with artifact copies, separate projects/profiles, a Cursor Git catalog, `audit.json`, and `CHECKLIST.md`. Cursor's copied catalog receives a unique name; source hashes describe the original artifacts before this fixture metadata change.

Launch one vendor at a time from an interactive terminal:

```sh
node scripts/audit-antiburn-core-plugin.mjs launch /absolute/audit-dir claude
node scripts/audit-antiburn-core-plugin.mjs launch /absolute/audit-dir codex
node scripts/audit-antiburn-core-plugin.mjs launch /absolute/audit-dir cursor
node scripts/audit-antiburn-core-plugin.mjs launch /absolute/audit-dir opencode
node scripts/audit-antiburn-core-plugin.mjs launch /absolute/audit-dir pi
```

An optional final argument supplies the vendor binary path when it is not on PATH. A per-audit `hosts.<vendor>.binary` entry in `audit.json` selects a persistent local binary; an explicit argument takes precedence. An absolute binary path also puts its directory first on the child's PATH, for runtimes installed beside that binary. No host package is downloaded. Codex's launcher installs the local catalog/plugin into its temporary `CODEX_HOME`. Claude and Pi use one-session flags. OpenCode receives a project command and runtime configuration with isolated HOME/XDG paths. Cursor receives separate user-data and extension directories plus a local plugin candidate. Its local discovery still needs host verification; the checklist includes the manual Git-catalog fallback.

These are separate profiles, not an OS sandbox. Inherited environment values, system credentials and vendor account sync may still apply. The launcher copies no credentials, sends no prompts and captures no screenshots. Sign in manually if needed. A Cursor account import is a manual action that can affect other devices. Codex desktop remains a separate manual gate.

Each launch writes its own `<vendor>-launch.json`; zero exit status never changes visual status from `pending`. Capture and assess the checklist manually. Exit all audit hosts before deleting the temporary directory. The launcher performs no global uninstall or recursive cleanup.

## Claude checks candidate

The core plugin assembler emits `claude-session-checks/` and a separate local
`catalogs/claude-session-checks/` marketplace. Load the candidate with
`claude --plugin-dir /absolute/path/to/generated-plugins/claude-session-checks`
in an isolated profile. It uses `UserPromptSubmit` to deliver saved results
without a skill invocation. The development mod is built separately with
`node scripts/assemble-claude-mod.mjs --dev`; it reads the independent UI
consumer. These local builds do not change the published usage plugin.

# Desktop agent integrations

The desktop app can register the existing JavaScript session-check adapters in
an existing local agent profile. This implementation is gated: **no native route
is enabled in `compatibility.json` yet**. A detected agent therefore remains
`pending_native_gate`; unit tests do not certify native activation.

The implementation depends on the managed CLI lifecycle described in
[CLI installation](cli-installation.md). Startup resolves its validated durable
absolute executable, version, SHA-256, and `delivery-v1` capability. It does not
resolve a managed CLI through PATH. Rust writes the resolved connection into each
vendor's private artifact. Generated command hooks pass its absolute executable
as `ANTIBURN_EXECUTABLE`; each JavaScript runtime also supplies that override in
the CLI child's environment. Pi and OpenCode use the same per-call binding
without changing the host process environment.

Repeated status reads on macOS APFS reuse the binary digest only while its path,
file identity, size, modification time, and change time match. Each read still
checks the receipt and ownership policy. Other filesystems and platforms keep
full binary verification. Setup and Repair always verify the binary in full.

Managed integrations use the receipt-validated absolute CLI path without a terminal
export. Fresh installations do not publish a terminal command; receipt-owned legacy
exports are retired. Unmanaged plugin discovery accepts both `antiburn-cli` and the
legacy `antiburn` export name. CLI settings owns the shared runtime diagnostics.

A synthetic delivery test runs each packaged vendor runtime with an empty PATH,
no terminal shortcut, and an injected absolute `ANTIBURN_EXECUTABLE`, including
acknowledgement. This does not establish native host acceptance. Source and debug
builds do not install agent integrations. Managed integrations keep working
without a terminal shortcut. If the internal executable becomes unavailable,
callbacks fail quietly. Settings offers status refresh and repair, but no integration
removal control.

## Routes and runtime boundaries

| Agent       | Proposed user route                                               | Runtime                      | Current native gate |
| ----------- | ----------------------------------------------------------------- | ---------------------------- | ------------------- |
| Claude Code | `settings.json` hooks: `UserPromptSubmit` and `PostToolUse`       | Discovered absolute Node 22+ | Closed              |
| Codex CLI   | `hooks.json`: `UserPromptSubmit`                                  | Discovered absolute Node 22+ | Closed              |
| Cursor CLI  | Version 1 `hooks.json`: `postToolUse`                             | Discovered absolute Node 22+ | Closed              |
| OpenCode    | One `plugin` file URL in `opencode.json` or `opencode.jsonc`      | Host plugin runtime          | Closed              |
| Pi          | Local package through native `pi install`, `pi list`, `pi remove` | Host extension runtime       | Closed              |

Claude uses direct hooks. It does not install a companion marketplace plugin.
The released public Claude usage plugin, marketplace, and immutable release pin
remain separate. An existing session-check companion or equivalent hook causes
setup to stop for review instead of registering duplicate callbacks.

The runtime payload contains complete adapters and libraries, a Pi package entry,
and the license. It excludes help skills, marketplace files, SDK declarations,
and development tools. `scripts/assemble-agent-integrations.mjs` assembles the
payload during the desktop build; runtime code only verifies and stages it.
Neither startup nor Repair downloads or bundles Node.

Agent profiles honor explicit supported environment overrides. A selected
profile is recorded and does not silently switch when the GUI environment
changes. Discovery checks bounded known locations; it does not scan the disk.
Claude, Codex, and Cursor require a stable numeric Node release of at least 22.
Pi and OpenCode do not depend on a separate Node discovery result.

The host's own version probe can create vendor state. Native acceptance must
verify those probes under the selected profile before any route is enabled.
All development probes, including `--help` and `--version`, must use disposable
HOME and agent-specific profile directories.

On Unix, a version probe can resolve an existing Volta layout-v4 command and its
pinned Node without running the shim. This uses the metadata shape in
[Volta 2.0.2](https://github.com/volta-cli/volta/blob/e83f04ccd79be994c0e65c0bb56838fbcda6fcf9/crates/volta-core/src/tool/package/metadata.rs)
and its [installed paths](https://github.com/volta-cli/volta/blob/e83f04ccd79be994c0e65c0bb56838fbcda6fcf9/crates/volta-layout/src/v4.rs).
The probe does not run Volta hooks or download tools. Missing tools, unknown
metadata, or paths outside the installed image require review. This does not
establish native delivery support or support for other runtime managers.

## Ownership and recovery

State and receipts live under the desktop app data directory's
`agent-integrations` subdirectory. A receipt records the exact target, original
bytes, proposed bytes, inserted fragments, artifact binding, and publication
state. A replacement receipt retains the active receipt until publication
completes. Recovery checks the current config against both receipts to retain
ownership across an interrupted upgrade; changed owned entries require review.
Config and receipt reads reject symbolic links and unsafe file types;
config writes refuse hard links, foreign ownership, writable shared files, and
locked files. Existing macOS files retain permissions, ACLs, and extended
attributes through cloning and atomic exchange. Other platforms' replacement
paths remain disabled pending native implementation and acceptance.
On Windows, receipt and config publication return `platform_native_gate_pending`
before staging or changing destination files. Windows directory durability is
not implemented.
Publication compares the displaced file's bytes, identity, permissions, ACLs,
and extended attributes. Concurrent changes cause restoration or require review.

Direct JSON/JSONC changes preserve original bytes, comments, trailing commas,
unknown keys, and unrelated callbacks. Duplicate keys and conflicting shapes
are refused. Remove reverses exact owned fragments and preserves unrelated
later edits. If a vendor removes or changes an owned entry, startup reports
`user_changed` and does not restore it. Repair is an explicit action. An unchanged
initial prepared transaction is archived so fresh reconciliation validates the
runtime binding again; it does not publish stale prepared paths.

Pi uses its native package manager rather than writing a package reference
directly. Readback must show exactly one string package entry and preserve the
other semantic settings and package order. A user-modified object with extension
filters is refused before removal. An interrupted command leaves a receipt for
inspection. **The external manager has no compare-and-swap contract here**:
readback can detect unrelated changes but cannot prove that a concurrent edit
was not overwritten inside the manager. This remains an acceptance blocker.
Pi package upgrades currently stop with `native_package_upgrade_requires_review`;
they do not silently replace the old package source.

Artifacts are content- and connection-bound. Interrupted staging can resume
only when every existing file matches the expected bytes. Extra files, links,
and changed owned files are refused. Old artifacts are retained; there is no
automatic garbage collection before references and in-flight host use can be
proved absent. Retention is bounded to 16 artifact-directory entries; reaching
the bound requires review and cleanup of unreferenced artifacts.

Settings distinguishes registration, required trust, required reload, disabled,
changed, and unavailable states. Registration does not claim activation. This
implementation does not emit `active`: host context delivery needs separate
native evidence. Status refresh is read-only; repair and removal are explicit
Agents-view IPC actions. CLI Repair emits its resulting state so integration
reconciliation can resume. Settings applies returned snapshots and change events
without another full read. It discards responses from an earlier subscription. Paths and raw
backend errors are not rendered in the Settings descriptions. Installation
analytics are intentionally deferred while
all native routes remain gated; this leaves an acknowledged measurement gap.

## Native acceptance ledger

Observed in a disposable macOS aarch64 profile: Pi 1.4.0 native install, list, and
remove accepted an absolute local package source without downloading a runtime.
Pi stored a profile-relative source and listed its canonical absolute path.
This proves that package-manager operation only. It does not prove extension
loading, context handoff, trust behavior, concurrent edit safety, or another
version/platform. No compatibility entry was enabled from that observation.

Before changing `nativeValidated` for one exact route, record a native report
under the ignored `.agent-artifacts/reviews/` directory with these results:

1. Exact host build, OS, architecture, CLI/desktop surface, profile override,
   selected runtime, packaged app identity, and installation route.
2. Install from a signed/quarantined distribution under a non-admin account;
   minimal GUI PATH; paths with spaces and non-ASCII characters; app relocation;
   restart twice without duplicate registration or config writes.
3. Public Claude usage-plugin coexistence, existing companion refusal, vendor
   policy disablement, trust denial, and reload requirements. Use the native
   host's supported review flow; never bypass trust.
4. A saved ready finding reaches the intended primary session boundary once;
   empty sessions, unsupported routes, subagents, absent CLI, missing Node,
   unavailable app, and removed artifacts fail quietly within bounds.
5. Unrelated comments/settings/hooks survive install, update, repair, and remove;
   vendor removal and edits remain respected; concurrent edits are preserved;
   interruption before and after publication recovers without stale paths.
6. Native file metadata, symlink/hard-link refusal, profile locking, managed or
   read-only config, unknown schemas, remote/unsupported surfaces, and upgrade
   or downgrade behavior. Pi also needs an accepted concurrency/upgrade policy.
7. Retain command output and before/after fixture files without credentials,
   transcripts, usernames, or machine-specific paths in tracked documentation.

Windows, Linux, Codex desktop, and Cursor desktop remain unverified. Existing
project-hook, plugin-directory, or marketplace evidence cannot enable a direct
user-configuration route. Treat newer host versions as pending until accepted.

Native contract references: [Claude hooks](https://code.claude.com/docs/en/hooks),
[Cursor hooks](https://cursor.com/docs/hooks), and the installed Pi package's
`docs/packages.md`. These describe route shapes; the acceptance ledger establishes
the narrower automatic-installation support claim.

## Connection management

Settings → Agents uses one expanded agent row at a time. The name and chevron
open Session locations by default. Integration is the second tab for supported
agents. Its row badge opens that tab directly and has a contextual tooltip.
Visibility switches hide sessions and reports; session processing and automatic checks continue.
Integration offers a status refresh action when setup is supported. Visibility switches remain independent. Rows show the name and a muted account badge on the first line. Desktop-app detection adds “Desktop” before the existing login text; it does not establish a desktop sign-in. No Terminal source is inferred. The second line groups “Searched X locations” and session counts; incomplete discovery says “Searching locations…”. The Integration badge sits directly before the visibility switch, centred across both lines. Desktop-app detection stays in the row badge without a repeated expanded label. Detection still determines switch availability.

Only approval, restart/reload and changed-configuration states show an amber
Integration/warning badge in the inventory. Clicking it opens Integration directly.
Configured, unconfigured, undetected and deliberately disabled setup has no row
badge. The Integration tab retains green check, neutral circle and amber warning
status indicators. These describe setup, not host delivery or check execution.
Shared runtime blockers suppress badges and appear inside the selected Integration
tab. The inventory has no shared setup-availability strip. The panel leads with a
semibold status coloured to match its row badge and muted supporting guidance.
Approval steps appear immediately as a numbered list. Refresh status and Repair
setup appear directly in a quiet maintenance row. Each operation has busy feedback;
Repair says “Repairing…” while pending. A polite inline result confirms completion
even when status is unchanged. Approval and restart requirements remain explicit,
and failures use the existing alert without a success result. The app does not expose integration
removal; internal removal support remains for recovery. Technical details and the
separate usage-plugin note start collapsed. Missing setup files explain the
prerequisite without an unsupported setup action. Refresh or Repair returns
focus to Integration if a status transition replaces the initiating control.
The public Claude usage plugin remains separate. Native compatibility gates stay
closed until acceptance passes.

Settings → CLI owns the shared runtime, its startup test, and repair.
Settings → Agents owns integration status and maintenance. An unavailable
runtime links back to CLI settings. Repair refreshes connection status
through the runtime state event; user-removed integrations stay removed. Removing
a connection does not remove the shared runtime.

Refresh connections inspects current profile/configuration and receipt ownership
without staging artifacts, changing vendor configuration, or repairing receipts.
Closed routes do not run host or Node probes. An accepted route uses disposable
HOME, profile, and XDG directories for its host version probe. The probe runs
outside projects, with `/` as its working directory. Host probes share a
four-second budget. Inspection does not probe Node.
Interrupted installation needs explicit repair. Development builds keep setup
unavailable. Unsupported native routes remain gated.

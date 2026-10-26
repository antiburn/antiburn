# Desktop CLI runtime

Distribution builds prepare a per-user `antiburn-cli` automatically in a background
worker after app state initialization. This is a separate native executable. The
existing `antiburn` desktop/AppImage launcher remains untouched. Installation uses
only the packaged payload: no downloads, compilation, Node installation, network
connection, administrator prompt, or shell-profile edits occur at startup.

`antiburn-cli capabilities --json` returns the CLI package version and
`delivery_protocols: ["delivery-v1"]`. It reads no config or database, starts no
file logging, and creates no profile files. `checks <session_id>` and the existing
`--delivery-v1 --consumer ... --ack ...` arguments retain their existing behavior.
The CLI package version is independent of the desktop version; the payload digest
identifies its exact build. Session/check evidence coverage does not change.

## Paths and ownership

The app data root is Tauri's identifier-scoped per-user data directory. The runtime
lives in `cli/versions/<sha256>/antiburn-cli` (`.exe` on Windows). The stable absolute
native entry point is `cli/bin/antiburn-cli` (`.exe`). POSIX uses a symlink to the
immutable payload. Windows uses a native executable copy, with no `.cmd` shim or
shell invocation. A missing bundle, DMG unmount, App Translocation, app relocation,
or AppImage remount cannot invalidate an already staged runtime.

Terminal exports are separate:

| Platform    | Export                                                             |
| ----------- | ------------------------------------------------------------------ |
| macOS/Linux | `~/.local/bin/antiburn-cli`, a symlink to the durable stable entry |
| Windows     | `%LOCALAPPDATA%\antiburn\bin\antiburn-cli.exe`, a native copy      |

The app neither creates an `antiburn` alias nor modifies PATH. The GUI process PATH
is diagnostic only; it cannot prove a user's interactive shell PATH. Use the
absolute path when the terminal command is not available in a shell.

An advisory OS file lock serializes startup, Repair, Remove, and resolver reads
across Antiburn processes. Lock contention returns immediately with a retryable
busy diagnostic. The lock file remains on disk; its lifetime is the OS handle,
so a crashed process cannot leave a stale held lock.

A schema-checked receipt records the fixed product identity, active and pending
manifests, previous manifest, terminal ownership and removal state. Paths derive
from trusted roots and validated digest components, never receipt-supplied paths.
Readers reject symlinked metadata files, oversized inputs, unknown manifest fields,
wrong targets, invalid digests, unsupported protocols, and changed payload bytes.
No private installation writes occur before the installation directory passes its
ownership and policy checks.

Create an absent destination. Replace only an entry matching the existing
receipt. Preserve unrelated or modified commands and report a collision. POSIX
terminal links use no-overwrite creation. Runtime updates use private app-owned
directories, synced temporary files and atomic replacement; a pending receipt
precedes publication. A failed replacement preserves the existing runtime. The
previous immutable artifact is retained for recovery. A superseded, unchanged
previous artifact is removed by exact digest path. Modified, locked, or orphaned
artifacts from an interrupted publication are preserved for review; the installer
never recursively deletes unknown directory contents.

These checks protect against ordinary collisions and differently owned paths;
they are not an isolation boundary against another process with the same user's
full filesystem authority. An external same-user writer can race ownership checks
and replacement/removal. Only Antiburn writers participate in the advisory lock.

On POSIX, redirected ancestors, other-user ownership, and group/other write access
are rejected (root-owned platform ancestors and sticky temporary roots are allowed).
macOS rejects extended ACLs on managed destinations; Linux rejects extended POSIX
ACLs. Existing policy is preserved rather than relaxed. On Windows, reparse points,
other-user ownership, readonly files, protected/unsupported ACLs and unrelated write
ACEs are rejected on managed destinations. Ancestors permit platform-owned roots,
but reject unrelated ownership or permission to alter existing children. The check
runs again after directory creation. Replacement uses `ReplaceFileW` with policy
merge errors intact.
Locked Windows executables defer replacement; retry on a later launch or Repair.
No chmod/chown broadening or ACL removal occurs in production code.

macOS copies extended attributes onto the staged inode before publication,
including direct quarantine. If the embedded file lacks its own quarantine, the
bounded enclosing `.app` lookup carries its bundle quarantine to the new inode.
The app never strips quarantine. A signature remains embedded in the copied bytes.
Actual Gatekeeper behavior still requires downloaded-build acceptance below.

Source and debug builds never export into the user's shell directory. Explicit
Repair in those builds stays under the identifier-scoped `cli/terminal` directory.
Use disposable profiles for development; do not exercise this against vendor
configuration or the production Antiburn profile.

## Resolver and Settings contract

`cli_install::resolve_cli(&tauri::AppHandle) -> Result<CliRuntime, String>` resolves
only the receipt-validated stable absolute native entry. It never searches PATH or
uses the desktop executable as a fallback. It can return unavailable or busy while
background installation is pending. Call it off the UI thread. `CliRuntime` has:

- `executable: PathBuf`
- `version: String`
- `sha256: String`
- `delivery_protocols: Vec<String>`

JSON uses camelCase (`deliveryProtocols`). Only the Settings window has these IPC
permissions: `get_cli_install_state`, `repair_cli_installation`, and
`remove_cli_installation`. The state contains `runtime`, `terminalPath`,
`terminalInstalled`, `binDirOnAppPath`, and an actionable local `diagnostic`.
Startup emits `cli-install:state` to Settings after reconciliation.

General → Command line shows runtime and terminal status. Repair can restore a
missing owned command; it cannot take over an unrelated destination. Remove removes
only an unchanged owned terminal export, keeps the integration runtime, and persists
that removal. A terminal command removed outside the app also stays absent on future
startup until Repair. A still-present broken owned POSIX link is repairable.

Vendor integration installation, runtime discovery for JavaScript adapters, trust,
reload, native-context delivery and onboarding are separate features. This runtime
contract does not bypass any vendor trust or policy gate.

## Packaging

Build before invoking the desktop bundler, passing the actual target:

```sh
node scripts/build-cli-sidecar.mjs --target aarch64-apple-darwin
pnpm --filter @antiburn/desktop exec tauri build --features distribution \
  --target aarch64-apple-darwin --config src-tauri/tauri.sidecar.conf.json
```

The script supports macOS, Windows MSVC and Linux GNU on x64/ARM64. It passes
`cargo build --locked --release --target <triple> --bin antiburn-cli`, validates
Mach-O/PE/ELF architecture, and records native dependencies. macOS uses `otool`,
Linux `readelf`, and Windows the installed Visual Studio `dumpbin`. Non-system
unbundled dependencies fail packaging. Windows links the CRT statically so the
export does not depend on an unshipped VC runtime DLL.

Release CI signs the CLI first (Developer ID hardened runtime or Authenticode),
then runs `--finalize --target <triple>` to seal the signed bytes' manifest,
then signs/notarizes/packages the desktop app. The payload and manifest are
resources inside installers and updater artifacts, covered by their signatures,
release checksums and attestations. CLI dependencies participate in cargo-deny,
third-party notices, and a separate CycloneDX inventory. Generated payloads are
ignored. Development builds need no generated payload to compile.

## Repeatable acceptance runsheet

Record results in a local audit under `.agent-artifacts/reviews/`. Record exact
app, CLI and installer hashes; app/CLI versions; toolchain/Node versions; OS/arch;
profile root, per-user scope and route; signing/quarantine state; expected and
observed result for each case. Use synthetic fixtures and disposable user profiles.
Do not copy private session data or captured machine paths into public docs.

1. Run CLI formatter/Clippy/nextest, backend formatter/Clippy/nextest, frontend
   lint/type/tests/build, script tests, notices/deny, slop and secrets checks.
2. Build all six targets in CI. Inspect the bundled resource's hash against the
   sealed manifest and the artifact's detached/platform signatures. Verify CLI
   signing precedes app signing/notarization. Verify the Linux glibc floor and
   Windows native dependencies on the actual build hosts.
3. Install offline with minimal GUI PATH in a disposable nonadmin account.
   Confirm stable native `capabilities --json` works independently of PATH, then
   test the terminal's ordinary and minimal PATH separately. Restart twice:
   no duplicates, profile changes, vendor installs, unrelated writes or prompts.
4. Exercise collisions at each destination, missing/broken owned links, changed
   receipts/files, oversized/corrupt payloads, concurrent locks, interrupted
   publication, denied/other-owner/ACL/locked directories, and rollback artifacts.
   Remove the terminal export both in Settings and externally, restart, Repair,
   and verify unrelated `antiburn` and `antiburn-cli` commands remain unchanged.
5. macOS: test the **actual downloaded quarantined signed build** without clearing
   quarantine: launch from DMG, install in Applications, relocate, unmount, and test
   the durable CLI. Record Gatekeeper behavior for both app and exported CLI.
   Confirm no sudo/admin/chown/chmod broadening, Full Disk Access or Accessibility
   request is required by installation. Denials must be actionable and bounded.
6. Linux: stage from an AppImage, unmount/remount or replace it, and execute the
   durable CLI. Windows: hold the old executable open while updating; the old
   runtime must continue to resolve, then retry after releasing the lock. Exercise
   protected ACLs, other-user paths and ordinary nonadmin startup on native hosts.
7. After the separate integration PR is available, assess a synthetic finding in
   the real app → CLI → native vendor context → visible response. Exercise
   dedup/clear/invalidation/lost acknowledgement/malformed input/timeouts/missing
   Node or runtime. Distinguish installation from vendor trust/reload. Verify the
   RELEASE Claude usage mod's payload hash, immutable pin and app/CLI independence.

The ignored backend test
`native_payload_runs_offline_with_a_minimal_path_after_bundle_removal` can run
locally after generating a native payload. It copies into a temporary root,
removes its synthetic bundle, and invokes only capabilities with an empty PATH.
It is not signed-download, GUI, Gatekeeper, Windows, Linux or vendor-context proof.
Installation has local diagnostic tracing and existing Settings visibility
analytics; no dedicated first-party installation-success/adoption event is added.
That intentional measurement gap prevents treating Settings visibility or payload
presence as proof of successful native/vendor execution.

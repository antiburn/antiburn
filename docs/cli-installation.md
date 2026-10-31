# Desktop CLI runtime

Distribution builds prepare an internal per-user `antiburn-cli` automatically in a background
worker after app state initialization. This is a separate native executable. The
existing `antiburn` desktop/AppImage launcher remains untouched. Installation uses
only the packaged payload: no downloads, compilation, Node installation, network
connection, administrator prompt, or shell-profile edits occur at startup.

`antiburn capabilities --json` returns the CLI package version and
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

The runtime is internal infrastructure. Installation and Repair do not publish a
terminal command or change PATH or shell profiles. They do not create an
`antiburn` alias.

The shared `antiburn-cli-runtime` crate owns private-runtime filesystem policy.
The desktop adapter calls it without a Tauri or product-database dependency in
the shared library. The standalone CLI does not provide installation controls.

Reconciliation creates no new terminal-export ownership claims. It retires legacy exports only when a receipt from an earlier implementation records prior
ownership. On macOS/Linux the legacy path is `~/.local/bin/antiburn-cli`; on
Windows it is `%LOCALAPPDATA%\antiburn\bin\antiburn-cli.exe`. Source/debug
profiles use the identifier-scoped `cli/terminal` directory. A POSIX link must
target the stable internal entry; Windows bytes must match the recorded export
digest. Changed or unrelated entries remain untouched. Missing commands are never
recreated, and fresh installations do not create these terminal directories.
Failed cleanup remains retryable on the next launch or Repair.

An advisory OS file lock serializes startup, Repair, Test, and resolver reads
across Antiburn processes. Startup reconciliation retries only lock contention
for up to five seconds, at 50 ms intervals on a background worker, so concurrent
integration readiness reads do not cancel automatic installation. Ownership,
payload and other errors are not retried. Interactive operations and resolver
reads still return immediately with a retryable busy diagnostic. The lock file
remains on disk; its lifetime is the OS handle, so a crashed process cannot leave
a stale held lock.

A schema-checked receipt records the fixed product identity, active and pending
manifests, previous manifest, compatibility export ownership and state. Paths derive
from trusted roots and validated digest components, never receipt-supplied paths.
Readers reject symlinked metadata files, oversized inputs, unknown manifest fields,
wrong targets, invalid digests, unsupported protocols, and changed payload bytes.
No private installation writes occur before the installation directory passes its
ownership and policy checks.

Create an absent destination. Replace only an entry matching the existing receipt.
Preserve unrelated or modified runtime destinations and report a collision.
Preserve changed legacy terminal destinations without claiming them again. Runtime updates use private app-owned
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
The desktop creates new app-data directories with mode `0700`, including under a
group-writable umask such as `002`. Existing directory permissions are preserved;
an existing insecure ancestor still requires the user to correct its policy.
Healthy reconciliation verifies the runtime without rewriting an unchanged receipt.
macOS inspects ancestor ACLs for mutation grants to unrelated principals and preserves safe deny-only/read grants. It rejects extended ACLs on managed destinations; Linux rejects extended POSIX
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

Source and debug builds stage the internal runtime only through explicit Repair. All files stay under the identifier-scoped `cli`
directory.
Run `pnpm --filter @antiburn/desktop dev:cli` for a source launch with the host
CLI payload prepared and resource-mapped, or `dev:bundle:cli` for a debug bundle.
These explicit commands compile the native CLI and then launch/build with both
debug and sidecar configurations. Normal `dev` and `dev:bundle` do not prepare a
payload; Repair requires one. Cross-target development requests are rejected.
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
permissions: `get_cli_install_state`, `test_cli_installation`, and
`repair_cli_installation`. The state contains only `runtime` and an actionable
local `diagnostic`. There is no terminal installation/removal IPC or capability.
Startup emits `cli-install:state` to Settings after reconciliation.

Settings → CLI is available in release and debug builds. It shows the internal purpose and a green Installed status beside the
runtime name. Technical details contains Test, Repair, copyable location and
technical diagnostics. Test results appear separately from Installed. Settings
contains no terminal setup instructions, export status or Remove action.

Test CLI runs only the receipt-validated absolute executable with
`capabilities --json`, null stdin and an empty PATH. It holds the installation
lock during the probe, enforces a three-second deadline and an 8 KiB bound per
output stream, kills failed or timed-out probes, and validates exit status, empty
stderr, JSON shape, version and protocols. A passed result applies only to that
runtime digest; it does not prove shell PATH or vendor integration delivery.
The probe does not read sessions or config, write profile files, repair the
installation, or contact vendors. Copy path uses the Settings-only clipboard
write capability. No path, output, or diagnostic enters product analytics.

Repair restores a missing owned runtime and retires receipt-owned legacy terminal
commands. It preserves
unrelated or changed destinations. Broken owned POSIX links are repairable.

Vendor integration installation, trust, reload, native-context delivery and
onboarding are separate features. Out-of-process JavaScript adapters cannot call
the Rust resolver directly. The separate integration feature must inject the receipt-validated absolute path as
`ANTIBURN_EXECUTABLE` through each generated hook or wrapper and prove delivery for
Claude, Codex, Cursor, Pi and OpenCode with empty PATH. This installer does not
provide a PATH fallback. The adapters currently search
for `antiburn` on PATH when no override is supplied, while this runtime is named
`antiburn-cli`; the explicit override resolves that mismatch. This runtime contract
does not bypass any vendor trust or policy gate.

## Packaging

Build before invoking the desktop bundler, passing the actual target:

```sh
node scripts/build-cli-sidecar.mjs --target aarch64-apple-darwin
pnpm --filter @antiburn/desktop exec tauri build --features distribution \
  --target aarch64-apple-darwin --config src-tauri/tauri.sidecar.conf.json
```

The script supports macOS, Windows MSVC and Linux GNU on x64/ARM64. It passes
`cargo build --locked --release --target <triple> --bin antiburn`, validates
Mach-O/PE/ELF architecture, and records native dependencies. macOS uses `otool`,
Linux `readelf`, and Windows the installed Visual Studio `dumpbin`. Non-system
unbundled dependencies fail packaging. Windows links the CRT statically so the
runtime does not depend on an unshipped VC runtime DLL.

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
   Confirm stable native `capabilities --json` works independently of PATH and no
   terminal export is created. Restart twice:
   no duplicates, profile changes, vendor installs, unrelated writes or prompts.
4. Exercise collisions at each destination, missing/broken owned links, changed
   receipts/files, oversized/corrupt payloads, concurrent locks, interrupted
   publication, denied/other-owner/ACL/locked directories, and rollback artifacts.
   Seed receipt-owned legacy exports, missing exports and changed/unrelated entries.
   Verify reconciliation retires only owned exports, never recreates missing
   commands and preserves replacements. Existing `antiburn` launchers remain unchanged.
5. macOS: test the **actual downloaded quarantined signed build** without clearing
   quarantine: launch from DMG, install in Applications, relocate, unmount, and test
   the durable CLI. Record Gatekeeper behavior for both app and internal CLI.
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
   Run every vendor route with empty PATH and an injected `ANTIBURN_EXECUTABLE`.

The ignored backend test
`native_payload_runs_offline_with_a_minimal_path_after_bundle_removal` can run
locally after generating a native payload. It copies into a temporary root,
removes its synthetic bundle, and invokes only capabilities with an empty PATH.
It is not signed-download, GUI, Gatekeeper, Windows, Linux or vendor-context proof.
Installation has local diagnostic tracing and the privacy-safe
`antiburn.cli_operation_completed` event for startup, Test and Repair outcomes.
It carries only closed operation/result labels. Settings visibility, payload
presence and startup/Test success do not prove successful vendor execution.

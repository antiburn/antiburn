<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# macOS folder access

How antiburn asks for `~/Documents`, `~/Desktop`, and `~/Downloads`, and why the
code is shaped the way it is. Written for whoever next touches discovery, because
most of the rules below are invisible in the code they constrain: they are things
the operating system does, not things a function signature can say.

Everything here is macOS-only. The protected-directory list is empty on Linux and
Windows, so the whole mechanism compiles to "nothing is ever protected" and every
branch below is dead.

## The problem this exists to solve

antiburn is a menu-bar application: no Dock icon, no window at launch. Its scan
scheduler runs a pass immediately at startup, and that pass resolves the working
directories of recorded agent sessions to repository roots — which means running
`git` with a working directory inside whatever folder the session used.

If any session ever ran under a protected folder, that pass touches it. macOS
responds with a permission dialog. The user sees a system alert naming an
application that has not drawn a single pixel, asking for a folder, for no stated
reason. That was a real bug, not a hypothetical: the machine it was found on had
four working directories waiting under `~/Documents`.

So the rule the whole design serves:

> **Nothing reads a protected folder until the user has been told why, and the
> dialog only ever appears in response to something they clicked.**

## What the operating system actually does

Four behaviours drive every design decision here. They are worth knowing exactly,
because each one has a plausible-sounding wrong version.

**`stat` is allowed; enumeration is not.** Checking that a path exists does not
prompt. Reading a directory's contents does. This is why existence checks use
`metadata`/`try_exists` freely, and why `read_dir` is treated as the dangerous
operation. It is also why a folder that simply does not exist never masquerades
as one needing permission.

**A subprocess inherits the responsibility.** Spawning `git -C ~/Documents/foo`
is attributed to antiburn, not to `git`. Handing the path to another program
does not launder it.

**There are three states, not two.** A folder is *allowed*, *refused*, or *never
asked*. Only the third produces a dialog. A refusal is remembered, and the system
answers from that memory instantly and silently — so a second request after a
refusal looks, from inside the app, exactly like an immediate denial with no user
present. That distinction is the whole reason probe timings are recorded.

**Grants are revoked without notice.** The user can switch a folder off in System
Settings at any time, and nothing tells the application. A recorded grant is
therefore a cache that can be wrong, and code that assumes otherwise will report a
folder as readable long after it stopped being so.

## The design

### Partition before touching anything

Every working directory is classified before any filesystem call:
`partition_cwds_by_grants` splits them into admitted and deferred using the
recorded grant set. A path under a protected folder that is not covered by a grant
is *deferred* — never read, never `stat`-ed for repository markers, never handed
to `git`. Deferred entries carry the folder name responsible, so the interface can
ask for exactly that grant.

The engine's resolution path is gated the same way. `resolve_repo_root_with_fallbacks`
checks consent before *every* strategy, including resolving the working directory
itself. An earlier version gated only its third fallback and relied on callers to
pre-filter — which is a convention, not a guarantee, and conventions are how the
original bug happened.

### The dialog is a deliberate act

`request_folder_access` is the only path that intentionally prompts. Two details
in it are load-bearing and easy to delete by accident:

1. **The window is focused first**, and the popover is held open across the call.
   An accessory application with no visible window can raise a dialog *behind*
   everything else, leaving the user waiting on a prompt they cannot see.
2. **The elapsed time is measured around the probe alone.** Opening a settings
   pane or writing to the database first would inflate the one number that
   separates "a human answered a dialog" from "the system answered from memory".

### Grants are cached, and kept honest lazily

The grant record lives in the app database (`consent_grant`, one row per folder
name — the granularity the system actually grants at). It is read at the start of
every pass and trusted as-is.

It is **never** verified by probing. Confirming a grant means reading the folder,
and reading a folder with no recorded decision is precisely what prompts. So
staleness is discovered *reactively*: `verify_dir_access` performs a read the code
wanted to do anyway, and if it comes back denied, records the probe and drops the
grant. The next pass then defers the folder normally.

This matters more than it sounds. An earlier revision of this design used an eager
staleness check and produced permission dialogs from a background process — the
exact failure the whole feature exists to prevent. **Do not add a "just check if
the grants are still valid" pass.**

The corollary is that eviction only happens where a read happens. That is why a
repository already known from a previous pass is *verified* rather than assumed
when it sits inside a protected folder: without that read there is no denial to
observe, the grant is never dropped, and the row keeps claiming to be readable.
Roots outside protected folders skip the check, so the steady-state cost is one
extra `read_dir` only for the rare protected case.

After a pass evicts something, the deferred list is re-derived. Partitioning runs
before any read, so it describes the world as it was at the start of the pass;
without re-deriving, the eviction lands in one pass and the notice explaining it
appears only in the next. Partitioning is pure, so re-running it is free.

### Timing tells you whether a dialog was shown

A refusal returning in under `RECORDED_DENIAL_MS` (500 ms) means the system
answered from a stored refusal without displaying anything. Asking again is
futile; the only way out is System Settings. Above that threshold, a dialog was
shown and answered.

The threshold is generous on purpose: it only has to separate microseconds from
the seconds a human takes to read a dialog. In practice a remembered refusal
returns in 0 ms.

This is tracked **per folder**. One stuck folder must not hide the working control
for the others.

### Who may prompt

| Path | May prompt? |
| --- | --- |
| Scheduled background pass | Never |
| Popover-open and timer passes | Never |
| `request_folder_access` (the notice's button) | Yes — that is its purpose |
| `recheck_folder_permissions` (the "Check again" button) | Yes, but only from that explicit click |

Anything reachable without a click belongs in the first two rows. If you add a
caller of `probe_path_access` or set `probe_protected: true`, it must be behind a
control the user pressed.

### Why there is no Full Disk Access shortcut

Every blocked state antiburn can reach is recoverable through the per-folder
control in Files and Folders. A recorded refusal *creates* that row, and the state
with no row — after a permissions reset — has no recorded decision either, so
asking again produces a real dialog. Full Disk Access would grant mail, messages,
browsing history, and every other application's private data, to an application
that wants three folders and says so on screen. Recorded as D-24 in
`docs/deviations.md`.

## Where the code lives

The split follows the engine's `ConsentGrants` seam: the engine decides *when*
consent matters, the application decides where the answer is kept and how it is
asked for.

| Concern | Location |
| --- | --- |
| Protected-folder list, `cwd_resolution_blocked` | `crates/antiburn-local/src/paths/protected.rs` |
| `ConsentGrants` trait, `partition_cwds_by_grants` | `crates/antiburn-local/src/repositories/consent.rs` |
| `verify_dir_access`, non-prompting existence check | `crates/antiburn-local/src/repositories/access.rs` |
| Consent-gated resolution | `crates/antiburn-local/src/platform/git.rs` |
| Grant storage, probe history, timing classification | `apps/desktop/src-tauri/src/consent.rs` |
| Commands (`request_folder_access`, re-check, diagnostics) | `apps/desktop/src-tauri/src/commands.rs` |
| Partitioning and eviction in the pass | `apps/desktop/src-tauri/src/repositories.rs` |
| Sequential request flow | `apps/desktop/src/lib/useFolderPermissionFlow.ts` |
| The pre-prompt notice | `apps/desktop/src/components/repositories/FolderPermissionNotice.tsx` |
| Dialog text macOS shows | `apps/desktop/src-tauri/Info.plist` |

The grant store is written against the engine's public trait rather than ported
from the private implementation, which is why no source-allowlist rule covers it.

## Testing it

Automated tests cover the logic — the blocked-resolution truth table, partitioning,
eviction, the timing classification, and the request flow's sequencing. They cannot
cover the operating system's behaviour, so the manual pass below is the only proof
that the permission machinery actually works.

**Use a bundled build.** `tauri dev` runs unbundled, so the permission subject is
your terminal, not antiburn, and every result is meaningless.

**Build under a throwaway identifier.** Debug and release currently share
`ai.antiburn.desktop`, so resetting permissions during development also clears them
for an installed copy:

```
pnpm exec tauri build --debug --bundles app \
  --config '{"identifier":"ai.antiburn.tccverify","productName":"antiburn-tccverify"}'
```

Seed a git repository under `~/Documents` and an agent session whose working
directory points at it, mark onboarding complete in the app's database (the launch
pass is gated on it), then:

1. **No unannounced dialog.** Reset the folder permissions and launch. Expect no
   dialog, the repository absent, and the folder listed in
   `internal:deferredPermissionDirs`. This is the original bug's regression test.
2. **Asking works.** Grant access from the notice. Expect the dialog frontmost
   with the popover still open behind it, then the grant recorded and the
   repository appearing.
3. **A remembered refusal is detected.** Switch the folder off in System Settings,
   rescan, then press the button again. Expect no dialog, a 0 ms `recorded-denial`
   in the diagnostics, and the notice offering System Settings instead of a retry.
4. **External grants are noticed.** Switch it back on and press "Check again".
   Expect no dialog and the repository returning.
5. **Revocation is noticed.** With a grant recorded, switch the folder off and
   rescan. Expect the grant dropped, the row marked as blocked, and the notice back
   — all with no dialog.

For step 5, `chmod 000` on the repository is a deterministic substitute: it
produces the same `PermissionDenied` the code reacts to, without involving the
permission system at all.

## Known limitations

**A reset can cost one dialog.** `tccutil reset` removes the decision rather than
recording a refusal. If a grant is still recorded when that happens, the next pass
reads the folder believing it is allowed, finds no decision, and the system
prompts — from a background pass, with nothing on screen. This is inherent to
trusting a cached grant: the only way to avoid it is to verify grants eagerly,
which reintroduces the worse bug. Switching a folder off in System Settings, which
is what users actually do, records a refusal and is handled silently.

**Debug and release share a permission identity.** See above; tracked separately.

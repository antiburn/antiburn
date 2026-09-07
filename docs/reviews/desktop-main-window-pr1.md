# PR 1: desktop main-window review

Review date: 2026-09-07. This is a local review of the first planned change,
not a published pull request or a merge approval.

## Decision

**Ratified as the implementation foundation for PR 2.** The final frontend and
independent native integration reviews found no actionable source blockers.
Architecture, lifecycle, security boundaries, and CI coverage are approved for
the next planned change.

**Cross-platform release acceptance remains conditional.** The platform and
performance gates below remain open. This ratification does not claim that those
checks passed, authorize publication, or start PR 2.

## Reviewed baseline

Base commit: `cdd8a58e5e5084a55b62d4837e5eafcf6066df1e`.
The implementation is an uncommitted working tree. No branch publication,
pull request creation, or PR 2 implementation is part of this review.

The reviewed non-Markdown snapshot contains 39 changed or untracked files.
Its SHA-256 is
`7ca4a09055e9b6816fcab4d56a47135d5dbcb5740b1f0e661dfbb11c14f637d1`.
The digest covers sorted paths from `git ls-files -m -o --exclude-standard`,
excluding Markdown. Each path, byte length, and file content is added in order,
with NUL separators after the path and length. This review record and subsequent
documentation-only ratification edits are outside that snapshot.

## Scope assessment

| Area                          | Assessment                                                                                                                                                                |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Native boundary               | The main-window crate owns window mechanics and geometry; the shell owns launch intent, readiness, persistence, and activation policy.                                    |
| Frontend boundary             | A dedicated entry mounts an empty, accessible themed canvas. Navigation and product views remain deferred.                                                                |
| Existing surfaces             | Tray/popover, Settings, HUD, notifications, and shared monitoring remain in place. The new entry adds no scan or provider polling loop.                                   |
| Ordinary application behavior | Regular macOS activation and Dock presence replace agent-only mode. Main close hides the retained renderer; explicit Quit exits.                                          |
| Geometry                      | 900×600 logical default, 560×420 normal minimum, initial outer frame capped at 85% of usable dimensions. Saved user sizes are preserved.                                  |
| macOS chrome                  | Native traffic lights remain; the custom drag strip supports dragging and double-click maximize/restore through Tauri.                                                    |
| macOS activation              | Previously closed or minimized main windows restore on activation. Startup and other surface interactions have explicit guards.                                           |
| Permissions                   | The main window has a dedicated capability for theme bootstrap, events, readiness, and titlebar actions. It does not inherit the broad interactive-window capability.     |
| Design system                 | The shared semantic tokens remain authoritative. The future sidebar is 220px and collapses below 800px; neither navigation nor view layouts are implemented here.         |
| Performance foundation        | Lazy background creation, retained reopening, a small dedicated entry, and separate cold/first/warm instrumentation are present. Measured performance remains unverified. |

## Validation evidence

Reuse successful checks while their relevant files remain unchanged. This review
does not claim a fresh all-platform CI run.

- Frontend: formatting, lint, type checking, 1,154 tests, production build,
  design drift, and unused-code checks passed during implementation. Subsequent
  fixes changed native behavior and documentation, not the frontend source.
- Native: the initial full shell suite passed with its two sandbox-sensitive
  filesystem watcher tests rerun successfully outside the sandbox. Later changes
  passed their relevant tests, including nine geometry tests and four main-window
  lifecycle tests. Rust formatting and all-target Clippy passed.
- Reporting: reveal-report tests cover sample classification, nearest-rank p95,
  invalid input, insufficient samples, and missed targets. CI includes this script
  and the new crate in its existing check structure.
- macOS packaged debug builds passed. Native checks observed onboarding-to-main,
  a fresh 900×600 main window, double-click expansion/restoration, close followed
  by activation, and actual Command-Tab restoration after minimization.
- Minimized restoration was confirmed with the window server: the main window
  was absent from the on-screen list before activation and present afterward.
  System Events retained a stale `AXMinimized` value and was not treated as the
  authoritative post-restoration result.
- Focused independent reviews approved the geometry, titlebar permission, closed
  activation, and minimized activation changes. The frontend integration review
  found no blocking issues. The final full native integration review approved the
  complete local change, including launch intent, single-instance ordering,
  onboarding handoff, lifecycle recovery, placement, permissions, and CI coverage.

## Open release-validation gates

These are unverified conditions, not successful checks:

1. Run the installed Windows and Linux native matrix, including taskbar grouping,
   switching, close/reopen, quit, startup, display scaling, and Linux compositor
   differences. CI configuration alone does not establish native behavior.
2. Complete the remaining macOS matrix for login startup, Settings/tray/HUD/
   notification interactions, monitor removal, and native keyboard/menu paths.
   Source guards and unit tests do not replace those interaction checks.
3. Measure optimized cold launch, first open, and at least 30 warm reopens.
   Establish native reveal p95 ≤100ms separately from presented-frame and input
   readiness timings. No latency target is ratified by this review.
4. Measure process-tree memory and hidden idle CPU before and after repeated
   hide/show cycles. Retention is intentional; bounded resource use still needs
   measured evidence.

Existing Windows/Linux login registrations without `--background` have a known
upgrade limitation: their first unreconciled launch is indistinguishable from a
manual launch. The updated app rewrites enabled registrations for later logins.
The [validation runbook](../runbooks/main-window.md) documents this behavior.

## Boundary for PR 2

Keep the main-window mechanism, shared services, retained renderer, and capability
boundary intact. PR 2 owns navigation and layout only. Confirm destination names
and order in its IA review before adding views, and preserve compact layout,
keyboard access, theme behavior, and visibility-based presentation work.

Starting PR 2 does not close any release-validation gate listed above.

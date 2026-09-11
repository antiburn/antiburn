# Menu-bar clicks open the main window

Keith approved this plan. Stack the change on PR #493.

## Scope

Replace the anchored report window with direct access to the main window.
Menu-bar clicks open or focus the main window. Remove popover pin controls
and redirect other user-facing paths that can open the anchored report.
Preserve the tray context menu and unrelated notification behaviour.

## Steps

| Step | Status |
| --- | --- |
| Trace window entry points and tray controls | Complete |
| Implement direct main-window opening and regression coverage | Complete |
| Run relevant checks and build | Complete |
| Open stacked PR and verify CI | [PR #496 and live checks](https://github.com/antiburn/antiburn/pull/496/checks) |

## Validation

- All 1,198 native unit tests passed.
- Native Clippy, Rust formatting, frontend formatting/build, design drift,
  full-tree slop, and secret checks passed.
- Native debug bundling and the separate doctest run passed.
- The review build was launched. Automated visual verification was unavailable
  because Computer Use permissions were not granted.
- Confirm manually: repeated primary clicks focus main; secondary click keeps
  the application menu; setup and the menu-bar notification open the right window.

The diff removes obsolete opening APIs and their dedicated tests. Legacy IPC
and lifecycle support remain for a separate removal of the old renderer.

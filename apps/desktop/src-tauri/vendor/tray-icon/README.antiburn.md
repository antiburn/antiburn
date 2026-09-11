# antiburn vendoring notes

This directory vendors `tray-icon` 0.24.1 from upstream revision
`0ada43072646fe4454b1f969f4f446dc401fa1aa`. That revision includes the macOS 27
menu attachment fix from [upstream PR 341](https://github.com/tauri-apps/tray-icon/pull/341).

The local patch adds an opt-in macOS highlight override. The override preserves
AppKit's default pressed behavior when unset. When set, it keeps primary mouse
events from changing the requested state, restores the latest requested state
after native menu tracking, and survives status-item recreation. The
`macos_highlight_override` test exercises the public API and native mouse event
target when `TRAY_ICON_RUN_NATIVE_TEST=1` is set.

The local patch also requires `objc2-app-kit` and `objc2-core-graphics` 0.3.2
or later, matching the safe bindings used by the desktop shell. Redundant
`unsafe` blocks are removed from the macOS implementation and native harness.
The standalone lockfile uses the same binding versions as the shell.

Remove this vendor copy when a compatible upstream release provides equivalent
highlight control and retains the macOS 27 menu attachment fix.

The upstream source remains licensed under MIT or Apache-2.0. See `LICENSE-MIT`,
`LICENSE-APACHE`, and `LICENSE.spdx` in this directory.

# antiburn vendoring notes

This directory vendors `tray-icon` 0.24.2 from upstream revision
`77c96c432cf2785679affc117389104a6e850528`. This is the last upstream revision
before the 0.25 backend and menu dependency changes. It includes the complete
macOS bindings update from [upstream PR 352](https://github.com/tauri-apps/tray-icon/pull/352),
including the dependency lockfile and example API updates.

Tauri 2.11.5 requires `tray-icon` 0.24, `muda` 0.19, and the legacy tray feature
names. Upstream 0.25 changes these interfaces and requires `muda` 0.20. Refresh
past this revision when Tauri supports those interfaces. The Windows GUID API
and the GTK-free Linux/BSD backend are not included in this baseline.

The local patch retains the macOS 27 menu attachment fix from
[upstream PR 341](https://github.com/tauri-apps/tray-icon/pull/341), originally
pinned at `0ada43072646fe4454b1f969f4f446dc401fa1aa`. It attaches the native menu
only during menu tracking so primary clicks continue to reach the tray target.

The local patch also adds an opt-in macOS highlight override. The override
preserves AppKit's default pressed behavior when unset. When set, it keeps
primary mouse events from changing the requested state, restores the latest
requested state after native menu tracking, and survives status-item recreation.
The `macos_highlight_override` test exercises the public API and native mouse
event target when `TRAY_ICON_RUN_NATIVE_TEST=1` is set. The patch uses the safe
bindings from the new baseline in both the implementation and test harness.
The macOS test dependency explicitly enables `NSApplication` and
`NSGraphicsContext` for synthetic mouse events; it does not rely on example dependencies enabling that feature.

Remove this vendor copy when a compatible upstream release provides equivalent
highlight control and retains the macOS 27 menu attachment fix.

The upstream source remains licensed under MIT or Apache-2.0. See `LICENSE-MIT`,
`LICENSE-APACHE`, and `LICENSE.spdx` in this directory.

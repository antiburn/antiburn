# antiburn vendoring notes

This directory is a local `tray-icon` fork based on upstream revision
`77c96c432cf2785679affc117389104a6e850528`. Its package version is `0.25.1` so
Cargo can patch the version required by Tauri 2.12. It is not an unmodified copy
of the upstream 0.25.1 source.

The fork uses `muda` 0.20 and the feature names that Tauri 2.12 requests. Keep
its public tray API compatible with upstream 0.25.1. Upstream's Windows GUID API
and GTK-free Linux/BSD backend are not included because the app does not use
them. Recheck upstream changes before each Tauri/tray update.

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

Keep this fork until upstream provides equivalent highlight control and retains
the macOS 27 menu attachment fix.

The upstream source remains licensed under MIT or Apache-2.0. See `LICENSE-MIT`,
`LICENSE-APACHE`, and `LICENSE.spdx` in this directory.

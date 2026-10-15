// The Rust workspaces that CI checks. The change classifier and the Rust
// check runner both read this list, so a workspace has one definition.
//
// Order matters: the runner checks the workspaces of a job in this order.
// The engine comes before the remote helper, and the shell comes first in the
// desktop group, so the largest build fills the shared target directory early.
//
// - `dependsOn`: path dependencies. A change selects every dependent workspace.
// - `linuxOnly`: the runner skips the workspace on other hosts.
// - `doctests`: the runner adds `cargo test --doc`, because nextest does not
//   run doctests. Set it when the library has Rust doctests.
// - `linuxSystemDeps`: a Linux job installs the Tauri system packages (GTK,
//   WebKit, ALSA, and others) before it checks the workspace.
export const WORKSPACES = [
  {
    id: "engine",
    directory: "crates/antiburn-local",
    group: "engine",
    dependsOn: [],
    doctests: true,
  },
  {
    id: "remote",
    directory: "crates/antiburn-remote",
    group: "engine",
    dependsOn: ["engine"],
    linuxOnly: true,
  },
  {
    id: "shell",
    directory: "apps/desktop/src-tauri",
    group: "desktop",
    dependsOn: [
      "engine",
      "remote",
      "nudge",
      "hud",
      "main-window",
      "anchored-window",
      "sound",
      "trace",
      "tray-icon",
    ],
    linuxSystemDeps: true,
  },
  {
    id: "trace",
    directory: "apps/desktop/src-tauri/crates/trace",
    group: "desktop",
    dependsOn: [],
  },
  {
    id: "hud",
    directory: "apps/desktop/src-tauri/crates/hud",
    group: "desktop",
    dependsOn: [],
    linuxSystemDeps: true,
  },
  {
    id: "main-window",
    directory: "apps/desktop/src-tauri/crates/main-window",
    group: "desktop",
    dependsOn: [],
    linuxSystemDeps: true,
  },
  {
    id: "anchored-window",
    directory: "apps/desktop/src-tauri/crates/anchored-window",
    group: "desktop",
    dependsOn: [],
    linuxSystemDeps: true,
  },
  {
    id: "nudge",
    directory: "apps/desktop/src-tauri/crates/nudge",
    group: "desktop",
    dependsOn: [],
    linuxSystemDeps: true,
  },
  {
    id: "sound",
    directory: "apps/desktop/src-tauri/crates/sound",
    group: "desktop",
    dependsOn: [],
    doctests: true,
    linuxSystemDeps: true,
  },
  {
    id: "tray-icon",
    directory: "apps/desktop/src-tauri/vendor/tray-icon",
    group: "desktop",
    dependsOn: [],
    doctests: true,
    linuxSystemDeps: true,
  },
];

/** The workspace with the longest directory that contains `file`, or null. */
export function owningWorkspace(file) {
  let found = null;
  for (const workspace of WORKSPACES) {
    if (!file.startsWith(`${workspace.directory}/`)) continue;
    if (found === null || workspace.directory.length > found.directory.length) {
      found = workspace;
    }
  }
  return found;
}

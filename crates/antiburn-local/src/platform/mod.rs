// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The engine's boundary with the local operating system.
//!
//! Everything that leaves the process lives here, and nothing here reaches the
//! network. The engine runs exactly two local programs — the user's `git`, and
//! `wsl.exe` on Windows to reach a mounted distribution — through a bounded,
//! window-free child-process mechanism.
//!
//! # Modules
//!
//! - [`process`] — bounded, deadline-enforced, window-free child processes.
//! - [`environment`] — host and mounted-WSL execution environments plus path
//!   translation between them.
//! - [`git`] — read-only local Git identity, worktree, and remote helpers.

pub mod environment;
pub mod git;
pub mod process;

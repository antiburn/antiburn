//! The engine's boundary with the local operating system.
//!
//! Everything that leaves the process lives here, and everything here stays
//! local. The engine runs exactly two local programs — the user's `git`, and
//! `wsl.exe` on Windows to reach a mounted distribution — through a bounded,
//! window-free child-process mechanism; neither one reaches a network.
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

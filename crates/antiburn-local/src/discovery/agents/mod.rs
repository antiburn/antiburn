//! One adapter per supported coding agent.
//!
//! Every module here implements [`AgentExplorer`](super::AgentExplorer) over a
//! vendor's documented on-disk layout or read-only database. The `*_subagents`
//! modules hold the orchestration half for the three vendors that record
//! parent → child spawns.
//!
//! Adapters read; they never write to a vendor's store, never contact a running
//! agent process, and never open a socket.

pub mod amp_code;
pub mod antigravity;
pub mod antigravity_subagents;
pub mod claude;
pub mod claude_subagents;
pub mod cline;
pub mod codex;
pub mod codex_subagents;
pub mod copilot;
pub mod cursor;
pub mod kiro;
pub mod opencode;
pub mod path_codec;
pub mod pi;
pub mod windsurf;

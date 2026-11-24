//! Network-free desktop capture of provider-native session source.
//! The non-default `cloud` feature enables the API.
//! See `docs/cloud/capture.md` for the public byte and transport contract.
//!
//! The desktop host supplies source access, consent, account and destination
//! bindings, a `Transport`, and scheduling. This crate owns admitted source
//! handles, exact saved chunks, queue records, and upload reconciliation.
//! Renderer code must not own credentials, choose upload locations, or call these
//! blocking discovery, capture, or queue operations on UI or async runtime
//! threads. The host supplies bounded blocking workers; this crate owns no runtime.
//!
//! Discover an authorized source, bind each candidate to its stable source scope,
//! then capture with a new identity. Schedule `Spool::resume_step` for pending
//! entries. Persist an accepted fingerprint and terminal request before calling
//! `Spool::acknowledge_accepted`. Stopping an executor never cancels its entries.

#[cfg(feature = "cloud")]
mod codex;
#[cfg(feature = "cloud")]
mod contract;
#[cfg(all(feature = "cloud", unix))]
mod cursor;
#[cfg(feature = "cloud")]
mod directory;
#[cfg(feature = "cloud")]
mod discovery;
#[cfg(feature = "cloud")]
mod filesystem;
#[cfg(feature = "cloud")]
mod header_file;
#[cfg(feature = "cloud")]
mod metadata;
#[cfg(all(feature = "cloud", unix))]
mod opencode;
#[cfg(feature = "cloud")]
mod pi;
#[cfg(feature = "cloud")]
mod project;
#[cfg(feature = "cloud")]
mod protocol;
#[cfg(feature = "cloud")]
mod spool;
#[cfg(all(feature = "cloud", unix))]
mod sqlite_native;
#[cfg(feature = "cloud")]
mod stream;

#[cfg(feature = "cloud")]
pub use codex::reopen_codex;
#[cfg(feature = "cloud")]
pub use contract::*;
#[cfg(all(feature = "cloud", unix))]
pub use cursor::{
    CursorCliDiscoveryPage, CursorIdeDiscoveryPage, discover_cursor_cli_page,
    discover_cursor_ide_page, reopen_cursor_cli, reopen_cursor_ide,
};
#[cfg(feature = "cloud")]
pub use discovery::{Candidate, CaptureSourceKind, discover_claude, discover_opencode};
#[cfg(feature = "cloud")]
pub use discovery::{ClaudeDiscoveryPage, discover_claude_page};
#[cfg(feature = "cloud")]
pub use discovery::{OpenCodeDiscoveryPage, discover_opencode_page};
#[cfg(feature = "cloud")]
pub use discovery::{reopen_claude, reopen_opencode};
#[cfg(feature = "cloud")]
pub use metadata::*;
#[cfg(feature = "cloud")]
pub use pi::reopen_pi;
#[cfg(feature = "cloud")]
pub use project::{PROJECT_RECORD_BYTES, ProjectEvidence, ProjectSelection};
#[cfg(feature = "cloud")]
pub use spool::Spool;

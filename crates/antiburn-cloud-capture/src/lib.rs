//! Network-free native capture. The non-default `cloud` feature enables the API.
//! See `docs/cloud/capture-v1.md` for the public byte and transport contract.

#[cfg(feature = "cloud")]
mod contract;
#[cfg(feature = "cloud")]
mod directory;
#[cfg(feature = "cloud")]
mod discovery;
#[cfg(feature = "cloud")]
mod filesystem;
#[cfg(feature = "cloud")]
mod metadata;
#[cfg(feature = "cloud")]
mod protocol;
#[cfg(feature = "cloud")]
mod spool;
#[cfg(feature = "cloud")]
mod stream;

#[cfg(feature = "cloud")]
pub use contract::*;
#[cfg(feature = "cloud")]
pub use discovery::{Candidate, discover_claude};
#[cfg(feature = "cloud")]
pub use metadata::*;
#[cfg(feature = "cloud")]
pub use spool::Spool;

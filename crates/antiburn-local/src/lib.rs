// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! antiburn-local: the canonical local engine behind antiburn.
//!
//! This crate owns local discovery of AI coding-agent sessions, transcript
//! analysis, repository identity, API-equivalent pricing, and the versioned
//! local persistence/export contracts. It is deliberately self-contained:
//!
//! - **No dependency on any service.** Nothing in this crate talks to a
//!   service of ours or carries telemetry; discovery reads documented files,
//!   read-only databases, and bounded WSL paths.
//! - **No private dependencies.** The crate builds and tests standalone with
//!   its own manifest and lockfile.
//! - **Local concepts only.** The public API carries no authentication,
//!   organization, remote-sharing, enrichment, or telemetry contracts.

pub mod analysis;
pub mod discovery;
pub mod insights;
pub mod model;
pub mod paths;
pub mod platform;
pub mod pricing;
pub mod repositories;

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Repository-discovery tests.
//!
//! Everything here exercises **local** repository identity and user-selected
//! roots: how a clone on disk is recognized, how worktrees and path spellings
//! collapse onto one identity, how bounded traversal behaves, and how the
//! consent and progress seams are driven. Fixture identities are synthetic.

mod bounded_resolution_tests;
mod bounded_root_detail_tests;
mod identity_tests;
mod matching_tests;
mod pipeline_tests;
mod scan_tests;
mod support;

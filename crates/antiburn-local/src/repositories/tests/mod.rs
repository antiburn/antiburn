//! Repository-discovery tests.
//!
//! Everything here exercises **local** repository identity and user-selected
//! roots: how a clone on disk is recognized, how worktrees and path spellings
//! collapse onto one identity, how bounded traversal behaves, and how the
//! consent and progress seams are driven. Fixture identities are synthetic.

mod bounded_resolution_tests;
mod identity_tests;
mod matching_tests;
mod scan_tests;
mod support;

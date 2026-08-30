//! Storage-neutral thread identity, derived from a chain of per-record
//! parent links (Claude's `uuid` / `parentUuid`, falling back to
//! `logicalParentUuid` across a compaction boundary).
//!
//! [`ThreadResolver`] lets a vendor adapter assign each record to a thread
//! without knowing anything about rows or storage: the main loop keeps one
//! thread across a compaction boundary, and an inline sidechain gets its
//! own thread. [`super::rows::turn_row_from_event`] reads the result back
//! off [`super::model::NormalizedEvent::thread_id`].

use std::collections::HashMap;

/// The most distinct uuids [`ThreadResolver`] records before it stops
/// tracking new ones. Real Claude transcripts (measured: 76 sessions) stay
/// orders of magnitude below this; the bound exists only to hold memory for
/// a pathologically long transcript.
const MAX_THREAD_IDENTITIES: usize = 50_000;

/// Assigns each record to a thread by its parent link.
///
/// A record joins the thread of its link target when that target was
/// already seen. A record with no link, or with an unseen target, starts a
/// thread named by its own uuid. A record without a uuid joins the current
/// thread.
#[derive(Default)]
pub(crate) struct ThreadResolver {
    thread_by_uuid: HashMap<String, String>,
    current: Option<String>,
    capped: bool,
}

impl ThreadResolver {
    /// Resolves the thread for one record and records its uuid.
    ///
    /// Rules, in order: (a) `link` names a uuid this resolver has already
    /// recorded — the record joins that uuid's thread; (b) `uuid` is
    /// `Some` — the record starts a new thread named by its own uuid (this
    /// also covers a `link` that names an uuid this resolver has not seen:
    /// the link is unresolved, and the caller's own unresolved-link check
    /// reports that separately); (c) `uuid` is `None` — the record joins
    /// the current thread. A `uuid` this resolver has already recorded
    /// keeps the thread from its first occurrence; a later occurrence with
    /// a different `link` does not move it. Once this resolver has
    /// recorded [`MAX_THREAD_IDENTITIES`] uuids, a record with a new uuid
    /// falls to rule (c) instead of starting or joining a thread, and
    /// [`Self::capped`] reports it.
    ///
    /// Sets the current thread to the result before returning it.
    pub(crate) fn resolve(&mut self, uuid: Option<&str>, link: Option<&str>) -> Option<String> {
        let resolved = match uuid {
            Some(uuid) => match self.thread_by_uuid.get(uuid) {
                Some(thread) => Some(thread.clone()),
                None if self.thread_by_uuid.len() >= MAX_THREAD_IDENTITIES => {
                    self.capped = true;
                    self.current.clone()
                }
                None => {
                    let thread = link
                        .and_then(|link| self.thread_by_uuid.get(link))
                        .cloned()
                        .unwrap_or_else(|| uuid.to_owned());
                    self.thread_by_uuid.insert(uuid.to_owned(), thread.clone());
                    Some(thread)
                }
            },
            None => self.current.clone(),
        };
        self.current.clone_from(&resolved);
        resolved
    }

    /// True once this resolver stopped recording new thread identities
    /// because it reached [`MAX_THREAD_IDENTITIES`]. The caller reports
    /// this as a coverage gap: thread assignment past this point degrades
    /// to "joins the current thread" instead of following the real chain.
    pub(crate) fn capped(&self) -> bool {
        self.capped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_chain_stays_one_thread_across_a_compaction_boundary() {
        let mut resolver = ThreadResolver::default();
        assert_eq!(resolver.resolve(Some("u1"), None), Some("u1".to_owned()));
        assert_eq!(
            resolver.resolve(Some("u2"), Some("u1")),
            Some("u1".to_owned())
        );
        // The compact_boundary record: `parentUuid` is null, so the caller
        // passes `logicalParentUuid` (the last pre-compaction record) as
        // `link` instead.
        assert_eq!(
            resolver.resolve(Some("boundary"), Some("u2")),
            Some("u1".to_owned())
        );
        // The record after the boundary continues from it, resuming the
        // main thread.
        assert_eq!(
            resolver.resolve(Some("u3"), Some("boundary")),
            Some("u1".to_owned())
        );
    }

    #[test]
    fn an_inline_sidechain_root_gets_its_own_thread_and_its_children_follow_it() {
        let mut resolver = ThreadResolver::default();
        resolver.resolve(Some("u1"), None);
        resolver.resolve(Some("u2"), Some("u1"));
        // A sidechain root: no link, even though the main loop is current.
        assert_eq!(resolver.resolve(Some("s1"), None), Some("s1".to_owned()));
        assert_eq!(
            resolver.resolve(Some("s2"), Some("s1")),
            Some("s1".to_owned())
        );
        // The main loop resumes from its own last uuid, unaffected by the
        // sidechain that ran in between.
        assert_eq!(
            resolver.resolve(Some("u3"), Some("u2")),
            Some("u1".to_owned())
        );
    }

    #[test]
    fn a_duplicated_uuid_keeps_its_first_thread() {
        let mut resolver = ThreadResolver::default();
        resolver.resolve(Some("u1"), None);
        resolver.resolve(Some("s1"), None);
        // The duplicate names a different link than its first occurrence
        // did; it must still resolve to its first thread, "u1", not move to
        // "s1" or start a fresh thread.
        assert_eq!(
            resolver.resolve(Some("u1"), Some("s1")),
            Some("u1".to_owned())
        );
    }

    #[test]
    fn a_record_without_a_uuid_inherits_the_current_thread() {
        let mut resolver = ThreadResolver::default();
        resolver.resolve(Some("u1"), None);
        assert_eq!(resolver.resolve(None, None), Some("u1".to_owned()));
    }

    #[test]
    fn an_unseen_link_starts_a_new_thread() {
        let mut resolver = ThreadResolver::default();
        assert_eq!(
            resolver.resolve(Some("orphan"), Some("ghost")),
            Some("orphan".to_owned())
        );
        assert!(!resolver.capped());
    }

    #[test]
    fn a_record_that_arrives_after_the_cap_falls_to_rule_c() {
        let mut resolver = ThreadResolver::default();
        for index in 0..MAX_THREAD_IDENTITIES {
            resolver.resolve(Some(&index.to_string()), None);
        }
        assert!(!resolver.capped());
        let current = resolver.resolve(None, None);

        let over_cap = resolver.resolve(Some("over-cap"), None);

        assert!(resolver.capped());
        assert_eq!(over_cap, current);
        // "over-cap" was never recorded, so a later record naming it as a
        // link cannot resolve through it — and, since this resolver stays
        // capped for good, that later record falls to rule (c) too instead
        // of starting its own thread.
        assert!(!resolver.thread_by_uuid.contains_key("over-cap"));
        assert_eq!(resolver.resolve(Some("child"), Some("over-cap")), current);
    }
}

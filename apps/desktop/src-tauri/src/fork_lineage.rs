//! Links a Claude Code "resume as fork" session to the session that owns
//! its first turns.
//!
//! Claude's resume-as-fork writes no marker. It copies a parent transcript's
//! leading records — `uuid` included — into a new file under a new session
//! id. Nothing in a fork's own file says where it came from.
//!
//! This link is made at publish time, not at describe time. Describe skips
//! re-describing a record whose transcript did not change, and a session's
//! turn rows exist only once a pass publishes them — so a fork pair that is
//! finished, or one first indexed alongside its parent at install, could
//! stop changing before a describe-time lookup ever ran against rows that
//! exist. Publish is the one moment guaranteed to run this lookup for every
//! session, however many times its transcript changes afterward.
//!
//! The link runs in both directions from whichever side just published:
//! outward, to claim an earlier session as this one's parent, and inward, to
//! name this session as parent for a later session that already looks like
//! its child. Only one side is guaranteed to publish after the other side
//! is already linkable, so checking one direction only would miss the pair
//! whose parent finishes after its fork.
//!
//! A newly recorded relation also requeues the child's evidence: the
//! relation changes what the Claude adapter counts for it (it now excludes
//! the parent's records from its own leading prefix), so the child's
//! already-published analysis is stale the moment the relation exists. Only
//! a relation this call actually inserts requeues — [`Store::
//! record_fork_parent`] returning `false` means the relation already
//! existed, so the child was requeued the first time it was recorded and
//! must not be requeued again on every later publish of either side.

use std::cmp::Ordering;

use antiburn_local::model::AgentKind;
use anyhow::Result;

use crate::store::{OwningSession, SessionKey, Store};

/// Orders two candidates that both own the same first turn uuid, so the
/// true original session sorts before every fork of it.
///
/// The earliest-indexed session sorts first: a fork gets its own session id
/// only when the user resumes an earlier one, so the original is always
/// indexed no later than any of its forks. A tie breaks on published turn
/// row count, fewer rows first: a parent the user has left keeps its row
/// count fixed, while a fork the user keeps using grows past it. A session
/// id breaks any remaining tie only to keep the order deterministic — a
/// wrong pick there only changes which side of an otherwise identical pair
/// shows as the parent.
fn rank(a: &OwningSession, b: &OwningSession) -> Ordering {
    a.first_seen_at
        .cmp(&b.first_seen_at)
        .then_with(|| a.published_turn_rows.cmp(&b.published_turn_rows))
        .then_with(|| a.session_id.cmp(&b.session_id))
}

/// Link `key`, a session that just published, to the fork parent its
/// leading turn rows point to, and to any fork of it that already
/// published first.
///
/// Does nothing for a non-Claude key, a session with no published turn
/// row yet, or a session whose first uuid nobody else owns.
pub fn link_claude_fork(store: &Store, key: &SessionKey) -> Result<()> {
    if key.agent != AgentKind::Claude.slug() {
        return Ok(());
    }
    let Some(uuid) = store.first_turn_uuid(key)? else {
        return Ok(());
    };
    let candidates = store.sessions_owning_turn_uuids(key, &[uuid])?;
    if candidates.is_empty() {
        return Ok(());
    }
    let Some(self_stats) = store.owning_session_stats(key)? else {
        return Ok(());
    };

    claim_earlier_parent(store, key, &self_stats, &candidates)?;
    claim_later_children(store, key, &self_stats, &candidates)?;
    Ok(())
}

/// (a) If `key` has no fork parent yet, record the best candidate that
/// ranks strictly before it as that parent — skipping a candidate that
/// already names `key` as its own parent, which would otherwise close a
/// two-session loop.
fn claim_earlier_parent(
    store: &Store,
    key: &SessionKey,
    self_stats: &OwningSession,
    candidates: &[OwningSession],
) -> Result<()> {
    if store.fork_parent(key)?.is_some() {
        return Ok(());
    }
    let mut earlier: Vec<&OwningSession> = candidates
        .iter()
        .filter(|candidate| rank(candidate, self_stats) == Ordering::Less)
        .collect();
    earlier.sort_by(|a, b| rank(a, b));
    for candidate in earlier {
        let candidate_key = candidate_key(key, candidate);
        if store.fork_parent(&candidate_key)?.as_deref() == Some(key.session_id.as_str()) {
            continue;
        }
        if store.record_fork_parent(key, &candidate.session_id)? {
            tracing::info!(
                event = "fork_lineage_linked",
                session_id = %key.session_id,
                parent_session_id = %candidate.session_id,
            );
            // `key` is the child in this relation: it just claimed
            // `candidate` as its own parent.
            store.requeue_session_evidence(key)?;
        }
        return Ok(());
    }
    Ok(())
}

/// (b) For every candidate that ranks strictly after `key` and has no fork
/// parent yet, record `key` as its parent. This is what makes the link
/// appear when a parent publishes after its fork: the fork already ran
/// this lookup and found nothing, so only the parent's own publish can
/// complete the pair.
fn claim_later_children(
    store: &Store,
    key: &SessionKey,
    self_stats: &OwningSession,
    candidates: &[OwningSession],
) -> Result<()> {
    for candidate in candidates
        .iter()
        .filter(|candidate| rank(candidate, self_stats) == Ordering::Greater)
    {
        let candidate_key = candidate_key(key, candidate);
        if store.fork_parent(&candidate_key)?.is_some() {
            continue;
        }
        if store.record_fork_parent(&candidate_key, &key.session_id)? {
            tracing::info!(
                event = "fork_lineage_linked",
                session_id = %candidate.session_id,
                parent_session_id = %key.session_id,
            );
            // `candidate_key` is the child in this relation: it just got
            // named a child of `key`.
            store.requeue_session_evidence(&candidate_key)?;
        }
    }
    Ok(())
}

/// `candidate`'s own key, in `key`'s environment and agent — the scope
/// [`Store::sessions_owning_turn_uuids`] already searched within.
fn candidate_key(key: &SessionKey, candidate: &OwningSession) -> SessionKey {
    SessionKey::new(
        key.environment_key.clone(),
        key.agent.clone(),
        candidate.session_id.clone(),
    )
}

#[cfg(test)]
mod tests;

//! Stop in-flight work for one destination without taking the Cloud worker lock.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

type Route = (String, String);

#[derive(Default)]
struct RouteState {
    generation: AtomicU64,
    pending_pauses: AtomicUsize,
}

/// Share fences only while a worker or control request holds a route. A stale
/// renderer request must not leave permanent entries in the registry.
#[derive(Default)]
pub(super) struct RouteFences {
    routes: Mutex<BTreeMap<Route, Weak<RouteState>>>,
}

impl RouteFences {
    /// Take a snapshot for the exact account and destination before sensitive work.
    pub(super) fn snapshot(&self, account: &str, destination: &str) -> Result<RouteFence, String> {
        let key = (account.to_owned(), destination.to_owned());
        let mut routes = self
            .routes
            .lock()
            .map_err(|_| "Cloud sync is unavailable.")?;
        routes.retain(|_, state| state.strong_count() != 0);
        let state = routes.get(&key).and_then(Weak::upgrade).unwrap_or_else(|| {
            let state = Arc::new(RouteState::default());
            routes.insert(key, Arc::downgrade(&state));
            state
        });
        let generation = state.generation.load(Ordering::Acquire);
        Ok(RouteFence { state, generation })
    }
}

/// A route snapshot stays fenced after a pause, even after the pause commits.
pub(super) struct RouteFence {
    state: Arc<RouteState>,
    generation: u64,
}

impl RouteFence {
    /// Deny old work and any new work admitted while a pause is waiting to save.
    pub(super) fn current(&self) -> bool {
        self.state.pending_pauses.load(Ordering::Acquire) == 0
            && self.state.generation.load(Ordering::Acquire) == self.generation
    }

    /// Fence this route before waiting for cleanup admission. Keep the guard
    /// in the cleanup closure until the pause write returns.
    pub(super) fn pause(&self) -> PendingPause {
        self.state.pending_pauses.fetch_add(1, Ordering::AcqRel);
        self.state.generation.fetch_add(1, Ordering::AcqRel);
        PendingPause(self.state.clone())
    }
}

/// Finish only this pause. Another queued pause still blocks the route.
pub(super) struct PendingPause(Arc<RouteState>);

impl Drop for PendingPause {
    fn drop(&mut self) {
        self.0.pending_pauses.fetch_sub(1, Ordering::AcqRel);
    }
}

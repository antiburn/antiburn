use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Serialize, Serializer};

use crate::lifecycle::RequestTransition;
use crate::model::AnchorRegion;

use super::super::AnchoredWindowManager;
use super::fixtures::config;

struct CloneCounter(Arc<AtomicUsize>);

impl Clone for CloneCounter {
    fn clone(&self) -> Self {
        self.0.fetch_add(1, Ordering::Relaxed);
        Self(self.0.clone())
    }
}

impl Serialize for CloneCounter {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_unit()
    }
}

#[test]
fn renderer_envelope_clones_presentation_only_when_delivery_is_ready() {
    let manager = AnchoredWindowManager::<&str, CloneCounter>::new(config());
    let cold_clones = Arc::new(AtomicUsize::new(0));
    let (_, cold_render) = manager.prepare_request(
        "cold",
        AnchorRegion {
            top: 0.0,
            height: 20.0,
        },
        Some(CloneCounter(cold_clones.clone())),
    );

    assert!(cold_render.is_none());
    assert_eq!(cold_clones.load(Ordering::Relaxed), 0);

    manager.lock_lifecycle().renderer_ready = true;
    let ready_clones = Arc::new(AtomicUsize::new(0));
    let (transition, ready_render) = manager.prepare_request(
        "ready",
        AnchorRegion {
            top: 20.0,
            height: 20.0,
        },
        Some(CloneCounter(ready_clones.clone())),
    );

    assert!(matches!(transition, RequestTransition::Retargeted { .. }));
    assert!(ready_render.is_some());
    assert_eq!(ready_clones.load(Ordering::Relaxed), 1);
}

use super::*;
use crate::session_projection::{self, Emission, ModelLoader, ProjectionEmitter, RowLoader};
use crate::store::PublishedModel;
use std::sync::atomic::{AtomicBool, AtomicUsize};

#[derive(Default)]
struct Loader {
    model_calls: Mutex<Vec<usize>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
    row_active: AtomicBool,
    release: (Mutex<bool>, Condvar),
}
impl Loader {
    fn wait(&self) {
        let guard = self.release.0.lock().unwrap();
        let _ = self
            .release
            .1
            .wait_timeout_while(guard, Duration::from_secs(10), |released| !*released)
            .unwrap();
    }
    fn release(&self) {
        *self.release.0.lock().unwrap() = true;
        self.release.1.notify_all();
    }
}
impl ModelLoader for Loader {
    fn load_models(&self, keys: &[SessionKey]) -> ModelResult {
        let call = {
            let mut calls = self.model_calls.lock().unwrap();
            calls.push(keys.len());
            calls.len()
        };
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        if call == 1 {
            self.wait();
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        if call == 2 {
            panic!("synthetic middle model page panic");
        }
        Ok((
            keys.iter()
                .map(|key| PublishedModel {
                    key: key.clone(),
                    incarnation: Incarnation(0),
                    published_fence: Some(7),
                    model: Some("sonnet".into()),
                })
                .collect(),
            Revision(1000),
        ))
    }
}
impl RowLoader for Loader {
    fn load(
        &self,
        _keys: &[SessionKey],
        _now: i64,
    ) -> anyhow::Result<HashMap<SessionKey, crate::dto::ActivityEntry>> {
        self.row_active.store(true, Ordering::SeqCst);
        self.wait();
        self.row_active.store(false, Ordering::SeqCst);
        Ok(HashMap::new())
    }
}
#[derive(Default)]
struct Emitter(Mutex<Vec<Sequenced>>);
impl ProjectionEmitter for Emitter {
    fn emit(&self, emission: Emission) {
        if let Emission::Lifecycle(event) = emission {
            self.0.lock().unwrap().push(event);
        }
    }
}
async fn wait_for(test: impl Fn() -> bool) {
    for _ in 0..1000 {
        if test() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("condition did not become true");
}

#[tokio::test]
async fn startup_model_pages_survive_blocked_loaders_rich_overflow_lag_and_middle_page_panic() {
    let harness = start(
        BASE,
        (0..513).map(|i| (key(&format!("{i:04}")), BASE)).collect(),
    );
    let events = harness.events.clone();
    let loader = Arc::new(Loader::default());
    let emitter = Arc::new(Emitter::default());
    let bus = events.subscribe();
    let task = {
        let events = events.clone();
        let loader = loader.clone();
        let emitter = emitter.clone();
        tokio::spawn(async move {
            session_projection::run_with_models(
                bus,
                loader.clone(),
                emitter,
                &|| events.current_seq(),
                &|| BASE,
                Some((&events, loader)),
            )
            .await;
        })
    };
    wait_for(|| loader.active.load(Ordering::SeqCst) == 1).await;
    events
        .report_async(Observation::RowChanged {
            session: key("0000"),
            facets: UpdateFacets {
                metadata: true,
                ..Default::default()
            },
            at: BASE,
        })
        .await;
    wait_for(|| loader.row_active.load(Ordering::SeqCst)).await;
    events
        .report_async(anonymous(AgentKind::Codex, BASE, 1))
        .await;
    wait_for(|| {
        emitter
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, SessionEvent::Activity { session: None, .. }))
    })
    .await;
    // A synchronous burst overflows the rich row queue and broadcast ring.
    for i in 0..1500 {
        publish(
            &events,
            vec![SessionEvent::Updated {
                session: SessionRef::from(&key(&format!("rich-{i}"))),
                facets: UpdateFacets {
                    metadata: true,
                    ..Default::default()
                },
                at: BASE,
            }],
        );
    }
    wait_for(|| {
        emitter
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, SessionEvent::Resync))
    })
    .await;
    assert_eq!(events.snapshot(0).working, 513);
    loader.release();
    wait_for(|| {
        events
            .snapshot(0)
            .sweep
            .iter()
            .flat_map(|agent| &agent.models)
            .map(|model| model.working)
            .sum::<usize>()
            == 513
    })
    .await;
    assert_eq!(*loader.model_calls.lock().unwrap(), [256, 256, 1, 256]);
    assert_eq!(loader.max_active.load(Ordering::SeqCst), 1);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

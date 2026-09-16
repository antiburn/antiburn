//! Compact model evidence belongs to the registry, independently of rich row projection.

use super::*;
use crate::store::PublishedModel;
use tokio::sync::oneshot;

pub(crate) const MODEL_PAGE: usize = 256;
pub(crate) type ModelResult = anyhow::Result<(Vec<PublishedModel>, Revision)>;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepCounts {
    pub agent: String,
    pub working: usize,
    pub anonymous: usize,
    pub model_pending_working: usize,
    pub model_failed_working: usize,
    pub model_none_working: usize,
    pub models: Vec<ModelCount>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCount {
    pub model: String,
    pub working: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelRequest {
    pub key: SessionKey,
    incarnation: Incarnation,
    epoch: u64,
    ticket: u64,
    minimum: Revision,
}

pub(crate) struct ModelReply {
    pub requests: Vec<ModelRequest>,
    pub result: ModelResult,
    pub ack: oneshot::Sender<()>,
}

#[derive(Debug)]
enum Status {
    Pending,
    Failed,
    Ready {
        published_fence: Option<i64>,
        model: Option<String>,
    },
}

#[derive(Debug)]
struct Slot {
    epoch: u64,
    ticket: u64,
    accepted: Revision,
    status: Status,
    due: Option<Instant>,
    backoff: Duration,
}

#[derive(Default)]
pub(super) struct Models {
    epoch: u64,
    ticket: u64,
    slots: BTreeMap<SessionKey, Slot>,
    queue: BTreeSet<(Instant, u64, SessionKey)>,
    broad: Option<Option<SessionKey>>,
}

impl Models {
    pub(super) fn invalidate(&mut self, key: &SessionKey) {
        let accepted = self
            .slots
            .get(key)
            .map_or(Revision::default(), |slot| slot.accepted);
        self.remove(key);
        self.ticket = self.ticket.checked_add(1).expect("model ticket exhausted");
        let due = Instant::now();
        self.slots.insert(
            key.clone(),
            Slot {
                epoch: self.epoch,
                ticket: self.ticket,
                accepted,
                status: Status::Pending,
                due: Some(due),
                backoff: RECONCILE_BACKOFF_MIN,
            },
        );
        self.queue.insert((due, self.ticket, key.clone()));
    }

    pub(super) fn remove(&mut self, key: &SessionKey) {
        if let Some(slot) = self.slots.remove(key)
            && let Some(due) = slot.due
        {
            self.queue.remove(&(due, slot.ticket, key.clone()));
        }
    }

    pub(super) fn invalidate_all(&mut self) {
        self.epoch = self.epoch.checked_add(1).expect("model epoch exhausted");
        self.broad = Some(None);
    }

    fn advance_broad(&mut self) {
        let Some(cursor) = self.broad.take() else {
            return;
        };
        let lower = cursor.map_or(Bound::Unbounded, Bound::Excluded);
        let keys: Vec<_> = self
            .slots
            .range((lower, Bound::Unbounded))
            .take(MODEL_PAGE)
            .map(|(key, _)| key.clone())
            .collect();
        if keys.len() == MODEL_PAGE {
            self.broad = Some(keys.last().cloned());
        }
        for key in keys {
            if self.slots[&key].epoch != self.epoch {
                self.invalidate(&key);
            }
        }
    }
}

impl Registry {
    pub(super) fn sweep_counts(&self) -> Vec<SweepCounts> {
        let mut agents: BTreeMap<String, SweepCounts> = BTreeMap::new();
        let mut models: BTreeMap<(String, String), usize> = BTreeMap::new();
        for (key, entry) in &self.live {
            if entry.quiet_published {
                continue;
            }
            let count = agents
                .entry(key.agent.clone())
                .or_insert_with(|| SweepCounts {
                    agent: key.agent.clone(),
                    ..Default::default()
                });
            count.working += 1;
            let slot = self.models.slots.get(key);
            match slot
                .filter(|slot| slot.epoch == self.models.epoch)
                .map(|slot| &slot.status)
            {
                None | Some(Status::Pending) => count.model_pending_working += 1,
                Some(Status::Failed) => count.model_failed_working += 1,
                Some(Status::Ready {
                    published_fence: Some(_),
                    model: Some(model),
                }) => {
                    *models
                        .entry((key.agent.clone(), model.clone()))
                        .or_default() += 1;
                }
                Some(Status::Ready { .. }) => count.model_none_working += 1,
            }
        }
        for agent in self.anonymous.keys() {
            let slug = agent.slug().to_string();
            agents
                .entry(slug.clone())
                .or_insert_with(|| SweepCounts {
                    agent: slug,
                    ..Default::default()
                })
                .anonymous += 1;
        }
        for ((agent, model), working) in models {
            agents
                .get_mut(&agent)
                .expect("working agent exists")
                .models
                .push(ModelCount { model, working });
        }
        agents.into_values().collect()
    }

    fn model_page(&mut self) -> (Vec<ModelRequest>, Option<Instant>) {
        // Each selection services one broad step before keyed work.
        self.models.advance_broad();
        let now = Instant::now();
        let requests = self
            .models
            .queue
            .iter()
            .take_while(|(due, _, _)| *due <= now)
            .take(MODEL_PAGE)
            .filter_map(|(_, ticket, key)| {
                let entry = self.live.get(key)?;
                let slot = self.models.slots.get(key)?;
                Some(ModelRequest {
                    key: key.clone(),
                    incarnation: entry.incarnation,
                    epoch: slot.epoch,
                    ticket: *ticket,
                    minimum: entry.exists_at.max(slot.accepted),
                })
            })
            .collect();
        let next = if self.models.broad.is_some() {
            Some(now)
        } else {
            self.models.queue.first().map(|(due, _, _)| *due)
        };
        (requests, next)
    }

    fn apply_models(&mut self, requests: Vec<ModelRequest>, result: ModelResult) {
        let received_at = Instant::now();
        let (mut rows, revision) = match result {
            Ok((rows, revision)) => (
                rows.into_iter()
                    .map(|row| (row.key.clone(), row))
                    .collect::<HashMap<_, _>>(),
                Some(revision),
            ),
            Err(_) => (HashMap::new(), None),
        };
        for request in requests {
            let Some(entry) = self.live.get(&request.key) else {
                continue;
            };
            let Some(slot) = self.models.slots.get_mut(&request.key) else {
                continue;
            };
            if entry.incarnation != request.incarnation
                || slot.ticket != request.ticket
                || slot.epoch != request.epoch
                || self.models.epoch != request.epoch
            {
                continue;
            }
            let row = rows.remove(&request.key);
            let valid = revision.is_some_and(|revision| {
                revision >= request.minimum.max(entry.exists_at).max(slot.accepted)
            }) && row
                .as_ref()
                .is_some_and(|row| row.incarnation == request.incarnation);
            if let Some(due) = slot.due.take() {
                self.models
                    .queue
                    .remove(&(due, slot.ticket, request.key.clone()));
            }
            if valid {
                let row = row.expect("valid row exists");
                slot.accepted = revision.expect("valid revision exists");
                slot.status = Status::Ready {
                    published_fence: row.published_fence,
                    model: row.model,
                };
                slot.backoff = RECONCILE_BACKOFF_MIN;
            } else {
                slot.status = Status::Failed;
                let due = received_at + slot.backoff;
                slot.backoff = (slot.backoff * 2).min(RECONCILE_BACKOFF_MAX);
                slot.due = Some(due);
                self.models.queue.insert((due, slot.ticket, request.key));
            }
        }
    }
}

impl SessionEvents {
    pub(crate) fn model_page(&self) -> (Vec<ModelRequest>, Option<Instant>) {
        self.registry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .model_page()
    }

    pub(crate) async fn models_changed(&self) {
        self.model_wake.notified().await;
    }

    pub(crate) async fn submit_models(&self, requests: Vec<ModelRequest>, result: ModelResult) {
        let (ack, received) = oneshot::channel();
        if self
            .model_results
            .send(ModelReply {
                requests,
                result,
                ack,
            })
            .await
            .is_ok()
        {
            let _ = received.await;
        }
    }
}

pub(super) fn apply_reply(events: &SessionEvents, reply: ModelReply) {
    apply(events, |registry| {
        let before = registry.sweep_counts();
        registry.apply_models(reply.requests, reply.result);
        let out = if before != registry.sweep_counts() {
            vec![SessionEvent::SweepChanged]
        } else {
            Vec::new()
        };
        (out, ())
    });
    let _ = reply.ack.send(());
}

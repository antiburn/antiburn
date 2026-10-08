use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};

use antiburn_local::checks::skill_opportunities::SkillOpportunitySnapshot;

use crate::agent_config::{ConfigContext, SkillSnapshotError, skill_opportunity_snapshot};
use crate::store::SessionKey;

use super::{InputLoadError, InventoryRevisionObserver};

#[cfg(test)]
pub(crate) static INVENTORY_DISCOVERY_COUNT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

struct ContextInventory {
    config: ConfigContext,
    seen: BTreeSet<SessionKey>,
    snapshot: Arc<SkillOpportunitySnapshot>,
    bytes: usize,
}

#[derive(Default)]
pub(crate) struct InventorySweepCache {
    contexts: VecDeque<ContextInventory>,
}

static INVENTORIES: LazyLock<Mutex<InventorySweepCache>> =
    LazyLock::new(|| Mutex::new(Default::default()));

const MAX_INVENTORY_BYTES: usize = 64 * 1024 * 1024;

fn snapshot_bytes(snapshot: &SkillOpportunitySnapshot) -> usize {
    snapshot
        .skills()
        .iter()
        .map(|skill| {
            skill.description.len()
                + skill.frontmatter.to_string().len()
                + skill.identity.len()
                + skill.name.len()
        })
        .sum::<usize>()
        .saturating_mul(4)
}

pub(crate) fn discover_inventory(
    config: &ConfigContext,
) -> Result<Arc<SkillOpportunitySnapshot>, SkillSnapshotError> {
    #[cfg(test)]
    INVENTORY_DISCOVERY_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    skill_opportunity_snapshot(config).map(Arc::new)
}

impl InventorySweepCache {
    pub(crate) fn snapshot(
        &mut self,
        config: &ConfigContext,
        key: &SessionKey,
    ) -> Result<Arc<SkillOpportunitySnapshot>, SkillSnapshotError> {
        if let Some(index) = self
            .contexts
            .iter()
            .position(|entry| entry.config == *config)
        {
            let mut entry = self.contexts.remove(index).expect("context cache index");
            if entry.seen.contains(key) || entry.seen.len() == InventoryRevisionObserver::MAX_INPUTS
            {
                entry.snapshot = discover_inventory(config)?;
                entry.bytes = snapshot_bytes(&entry.snapshot);
                entry.seen.clear();
            }
            entry.seen.insert(key.clone());
            let snapshot = Arc::clone(&entry.snapshot);
            self.retain(entry);
            return Ok(snapshot);
        }
        let snapshot = discover_inventory(config)?;
        self.retain(ContextInventory {
            config: config.clone(),
            seen: BTreeSet::from([key.clone()]),
            snapshot: Arc::clone(&snapshot),
            bytes: snapshot_bytes(&snapshot),
        });
        Ok(snapshot)
    }

    fn retain(&mut self, entry: ContextInventory) {
        if entry.bytes > MAX_INVENTORY_BYTES {
            return;
        }
        while self.contexts.len() >= InventoryRevisionObserver::MAX_CONTEXTS
            || self
                .contexts
                .iter()
                .map(|context| context.bytes)
                .sum::<usize>()
                + entry.bytes
                > MAX_INVENTORY_BYTES
        {
            self.contexts.pop_front();
        }
        self.contexts.push_back(entry);
    }
}

pub(crate) fn sweep_inventory(
    config: &ConfigContext,
    key: &SessionKey,
) -> Result<Arc<SkillOpportunitySnapshot>, InputLoadError> {
    INVENTORIES
        .lock()
        .map_err(|_| {
            InputLoadError::Preparation(
                antiburn_local::analysis::jev::JevError::InvalidCheckContext,
            )
        })?
        .snapshot(config, key)
        .map_err(InputLoadError::Inventory)
}

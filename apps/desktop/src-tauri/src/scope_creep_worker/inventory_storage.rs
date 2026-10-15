use antiburn_local::analysis::jev::JevEvidenceReference;
use antiburn_local::analysis::jev_evidence::ContentEventReference;
use antiburn_local::checks::scope_creep::{
    ScopeCreepPrepared, ScopeDescriptorInventory, ScopeInventoryLimit, WorkBinding, WorkGroup,
    WorkObservationKind,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
struct Group(
    String,
    String,
    Vec<usize>,
    Vec<usize>,
    Vec<String>,
    Option<String>,
    Vec<JevEvidenceReference>,
    WorkObservationKind,
    #[serde(default)] Vec<antiburn_local::checks::scope_creep::ScopeExcerpt>,
);

#[derive(Serialize, Deserialize)]
struct Binding(String, usize, usize, u64, Option<String>, u32, bool, String);

#[derive(Serialize, Deserialize)]
struct Inventory {
    source_revision: String,
    next_action: usize,
    bindings: Vec<Binding>,
    sources: Vec<String>,
    groups: Vec<Group>,
    complete: bool,
    #[serde(default)]
    limitation: Option<ScopeInventoryLimit>,
    #[serde(default)]
    descriptor_bytes: usize,
}

pub(super) fn serialize<S: Serializer>(
    inventory: &ScopeDescriptorInventory,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut bindings = Vec::new();
    let mut indices = BTreeMap::new();
    let mut sources = Vec::new();
    let mut source_indices = BTreeMap::new();
    let mut source_index = |digest: &str| {
        *source_indices.entry(digest.to_owned()).or_insert_with(|| {
            sources.push(digest.to_owned());
            sources.len() - 1
        })
    };
    let mut intern = |items: &[WorkBinding]| -> Result<Vec<usize>, S::Error> {
        items
            .iter()
            .map(|binding| {
                let key = serde_json::to_string(binding).map_err(serde::ser::Error::custom)?;
                Ok(*indices.entry(key).or_insert_with(|| {
                    let reference = &binding.reference;
                    bindings.push(Binding(
                        reference.id.clone(),
                        source_index(&reference.source_key_digest),
                        source_index(&reference.thread_digest),
                        reference.turn_index,
                        reference.native_record_id.clone(),
                        reference.part_index,
                        reference.stable,
                        binding.digest.clone(),
                    ));
                    bindings.len() - 1
                }))
            })
            .collect()
    };
    let groups = inventory
        .groups
        .iter()
        .map(|group| {
            Ok(Group(
                group.id.clone(),
                group.semantic_digest.clone(),
                intern(&group.work)?,
                intern(&group.context)?,
                group.window_ids.clone(),
                group.limitation.clone(),
                group.task_scope.clone(),
                group.observation_kind,
                group.selected_excerpts.clone(),
            ))
        })
        .collect::<Result<_, S::Error>>()?;
    Inventory {
        source_revision: inventory.source_revision.clone(),
        next_action: inventory.next_action,
        bindings,
        sources,
        groups,
        complete: inventory.complete,
        limitation: inventory.limitation,
        descriptor_bytes: inventory.descriptor_bytes,
    }
    .serialize(serializer)
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<ScopeDescriptorInventory, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Compact(Inventory),
        Legacy(ScopeDescriptorInventory),
    }
    let inventory = match Stored::deserialize(deserializer)? {
        Stored::Legacy(inventory) => return Ok(inventory),
        Stored::Compact(inventory) => inventory,
    };
    let source = |index: usize| {
        inventory
            .sources
            .get(index)
            .cloned()
            .ok_or_else(|| serde::de::Error::custom("invalid scope source index"))
    };
    let bindings: Vec<WorkBinding> = inventory
        .bindings
        .into_iter()
        .map(
            |Binding(id, key, thread, turn_index, native_record_id, part_index, stable, digest)| {
                Ok(WorkBinding {
                    reference: ContentEventReference {
                        id,
                        source_key_digest: source(key)?,
                        thread_digest: source(thread)?,
                        turn_index,
                        native_record_id,
                        part_index,
                        stable,
                    },
                    digest,
                })
            },
        )
        .collect::<Result<_, D::Error>>()?;
    let expand = |indices: Vec<usize>| -> Result<Vec<WorkBinding>, D::Error> {
        indices
            .into_iter()
            .map(|index| {
                bindings
                    .get(index)
                    .cloned()
                    .ok_or_else(|| serde::de::Error::custom("invalid scope binding index"))
            })
            .collect()
    };
    let groups = inventory
        .groups
        .into_iter()
        .map(
            |Group(
                id,
                semantic_digest,
                work,
                context,
                window_ids,
                limitation,
                task_scope,
                observation_kind,
                selected_excerpts,
            )| {
                Ok(WorkGroup {
                    id,
                    semantic_digest,
                    work: expand(work)?,
                    context: expand(context)?,
                    window_ids,
                    limitation,
                    task_scope,
                    observation_kind,
                    selected_excerpts,
                })
            },
        )
        .collect::<Result<_, D::Error>>()?;
    Ok(ScopeDescriptorInventory {
        source_revision: inventory.source_revision,
        next_action: inventory.next_action,
        groups,
        complete: inventory.complete,
        limitation: inventory.limitation,
        descriptor_bytes: inventory.descriptor_bytes,
    })
}

#[derive(Serialize, Deserialize)]
struct Prepared {
    scope_digest: String,
    semantic_epoch: antiburn_local::checks::sampling::StableId,
    source_generation: i64,
    publication_fence: i64,
    #[serde(with = "self")]
    groups: ScopeDescriptorInventory,
    scope_bindings: Vec<JevEvidenceReference>,
    session_limitation: Option<antiburn_local::analysis::session_scope::SessionScopeError>,
}

pub(super) fn serialize_prepared<S: Serializer>(
    prepared: &ScopeCreepPrepared,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    Prepared {
        scope_digest: prepared.scope_digest.clone(),
        semantic_epoch: prepared.semantic_epoch,
        source_generation: prepared.source_generation,
        publication_fence: prepared.publication_fence,
        groups: ScopeDescriptorInventory {
            groups: prepared.groups.clone(),
            ..Default::default()
        },
        scope_bindings: Vec::new(),
        session_limitation: prepared.session_limitation.clone(),
    }
    .serialize(serializer)
}

pub(super) fn deserialize_prepared<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<ScopeCreepPrepared, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Compact(Prepared),
        Legacy(ScopeCreepPrepared),
    }
    let prepared = match Stored::deserialize(deserializer)? {
        Stored::Legacy(prepared) => return Ok(prepared),
        Stored::Compact(prepared) => prepared,
    };
    Ok(ScopeCreepPrepared {
        scope_digest: prepared.scope_digest,
        semantic_epoch: prepared.semantic_epoch,
        source_generation: prepared.source_generation,
        publication_fence: prepared.publication_fence,
        groups: prepared.groups.groups,
        scope_bindings: prepared.scope_bindings,
        session_limitation: prepared.session_limitation,
    })
}

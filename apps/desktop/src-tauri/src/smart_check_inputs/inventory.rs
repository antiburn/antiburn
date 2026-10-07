use std::collections::BTreeMap;
use std::path::Path;

use crate::agent_config::{
    ConfigContext, EnabledState, ResourceKind, SkillSnapshotError, advisory_resource_inventory,
    skill_opportunity_snapshot,
};
use crate::store::{SessionKey, Store};
use antiburn_local::checks::skill_opportunities::{
    SKILL_USE_SELECTION, SkillOpportunitiesCheck, SkillOpportunitySnapshot, SkillUseBoundary,
    SkillUseSnapshot,
};

use super::{
    DetectorInput, InputLoadError, InputUnavailable, SmartCheckInputSnapshot, digest, unavailable,
};

#[derive(Debug, Clone)]
pub struct SkillInputs {
    inventory: SkillOpportunitySnapshot,
    usage: SkillUseSnapshot,
    input: SmartCheckInputSnapshot,
    revision: String,
}

impl SkillInputs {
    pub(crate) fn for_candidate(
        mut self,
        candidate: &crate::store::BurnCheckCandidate,
    ) -> Result<Self, InputLoadError> {
        self.input = self.input.for_candidate(candidate)?;
        self.revision = digest(&serde_json::json!((
            &self.revision,
            self.input.input_revision()
        )))?;
        Ok(self)
    }
    pub fn inventory(&self) -> &SkillOpportunitySnapshot {
        &self.inventory
    }
    pub fn usage(&self) -> &SkillUseSnapshot {
        &self.usage
    }
    pub fn input(&self) -> &SmartCheckInputSnapshot {
        &self.input
    }
    pub fn input_revision(&self) -> &str {
        &self.revision
    }

    pub fn check(&self) -> Result<SkillOpportunitiesCheck, InputLoadError> {
        SkillOpportunitiesCheck::new(
            self.input.content(),
            &self.inventory,
            &self.usage,
            self.input.scope(),
        )
        .map_err(InputLoadError::Preparation)
    }
}

impl Store {
    /// ConfigContext describes current native inventory, not historical launch settings.
    pub fn load_smart_check_skill_inputs(
        &self,
        input: SmartCheckInputSnapshot,
        context: &ConfigContext,
    ) -> Result<SkillInputs, InputLoadError> {
        input.require_detector(DetectorInput::SkillOpportunities)?;
        validate_environment(&input.key.environment_key, context)?;
        if context.agent.slug() != input.key.agent {
            return Err(unavailable(InputUnavailable::InventoryContextMismatch));
        }
        let stored = self
            .with_published_content(
                &input.key,
                input.scope.source_generation(),
                |connection, fence| {
                    let (cwd, wsl): (Option<String>, Option<String>) = connection.query_row(
                        "SELECT cwd, wsl_distro FROM session WHERE environment_key = ?1
                     AND agent = ?2 AND session_id = ?3",
                        rusqlite::params![
                            input.key.environment_key,
                            input.key.agent,
                            input.key.session_id
                        ],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?;
                    Ok((fence, cwd, wsl))
                },
            )
            .map_err(InputLoadError::Query)?
            .ok_or_else(|| unavailable(InputUnavailable::PublicationChanged))?;
        if stored.0 != input.scope.publication_fence() {
            return Err(unavailable(InputUnavailable::PublicationChanged));
        }
        if stored.2.is_some() {
            return Err(unavailable(
                InputUnavailable::UnsupportedInventoryEnvironment,
            ));
        }
        let cwd = stored
            .1
            .as_deref()
            .map(Path::new)
            .map(Path::canonicalize)
            .transpose()
            .map_err(|_| unavailable(InputUnavailable::InventoryContextMismatch))?;
        let context_cwd = context
            .workspace_cwd
            .as_deref()
            .map(Path::canonicalize)
            .transpose()
            .map_err(|_| unavailable(InputUnavailable::InventoryContextMismatch))?;
        if cwd != context_cwd {
            return Err(unavailable(InputUnavailable::InventoryContextMismatch));
        }
        let inventory = current_inventory(&input.key.environment_key, context)?;
        let content = antiburn_local::analysis::jev_evidence::select_session_content(
            input.content(),
            SKILL_USE_SELECTION,
        );
        let boundary = SkillUseBoundary {
            session_identity: content.session_identity_digest.clone(),
            native_session_id: input.key.session_id.clone(),
            publication_fence: input.scope.publication_fence(),
            scope: inventory.scope().clone(),
        };
        let usage = SkillUseSnapshot::from_published_content(&content, &boundary)
            .map_err(InputLoadError::SkillUse)?;
        let revision = digest(&serde_json::json!({
            "input": input.input_revision(), "inventory": inventory.revision(),
            "use": usage.revision(),
        }))?;
        self.validate_smart_check_publication(
            &input.key,
            input.scope.publication_fence(),
            input.scope.source_generation(),
            &input.boundary,
        )?;
        Ok(SkillInputs {
            inventory,
            usage,
            input,
            revision,
        })
    }
}

fn validate_environment(environment: &str, context: &ConfigContext) -> Result<(), InputLoadError> {
    if environment != "native" || !context.native_environment {
        Err(unavailable(
            InputUnavailable::UnsupportedInventoryEnvironment,
        ))
    } else {
        Ok(())
    }
}

fn current_inventory(
    environment: &str,
    context: &ConfigContext,
) -> Result<SkillOpportunitySnapshot, InputLoadError> {
    validate_environment(environment, context)?;
    let resources = advisory_resource_inventory(context, [])
        .map_err(|error| InputLoadError::Inventory(SkillSnapshotError::Config(error)))?;
    if resources
        .issues
        .iter()
        .any(|issue| issue.kind.is_none() || issue.kind == Some(ResourceKind::Skill))
    {
        return Err(unavailable(InputUnavailable::InventoryIncomplete));
    }
    let inventory = skill_opportunity_snapshot(context).map_err(InputLoadError::Inventory)?;
    if resources
        .resources
        .iter()
        .filter(|resource| resource.kind == ResourceKind::Skill)
        .any(|resource| {
            resource.enabled == EnabledState::Unknown
                || !inventory.skills().iter().any(|skill| {
                    skill.name == resource.canonical_name
                        || skill.aliases.contains(&resource.canonical_name)
                })
        })
    {
        return Err(unavailable(InputUnavailable::InventoryIncomplete));
    }
    Ok(inventory)
}

/// A caller observes a bounded set of admitted native contexts. No scheduler is modified.
#[derive(Debug, Default)]
pub struct InventoryRevisionObserver {
    revisions: BTreeMap<String, String>,
    inputs: BTreeMap<SessionKey, ObservedSkillInput>,
    input_generation: u64,
}

#[derive(Debug)]
struct ObservedSkillInput {
    revision: Option<String>,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryRevisionChange {
    pub context_identity: String,
    pub previous_revision: Option<String>,
    pub current_revision: String,
}

impl InventoryRevisionObserver {
    pub const MAX_CONTEXTS: usize = 64;
    pub(crate) const MAX_INPUTS: usize = 512;

    /// Eviction rejects old write permits. Durable revisions still detect changes after restart.
    pub(crate) fn observe_input(
        &mut self,
        key: &SessionKey,
        revision: Option<String>,
    ) -> Result<u64, InputLoadError> {
        if let Some(observed) = self.inputs.get(key)
            && observed.revision == revision
        {
            return Ok(observed.generation);
        }
        let generation = self
            .input_generation
            .checked_add(1)
            .ok_or_else(|| unavailable(InputUnavailable::AssemblyLimitReached))?;
        if !self.inputs.contains_key(key) && self.inputs.len() == Self::MAX_INPUTS {
            let oldest = self
                .inputs
                .iter()
                .min_by_key(|(_, value)| value.generation)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.inputs.remove(&oldest);
            }
        }
        self.input_generation = generation;
        self.inputs.insert(
            key.clone(),
            ObservedSkillInput {
                revision,
                generation,
            },
        );
        Ok(generation)
    }

    pub(crate) fn input_is_current(
        &self,
        key: &SessionKey,
        revision: &str,
        generation: u64,
    ) -> bool {
        self.inputs.get(key).is_some_and(|observed| {
            observed.revision.as_deref() == Some(revision) && observed.generation == generation
        })
    }

    /// Call at the owner's reconciliation interval or after a config change.
    /// Availability changes also change revision, so old findings can be invalidated.
    pub fn observe(
        &mut self,
        environment: &str,
        context: &ConfigContext,
    ) -> Result<Option<InventoryRevisionChange>, InputLoadError> {
        validate_environment(environment, context)?;
        let identity = digest(&serde_json::json!([
            environment,
            context.agent.slug(),
            context.home_root.as_os_str().as_encoded_bytes(),
            context
                .workspace_cwd
                .as_ref()
                .map(|path| path.as_os_str().as_encoded_bytes()),
            context
                .trusted_workspace_root
                .as_ref()
                .map(|path| path.as_os_str().as_encoded_bytes()),
        ]))?;
        if !self.revisions.contains_key(&identity) && self.revisions.len() == Self::MAX_CONTEXTS {
            return Err(unavailable(InputUnavailable::AssemblyLimitReached));
        }
        let state = match current_inventory(environment, context) {
            Ok(inventory) => serde_json::json!({"inventory": inventory.revision()}),
            Err(InputLoadError::Inventory(error)) => {
                serde_json::json!({"unavailable": format!("{error:?}")})
            }
            Err(InputLoadError::Unavailable(reason)) => {
                serde_json::json!({"unavailable": format!("{reason:?}")})
            }
            Err(error) => return Err(error),
        };
        let revision = digest(&state)?;
        if self.revisions.get(&identity) == Some(&revision) {
            return Ok(None);
        }
        let previous_revision = self.revisions.insert(identity.clone(), revision.clone());
        Ok(Some(InventoryRevisionChange {
            context_identity: identity,
            previous_revision,
            current_revision: revision,
        }))
    }

    pub fn forget(&mut self, context_identity: &str) {
        self.revisions.remove(context_identity);
    }
}

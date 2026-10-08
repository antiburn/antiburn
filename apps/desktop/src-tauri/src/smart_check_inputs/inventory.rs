use std::collections::BTreeMap;
use std::path::Path;

use crate::agent_config::{ConfigContext, skill_opportunity_snapshot};
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
    skill_opportunity_snapshot(context).map_err(InputLoadError::Inventory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use antiburn_local::checks::skill_opportunities::{
        SkillOpportunityLimit, SkillUseEvidence, SkillUseOrdering, SkillUseStatus, SkillWorkContext,
    };
    use antiburn_local::model::AgentKind;

    #[test]
    fn inventory_admits_bound_definitions_with_unsupported_siblings_and_unknown_use() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join(".claude/skills");
        for (name, text) in [
            (
                "review",
                "---\nname: review\ndescription: Review code.\n---\n",
            ),
            ("malformed", "---\nname: [\n---\n"),
            ("unsupported", "---\ndescription: 42\n---\n"),
        ] {
            let path = root.join(name).join("SKILL.md");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let context = ConfigContext::native(AgentKind::Claude, directory.path(), None);
        let inventory = current_inventory("native", &context).unwrap();
        assert_eq!(inventory.skills().len(), 1);
        let definition = &inventory.skills()[0];
        assert_eq!(definition.name, "review");
        assert!(definition.enabled);
        assert!(!definition.identity.is_empty());
        assert!(!definition.revision.is_empty());
        let work = SkillWorkContext {
            session_identity: "session".into(),
            scope: inventory.scope().clone(),
            relevant_work_at_ms: None,
        };
        let usage = SkillUseEvidence {
            session_identity: work.session_identity.clone(),
            scope: work.scope.clone(),
            status: SkillUseStatus::Unknown,
            ordering: SkillUseOrdering::Unknown,
            events: Vec::new(),
        };
        let candidates = inventory.eligible_candidates(&work, &usage).unwrap();
        assert_eq!(candidates.len(), 1);
        assert!(
            candidates[0]
                .limitations()
                .contains(&SkillOpportunityLimit::InventoryIncomplete)
        );
        assert!(
            candidates[0]
                .limitations()
                .contains(&SkillOpportunityLimit::UseEvidenceIncomplete)
        );
        assert_eq!(usage.status, SkillUseStatus::Unknown);
        assert!(matches!(
            current_inventory("ssh", &context),
            Err(InputLoadError::Unavailable(
                InputUnavailable::UnsupportedInventoryEnvironment
            ))
        ));
        std::fs::remove_dir_all(root.join("malformed")).unwrap();
        std::fs::remove_dir_all(root.join("unsupported")).unwrap();
        let complete = current_inventory("native", &context).unwrap();
        assert_eq!(complete.skills(), inventory.skills());
        assert_ne!(complete.revision(), inventory.revision());
        assert!(
            !complete.eligible_candidates(&work, &usage).unwrap()[0]
                .limitations()
                .contains(&SkillOpportunityLimit::InventoryIncomplete)
        );
    }
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

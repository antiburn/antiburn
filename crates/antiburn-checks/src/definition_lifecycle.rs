//! Effective definition reconciliation and activation generation transitions.

use std::path::PathBuf;
use std::sync::Arc;

use crate::compiler::CompiledCheck;
use crate::discovery::{DefinitionScope, DefinitionState, EffectiveDefinition};
use crate::domain::{ActivationId, ActivationScope, Generation, ObservationPosition};
use crate::source::PublicationCursor;

/// An effective ready definition bound to one activation scope and source boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectiveActivation {
    /// Stable logical ID and generation.
    pub id: ActivationId,
    /// Content revision selected by discovery.
    pub revision: [u8; 32],
    /// Source scope selected by discovery.
    pub definition_scope: DefinitionScope,
    /// Source paths that identify the selected definition.
    pub source_paths: Vec<PathBuf>,
    /// Resolved repository root, independent of the exact session CWD.
    pub project_root: Option<PathBuf>,
    /// Compiled definition shared by candidates in this generation.
    pub compiled: Arc<CompiledCheck>,
    /// Source high-water mark established when this generation begins.
    pub bootstrap_observation_position: ObservationPosition,
    /// Publication resume boundary. This is not an observation position.
    pub bootstrap_publication: PublicationCursor,
}

/// The effective state retained for a logical definition.
#[derive(Clone, Debug, PartialEq)]
pub enum LifecycleState {
    /// The definition can produce candidates.
    Active(EffectiveActivation),
    /// A prior active generation whose current definition cannot compile.
    /// Reconciliation must revoke it before recovery can expose its work.
    Uncompiled {
        /// Latest persisted generation.
        generation: Generation,
    },
    /// The selected definition is paused or invalid.
    Inactive {
        /// Selected revision, retained to detect future edits.
        revision: [u8; 32],
        /// Project binding used for this state.
        project_root: Option<PathBuf>,
        /// Latest persisted generation.
        generation: Generation,
        /// Source scope selected by discovery.
        scope: DefinitionScope,
        /// Source paths that identify the selected definition.
        source_paths: Vec<PathBuf>,
        /// Whether discovery found an invalid or paused definition.
        inactive_kind: InactiveKind,
    },
    /// No effective definition currently exists.
    Removed {
        /// Latest generation retained as a replay guard.
        generation: Generation,
    },
}

/// Reason the selected definition cannot activate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InactiveKind {
    /// The selected file did not compile for this runtime.
    Invalid,
    /// The selected file is disabled.
    Paused,
}

/// Lifecycle event emitted by one reconciliation.
#[derive(Clone, Debug, PartialEq)]
pub enum LifecycleTransition {
    /// The active generation continues without resetting its bootstrap boundary.
    Unchanged(EffectiveActivation),
    /// A new active generation must be persisted atomically with its boundary.
    Activated(EffectiveActivation),
    /// The definition became invalid or paused; pending work loses publication rights.
    Deactivated {
        /// Selected revision retained for change detection.
        revision: [u8; 32],
        /// Project binding retained for change detection.
        project_root: Option<PathBuf>,
        /// Source scope retained for change detection.
        scope: DefinitionScope,
        /// Source paths retained for change detection.
        source_paths: Vec<PathBuf>,
        /// Invalid or paused state retained for change detection.
        inactive_kind: InactiveKind,
        /// Generation that invalidates pending work.
        next_generation: Generation,
    },
    /// The definition was removed; pending work loses publication rights.
    Removed { next_generation: Generation },
    /// The selected inactive definition did not change.
    UnchangedInactive,
}

/// Reconciles one discovered definition against its previously effective state.
#[derive(Clone, Debug, Default)]
pub struct DefinitionLifecycle;

impl DefinitionLifecycle {
    /// Reconciles an effective definition and returns its activation transition.
    ///
    /// A generation changes when revision, readiness, enablement, or project
    /// binding changes. An unchanged active definition keeps its original
    /// bootstrap boundary. The caller persists the transition with its Store.
    pub fn reconcile(
        &self,
        previous: &LifecycleState,
        definition: Option<&EffectiveDefinition>,
        project_root: Option<PathBuf>,
        bootstrap_head: ObservationPosition,
    ) -> LifecycleTransition {
        self.reconcile_scoped(
            previous,
            definition,
            project_root,
            ActivationScope::default(),
            bootstrap_head,
            PublicationCursor::default(),
        )
    }

    /// Reconciles within an explicit isolation scope and independent source orders.
    /// The host loads the previous persisted state before calling this method.
    pub fn reconcile_scoped(
        &self,
        previous: &LifecycleState,
        definition: Option<&EffectiveDefinition>,
        project_root: Option<PathBuf>,
        scope_binding: ActivationScope,
        bootstrap_head: ObservationPosition,
        bootstrap_publication: PublicationCursor,
    ) -> LifecycleTransition {
        let Some(definition) = definition else {
            return match previous {
                LifecycleState::Removed { generation } => LifecycleTransition::Removed {
                    next_generation: *generation,
                },
                _ => LifecycleTransition::Removed {
                    next_generation: next_generation(previous),
                },
            };
        };

        let (compiled, inactive_kind) = match &definition.state {
            DefinitionState::Ready(compiled) => (Some(Arc::new((**compiled).clone())), None),
            DefinitionState::Invalid(_) => (None, Some(InactiveKind::Invalid)),
            DefinitionState::Paused(_) => (None, Some(InactiveKind::Paused)),
        };
        let Some(compiled) = compiled else {
            if let LifecycleState::Inactive {
                revision,
                project_root: old_project,
                scope,
                source_paths,
                inactive_kind: old_kind,
                ..
            } = previous
                && *revision == definition.content_revision
                && *old_project == project_root
                && *scope == definition.scope
                && *source_paths == definition.source_paths
                && Some(*old_kind) == inactive_kind
            {
                return LifecycleTransition::UnchangedInactive;
            }
            return LifecycleTransition::Deactivated {
                revision: definition.content_revision,
                project_root,
                scope: definition.scope,
                source_paths: definition.source_paths.clone(),
                inactive_kind: inactive_kind.expect("inactive definition has a reason"),
                next_generation: next_generation(previous),
            };
        };

        if let LifecycleState::Active(active) = previous
            && active.id.scope == scope_binding
            && active.revision == definition.content_revision
            && active.project_root == project_root
            && active.definition_scope == definition.scope
            && active.source_paths == definition.source_paths
        {
            return LifecycleTransition::Unchanged(active.clone());
        }

        let generation = next_generation(previous);
        LifecycleTransition::Activated(EffectiveActivation {
            id: ActivationId {
                scope: scope_binding,
                definition_id: definition.id.clone(),
                generation,
            },
            revision: definition.content_revision,
            definition_scope: definition.scope,
            source_paths: definition.source_paths.clone(),
            project_root,
            compiled,
            bootstrap_observation_position: bootstrap_head,
            bootstrap_publication,
        })
    }
}

impl LifecycleState {
    /// Applies a lifecycle event to produce the next in-memory state.
    #[must_use]
    pub fn apply(&self, transition: &LifecycleTransition) -> Self {
        match transition {
            LifecycleTransition::Unchanged(active) | LifecycleTransition::Activated(active) => {
                Self::Active(active.clone())
            }
            LifecycleTransition::UnchangedInactive => self.clone(),
            LifecycleTransition::Deactivated {
                revision,
                project_root,
                scope,
                source_paths,
                inactive_kind,
                next_generation,
            } => Self::Inactive {
                revision: *revision,
                project_root: project_root.clone(),
                generation: *next_generation,
                scope: *scope,
                source_paths: source_paths.clone(),
                inactive_kind: *inactive_kind,
            },
            LifecycleTransition::Removed { next_generation } => Self::Removed {
                generation: *next_generation,
            },
        }
    }
}

/// Return the next persisted generation after any prior state.
fn next_generation(previous: &LifecycleState) -> Generation {
    let current = match previous {
        LifecycleState::Active(active) => active.id.generation.0,
        LifecycleState::Inactive { generation, .. }
        | LifecycleState::Removed { generation }
        | LifecycleState::Uncompiled { generation } => generation.0,
    };
    Generation(current.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(revision: [u8; 32]) -> EffectiveDefinition {
        EffectiveDefinition {
            id: "retry".to_owned(),
            scope: crate::discovery::DefinitionScope::Global,
            source_paths: vec![],
            content_revision: revision,
            state: DefinitionState::Ready(Box::new(
                crate::compiler::compile(
                    "name: Retry\non:\n  tool_result:\ndecisions:\n  review:\n    input: {activity: \"${{ message.tool.result.status }}\"}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    input: {result: failure}\n",
                )
                .unwrap(),
            )),
        }
    }

    #[test]
    fn unchanged_revision_keeps_bootstrap_and_restore_advances_generation() {
        let lifecycle = DefinitionLifecycle;
        let first = lifecycle.reconcile(
            &LifecycleState::Removed {
                generation: Generation(0),
            },
            Some(&definition([1; 32])),
            None,
            ObservationPosition(10),
        );
        let state = LifecycleState::Removed {
            generation: Generation(0),
        }
        .apply(&first);
        let unchanged = lifecycle.reconcile(
            &state,
            Some(&definition([1; 32])),
            None,
            ObservationPosition(99),
        );
        let LifecycleTransition::Unchanged(active) = unchanged else {
            panic!("same effective content must keep its activation");
        };
        assert_eq!(
            active.bootstrap_observation_position,
            ObservationPosition(10)
        );

        let paused = EffectiveDefinition {
            state: DefinitionState::Paused(vec![]),
            ..definition([2; 32])
        };
        let deactivated = lifecycle.reconcile(&state, Some(&paused), None, ObservationPosition(11));
        let inactive = state.apply(&deactivated);
        let restored = lifecycle.reconcile(
            &inactive,
            Some(&definition([1; 32])),
            None,
            ObservationPosition(12),
        );
        let LifecycleTransition::Activated(active) = restored else {
            panic!("restoring a definition must start a new generation");
        };
        assert_eq!(active.id.generation, Generation(3));
        assert_eq!(
            active.bootstrap_observation_position,
            ObservationPosition(12)
        );
    }
}

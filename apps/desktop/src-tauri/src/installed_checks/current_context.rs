use std::path::{Path, PathBuf};

use antiburn_checks::check_store::StoreError;
use antiburn_checks::context::{CurrentInventories, InventorySnapshot};
use antiburn_checks::definition_lifecycle::EffectiveActivation;
use antiburn_checks::domain::Observation;
use antiburn_checks::source::PublicationCursor;
use antiburn_local::analysis::SourceFormat;
use antiburn_local::model::AgentKind;
use antiburn_local::repositories::ConsentGrants;
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};

use crate::agent_config::ConfigContext;
use crate::agent_config::current_context::{CurrentContextRequest, collect_current_context};
use crate::consent::StoreConsentGrants;
use crate::store::{SessionKey, Store};

#[derive(Default)]
pub(super) struct CurrentContextCapture {
    native_home: Option<PathBuf>,
    #[cfg(test)]
    pub calls: usize,
}

impl CurrentContextCapture {
    pub fn new(native_home: Option<PathBuf>) -> Self {
        Self {
            native_home,
            #[cfg(test)]
            calls: 0,
        }
    }

    pub fn capture(
        &mut self,
        store: &Store,
        activation: &EffectiveActivation,
        cursor: PublicationCursor,
        observation: &Observation,
    ) -> std::result::Result<Option<CurrentInventories>, StoreError> {
        super::diagnostics::runtime_result(
            &activation.id,
            super::diagnostics::RuntimeStage::CurrentContext,
            self.capture_inner(store, activation, cursor, observation)
                .map(Some),
        )
    }

    fn capture_inner(
        &mut self,
        store: &Store,
        activation: &EffectiveActivation,
        cursor: PublicationCursor,
        observation: &Observation,
    ) -> Result<CurrentInventories> {
        if activation.id.scope.environment != "native" {
            return Ok(unavailable("non_native_environment"));
        }
        let Some(home) = self.native_home.as_deref() else {
            return Ok(unavailable("native_home_unavailable"));
        };
        let context = source_context(store, activation, cursor, observation, home)?;
        let recorded_grants = StoreConsentGrants::new(store).granted_dirs();
        #[cfg(test)]
        {
            self.calls += 1;
        }
        let snapshot = collect_current_context(CurrentContextRequest::from_context(
            &context,
            &recorded_grants,
        ));
        Ok(serde_json::from_value(serde_json::to_value(snapshot)?)?)
    }
}

fn source_context(
    store: &Store,
    activation: &EffectiveActivation,
    cursor: PublicationCursor,
    observation: &Observation,
    home: &Path,
) -> Result<ConfigContext> {
    let target = crate::store::native_tool_results::evidence::target_predicate("p", 4, 5, 6);
    let binding = store
        .lock()
        .query_row(
            &format!(
                "SELECT s.environment_key, s.agent, s.session_id, s.cwd, p.source_format
         FROM native_tool_result_publication p JOIN session s
           USING(environment_key, agent, session_id)
         WHERE p.publication_sequence = ?1 AND p.outcome = 'update'
           AND p.environment_key = ?2 AND s.cwd = ?3 AND s.wsl_distro IS NULL
             AND {target}
           AND (p.session_incarnation IS NULL OR
              (p.session_incarnation = s.incarnation AND p.project_cwd IS s.cwd))"
            ),
            params![
                i64::try_from(cursor.0)?,
                activation.id.scope.environment,
                activation.id.scope.session_cwd,
                activation.id.definition_id,
                crate::check_store::StoreCheckStore::activation_scope_key(&activation.id.scope),
                activation.id.generation.0
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?
        .context("current inventory source binding is unavailable")?;
    let (environment, agent, session_id, cwd, source_format) = binding;
    ensure!(
        SessionKey::new(&environment, &agent, session_id).to_identity_json()
            == observation.branch.0,
        "current inventory source belongs to another branch"
    );
    Ok(ConfigContext {
        agent: AgentKind::from_slug(&agent).context("current inventory agent is unavailable")?,
        source_format: Some(serde_json::from_str::<SourceFormat>(&source_format)?),
        home_root: home.to_owned(),
        workspace_cwd: Some(PathBuf::from(cwd)),
        trusted_workspace_root: activation.project_root.clone(),
        native_environment: true,
        runtime_override_present: false,
        managed_configuration_present: false,
    })
}

fn unavailable(reason: &str) -> CurrentInventories {
    let scope = InventorySnapshot {
        instructions_limits: vec![reason.to_owned()],
        skills_limits: vec![reason.to_owned()],
        ..InventorySnapshot::default()
    };
    CurrentInventories {
        global: scope.clone(),
        repo: scope,
    }
}

use super::*;
use crate::agent_config::opencode_config_authority::CurrentConfigAuthority;
use antiburn_local::analysis::{
    EvidenceValue, RelationConfidence, RelationProvenance, SubagentChild, SubagentConfigBinding,
};
use antiburn_local::insights::ReportWindow;

struct Fixture {
    _temporary: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
    path: PathBuf,
    store: Store,
    controller: RemediationController,
    finding: CurrentFinding,
}

fn binding() -> SubagentConfigBinding {
    SubagentConfigBinding {
        agent_id: "review".into(),
        provider_id: "openai".into(),
        model_id: "gpt-5.6-sol".into(),
    }
}

impl Fixture {
    fn new(binding: Option<SubagentConfigBinding>) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let home = root.join("home");
        let cwd = root.join("repo");
        let path = home.join(".config/opencode/opencode.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let store = Store::open(&root).unwrap();
        let mut evidence = opencode_v2_tests::model_evidence(SourceFormat::OpenCodeSqliteV2);
        let EvidenceValue::Complete(subagents) = &mut evidence.subagents else {
            panic!("the fixture requires subagent evidence")
        };
        subagents.spawn_count = 1;
        subagents.children.push(SubagentChild {
            ordinal: 1,
            parent_model: Some("gpt-5.6-sol".into()),
            parent_call_id: Some("call".into()),
            observed_child_models: ["gpt-5.6-sol".into()].into(),
            child_model: EvidenceValue::Complete(()),
            confidence: RelationConfidence::Observed,
            provenance: RelationProvenance::TaskToolUse,
            config_binding: binding,
        });
        opencode_v2_tests::publish_evidence(&store, &evidence);
        store
            .lock()
            .execute(
                "UPDATE session SET cwd=?1",
                [cwd.to_string_lossy().as_ref()],
            )
            .unwrap();
        store
            .replace_repositories(&[crate::store::RepositoryRecord {
                key: "repo".into(),
                repo_name: "repo".into(),
                full_name: "synthetic/repo".into(),
                status: "accessible".into(),
                repo_root: Some(cwd.to_string_lossy().into_owned()),
                suspected_path: None,
                worktree_count: 1,
                session_count: 1,
                wsl_distro: None,
                enabled: true,
            }])
            .unwrap();
        let finding = insights_report::list_current_findings(
            &root,
            CurrentFindingsRequest {
                environment_key: "native".into(),
                window: ReportWindow {
                    start_epoch: 0,
                    end_epoch: i64::MAX,
                },
                detector: DetectorId::OverpoweredSubagents,
            },
        )
        .unwrap()
        .findings
        .into_iter()
        .next()
        .expect("bound and unbound native S facts must remain findings");
        Self {
            controller: RemediationController::new(root),
            _temporary: temporary,
            home,
            cwd,
            path,
            store,
            finding,
        }
    }

    fn authority(&self, raw: serde_json::Value) -> CurrentConfigAuthority {
        std::fs::write(&self.path, raw.to_string()).unwrap();
        let mut canonical = raw;
        for agent in canonical["agents"].as_object_mut().unwrap().values_mut() {
            if let Some(reference) = agent.get("model").and_then(serde_json::Value::as_str) {
                let (provider, model) = reference.split_once('/').unwrap();
                let (model, variant) = model
                    .split_once('#')
                    .map_or((model, None), |(model, variant)| (model, Some(variant)));
                let mut value = json!({"providerID":provider,"model":model});
                if let Some(variant) = variant {
                    value["variant"] = variant.into();
                }
                agent["model"] = value;
            }
        }
        CurrentConfigAuthority::fixture(
            &self.cwd,
            json!([{"type":"document","path":self.path,"info":canonical}]),
        )
    }

    fn resolve(&self, authority: Option<&CurrentConfigAuthority>) -> CachedTarget {
        self.controller
            .resolve_opencode_target(
                &self.store,
                self.finding.clone(),
                Some(&self.home),
                authority,
            )
            .unwrap()
            .1
    }
}

#[test]
#[cfg(not(windows))]
fn native_s_binds_only_the_exact_agent_and_provider_and_retains_recovery_identity() {
    let fixture = Fixture::new(Some(binding()));
    let authority = fixture.authority(json!({"agents":{
        "review":{"model":"openai/gpt-5.6-sol","mode":"subagent"},
        "other":{"model":"openai/gpt-5.6-sol","mode":"subagent"}
    }}));
    let target = fixture.resolve(Some(&authority));
    let config = target.config.as_ref().unwrap();
    assert_eq!(config.operation.setting, ConfigSetting::SubagentModel);
    assert_eq!(
        config.operation.expected_value.scalar(),
        Some("openai/gpt-5.6-sol")
    );
    assert_eq!(
        config.operation.proposed_value.scalar(),
        Some("openai/gpt-5.6-luna")
    );
    assert!(remediation_prompt(target.finding()).is_ok());
    let definition = watch_definition(&target);
    assert_eq!(
        opencode_recovery_subject(&definition, ConfigSetting::SubagentModel),
        Some(OpenCodeConfigSubject::AgentModel("review".into()))
    );
    assert!(!watch_verification_available(
        &definition,
        &target.scope_kind,
        "opencode",
        DetectorId::OverpoweredSubagents
    ));
    let revision = fixture.store.check_preferences_revision().unwrap();
    fixture
        .controller
        .state
        .lock()
        .unwrap()
        .targets
        .entry(DetectorId::OverpoweredSubagents)
        .or_default()
        .push_back(TimedTarget {
            id: "native-s".into(),
            value: target,
            check_preferences_revision: revision,
            smart_master_generation: None,
            created_at_epoch: 10,
        });
    let review = fixture
        .controller
        .prepare_auto_fix_with_receipt_at(&fixture.store, "native-s", 10, Some(&authority))
        .unwrap();
    assert_eq!(review.effect, AutoFixEffect::WorkerModelSelection);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&fixture.path).unwrap())
            .unwrap()["agents"]["other"]["model"],
        "openai/gpt-5.6-sol"
    );
    let mut changed = definition.clone();
    changed.config_expected_value = Some("anthropic/gpt-5.6-sol".into());
    assert!(opencode_recovery_subject(&changed, ConfigSetting::SubagentModel).is_none());
    changed = definition;
    changed.canonical_identity = "{}".into();
    assert!(opencode_recovery_subject(&changed, ConfigSetting::SubagentModel).is_none());
}

#[test]
fn native_s_missing_or_mismatched_bindings_remain_prompt_only() {
    for binding in [
        None,
        Some(SubagentConfigBinding {
            provider_id: "custom".into(),
            ..binding()
        }),
        Some(SubagentConfigBinding {
            model_id: "gpt-5.5".into(),
            ..binding()
        }),
        Some(SubagentConfigBinding {
            agent_id: "absent".into(),
            ..binding()
        }),
    ] {
        let fixture = Fixture::new(binding);
        let authority =
            fixture.authority(json!({"agents":{"review":{"model":"openai/gpt-5.6-sol"}}}));
        let target = fixture.resolve(Some(&authority));
        assert!(target.config.is_none());
        assert!(target.physical_target_key.is_none());
        assert!(remediation_prompt(target.finding()).is_ok());
    }
}

#[test]
fn native_s_rejects_unproven_consumers_and_current_route_overrides() {
    let fixture = Fixture::new(Some(binding()));
    for config in [
        json!({"agents":{"review":{"model":"anthropic/gpt-5.6-sol"}}}),
        json!({"agents":{"review":{"model":"openai/gpt-5.6-sol#high"}}}),
        json!({"agents":{"review":{"model":"openai/gpt-5.6-sol"}},"providers":{"openai":{"settings":{"baseURL":"https://invalid.example"}}}}),
        json!({"agents":{"review":{"model":"openai/gpt-5.6-sol"}},"providers":{"openai":{"models":{"gpt-5.6-luna":{"modelID":"other"}}}}}),
    ] {
        let authority = fixture.authority(config);
        assert!(fixture.resolve(Some(&authority)).config.is_none());
    }
    assert!(fixture.resolve(None).config.is_none());
}

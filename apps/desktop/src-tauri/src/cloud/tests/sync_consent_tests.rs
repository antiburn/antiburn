use super::*;

fn consent() -> serde_json::Value {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let group = |path: &str| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(
            serde_json::to_vec(&("native", path)).unwrap(),
        ))
    };
    let choices = BTreeMap::from([
        (group("/Example/project"), true),
        (group("/Example/excluded"), false),
    ]);
    serde_json::json!({
        "version": ONGOING_CONSENT_VERSION, "accountId": "account", "destinationId": "personal",
        "selectedSources": [{"sourceKind":"claude_jsonl","path":"/Example/sources"}],
        "projects": [{"sourceKind":"claude_jsonl","sourcePath":"/Example/sources","projectPath":"/Example/project"}],
        "projectChoices": choices,
        "fullContent": true, "ongoing": true, "attachments": false,
    })
}

#[test]
fn cloud_sync_consent_accepts_the_exact_project_choices_field_with_false_values() {
    let input = consent();
    let parsed: Consent = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&parsed.project_choices).unwrap(),
        input["projectChoices"]
    );
    assert_eq!(
        parsed
            .project_choices
            .values()
            .filter(|selected| !**selected)
            .count(),
        1
    );
}

#[test]
fn cloud_sync_consent_requires_an_explicit_project_choices_snapshot() {
    let mut input = consent();
    input.as_object_mut().unwrap().remove("projectChoices");
    assert!(serde_json::from_value::<Consent>(input).is_err());
}

#[test]
fn cloud_sync_consent_status_choices_do_not_populate_permission_fields() {
    let parsed: Consent = serde_json::from_value(consent()).unwrap();
    let value = serde_json::to_value(Status {
        project_choices: parsed.project_choices.clone(),
        ..Status::default()
    })
    .unwrap();
    assert_eq!(
        value["projectChoices"],
        serde_json::to_value(parsed.project_choices).unwrap()
    );
    assert!(value.get("project_choices").is_none());
    assert_eq!(value["projects"], serde_json::json!([]));
    assert_eq!(value["selectedSources"], serde_json::json!([]));
}

#[test]
fn cloud_move_guards_use_camel_case_on_both_commands() {
    let mut input = consent();
    assert!(
        serde_json::from_value::<Consent>(input.clone())
            .unwrap()
            .move_from
            .is_none()
    );
    let move_from = serde_json::json!({ "destinationId": "team", "policyRevision": "after-off" });
    input["moveFrom"] = move_from.clone();
    let parsed: Consent = serde_json::from_value(input.clone()).unwrap();
    let guard = parsed.move_from.unwrap();
    assert_eq!(guard.destination_id, "team");
    assert_eq!(guard.policy_revision, "after-off");

    let mut change = serde_json::json!({
        "accountId": "account", "destinationId": "team", "groupId": "group",
        "selected": false, "projects": [], "folderPaths": ["/Example/project"],
        "expectedPolicyRevision": "before-off", "expectedRemovals": input["projects"],
    });
    let parsed: ProjectChange = serde_json::from_value(change.clone()).unwrap();
    assert_eq!(parsed.expected_removals.unwrap().len(), 1);
    assert!(parsed.move_from.is_none());

    change.as_object_mut().unwrap().remove("expectedRemovals");
    change["selected"] = true.into();
    change["projects"] = input["projects"].clone();
    change["moveFrom"] = move_from;
    let parsed: ProjectChange = serde_json::from_value(change.clone()).unwrap();
    assert!(parsed.expected_removals.is_none());
    assert_eq!(parsed.move_from.unwrap().policy_revision, "after-off");
    change["moveFrom"] = serde_json::json!({ "destinationId": "team" });
    assert!(serde_json::from_value::<ProjectChange>(change).is_err());
}

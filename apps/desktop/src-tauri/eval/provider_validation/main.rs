#[path = "../support/mod.rs"]
mod support;
pub(crate) use support::jev_cloudflare;
pub(crate) mod jev {
    pub(crate) use crate::support::config;
}

use antiburn_local::analysis::jev::{
    JevInputWindow, JevQuestion, JevWorkItem, pack_work_items_with_capabilities,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[tokio::test]
#[ignore = "One authorized live call through the selected production provider"]
async fn live() -> Result<(), String> {
    let suite = support::selection::suite();
    let provider = support::provider::configuration();
    let client = match support::provider::EvalClient::from_environment().await {
        Ok(client) => client,
        Err(error) => {
            let report = json!({"check":"provider_validation","suite":suite,"provider":provider.identity(),"failure":error.to_string()});
            let path = support::capture::report("provider_validation", &suite, &report);
            return Err(format!(
                "Provider setup failed: {error}; report={}",
                path.display()
            ));
        }
    };
    let items = support::selection::select(
        vec![JevWorkItem {
            id: "completed-task".into(),
            window: JevInputWindow {
                fields: json!({"text":"The task is to read one configuration file. The assistant read it and reported its contents."}),
                evidence: vec![],
            },
            questions: BTreeMap::from([
                (
                    "sufficient".into(),
                    JevQuestion::Noul {
                        instructions: json!("Was the requested task completed?"),
                        criteria: Some(json!({"true":"Completed","false":"Not completed"})),
                    },
                ),
                (
                    "scope".into(),
                    JevQuestion::Choice {
                        instructions: json!("Classify the work against the task."),
                        criteria: BTreeMap::from([
                            ("within".into(), json!("The work stays within the task.")),
                            ("outside".into(), json!("The work exceeds the task.")),
                            (
                                "unknown".into(),
                                json!("The evidence does not resolve scope."),
                            ),
                        ]),
                    },
                ),
            ]),
        }],
        |item| &item.id,
    );
    let usage = Arc::new(Mutex::new(support::run::RunUsage::default()));
    let packed = pack_work_items_with_capabilities(&items, &provider.capabilities);
    let mut errors = Vec::new();
    for batch in &packed.batches {
        if let Err(error) =
            support::run::evaluate_batch(&client, &usage, "completed-task", "protocol", batch).await
        {
            errors.push(error.to_string());
            break;
        }
    }
    let stopped = support::run::stop_reason(
        false,
        !packed.skipped_item_ids.is_empty() || packed.batches.is_empty(),
        !errors.is_empty(),
    );
    let report = json!({"check":"provider_validation","suite":suite,"provider":provider.identity(),"skipped":packed.skipped_item_ids,
        "errors":errors,"stopped":stopped,"usage":support::run::usage_report(&usage.lock().expect("Usage lock"))});
    println!("{report}");
    let path = support::capture::report("provider_validation", &suite, &report);
    if stopped.is_some() {
        return Err(format!(
            "Protocol diagnostic failed; report={}",
            path.display()
        ));
    }
    Ok(())
}

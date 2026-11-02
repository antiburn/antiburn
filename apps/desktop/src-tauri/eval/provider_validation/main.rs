#[path = "../support/protocol.rs"]
mod support;
#[path = "../../src/store/burn_checks/trace_capture_tests.rs"]
mod trace_capture;
use support::fixtures;
pub(crate) use support::{jev_cloudflare, jev_ollama};
pub(crate) mod jev {
    pub(crate) use crate::support::config;
}

use antiburn_local::analysis::jev::validate_jev_request_with_capabilities;
use serde_json::json;
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
    let cases = support::selection::select(vec![fixtures::CASE_ID], |id| id);
    let request = fixtures::request(&provider.capabilities.model);
    let usage = Arc::new(Mutex::new(support::run::RunUsage::default()));
    let mut errors = Vec::new();
    for case_id in &cases {
        if let Err(error) = validate_jev_request_with_capabilities(&request, &provider.capabilities)
        {
            errors.push(error.to_string());
            break;
        }
        if let Err(error) =
            support::run::evaluate_request(&client, &usage, case_id, "protocol", &request).await
        {
            errors.push(error.to_string());
            break;
        }
    }
    let stopped = support::run::stop_reason(false, cases.is_empty(), !errors.is_empty());
    let report = json!({"check":"provider_validation","suite":suite,"provider":provider.identity(),"cases":cases,
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

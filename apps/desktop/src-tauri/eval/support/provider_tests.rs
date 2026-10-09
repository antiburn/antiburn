use super::*;
use antiburn_local::analysis::jev::{
    JevInputWindow, JevQuestion, JevWorkItem, pack_work_items_with_capabilities,
};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

#[test]
fn presets_bind_exact_models_routes_and_response_modes() {
    for name in [
        "jev",
        "ollama-nimble",
        "ollama-clef-flash",
        "ollama-tev-small",
        "cloudflare-clef",
        "cloudflare-clef-flash",
        "custom-jev-direct",
        "custom-ollama-nimble-direct",
        "custom-ollama-clef-flash-direct",
        "custom-cloudflare-clef-envelope",
        "custom-cloudflare-clef-flash-envelope",
    ] {
        let preset = ProviderPreset::parse(name).unwrap();
        let connection = preset.connection(Some("synthetic-account")).unwrap();
        connection.validate().unwrap();
        assert_eq!(connection.model, preset.model());
        assert!(connection.credential.is_none());
        let capabilities = connection.capabilities().unwrap();
        assert!(capabilities.usable_input_tokens().unwrap() > 0);
        assert!(capabilities.usable_state_tokens().unwrap() > 0);
        if connection.provider == SystemOneProvider::Custom {
            assert_eq!(
                connection
                    .context_override
                    .as_ref()
                    .unwrap()
                    .total_input_tokens,
                Some(if preset.local() { 8192 } else { 65_536 })
            );
        } else {
            assert!(connection.context_override.is_none());
        }
        assert_eq!(batch(&capabilities).work_item_ids, ["exact-work"]);
        let endpoint = connection.inference_endpoint().unwrap();
        if preset.local() {
            assert_eq!(endpoint, "http://127.0.0.1:11434/v1/systemone");
        } else if preset.cloudflare() {
            assert_eq!(
                endpoint,
                format!(
                    "https://api.cloudflare.com/client/v4/accounts/synthetic-account/ai/run/@cf/cloudflare/{}",
                    preset.model()
                )
            );
        } else {
            assert_eq!(endpoint, "https://api.typesafe.ai/v1/systemone");
        }
        assert_eq!(
            connection.response_mode == SystemOneResponseMode::CloudflareEnvelope,
            name.starts_with("custom-cloudflare-")
        );
    }
    assert!(ProviderPreset::parse("custom").is_err());
    assert!(ProviderPreset::parse("ollama-arbitrary").is_err());
    assert!(ProviderPreset::CloudflareClef.connection(None).is_err());
}

#[test]
fn custom_preset_limits_preserve_explicit_bounds_and_lower_discovered_limits() {
    let mut connection = ProviderPreset::CustomOllamaNimbleDirect
        .connection(None)
        .unwrap();
    connection.context_override = Some(ContextLimitOverride {
        total_input_tokens: Some(6144),
        state_and_longest_question_tokens: Some(5120),
        runtime_context_tokens: Some(7168),
    });
    connection.validate().unwrap();
    let mut discovered = ModelCapabilities::jev_default();
    discovered.runtime_context_tokens =
        antiburn_local::analysis::jev::capabilities::CapabilityLimit::known(
            5000,
            antiburn_local::analysis::jev::capabilities::CapabilitySource::RuntimeMetadata,
        );
    let capabilities = connection.apply_capability_overrides(discovered);
    assert_eq!(capabilities.total_input_tokens.value, Some(6144));
    assert_eq!(
        capabilities.state_and_longest_question_tokens.value,
        Some(5120)
    );
    assert_eq!(capabilities.runtime_context_tokens.value, Some(5000));
    assert_eq!(capabilities.usable_input_tokens(), Some(904));
    assert_eq!(capabilities.usable_state_tokens(), Some(904));
}

fn batch(capabilities: &ModelCapabilities) -> antiburn_local::analysis::jev::JevRequestBatch {
    let item = JevWorkItem {
        id: "exact-work".into(),
        window: JevInputWindow {
            fields: json!({"text":"synthetic evidence"}),
            evidence: vec![],
        },
        questions: BTreeMap::from([(
            "q".into(),
            JevQuestion::Noul {
                instructions: json!("Is the evidence sufficient?"),
                criteria: Some(json!({"true":"sufficient","false":"insufficient"})),
            },
        )]),
    };
    pack_work_items_with_capabilities(&[item], capabilities)
        .batches
        .remove(0)
}

async fn loopback_batch(
    mode: SystemOneResponseMode,
    missing_answer: bool,
    ollama: bool,
    context_rejected: bool,
) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let endpoint = format!("{base}/exact/path?route=validation");
    let mut capabilities = ModelCapabilities::jev_default();
    capabilities.model = "local-model".into();
    capabilities.model_revision = Some("synthetic-digest".into());
    let batch = batch(&capabilities);
    let answer_id = batch.request.questions.keys().next().unwrap().clone();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let request = loop {
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                if bytes.len() >= end + 4 + length {
                    assert!(headers.starts_with(if ollama {
                        "POST /v1/systemone HTTP/1.1"
                    } else {
                        "POST /exact/path?route=validation HTTP/1.1"
                    }));
                    assert!(!headers.to_ascii_lowercase().contains("authorization:"));
                    break serde_json::from_slice::<Value>(&bytes[end + 4..end + 4 + length])
                        .unwrap();
                }
            }
        };
        assert_eq!(request["model"], "local-model");
        let answers = if missing_answer {
            json!({})
        } else {
            json!({answer_id:{"type":"noul","noul":0.9}})
        };
        let response = json!({"model":"local-model","answers":answers,"usage":{"input_tokens":2048,"output_tokens":1}});
        let body = if context_rejected {
            json!({"error":"prompt exceeds context; input is never truncated"})
        } else if mode == SystemOneResponseMode::CloudflareEnvelope {
            json!({"success":true,"result":response})
        } else {
            response
        }
        .to_string();
        let status = if context_rejected {
            "400 Bad Request"
        } else {
            "200 OK"
        };
        write!(
            socket,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let connection = SystemOneConnection {
        provider: SystemOneProvider::Custom,
        endpoint: SystemOneEndpoint::ExactUrl(endpoint.clone()),
        model: capabilities.model.clone(),
        model_revision: capabilities.model_revision.clone(),
        response_mode: mode,
        credential: None,
        revision: 1,
        context_override: Some(ContextLimitOverride {
            total_input_tokens: capabilities.total_input_tokens.value,
            ..ContextLimitOverride::default()
        }),
    };
    connection.validate().unwrap();
    let configuration = EvalConfiguration {
        preset: ProviderPreset::CustomOllamaNimbleDirect,
        connection,
        capabilities,
    };
    let transport = if ollama {
        Transport::Ollama(jev_ollama::OllamaClient::new(&base, None).unwrap())
    } else {
        Transport::Custom {
            endpoint,
            credential: None,
            mode,
        }
    };
    let client = EvalClient {
        configuration,
        transport,
    };
    let usage = Arc::new(Mutex::new(super::super::run::RunUsage::default()));
    let result =
        super::super::run::evaluate_batch(&client, &usage, "case", "assessment", &batch).await;
    server.join().unwrap();
    let failed = missing_answer || context_rejected;
    assert_eq!(result.is_err(), failed);
    if context_rejected {
        assert_eq!(result, Err(JevError::ContextRejected));
    }
    let usage = usage.lock().unwrap();
    assert_eq!(usage.requests, 1);
    assert_eq!(usage.calls.len(), 1);
    let call = &usage.calls[0];
    assert_eq!(
        client.configuration.capabilities.model_revision.as_deref(),
        Some("synthetic-digest")
    );
    if !failed {
        assert_eq!(call["response"]["model"], "local-model");
    }
    assert_eq!(call["work_item_ids"], json!(["exact-work"]));
    assert_eq!(call["response_validated"], !failed);
    if failed {
        assert!(call["usage"].is_null());
        assert!(!call["failure"].is_null());
        assert_eq!(usage.input_tokens, 0);
    } else {
        assert!(2048 > batch.serialized_bytes);
        assert_eq!(usage.input_tokens, 2048);
        assert_eq!(usage.output_tokens, 1);
    }
}

#[tokio::test]
async fn shared_runner_uses_production_custom_direct_and_envelope_transports() {
    loopback_batch(SystemOneResponseMode::Direct, false, false, false).await;
    loopback_batch(
        SystemOneResponseMode::CloudflareEnvelope,
        false,
        false,
        false,
    )
    .await;
}

#[tokio::test]
async fn shared_runner_uses_production_ollama_transport_and_keeps_failed_usage_unknown() {
    loopback_batch(SystemOneResponseMode::Direct, false, true, false).await;
    loopback_batch(SystemOneResponseMode::Direct, true, true, false).await;
}

#[tokio::test]
async fn ollama_context_rejection_reaches_the_shared_runner_without_claiming_usage() {
    loopback_batch(SystemOneResponseMode::Direct, false, true, true).await;
}

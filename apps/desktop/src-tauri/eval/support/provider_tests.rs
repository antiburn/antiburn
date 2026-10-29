use super::*;
use antiburn_local::analysis::jev::validate_jev_request_with_capabilities;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use super::super::fixtures;

#[test]
fn presets_bind_exact_models_routes_and_response_modes() {
    for (name, model) in [
        ("jev", antiburn_local::analysis::jev::NATIVE_MODEL_ALIAS),
        ("ollama-nimble", "nimble:latest"),
        ("ollama-clef-flash", "clef-flash:latest"),
        ("ollama-tev-small", "tev1:0.8b"),
        ("cloudflare-clef", "clef"),
        ("cloudflare-clef-flash", "clef-flash"),
        ("custom-jev-direct", "jev-1.14.0"),
        ("custom-ollama-nimble-direct", "nimble:latest"),
        ("custom-ollama-clef-flash-direct", "clef-flash:latest"),
        ("custom-cloudflare-clef-envelope", "clef"),
        ("custom-cloudflare-clef-flash-envelope", "clef-flash"),
    ] {
        let preset = ProviderPreset::parse(name).unwrap();
        let connection = preset.connection(Some("synthetic-account")).unwrap();
        connection.validate().unwrap();
        assert_eq!(connection.model, model);
        assert_eq!(
            connection.provider,
            if name == "jev" {
                SystemOneProvider::Jev
            } else if name.starts_with("ollama-") {
                SystemOneProvider::Ollama
            } else if name.starts_with("cloudflare-") {
                SystemOneProvider::Cloudflare
            } else {
                SystemOneProvider::Custom
            }
        );
        assert!(connection.credential.is_none());
        assert!(connection.context_override.is_none());
        let capabilities = connection.capabilities().unwrap();
        assert_eq!(capabilities.model, model);
        if preset.local() {
            assert_eq!(capabilities.usable_input_tokens(), None);
            assert_eq!(capabilities.usable_state_tokens(), None);
            assert_eq!(capabilities.output, Default::default());
        } else {
            assert!(capabilities.usable_input_tokens().unwrap() > 0);
            assert!(capabilities.usable_state_tokens().unwrap() > 0);
        }
        if preset.cloudflare() && model == "clef-flash" {
            assert_eq!(capabilities.total_input_tokens.value, Some(24_576));
        }
        if capabilities.usable_state_tokens().is_some() {
            let request = fixtures::request(&capabilities.model);
            validate_jev_request_with_capabilities(&request, &capabilities).unwrap();
            assert_eq!(
                request.questions.keys().collect::<Vec<_>>(),
                ["completion", "scope", "sufficient"]
            );
            assert!(request.state.get("work_items").is_none());
        } else {
            assert!(preset.local());
            assert!(matches!(
                validate_jev_request_with_capabilities(
                    &fixtures::request(&capabilities.model),
                    &capabilities,
                ),
                Err(JevError::RequestTokenLimitExceeded { maximum: 0, .. })
            ));
        }
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
fn direct_request_keeps_all_questions_and_obeys_selected_capabilities() {
    let mut capabilities = ModelCapabilities::jev_default();
    let request = fixtures::request(&capabilities.model);
    let bytes = validate_jev_request_with_capabilities(&request, &capabilities).unwrap();
    assert_eq!(bytes, serde_json::to_vec(&request).unwrap().len());

    capabilities.questions_per_request.value = Some(1);
    assert_eq!(
        validate_jev_request_with_capabilities(&request, &capabilities),
        Err(JevError::QuestionLimitExceeded)
    );
    assert_eq!(request.questions.len(), 3);

    capabilities.questions_per_request.value = Some(3);
    capabilities.request_body_bytes.value = Some(bytes as u64 - 1);
    assert_eq!(
        validate_jev_request_with_capabilities(&request, &capabilities),
        Err(JevError::RequestTooLarge {
            bytes,
            maximum: bytes - 1
        })
    );
}

#[test]
fn native_and_custom_eval_presets_use_their_production_route_limits() {
    let native = ProviderPreset::Jev.connection(None).unwrap();
    let custom = ProviderPreset::CustomJevDirect.connection(None).unwrap();
    assert_eq!(
        native.model,
        antiburn_local::analysis::jev::NATIVE_MODEL_ALIAS
    );
    assert_eq!(
        native.capabilities().unwrap().total_input_tokens.value,
        Some(262_000)
    );
    assert_eq!(custom.model, "jev-1.14.0");
    let standard = custom.capabilities().unwrap();
    assert_eq!(standard.total_input_tokens.value, Some(65_536));
    assert_eq!(
        standard.state_and_longest_question_tokens.value,
        Some(32_768)
    );
}

#[test]
fn custom_preset_ignores_legacy_bounds_and_preserves_discovered_limits() {
    let mut connection = ProviderPreset::CustomOllamaNimbleDirect
        .connection(None)
        .unwrap();
    let unknown = connection.capabilities().unwrap();
    assert_eq!(unknown.usable_input_tokens(), None);
    assert_eq!(unknown.usable_state_tokens(), None);
    connection.context_override = Some(ContextLimitOverride {
        total_input_tokens: Some(6144),
        state_and_longest_question_tokens: Some(5120),
        runtime_context_tokens: Some(7168),
    });
    connection.validate().unwrap();
    assert_eq!(connection.capabilities().unwrap(), unknown);
    let mut discovered = ModelCapabilities {
        model: connection.model.clone(),
        model_revision: Some("synthetic-digest".into()),
        ..ModelCapabilities::jev_default()
    };
    discovered.runtime_context_tokens =
        antiburn_local::analysis::jev::capabilities::CapabilityLimit::known(
            5000,
            antiburn_local::analysis::jev::capabilities::CapabilitySource::RuntimeMetadata,
        );
    let capabilities = connection.apply_capability_overrides(discovered.clone());
    assert_eq!(capabilities, discovered);
    assert_eq!(capabilities.total_input_tokens.value, Some(65_536));
    assert_eq!(
        capabilities.state_and_longest_question_tokens.value,
        Some(32_768)
    );
    assert_eq!(capabilities.runtime_context_tokens.value, Some(5000));
    assert_eq!(capabilities.rendering_reserve_tokens, 4096);
    assert_eq!(capabilities.usable_input_tokens(), Some(904));
    assert_eq!(capabilities.usable_state_tokens(), Some(904));
}

async fn loopback_request(
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
    let request = fixtures::request(&capabilities.model);
    let expected_request = serde_json::to_value(&request).unwrap();
    let server = std::thread::spawn(move || {
        if ollama {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut headers = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).unwrap();
                assert!(count > 0);
                headers.extend_from_slice(&buffer[..count]);
                assert!(headers.len() < 8192);
                if headers.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    break;
                }
            }
            let headers = String::from_utf8(headers).unwrap().to_ascii_lowercase();
            assert!(headers.starts_with("get /api/tags http/1.1\r\n"));
            assert!(!headers.contains("authorization:"));
            let tags = json!({"models": [{"name": "local-model", "digest": "synthetic-digest"}]})
                .to_string();
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{tags}",
                tags.len()
            )
            .unwrap();
        }
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
                    assert!(
                        !headers
                            .to_ascii_lowercase()
                            .contains("typesafe-danger-zone-long-context:")
                    );
                    break serde_json::from_slice::<Value>(&bytes[end + 4..end + 4 + length])
                        .unwrap();
                }
            }
        };
        assert_eq!(request, expected_request);
        let answers = if missing_answer {
            json!({"scope":{"type":"choice","choice":"within","probabilities":{"within":0.9,"outside":0.05,"unknown":0.05},"confidence":0.9}})
        } else {
            fixtures::expected_answers()
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
    let result = super::super::run::evaluate_request(
        &client,
        &usage,
        fixtures::CASE_ID,
        "protocol",
        &request,
    )
    .await;
    server.join().unwrap();
    let failed = missing_answer || context_rejected;
    assert_eq!(result.is_err(), failed);
    if context_rejected {
        assert_eq!(result, Err(JevError::ContextRejected));
    } else if missing_answer {
        assert_eq!(
            result,
            Err(if ollama {
                JevError::ResponseDecode
            } else {
                JevError::ResponseAnswerCountMismatch
            })
        );
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
    assert!(call.get("work_item_ids").is_none());
    assert_eq!(call["case_id"], fixtures::CASE_ID);
    assert_eq!(
        call["question_ids"],
        json!(["completion", "scope", "sufficient"])
    );
    assert_eq!(call["questions"], 3);
    assert_eq!(call["response_validated"], !failed);
    if failed {
        assert!(call["usage"].is_null());
        assert!(!call["failure"].is_null());
        assert_eq!(usage.input_tokens, 0);
    } else {
        assert!(2048 > serde_json::to_vec(&request).unwrap().len());
        assert_eq!(usage.input_tokens, 2048);
        assert_eq!(usage.output_tokens, 1);
    }
}

#[tokio::test]
async fn shared_runner_uses_production_custom_direct_and_envelope_transports() {
    loopback_request(SystemOneResponseMode::Direct, false, false, false).await;
    loopback_request(SystemOneResponseMode::Direct, true, false, false).await;
    loopback_request(
        SystemOneResponseMode::CloudflareEnvelope,
        false,
        false,
        false,
    )
    .await;
    loopback_request(
        SystemOneResponseMode::CloudflareEnvelope,
        true,
        false,
        false,
    )
    .await;
}

#[tokio::test]
async fn shared_runner_uses_production_ollama_transport_and_keeps_failed_usage_unknown() {
    loopback_request(SystemOneResponseMode::Direct, false, true, false).await;
    loopback_request(SystemOneResponseMode::Direct, true, true, false).await;
}

#[tokio::test]
async fn ollama_context_rejection_reaches_the_shared_runner_without_claiming_usage() {
    loopback_request(SystemOneResponseMode::Direct, false, true, true).await;
}

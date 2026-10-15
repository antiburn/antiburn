use super::*;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::jev::config::{
    ContextLimitOverride, SystemOneConnection, SystemOneEndpoint, SystemOneProvider,
    SystemOneResponseMode,
};
use crate::jev::worker::JevCheckDescriptor;
use crate::jev::worker::{TestBatchExecution, execute_batch_for_test, unix_now};
use crate::store::Store;
use antiburn_local::analysis::ignored_instructions::SessionContentEvidence;
use antiburn_local::analysis::jev::capabilities::{CapabilityLimit, ModelCapabilities};
use antiburn_local::analysis::jev::{
    JevAnswer, JevCheck, JevQuestion, JevRequest, JevResponse, JevUsage,
    estimate_jev_rendered_question_tokens, run_jev_check_prepared,
};
use antiburn_local::analysis::{SelectedContentRequest, SourceFormat};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinSet;

#[derive(Default)]
struct TransportMetrics {
    requests: AtomicUsize,
    request_bytes: AtomicUsize,
    response_bytes: AtomicUsize,
    estimated_tokens: AtomicUsize,
    synthetic_input_tokens: AtomicUsize,
    synthetic_output_tokens: AtomicUsize,
    max_questions: AtomicUsize,
    max_rendered_tokens: AtomicUsize,
    active: AtomicUsize,
    peak: AtomicUsize,
}

#[derive(Clone)]
struct ResponseProfile {
    choice: String,
    criteria: BTreeSet<String>,
    capabilities: ModelCapabilities,
}

struct LoopbackProvider {
    endpoint: String,
    stop: tokio::sync::watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    metrics: Arc<TransportMetrics>,
    responses: Arc<RwLock<BTreeMap<String, ResponseProfile>>>,
}

impl LoopbackProvider {
    async fn start() -> anyhow::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/v1/systemone", listener.local_addr()?);
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        let metrics = Arc::new(TransportMetrics::default());
        let server_metrics = Arc::clone(&metrics);
        let responses = Arc::new(RwLock::new(BTreeMap::new()));
        let server_responses = Arc::clone(&responses);
        let task = tokio::spawn(async move {
            let mut requests = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted?;
                        let metrics = Arc::clone(&server_metrics);
                        let responses = Arc::clone(&server_responses);
                        requests.spawn(async move { serve_one(socket, metrics, responses).await });
                    }
                    changed = stopped.changed() => {
                        if changed.is_err() || *stopped.borrow() {
                            break;
                        }
                    }
                }
            }
            while let Some(result) = requests.join_next().await {
                result??;
            }
            Ok(())
        });
        Ok(Self {
            endpoint,
            stop,
            task: Some(task),
            metrics,
            responses,
        })
    }

    fn register_response(
        &self,
        model: impl Into<String>,
        profile: ResponseProfile,
    ) -> anyhow::Result<()> {
        let model = model.into();
        let mut responses = self
            .responses
            .write()
            .map_err(|_| anyhow::anyhow!("loopback response registry lock failed"))?;
        anyhow::ensure!(
            responses.insert(model.clone(), profile).is_none(),
            "loopback response case was registered twice: {model}"
        );
        Ok(())
    }

    async fn stop(mut self) -> anyhow::Result<Arc<TransportMetrics>> {
        let _ = self.stop.send(true);
        self.task
            .take()
            .ok_or_else(|| anyhow::anyhow!("loopback server task was already joined"))?
            .await??;
        Ok(Arc::clone(&self.metrics))
    }
}

impl Drop for LoopbackProvider {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

async fn serve_one(
    mut socket: tokio::net::TcpStream,
    metrics: Arc<TransportMetrics>,
    responses: Arc<RwLock<BTreeMap<String, ResponseProfile>>>,
) -> anyhow::Result<()> {
    let active = metrics.active.fetch_add(1, Ordering::AcqRel) + 1;
    metrics.peak.fetch_max(active, Ordering::AcqRel);
    let result = async {
        let request_bytes = read_http_request(&mut socket).await?;
        let body_start = request_bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
            .ok_or_else(|| anyhow::anyhow!("loopback request has no header boundary"))?;
        let body = &request_bytes[body_start..];
        let request: JevRequest = serde_json::from_slice(body)?;
        metrics.requests.fetch_add(1, Ordering::AcqRel);
        metrics
            .request_bytes
            .fetch_add(request_bytes.len(), Ordering::Relaxed);
        metrics
            .max_questions
            .fetch_max(request.questions.len(), Ordering::Relaxed);
        let profile = responses
            .read()
            .map_err(|_| anyhow::anyhow!("loopback response registry lock failed"))?
            .get(&request.model)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown loopback case model: {}", request.model))?;
        let rendered_tokens = final_fit_tokens(&request, &profile.capabilities)?;
        let rendered_metric = usize::try_from(rendered_tokens)
            .map_err(|_| anyhow::anyhow!("rendered token estimate exceeds the metric range"))?;
        let input_limit = profile
            .capabilities
            .usable_input_tokens()
            .ok_or_else(|| anyhow::anyhow!("loopback case has no usable input limit"))?;
        if profile.capabilities.uses_ollama_rendered_accounting() {
            let state_limit = profile
                .capabilities
                .usable_state_tokens()
                .unwrap_or_default();
            anyhow::ensure!(
                rendered_tokens <= state_limit,
                "model={} rendered request tokens={} state_limit={state_limit}",
                request.model,
                rendered_tokens
            );
        } else {
            anyhow::ensure!(
                rendered_tokens <= input_limit,
                "model={} rendered request tokens={} input_limit={input_limit}",
                request.model,
                rendered_tokens
            );
        }
        anyhow::ensure!(
            request.questions.len()
                <= profile
                    .capabilities
                    .questions_per_request
                    .value
                    .unwrap_or_default() as usize,
            "model={} request question cap exceeded: {}",
            request.model,
            request.questions.len()
        );
        metrics
            .estimated_tokens
            .fetch_add(rendered_metric, Ordering::Relaxed);
        metrics
            .max_rendered_tokens
            .fetch_max(rendered_metric, Ordering::Relaxed);
        let input_tokens = 480 + u64::try_from(body.len().div_ceil(48)).unwrap_or(u64::MAX);
        let output_tokens = 16;
        metrics
            .synthetic_input_tokens
            .fetch_add(input_tokens as usize, Ordering::Relaxed);
        metrics
            .synthetic_output_tokens
            .fetch_add(output_tokens as usize, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let response = response_for_request(&request, &profile, input_tokens, output_tokens)?;
        let response_body = serde_json::to_vec(&response)?;
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        );
        metrics
            .response_bytes
            .fetch_add(header.len() + response_body.len(), Ordering::Relaxed);
        socket.write_all(header.as_bytes()).await?;
        socket.write_all(&response_body).await?;
        socket.flush().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    metrics.active.fetch_sub(1, Ordering::AcqRel);
    result
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = socket.read(&mut chunk).await?;
        if count == 0 {
            anyhow::bail!("loopback provider received an incomplete request");
        }
        bytes.extend_from_slice(&chunk[..count]);
        let Some(boundary) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let header = std::str::from_utf8(&bytes[..boundary])?;
        let content_length = header
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            })
            .ok_or_else(|| anyhow::anyhow!("loopback request has no content length"))?;
        if bytes.len() >= boundary + 4 + content_length {
            return Ok(bytes);
        }
    }
}

fn response_for_request(
    request: &JevRequest,
    profile: &ResponseProfile,
    input_tokens: u64,
    output_tokens: u64,
) -> anyhow::Result<JevResponse> {
    let answers = request
        .questions
        .iter()
        .map(|(id, question)| {
            let answer = match question {
                JevQuestion::Choice { criteria, .. } => {
                    let actual = criteria.keys().cloned().collect::<BTreeSet<_>>();
                    anyhow::ensure!(
                        actual == profile.criteria,
                        "model={} question={id} criteria mismatch: actual={actual:?} expected={:?}",
                        request.model,
                        profile.criteria
                    );
                    anyhow::ensure!(
                        criteria.contains_key(&profile.choice),
                        "model={} question={id} does not define selected criterion {}",
                        request.model,
                        profile.choice
                    );
                    let probabilities = criteria
                        .keys()
                        .map(|key| {
                            (
                                key.clone(),
                                if key == &profile.choice {
                                    0.96
                                } else {
                                    0.04 / criteria.len().saturating_sub(1).max(1) as f64
                                },
                            )
                        })
                        .collect();
                    JevAnswer::Choice {
                        choice: profile.choice.clone(),
                        probabilities,
                        confidence: 0.96,
                    }
                }
                JevQuestion::Noul { .. } | JevQuestion::Score { .. } => anyhow::bail!(
                    "model={} question={id} used an unsupported benchmark question type",
                    request.model
                ),
            };
            Ok((id.clone(), answer))
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(JevResponse {
        model: request.model.clone(),
        answers,
        usage: JevUsage {
            input_tokens,
            output_tokens,
        },
    })
}

fn final_fit_tokens(request: &JevRequest, capabilities: &ModelCapabilities) -> anyhow::Result<u64> {
    if capabilities.uses_ollama_rendered_accounting() {
        return Ok(estimate_jev_rendered_question_tokens(
            request,
            capabilities,
        )?);
    }
    let serialized = serde_json::to_string(request)?;
    let estimated = capabilities.estimate_text_tokens(&serialized);
    let state = serde_json::to_string(&request.state)?;
    let state_tokens = capabilities.estimate_text_tokens(&state);
    let longest_question_tokens = request
        .questions
        .values()
        .map(|question| {
            serde_json::to_string(question)
                .map(|serialized| capabilities.estimate_text_tokens(&serialized))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .max()
        .unwrap_or_default();
    anyhow::ensure!(
        state_tokens.saturating_add(longest_question_tokens)
            <= capabilities.usable_state_tokens().unwrap_or_default(),
        "final request state and question exceed the configured context limit"
    );
    anyhow::ensure!(
        estimated <= capabilities.usable_input_tokens().unwrap_or_default(),
        "final request exceeds the configured input limit"
    );
    Ok(estimated)
}

fn benchmark_connection(endpoint: &str, model: &str, constrained: bool) -> SystemOneConnection {
    let mut connection = SystemOneConnection::jev_default();
    connection.provider = SystemOneProvider::Custom;
    connection.endpoint = SystemOneEndpoint::ExactUrl(endpoint.to_owned());
    connection.model = model.to_owned();
    connection.credential = None;
    connection.response_mode = SystemOneResponseMode::Direct;
    connection.context_override = Some(ContextLimitOverride {
        total_input_tokens: Some(if constrained { 16_384 } else { 32_768 }),
        state_and_longest_question_tokens: Some(if constrained { 8_192 } else { 16_384 }),
        runtime_context_tokens: Some(if constrained { 8_192 } else { 16_384 }),
    });
    connection
}

fn capabilities_for_model(
    endpoint: &str,
    model: &str,
    constrained: bool,
) -> anyhow::Result<(SystemOneConnection, ModelCapabilities)> {
    let connection = benchmark_connection(endpoint, model, constrained);
    let capabilities = connection.capabilities().map_err(|error| {
        anyhow::anyhow!("benchmark connection has invalid model limits: {error:?}")
    })?;
    Ok((connection, capabilities))
}

async fn run_pipeline_regression() -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(120), async {
        run_native_worker_adapters(false).await?;
        run_long_read_session().await?;
        run_native_worker_adapters(true).await?;
        run_over_exploring_scheduler_e2e().await?;
        Ok::<(), anyhow::Error>(())
    })
    .await??;
    Ok(())
}

struct FixtureKeepAlive {
    _directories: Vec<tempfile::TempDir>,
    _native_fixture: Option<crate::scope_creep_worker::tests::NativeFixture>,
}

struct AdapterCase {
    name: String,
    model: String,
    check: &'static dyn JevCheckDescriptor,
    store: Store,
    candidate: BurnCheckCandidate,
    expected_choice: &'static str,
    expected_finding: bool,
    instruction_format: Option<SourceFormat>,
    instruction_content: Option<SessionContentEvidence>,
    skill_input: Option<crate::skill_opportunities_worker::PreparedSkillOpportunityInput>,
    _keep_alive: FixtureKeepAlive,
}

fn expected_criteria(check_id: &str) -> anyhow::Result<BTreeSet<String>> {
    let values: &[&str] = match check_id {
        "ignored_instructions" => &["conflict", "no_conflict", "uncertain"],
        "scope_creep" => &["unauthorized_extra", "in_scope", "uncertain"],
        "over_exploring" => &["likely_excess", "justified_or_minor", "uncertain"],
        "skill_opportunities" => &[
            "actionable_help",
            "already_adequate_or_unrelated",
            "uncertain",
        ],
        _ => anyhow::bail!("unsupported benchmark check: {check_id}"),
    };
    Ok(values.iter().map(|value| (*value).to_owned()).collect())
}

fn response_profile(
    check_id: &str,
    choice: &str,
    capabilities: ModelCapabilities,
) -> anyhow::Result<ResponseProfile> {
    let criteria = expected_criteria(check_id)?;
    anyhow::ensure!(
        criteria.contains(choice),
        "check={check_id} response selected an unknown criterion: {choice}"
    );
    Ok(ResponseProfile {
        choice: choice.to_owned(),
        criteria,
        capabilities,
    })
}

fn instruction_case(
    name: &str,
    model: String,
    rule: &str,
    expected_choice: &'static str,
    expected_finding: bool,
) -> anyhow::Result<AdapterCase> {
    let fixture = crate::scope_creep_worker::tests::NativeFixture::new(0);
    fixture.store.set_check_enabled(
        antiburn_local::checks::DetectorId::IgnoredInstructions,
        true,
    )?;
    fixture
        .store
        .capture_burn_check_boundaries(&[crate::ignored_instructions_worker::CHECK.id()], 0)?;
    let workspace = fixture.directory.path();
    std::fs::write(workspace.join("AGENTS.md"), rule)?;
    fixture.append(
        2,
        "assistant",
        serde_json::json!({"type":"tool","tool":"bash","callID":"benchmark-command","state":{"status":"completed","input":{"command":"cargo test --lib"},"output":"FAILED (failures=1)"}}),
    );
    let published = fixture.publish();
    fixture.store.lock().execute(
        "UPDATE session SET cwd = ?1 WHERE environment_key = ?2 AND agent = ?3 AND session_id = ?4",
        rusqlite::params![
            workspace.to_string_lossy(),
            published.session.key.environment_key,
            published.session.key.agent,
            published.session.key.session_id,
        ],
    )?;
    let mut candidate = fixture
        .store
        .burn_check_candidates_for_revision(
            crate::ignored_instructions_worker::CHECK.id(),
            &crate::ignored_instructions_worker::CHECK.evaluator_revision(),
            unix_now(),
            crate::ignored_instructions_worker::CHECK.policy().idle_secs,
            8,
        )?
        .into_iter()
        .find(|candidate| candidate.session.key.session_id == "scope")
        .ok_or_else(|| anyhow::anyhow!("case={name} Instructions candidate was not admitted"))?;
    candidate.historical = true;
    candidate.boundary_positions.clear();
    candidate.boundary_at_epoch = 0;
    Ok(AdapterCase {
        name: name.to_owned(),
        model,
        check: &crate::ignored_instructions_worker::CHECK,
        store: fixture.store.clone(),
        candidate,
        expected_choice,
        expected_finding,
        instruction_format: Some(SourceFormat::OpenCodeSqliteV2),
        instruction_content: None,
        skill_input: None,
        _keep_alive: FixtureKeepAlive {
            _directories: Vec::new(),
            _native_fixture: Some(fixture),
        },
    })
}

fn scope_case(
    name: &str,
    model: String,
    approved: bool,
    expected_choice: &'static str,
    expected_finding: bool,
) -> anyhow::Result<AdapterCase> {
    let fixture = crate::scope_creep_worker::tests::NativeFixture::new(1);
    let source_path = fixture.directory.path().join("opencode.db");
    {
        let source = rusqlite::Connection::open(&source_path)?;
        source.execute("DELETE FROM part WHERE id = 'p2'", [])?;
        source.execute("DELETE FROM message WHERE id = 'm2'", [])?;
    }
    if approved {
        fixture.append(
            2,
            "user",
            serde_json::json!({"type":"text","text":"I approve and accept the completed billing feature."}),
        );
    }
    let source = rusqlite::Connection::open(source_path)?;
    let source_messages: i64 =
        source.query_row("SELECT COUNT(*) FROM message", [], |row| row.get(0))?;
    let expected_messages = if approved { 3_i64 } else { 2_i64 };
    anyhow::ensure!(
        source_messages == expected_messages,
        "case={name} source message count={source_messages} expected={expected_messages}"
    );
    drop(source);
    let candidate = fixture.publish();
    Ok(AdapterCase {
        name: name.to_owned(),
        model,
        check: &crate::scope_creep_worker::CHECK,
        store: fixture.store.clone(),
        candidate,
        expected_choice,
        expected_finding,
        instruction_format: None,
        instruction_content: None,
        skill_input: None,
        _keep_alive: FixtureKeepAlive {
            _directories: Vec::new(),
            _native_fixture: Some(fixture),
        },
    })
}

fn over_exploring_case(name: &str, model: String, positive: bool) -> anyhow::Result<AdapterCase> {
    let read_content = "recorded parser output 123\n".repeat(320);
    anyhow::ensure!(
        read_content.len() >= 8 * 1024,
        "case={name} repeated read output is below 8 KiB"
    );
    if positive {
        let directory = tempfile::tempdir()?;
        let (store, candidate) = fixture_in_store(
            Store::open(directory.path())?,
            "over-positive",
            "user",
            &["/synthetic/parser.rs"; 2],
            &read_content,
        );
        return Ok(AdapterCase {
            name: name.to_owned(),
            model,
            check: &CHECK,
            store,
            candidate,
            expected_choice: "likely_excess",
            expected_finding: true,
            instruction_format: None,
            instruction_content: None,
            skill_input: None,
            _keep_alive: FixtureKeepAlive {
                _directories: vec![directory],
                _native_fixture: None,
            },
        });
    }

    let directory = tempfile::tempdir()?;
    let (store, candidate) = fixture_in_store(
        Store::open(directory.path())?,
        "over-hard-negative",
        "user",
        &["/synthetic/parser.rs"; 3],
        &read_content,
    );
    Ok(AdapterCase {
        name: name.to_owned(),
        model,
        check: &CHECK,
        store,
        candidate,
        expected_choice: "justified_or_minor",
        expected_finding: false,
        instruction_format: None,
        instruction_content: None,
        skill_input: None,
        _keep_alive: FixtureKeepAlive {
            _directories: vec![directory],
            _native_fixture: None,
        },
    })
}

fn benchmark_skill_fixture() -> anyhow::Result<(
    crate::scope_creep_worker::tests::NativeFixture,
    crate::skill_opportunities_worker::PreparedSkillOpportunityInput,
)> {
    let fixture = crate::scope_creep_worker::tests::NativeFixture::paged_groups(1);
    for (offset, command, output) in [
        (0, "cargo test parser", "Tests fail."),
        (1, "cargo check", "Check passes."),
    ] {
        fixture.append(
            4 + offset,
            "assistant",
            serde_json::json!({
                "type": "tool",
                "tool": "bash",
                "callID": format!("workflow-{offset}"),
                "state": {
                    "status": "completed",
                    "input": {"command": command},
                    "output": output,
                },
            }),
        );
    }
    fixture
        .store
        .set_check_enabled(antiburn_local::checks::DetectorId::SkillOpportunities, true)?;
    fixture
        .store
        .capture_burn_check_boundaries(&[crate::skill_opportunities_worker::CHECK.id()], 0)?;
    let home = fixture.directory.path().join("home");
    let workspace = fixture.directory.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&workspace)?;
    let skill_path = home.join(".opencode/skills/review-0/SKILL.md");
    let skill_directory = skill_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("skill fixture path has no parent"))?;
    std::fs::create_dir_all(skill_directory)?;
    std::fs::write(
        &skill_path,
        "---\nname: review-0\ndescription: |\n  1. Run `cargo test parser`.\n  2. Run `cargo check` and inspect lock ordering before changing code.\n---\n",
    )?;
    let mut candidate = fixture.publish();
    candidate.session.cwd = Some(workspace.to_string_lossy().into_owned());
    fixture.store.lock().execute(
        "UPDATE session SET cwd = ?1 WHERE session_id = ?2",
        rusqlite::params![candidate.session.cwd, candidate.session.key.session_id],
    )?;
    let config = crate::agent_config::ConfigContext::native(
        antiburn_local::model::AgentKind::OpenCode,
        &home,
        Some(workspace),
    );
    let snapshot = fixture
        .store
        .load_smart_check_inputs(
            &candidate.session.key,
            candidate.published_fence,
            candidate.source_generation,
            crate::smart_check_inputs::DetectorInput::SkillOpportunities,
        )
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let skills = fixture
        .store
        .load_smart_check_skill_inputs(snapshot, &config)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let input = crate::skill_opportunities_worker::prepare(
        &candidate,
        skills,
        crate::skill_opportunities_worker::CHECK.evaluator_revision(),
    )
    .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    Ok((fixture, input))
}

fn skill_case(
    home: &std::path::Path,
    name: &str,
    model: String,
    expected_choice: &'static str,
    expected_finding: bool,
) -> anyhow::Result<AdapterCase> {
    let (fixture, input) = benchmark_skill_fixture()?;
    let fixture_skill = fixture
        .directory
        .path()
        .join("home/.opencode/skills/review-0/SKILL.md");
    let shared_skill = home.join(".opencode/skills/review-0/SKILL.md");
    let shared_skill_directory = shared_skill
        .parent()
        .ok_or_else(|| anyhow::anyhow!("shared skill path has no parent"))?;
    std::fs::create_dir_all(shared_skill_directory)?;
    std::fs::copy(fixture_skill, shared_skill)?;
    let candidate = fixture
        .store
        .burn_check_candidates_for_revision(
            crate::skill_opportunities_worker::CHECK.id(),
            &crate::skill_opportunities_worker::CHECK.evaluator_revision(),
            unix_now(),
            crate::skill_opportunities_worker::CHECK.policy().idle_secs,
            8,
        )?
        .into_iter()
        .find(|candidate| candidate.session.key == input.durable.key)
        .ok_or_else(|| anyhow::anyhow!("case={name} Skills candidate was not admitted"))?;
    Ok(AdapterCase {
        name: name.to_owned(),
        model,
        check: &crate::skill_opportunities_worker::CHECK,
        store: fixture.store.clone(),
        candidate,
        expected_choice,
        expected_finding,
        instruction_format: None,
        instruction_content: None,
        skill_input: Some(input),
        _keep_alive: FixtureKeepAlive {
            _directories: Vec::new(),
            _native_fixture: Some(fixture),
        },
    })
}

fn create_adapter_cases(home: &std::path::Path, mode: &str) -> anyhow::Result<Vec<AdapterCase>> {
    let model = |name: &str| format!("antiburn-benchmark-{name}-{mode}");
    Ok(vec![
        instruction_case(
            "ignored_instructions_positive",
            model("ignored-instructions-positive"),
            "Do not run Bash commands.\n",
            "conflict",
            true,
        )?,
        instruction_case(
            "ignored_instructions_hard_negative",
            model("ignored-instructions-hard-negative"),
            "Use Bash to run `cargo test --lib` when verifying the parser.\n",
            "no_conflict",
            false,
        )?,
        scope_case(
            "scope_creep_positive",
            model("scope-creep-positive"),
            false,
            "unauthorized_extra",
            true,
        )?,
        scope_case(
            "scope_creep_hard_negative",
            model("scope-creep-hard-negative"),
            true,
            "in_scope",
            false,
        )?,
        over_exploring_case(
            "over_exploring_positive",
            model("over-exploring-positive"),
            true,
        )?,
        over_exploring_case(
            "over_exploring_hard_negative",
            model("over-exploring-hard-negative"),
            false,
        )?,
        skill_case(
            home,
            "skill_opportunities_positive",
            model("skill-opportunities-positive"),
            "actionable_help",
            true,
        )?,
        skill_case(
            home,
            "skill_opportunities_hard_negative",
            model("skill-opportunities-hard-negative"),
            "already_adequate_or_unrelated",
            false,
        )?,
    ])
}

async fn instructions_preflight(
    store: &Store,
    candidate: &BurnCheckCandidate,
    home: &std::path::Path,
    format: SourceFormat,
    capabilities: &ModelCapabilities,
) -> anyhow::Result<SessionContentEvidence> {
    anyhow::ensure!(
        candidate.historical
            && candidate.boundary_at_epoch == 0
            && candidate.boundary_positions.is_empty(),
        "Instructions fixture must use the characterized historical baseline"
    );
    let cwd = candidate
        .session
        .cwd
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Instructions candidate has no workspace"))?;
    let cwd = std::path::Path::new(cwd);
    let root = antiburn_local::platform::git::repo_root_at(cwd)
        .await
        .unwrap_or_else(|_| cwd.to_path_buf());
    let agent = crate::agents::kind_from_slug(&candidate.session.key.agent)
        .map(crate::agents::vendor_label)
        .unwrap_or(candidate.session.key.agent.as_str());
    let discovery = antiburn_local::analysis::ignored_instructions::discover_current_instructions(
        agent, format, &root, cwd, home,
    )
    .await;
    anyhow::ensure!(
        !discovery.snapshots.is_empty(),
        "Instructions fixture has no discovered current rule: {:?}",
        discovery.limitations
    );
    let boundary_positions = candidate.boundary_positions.clone();
    let historical_boundary_ms = candidate.boundary_at_epoch.saturating_mul(1000);
    let after_ms = Some(historical_boundary_ms);
    let page = store
        .published_turn_content_keyset_selected(
            &candidate.session.key,
            SelectedContentRequest {
                source_generation: candidate.source_generation,
                after_ms,
                source_positions: &boundary_positions,
                selection: antiburn_local::analysis::ignored_instructions::IgnoredInstructionsCheck
                    .input_selection(),
                cursor: None,
            },
        )?
        .ok_or_else(|| anyhow::anyhow!("Instructions fixture has no published content"))?;
    anyhow::ensure!(
        page.next_cursor.is_none() && !page.content.coverage.more_parts,
        "Instructions fixture must fit in one indexed page"
    );
    let mut content = antiburn_local::analysis::ignored_instructions::prepare_session_content(
        &format!(
            "{}\0{}\0{}",
            candidate.session.key.environment_key,
            candidate.session.key.agent,
            candidate.session.key.session_id
        ),
        format,
        page.content,
        discovery.snapshots,
    );
    content.limitations.extend(discovery.limitations);
    if !discovery.scan_complete {
        content
            .limitations
            .push("instruction_scan_incomplete".to_owned());
    }
    content.limitations.sort();
    content.limitations.dedup();
    content.complete &= content.limitations.is_empty();
    content = antiburn_local::analysis::ignored_instructions::select_session_content(
        &content,
        antiburn_local::analysis::ignored_instructions::IgnoredInstructionsCheck.input_selection(),
    );
    let bash_action = content
        .actions
        .iter()
        .find(|action| {
            action.authority == "assistant"
                && action.kind == "tool_input"
                && !action.context_only
                && !action.truncated
                && action.text.len() <= 512
                && action
                    .tool_name
                    .as_deref()
                    .is_some_and(|tool| tool.eq_ignore_ascii_case("bash"))
                && action.tool_call_id.is_some()
                && action.reference.stable
                && action
                    .normalized_fields
                    .as_ref()
                    .and_then(|fields| {
                        fields
                            .values
                            .get(&antiburn_local::analysis::jev::JevInputField::BashCommandInput)
                    })
                    .is_some_and(|command| command.contains("cargo test --lib"))
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Instructions fixture has no stable, bound Bash input for `cargo test --lib`"
            )
        })?;
    let action_timestamp_ms = bash_action
        .timestamp_ms
        .ok_or_else(|| anyhow::anyhow!("Bash action has no indexed timestamp"))?;
    anyhow::ensure!(
        action_timestamp_ms > historical_boundary_ms,
        "Bash action is not after the historical fixture boundary: action_turn={} action_timestamp_ms={action_timestamp_ms} boundary_ms={}",
        bash_action.reference.turn_index,
        historical_boundary_ms
    );
    let prior_history_complete = !content.limitations.iter().any(|limitation| {
        matches!(
            limitation.as_str(),
            "branch_context_limit"
                | "content_part_limit"
                | "content_byte_limit"
                | "oversized_content_part"
                | "truncated_source_content"
        )
    });
    let digest_material = format!(
        "{}\0{}",
        content.selected_input_digest,
        content.limitations.join("\0")
    );
    content.selected_input_digest =
        antiburn_local::analysis::ignored_instructions::sha256_hex(digest_material.as_bytes());
    let page_actions = content.actions.clone();
    let assessment_input = antiburn_local::analysis::ignored_instructions::AssessmentInput {
        content: content.clone(),
        prior_history_complete,
        activity_after_ms: after_ms,
        boundary_positions,
        source_generation: candidate.source_generation,
        source_fingerprint: candidate.source_fingerprint.clone(),
        incarnation: candidate.incarnation,
        comparison_after: None,
    };
    let mut context =
        antiburn_local::analysis::ignored_instructions::build_jev_context_with_capabilities(
            &assessment_input,
            &antiburn_local::analysis::ignored_instructions::SamplingLedger::default(),
            capabilities,
        )?;
    let mut carried = Vec::new();
    antiburn_local::analysis::ignored_instructions::extend_jev_context_with_history_and_capabilities(
        &mut context,
        &mut carried,
        &page_actions,
        prior_history_complete,
        capabilities,
    )?;
    let plan = antiburn_local::analysis::ignored_instructions::IgnoredInstructionsCheck
        .prepare_with_capabilities(&context, capabilities)?;
    anyhow::ensure!(
        plan.prepared.coverage.candidate_pairs > 0
            && plan.prepared.coverage.selected_comparisons > 0
            && !plan.work_items.is_empty(),
        "Instructions fixture has no prepared rule/action candidates: {:?}",
        plan.prepared.coverage
    );
    Ok(content)
}

async fn adapter_preflight(
    case: &mut AdapterCase,
    home: &std::path::Path,
    capabilities: &ModelCapabilities,
) -> anyhow::Result<()> {
    match case.check.id() {
        "ignored_instructions" => {
            let format = case.instruction_format.ok_or_else(|| {
                anyhow::anyhow!("case={} has no Instructions source format", case.name)
            })?;
            case.instruction_content = Some(
                instructions_preflight(&case.store, &case.candidate, home, format, capabilities)
                    .await?,
            );
        }
        "scope_creep" => {
            let input =
                crate::scope_creep_worker::load_input(&case.store, &case.candidate, capabilities)
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?;
            anyhow::ensure!(
                input.plan.work_items.len() == 1
                    && input.plan.coverage.not_selected_items == 0
                    && !input.plan.coverage.processing_limit_reached
                    && !input
                        .plan
                        .coverage
                        .limitations
                        .iter()
                        .any(|limitation| limitation == "candidate_limit_reached"),
                "case={} scope preflight work_items={} coverage={:?}",
                case.name,
                input.plan.work_items.len(),
                input.plan.coverage
            );
        }
        "over_exploring" => {
            let input = load_input(&case.store, &case.candidate, capabilities)
                .map_err(|error| anyhow::anyhow!("{error:?}"))?;
            anyhow::ensure!(
                !input.plan.work_items.is_empty(),
                "case={} has no prepared read candidates",
                case.name
            );
            let fields = &input
                .plan
                .shared_context
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("case={} has no shared task context", case.name))?
                .fields;
            anyhow::ensure!(
                fields.to_string().contains("Reuse matching recorded reads")
                    && input.plan.prepared.targets.values().any(|target| {
                        target.reason
                            == antiburn_local::checks::over_exploring::Reason::ExcessiveWithinFileReading
                    }),
                "case={} lacks the bound parser task or repeated-read target",
                case.name
            );
            let snapshot = case
                .store
                .load_smart_check_inputs(
                    &case.candidate.session.key,
                    case.candidate.published_fence,
                    case.candidate.source_generation,
                    crate::smart_check_inputs::DetectorInput::OverExploring,
                )
                .map_err(|error| anyhow::anyhow!("{error:?}"))?;
            let parser_reads = snapshot
                .content()
                .actions
                .iter()
                .filter_map(|action| action.metadata.read_request.as_ref())
                .filter(|read| read.paths.iter().any(|path| path == "/synthetic/parser.rs"))
                .count();
            let expected_reads = if case.expected_finding { 2 } else { 3 };
            anyhow::ensure!(
                parser_reads == expected_reads,
                "case={} indexed parser read count={} expected={expected_reads}",
                case.name,
                parser_reads
            );
        }
        "skill_opportunities" => {
            let input = case.skill_input.as_ref().ok_or_else(|| {
                anyhow::anyhow!("case={} has no prepared Skills input", case.name)
            })?;
            let plan = input
                .check
                .prepare_with_capabilities(&input.check.session_context(), capabilities)?;
            anyhow::ensure!(
                !plan.work_items.is_empty(),
                "case={} has no prepared skill candidates",
                case.name
            );
        }
        check_id => anyhow::bail!("unsupported adapter case: {check_id}"),
    }
    Ok(())
}

fn persisted_findings<'a>(
    case: &AdapterCase,
    publication: &'a serde_json::Value,
) -> anyhow::Result<&'a [serde_json::Value]> {
    let path = match case.check.id() {
        "ignored_instructions" | "over_exploring" | "skill_opportunities" => "/findings",
        "scope_creep" => "/assessment/findings",
        check_id => anyhow::bail!("unsupported persisted check result: {check_id}"),
    };
    publication
        .pointer(path)
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| anyhow::anyhow!("case={} has no {path} array", case.name))
}

fn assert_adapter_publication(
    case: &AdapterCase,
    saved: &crate::store::BurnCheckAssessment,
) -> anyhow::Result<usize> {
    let error_category: Option<String> = case.store.lock().query_row(
        "SELECT last_error_category FROM burn_check_assessment WHERE check_id = ?1 AND session_id = ?2",
        rusqlite::params![case.check.id(), case.candidate.session.key.session_id],
        |row| row.get(0),
    )?;
    anyhow::ensure!(
        saved.status == "completed",
        "case={} status={} category={error_category:?} result={:?}",
        case.name,
        saved.status,
        saved.result_json
    );
    anyhow::ensure!(
        saved.request_count == 1,
        "case={} logical request count={}",
        case.name,
        saved.request_count
    );
    let value: serde_json::Value = serde_json::from_str(
        saved
            .result_json
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("case={} persisted no result", case.name))?,
    )?;
    let findings = persisted_findings(case, &value)?;
    anyhow::ensure!(
        !findings.is_empty() == case.expected_finding,
        "case={} expected_finding={} actual_findings={}",
        case.name,
        case.expected_finding,
        findings.len()
    );
    let findings_count = findings.len();
    match case.check.id() {
        "ignored_instructions" => {
            let result: antiburn_local::analysis::ignored_instructions::AssessmentResult =
                serde_json::from_value(value)?;
            anyhow::ensure!(
                result.coverage.selected_comparisons > 0
                    && result.unassessed_comparisons.is_empty(),
                "case={} Instructions result was not fully assessed: {:?}",
                case.name,
                result.coverage
            );
            if case.expected_finding {
                let content = case
                    .instruction_content
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("case={} lost source content", case.name))?;
                for finding in &result.findings {
                    anyhow::ensure!(
                        finding.decision_record().is_some(),
                        "case={} finding has no source-bound decision proof",
                        case.name
                    );
                    anyhow::ensure!(
                        matches!(
                            antiburn_local::analysis::ignored_instructions::resolve_content_reference(
                                content,
                                case.candidate.published_fence,
                                &content.selected_input_digest,
                                &finding.reference.action_id,
                            ),
                            antiburn_local::analysis::ignored_instructions::ContentReferenceResolution::Found(_)
                        ),
                        "case={} finding action is not bound to the indexed source",
                        case.name
                    );
                }
            }
        }
        "scope_creep" => {
            let publication: crate::scope_creep_worker::Publication =
                serde_json::from_value(value)?;
            let expected_findings = if case.expected_finding { 1 } else { 0 };
            anyhow::ensure!(
                crate::scope_creep_worker::publication_has_clean_coverage(&publication.assessment)
                    && publication.assessment.assessed_candidates == 1
                    && publication.assessment.decisions.len() == 1
                    && publication.assessment.findings.len() == expected_findings,
                "case={} scope result is incomplete: {:?}",
                case.name,
                publication.assessment.coverage
            );
            if case.expected_finding {
                anyhow::ensure!(
                    publication.assessment.decisions[0].status
                        == antiburn_local::checks::scope_creep::ScopeCreepStatus::Finding,
                    "case={} scope positive did not persist a Finding decision",
                    case.name
                );
                for finding in &publication.assessment.findings {
                    anyhow::ensure!(
                        crate::scope_creep_worker::publishable_finding(finding, &publication),
                        "case={} scope finding lost its source binding",
                        case.name
                    );
                }
            } else {
                anyhow::ensure!(
                    publication.assessment.decisions[0].status
                        == antiburn_local::checks::scope_creep::ScopeCreepStatus::Clean,
                    "case={} scope negative did not persist a Clean decision",
                    case.name
                );
            }
        }
        "over_exploring" => {
            let publication: Publication = serde_json::from_value(value)?;
            if case.expected_finding {
                for finding in &publication.assessment.findings {
                    anyhow::ensure!(
                        publishable_finding(finding, &publication),
                        "case={} read finding lost its source binding",
                        case.name
                    );
                }
            } else {
                let completed_episodes = publication
                    .assessment
                    .completed_episode_ids
                    .iter()
                    .collect::<BTreeSet<_>>();
                let clean_episodes = publication
                    .assessment
                    .clean_episode_ids
                    .iter()
                    .collect::<BTreeSet<_>>();
                anyhow::ensure!(
                    publication_has_clean_coverage(&publication.assessment)
                        && !publication.assessment.completed_work_item_ids.is_empty()
                        && !clean_episodes.is_empty()
                        && completed_episodes == clean_episodes,
                    "case={} read negative was not fully assessed: completed={completed_episodes:?} clean={clean_episodes:?} work_items={:?}",
                    case.name,
                    publication.assessment.completed_work_item_ids
                );
            }
        }
        "skill_opportunities" => {
            let result: antiburn_local::checks::skill_opportunities::SkillOpportunitiesResult =
                serde_json::from_value(value)?;
            anyhow::ensure!(
                crate::skill_opportunities_worker::publication_has_assessed_coverage(&result)
                    && !result.decisions.is_empty(),
                "case={} Skills result was not fully assessed",
                case.name
            );
            if case.expected_finding {
                for finding in &result.findings {
                    anyhow::ensure!(
                        crate::skill_opportunities_worker::publishable_finding(finding),
                        "case={} skill finding lost its source binding",
                        case.name
                    );
                }
            }
        }
        _ => unreachable!("adapter check was matched above"),
    }
    Ok(findings_count)
}

async fn run_scope_candidate_through_continuations(
    case: &AdapterCase,
    handle: &crate::jev::worker::WorkerHandle,
    effects: &crate::jev::worker::TestWorkerEffects,
    events: &crate::session_lifecycle::SessionEvents,
) -> anyhow::Result<crate::store::BurnCheckAssessment> {
    for turn in 0..crate::jev::worker::CANDIDATES_PER_WAKE {
        handle.start_candidate_turn(case.check.id(), &case.candidate.session.key);
        let outcome = case
            .check
            .run_candidate(crate::jev::worker::CandidateExecution {
                effects,
                store: &case.store,
                candidate: &case.candidate,
                client: crate::jev::client::TypeSafeClient::new("unused".into())?,
                handle,
                key_generation: 1,
                events,
            })
            .await;
        handle.end_candidate_turn(case.check.id(), &case.candidate.session.key);
        outcome?;
        let saved = case
            .store
            .burn_check_assessment(&case.candidate.session.key, case.check.id())?
            .ok_or_else(|| anyhow::anyhow!("case={} persisted no assessment", case.name))?;
        if saved.status == "completed" {
            return Ok(saved);
        }
        let error_category: Option<String> = case.store.lock().query_row(
            "SELECT last_error_category FROM burn_check_assessment WHERE check_id = ?1 AND session_id = ?2",
            rusqlite::params![case.check.id(), case.candidate.session.key.session_id],
            |row| row.get(0),
        )?;
        if error_category.as_deref() != Some("continuing") {
            return Ok(saved);
        }
        anyhow::ensure!(
            turn + 1 < crate::jev::worker::CANDIDATES_PER_WAKE,
            "case={} requested more than {} Store-backed Scope turns",
            case.name,
            crate::jev::worker::CANDIDATES_PER_WAKE
        );
        let input = crate::store::BurnCheckInput {
            key: case.candidate.session.key.clone(),
            check_id: case.check.id().to_owned(),
            incarnation: case.candidate.incarnation,
            source_generation: case.candidate.source_generation,
            source_fingerprint: case.candidate.source_fingerprint.clone(),
            activity_cursor: case.candidate.activity_cursor.clone(),
            published_fence: case.candidate.published_fence,
            input_revision: saved.input_revision.clone().ok_or_else(|| {
                anyhow::anyhow!("case={} continuation has no input revision", case.name)
            })?,
            evaluator_revision: case.check.evaluator_revision(),
            boundary_at_epoch: case.candidate.boundary_at_epoch,
        };
        let next_attempt = case
            .store
            .burn_check_next_attempt_at(&input)?
            .ok_or_else(|| {
                anyhow::anyhow!("case={} continuation has no scheduled retry", case.name)
            })?;
        let delay_seconds = next_attempt.saturating_sub(unix_now()).max(0) as u64;
        if delay_seconds > 0 {
            tokio::time::sleep(Duration::from_secs(delay_seconds)).await;
        }
    }
    anyhow::bail!(
        "case={} exhausted the bounded Scope continuation loop",
        case.name
    )
}

fn usage_cache_hits(cases: &[AdapterCase]) -> anyhow::Result<u64> {
    cases.iter().try_fold(0_u64, |total, case| {
        Ok(total + case.store.burn_check_usage_summary()?.cache_hits)
    })
}

async fn run_native_worker_adapters(constrained: bool) -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    let home_path = home.path().to_owned();
    let mode = if constrained {
        "constrained"
    } else {
        "default"
    };
    let mut cases =
        tokio::task::spawn_blocking(move || create_adapter_cases(&home_path, mode)).await??;
    let provider = LoopbackProvider::start().await?;
    let prepare_result = async {
        let mut rendered_fit_limit = None;
        for case in &mut cases {
            let (_, capabilities) =
                capabilities_for_model(&provider.endpoint, &case.model, constrained)?;
            let input_limit = capabilities
                .usable_input_tokens()
                .ok_or_else(|| anyhow::anyhow!("case={} has no rendered fit limit", case.name))?;
            rendered_fit_limit = Some(
                rendered_fit_limit.map_or(input_limit, |current: u64| current.min(input_limit)),
            );
            provider.register_response(
                case.model.clone(),
                response_profile(case.check.id(), case.expected_choice, capabilities.clone())?,
            )?;
            adapter_preflight(case, home.path(), &capabilities).await?;
        }
        let effects = crate::jev::worker::TestWorkerEffects::default();
        let events = crate::session_lifecycle::SessionEvents::default();
        // The timer starts after native publication and input preflight.
        let started = Instant::now();
        let mut first_finding_ms = None;
        let mut logical_requests = 0_usize;
        let mut findings_total = 0_usize;
        let mut case_outcomes = Vec::new();
        for case in &cases {
            let (connection, _) =
                capabilities_for_model(&provider.endpoint, &case.model, constrained)?;
            let handle = crate::jev::worker::WorkerHandle::default();
            handle
                .set_system_one_connection(connection, None)
                .map_err(|error| anyhow::anyhow!("invalid benchmark connection: {error:?}"))?;
            let saved = if case.check.id() == "scope_creep" {
                run_scope_candidate_through_continuations(case, &handle, &effects, &events).await?
            } else {
                let home_override = matches!(
                    case.check.id(),
                    "ignored_instructions" | "skill_opportunities"
                )
                .then(|| HomeDirectory::set(home.path().to_owned()));
                handle.start_candidate_turn(case.check.id(), &case.candidate.session.key);
                let outcome = case
                    .check
                    .run_candidate(crate::jev::worker::CandidateExecution {
                        effects: &effects,
                        store: &case.store,
                        candidate: &case.candidate,
                        client: crate::jev::client::TypeSafeClient::new("unused".into())?,
                        handle: &handle,
                        key_generation: 1,
                        events: &events,
                    })
                    .await;
                handle.end_candidate_turn(case.check.id(), &case.candidate.session.key);
                drop(home_override);
                outcome?;
                case.store
                    .burn_check_assessment(&case.candidate.session.key, case.check.id())?
                    .ok_or_else(|| anyhow::anyhow!("case={} persisted no assessment", case.name))?
            };
            let findings = assert_adapter_publication(case, &saved)?;
            if findings > 0 && first_finding_ms.is_none() {
                first_finding_ms = Some(started.elapsed().as_millis());
            }
            findings_total += findings;
            logical_requests += saved.request_count;
            case_outcomes.push(format!(
                "{}={}",
                case.name,
                if findings == 0 { "clean" } else { "finding" }
            ));
        }
        anyhow::ensure!(
            cases.len() == 8
                && cases.iter().filter(|case| case.expected_finding).count() == 4
                && cases.iter().filter(|case| !case.expected_finding).count() == 4,
            "adapter workload did not contain four positive and four hard-negative cases"
        );
        anyhow::ensure!(findings_total > 0, "adapter workload persisted no finding");
        Ok::<_, anyhow::Error>((
            started.elapsed().as_millis(),
            first_finding_ms,
            logical_requests,
            findings_total,
            case_outcomes.join(","),
            rendered_fit_limit.unwrap_or_default(),
        ))
    }
    .await;
    let metrics = provider.stop().await?;
    let (completion_ms, first_finding_ms, logical_requests, findings, outcomes, rendered_fit_limit) =
        prepare_result?;
    anyhow::ensure!(
        metrics.requests.load(Ordering::Relaxed) == logical_requests,
        "adapter workload HTTP sends={} logical requests={logical_requests}",
        metrics.requests.load(Ordering::Relaxed)
    );
    let cache_hits = usage_cache_hits(&cases)?;
    println!(
        "workload=adapter_cases mode={mode} status=PASS cases={} outcomes={outcomes} first_persisted_finding_ms={} completion_ms={completion_ms} logical_requests={logical_requests} http_sends={} cache_hits={cache_hits} request_bytes={} response_bytes={} estimated_tokens={} max_rendered_tokens={} rendered_fit_limit_tokens={rendered_fit_limit} synthetic_input_tokens={} synthetic_output_tokens={} peak_transport={} findings={findings}",
        cases.len(),
        first_finding_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".into()),
        metrics.requests.load(Ordering::Relaxed),
        metrics.request_bytes.load(Ordering::Relaxed),
        metrics.response_bytes.load(Ordering::Relaxed),
        metrics.estimated_tokens.load(Ordering::Relaxed),
        metrics.max_rendered_tokens.load(Ordering::Relaxed),
        metrics.synthetic_input_tokens.load(Ordering::Relaxed),
        metrics.synthetic_output_tokens.load(Ordering::Relaxed),
        metrics.peak.load(Ordering::Relaxed),
    );
    Ok(())
}

async fn run_long_read_session() -> anyhow::Result<()> {
    let (_store_directory, store, candidate) =
        tokio::task::spawn_blocking(|| -> anyhow::Result<_> {
            let directory = tempfile::tempdir()?;
            let parser_read_content = "recorded parser output 123\n".repeat(320);
            let garden_read_content =
                "Garden notes\nPlant basil in bed one. Water after planting.\n";
            anyhow::ensure!(parser_read_content.len() >= 8 * 1024);
            anyhow::ensure!(garden_read_content.len() < 8 * 1024);
            let (store, candidate) = fixture_in_store_with_long_read_context(
                Store::open(directory.path())?,
                "long-indexed-read",
                &parser_read_content,
                garden_read_content,
            );
            Ok((directory, store, candidate))
        })
        .await??;
    let provider = LoopbackProvider::start().await?;
    let workload = async {
        let model = "antiburn-benchmark-over-exploring-long-positive";
        let (connection, capabilities) = capabilities_for_model(&provider.endpoint, model, false)?;
        provider.register_response(
            model,
            response_profile(CHECK_ID, "likely_excess", capabilities.clone())?,
        )?;
        let snapshot = store
            .load_smart_check_inputs(
                &candidate.session.key,
                candidate.published_fence,
                candidate.source_generation,
                crate::smart_check_inputs::DetectorInput::OverExploring,
            )
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        let actions = &snapshot.content().actions;
        let parser_request_actions = actions
            .iter()
            .filter(|action| {
                action.metadata.read_request.as_ref().is_some_and(|read| {
                    read.paths.iter().any(|path| path == "/synthetic/parser.rs")
                })
            })
            .collect::<Vec<_>>();
        let parser_reads = parser_request_actions.len();
        let garden_request_action = actions
            .iter()
            .find(|action| {
                action.metadata.read_request.as_ref().is_some_and(|read| {
                    read.paths
                        .iter()
                        .any(|path| path == "/synthetic/garden/README.md")
                })
            })
            .ok_or_else(|| anyhow::anyhow!("long read fixture has no garden request"))?;
        let garden_request = garden_request_action
            .metadata
            .read_request
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("garden action has no request metadata"))?;
        let garden_result_action = actions
            .iter()
            .find(|action| {
                action.metadata.read_result.as_ref().is_some_and(|result| {
                    result.request_reference_id.as_deref()
                        == Some(garden_request.reference_id.as_str())
                })
            })
            .ok_or_else(|| anyhow::anyhow!("garden request has no indexed result"))?;
        let garden_result = garden_result_action
            .metadata
            .read_result
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("garden result action has no read result"))?;
        let garden_purpose = actions
            .iter()
            .find(|action| {
                action.turn_role == "assistant"
                    && action.text.contains("different gardening task")
                    && action.text.contains("separate garden README")
            })
            .ok_or_else(|| anyhow::anyhow!("garden read has no indexed assistant purpose"))?;
        anyhow::ensure!(
            garden_purpose.reference.stable
                && !garden_purpose.context_only
                && !garden_purpose.truncated
                && garden_request_action.reference.stable
                && !garden_request_action.context_only
                && !garden_request_action.truncated
                && garden_request.reference_id == garden_request_action.reference.id
                && !garden_request.truncated
                && garden_result_action.reference.stable
                && garden_result_action.reference.id == garden_result.reference_id
                && garden_result.request_reference_id.as_deref()
                    == Some(garden_request.reference_id.as_str())
                && garden_result.status
                    == antiburn_local::analysis::jev_evidence::JevReadStatus::Success
                && garden_result.kind
                    == antiburn_local::analysis::jev_evidence::JevReadResultKind::File
                && garden_result.recorded_output_bytes < 8 * 1024
                && garden_purpose.reference.turn_index < garden_request_action.reference.turn_index,
            "garden detour purpose or request/result source binding is invalid"
        );
        for action in &parser_request_actions {
            let request = action
                .metadata
                .read_request
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("parser action has no read request"))?;
            let result_action = actions
                .iter()
                .find(|candidate| {
                    candidate.metadata.read_result.as_ref().is_some_and(|result| {
                        result.request_reference_id.as_deref()
                            == Some(request.reference_id.as_str())
                    })
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "parser request {} has no indexed result action",
                        request.reference_id
                    )
                })?;
            let result = result_action.metadata.read_result.as_ref().ok_or_else(|| {
                anyhow::anyhow!("parser result action has no recorded result")
            })?;
            anyhow::ensure!(
                action.reference.stable
                    && request.reference_id == action.reference.id
                    && !action.context_only
                    && request.extent.unit
                        == antiburn_local::analysis::jev_evidence::JevReadUnit::Lines
                    && request.extent.offset == Some(1)
                    && !request.truncated
                    && !action.truncated
                    && result_action.reference.stable
                    && result_action.reference.id == result.reference_id
                    && !result_action.truncated
                    && result.status == antiburn_local::analysis::jev_evidence::JevReadStatus::Success
                    && result.kind == antiburn_local::analysis::jev_evidence::JevReadResultKind::File
                    && result.recorded_output_bytes >= 8 * 1024
                    && !result.truncated,
                "parser read {} lacks a complete >=8 KiB source result",
                request.reference_id
            );
        }
        let input = load_input(&store, &candidate, &capabilities)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        anyhow::ensure!(!input.plan.work_items.is_empty(), "long read input has no selected work");
        anyhow::ensure!(parser_reads == 4, "indexed read fixture parser reads={parser_reads}");
        for item in &input.plan.work_items {
            let target = input
                .plan
                .prepared
                .targets
                .get(&item.id)
                .ok_or_else(|| anyhow::anyhow!("selected parser work item has no prepared target"))?;
            anyhow::ensure!(
                target.reason
                    == antiburn_local::checks::over_exploring::Reason::ExcessiveWithinFileReading,
                "selected long-read target has unexpected reason: {:?}",
                target.reason
            );
            anyhow::ensure!(
                !target.read_indexes.is_empty(),
                "selected repeated-read target has no read indexes"
            );
            let episode = input
                .plan
                .prepared
                .episodes
                .iter()
                .find(|episode| episode.id == target.episode_id)
                .ok_or_else(|| anyhow::anyhow!("selected parser target has no episode"))?;
            let path = item
                .window
                .fields
                .pointer("/repeat/path")
                .and_then(serde_json::Value::as_str);
            anyhow::ensure!(
                path == Some("/synthetic/parser.rs"),
                "selected repeated-read target does not name the parser path: {path:?}"
            );
            for read_index in &target.read_indexes {
                let read = episode
                    .reads
                    .get(*read_index)
                    .ok_or_else(|| anyhow::anyhow!("selected parser target has invalid read index"))?;
                let request_part = format!("reads[{read_index}].request");
                let result_part = format!("reads[{read_index}].result");
                let result = read
                    .result
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("selected parser read has no result"))?;
                anyhow::ensure!(
                    item.window.evidence.iter().any(|evidence| {
                        evidence.part_id == request_part
                            && evidence.source_id == read.request.reference_id
                    }) && item.window.evidence.iter().any(|evidence| {
                        evidence.part_id == result_part
                            && evidence.source_id == result.reference_id
                    }),
                    "selected parser work item lost exact request/result binding for read index {read_index}"
                );
            }
            let first_result = episode.reads[target.read_indexes[0]]
                .result
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("selected parser target has no first result"))?;
            anyhow::ensure!(
                item.window.evidence.iter().any(|evidence| {
                    evidence.part_id == "repeat.header_sample.text"
                        && evidence.source_id == first_result.reference_id
                }),
                "selected parser work item lost its bound leading sample"
            );
        }
        anyhow::ensure!(
            input.plan.work_items.iter().any(|item| {
                input.plan.prepared.targets.get(&item.id).is_some_and(|target| {
                    target.reason
                        == antiburn_local::checks::over_exploring::Reason::ExcessiveWithinFileReading
                })
            }),
            "long-read selection contains no supported repeated parser-read target"
        );
        let task_fields = input
            .plan
            .shared_context
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("long read input has no shared task context"))?
            .fields
            .to_string();
        anyhow::ensure!(
            task_fields.contains("Fix the parser")
                && task_fields.contains("Reuse matching recorded reads"),
            "long read input lost its parser task"
        );
        anyhow::ensure!(
            input.plan.coverage.not_selected_items == 0
                && !input.plan.coverage.processing_limit_reached
                && input.plan.prepared.candidates.len() == 1
                && input.plan.work_items.len() <= 3
                && input.plan.prepared.unassessed.is_empty(),
            "long read preflight has unexpected coverage or omissions: coverage={:?} unassessed={:?}",
            input.plan.coverage,
            input.plan.prepared.unassessed,
        );
        let repeat_targets = input
            .plan
            .prepared
            .targets
            .values()
            .filter(|target| {
                target.reason
                    == antiburn_local::checks::over_exploring::Reason::ExcessiveWithinFileReading
            })
            .count();
        anyhow::ensure!(
            repeat_targets > 0 && repeat_targets <= 2,
            "long read selection exceeded its bounds: repeats={repeat_targets}"
        );
        let handle = crate::jev::worker::WorkerHandle::default();
        handle
            .set_system_one_connection(connection, None)
            .map_err(|error| anyhow::anyhow!("invalid benchmark connection: {error:?}"))?;
        let effects = crate::jev::worker::TestWorkerEffects::default();
        let events = crate::session_lifecycle::SessionEvents::default();
        let started = Instant::now();
        handle.start_candidate_turn(CHECK_ID, &candidate.session.key);
        let outcome = CHECK
            .run_candidate(crate::jev::worker::CandidateExecution {
                effects: &effects,
                store: &store,
                candidate: &candidate,
                client: crate::jev::client::TypeSafeClient::new("unused".into())?,
                handle: &handle,
                key_generation: 1,
                events: &events,
            })
            .await;
        handle.end_candidate_turn(CHECK_ID, &candidate.session.key);
        outcome?;
        let saved = store
            .burn_check_assessment(&candidate.session.key, CHECK_ID)?
            .ok_or_else(|| anyhow::anyhow!("long read assessment was not persisted"))?;
        let failure_category: Option<String> = store.lock().query_row(
            "SELECT last_error_category FROM burn_check_assessment
             WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = ?4",
            rusqlite::params![
                candidate.session.key.environment_key,
                candidate.session.key.agent,
                candidate.session.key.session_id,
                CHECK_ID,
            ],
            |row| row.get(0),
        )?;
        anyhow::ensure!(
            saved.status == "completed",
            "long read status={} failure_category={failure_category:?} request_count={} progress={} result={}",
            saved.status,
            saved.request_count,
            saved.progress_json,
            saved.result_json.as_deref().unwrap_or("<none>"),
        );
        anyhow::ensure!(saved.request_count == 1, "long read logical requests={}", saved.request_count);
        let publication: Publication =
            serde_json::from_str(saved.result_json.as_deref().ok_or_else(|| {
                anyhow::anyhow!("long read assessment has no result")
            })?)?;
        anyhow::ensure!(
            !publication.assessment.findings.is_empty(),
            "long read fixture produced no supported finding"
        );
        for finding in &publication.assessment.findings {
            anyhow::ensure!(
                publishable_finding(finding, &publication),
                "long read finding lost its source binding"
            );
        }
        anyhow::ensure!(
            publication.assessment.unassessed.is_empty()
                && publication.assessment.unassessed == input.plan.prepared.unassessed
                && publication.assessment.coverage.not_selected_items == 0
                && !publication.assessment.coverage.processing_limit_reached
                && publication.assessment.completed_work_item_ids.len()
                    == publication.assessment.coverage.selected_items,
            "long read result changed selected or unassessed coverage: coverage={:?} unassessed={:?} prepared_unassessed={:?}",
            publication.assessment.coverage,
            publication.assessment.unassessed,
            input.plan.prepared.unassessed,
        );
        let finding_count = publication.assessment.findings.len();
        let request_count = saved.request_count;
        let cache_hits = store.burn_check_usage_summary()?.cache_hits;
        let elapsed_ms = started.elapsed().as_millis();
        let unassessed_count = publication.assessment.unassessed.len();
        Ok::<_, anyhow::Error>((
            parser_reads,
            parser_reads - 1,
            repeat_targets,
            unassessed_count,
            finding_count,
            request_count,
            cache_hits,
            elapsed_ms,
        ))
    }
    .await;
    let metrics = provider.stop().await?;
    let (
        parser_reads,
        repeated_parser_reads,
        repeat_targets,
        unassessed_count,
        finding_count,
        request_count,
        cache_hits,
        elapsed_ms,
    ) = workload?;
    anyhow::ensure!(metrics.requests.load(Ordering::Relaxed) == request_count);
    println!(
        "workload=long_indexed_read_session status=PASS case_outcome=over_exploring:finding parser_reads={parser_reads} necessary_parser_reads=1 repeated_parser_reads={repeated_parser_reads} selected_repeat_targets={repeat_targets} explicitly_unassessed={unassessed_count} first_persisted_finding_ms={elapsed_ms} completion_ms={elapsed_ms} logical_requests={request_count} http_sends={} cache_hits={cache_hits} request_bytes={} response_bytes={} estimated_tokens={} max_rendered_tokens={} synthetic_input_tokens={} synthetic_output_tokens={} peak_transport={} findings={finding_count}",
        metrics.requests.load(Ordering::Relaxed),
        metrics.request_bytes.load(Ordering::Relaxed),
        metrics.response_bytes.load(Ordering::Relaxed),
        metrics.estimated_tokens.load(Ordering::Relaxed),
        metrics.max_rendered_tokens.load(Ordering::Relaxed),
        metrics.synthetic_input_tokens.load(Ordering::Relaxed),
        metrics.synthetic_output_tokens.load(Ordering::Relaxed),
        metrics.peak.load(Ordering::Relaxed),
    );
    Ok(())
}

struct HomeDirectory(Option<std::ffi::OsString>);

impl HomeDirectory {
    fn set(path: std::path::PathBuf) -> Self {
        let previous = std::env::var_os("HOME");
        // The adapters read HOME directly. CandidateExecution has no home setting.
        unsafe { std::env::set_var("HOME", path) };
        Self(previous)
    }
}

impl Drop for HomeDirectory {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}

async fn run_over_exploring_scheduler_e2e() -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(120), async {
        let provider = LoopbackProvider::start().await?;
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path())?;
        let model = "antiburn-benchmark-scheduler-selection";
        let (connection, capabilities) = capabilities_for_model(&provider.endpoint, model, false)?;
        provider.register_response(
            model,
            response_profile(CHECK_ID, "justified_or_minor", capabilities.clone())?,
        )?;
        let handle = crate::jev::worker::WorkerHandle::default();
        handle
            .set_system_one_connection(connection, None)
            .map_err(|error| anyhow::anyhow!("invalid benchmark connection: {error:?}"))?;
        let seed_store = store.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            for index in 0..24 {
                let repeated = format!(
                    "// session-specific fixture: selection-{index:02}\n{}",
                    "same recorded parser evidence ".repeat(256)
                );
                let paths = vec!["/synthetic/parser.rs"; 3];
                fixture_in_store(
                    seed_store.clone(),
                    &format!("selection-{index:02}"),
                    "user",
                    &paths,
                    &repeated,
                );
            }
            let checks = crate::jev::worker::registered_checks();
            let ids = checks.iter().map(|check| check.id()).collect::<Vec<_>>();
            for check in checks {
                seed_store
                    .set_check_enabled(
                        antiburn_local::checks::DetectorId::from_key(check.id())
                            .ok_or_else(|| anyhow::anyhow!("unknown registered check"))?,
                        true,
                    )
                    ?;
            }
            seed_store.capture_burn_check_boundaries(&ids, unix_now())?;
            let candidates = seed_store
                .burn_check_candidates_for_revision(
                    CHECK_ID,
                    &CHECK.evaluator_revision(),
                    unix_now(),
                    POLICY.idle_secs,
                    32,
                )?;
            for candidate in candidates {
                for check in checks {
                    let input = crate::store::BurnCheckInput {
                        key: candidate.session.key.clone(),
                        check_id: check.id().into(),
                        incarnation: candidate.incarnation,
                        source_generation: candidate.source_generation,
                        source_fingerprint: candidate.source_fingerprint.clone(),
                        activity_cursor: candidate.activity_cursor.clone(),
                        published_fence: candidate.published_fence,
                        input_revision: format!("selection:{}", check.id()),
                        evaluator_revision: check.evaluator_revision(),
                        boundary_at_epoch: candidate.boundary_at_epoch,
                    };
                    seed_store
                        .queue_burn_check_assessment(
                            &input,
                            unix_now(),
                            check.policy().idle_secs,
                        )?;
                }
            }
            Ok(())
        })
        .await??;

        let checks: &[&'static dyn crate::jev::worker::JevCheckDescriptor] = &[&CHECK];
        for check in checks {
            let candidates = store
                .burn_check_candidates_for_revision(
                    check.id(),
                    &check.evaluator_revision(),
                    unix_now(),
                    check.policy().idle_secs,
                    32,
                )?;
            anyhow::ensure!(candidates.len() == 24, "check={} candidates={}", check.id(), candidates.len());
        }

        let mut cursor = crate::jev::worker::SchedulerCursor::default();
        let mut served = std::collections::BTreeSet::new();
        let mut sessions = std::collections::BTreeSet::new();
        let mut selected_candidates = Vec::new();
        let mut slice_count = 0;
        let mut slices = 0;
        let started = Instant::now();
        let effects = crate::jev::worker::TestWorkerEffects::default();
        let events = crate::session_lifecycle::SessionEvents::default();
        for _ in 0..24 {
            if slice_count == crate::jev::worker::CANDIDATES_PER_WAKE {
                slice_count = 0;
                slices += 1;
            }
            let (index, candidate, _) = crate::jev::worker::schedule_candidate_turn(
                &store,
                &handle,
                checks,
                &mut cursor,
                unix_now(),
                &served,
                &mut slice_count,
            )?
            .ok_or_else(|| {
                anyhow::anyhow!("eligible candidates ended after {} selections", sessions.len())
            })?;
            let check_id = checks[index].id();
            anyhow::ensure!(
                served.insert((check_id, candidate.session.key.clone())),
                "scheduler selected a duplicate candidate"
            );
            sessions.insert(candidate.session.key.session_id.clone());
            selected_candidates.push(candidate.clone());
            handle.start_candidate_turn(check_id, &candidate.session.key);
            let result = CHECK
                .run_candidate(CandidateExecution {
                    effects: &effects,
                    store: &store,
                    candidate: &candidate,
                    client: crate::jev::client::TypeSafeClient::new("unused".to_owned())?,
                    handle: &handle,
                    key_generation: 1,
                    events: &events,
                })
                .await;
            handle.end_candidate_turn(check_id, &candidate.session.key);
            result?;
        }
        anyhow::ensure!(sessions.len() == 24, "selected sessions={}", sessions.len());
        anyhow::ensure!(slices == 1, "candidate slices={slices}");
        anyhow::ensure!(cursor.turn == 24, "scheduler turn={}", cursor.turn);
        let mut clean = 0;
        let mut logical_requests = 0_usize;
        for candidate in selected_candidates {
            let saved = store
                .burn_check_assessment(&candidate.session.key, CHECK_ID)?
                .ok_or_else(|| anyhow::anyhow!("candidate has no durable assessment"))?;
            anyhow::ensure!(saved.status == "completed", "assessment status={}", saved.status);
            anyhow::ensure!(saved.request_count == 1, "session={} logical requests={}", candidate.session.key.session_id, saved.request_count);
            logical_requests += saved.request_count;
            let publication: Publication = serde_json::from_str(
                saved.result_json.as_deref().ok_or_else(|| anyhow::anyhow!("assessment has no result"))?,
            )?;
            anyhow::ensure!(
                publication.assessment.findings.is_empty()
                    && publication_has_clean_coverage(&publication.assessment),
                "scheduler fixture was not a fully assessed justified-read result: session={}",
                candidate.session.key.session_id
            );
            clean += 1;
        }
        anyhow::ensure!(clean == 24, "scheduler clean results={clean}");
        anyhow::ensure!(effects.events.load(Ordering::Relaxed) == 24);
        anyhow::ensure!(
            effects
                .assessments
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len()
                == 24
        );
        let metrics = provider.stop().await?;
        anyhow::ensure!(
            metrics.requests.load(Ordering::Relaxed) == logical_requests,
            "scheduler HTTP sends={} logical requests={logical_requests}",
            metrics.requests.load(Ordering::Relaxed)
        );
        anyhow::ensure!(
            metrics.max_questions.load(Ordering::Relaxed)
                <= capabilities.questions_per_request.value.unwrap_or_default() as usize,
            "check={} request question cap exceeded: {}",
            CHECK_ID,
            metrics.max_questions.load(Ordering::Relaxed)
        );
        let cache_hits = store.burn_check_usage_summary()?.cache_hits;
        println!(
            "workload=scheduler_selection status=PASS sessions={} selections={} manual_slices={} case_outcomes=clean:24 first_persisted_finding_ms=none completion_ms={} logical_requests={logical_requests} http_sends={} cache_hits={cache_hits} request_bytes={} response_bytes={} estimated_tokens={} max_rendered_tokens={} synthetic_input_tokens={} synthetic_output_tokens={} peak_transport={} findings=0 slice_limit={}",
            sessions.len(),
            cursor.turn,
            slices,
            started.elapsed().as_millis(),
            metrics.requests.load(Ordering::Relaxed),
            metrics.request_bytes.load(Ordering::Relaxed),
            metrics.response_bytes.load(Ordering::Relaxed),
            metrics.estimated_tokens.load(Ordering::Relaxed),
            metrics.max_rendered_tokens.load(Ordering::Relaxed),
            metrics.synthetic_input_tokens.load(Ordering::Relaxed),
            metrics.synthetic_output_tokens.load(Ordering::Relaxed),
            metrics.peak.load(Ordering::Relaxed),
            crate::jev::worker::CANDIDATES_PER_WAKE,
        );
        Ok::<(), anyhow::Error>(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
#[ignore = "Run scheduler throughput benchmarks explicitly"]
async fn scheduler_selects_24_sessions_beyond_one_manual_slice() {
    run_over_exploring_scheduler_e2e().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "Run compact pipeline throughput benchmarks explicitly"]
async fn compact_smart_check_pipeline_regression() {
    run_pipeline_regression().await.unwrap();
}

const LARGE_SESSION_RECORDS: usize = 25_000;

fn large_session_fixture(
    index: usize,
    record_count: usize,
) -> anyhow::Result<(Store, BurnCheckCandidate, serde_json::Value)> {
    let mut timings = crate::jev::benchmark::FixtureTimings::new();
    anyhow::ensure!(record_count >= 6, "Read fixture needs at least six records");
    let parser_output = "recorded parser output 123\n".repeat(320);
    let background_records = record_count - 6;
    let mut background = serde_json::Value::Null;
    let mut fixture = crate::scope_creep_worker::tests::NativeFixture::new(0);
    fixture.store =
        Store::open_in_memory(std::path::Path::new("/synthetic/benchmark-over-exploring"))?;
    fixture
        .store
        .set_check_enabled(antiburn_local::checks::DetectorId::ScopeCreep, true)?;
    fixture
        .store
        .set_check_enabled(antiburn_local::checks::DetectorId::OverExploring, true)?;
    fixture
        .store
        .capture_burn_check_boundaries(&[crate::scope_creep_worker::CHECK_ID, CHECK_ID], 0)?;
    timings.checkpoint("fixture_initialization");
    let native_source_timings = crate::jev::benchmark::with_native_part_batch(
        &fixture.directory.path().join("opencode.db"),
        "scope",
        |batch| {
            batch.clear_session()?;
            background = batch.append_background(0, background_records)?;
            batch.append(background_records, "user", serde_json::json!({"type":"text","text":"Fix the parser. Reuse matching recorded reads."}))?;
            for read in 0..4 {
                batch.append(
                    background_records + 1 + read,
                    "assistant",
                    indexed_read_record(
                        "/synthetic/parser.rs",
                        &format!("parser-read-{read}"),
                        &parser_output,
                    ),
                )?;
            }
            batch.append(
                background_records + 5,
                "user",
                serde_json::json!({"type":"text","text":"Now explain the result."}),
            )?;
            Ok(())
        },
    )?;
    timings.checkpoint("native_source_build");
    let source = rusqlite::Connection::open(fixture.directory.path().join("opencode.db"))?;
    let (records, source_bytes): (usize, usize) = source.query_row(
        "SELECT COUNT(*), SUM(length(CAST(data AS BLOB))) FROM part",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    drop(source);
    timings.checkpoint("source_payload_measurement");
    let mut metadata = serde_json::json!({
        "synthetic": true, "session": index, "source_format": "OpenCodeSqliteV2",
        "records": records, "source_part_bytes": source_bytes,
        "byte_measure": "UTF-8 native part JSON payloads",
        "background_records": background_records, "background": background,
        "indexed_store": "in-memory SQLite; native source database is temporary",
        "read_requests": 4, "read_results": 4,
        "read_measure": "target episode; background reads also stay on the main thread",
        "parser_output_bytes_per_read": parser_output.len(),
        "parser_output_bytes": parser_output.len() * 4,
    });
    let published = fixture.publish();
    timings.checkpoint("native_publication");
    let store = fixture.store.clone();
    store
        .lock()
        .execute("UPDATE session SET cwd = '/synthetic'", [])?;
    let candidate = store
        .burn_check_candidates_for_revision(
            CHECK_ID,
            &CHECK.evaluator_revision(),
            1000,
            POLICY.idle_secs,
            8,
        )?
        .into_iter()
        .find(|candidate| candidate.session.key == published.session.key)
        .ok_or_else(|| anyhow::anyhow!("Large read fixture candidate was not admitted"))?;
    timings.checkpoint("candidate_admission");
    let input = load_input(&store, &candidate, &ModelCapabilities::jev_default())
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    metadata["selected_work_items"] = serde_json::json!(input.plan.work_items.len());
    for item in &input.plan.work_items {
        let target = &input.plan.prepared.targets[&item.id];
        let episode = input
            .plan
            .prepared
            .episodes
            .iter()
            .find(|episode| episode.id == target.episode_id)
            .ok_or_else(|| anyhow::anyhow!("large session target has no episode"))?;
        for read_index in &target.read_indexes {
            let read = &episode.reads[*read_index];
            let result = read
                .result
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("large session read has no result"))?;
            anyhow::ensure!(
                !read.request.truncated
                    && !result.truncated
                    && result.recorded_output_bytes >= 8192
                    && result.status
                        == antiburn_local::analysis::jev_evidence::JevReadStatus::Success
                    && result.request_reference_id.as_deref()
                        == Some(read.request.reference_id.as_str())
                    && item.window.evidence.iter().any(|evidence| {
                        evidence.part_id == format!("reads[{read_index}].request")
                            && evidence.source_id == read.request.reference_id
                    })
                    && item.window.evidence.iter().any(|evidence| {
                        evidence.part_id == format!("reads[{read_index}].result")
                            && evidence.source_id == result.reference_id
                    }),
                "large session read lost its complete request/result binding"
            );
        }
    }
    timings.checkpoint("input_preparation_and_validation");
    metadata["setup_phase_ms"] = timings.finish();
    metadata["native_source_phase_ms"] = native_source_timings;
    Ok((store, candidate, metadata))
}

#[ignore = "live provider benchmark; requires provider setup"]
#[tokio::test]
async fn benchmark_large_sessions() {
    crate::jev::benchmark::run_benchmark(&CHECK, |index| {
        std::thread::spawn(move || large_session_fixture(index, LARGE_SESSION_RECORDS))
            .join()
            .map_err(|_| anyhow::anyhow!("large session indexing thread panicked"))?
    })
    .await
    .unwrap();
}

#[test]
fn large_session_has_indexed_bound_read_work() {
    let (store, candidate, metadata) = large_session_fixture(0, 1_000).unwrap();
    assert_eq!(metadata["records"], 1_000);
    assert!(metadata["source_part_bytes"].as_u64().unwrap() > 5 * 1024 * 1024);
    let snapshot = store
        .load_smart_check_inputs(
            &candidate.session.key,
            candidate.published_fence,
            candidate.source_generation,
            crate::smart_check_inputs::DetectorInput::OverExploring,
        )
        .unwrap();
    assert_eq!(
        snapshot
            .content()
            .actions
            .iter()
            .filter(|action| { action.metadata.read_request.is_some() })
            .count(),
        4
    );
    assert_eq!(
        snapshot
            .content()
            .actions
            .iter()
            .filter(|action| { action.metadata.read_result.is_some() })
            .count(),
        4
    );
}

#[tokio::test]
async fn below_minimum_capability_dispatches_no_provider_request() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let provider = LoopbackProvider::start().await.unwrap();
        let (store, candidate) = tokio::task::spawn_blocking(|| fixture(None, "user"))
            .await
            .unwrap();
        let mut capabilities = ModelCapabilities::jev_default();
        capabilities.total_input_tokens = CapabilityLimit::known(
            1,
            antiburn_local::analysis::jev::capabilities::CapabilitySource::Manual,
        );
        capabilities.state_and_longest_question_tokens.value = Some(1);
        capabilities.runtime_context_tokens.value = Some(1);
        let input = load_input(&store, &candidate, &capabilities).unwrap();
        assert!(input.plan.work_items.is_empty());
        let mut plan = input.plan.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        let outcome = run_jev_check_prepared(
            &OverExploringCheck,
            &input.context,
            &mut plan,
            JevRunProgress::default(),
            antiburn_local::analysis::jev::admit_jev_orchestration()
                .await
                .unwrap(),
            move |_batch| {
                callback_calls.fetch_add(1, Ordering::Relaxed);
                async move { Err(JevError::ProviderUnavailable) }
            },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert_eq!(outcome.progress.request_count, 0);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(provider.metrics.requests.load(Ordering::Relaxed), 0);
        provider.stop().await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn constrained_capabilities_execute_fitting_work_without_sending_oversized_work() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let provider = LoopbackProvider::start().await.unwrap();
        let endpoint = provider.endpoint.clone();
        let mut connection = SystemOneConnection::jev_default();
        connection.provider = SystemOneProvider::Custom;
        connection.endpoint = SystemOneEndpoint::ExactUrl(endpoint);
        connection.model = "antiburn-benchmark-model".into();
        connection.response_mode = SystemOneResponseMode::Direct;
        connection.context_override = Some(ContextLimitOverride {
            total_input_tokens: Some(32_768),
            state_and_longest_question_tokens: Some(16_384),
            runtime_context_tokens: Some(16_384),
        });
        let mut capabilities = connection.capabilities().unwrap();
        capabilities.use_ollama_tev1_accounting("", "");
        provider
            .register_response(
                connection.model.clone(),
                response_profile(CHECK_ID, "justified_or_minor", capabilities.clone()).unwrap(),
            )
            .unwrap();
        let handle = crate::jev::worker::WorkerHandle::default();
        handle.set_system_one_connection(connection, None).unwrap();
        let (store, fitting_candidate, oversized_store, oversized_candidate) =
            tokio::task::spawn_blocking(|| {
                let paths = vec![
                    "/synthetic/parser.rs",
                    "/synthetic/parser.rs",
                    "/synthetic/parser.rs",
                    "/synthetic/reference.txt",
                ];
                let fitting_content =
                    "fn parse(input: &str) -> Result<(), Error> { Ok(()) } ".repeat(8);
                let store =
                    Store::open_in_memory(std::path::Path::new("synthetic-fitting")).unwrap();
                let (store, fitting) = fixture_in_store(
                    store,
                    "constrained-fitting",
                    "user",
                    &paths,
                    &fitting_content,
                );
                let oversized_content =
                    "fn parse(input: &str) -> Result<(), Error> { Ok(()) } ".repeat(10_000);
                let oversized_store =
                    Store::open_in_memory(std::path::Path::new("synthetic-oversized")).unwrap();
                let (oversized_store, oversized) = fixture_in_store(
                    oversized_store,
                    "constrained-oversized",
                    "user",
                    &paths,
                    &oversized_content,
                );
                (store, fitting, oversized_store, oversized)
            })
            .await
            .unwrap();
        let input = load_input(&store, &fitting_candidate, &capabilities).unwrap();
        let oversized = load_input(&oversized_store, &oversized_candidate, &capabilities).unwrap();
        assert!(!input.plan.work_items.is_empty());
        assert!(oversized.plan.work_items.is_empty());
        assert!(
            store
                .queue_burn_check_assessment(&input.durable, unix_now(), POLICY.idle_secs)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(
                    &input.durable,
                    unix_now(),
                    POLICY.lease_secs,
                    POLICY.idle_secs,
                )
                .unwrap()
        );
        let packed = match &input.plan.shared_context {
            Some(shared) => antiburn_local::analysis::jev::pack_work_items_with_shared_context(
                &input.plan.work_items,
                &capabilities,
                shared,
            ),
            None => antiburn_local::analysis::jev::pack_work_items_with_capabilities(
                &input.plan.work_items,
                &capabilities,
            ),
        };
        assert!(!packed.batches.is_empty());
        let batch = Arc::new(packed.batches.into_iter().next().unwrap());
        assert!(
            batch.request.questions.len()
                <= capabilities.questions_per_request.value.unwrap() as usize
        );
        assert!(
            antiburn_local::analysis::jev::estimate_jev_rendered_question_tokens(
                &batch.request,
                &capabilities,
            )
            .unwrap()
                <= capabilities.usable_input_tokens().unwrap()
        );
        execute_batch_for_test(
            TestBatchExecution {
                store: &store,
                input: &input.durable,
                client: crate::jev::client::TypeSafeClient::new("unused".to_owned()).unwrap(),
                handle: &handle,
                key_generation: 1,
                events: &crate::session_lifecycle::SessionEvents::default(),
                policy: POLICY,
                capabilities: &capabilities,
            },
            batch,
        )
        .await
        .unwrap();
        assert_eq!(provider.metrics.requests.load(Ordering::Relaxed), 1);
        provider.stop().await.unwrap();
    })
    .await
    .unwrap();
}

fn dispatched_positive_read_fixture(
    capabilities: ModelCapabilities,
    handle: &crate::jev::worker::WorkerHandle,
    provider_generation: u64,
    attempts: usize,
) -> anyhow::Result<(
    tempfile::TempDir,
    Store,
    BurnCheckCandidate,
    Arc<PreparedInput>,
    AssessmentCursor,
)> {
    let directory = tempfile::tempdir()?;
    let read_content = "recorded parser output 123\n".repeat(320);
    let (store, candidate) = fixture_in_store(
        Store::open(directory.path())?,
        "over-cache-positive",
        "user",
        &["/synthetic/parser.rs"; 2],
        &read_content,
    );
    let input = load_input(&store, &candidate, &capabilities)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    anyhow::ensure!(
        input.plan.prepared.candidates.len() == 1 && !input.plan.work_items.is_empty(),
        "positive recovery fixture must have one selected read target: candidates={}",
        input.plan.prepared.candidates.len()
    );
    let now = unix_now();
    anyhow::ensure!(
        store.queue_burn_check_assessment(&input.durable, now, POLICY.idle_secs)?,
        "positive recovery assessment was not queued"
    );
    anyhow::ensure!(
        store.claim_burn_check_assessment(
            &input.durable,
            now,
            POLICY.lease_secs,
            POLICY.idle_secs,
        )?,
        "positive recovery assessment was not claimed"
    );
    let mut sampling = new_sampling()?;
    synchronize_sampling(&input.plan, &mut sampling)
        .map_err(|error| anyhow::anyhow!("sampling setup failed: {error:?}"))?;
    sampling.begin_run();
    let job = sampling
        .choose_job()
        .ok_or_else(|| anyhow::anyhow!("positive recovery fixture has no sampling job"))?;
    let mut plan = input.plan.clone();
    PreparedAssessment::select_jobs(&mut plan, std::slice::from_ref(&job))?;
    let identities = request_identities(handle, &input.durable, &capabilities, &plan);
    anyhow::ensure!(
        !identities.is_empty(),
        "positive recovery has no request identity"
    );
    let mut cursor = restore_cursor(None, &input.durable, provider_generation);
    cursor.sampling = Some(sampling);
    cursor.active_job = Some(job);
    cursor.run_started = true;
    cursor.result = Some(OverExploringCheck.reduce(&input.plan, &[], false)?);
    cursor.active_capabilities = Some(capabilities);
    cursor.active_connection = Some(handle.system_one_connection());
    cursor.active_request_identities.clone_from(&identities);
    anyhow::ensure!(
        save_cursor(&store, &input.durable, &cursor)?,
        "positive recovery cursor was not saved"
    );
    for identity in identities {
        store.lock().execute(
            "INSERT INTO burn_check_dispatch_attempt
                (request_identity, environment_key, agent, session_id, attempts, terminal,
                 last_attempt_at_epoch)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                identity,
                input.durable.key.environment_key,
                input.durable.key.agent,
                input.durable.key.session_id,
                attempts,
                i64::from(attempts >= 3),
                now,
            ],
        )?;
    }
    Ok((directory, store, candidate, input, cursor))
}

#[tokio::test]
async fn accepted_third_attempt_cache_resumes_through_worker_and_persists_finding() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let provider = LoopbackProvider::start().await.unwrap();
        let model = "antiburn-benchmark-over-cache-positive";
        let (connection, capabilities) =
            capabilities_for_model(&provider.endpoint, model, false).unwrap();
        provider
            .register_response(
                model,
                response_profile(CHECK_ID, "likely_excess", capabilities.clone()).unwrap(),
            )
            .unwrap();
        let handle = crate::jev::worker::WorkerHandle::default();
        handle.set_system_one_connection(connection, None).unwrap();
        let setup_handle = Arc::new(handle);
        let setup_capabilities = capabilities.clone();
        let ((_store_directory, store, candidate, input, cursor), handle_owner) =
            tokio::task::spawn_blocking(move || {
                let fixture =
                    dispatched_positive_read_fixture(setup_capabilities, &setup_handle, 1, 2)?;
                Ok::<_, anyhow::Error>((fixture, setup_handle))
            })
            .await
            .unwrap()
            .unwrap();
        let handle = handle_owner.as_ref();
        let job = cursor.active_job.unwrap();
        let mut plan = input.plan.clone();
        PreparedAssessment::select_jobs(&mut plan, std::slice::from_ref(&job)).unwrap();
        let events = crate::session_lifecycle::SessionEvents::default();
        let packed = match &plan.shared_context {
            Some(shared) => antiburn_local::analysis::jev::pack_work_items_with_shared_context(
                &plan.work_items,
                &capabilities,
                shared,
            ),
            None => antiburn_local::analysis::jev::pack_work_items_with_capabilities(
                &plan.work_items,
                &capabilities,
            ),
        };
        let batch = Arc::new(packed.batches.into_iter().next().unwrap());
        execute_batch_for_test(
            TestBatchExecution {
                store: &store,
                input: &input.durable,
                client: crate::jev::client::TypeSafeClient::new("unused".to_owned()).unwrap(),
                handle,
                key_generation: 1,
                events: &events,
                policy: POLICY,
                capabilities: &capabilities,
            },
            Arc::clone(&batch),
        )
        .await
        .unwrap();
        assert_eq!(
            store
                .burn_check_dispatch_attempts(&cursor.active_request_identities)
                .unwrap(),
            3 * cursor.active_request_identities.len()
        );
        assert_eq!(provider.metrics.requests.load(Ordering::Relaxed), 1);
        assert!(
            store
                .release_failed_burn_check_lease(&input.durable, "continuing", unix_now())
                .unwrap()
        );
        let effects = crate::jev::worker::TestWorkerEffects::default();
        let events = crate::session_lifecycle::SessionEvents::default();
        handle.start_candidate_turn(CHECK_ID, &candidate.session.key);
        let resumed = CHECK
            .run_candidate(crate::jev::worker::CandidateExecution {
                effects: &effects,
                store: &store,
                candidate: &candidate,
                client: crate::jev::client::TypeSafeClient::new("unused".into()).unwrap(),
                handle,
                key_generation: 1,
                events: &events,
            })
            .await;
        handle.end_candidate_turn(CHECK_ID, &candidate.session.key);
        resumed.unwrap();
        assert_eq!(provider.metrics.requests.load(Ordering::Relaxed), 1);
        assert_eq!(store.burn_check_usage_summary().unwrap().cache_hits, 1);
        assert_eq!(
            store
                .burn_check_dispatch_attempts(&cursor.active_request_identities)
                .unwrap(),
            3 * cursor.active_request_identities.len()
        );
        let saved = store
            .burn_check_assessment(&candidate.session.key, CHECK_ID)
            .unwrap()
            .unwrap();
        assert_eq!(saved.status, "completed");
        assert_eq!(saved.request_count, 1);
        let publication: Publication =
            serde_json::from_str(saved.result_json.as_deref().unwrap()).unwrap();
        assert_eq!(publication.assessment.findings.len(), 1);
        assert!(publishable_finding(
            &publication.assessment.findings[0],
            &publication
        ));
        provider.stop().await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn exhausted_cache_miss_does_not_send_a_fourth_request() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let provider = LoopbackProvider::start().await.unwrap();
        let model = "antiburn-benchmark-over-cache-exhausted";
        let (connection, capabilities) =
            capabilities_for_model(&provider.endpoint, model, false).unwrap();
        provider
            .register_response(
                model,
                response_profile(CHECK_ID, "likely_excess", capabilities.clone()).unwrap(),
            )
            .unwrap();
        let handle = crate::jev::worker::WorkerHandle::default();
        handle.set_system_one_connection(connection, None).unwrap();
        let setup_handle = Arc::new(handle);
        let setup_capabilities = capabilities.clone();
        let ((_store_directory, store, candidate, input, cursor), handle_owner) =
            tokio::task::spawn_blocking(move || {
                let fixture =
                    dispatched_positive_read_fixture(setup_capabilities, &setup_handle, 1, 3)?;
                Ok::<_, anyhow::Error>((fixture, setup_handle))
            })
            .await
            .unwrap()
            .unwrap();
        let handle = handle_owner.as_ref();
        assert_eq!(
            store
                .burn_check_dispatch_attempts(&cursor.active_request_identities)
                .unwrap(),
            3 * cursor.active_request_identities.len()
        );
        assert!(
            store
                .release_failed_burn_check_lease(&input.durable, "continuing", unix_now())
                .unwrap()
        );
        let effects = crate::jev::worker::TestWorkerEffects::default();
        let events = crate::session_lifecycle::SessionEvents::default();
        handle.start_candidate_turn(CHECK_ID, &candidate.session.key);
        CHECK
            .run_candidate(crate::jev::worker::CandidateExecution {
                effects: &effects,
                store: &store,
                candidate: &candidate,
                client: crate::jev::client::TypeSafeClient::new("unused".into()).unwrap(),
                handle,
                key_generation: 1,
                events: &events,
            })
            .await
            .unwrap();
        handle.end_candidate_turn(CHECK_ID, &candidate.session.key);
        assert_eq!(provider.metrics.requests.load(Ordering::Relaxed), 0);
        assert_eq!(
            store
                .burn_check_dispatch_attempts(&cursor.active_request_identities)
                .unwrap(),
            3 * cursor.active_request_identities.len()
        );
        let saved = store
            .burn_check_assessment(&candidate.session.key, CHECK_ID)
            .unwrap()
            .unwrap();
        assert_eq!(saved.status, "failed");
        assert_eq!(saved.request_count, 0);
        let publication: Publication =
            serde_json::from_str(saved.result_json.as_deref().unwrap()).unwrap();
        assert!(publication.assessment.findings.is_empty());
        assert!(!publication.assessment.unassessed.is_empty());
        assert!(!publication_has_clean_coverage(&publication.assessment));
        provider.stop().await.unwrap();
    })
    .await
    .unwrap();
}

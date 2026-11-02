//! Bounded stored-session reads from an already-running WSL V2 service.

use super::*;
use crate::opencode_registration::{
    LocalEndpoint, MAX_REGISTRATION_BYTES, RegistrationError,
    ServiceRegistration as WslServiceInfo, decode_registration,
};
use base64::Engine;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::time::Instant;

const SERVICE_MAX_BYTES: usize = 64 * 1024;
const EXPORT_MAX_BYTES: usize = 16 * 1024 * 1024;
const EXPORT_MAX_MESSAGES: usize = 100_000;
const CURSOR_MAX_BYTES: usize = 4096;
const STATE_HOME_MAX_BYTES: usize = 4096;
const V2_SESSION_PAGE_SIZE: usize = 100;
const V2_SESSION_MAX_RECORDS: usize = 500;
const V2_SESSION_MAX_PAGES: usize = V2_SESSION_MAX_RECORDS / V2_SESSION_PAGE_SIZE;
const V2_SESSION_MAX_EXPORTS: usize = 100;
const V2_SESSION_MAX_BYTES: usize = 32 * 1024 * 1024;
pub(super) const V2_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(60);
const HTTP_STATUS_TRAILER: &[u8] = b"\nantiburn-http-status:200\n";

struct ExportByteBudget {
    remaining: AtomicUsize,
}

impl ExportByteBudget {
    fn new(remaining: usize) -> Self {
        Self {
            remaining: AtomicUsize::new(remaining),
        }
    }

    fn remaining(&self) -> usize {
        self.remaining.load(Ordering::Relaxed)
    }

    fn charge(&self, count: usize) -> Result<(), CliBridgeError> {
        match self
            .remaining
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                Some(remaining.saturating_sub(count))
            }) {
            Ok(remaining) if count <= remaining => Ok(()),
            _ => Err(CliBridgeError::Oversized),
        }
    }
}

#[derive(Deserialize)]
struct V2SessionPage {
    data: Vec<Value>,
    cursor: V2SessionCursor,
}

#[derive(Deserialize)]
struct V2SessionCursor {
    next: Option<String>,
}

pub(super) async fn discover_recent_v2_api(
    info: &WslEnvironmentInfo,
    cutoff: i64,
) -> Result<Vec<SessionLog>, CliBridgeError> {
    let deadline = Instant::now() + V2_DISCOVERY_TIMEOUT;
    let service_path = tokio::time::timeout_at(deadline, service_path(info))
        .await
        .map_err(|_| CliBridgeError::Timeout)??;
    let service = tokio::time::timeout_at(deadline, read_service(&service_path))
        .await
        .map_err(|_| CliBridgeError::Timeout)??;
    let environment = &info.context.environment;
    let logs = discover_from_service(
        environment,
        &service,
        cutoff,
        deadline,
        |path, limit, budget| {
            let environment = environment.clone();
            let server = service.endpoint.base_url().as_str().to_owned();
            let password = service.endpoint.password().map(str::to_owned);
            async move {
                run_v2_http(
                    &environment,
                    &server,
                    &path,
                    password.as_deref(),
                    limit,
                    budget,
                )
                .await
            }
        },
    )
    .await?;
    if tokio::time::timeout_at(deadline, read_service(&service_path))
        .await
        .map_err(|_| CliBridgeError::Timeout)??
        != service
    {
        return Err(invalid("registered WSL service changed during discovery"));
    }
    Ok(logs)
}

async fn service_path(info: &WslEnvironmentInfo) -> Result<PathBuf, CliBridgeError> {
    let state_home = if info.context.environment.is_native() {
        env_path_when_real_home(&info.context.home, "XDG_STATE_HOME")
    } else {
        let mut command = crate::platform::environment::configure_command_for_environment(
            &info.context.environment,
            "printenv",
            None,
        )
        .map_err(|_| invalid("WSL state directory could not be resolved"))?;
        command
            .arg("XDG_STATE_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|_| invalid("WSL state directory could not be resolved"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| invalid("WSL state stdout was unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| invalid("WSL state stderr was unavailable"))?;
        let output = tokio::time::timeout(
            CLI_TIMEOUT,
            collect_opencode_cli_output(child, stdout, stderr, STATE_HOME_MAX_BYTES),
        )
        .await
        .map_err(|_| CliBridgeError::Timeout)?;
        if output
            .2
            .as_ref()
            .is_ok_and(|status| status.code() == Some(1))
            && output.0.as_ref().is_ok_and(Vec::is_empty)
        {
            None
        } else {
            let bytes = classify_opencode_cli_output(output, STATE_HOME_MAX_BYTES)?;
            let value = std::str::from_utf8(&bytes)
                .map_err(|_| invalid("WSL state directory was invalid"))?;
            let value = value.strip_suffix('\n').unwrap_or(value);
            if value.contains(['\r', '\n', '\0']) {
                return Err(invalid("WSL state directory was invalid"));
            }
            (!value.is_empty()).then(|| PathBuf::from(value))
        }
    };
    service_path_in(&info.context, state_home.as_deref())
}

fn service_path_in(
    context: &crate::platform::environment::AgentContext,
    state_home: Option<&Path>,
) -> Result<PathBuf, CliBridgeError> {
    if let Some(state_home) = state_home.filter(|path| {
        if context.environment.is_native() {
            path.is_absolute()
        } else {
            path.to_string_lossy().starts_with('/')
        }
    }) {
        if context.environment.is_native() {
            return Ok(state_home.join("opencode/service.json"));
        }
        let path = format!(
            "{}/opencode/service.json",
            state_home.to_string_lossy().trim_end_matches('/')
        );
        return context
            .environment
            .host_path(&path)
            .map_err(|_| invalid("WSL state directory could not be mapped"));
    }
    Ok(context.home.join(".local/state/opencode/service.json"))
}

async fn read_service(path: &Path) -> Result<WslServiceInfo, CliBridgeError> {
    let file = tokio::fs::File::open(path).await.map_err(|_| {
        CliBridgeError::Unavailable("registered WSL service was unavailable".into())
    })?;
    let bytes = read_bounded(file, MAX_REGISTRATION_BYTES + 1)
        .await
        .map_err(|_| invalid("registered WSL service could not be read"))?;
    parse_service(&bytes)
}

fn parse_service(bytes: &[u8]) -> Result<WslServiceInfo, CliBridgeError> {
    let service = decode_registration(bytes).map_err(|error| match error {
        RegistrationError::TooLarge => CliBridgeError::Oversized,
        _ => invalid("registered WSL service metadata was invalid"),
    })?;
    if !service
        .version
        .as_deref()
        .is_some_and(|version| version.trim_start_matches('v').starts_with("2."))
    {
        return Err(invalid(
            "registered WSL service is not a supported local V2 service",
        ));
    }
    Ok(service)
}

async fn discover_from_service<F, Fut>(
    environment: &DiscoveryEnvironment,
    service: &WslServiceInfo,
    cutoff: i64,
    deadline: Instant,
    mut request: F,
) -> Result<Vec<SessionLog>, CliBridgeError>
where
    F: FnMut(String, usize, Option<Arc<ExportByteBudget>>) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, CliBridgeError>> + Send + 'static,
{
    // Final service checks must finish before the discovery deadline.
    let export_deadline = deadline - CLI_TIMEOUT;
    let mut total_bytes = 0;
    let records = tokio::time::timeout_at(export_deadline, async {
        probe_service(
            service,
            &request("/api/info".into(), SERVICE_MAX_BYTES, None).await?,
        )?;
        list_sessions(cutoff, &mut total_bytes, &mut |path, limit| {
            request(path, limit, None)
        })
        .await
    })
    .await
    .map_err(|_| CliBridgeError::Timeout)??;
    let logs = collect_v2_exports(
        environment,
        records,
        total_bytes,
        export_deadline,
        &mut request,
    )
    .await?;
    let health = tokio::time::timeout_at(
        deadline,
        request("/api/info".into(), SERVICE_MAX_BYTES, None),
    )
    .await
    .map_err(|_| CliBridgeError::Timeout)??;
    probe_service(service, &health)?;
    Ok(logs)
}

async fn collect_v2_exports<F, Fut>(
    environment: &DiscoveryEnvironment,
    records: Vec<(String, i64)>,
    total_bytes: usize,
    deadline: Instant,
    request: &mut F,
) -> Result<Vec<SessionLog>, CliBridgeError>
where
    F: FnMut(String, usize, Option<Arc<ExportByteBudget>>) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, CliBridgeError>> + Send + 'static,
{
    let mut records = records.into_iter().enumerate().peekable();
    let mut exports = tokio::task::JoinSet::new();
    let budget = Arc::new(ExportByteBudget::new(
        V2_SESSION_MAX_BYTES
            .checked_sub(total_bytes)
            .ok_or(CliBridgeError::Oversized)?,
    ));
    let mut completed = Vec::new();
    let mut first_error = None;
    loop {
        while exports.len() < CLI_EXPORT_CONCURRENCY && records.peek().is_some() {
            if budget.remaining() == 0 || Instant::now() >= deadline {
                break;
            }
            let (index, (session_id, updated_at)) = records.next().expect("pending record");
            let response = request(
                format!("/api/experimental/session/{session_id}/export"),
                EXPORT_MAX_BYTES,
                Some(budget.clone()),
            );
            exports.spawn(async move { (index, session_id, updated_at, response.await) });
        }
        if exports.is_empty() {
            if records.peek().is_some() {
                first_error.get_or_insert(if Instant::now() >= deadline {
                    CliBridgeError::Timeout
                } else {
                    CliBridgeError::Oversized
                });
            }
            break;
        }
        let result = match tokio::time::timeout_at(deadline, exports.join_next()).await {
            Ok(Some(result)) => result,
            Ok(None) => break,
            Err(_) => {
                first_error.get_or_insert(CliBridgeError::Timeout);
                break;
            }
        };
        let (index, session_id, updated_at, output) = match result {
            Ok(result) => result,
            Err(_) => {
                first_error.get_or_insert_with(|| invalid("V2 session export task failed"));
                break;
            }
        };
        let log = output
            .and_then(|output| exported_session_log(environment, &session_id, updated_at, &output));
        match log {
            Ok(log) => completed.push((index, log)),
            Err(error) => {
                tracing::debug!(event = "opencode_wsl_v2_export_skipped", error = %error,
                    "OpenCode WSL V2 discovery skipped one session export");
                first_error.get_or_insert(error);
            }
        }
    }
    drop(exports);
    if let Some(error) = first_error {
        if completed.is_empty() {
            return Err(error);
        }
        tracing::debug!(event = "opencode_wsl_v2_partial_exports", error = %error,
            "OpenCode WSL V2 discovery kept completed session exports");
    }
    completed.sort_unstable_by_key(|(index, _)| *index);
    Ok(completed.into_iter().map(|(_, log)| log).collect())
}

fn exported_session_log(
    environment: &DiscoveryEnvironment,
    session_id: &str,
    updated_at: i64,
    output: &[u8],
) -> Result<SessionLog, CliBridgeError> {
    let data = parse_v2_session_export(output, session_id)?;
    let content = serde_json::to_string(&data)
        .map_err(|_| invalid("V2 session export could not be encoded"))?;
    Ok(SessionLog {
        environment: environment.clone(),
        agent_type: AgentKind::OpenCode,
        source: SessionSource::Inline {
            label: format!("opencode-v2-export:{session_id}"),
            content,
            source_format: Some(crate::analysis::SourceFormat::OpenCodeV2SessionExport),
        },
        updated_at: Some(updated_at),
    })
}

fn probe_service(service: &WslServiceInfo, bytes: &[u8]) -> Result<(), CliBridgeError> {
    if bytes.len() > SERVICE_MAX_BYTES {
        return Err(CliBridgeError::Oversized);
    }
    let info: Value = serde_json::from_slice(bytes)
        .map_err(|_| invalid("registered WSL service health response was invalid"))?;
    if info.get("pid").and_then(Value::as_u64) != Some(u64::from(service.pid))
        || info.get("version").and_then(Value::as_str) != service.version.as_deref()
    {
        return Err(invalid(
            "registered WSL service PID or version did not match",
        ));
    }
    Ok(())
}

async fn list_sessions<F, Fut>(
    cutoff: i64,
    total_bytes: &mut usize,
    request: &mut F,
) -> Result<Vec<(String, i64)>, CliBridgeError>
where
    F: FnMut(String, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, CliBridgeError>>,
{
    let mut cursor = None;
    let mut records = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut seen_cursors = HashSet::new();
    let mut previous_updated = i64::MAX;
    let mut export_bound = false;
    for page_index in 0..V2_SESSION_MAX_PAGES {
        let output = request(session_list_path(cursor.as_deref()), CLI_METADATA_MAX_BYTES).await?;
        add_bytes(total_bytes, output.len())?;
        let page: V2SessionPage = serde_json::from_slice(&output)
            .map_err(|_| invalid("V2 session list response was invalid"))?;
        if page.data.len() > V2_SESSION_PAGE_SIZE {
            return Err(CliBridgeError::Oversized);
        }
        let short_page = page.data.len() < V2_SESSION_PAGE_SIZE;
        let mut reached_cutoff = false;
        for session in page.data {
            if !valid_session_info(&session) {
                return Err(invalid("V2 session list did not match Session.Info"));
            }
            let id = session
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| valid_opencode_v2_session_id(id))
                .ok_or_else(|| invalid("V2 session list contained an invalid session id"))?;
            if !seen_ids.insert(id.to_owned()) {
                return Err(invalid("V2 session list repeated a session id"));
            }
            let updated = session
                .pointer("/time/updated")
                .and_then(v2_epoch_seconds)
                .ok_or_else(|| invalid("V2 session list omitted a valid update time"))?;
            if updated > previous_updated {
                return Err(invalid("V2 session list was not in newest-first order"));
            }
            previous_updated = updated;
            if updated < cutoff {
                reached_cutoff = true;
            } else if records.len() < V2_SESSION_MAX_EXPORTS {
                records.push((id.to_owned(), updated));
            } else {
                export_bound = true;
            }
        }
        // Stock 2.0.32 returns a next cursor for every nonempty page.
        if short_page || reached_cutoff {
            break;
        }
        let next = page
            .cursor
            .next
            .ok_or_else(|| invalid("V2 session list omitted its next cursor"))?;
        if next.is_empty() || next.len() > CURSOR_MAX_BYTES || !seen_cursors.insert(next.clone()) {
            return Err(invalid("V2 session pagination cursor was invalid"));
        }
        cursor = Some(next);
        if page_index + 1 == V2_SESSION_MAX_PAGES {
            tracing::debug!(
                event = "opencode_wsl_v2_list_bound",
                "OpenCode WSL V2 discovery reached its session scan limit"
            );
        }
    }
    if export_bound {
        tracing::debug!(
            event = "opencode_wsl_v2_export_bound",
            "OpenCode WSL V2 discovery exports only the newest 100 recent sessions"
        );
    }
    Ok(records)
}

fn add_bytes(total: &mut usize, count: usize) -> Result<(), CliBridgeError> {
    *total = total.saturating_add(count);
    if *total > V2_SESSION_MAX_BYTES {
        return Err(CliBridgeError::Oversized);
    }
    Ok(())
}

fn session_list_path(cursor: Option<&str>) -> String {
    match cursor {
        Some(cursor) => format!(
            "{}{cursor}",
            session_cursor_prefix(),
            cursor = encode_query_component(cursor)
        ),
        None => format!("/api/session?limit={V2_SESSION_PAGE_SIZE}&order=desc"),
    }
}

fn session_cursor_prefix() -> String {
    format!("/api/session?limit={V2_SESSION_PAGE_SIZE}&cursor=")
}

fn v2_epoch_seconds(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .filter(|value| *value >= 0)
        .map(|value| value / 1000)
}

fn encode_query_component(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn parse_v2_session_export(output: &[u8], expected_id: &str) -> Result<Value, CliBridgeError> {
    if output.len() > EXPORT_MAX_BYTES {
        return Err(CliBridgeError::Oversized);
    }
    let mut value: Value = serde_json::from_slice(output)
        .map_err(|_| invalid("V2 session export response was invalid"))?;
    let data = value
        .get("data")
        .filter(|data| data.is_object())
        .ok_or_else(|| invalid("V2 session export omitted its data envelope"))?;
    let messages = data
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("V2 session export omitted its messages"))?;
    if messages.len() > EXPORT_MAX_MESSAGES {
        return Err(CliBridgeError::Oversized);
    }
    let mut message_ids = HashSet::new();
    if value.as_object().is_none_or(|object| object.len() != 1)
        || data.as_object().is_none_or(|object| object.len() != 2)
        || data.pointer("/info/id").and_then(Value::as_str) != Some(expected_id)
        || !valid_session_info(&data["info"])
        || messages.iter().any(|message| {
            let id = message.get("id").and_then(Value::as_str);
            id.is_none_or(|id| {
                !id.starts_with("msg_") || !valid_session_id(id) || !message_ids.insert(id)
            }) || !valid_message(message)
                || message
                    .get("sessionID")
                    .is_some_and(|id| id.as_str() != Some(expected_id))
        })
    {
        return Err(invalid(
            "V2 session export did not match SessionTransfer.Data",
        ));
    }
    Ok(value["data"].take())
}

// These validators cover the public transfer schemas at producer 843a6c514e60.
fn required_fields(value: &Value, names: &[&str], check: fn(&Value) -> bool) -> bool {
    names.iter().all(|name| value.get(name).is_some_and(check))
}

fn optional_fields(value: &Value, names: &[&str], check: fn(&Value) -> bool) -> bool {
    names.iter().all(|name| value.get(name).is_none_or(check))
}

fn array_of(value: &Value, check: fn(&Value) -> bool) -> bool {
    value
        .as_array()
        .is_some_and(|values| values.iter().all(check))
}

fn literal(value: &Value, choices: &[&str]) -> bool {
    value.as_str().is_some_and(|value| choices.contains(&value))
}

fn finite(value: &Value) -> bool {
    value.as_f64().is_some_and(f64::is_finite)
}

fn timestamp(value: &Value) -> bool {
    v2_epoch_seconds(value).is_some()
}

fn valid_time(value: &Value) -> bool {
    required_fields(value, &["created"], timestamp)
        && optional_fields(
            value,
            &[
                "updated",
                "idle",
                "viewed",
                "archived",
                "completed",
                "ran",
                "streamed",
            ],
            timestamp,
        )
}

fn valid_model(value: &Value) -> bool {
    required_fields(value, &["id", "providerID"], Value::is_string)
        && optional_fields(value, &["variant"], Value::is_string)
}

fn valid_tokens(value: &Value) -> bool {
    required_fields(value, &["input", "output", "reasoning"], finite)
        && required_fields(&value["cache"], &["read", "write"], finite)
}

fn session_id(value: &Value) -> bool {
    value.as_str().is_some_and(valid_opencode_v2_session_id)
}

fn message_id(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|id| id.starts_with("msg_") && valid_session_id(id))
}

fn valid_location(value: &Value) -> bool {
    required_fields(value, &["directory"], Value::is_string)
}

fn outcome(value: &Value) -> bool {
    literal(value, &["succeeded", "failed", "interrupted"])
}

fn valid_session_info(value: &Value) -> bool {
    required_fields(value, &["id"], session_id)
        && required_fields(value, &["projectID"], Value::is_string)
        && required_fields(value, &["cost"], finite)
        && required_fields(value, &["tokens"], valid_tokens)
        && required_fields(value, &["location"], valid_location)
        && valid_time(&value["time"])
        && required_fields(&value["time"], &["updated"], timestamp)
        && optional_fields(value, &["title", "agent", "subpath"], Value::is_string)
        && optional_fields(value, &["parentID"], session_id)
        && optional_fields(value, &["model"], valid_model)
        && optional_fields(value, &["outcome"], outcome)
        && optional_fields(value, &["metadata"], Value::is_object)
        && optional_fields(value, &["fork"], |fork| {
            session_id(&fork["sessionID"])
                && message_id(&fork["boundary"]["messageID"])
                && literal(&fork["boundary"]["type"], &["before", "through"])
        })
        && optional_fields(value, &["permissions"], |rules| {
            array_of(rules, |rule| {
                required_fields(rule, &["action", "resource"], Value::is_string)
                    && literal(&rule["effect"], &["allow", "deny", "ask"])
            })
        })
        && optional_fields(value, &["revert"], |revert| {
            message_id(&revert["messageID"])
                && optional_fields(revert, &["partID", "snapshot"], Value::is_string)
                && optional_fields(revert, &["files"], |files| {
                    array_of(files, |file| {
                        required_fields(file, &["file", "patch"], Value::is_string)
                            && required_fields(file, &["additions", "deletions"], |n| {
                                n.as_u64().is_some()
                            })
                            && literal(&file["status"], &["added", "deleted", "modified"])
                    })
                })
        })
}

fn valid_error(value: &Value) -> bool {
    required_fields(value, &["type", "message"], Value::is_string)
        && optional_fields(value, &["status"], |status| {
            status.as_u64().is_some_and(|n| (100..=599).contains(&n))
        })
        && optional_fields(value, &["response"], |response| {
            required_fields(response, &["body"], Value::is_string)
        })
}

fn valid_message(value: &Value) -> bool {
    if !message_id(&value["id"])
        || !valid_time(&value["time"])
        || !optional_fields(value, &["metadata"], Value::is_object)
    {
        return false;
    }
    match value["type"].as_str() {
        Some("user") => {
            value["text"].is_string()
                && optional_fields(value, &["files"], |files| array_of(files, valid_file))
                && optional_fields(value, &["agents"], |agents| {
                    array_of(agents, |agent| {
                        agent["name"].is_string()
                            && optional_fields(agent, &["mention"], valid_mention)
                    })
                })
                && optional_fields(value, &["skills"], |skills| {
                    array_of(skills, |skill| {
                        required_fields(skill, &["id", "name"], Value::is_string)
                            && optional_fields(skill, &["text"], Value::is_string)
                            && optional_fields(skill, &["mention"], valid_mention)
                    })
                })
        }
        Some("synthetic" | "system") => {
            value["text"].is_string() && optional_fields(value, &["description"], Value::is_string)
        }
        Some("skill") => required_fields(value, &["skill", "name", "text"], Value::is_string),
        Some("agent-switched") => {
            value["agent"].is_string() && optional_fields(value, &["previous"], Value::is_string)
        }
        Some("model-switched") => {
            valid_model(&value["model"]) && optional_fields(value, &["previous"], valid_model)
        }
        Some("location-switched") => {
            valid_location(&value["location"])
                && optional_fields(value, &["projectID", "subpath"], Value::is_string)
                && optional_fields(value, &["previous"], |previous| {
                    valid_location(&previous["location"])
                        && optional_fields(previous, &["projectID", "subpath"], Value::is_string)
                })
        }
        Some("shell") => {
            value["shellID"]
                .as_str()
                .is_some_and(|id| id.starts_with("sh_"))
                && value["command"].is_string()
                && literal(
                    &value["status"],
                    &["running", "exited", "timeout", "killed"],
                )
                && optional_fields(value, &["exit"], finite)
                && optional_fields(value, &["output"], |output| {
                    output["output"].is_string()
                        && output["truncated"].is_boolean()
                        && required_fields(output, &["cursor", "size"], |n| n.as_u64().is_some())
                })
        }
        Some("assistant") => valid_assistant(value),
        Some("compaction") => valid_compaction(value),
        Some("idle") => {
            outcome(&value["outcome"]) && optional_fields(value, &["error"], valid_error)
        }
        _ => false,
    }
}

fn valid_mention(value: &Value) -> bool {
    required_fields(value, &["start", "end"], finite) && value["text"].is_string()
}

fn valid_file(value: &Value) -> bool {
    required_fields(value, &["data", "mime"], Value::is_string)
        && value["data"].as_str().is_some_and(|data| {
            let unpadded = data.trim_end_matches('=');
            data.len().is_multiple_of(4)
                && data.len() - unpadded.len() <= 2
                && unpadded
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+/".contains(&b))
        })
        && optional_fields(value, &["name", "description"], Value::is_string)
        && optional_fields(value, &["mention"], valid_mention)
        && match value["source"]["type"].as_str() {
            Some("inline") => true,
            Some("uri") => value["source"]["uri"].is_string(),
            _ => false,
        }
}

fn valid_assistant(value: &Value) -> bool {
    value["agent"].is_string()
        && valid_model(&value["model"])
        && array_of(&value["content"], valid_assistant_content)
        && optional_fields(value, &["cost"], finite)
        && optional_fields(value, &["tokens"], valid_tokens)
        && optional_fields(value, &["rawFinish"], Value::is_string)
        && optional_fields(value, &["providerState"], Value::is_object)
        && optional_fields(value, &["error"], valid_error)
        && optional_fields(value, &["finish"], |finish| {
            literal(
                finish,
                &[
                    "stop",
                    "length",
                    "tool-calls",
                    "content-filter",
                    "error",
                    "unknown",
                ],
            )
        })
        && optional_fields(value, &["snapshot"], |snapshot| {
            snapshot.is_object()
                && optional_fields(snapshot, &["start", "end"], Value::is_string)
                && optional_fields(snapshot, &["files"], |files| {
                    array_of(files, Value::is_string)
                })
        })
        && optional_fields(value, &["retry"], |retry| {
            retry["attempt"].as_u64().is_some_and(|n| n > 0)
                && timestamp(&retry["at"])
                && valid_error(&retry["error"])
        })
}

fn valid_assistant_content(value: &Value) -> bool {
    match value["type"].as_str() {
        Some("text" | "reasoning") => {
            value["text"].is_string()
                && optional_fields(value, &["state"], Value::is_object)
                && optional_fields(value, &["time"], valid_time)
        }
        Some("tool") => {
            required_fields(value, &["id", "name"], Value::is_string)
                && valid_time(&value["time"])
                && optional_fields(value, &["executed"], Value::is_boolean)
                && optional_fields(
                    value,
                    &["providerState", "providerResultState"],
                    Value::is_object,
                )
                && valid_tool_state(&value["state"])
        }
        _ => false,
    }
}

fn valid_tool_state(value: &Value) -> bool {
    if !optional_fields(value, &["metadata"], Value::is_object) {
        return false;
    }
    match value["status"].as_str() {
        Some("streaming") => value["input"].is_string(),
        Some("running") => value["input"].is_object() && value["metadata"].is_object(),
        Some("completed") => value["input"].is_object() && valid_tool_output(&value["content"]),
        Some("error") => {
            value["input"].is_object()
                && valid_error(&value["error"])
                && optional_fields(value, &["content"], valid_tool_output)
        }
        _ => false,
    }
}

fn valid_tool_output(value: &Value) -> bool {
    value.as_array().is_some_and(|values| !values.is_empty())
        && array_of(value, |content| match content["type"].as_str() {
            Some("text") => content["text"].is_string(),
            Some("file") => {
                required_fields(content, &["uri", "mime"], Value::is_string)
                    && optional_fields(content, &["name"], Value::is_string)
            }
            _ => false,
        })
}

fn valid_compaction(value: &Value) -> bool {
    literal(&value["reason"], &["auto", "manual"])
        && optional_fields(value, &["cost"], finite)
        && optional_fields(value, &["tokens"], valid_tokens)
        && match value["status"].as_str() {
            Some("running") => required_fields(value, &["summary", "recent"], Value::is_string),
            Some("completed") => {
                required_fields(value, &["summary", "recent"], Value::is_string)
                    && optional_fields(value, &["model"], valid_model)
                    && optional_fields(value, &["providerState"], Value::is_object)
                    && optional_fields(value, &["providerContext"], |context| {
                        context["version"] == 1
                            && context.get("messages").is_some()
                            && required_fields(
                                &context["provenance"],
                                &[
                                    "providerID",
                                    "provider",
                                    "modelID",
                                    "route",
                                    "protocol",
                                    "endpoint",
                                ],
                                Value::is_string,
                            )
                    })
            }
            Some("failed") => valid_error(&value["error"]),
            _ => false,
        }
}

fn invalid(message: &str) -> CliBridgeError {
    CliBridgeError::InvalidOutput(message.into())
}

async fn run_v2_http(
    environment: &DiscoveryEnvironment,
    server: &str,
    path: &str,
    password: Option<&str>,
    max_bytes: usize,
    budget: Option<Arc<ExportByteBudget>>,
) -> Result<Vec<u8>, CliBridgeError> {
    let command = build_v2_http_command(environment, server, path)?;
    run_command(command, password, max_bytes, CLI_TIMEOUT, budget).await
}

async fn run_command(
    mut command: tokio::process::Command,
    password: Option<&str>,
    max_bytes: usize,
    timeout: Duration,
    budget: Option<Arc<ExportByteBudget>>,
) -> Result<Vec<u8>, CliBridgeError> {
    let mut child = command.spawn().map_err(|_| {
        CliBridgeError::Unavailable(
            "WSL V2 HTTP reads require curl in the selected distribution".into(),
        )
    })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| invalid("V2 HTTP stdin was unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid("V2 HTTP stdout was unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| invalid("V2 HTTP stderr was unavailable"))?;
    let output_limit = max_bytes
        .checked_add(HTTP_STATUS_TRAILER.len())
        .ok_or(CliBridgeError::Oversized)?;
    tokio::time::timeout(timeout, async {
        let write = async move {
            stdin.write_all(&curl_config(password)).await?;
            stdin.shutdown().await
        };
        let (write_result, stdout, stderr, status) = tokio::join!(
            write,
            read_http_output(stdout, max_bytes, budget.as_deref()),
            read_bounded(stderr, 64 * 1024 + 1),
            child.wait(),
        );
        let bytes = classify_http_output(stdout?, stderr, status, output_limit)?;
        write_result.map_err(|_| invalid("V2 HTTP stdin write failed"))?;
        Ok(bytes)
    })
    .await
    .map_err(|_| CliBridgeError::Timeout)?
}

async fn read_http_output(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    max_bytes: usize,
    budget: Option<&ExportByteBudget>,
) -> Result<Vec<u8>, CliBridgeError> {
    let output_limit = max_bytes
        .checked_add(HTTP_STATUS_TRAILER.len())
        .ok_or(CliBridgeError::Oversized)?;
    let probe_limit = output_limit
        .checked_add(1)
        .ok_or(CliBridgeError::Oversized)?;
    let mut bytes = Vec::new();
    let mut charged = 0;
    let mut chunk = [0; 8192];
    loop {
        let read_limit = chunk.len().min(probe_limit - bytes.len());
        let count = reader
            .read(&mut chunk[..read_limit])
            .await
            .map_err(|_| invalid("V2 HTTP output could not be read"))?;
        if count == 0 {
            if !has_http_status_trailer(&bytes)
                && let Some(budget) = budget
            {
                budget.charge(bytes.len() - charged)?;
            }
            return Ok(bytes);
        }
        let length = bytes.len() + count;
        // Keep the curl status trailer outside the shared response-body budget.
        let body_bytes = length
            .saturating_sub(HTTP_STATUS_TRAILER.len())
            .min(max_bytes);
        if let Some(budget) = budget {
            budget.charge(body_bytes - charged)?;
        }
        charged = body_bytes;
        if length > output_limit {
            return Err(CliBridgeError::Oversized);
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}

fn has_http_status_trailer(bytes: &[u8]) -> bool {
    let Some(start) = bytes.len().checked_sub(HTTP_STATUS_TRAILER.len()) else {
        return false;
    };
    let trailer = &bytes[start..];
    let status_start = trailer.len() - 4;
    trailer[..status_start] == HTTP_STATUS_TRAILER[..status_start]
        && trailer[status_start..trailer.len() - 1]
            .iter()
            .all(u8::is_ascii_digit)
        && trailer.last() == Some(&b'\n')
}

fn curl_config(password: Option<&str>) -> Vec<u8> {
    match password {
        Some(password) => {
            // Base64 excludes quotes, backslashes, and control bytes in curl config.
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(format!("opencode:{password}"));
            format!("header = \"Authorization: Basic {encoded}\"\n").into_bytes()
        }
        None => Vec::new(),
    }
}

fn classify_http_output(
    mut bytes: Vec<u8>,
    stderr: std::io::Result<Vec<u8>>,
    status: std::io::Result<std::process::ExitStatus>,
    output_limit: usize,
) -> Result<Vec<u8>, CliBridgeError> {
    if bytes.len() > output_limit {
        return Err(CliBridgeError::Oversized);
    }
    let stderr = stderr.map_err(|_| invalid("V2 HTTP error pipe could not be read"))?;
    if stderr.len() > 64 * 1024 {
        return Err(CliBridgeError::Oversized);
    }
    let status = status.map_err(|_| invalid("V2 HTTP command could not be reaped"))?;
    if status.code() == Some(28) {
        return Err(CliBridgeError::Timeout);
    }
    if !status.success() {
        return Err(CliBridgeError::Unavailable(format!(
            "WSL V2 HTTP read failed ({status}); curl must be installed and the registered service must be running"
        )));
    }
    if !bytes.ends_with(HTTP_STATUS_TRAILER) {
        return Err(invalid("V2 HTTP read did not return HTTP 200"));
    }
    bytes.truncate(bytes.len() - HTTP_STATUS_TRAILER.len());
    Ok(bytes)
}

fn build_v2_http_command(
    environment: &DiscoveryEnvironment,
    server: &str,
    path: &str,
) -> Result<tokio::process::Command, CliBridgeError> {
    let endpoint = LocalEndpoint::parse(server, None)
        .map_err(|_| invalid("V2 API server was not numeric loopback"))?;
    let allowed_path = path == "/api/info"
        || path == session_list_path(None)
        || path
            .strip_prefix(&session_cursor_prefix())
            .is_some_and(|cursor| {
                !cursor.is_empty()
                    && cursor.len() <= CURSOR_MAX_BYTES * 3
                    && cursor
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.~%".contains(&byte))
            })
        || path
            .strip_prefix("/api/experimental/session/")
            .and_then(|id| id.strip_suffix("/export"))
            .is_some_and(valid_opencode_v2_session_id);
    if !allowed_path {
        return Err(invalid("V2 API request was not allowed"));
    }
    let mut command =
        crate::platform::environment::configure_command_for_environment(environment, "curl", None)
            .map_err(|_| CliBridgeError::Unavailable("WSL curl command was unavailable".into()))?;
    command.args([
        "--disable",
        "--silent",
        "--show-error",
        "--no-buffer",
        "--globoff",
        "--proxy",
        "",
        "--noproxy",
        "*",
        "--proto",
        "=http",
        "--connect-timeout",
        "5",
        "--max-time",
        "15",
        "--max-redirs",
        "0",
        "--request",
        "GET",
        "--config",
        "-",
        "--write-out",
        "\nantiburn-http-status:%{http_code}\n",
        "--url",
        endpoint
            .base_url()
            .join(path)
            .map_err(|_| invalid("V2 API request URL was invalid"))?
            .as_str(),
    ]);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(command)
}

#[cfg(test)]
#[path = "../tests/opencode_wsl_v2_tests.rs"]
mod tests;

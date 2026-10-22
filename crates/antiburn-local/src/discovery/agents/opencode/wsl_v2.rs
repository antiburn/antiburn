//! Bounded stored-session reads from an already-running WSL V2 service.

use super::*;
use base64::Engine;
use std::future::Future;
use tokio::io::AsyncWriteExt;

const SERVICE_MAX_BYTES: usize = 64 * 1024;
const EXPORT_MAX_BYTES: usize = 16 * 1024 * 1024;
const EXPORT_MAX_MESSAGES: usize = 100_000;
const CURSOR_MAX_BYTES: usize = 4096;
const V2_SESSION_PAGE_SIZE: usize = 100;
const V2_SESSION_MAX_RECORDS: usize = 500;
const V2_SESSION_MAX_PAGES: usize = V2_SESSION_MAX_RECORDS / V2_SESSION_PAGE_SIZE;
const V2_SESSION_MAX_EXPORTS: usize = 100;
const V2_SESSION_MAX_BYTES: usize = 32 * 1024 * 1024;
pub(super) const V2_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(60);
const HTTP_STATUS_TRAILER: &[u8] = b"\nantiburn-http-status:200\n";

#[derive(Deserialize, PartialEq, Eq)]
struct WslServiceInfo {
    url: String,
    pid: u32,
    version: String,
    id: Option<String>,
    password: Option<String>,
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
    let service_path = info.context.home.join(".local/state/opencode/service.json");
    let service = read_service(&service_path).await?;
    let environment = &info.context.environment;
    let server = service.url.as_str();
    let password = service.password.as_deref();
    let logs = discover_from_service(environment, &service, cutoff, |path, limit| async move {
        run_v2_http(environment, server, &path, password, limit).await
    })
    .await?;
    if read_service(&service_path).await? != service {
        return Err(invalid("registered WSL service changed during discovery"));
    }
    Ok(logs)
}

async fn read_service(path: &Path) -> Result<WslServiceInfo, CliBridgeError> {
    let file = tokio::fs::File::open(path).await.map_err(|_| {
        CliBridgeError::Unavailable("registered WSL service was unavailable".into())
    })?;
    let bytes = read_bounded(file, SERVICE_MAX_BYTES + 1)
        .await
        .map_err(|_| invalid("registered WSL service could not be read"))?;
    parse_service(&bytes)
}

fn parse_service(bytes: &[u8]) -> Result<WslServiceInfo, CliBridgeError> {
    if bytes.len() > SERVICE_MAX_BYTES {
        return Err(CliBridgeError::Oversized);
    }
    let service: WslServiceInfo = serde_json::from_slice(bytes)
        .map_err(|_| invalid("registered WSL service metadata was invalid"))?;
    if !loopback_server(&service.url)
        || service.pid == 0
        || !service.version.trim_start_matches('v').starts_with("2.")
        || service
            .password
            .as_deref()
            .is_some_and(|password| password.contains(['\r', '\n', '\0']) || password.len() > 4096)
    {
        return Err(invalid(
            "registered WSL service is not a supported local V2 service",
        ));
    }
    Ok(service)
}

fn loopback_server(server: &str) -> bool {
    let Some(authority) = server.strip_prefix("http://") else {
        return false;
    };
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    authority
        .strip_prefix("127.0.0.1:")
        .or_else(|| authority.strip_prefix("[::1]:"))
        .filter(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|port| port.parse::<u16>().ok())
        .is_some_and(|port| port > 0)
}

async fn discover_from_service<F, Fut>(
    environment: &DiscoveryEnvironment,
    service: &WslServiceInfo,
    cutoff: i64,
    mut request: F,
) -> Result<Vec<SessionLog>, CliBridgeError>
where
    F: FnMut(String, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, CliBridgeError>>,
{
    let mut total_bytes = 0;
    probe_service(
        service,
        &request("/api/info".into(), SERVICE_MAX_BYTES).await?,
    )?;
    let records = list_sessions(cutoff, &mut total_bytes, &mut request).await?;
    let mut logs = Vec::new();
    for (session_id, updated_at) in records {
        let path = format!("/api/experimental/session/{session_id}/export");
        let remaining = V2_SESSION_MAX_BYTES.saturating_sub(total_bytes);
        if remaining == 0 {
            return Err(CliBridgeError::Oversized);
        }
        let output = request(path, EXPORT_MAX_BYTES.min(remaining)).await?;
        add_bytes(&mut total_bytes, output.len())?;
        let data = parse_v2_session_export(&output, &session_id)?;
        let content = serde_json::to_string(&data)
            .map_err(|_| invalid("V2 session export could not be encoded"))?;
        logs.push(SessionLog {
            environment: environment.clone(),
            agent_type: AgentKind::OpenCode,
            source: SessionSource::Inline {
                label: format!("opencode-v2-export:{session_id}"),
                content,
            },
            updated_at: Some(updated_at),
        });
    }
    probe_service(
        service,
        &request("/api/info".into(), SERVICE_MAX_BYTES).await?,
    )?;
    Ok(logs)
}

fn probe_service(service: &WslServiceInfo, bytes: &[u8]) -> Result<(), CliBridgeError> {
    if bytes.len() > SERVICE_MAX_BYTES {
        return Err(CliBridgeError::Oversized);
    }
    let info: Value = serde_json::from_slice(bytes)
        .map_err(|_| invalid("registered WSL service health response was invalid"))?;
    if info.get("pid").and_then(Value::as_u64) != Some(u64::from(service.pid))
        || info.get("version").and_then(Value::as_str) != Some(service.version.as_str())
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
                .filter(|id| id.starts_with("ses_") && valid_session_id(id))
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
            "/api/session?limit={V2_SESSION_PAGE_SIZE}&cursor={}",
            encode_query_component(cursor)
        ),
        None => format!("/api/session?limit={V2_SESSION_PAGE_SIZE}&order=desc"),
    }
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
    value
        .as_str()
        .is_some_and(|id| id.starts_with("ses_") && valid_session_id(id))
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
) -> Result<Vec<u8>, CliBridgeError> {
    let command = build_v2_http_command(environment, server, "GET", path)?;
    run_command(command, password, max_bytes, CLI_TIMEOUT).await
}

async fn run_command(
    mut command: tokio::process::Command,
    password: Option<&str>,
    max_bytes: usize,
    timeout: Duration,
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
        let (write_result, output) = tokio::join!(
            write,
            collect_opencode_cli_output(child, stdout, stderr, output_limit)
        );
        let bytes = classify_http_output(output, output_limit)?;
        write_result.map_err(|_| invalid("V2 HTTP stdin write failed"))?;
        Ok(bytes)
    })
    .await
    .map_err(|_| CliBridgeError::Timeout)?
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
    (stdout, stderr, status): CliProcessOutput,
    output_limit: usize,
) -> Result<Vec<u8>, CliBridgeError> {
    let mut bytes = stdout.map_err(|_| invalid("V2 HTTP output could not be read"))?;
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
    method: &str,
    path: &str,
) -> Result<tokio::process::Command, CliBridgeError> {
    if !loopback_server(server) {
        return Err(invalid("V2 API server was not numeric loopback"));
    }
    let allowed_path = path == "/api/info"
        || path == session_list_path(None)
        || path
            .strip_prefix("/api/session?limit=100&cursor=")
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
            .is_some_and(|id| id.starts_with("ses_") && valid_session_id(id));
    if method != "GET" || !allowed_path {
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
        method,
        "--config",
        "-",
        "--write-out",
        "\nantiburn-http-status:%{http_code}\n",
        "--url",
        &format!("{}{path}", server.trim_end_matches('/')),
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

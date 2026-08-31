//! Deterministic rows and read-only PID diagnostics for local memory reports.

use antiburn_local::analysis::{ModelRun, SessionCost};
use serde::Serialize;

use crate::dto::ActivityEntry;

const SESSIONS_ENV: &str = "ANTIBURN_MEMORY_SESSIONS";
const FIXTURE_SEED_ENV: &str = "ANTIBURN_MEMORY_FIXTURE_SEED";
const MAX_SESSIONS: usize = 500;
const PREFIX: &str = "@antiburn-mem ";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebContentEvent<'a> {
    event: &'static str,
    window: &'a str,
    generation: u64,
    pid: u32,
}

pub fn synthetic_sessions() -> Result<Option<Vec<ActivityEntry>>, String> {
    let Some(count) = std::env::var(SESSIONS_ENV).ok() else {
        return Ok(None);
    };
    let count = count
        .parse::<usize>()
        .map_err(|error| format!("invalid {SESSIONS_ENV}: {error}"))?;
    let seed = std::env::var(FIXTURE_SEED_ENV)
        .ok()
        .map(|value| value.parse::<u64>())
        .transpose()
        .map_err(|error| format!("invalid {FIXTURE_SEED_ENV}: {error}"))?
        .unwrap_or(0);
    generate_sessions(count, seed, time::OffsetDateTime::now_utc()).map(Some)
}

#[cfg(target_os = "macos")]
pub fn report_web_content(window: &tauri::WebviewWindow, generation: u64) {
    use objc2::runtime::AnyObject;

    let result = window.with_webview(move |webview| {
        // SAFETY: Tauri supplies a live WKWebView on the main thread. This
        // feature is excluded from distributed builds.
        let pid = unsafe {
            let view = &*webview.inner().cast::<AnyObject>();
            let selector = objc2::sel!(_webProcessIdentifier);
            let responds: bool = objc2::msg_send![view, respondsToSelector: selector];
            if responds {
                let value: i32 = objc2::msg_send![view, _webProcessIdentifier];
                u32::try_from(value).ok().filter(|value| *value > 0)
            } else {
                None
            }
        };
        if let Some(pid) = pid {
            eprintln!("{}{}", PREFIX, web_content_event(generation, pid));
        } else {
            eprintln!("{PREFIX}{{\"event\":\"webcontent-unavailable\",\"window\":\"popover\",\"generation\":{generation}}}");
        }
    });
    if let Err(error) = result {
        eprintln!(
            "{PREFIX}{{\"event\":\"webcontent-error\",\"window\":\"popover\",\"generation\":{generation},\"message\":{}}}",
            serde_json::Value::String(error.to_string())
        );
    }
}

#[cfg(not(target_os = "macos"))]
pub fn report_web_content(_window: &tauri::WebviewWindow, _generation: u64) {}

fn web_content_event(generation: u64, pid: u32) -> String {
    serde_json::to_string(&WebContentEvent {
        event: "webcontent",
        window: crate::popover::LABEL,
        generation,
        pid,
    })
    .expect("the WebContent diagnostic is serializable")
}

fn generate_sessions(
    count: usize,
    seed: u64,
    now: time::OffsetDateTime,
) -> Result<Vec<ActivityEntry>, String> {
    if count > MAX_SESSIONS {
        return Err(format!("{SESSIONS_ENV} must not exceed {MAX_SESSIONS}"));
    }
    let now = now
        .replace_second(0)
        .and_then(|value| value.replace_nanosecond(0))
        .expect("zero is a valid second and nanosecond");
    let agents = ["claude-code", "codex", "gemini-cli"];
    let models = ["claude-opus-4-1", "gpt-5", "gemini-2.5-pro"];
    Ok((0..count)
        .map(|index| {
            let shape = (index as u64).wrapping_add(seed) as usize;
            let agent = agents[shape % agents.len()];
            let model = models[shape % models.len()];
            let timestamp = now - time::Duration::minutes(index as i64 * 15);
            ActivityEntry {
                agent: agent.to_string(),
                session_id: format!("probe-{seed:08x}-{index:04}"),
                repo: if shape.is_multiple_of(9) {
                    "deterministic-repository-with-a-long-display-name".to_string()
                } else {
                    format!("synthetic-repo-{}", shape % 7)
                },
                timestamp: timestamp
                    .format(&time::format_description::well_known::Rfc3339)
                    .expect("OffsetDateTime supports RFC 3339"),
                is_active: index < 3,
                surface: if shape.is_multiple_of(4) {
                    "ide_desktop"
                } else {
                    "cli"
                }
                .to_string(),
                wsl_distro: shape
                    .is_multiple_of(11)
                    .then(|| "Ubuntu-24.04".to_string()),
                title: Some(if shape.is_multiple_of(8) {
                    "Investigate deterministic renderer memory with a deliberately long synthetic title".to_string()
                } else {
                    format!("Synthetic coding session {index}")
                }),
                has_fork_parent: shape.is_multiple_of(13),
                fork_child_count: u32::from(shape.is_multiple_of(17)) * 2,
                cost: Some(SessionCost {
                    total_usd: 0.25 + (shape % 100) as f64 / 10.0,
                    input_usd: 0.1,
                    output_usd: 0.1,
                    cache_read_usd: 0.025,
                    cache_write_usd: 0.025,
                }),
                models: vec![model.to_string()],
                model_runs: vec![ModelRun {
                    model: model.to_string(),
                    thinking_mode: shape.is_multiple_of(5).then(|| "high".to_string()),
                }],
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_rows_are_deterministic_full_shape_and_bounded() {
        let now = time::OffsetDateTime::from_unix_timestamp(1_788_192_000).unwrap();
        assert_eq!(
            serde_json::to_value(generate_sessions(225, 42, now).unwrap()).unwrap(),
            serde_json::to_value(generate_sessions(225, 42, now).unwrap()).unwrap()
        );
        let rows = generate_sessions(500, 42, now).unwrap();
        let oldest = time::OffsetDateTime::parse(
            &rows.last().unwrap().timestamp,
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        assert_eq!(rows.len(), 500);
        assert!(now - oldest < time::Duration::days(7));
        assert!(rows.iter().any(|row| row.wsl_distro.is_some()));
        assert!(rows.iter().any(|row| row.has_fork_parent));
        assert!(rows.iter().all(|row| row.cost.is_some()));
        assert!(generate_sessions(501, 0, now).is_err());
    }

    #[test]
    fn web_content_diagnostic_has_one_prefixed_json_payload() {
        let line = format!("{PREFIX}{}", web_content_event(7, 48122));
        assert_eq!(line.lines().count(), 1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line[PREFIX.len()..]).unwrap(),
            serde_json::json!({
                "event": "webcontent",
                "window": "popover",
                "generation": 7,
                "pid": 48122
            })
        );
    }
}

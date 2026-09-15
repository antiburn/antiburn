//! The HUD token map: tokens per minute by mode for every session that wrote
//! in the last few minutes.
//!
//! The HUD polls this on its liveness tick, so a poll must stay cheap. Each
//! session's mode samples are cached under the same fingerprint the popover
//! uses (`analysis::fingerprint_with_subagents`), and a transcript is
//! re-parsed only when that fingerprint changes.

use std::collections::HashMap;
use std::sync::Mutex;

use antiburn_local::analysis::{
    EventSource, ModeSample, RawSource, SessionInput, WorkMode, mode_samples, normalize_source,
};
use antiburn_local::discovery::Explorers;
use antiburn_local::discovery::SessionSource;
use antiburn_local::model::AgentKind;
use serde::Serialize;
use tauri::Manager;

use crate::agents::{kind_from_slug, vendor_label};
use crate::analysis;
use crate::scan;
use crate::store::Store;
use crate::store::model::SessionRecord;

/// The window the map sums over when the HUD passes none.
pub const DEFAULT_WINDOW_SECS: u32 = 300;
const MIN_WINDOW_SECS: u32 = 60;
const MAX_WINDOW_SECS: u32 = 3_600;
/// Sessions that wrote inside the window. A HUD cannot show more than this.
const MAX_SESSIONS: usize = 64;
/// Samples older than this leave the cache, so a long session stays bounded.
const KEEP_SECS: i64 = 3_600;

/// Tokens per mode over the window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HudModeTokens {
    pub looking: u64,
    pub running: u64,
    pub changing: u64,
    pub delegating: u64,
    pub thinking: u64,
    pub talking: u64,
    pub other: u64,
}

impl HudModeTokens {
    fn add(&mut self, mode: WorkMode, tokens: u64) {
        let slot = match mode {
            WorkMode::Looking => &mut self.looking,
            WorkMode::Running => &mut self.running,
            WorkMode::Changing => &mut self.changing,
            WorkMode::Delegating => &mut self.delegating,
            WorkMode::Thinking => &mut self.thinking,
            WorkMode::Talking => &mut self.talking,
            WorkMode::Other => &mut self.other,
        };
        *slot = slot.saturating_add(tokens);
    }

    fn total(&self) -> u64 {
        self.looking
            + self.running
            + self.changing
            + self.delegating
            + self.thinking
            + self.talking
            + self.other
    }
}

/// One sub-agent's share of the window.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HudTokenMapSubagent {
    pub subagent_id: String,
    pub tokens_per_min: f64,
    pub modes: HudModeTokens,
}

/// One session's share of the window. `modes` and `tokens_per_min` cover the
/// parent transcript only; sub-agents carry their own.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HudTokenMapSession {
    pub agent: String,
    pub session_id: String,
    pub title: Option<String>,
    pub last_turn_epoch: Option<i64>,
    pub tokens_per_min: f64,
    pub modes: HudModeTokens,
    pub subagents: Vec<HudTokenMapSubagent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HudTokenMapPayload {
    pub now_epoch: i64,
    pub window_secs: u32,
    /// Busiest first.
    pub sessions: Vec<HudTokenMapSession>,
}

struct CachedSamples {
    fingerprint: String,
    parent: Vec<ModeSample>,
    subagents: Vec<(String, Vec<ModeSample>)>,
}

static CACHE: Mutex<Option<HashMap<String, CachedSamples>>> = Mutex::new(None);

fn cache_key(record: &SessionRecord) -> String {
    format!(
        "{}|{}|{}",
        record.key.environment_key, record.key.agent, record.key.session_id
    )
}

fn cached_fingerprint(key: &str) -> Option<String> {
    let cache = CACHE.lock().ok()?;
    cache
        .as_ref()?
        .get(key)
        .map(|entry| entry.fingerprint.clone())
}

fn with_cached<T>(key: &str, read: impl FnOnce(&CachedSamples) -> T) -> Option<T> {
    let cache = CACHE.lock().ok()?;
    cache.as_ref()?.get(key).map(read)
}

fn store_cached(key: String, entry: CachedSamples) {
    if let Ok(mut cache) = CACHE.lock() {
        cache.get_or_insert_with(HashMap::new).insert(key, entry);
    }
}

/// Drop cache entries for sessions that left the recent list.
fn retain_cached(keys: &[String]) {
    if let Ok(mut cache) = CACHE.lock()
        && let Some(map) = cache.as_mut()
    {
        map.retain(|key, _| keys.contains(key));
    }
}

fn trim(samples: Vec<ModeSample>, keep_after_ms: i64) -> Vec<ModeSample> {
    samples
        .into_iter()
        .filter(|sample| sample.ts_ms.is_some_and(|ts| ts >= keep_after_ms))
        .collect()
}

/// Normalize the parent and its sub-agents and attribute every turn.
async fn parse_samples(
    kind: AgentKind,
    session_id: &str,
    wsl_distro: Option<&str>,
    keep_after_ms: i64,
) -> Option<(String, CachedSamples)> {
    let source = analysis::locate(kind, session_id, wsl_distro).await?;
    let raw = analysis::raw_source(kind, &source).await?;
    let fingerprint =
        analysis::fingerprint_with_subagents(kind, session_id, wsl_distro, &source).await;
    let label = vendor_label(kind).to_string();
    let parent_input = SessionInput {
        agent: label.clone(),
        session_id: session_id.to_string(),
        source: raw,
        source_format: analysis::source_format(kind, &source),
        fork_parent_session_id: None,
    };
    let mut subagent_paths = Explorers::DISK
        .list_subagents_in_environment(&kind, session_id, wsl_distro)
        .await;
    subagent_paths.sort();
    let subagent_inputs: Vec<SessionInput> = subagent_paths
        .into_iter()
        .filter_map(|path| {
            let subagent_id = Explorers::DISK.subagent_id(&kind, &path)?;
            let source_format = analysis::source_format(kind, &SessionSource::File(path.clone()));
            Some(SessionInput {
                agent: label.clone(),
                session_id: subagent_id,
                source: RawSource::File(path),
                source_format,
                fork_parent_session_id: None,
            })
        })
        .collect();

    // Normalizing is CPU-bound; keep it off the async runtime.
    let parsed = tauri::async_runtime::spawn_blocking(move || {
        let parent = normalize_source(&parent_input).ok()?;
        let parent_samples = trim(mode_samples(&parent.events), keep_after_ms);
        let subagents = subagent_inputs
            .iter()
            .filter_map(|input| {
                let session = normalize_source(input).ok()?;
                let mut samples = trim(mode_samples(&session.events), keep_after_ms);
                for sample in &mut samples {
                    sample.source = EventSource::Subagent;
                }
                Some((input.session_id.clone(), samples))
            })
            .collect();
        Some((parent_samples, subagents))
    })
    .await
    .ok()??;

    Some((
        fingerprint.clone(),
        CachedSamples {
            fingerprint,
            parent: parsed.0,
            subagents: parsed.1,
        },
    ))
}

fn sum_window(samples: &[ModeSample], since_ms: i64) -> (HudModeTokens, Option<i64>) {
    let mut modes = HudModeTokens::default();
    let mut last_ts = None;
    for sample in samples {
        let Some(ts) = sample.ts_ms else { continue };
        if ts < since_ms {
            continue;
        }
        modes.add(sample.mode, sample.tokens);
        last_ts = last_ts.max(Some(ts));
    }
    (modes, last_ts)
}

fn per_minute(tokens: u64, window_secs: u32) -> f64 {
    tokens as f64 * 60.0 / f64::from(window_secs)
}

/// Build one session's row from its cached samples. `None` when nothing in
/// the window carried tokens.
fn session_row(
    record: &SessionRecord,
    cached: &CachedSamples,
    since_ms: i64,
    window_secs: u32,
) -> Option<HudTokenMapSession> {
    let (modes, last_ts) = sum_window(&cached.parent, since_ms);
    let mut last_turn_ms = last_ts;
    let subagents: Vec<HudTokenMapSubagent> = cached
        .subagents
        .iter()
        .filter_map(|(subagent_id, samples)| {
            let (modes, last_ts) = sum_window(samples, since_ms);
            if modes.total() == 0 {
                return None;
            }
            last_turn_ms = last_turn_ms.max(last_ts);
            Some(HudTokenMapSubagent {
                subagent_id: subagent_id.clone(),
                tokens_per_min: per_minute(modes.total(), window_secs),
                modes,
            })
        })
        .collect();
    if modes.total() == 0 && subagents.is_empty() {
        return None;
    }
    Some(HudTokenMapSession {
        agent: record.key.agent.clone(),
        session_id: record.key.session_id.clone(),
        title: record.title.clone(),
        last_turn_epoch: last_turn_ms.map(|ms| ms / 1000),
        tokens_per_min: per_minute(modes.total(), window_secs),
        modes,
        subagents,
    })
}

fn row_rate(row: &HudTokenMapSession) -> f64 {
    row.tokens_per_min
        + row
            .subagents
            .iter()
            .map(|subagent| subagent.tokens_per_min)
            .sum::<f64>()
}

/// Tokens per minute by mode for every session that wrote in the window.
#[tauri::command]
pub async fn get_hud_token_map(
    app: tauri::AppHandle,
    window_secs: Option<u32>,
) -> Result<HudTokenMapPayload, String> {
    let window_secs = window_secs
        .unwrap_or(DEFAULT_WINDOW_SECS)
        .clamp(MIN_WINDOW_SECS, MAX_WINDOW_SECS);
    let now = scan::unix_now();
    let since = now - i64::from(window_secs);
    let since_ms = since * 1000;
    let keep_after_ms = (now - KEEP_SECS) * 1000;

    let records = {
        let store = app.state::<Store>();
        store
            .recent_sessions(since, MAX_SESSIONS)
            .map_err(|error| error.to_string())?
    };

    let keys: Vec<String> = records.iter().map(cache_key).collect();
    retain_cached(&keys);

    let mut sessions = Vec::with_capacity(records.len());
    for (record, key) in records.iter().zip(&keys) {
        let Some(kind) = kind_from_slug(&record.key.agent) else {
            continue;
        };
        let wsl_distro = record.wsl_distro.as_deref();
        let source = analysis::locate(kind, &record.key.session_id, wsl_distro).await;
        let fingerprint = match &source {
            Some(source) => {
                analysis::fingerprint_with_subagents(
                    kind,
                    &record.key.session_id,
                    wsl_distro,
                    source,
                )
                .await
            }
            None => continue,
        };
        if cached_fingerprint(key).as_deref() != Some(fingerprint.as_str()) {
            let Some((_, entry)) =
                parse_samples(kind, &record.key.session_id, wsl_distro, keep_after_ms).await
            else {
                continue;
            };
            store_cached(key.clone(), entry);
        }
        if let Some(Some(row)) = with_cached(key, |cached| {
            session_row(record, cached, since_ms, window_secs)
        }) {
            sessions.push(row);
        }
    }
    sessions.sort_by(|a, b| row_rate(b).total_cmp(&row_rate(a)));

    Ok(HudTokenMapPayload {
        now_epoch: now,
        window_secs,
        sessions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ts_ms: i64, mode: WorkMode, tokens: u64) -> ModeSample {
        ModeSample {
            ts_ms: Some(ts_ms),
            mode,
            tokens,
            source: EventSource::Parent,
        }
    }

    fn record() -> SessionRecord {
        SessionRecord {
            key: crate::store::model::SessionKey::for_session("claude-code", "s1", None),
            source_kind: "file".into(),
            source_label: "/tmp/s1.jsonl".into(),
            wsl_distro: None,
            title: Some("Fix HUD".into()),
            title_source: None,
            cwd: None,
            surface: "cli".into(),
            updated_at_epoch: Some(1_000),
            activity_cursor: String::new(),
            activity_source: "event".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: None,
        }
    }

    #[test]
    fn sums_only_samples_inside_the_window() {
        let cached = CachedSamples {
            fingerprint: "fp".into(),
            parent: vec![
                sample(50_000, WorkMode::Looking, 100),
                sample(150_000, WorkMode::Looking, 300),
                sample(200_000, WorkMode::Changing, 60),
            ],
            subagents: vec![(
                "sub-a".into(),
                vec![sample(180_000, WorkMode::Running, 120)],
            )],
        };
        let row = session_row(&record(), &cached, 100_000, 300).expect("row");
        assert_eq!(row.modes.looking, 300);
        assert_eq!(row.modes.changing, 60);
        assert_eq!(row.tokens_per_min, 72.0);
        assert_eq!(row.last_turn_epoch, Some(200));
        assert_eq!(row.subagents.len(), 1);
        assert_eq!(row.subagents[0].modes.running, 120);
        assert_eq!(row.subagents[0].tokens_per_min, 24.0);
        assert_eq!(row_rate(&row), 96.0);
    }

    #[test]
    fn a_quiet_session_yields_no_row() {
        let cached = CachedSamples {
            fingerprint: "fp".into(),
            parent: vec![sample(10_000, WorkMode::Talking, 5)],
            subagents: Vec::new(),
        };
        assert!(session_row(&record(), &cached, 100_000, 300).is_none());
    }

    #[test]
    fn trim_drops_old_and_untimed_samples() {
        let mut untimed = sample(0, WorkMode::Other, 1);
        untimed.ts_ms = None;
        let kept = trim(
            vec![
                sample(1, WorkMode::Other, 1),
                sample(9, WorkMode::Other, 2),
                untimed,
            ],
            5,
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].tokens, 2);
    }
}

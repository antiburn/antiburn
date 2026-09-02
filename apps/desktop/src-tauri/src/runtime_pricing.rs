//! Runtime model-pricing refresh and last-known-good persistence.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use antiburn_local::pricing::ModelPricing;
use anyhow::{Context, Result, anyhow, bail};
use reqwest::StatusCode;
use reqwest::header::{ETAG, IF_NONE_MATCH};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

const CACHE_FILE: &str = "model-pricing.json";
const CACHE_SCHEMA: u32 = 1;
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_MODELS: usize = 50_000;
const MAX_MODEL_ID_BYTES: usize = 512;
const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(15 * 60);
const REQUEST_COOLDOWN_SECS: u64 = 15 * 60;

// These provider IDs identify model creators, not price values.
const ORIGIN_PROVIDERS: &[&str] = &[
    "anthropic",
    "cohere",
    "deepseek",
    "google",
    "minimax",
    "mistral",
    "moonshotai",
    "openai",
    "qwen",
    "xai",
    "zai",
];

#[derive(Debug, Clone, Deserialize)]
struct ModelsDevProvider {
    #[serde(default)]
    models: BTreeMap<String, ModelsDevModel>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelsDevModel {
    #[serde(default)]
    last_updated: Option<String>,
    #[serde(default)]
    cost: Option<ModelsDevCost>,
    #[serde(default)]
    experimental: Option<ModelsDevExperimental>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelsDevExperimental {
    #[serde(default)]
    modes: BTreeMap<String, ModelsDevMode>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelsDevMode {
    #[serde(default)]
    cost: Option<ModelsDevCost>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelsDevCost {
    input: Option<f64>,
    output: Option<f64>,
    #[serde(default)]
    cache_read: Option<f64>,
    #[serde(default)]
    cache_write: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PricingSnapshot {
    schema: u32,
    source: String,
    version: String,
    fetched_at: String,
    etag: Option<String>,
    models: HashMap<String, ModelPricing>,
}

/// Shared pricing refresh state managed by Tauri.
pub struct PricingState {
    cache_path: PathBuf,
    version: RwLock<String>,
    etag: Mutex<Option<String>>,
    ready: AtomicBool,
    ready_notify: Notify,
    refresh_notify: Notify,
    last_request_epoch: AtomicU64,
}

impl PricingState {
    /// Load the last valid snapshot before background work starts.
    pub fn load(data_dir: &Path) -> Self {
        let state = Self {
            cache_path: data_dir.join(CACHE_FILE),
            version: RwLock::new("unavailable".to_string()),
            etag: Mutex::new(None),
            ready: AtomicBool::new(false),
            ready_notify: Notify::new(),
            refresh_notify: Notify::new(),
            last_request_epoch: AtomicU64::new(0),
        };

        match read_snapshot(&state.cache_path) {
            Ok(Some(snapshot)) => {
                antiburn_local::analysis::install_runtime_pricing(snapshot.models);
                *state
                    .version
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot.version;
                *state
                    .etag
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot.etag;
                state.mark_ready();
            }
            Ok(None) => {}
            Err(error) => {
                ::tracing::warn!(event = "pricing_cache_read_failed", error = %error);
            }
        }
        state
    }

    pub fn version(&self) -> String {
        self.version
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn etag(&self) -> Option<String> {
        self.etag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn mark_ready(&self) {
        if !self.ready.swap(true, Ordering::Release) {
            self.ready_notify.notify_waiters();
        }
    }

    fn request_refresh(&self) {
        let now = epoch_seconds();
        let previous = self.last_request_epoch.load(Ordering::Relaxed);
        if now.saturating_sub(previous) < REQUEST_COOLDOWN_SECS {
            return;
        }
        if self
            .last_request_epoch
            .compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            self.refresh_notify.notify_one();
        }
    }
}

/// Wait until a cache loads or the first network attempt completes.
pub async fn wait_until_ready(app: &AppHandle) {
    let state = app.state::<PricingState>();
    loop {
        let notified = state.ready_notify.notified();
        if state.ready.load(Ordering::Acquire) {
            return;
        }
        notified.await;
    }
}

/// Request an early refresh when the UI encounters an unpriced model.
pub fn request_refresh(app: &AppHandle) {
    app.state::<PricingState>().request_refresh();
}

/// Return the active catalog version shown in exports and About.
pub fn catalog_version(app: &AppHandle) -> String {
    app.state::<PricingState>().version()
}

/// Start the hourly refresh scheduler.
pub fn spawn_scheduler(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let success = match refresh(&app).await {
                Ok(invalidated) => {
                    if invalidated {
                        let _ = app.emit(crate::commands::SESSIONS_INVALIDATED_EVENT, ());
                    }
                    true
                }
                Err(error) => {
                    ::tracing::warn!(event = "pricing_refresh_failed", error = ?error);
                    false
                }
            };
            app.state::<PricingState>().mark_ready();

            let delay = if success {
                REFRESH_INTERVAL
            } else {
                RETRY_INTERVAL
            };
            let state = app.state::<PricingState>();
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = state.refresh_notify.notified() => {}
            }
        }
    })
}

async fn refresh(app: &AppHandle) -> Result<bool> {
    let state = app.state::<PricingState>();
    state
        .last_request_epoch
        .store(epoch_seconds(), Ordering::Relaxed);
    let etag = state.etag();
    let downloaded = tauri::async_runtime::spawn_blocking(move || download(etag))
        .await
        .context("pricing download task failed")??;
    let Some(snapshot) = downloaded else {
        ::tracing::debug!(event = "pricing_catalog_not_modified");
        return Ok(false);
    };

    validate_snapshot(&snapshot)?;
    let version_changed = app.state::<PricingState>().version() != snapshot.version;
    let changed = antiburn_local::analysis::install_runtime_pricing(snapshot.models.clone());
    if let Err(error) = antiburn_local::paths::state_files::write_json_atomic(
        &app.state::<PricingState>().cache_path,
        &snapshot,
    )
    .await
    {
        ::tracing::warn!(event = "pricing_cache_write_failed", error = %error);
    }

    let state = app.state::<PricingState>();
    *state
        .version
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot.version.clone();
    *state
        .etag
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot.etag.clone();
    ::tracing::info!(
        event = "pricing_catalog_refreshed",
        version = snapshot.version,
        models = snapshot.models.len(),
        changed
    );
    Ok(changed || version_changed)
}

fn download(etag: Option<String>) -> Result<Option<PricingSnapshot>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("antiburn/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build the pricing client")?;
    let mut request = client.get(crate::runtime_pricing_config::MODELS_DEV.url());
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let mut response = request.send().context("failed to download model pricing")?;
    if response.status() == StatusCode::NOT_MODIFIED {
        return Ok(None);
    }
    if !response.status().is_success() {
        bail!("models.dev returned {}", response.status());
    }

    let response_etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let mut payload = Vec::new();
    response
        .by_ref()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut payload)
        .context("failed to read the pricing response")?;
    if payload.len() as u64 > MAX_RESPONSE_BYTES {
        bail!("models.dev response exceeded the size limit");
    }

    let catalog: BTreeMap<String, ModelsDevProvider> =
        serde_json::from_slice(&payload).context("failed to parse models.dev pricing")?;
    Ok(Some(snapshot_from_catalog(catalog, response_etag)?))
}

fn snapshot_from_catalog(
    catalog: BTreeMap<String, ModelsDevProvider>,
    etag: Option<String>,
) -> Result<PricingSnapshot> {
    let mut models = HashMap::new();
    let mut bare_candidates: BTreeMap<String, Vec<(String, ModelPricing)>> = BTreeMap::new();
    let mut version = String::new();

    for (provider_id, provider) in catalog {
        for (model_id, model) in provider.models {
            if let Some(last_updated) = model.last_updated.as_deref()
                && last_updated > version.as_str()
            {
                version = last_updated.to_string();
            }
            if let Some(pricing) = model.cost.as_ref().and_then(convert_cost) {
                add_candidate(
                    &mut models,
                    &mut bare_candidates,
                    &provider_id,
                    &model_id,
                    pricing,
                );
            }
            if let Some(experimental) = model.experimental {
                for (mode, mode_data) in experimental.modes {
                    if let Some(pricing) = mode_data.cost.as_ref().and_then(convert_cost) {
                        add_candidate(
                            &mut models,
                            &mut bare_candidates,
                            &provider_id,
                            &format!("{model_id}-{mode}"),
                            pricing,
                        );
                    }
                }
            }
        }
    }

    for (model_id, candidates) in bare_candidates {
        if let Some(pricing) = select_bare_pricing(&candidates) {
            models.insert(model_id, pricing);
        }
    }

    if models.is_empty() {
        bail!("models.dev returned no usable prices");
    }
    if version.is_empty() {
        return Err(anyhow!("models.dev returned no catalog version"));
    }

    Ok(PricingSnapshot {
        schema: CACHE_SCHEMA,
        source: "models.dev".to_string(),
        version,
        fetched_at: antiburn_local::paths::state_files::now_rfc3339(),
        etag,
        models,
    })
}

/// Use a creator's rate or a multi-provider consensus for a bare model ID.
fn select_bare_pricing(candidates: &[(String, ModelPricing)]) -> Option<ModelPricing> {
    let origins = candidates
        .iter()
        .filter(|(provider, _)| ORIGIN_PROVIDERS.contains(&provider.as_str()))
        .map(|(_, pricing)| pricing)
        .collect::<Vec<_>>();
    if let Some(first) = origins.first() {
        return origins
            .iter()
            .all(|pricing| *pricing == *first)
            .then(|| (*first).clone());
    }
    if candidates.len() < 2 {
        return None;
    }
    let first = &candidates.first()?.1;
    candidates
        .iter()
        .all(|(_, pricing)| pricing == first)
        .then(|| first.clone())
}

fn add_candidate(
    models: &mut HashMap<String, ModelPricing>,
    bare_candidates: &mut BTreeMap<String, Vec<(String, ModelPricing)>>,
    provider_id: &str,
    model_id: &str,
    pricing: ModelPricing,
) {
    if model_id.len() > MAX_MODEL_ID_BYTES
        || provider_id.len() + model_id.len() + 1 > MAX_MODEL_ID_BYTES
    {
        return;
    }
    let provider_id = provider_id.to_lowercase();
    let model_id = model_id.to_lowercase();
    models.insert(format!("{provider_id}/{model_id}"), pricing.clone());
    models.insert(format!("{provider_id}.{model_id}"), pricing.clone());
    bare_candidates
        .entry(model_id)
        .or_default()
        .push((provider_id, pricing));
}

fn convert_cost(cost: &ModelsDevCost) -> Option<ModelPricing> {
    let input = cost.input?;
    let output = cost.output?;
    let values = [
        input,
        output,
        cost.cache_read.unwrap_or(input),
        cost.cache_write.unwrap_or(0.0),
    ];
    if values
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return None;
    }
    Some(ModelPricing {
        input_cost_per_token: values[0] / 1_000_000.0,
        output_cost_per_token: values[1] / 1_000_000.0,
        cache_read_cost_per_token: values[2] / 1_000_000.0,
        cache_write_cost_per_token: values[3] / 1_000_000.0,
    })
}

fn read_snapshot(path: &Path) -> Result<Option<PricingSnapshot>> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_RESPONSE_BYTES => {
            bail!("pricing cache exceeded the size limit");
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("failed to inspect the pricing cache"),
    }
    let payload = match std::fs::read(path) {
        Ok(payload) => payload,
        Err(error) => return Err(error).context("failed to read the pricing cache"),
    };
    let snapshot: PricingSnapshot =
        serde_json::from_slice(&payload).context("failed to parse the pricing cache")?;
    validate_snapshot(&snapshot)?;
    Ok(Some(snapshot))
}

fn validate_snapshot(snapshot: &PricingSnapshot) -> Result<()> {
    if snapshot.schema != CACHE_SCHEMA || snapshot.source != "models.dev" {
        bail!("unsupported pricing cache format");
    }
    if snapshot.models.is_empty()
        || snapshot.models.len() > MAX_MODELS
        || !valid_catalog_version(&snapshot.version)
    {
        bail!("pricing cache has no usable catalog");
    }
    let valid = snapshot.models.iter().all(|(model, pricing)| {
        model.len() <= MAX_MODEL_ID_BYTES
            && [
                pricing.input_cost_per_token,
                pricing.output_cost_per_token,
                pricing.cache_read_cost_per_token,
                pricing.cache_write_cost_per_token,
            ]
            .iter()
            .all(|value| value.is_finite() && *value >= 0.0)
    });
    if !valid {
        bail!("pricing cache contains an invalid price");
    }
    Ok(())
}

fn valid_catalog_version(version: &str) -> bool {
    let bytes = version.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost(input: f64, output: f64) -> ModelsDevCost {
        ModelsDevCost {
            input: Some(input),
            output: Some(output),
            cache_read: Some(input / 10.0),
            cache_write: None,
        }
    }

    #[test]
    fn origin_provider_wins_a_conflicting_bare_model_id() {
        let catalog = BTreeMap::from([
            (
                "anthropic".to_string(),
                ModelsDevProvider {
                    models: BTreeMap::from([(
                        "sample-model".to_string(),
                        ModelsDevModel {
                            last_updated: Some("2026-09-01".to_string()),
                            cost: Some(cost(10.0, 50.0)),
                            experimental: None,
                        },
                    )]),
                },
            ),
            (
                "reseller".to_string(),
                ModelsDevProvider {
                    models: BTreeMap::from([(
                        "sample-model".to_string(),
                        ModelsDevModel {
                            last_updated: Some("2026-08-01".to_string()),
                            cost: Some(cost(20.0, 60.0)),
                            experimental: None,
                        },
                    )]),
                },
            ),
        ]);
        let snapshot = snapshot_from_catalog(catalog, Some("tag".to_string())).unwrap();
        assert_eq!(snapshot.version, "2026-09-01");
        assert_eq!(snapshot.models["sample-model"].input_cost_per_token, 10e-6);
        assert_eq!(
            snapshot.models["reseller/sample-model"].input_cost_per_token,
            20e-6
        );
    }

    #[test]
    fn experimental_modes_become_model_suffixes() {
        let catalog = BTreeMap::from([(
            "openai".to_string(),
            ModelsDevProvider {
                models: BTreeMap::from([(
                    "sample-model".to_string(),
                    ModelsDevModel {
                        last_updated: Some("2026-09-01".to_string()),
                        cost: Some(cost(1.0, 2.0)),
                        experimental: Some(ModelsDevExperimental {
                            modes: BTreeMap::from([(
                                "fast".to_string(),
                                ModelsDevMode {
                                    cost: Some(cost(3.0, 4.0)),
                                },
                            )]),
                        }),
                    },
                )]),
            },
        )]);
        let snapshot = snapshot_from_catalog(catalog, None).unwrap();
        assert_eq!(
            snapshot.models["sample-model-fast"].input_cost_per_token,
            3e-6
        );
    }

    #[test]
    fn a_single_reseller_price_stays_provider_qualified() {
        let candidates = vec![(
            "reseller".to_string(),
            convert_cost(&cost(1.0, 2.0)).unwrap(),
        )];
        assert!(select_bare_pricing(&candidates).is_none());
    }

    #[test]
    fn invalid_or_incomplete_costs_are_rejected() {
        assert!(convert_cost(&cost(f64::NAN, 1.0)).is_none());
        assert!(
            convert_cost(&ModelsDevCost {
                input: Some(1.0),
                output: None,
                cache_read: None,
                cache_write: None,
            })
            .is_none()
        );
    }
}

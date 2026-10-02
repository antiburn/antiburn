//! Claude Code profiles: extra `CLAUDE_CONFIG_DIR` directories the reader
//! adds in Settings → Sources, each with the reader's own label.
//!
//! A reader can run one Claude Code directory per subscription, for example
//! `~/.claude` for a personal plan and `~/.claude-work` for a work plan. Each
//! directory has its own login, transcripts, and usage cache.
//!
//! The store keeps the list as one bounded JSON value. [`load`] and every
//! change copy it into a process-wide registry. Session discovery reads the
//! directories from the engine, and the Claude live usage source reads the
//! directories and labels from [`registered_profiles`] and [`default_label`].

use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use tauri::Manager;

use crate::commands::{CommandResult, fail, run_blocking};
use crate::scan::{ScanController, ScanTrigger};
use crate::store::Store;

/// The `setting` row that holds the profile list.
const STORE_KEY: &str = "internal:claudeProfiles";
/// The `setting` row that maps a provider account key to the profile label
/// it was last read under.
const ACCOUNT_LABELS_KEY: &str = "internal:accountLabels";
/// A bound on the remembered account labels.
const MAX_ACCOUNT_LABELS: usize = 64;
/// The id of the built-in profile, the CLI's default directory.
const DEFAULT_PROFILE_ID: &str = "default";
/// The built-in profile's label until the reader renames it.
const DEFAULT_LABEL: &str = "Claude";
/// Each added profile runs its own usage request. This bound limits them.
const MAX_PROFILES: usize = 16;
const MAX_LABEL_CHARS: usize = 80;
/// A bound on the home directory entries the suggestion pass reads.
const MAX_HOME_ENTRIES: usize = 4096;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredProfiles {
    #[serde(default)]
    default_label: Option<String>,
    #[serde(default)]
    profiles: Vec<StoredProfile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredProfile {
    id: String,
    label: String,
    path: String,
    #[serde(default)]
    added_at: String,
}

/// One Claude profile as Settings shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeProfileItem {
    pub id: String,
    pub label: String,
    pub path: String,
    /// The CLI's default directory. It can be renamed but not removed.
    pub built_in: bool,
}

/// A directory that looks like a Claude Code profile and is not added yet.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeProfileSuggestion {
    pub path: String,
    pub label: String,
}

/// What Settings shows: the profiles, then the directories it can add.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeProfilesPayload {
    pub profiles: Vec<ClaudeProfileItem>,
    pub suggestions: Vec<ClaudeProfileSuggestion>,
    pub max_profiles: usize,
    pub max_label_chars: usize,
}

/// An added profile that the live usage source reads.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredProfile {
    pub config_dir: PathBuf,
    pub label: String,
}

struct Registry {
    default_label: Option<String>,
    profiles: Vec<RegisteredProfile>,
}

static REGISTRY: RwLock<Registry> = RwLock::new(Registry {
    default_label: None,
    profiles: Vec::new(),
});

/// Serializes every read-modify-write of the profile list and the account
/// labels, so two quick edits or a live pass cannot drop a change.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn write_lock() -> std::sync::MutexGuard<'static, ()> {
    WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The reader's label for the built-in profile, when the reader set one.
pub fn default_label() -> Option<String> {
    REGISTRY
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .default_label
        .clone()
}

/// The built-in profile's name as Settings shows it.
pub fn default_display_label() -> String {
    default_label().unwrap_or_else(|| DEFAULT_LABEL.to_owned())
}

/// The name for the built-in login's readings and failures: its Settings
/// name once the reader added a profile or renamed it, `None` otherwise. A
/// reader with one Claude login keeps the provider's own naming.
pub fn default_reading_label() -> Option<String> {
    (default_label().is_some() || !registered_profiles().is_empty()).then(default_display_label)
}

fn account_label_key(provider: &str, account_key: &str) -> String {
    format!("{provider}:{account_key}")
}

/// Every remembered profile label, by provider and account key.
pub fn account_labels(store: &Store) -> std::collections::BTreeMap<String, String> {
    store
        .internal_value(ACCOUNT_LABELS_KEY)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// The remembered profile label for one provider account.
pub fn account_label(
    labels: &std::collections::BTreeMap<String, String>,
    provider: &str,
    account_key: &str,
) -> Option<String> {
    labels
        .get(&account_label_key(provider, account_key))
        .cloned()
}

/// Remember the label a provider account was read under, so views that
/// read stored history, such as the Limits page, can name the account. This
/// writes the store only when the label changes.
pub fn remember_account_label(store: &Store, provider: &str, account_key: &str, label: &str) {
    remember_account_label_in(store, provider, account_key, label, current_labels);
}

/// [`remember_account_label`] against a source of the names in use.
///
/// `current` runs under the write lock. A profile change also installs its
/// registry under that lock, so a read that started before the reader
/// removed or renamed its profile cannot bring the old name back.
fn remember_account_label_in(
    store: &Store,
    provider: &str,
    account_key: &str,
    label: &str,
    current: impl FnOnce() -> Vec<String>,
) {
    let _guard = write_lock();
    if !current().iter().any(|name| name == label) {
        return;
    }
    let mut labels = account_labels(store);
    let key = account_label_key(provider, account_key);
    if labels.get(&key).map(String::as_str) == Some(label) {
        return;
    }
    // A label names one login. After the reader signs a profile in to
    // another account, the older account loses the label.
    let prefix = format!("{provider}:");
    labels.retain(|existing, value| !(existing.starts_with(&prefix) && value == label));
    if labels.len() >= MAX_ACCOUNT_LABELS {
        return;
    }
    labels.insert(key, label.to_owned());
    if let Ok(raw) = serde_json::to_string(&labels) {
        store.set_internal_value(ACCOUNT_LABELS_KEY, &raw);
    }
}

/// Forget the labels that no profile uses any more, after a removal or a
/// rename.
fn forget_account_labels(store: &Store, provider: &str, unused: &[String]) {
    if unused.is_empty() {
        return;
    }
    let _guard = write_lock();
    let mut labels = account_labels(store);
    let prefix = format!("{provider}:");
    let before = labels.len();
    labels.retain(|key, label| !(key.starts_with(&prefix) && unused.contains(label)));
    if labels.len() != before
        && let Ok(raw) = serde_json::to_string(&labels)
    {
        store.set_internal_value(ACCOUNT_LABELS_KEY, &raw);
    }
}

/// Name the account each profile is signed in to now, from the account UUID
/// in that profile's `.claude.json`. This needs no network request, so an
/// account that is rate-limited or not yet read still gets its name.
///
/// Each added profile without a credentials file costs one Keychain
/// attribute read, so this runs only in the background.
fn remember_signed_in_account_labels(store: &Store) {
    use crate::provider_usage::live::sources::anthropic_fetch::{
        claude_json_account_uuid, claude_login_present,
    };
    let mut logins: Vec<(Option<String>, String)> = default_reading_label()
        .map(|label| (claude_json_account_uuid(None), label))
        .into_iter()
        .collect();
    logins.extend(
        registered_profiles()
            .into_iter()
            .filter(|profile| claude_login_present(&profile.config_dir))
            .map(|profile| {
                (
                    claude_json_account_uuid(Some(&profile.config_dir)),
                    profile.label,
                )
            }),
    );
    remember_login_labels(store, logins, current_labels);
}

/// Name each signed-in account UUID with its profile label.
fn remember_login_labels(
    store: &Store,
    logins: Vec<(Option<String>, String)>,
    current: impl Fn() -> Vec<String>,
) {
    let anthropic = crate::provider_usage::providers::ANTHROPIC;
    for (uuid, label) in logins {
        let Some(uuid) = uuid else {
            continue;
        };
        if let Ok(Some(account_key)) = crate::provider_accounts::opaque_key(store, anthropic, &uuid)
        {
            remember_account_label_in(store, anthropic, &account_key, &label, &current);
        }
    }
}

/// The added profiles, in the order the reader added them.
pub fn registered_profiles() -> Vec<RegisteredProfile> {
    REGISTRY
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .profiles
        .clone()
}

/// Install the stored list into the registry and the session engine. Call
/// once at startup, before the scan and live usage schedulers start.
pub fn load(store: &Store) {
    install(&read(store));
}

/// Name the signed-in accounts off the launch path: the pass reads each
/// profile's `.claude.json` and Keychain attributes.
pub fn refresh_account_labels_in_background(store: &Store) {
    let store = store.clone();
    tauri::async_runtime::spawn_blocking(move || remember_signed_in_account_labels(&store));
}

fn install(stored: &StoredProfiles) {
    let profiles: Vec<RegisteredProfile> = stored
        .profiles
        .iter()
        .map(|profile| RegisteredProfile {
            config_dir: PathBuf::from(&profile.path),
            label: profile.label.clone(),
        })
        .collect();
    antiburn_local::discovery::agents::claude::set_profile_config_dirs(
        profiles
            .iter()
            .map(|profile| profile.config_dir.clone())
            .collect(),
    );
    let mut registry = REGISTRY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    registry.default_label = stored.default_label.clone();
    registry.profiles = profiles;
}

fn read(store: &Store) -> StoredProfiles {
    store
        .internal_value(STORE_KEY)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write(store: &Store, stored: &StoredProfiles) -> anyhow::Result<()> {
    store.set_internal_value_checked(STORE_KEY, &serde_json::to_string(stored)?)
}

/// The directory the CLI uses without `CLAUDE_CONFIG_DIR`.
fn default_config_dir(home: &Path) -> PathBuf {
    home.join(".claude")
}

fn payload(stored: &StoredProfiles, home: Option<&Path>) -> ClaudeProfilesPayload {
    let mut profiles = vec![ClaudeProfileItem {
        id: DEFAULT_PROFILE_ID.to_owned(),
        label: stored
            .default_label
            .clone()
            .unwrap_or_else(|| DEFAULT_LABEL.to_owned()),
        path: home
            .map(|home| default_config_dir(home).to_string_lossy().to_string())
            .unwrap_or_default(),
        built_in: true,
    }];
    profiles.extend(stored.profiles.iter().map(|profile| ClaudeProfileItem {
        id: profile.id.clone(),
        label: profile.label.clone(),
        path: profile.path.clone(),
        built_in: false,
    }));
    ClaudeProfilesPayload {
        profiles,
        suggestions: home
            .map(|home| suggestions(stored, home))
            .unwrap_or_default(),
        max_profiles: MAX_PROFILES,
        max_label_chars: MAX_LABEL_CHARS,
    }
}

/// Directories under `home` named `.claude*` that hold a `projects`
/// directory and are not added yet. The `projects` test also excludes
/// `.claude.json` and its backups.
fn suggestions(stored: &StoredProfiles, home: &Path) -> Vec<ClaudeProfileSuggestion> {
    if stored.profiles.len() >= MAX_PROFILES {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(home) else {
        return Vec::new();
    };
    let default_dir = default_config_dir(home);
    let mut found: Vec<ClaudeProfileSuggestion> = entries
        .take(MAX_HOME_ENTRIES)
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            let rest = name.strip_prefix(".claude")?;
            let path = entry.path();
            if path == default_dir
                || !path.join("projects").is_dir()
                || stored
                    .profiles
                    .iter()
                    .any(|profile| Path::new(&profile.path) == path)
            {
                return None;
            }
            Some(ClaudeProfileSuggestion {
                path: path.to_string_lossy().to_string(),
                label: suggested_label(rest),
            })
        })
        .collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    // A suggestion never offers a name that a profile or an earlier
    // suggestion already uses.
    let mut taken: Vec<String> = stored_labels(stored);
    for suggestion in &mut found {
        let base = suggestion.label.clone();
        let mut number = 2;
        while label_in(&taken, &suggestion.label) {
            suggestion.label = format!("{base} {number}");
            number += 1;
        }
        taken.push(suggestion.label.clone());
    }
    found
}

/// Every label the registry uses now: the built-in profile's, then each
/// added profile's.
fn current_labels() -> Vec<String> {
    let mut labels = vec![default_display_label()];
    labels.extend(
        registered_profiles()
            .into_iter()
            .map(|profile| profile.label),
    );
    labels
}

/// Every label in use: the built-in profile's, then each added profile's.
fn stored_labels(stored: &StoredProfiles) -> Vec<String> {
    let mut labels = vec![
        stored
            .default_label
            .clone()
            .unwrap_or_else(|| DEFAULT_LABEL.to_owned()),
    ];
    labels.extend(stored.profiles.iter().map(|profile| profile.label.clone()));
    labels
}

/// Labels identify accounts in the views, so two profiles never share one,
/// whatever the case.
fn label_in(labels: &[String], label: &str) -> bool {
    labels
        .iter()
        .any(|existing| existing.to_lowercase() == label.to_lowercase())
}

/// `-work` becomes `Claude Work`; an empty or odd suffix keeps `Claude`.
fn suggested_label(suffix: &str) -> String {
    let words: Vec<String> = suffix
        .split(['-', '_', '.'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect();
    if words.is_empty() {
        DEFAULT_LABEL.to_owned()
    } else {
        format!("{DEFAULT_LABEL} {}", words.join(" "))
    }
}

fn normalized_label(label: &str) -> Result<String, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("Enter a name for this profile.".to_owned());
    }
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(format!("Use {MAX_LABEL_CHARS} characters or fewer."));
    }
    if label.chars().any(char::is_control) {
        return Err("Remove the control characters from the name.".to_owned());
    }
    Ok(label.to_owned())
}

fn normalized_path(
    path: &str,
    stored: &StoredProfiles,
    home: Option<&Path>,
) -> Result<String, String> {
    let trimmed = path.trim().trim_end_matches(['/', '\\']);
    let dir = Path::new(trimmed);
    if trimmed.is_empty() || !dir.is_absolute() {
        return Err("Choose a folder with an absolute path.".to_owned());
    }
    if !dir.is_dir() {
        return Err("This folder does not exist.".to_owned());
    }
    if home.is_some_and(|home| dir == home) {
        return Err("Choose a Claude Code folder, not your home folder.".to_owned());
    }
    // Discovery reads every transcript under the folder's `projects`, so the
    // folder must be a Claude Code configuration directory.
    if !(dir.join("projects").is_dir()
        || dir.join(".claude.json").is_file()
        || dir.join(".credentials.json").is_file())
    {
        return Err("This folder is not a Claude Code folder.".to_owned());
    }
    if home.is_some_and(|home| dir == default_config_dir(home)) {
        return Err("This folder is the built-in Claude profile.".to_owned());
    }
    if stored
        .profiles
        .iter()
        .any(|profile| Path::new(&profile.path) == dir)
    {
        return Err("This folder is already a profile.".to_owned());
    }
    Ok(trimmed.to_owned())
}

/// A stable id derived from the path, so the same folder keeps its id.
fn profile_id(path: &str) -> String {
    let digest = sha2::Sha256::digest(path.as_bytes());
    digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn add(
    stored: &mut StoredProfiles,
    label: &str,
    path: &str,
    home: Option<&Path>,
) -> Result<(), String> {
    if stored.profiles.len() >= MAX_PROFILES {
        return Err(format!("You can add up to {MAX_PROFILES} profiles."));
    }
    let label = normalized_label(label)?;
    if label_in(&stored_labels(stored), &label) {
        return Err(DUPLICATE_LABEL.to_owned());
    }
    let path = normalized_path(path, stored, home)?;
    stored.profiles.push(StoredProfile {
        id: profile_id(&path),
        label,
        path,
        added_at: antiburn_local::paths::state_files::now_rfc3339(),
    });
    Ok(())
}

const DUPLICATE_LABEL: &str = "Another profile already uses this name.";

fn rename(stored: &mut StoredProfiles, id: &str, label: &str) -> Result<(), String> {
    let label = normalized_label(label)?;
    let others: Vec<String> = stored_labels(stored)
        .into_iter()
        .zip(
            std::iter::once(DEFAULT_PROFILE_ID)
                .chain(stored.profiles.iter().map(|p| p.id.as_str())),
        )
        .filter(|(_, owner)| *owner != id)
        .map(|(label, _)| label)
        .collect();
    if label_in(&others, &label) {
        return Err(DUPLICATE_LABEL.to_owned());
    }
    if id == DEFAULT_PROFILE_ID {
        stored.default_label = (label != DEFAULT_LABEL).then_some(label);
        return Ok(());
    }
    let profile = stored
        .profiles
        .iter_mut()
        .find(|profile| profile.id == id)
        .ok_or_else(|| "This profile no longer exists.".to_owned())?;
    profile.label = label;
    Ok(())
}

fn remove(stored: &mut StoredProfiles, id: &str) -> Result<(), String> {
    if id == DEFAULT_PROFILE_ID {
        return Err("The built-in Claude profile cannot be removed.".to_owned());
    }
    let before = stored.profiles.len();
    stored.profiles.retain(|profile| profile.id != id);
    if stored.profiles.len() == before {
        return Err("This profile no longer exists.".to_owned());
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Change {
    Added,
    Renamed,
    Removed,
}

/// What one applied change produced.
#[derive(Debug)]
struct Applied {
    payload: ClaudeProfilesPayload,
    /// The added-profile count, for the analytics bucket.
    count: usize,
    /// False when the change left the list as it was, such as a rename to
    /// the same name. The caller then persists and records nothing.
    changed: bool,
}

/// Apply one change to the stored list, persist it, and install it. A
/// rejected change or a failed write returns the error and leaves the
/// stored list and the registry as they were.
fn apply_change(
    store: &Store,
    home: Option<&Path>,
    apply: impl FnOnce(&mut StoredProfiles, Option<&Path>) -> Result<(), String>,
) -> Result<Applied, String> {
    let (stored, unused, changed) = {
        let _guard = write_lock();
        let mut stored = read(store);
        let original = stored.clone();
        let before = stored_labels(&stored);
        apply(&mut stored, home)?;
        if stored == original {
            (stored, Vec::new(), false)
        } else {
            write(store, &stored).map_err(fail)?;
            install(&stored);
            let after = stored_labels(&stored);
            let unused: Vec<String> = before
                .into_iter()
                .filter(|label| !after.contains(label))
                .collect();
            (stored, unused, true)
        }
    };
    forget_account_labels(store, crate::provider_usage::providers::ANTHROPIC, &unused);
    Ok(Applied {
        payload: payload(&stored, home),
        count: stored.profiles.len(),
        changed,
    })
}

/// The analytics label for a change.
fn change_event(kind: Change) -> crate::analytics::event::ClaudeProfileChange {
    match kind {
        Change::Added => crate::analytics::event::ClaudeProfileChange::Added,
        Change::Renamed => crate::analytics::event::ClaudeProfileChange::Renamed,
        Change::Removed => crate::analytics::event::ClaudeProfileChange::Removed,
    }
}

/// Apply one change, then record it and refresh the views that use it.
async fn change(
    app: &tauri::AppHandle,
    kind: Change,
    apply: impl FnOnce(&mut StoredProfiles, Option<&Path>) -> Result<(), String> + Send + 'static,
) -> CommandResult<ClaudeProfilesPayload> {
    let store = app.state::<Store>().inner().clone();
    let label_store = store.clone();
    let applied = run_blocking(move || {
        let home = antiburn_local::paths::home_dir();
        apply_change(&store, home.as_deref(), apply)
    })
    .await?;
    // A rename to the same name changes nothing. Record nothing for it.
    if !applied.changed {
        return Ok(applied.payload);
    }
    refresh_account_labels_in_background(&label_store);
    crate::analytics::record_claude_profile_changed(app, change_event(kind), applied.count);
    // Labels and the profile set feed the live usage names, so every open
    // view gets a new summary now instead of at the next poll.
    crate::usage_alerts::republish_after_settings_change(app);
    if !matches!(kind, Change::Renamed) {
        app.state::<ScanController>()
            .request(ScanTrigger::ClaudeProfilesChanged);
    }
    Ok(applied.payload)
}

/// The Claude profiles and the directories Settings can suggest.
#[tauri::command]
pub async fn list_claude_profiles(app: tauri::AppHandle) -> CommandResult<ClaudeProfilesPayload> {
    let store = app.state::<Store>().inner().clone();
    run_blocking(move || {
        let home = antiburn_local::paths::home_dir();
        Ok(payload(&read(&store), home.as_deref()))
    })
    .await
}

/// Add a Claude Code directory as a profile with the reader's label.
#[tauri::command]
pub async fn add_claude_profile(
    app: tauri::AppHandle,
    label: String,
    path: String,
) -> CommandResult<ClaudeProfilesPayload> {
    change(&app, Change::Added, move |stored, home| {
        add(stored, &label, &path, home)
    })
    .await
}

/// Rename a profile. The built-in profile can be renamed too.
#[tauri::command]
pub async fn rename_claude_profile(
    app: tauri::AppHandle,
    id: String,
    label: String,
) -> CommandResult<ClaudeProfilesPayload> {
    change(&app, Change::Renamed, move |stored, _| {
        rename(stored, &id, &label)
    })
    .await
}

/// Remove an added profile. Its sessions already in the index stay there.
#[tauri::command]
pub async fn remove_claude_profile(
    app: tauri::AppHandle,
    id: String,
) -> CommandResult<ClaudeProfilesPayload> {
    change(&app, Change::Removed, move |stored, _| remove(stored, &id)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn in_use() -> Vec<String> {
        ["Claude", "Claude Work", "Claude Api"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn profile_dir(home: &Path, name: &str) -> PathBuf {
        let dir = home.join(name);
        std::fs::create_dir_all(dir.join("projects")).unwrap();
        dir
    }

    #[test]
    fn suggested_labels_name_the_suffix() {
        assert_eq!(suggested_label("-work"), "Claude Work");
        assert_eq!(suggested_label("-side_project"), "Claude Side Project");
        assert_eq!(suggested_label(""), "Claude");
    }

    #[test]
    fn suggestions_list_profile_dirs_that_are_not_added() {
        let home = TempDir::new().unwrap();
        profile_dir(home.path(), ".claude");
        let work = profile_dir(home.path(), ".claude-work");
        let added = profile_dir(home.path(), ".claude-added");
        std::fs::create_dir_all(home.path().join(".claude-empty")).unwrap();
        std::fs::write(home.path().join(".claude.json"), "{}").unwrap();
        let stored = StoredProfiles {
            default_label: None,
            profiles: vec![StoredProfile {
                id: "x".into(),
                label: "Added".into(),
                path: added.to_string_lossy().to_string(),
                added_at: String::new(),
            }],
        };

        let found = suggestions(&stored, home.path());

        assert_eq!(
            found,
            vec![ClaudeProfileSuggestion {
                path: work.to_string_lossy().to_string(),
                label: "Claude Work".into(),
            }]
        );
    }

    #[test]
    fn add_rejects_the_default_dir_duplicates_and_missing_folders() {
        let home = TempDir::new().unwrap();
        let default_dir = profile_dir(home.path(), ".claude");
        let work = profile_dir(home.path(), ".claude-work");
        let work_path = work.to_string_lossy().to_string();
        let mut stored = StoredProfiles::default();

        assert!(
            add(
                &mut stored,
                "Default",
                &default_dir.to_string_lossy(),
                Some(home.path())
            )
            .is_err()
        );
        assert!(
            add(
                &mut stored,
                "Gone",
                &home.path().join("missing").to_string_lossy(),
                Some(home.path())
            )
            .is_err()
        );
        assert!(add(&mut stored, "Relative", ".claude-work", Some(home.path())).is_err());
        add(
            &mut stored,
            "  Work  ",
            &format!("{work_path}/"),
            Some(home.path()),
        )
        .unwrap();
        assert!(add(&mut stored, "Again", &work_path, Some(home.path())).is_err());

        assert_eq!(stored.profiles.len(), 1);
        assert_eq!(stored.profiles[0].label, "Work");
        assert_eq!(stored.profiles[0].path, work_path);
        assert_eq!(stored.profiles[0].id, profile_id(&work_path));
    }

    #[test]
    fn add_stops_at_the_profile_limit() {
        let home = TempDir::new().unwrap();
        let mut stored = StoredProfiles::default();
        for index in 0..MAX_PROFILES {
            let dir = profile_dir(home.path(), &format!(".claude-{index}"));
            add(
                &mut stored,
                &format!("P{index}"),
                &dir.to_string_lossy(),
                Some(home.path()),
            )
            .unwrap();
        }
        let extra = profile_dir(home.path(), ".claude-extra");

        assert!(
            add(
                &mut stored,
                "Extra",
                &extra.to_string_lossy(),
                Some(home.path())
            )
            .is_err()
        );
        assert!(suggestions(&stored, home.path()).is_empty());
    }

    #[test]
    fn labels_are_trimmed_bounded_and_printable() {
        assert_eq!(normalized_label("  Work ").unwrap(), "Work");
        assert!(normalized_label("   ").is_err());
        assert!(normalized_label(&"a".repeat(MAX_LABEL_CHARS + 1)).is_err());
        assert!(normalized_label("Wo\nrk").is_err());
    }

    #[test]
    fn the_built_in_profile_renames_but_never_leaves() {
        let mut stored = StoredProfiles::default();

        rename(&mut stored, DEFAULT_PROFILE_ID, "Personal").unwrap();
        assert_eq!(stored.default_label.as_deref(), Some("Personal"));
        rename(&mut stored, DEFAULT_PROFILE_ID, DEFAULT_LABEL).unwrap();
        assert_eq!(stored.default_label, None);
        assert!(remove(&mut stored, DEFAULT_PROFILE_ID).is_err());
    }

    #[test]
    fn payload_lists_the_built_in_profile_first() {
        let home = TempDir::new().unwrap();
        let stored = StoredProfiles {
            default_label: Some("Personal".into()),
            profiles: vec![StoredProfile {
                id: "abc".into(),
                label: "Work".into(),
                path: "/work/.claude".into(),
                added_at: String::new(),
            }],
        };

        let payload = payload(&stored, Some(home.path()));

        assert_eq!(payload.profiles.len(), 2);
        assert!(payload.profiles[0].built_in);
        assert_eq!(payload.profiles[0].label, "Personal");
        assert_eq!(
            payload.profiles[0].path,
            home.path().join(".claude").to_string_lossy()
        );
        assert_eq!(payload.profiles[1].id, "abc");
        assert!(!payload.profiles[1].built_in);
    }

    #[test]
    fn an_account_label_moves_to_the_newest_account() {
        let state = TempDir::new().unwrap();
        let store = Store::open_in_memory(state.path()).unwrap();

        remember_account_label_in(&store, "anthropic", "old", "Claude Work", in_use);
        remember_account_label_in(&store, "anthropic", "personal", "Claude", in_use);
        remember_account_label_in(&store, "anthropic", "new", "Claude Work", in_use);
        let labels = account_labels(&store);

        assert_eq!(account_label(&labels, "anthropic", "old"), None);
        assert_eq!(
            account_label(&labels, "anthropic", "new").as_deref(),
            Some("Claude Work")
        );
        assert_eq!(
            account_label(&labels, "anthropic", "personal").as_deref(),
            Some("Claude")
        );
    }

    #[test]
    fn signed_in_logins_name_their_account_without_a_reading() {
        let state = TempDir::new().unwrap();
        let store = Store::open_in_memory(state.path()).unwrap();

        remember_login_labels(
            &store,
            vec![
                (Some("personal-uuid".into()), "Claude".into()),
                (None, "Claude Api".into()),
                (Some("work-uuid".into()), "Claude Work".into()),
            ],
            in_use,
        );

        let labels = account_labels(&store);
        let key = |uuid: &str| {
            crate::provider_accounts::opaque_key(&store, "anthropic", uuid)
                .unwrap()
                .unwrap()
        };
        assert_eq!(labels.len(), 2);
        assert_eq!(
            account_label(&labels, "anthropic", &key("work-uuid")).as_deref(),
            Some("Claude Work")
        );
        assert_eq!(
            account_label(&labels, "anthropic", &key("personal-uuid")).as_deref(),
            Some("Claude")
        );
    }

    #[test]
    fn removed_or_renamed_labels_are_forgotten() {
        let state = TempDir::new().unwrap();
        let store = Store::open_in_memory(state.path()).unwrap();
        remember_account_label_in(&store, "anthropic", "work", "Claude Work", in_use);
        remember_account_label_in(&store, "anthropic", "personal", "Claude", in_use);

        forget_account_labels(&store, "anthropic", &["Claude Work".into()]);

        let labels = account_labels(&store);
        assert_eq!(account_label(&labels, "anthropic", "work"), None);
        assert_eq!(
            account_label(&labels, "anthropic", "personal").as_deref(),
            Some("Claude")
        );
    }

    #[test]
    fn labels_stay_unique_across_profiles_and_the_built_in_one() {
        let home = TempDir::new().unwrap();
        let work = profile_dir(home.path(), ".claude-work");
        let side = profile_dir(home.path(), ".claude-side");
        let mut stored = StoredProfiles::default();

        assert_eq!(
            add(
                &mut stored,
                "claude",
                &work.to_string_lossy(),
                Some(home.path())
            ),
            Err(DUPLICATE_LABEL.to_owned())
        );
        add(
            &mut stored,
            "Work",
            &work.to_string_lossy(),
            Some(home.path()),
        )
        .unwrap();
        assert_eq!(
            add(
                &mut stored,
                " WORK ",
                &side.to_string_lossy(),
                Some(home.path())
            ),
            Err(DUPLICATE_LABEL.to_owned())
        );
        let work_id = stored.profiles[0].id.clone();
        assert_eq!(
            rename(&mut stored, DEFAULT_PROFILE_ID, "work"),
            Err(DUPLICATE_LABEL.to_owned())
        );
        rename(&mut stored, &work_id, "work").unwrap();
        assert_eq!(stored.profiles[0].label, "work");
    }

    #[test]
    fn suggestions_never_repeat_a_name_in_use() {
        let home = TempDir::new().unwrap();
        profile_dir(home.path(), ".claude-");
        profile_dir(home.path(), ".claude_");

        let names: Vec<String> = suggestions(&StoredProfiles::default(), home.path())
            .into_iter()
            .map(|suggestion| suggestion.label)
            .collect();

        assert_eq!(names, ["Claude 2", "Claude 3"]);
    }

    #[test]
    fn a_change_reports_success_rejection_no_op_and_write_failure() {
        let state = TempDir::new().unwrap();
        let store = Store::open_in_memory(state.path()).unwrap();
        let home = TempDir::new().unwrap();
        let work = profile_dir(home.path(), ".claude-work");
        let work_path = work.to_string_lossy().to_string();

        let added = apply_change(&store, Some(home.path()), |stored, home| {
            add(stored, "Work", &work_path, home)
        })
        .unwrap();
        assert!(added.changed);
        assert_eq!(added.count, 1);
        assert_eq!(read(&store).profiles.len(), 1);

        let duplicate = apply_change(&store, Some(home.path()), |stored, home| {
            add(stored, "work", &work_path, home)
        });
        assert_eq!(duplicate.unwrap_err(), DUPLICATE_LABEL);

        let id = read(&store).profiles[0].id.clone();
        let same = apply_change(&store, Some(home.path()), |stored, _| {
            rename(stored, &id, "Work")
        })
        .unwrap();
        assert!(!same.changed);

        store
            .lock()
            .execute_batch(
                "CREATE TRIGGER refuse_profile_update BEFORE UPDATE ON setting
                   WHEN NEW.key = 'internal:claudeProfiles'
                   BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END;",
            )
            .unwrap();
        let failed = apply_change(&store, Some(home.path()), |stored, _| {
            rename(stored, &id, "Office")
        });
        assert!(failed.unwrap_err().contains("synthetic write failure"));
        assert_eq!(read(&store).profiles[0].label, "Work");
        assert_eq!(registered_profiles()[0].label, "Work");

        install(&StoredProfiles::default());
    }

    #[test]
    fn each_change_maps_to_its_event_label() {
        use crate::analytics::event::ClaudeProfileChange;
        assert_eq!(change_event(Change::Added), ClaudeProfileChange::Added);
        assert_eq!(change_event(Change::Renamed), ClaudeProfileChange::Renamed);
        assert_eq!(change_event(Change::Removed), ClaudeProfileChange::Removed);
    }

    #[test]
    fn a_stale_read_never_restores_a_removed_name() {
        let state = TempDir::new().unwrap();
        let store = Store::open_in_memory(state.path()).unwrap();

        remember_account_label_in(&store, "anthropic", "gone", "Claude Gone", in_use);

        assert_eq!(
            account_label(&account_labels(&store), "anthropic", "gone"),
            None
        );
    }

    #[test]
    fn only_claude_code_folders_can_become_profiles() {
        let home = TempDir::new().unwrap();
        let plain = home.path().join("notes");
        std::fs::create_dir_all(&plain).unwrap();
        let mut stored = StoredProfiles::default();

        assert!(
            add(
                &mut stored,
                "Notes",
                &plain.to_string_lossy(),
                Some(home.path())
            )
            .is_err()
        );
        assert!(
            add(
                &mut stored,
                "Home",
                &home.path().to_string_lossy(),
                Some(home.path())
            )
            .is_err()
        );
        let logged_in = home.path().join(".claude-login");
        std::fs::create_dir_all(&logged_in).unwrap();
        std::fs::write(logged_in.join(".claude.json"), "{}").unwrap();
        assert!(
            add(
                &mut stored,
                "Login",
                &logged_in.to_string_lossy(),
                Some(home.path())
            )
            .is_ok()
        );
    }

    #[test]
    fn stored_profiles_survive_a_round_trip_and_tolerate_bad_json() {
        let state = TempDir::new().unwrap();
        let store = Store::open_in_memory(state.path()).unwrap();
        assert_eq!(read(&store), StoredProfiles::default());
        let stored = StoredProfiles {
            default_label: Some("Personal".into()),
            profiles: vec![StoredProfile {
                id: "abc".into(),
                label: "Work".into(),
                path: "/work/.claude".into(),
                added_at: "2026-10-02T00:00:00Z".into(),
            }],
        };
        write(&store, &stored).unwrap();
        assert_eq!(read(&store), stored);

        store.set_internal_value(STORE_KEY, "not json");
        assert_eq!(read(&store), StoredProfiles::default());
    }
}

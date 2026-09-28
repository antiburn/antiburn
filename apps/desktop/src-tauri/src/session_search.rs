//! Tauri command for local session metadata search.

use serde::Serialize;
use tauri::Manager;

use crate::store::{Store, iso_from_epoch};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchEntry {
    pub environment_key: String,
    pub agent: String,
    pub session_id: String,
    pub wsl_distro: Option<String>,
    pub title: Option<String>,
    pub repository: String,
    pub cwd_label: String,
    pub models: Vec<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchResponse {
    pub results: Vec<SessionSearchEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub indexing: bool,
}

#[tauri::command]
pub async fn search_sessions(
    app: tauri::AppHandle,
    query: String,
    cursor: Option<String>,
    scope: Option<crate::session_search_scope::SessionSearchScope>,
) -> Result<SessionSearchResponse, String> {
    if let Some(scope) = &scope {
        scope.validate().map_err(|e| e.to_string())?;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let page = app
            .state::<Store>()
            .search_sessions(&query, cursor.as_deref(), scope.as_ref())
            .map_err(|error| error.to_string())?;
        Ok(SessionSearchResponse {
            results: page
                .results
                .into_iter()
                .map(|result| SessionSearchEntry {
                    environment_key: result.environment_key,
                    agent: result.agent,
                    session_id: result.session_id,
                    wsl_distro: result.wsl_distro,
                    title: result.title,
                    repository: result.repository,
                    cwd_label: result.cwd_label,
                    models: result.models,
                    timestamp: iso_from_epoch(result.updated_at_epoch),
                })
                .collect(),
            next_cursor: page.next_cursor,
            has_more: page.has_more,
            indexing: page.indexing,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

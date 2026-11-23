//! Search authorized Cloud metadata and merge only proven local matches.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::{
    cli::SearchMode,
    config::Config,
    search::{SearchSession, lookup_with_receipts},
    sessions::{SearchResponse, SearchSources, SessionMatch, SourceState},
};

use super::{CloudClient, Page, read_json};

const MAX_SEARCH_BYTES: usize = 512 * 1024;
const MAX_LIMIT: u16 = 50;
const MAX_QUERY_BYTES: usize = 256;

#[derive(Deserialize, Serialize)]
struct Agent {
    #[serde(deserialize_with = "Deserialize::deserialize")]
    name: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    version: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct CloudSession {
    pub id: Uuid,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub title: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    agent: Option<Agent>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    started_at: Option<String>,
    #[serde(skip_serializing, deserialize_with = "Deserialize::deserialize")]
    source_key: Option<String>,
}

impl CloudSession {
    /// Cloud document reads return an ID but no search metadata.
    pub(crate) fn from_id(id: Uuid) -> Self {
        Self {
            id,
            title: None,
            agent: None,
            started_at: None,
            source_key: None,
        }
    }
}

#[derive(Deserialize)]
struct SearchPage {
    workspace_id: Option<Uuid>,
    items: Vec<CloudSession>,
    page: Page,
}

fn latest_known_time(session: &SessionMatch) -> Option<i128> {
    let local = session
        .local
        .as_ref()
        .and_then(|row| row.updated_at_epoch)
        .map(|seconds| i128::from(seconds) * 1_000_000_000);
    let cloud = session
        .cloud
        .as_ref()
        .and_then(|row| row.started_at.as_deref())
        .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
        .map(OffsetDateTime::unix_timestamp_nanos);
    local.max(cloud)
}

/// Query Cloud first so denied Cloud access cannot be replaced with a local result.
pub(crate) async fn search(
    query: String,
    limit: Option<u16>,
    mode: SearchMode,
    config: Config,
    client: CloudClient,
) -> Result<SearchResponse, String> {
    let limit = limit.unwrap_or(config.search_limit.min(MAX_LIMIT));
    if limit > MAX_LIMIT || query.len() > MAX_QUERY_BYTES {
        return Err("Cloud search allows 1 to 50 results and queries up to 256 bytes".into());
    }
    let page: SearchPage = read_json(
        client
            .sessions(None)
            .query(&[("q", query.as_str())])
            .query(&[("page[size]", limit)]),
        MAX_SEARCH_BYTES,
    )
    .await?;
    validate_page(&page, client.workspace_id, usize::from(limit))?;
    let cloud_truncated = page.page.next_cursor.is_some();
    let mut sessions: Vec<_> = page
        .items
        .into_iter()
        .map(|row| SessionMatch {
            local: None,
            cloud: Some(row),
        })
        .collect();

    let local = if matches!(mode, SearchMode::All) {
        let (result, keys) = lookup_with_receipts(
            query,
            limit,
            config,
            client.workspace_id.to_string(),
            client.origin.origin().ascii_serialization(),
        )
        .await?;
        merge(&mut sessions, result.sessions, keys);
        sessions.sort_by_cached_key(|row| {
            std::cmp::Reverse((row.local.is_some(), latest_known_time(row)))
        });
        Some(SourceState {
            truncated: result.truncated,
        })
    } else {
        None
    };
    let truncated = cloud_truncated
        || local.as_ref().is_some_and(|state| state.truncated)
        || sessions.len() > usize::from(limit);
    sessions.truncate(usize::from(limit));
    Ok(SearchResponse {
        source: if local.is_some() { "all" } else { "cloud" },
        sources: SearchSources {
            local,
            cloud: Some(SourceState {
                truncated: cloud_truncated,
            }),
        },
        workspace_id: Some(client.workspace_id),
        sessions,
        truncated,
    })
}

fn validate_page(page: &SearchPage, workspace_id: Uuid, limit: usize) -> Result<(), String> {
    let mut ids = HashSet::new();
    let mut keys = HashSet::new();
    if page.workspace_id.is_some_and(|id| id != workspace_id)
        || page.items.len() > limit
        || page.items.iter().any(|row| {
            row.id.is_nil()
                || !ids.insert(row.id)
                || row.agent.is_none()
                || row
                    .source_key
                    .as_ref()
                    .is_some_and(|key| !valid_source_key(key) || !keys.insert(key))
                || row
                    .started_at
                    .as_ref()
                    .is_some_and(|value| OffsetDateTime::parse(value, &Rfc3339).is_err())
        })
    {
        return Err("Cloud search response does not match the request".into());
    }
    Ok(())
}

fn valid_source_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn merge(sessions: &mut Vec<SessionMatch>, local: Vec<SearchSession>, keys: Vec<Option<String>>) {
    let cloud: HashMap<_, _> = sessions
        .iter()
        .enumerate()
        .filter_map(|(index, row)| Some((row.cloud.as_ref()?.source_key.as_ref()?.clone(), index)))
        .collect();
    let mut seen = HashSet::new();
    let ambiguous: HashSet<_> = keys
        .iter()
        .flatten()
        .filter(|&key| !seen.insert(key))
        .collect();
    for (row, key) in local.into_iter().zip(&keys) {
        if let Some(key) = key.as_ref().filter(|key| !ambiguous.contains(key))
            && let Some(&index) = cloud.get(key)
            && let Some(agent) = sessions[index]
                .cloud
                .as_ref()
                .and_then(|remote| remote.agent.as_ref())
            && agent.name.as_deref() == Some(row.identity.agent.as_str())
        {
            sessions[index].local = Some(row);
        } else {
            sessions.push(SessionMatch {
                local: Some(row),
                cloud: None,
            });
        }
    }
}

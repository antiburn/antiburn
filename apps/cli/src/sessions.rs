//! Run session commands and format the same result for terminals and scripts.

use std::fmt::Write;

use serde::Serialize;

#[cfg(feature = "cloud")]
use crate::cloud::{self, CloudSession};
use crate::{
    cli::{GetArgs, GetSource, SearchArgs, SearchMode},
    config::Config,
    document::SessionDocument,
    search::{self, SearchSession},
};

/// A result carries each source identity separately. Only a confirmed match has both.
#[derive(Serialize)]
pub(crate) struct SessionMatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<SearchSession>,
    #[cfg(feature = "cloud")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud: Option<CloudSession>,
}

impl SessionMatch {
    fn local(row: SearchSession) -> Self {
        Self {
            local: Some(row),
            #[cfg(feature = "cloud")]
            cloud: None,
        }
    }

    fn write_summary(&self, text: &mut String) {
        let local_title = self.local.as_ref().and_then(|row| row.title.as_deref());
        #[cfg(feature = "cloud")]
        let local_title = local_title.or_else(|| self.cloud.as_ref()?.title.as_deref());
        let _ = writeln!(
            text,
            "{}",
            safe_text(local_title.unwrap_or("Untitled session"))
        );
        if let Some(local) = &self.local {
            let id = &local.identity;
            let _ = writeln!(
                text,
                "  Local: {} · {} · {}",
                safe_text(&id.session_id),
                safe_text(&id.agent),
                safe_text(&id.environment_key),
            );
        }
        #[cfg(feature = "cloud")]
        if let Some(cloud) = &self.cloud {
            let _ = writeln!(text, "  Cloud: {}", cloud.id);
        }
    }
}

/// Completeness of each requested source before merging and applying the result limit.
#[derive(Serialize)]
pub(crate) struct SourceState {
    pub truncated: bool,
}

#[derive(Serialize)]
pub(crate) struct SearchSources {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<SourceState>,
    #[cfg(feature = "cloud")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud: Option<SourceState>,
}

/// The search response uses the same session entry shape for every source.
#[derive(Serialize)]
pub(crate) struct SearchResponse {
    pub source: &'static str,
    pub sources: SearchSources,
    #[cfg(feature = "cloud")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<uuid::Uuid>,
    pub sessions: Vec<SessionMatch>,
    pub truncated: bool,
}

/// Keep the requested display mode with the typed session result.
pub struct SessionOutput {
    json: bool,
    result: SessionResult,
}

enum SessionResult {
    Search(SearchResponse),
    Get(Box<SessionDocument>),
}

impl SessionOutput {
    pub(crate) async fn write(
        self,
        writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    ) -> Result<(), String> {
        if self.json {
            return match self.result {
                SessionResult::Search(response) => super::write_response(writer, &response).await,
                SessionResult::Get(response) => {
                    super::write_output(writer, &response.to_json()?).await
                }
            };
        }
        let mut text = String::new();
        match self.result {
            SessionResult::Search(response) => {
                let count = response.sessions.len();
                let _ = writeln!(
                    text,
                    "{count} {} ({})",
                    if count == 1 { "session" } else { "sessions" },
                    response.source,
                );
                for session in &response.sessions {
                    session.write_summary(&mut text);
                }
                if response.truncated {
                    text.push_str("More matches may exist. Narrow the query or raise --limit.\n");
                }
            }
            SessionResult::Get(response) => {
                return super::write_output(writer, &response.to_text()?).await;
            }
        }
        super::write_output(writer, text.as_bytes()).await
    }
}

/// Search local metadata by default. Cloud requires the feature and read setup.
pub(crate) async fn search(args: SearchArgs, config: Config) -> Result<SessionOutput, String> {
    let response = match args.source {
        Some(SearchMode::Local) => local_search(args.query, args.limit, config).await?,
        #[cfg(feature = "cloud")]
        Some(mode) => {
            let client = cloud_client(&config).await?;
            cloud::search(args.query, args.limit, mode, config, client).await?
        }
        #[cfg(feature = "cloud")]
        None => match cloud::CloudClient::connect(&config).await? {
            Some(client) => {
                cloud::search(args.query, args.limit, SearchMode::All, config, client).await?
            }
            None => local_search(args.query, args.limit, config).await?,
        },
        #[cfg(not(feature = "cloud"))]
        None => local_search(args.query, args.limit, config).await?,
    };
    Ok(SessionOutput {
        json: args.json,
        result: SessionResult::Search(response),
    })
}

async fn local_search(
    query: String,
    limit: Option<u16>,
    config: Config,
) -> Result<SearchResponse, String> {
    let result = search::lookup(query, limit, config).await?;
    Ok(SearchResponse {
        source: "local",
        sources: SearchSources {
            local: Some(SourceState {
                truncated: result.truncated,
            }),
            #[cfg(feature = "cloud")]
            cloud: None,
        },
        #[cfg(feature = "cloud")]
        workspace_id: None,
        sessions: result
            .sessions
            .into_iter()
            .map(SessionMatch::local)
            .collect(),
        truncated: result.truncated,
    })
}

/// Read one document for the exact identity without falling back to another source.
pub(crate) async fn get(args: GetArgs, config: Config) -> Result<SessionOutput, String> {
    let GetArgs {
        session_id,
        source,
        agent,
        environment,
        json,
    } = args;
    let response = match source {
        GetSource::Local => {
            search::get_local_document(session_id, agent, environment, config).await?
        }
        #[cfg(feature = "cloud")]
        GetSource::Cloud => {
            if agent.is_some() || environment.is_some() {
                return Err("--agent and --environment select a local session only".into());
            }
            let client = cloud_client(&config).await?;
            cloud::get(&session_id, client).await?
        }
    };
    Ok(SessionOutput {
        json,
        result: SessionResult::Get(Box::new(response)),
    })
}

#[cfg(feature = "cloud")]
async fn cloud_client(config: &Config) -> Result<cloud::CloudClient, String> {
    cloud::CloudClient::connect(config)
        .await?
        .ok_or_else(|| "connect Antiburn Desktop to Cloud to use this command".into())
}

/// Render untrusted metadata on one terminal line without control codes.
pub(super) fn safe_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

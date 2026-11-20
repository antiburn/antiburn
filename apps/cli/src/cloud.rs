//! Bounded, bearer-only requests to one explicitly configured Cloud origin.

use std::{fmt::Write, time::Duration};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, RequestBuilder, StatusCode, Url, header::HeaderValue, redirect::Policy};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    config::Config,
    sessions::{GetResponse, SessionMatch, safe_text},
};

const MAX_PAGE_BYTES: usize = 128 * 1024;
const MAX_MESSAGES: usize = 8;
const MAX_MESSAGE_BYTES: usize = 8 * 1024;

mod device;
mod search;
pub(crate) use search::{CloudSession, search};

/// Restrict Cloud requests to the service and workspace from the device connection.
pub(crate) struct CloudClient {
    http: Client,
    origin: Url,
    workspace_id: Uuid,
    authorization: HeaderValue,
}

impl CloudClient {
    /// Load the app's current access proof. Absence means Cloud is not available.
    pub(crate) async fn connect(config: &Config) -> Result<Option<Self>, String> {
        let Some(connection) = device::current(config).await? else {
            return Ok(None);
        };
        let origin = validate_origin(&connection.origin)?;
        let config = config.cloud.clone().unwrap_or_default();
        let workspace_id = config
            .workspace_id
            .unwrap_or(connection.personal_workspace_id);
        if workspace_id.is_nil() {
            return Err("the Cloud device connection did not return a workspace".into());
        }
        let token = connection.access_token;
        if token.is_empty()
            || token.len() > 16_384
            || token.bytes().any(|byte| !byte.is_ascii_graphic())
        {
            return Err("invalid Cloud device access proof".into());
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| "invalid Cloud device access proof")?;
        authorization.set_sensitive(true);
        let builder = Client::builder()
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
            .timeout(Duration::from_secs(config.request_timeout_secs));
        let builder = if origin.scheme() == "http" && origin.host_str() == Some("localhost") {
            builder.resolve("localhost", ([127, 0, 0, 1], 0).into())
        } else {
            builder
        };
        let http = builder
            .build()
            .map_err(|_| "could not initialize the Cloud client")?;
        Ok(Some(Self {
            http,
            origin,
            workspace_id,
            authorization,
        }))
    }

    fn sessions(&self, session_id: Option<Uuid>) -> RequestBuilder {
        let mut url = self.origin.clone();
        url.set_path(&format!("/v1/workspaces/{}/sessions", self.workspace_id));
        if let Some(session_id) = session_id {
            url.set_path(&format!("{}/{session_id}/document", url.path()));
        }
        self.http
            .get(url)
            .header(reqwest::header::AUTHORIZATION, self.authorization.clone())
            .header(reqwest::header::ACCEPT, "application/json")
    }
}

#[derive(Deserialize)]
struct Page {
    #[serde(deserialize_with = "Deserialize::deserialize")]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct DocumentPage {
    #[serde(default)]
    workspace_id: Option<Uuid>,
    session_id: Uuid,
    revision: Uuid,
    stale: bool,
    messages: Vec<Box<RawValue>>,
    page: Page,
}

/// One bounded Cloud document page. Message values are inert, untrusted JSON.
#[derive(Serialize)]
pub(crate) struct CloudDocument {
    revision: Uuid,
    stale: bool,
    messages: Vec<Box<RawValue>>,
    next_cursor: Option<String>,
}

impl CloudDocument {
    pub(crate) fn write_text(&self, text: &mut String) {
        let _ = writeln!(text, "\nDocument revision: {}", self.revision);
        if self.stale {
            text.push_str("Cloud returned an older publication.\n");
        }
        let _ = writeln!(text, "Messages on this page: {}", self.messages.len());
        for message in &self.messages {
            // Keep message strings JSON-escaped so they cannot control the terminal.
            let _ = writeln!(text, "{}", safe_text(message.get()));
        }
        if let Some(cursor) = &self.next_cursor {
            let _ = writeln!(text, "Next page: --after {cursor}");
        }
    }
}

/// Read one Cloud page for the explicit session UUID. Never read local content.
pub(crate) async fn get(
    session_id: &str,
    after: Option<&str>,
    client: CloudClient,
) -> Result<GetResponse, String> {
    let session_id = Uuid::parse_str(session_id).map_err(|_| "select a Cloud session UUID")?;
    if session_id.is_nil() {
        return Err("select a non-nil Cloud session UUID".into());
    }
    let mut request = client.sessions(Some(session_id));
    let continuation = after
        .map(|after| Continuation::parse(after, &client.origin, client.workspace_id, session_id))
        .transpose()?;
    if let Some(continuation) = &continuation {
        request = request.query(&[("page[after]", continuation.token)]);
    }
    let page: DocumentPage = read_json(request, MAX_PAGE_BYTES).await?;
    if page
        .workspace_id
        .is_some_and(|workspace| workspace != client.workspace_id)
        || page.session_id != session_id
        || page.revision.is_nil()
        || continuation
            .as_ref()
            .is_some_and(|cursor| cursor.revision != page.revision)
        || page.messages.len() > MAX_MESSAGES
        || page.messages.iter().any(|message| {
            message.get().len() > MAX_MESSAGE_BYTES || !message.get().starts_with('{')
        })
        || page.page.next_cursor.as_ref().is_some_and(|after| {
            !valid_server_cursor(after)
                || continuation
                    .as_ref()
                    .is_some_and(|cursor| cursor.token == after)
                || page.messages.is_empty()
        })
    {
        return Err("Cloud document response does not match the request".into());
    }
    let next_cursor = page.page.next_cursor.map(|token| {
        format!(
            "{}.{}.{}.{}.{token}",
            origin_key(&client.origin),
            client.workspace_id,
            session_id,
            page.revision
        )
    });
    Ok(GetResponse {
        session: SessionMatch {
            local: None,
            cloud: Some(CloudSession::from_id(session_id)),
        },
        workspace_id: Some(client.workspace_id),
        document: Some(CloudDocument {
            revision: page.revision,
            stale: page.stale,
            messages: page.messages,
            next_cursor,
        }),
    })
}

struct Continuation<'a> {
    revision: Uuid,
    token: &'a str,
}

impl<'a> Continuation<'a> {
    /// Keep a continuation bound to its original service, session, and publication.
    fn parse(value: &'a str, origin: &Url, workspace: Uuid, session: Uuid) -> Result<Self, String> {
        let invalid = "invalid document cursor";
        if value.len() != 44 + 3 * 37 + 64 {
            return Err(invalid.into());
        }
        let mut parts = value.splitn(5, '.');
        if parts.next() != Some(origin_key(origin).as_str()) {
            return Err(invalid.into());
        }
        let matched_workspace = parts.next().and_then(|part| Uuid::parse_str(part).ok());
        let matched_session = parts.next().and_then(|part| Uuid::parse_str(part).ok());
        let revision = parts
            .next()
            .and_then(|part| Uuid::parse_str(part).ok())
            .filter(|id| !id.is_nil())
            .ok_or(invalid)?;
        let token = parts
            .next()
            .filter(|part| valid_server_cursor(part))
            .ok_or(invalid)?;
        if matched_workspace != Some(workspace) || matched_session != Some(session) {
            return Err(invalid.into());
        }
        Ok(Self { revision, token })
    }
}

fn origin_key(origin: &Url) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(origin.origin().ascii_serialization()))
}

fn validate_origin(value: &str) -> Result<Url, String> {
    let invalid = "Cloud origin must be a canonical HTTPS origin with no path";
    if value.len() > 2048 {
        return Err(invalid.into());
    }
    let origin = Url::parse(value).map_err(|_| invalid)?;
    let https = origin.scheme() == "https";
    let loopback_debug = cfg!(debug_assertions)
        && origin.scheme() == "http"
        && matches!(origin.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(https || loopback_debug)
        || origin.host().is_none()
        || origin.origin().ascii_serialization() != value
    {
        return Err(invalid.into());
    }
    Ok(origin)
}

fn valid_server_cursor(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Read only the expected success body. Never put transport or response details in diagnostics.
async fn read_json<T: DeserializeOwned>(
    request: RequestBuilder,
    max_bytes: usize,
) -> Result<T, String> {
    let mut response = request.send().await.map_err(|_| "Cloud request failed")?;
    match response.status() {
        StatusCode::OK => {}
        StatusCode::BAD_REQUEST => return Err("Cloud rejected the query or cursor".into()),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            return Err("Cloud access denied".into());
        }
        StatusCode::NOT_FOUND => return Err("Cloud session or workspace not found".into()),
        StatusCode::CONFLICT => {
            return Err("Cloud document changed. Start again without --after".into());
        }
        StatusCode::UNPROCESSABLE_ENTITY => {
            return Err("Cloud does not support reading this document format or size".into());
        }
        _ => return Err("Cloud request failed".into()),
    }
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err("Cloud response exceeds the size limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Cloud response could not be read")?
    {
        if chunk.len() > max_bytes - bytes.len() {
            return Err("Cloud response exceeds the size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "Cloud returned an invalid response".into())
}

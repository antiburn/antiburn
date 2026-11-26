//! Bounded, bearer-only requests to one explicitly configured Cloud origin.

use std::time::Duration;

use reqwest::{Client, RequestBuilder, StatusCode, Url, header::HeaderValue, redirect::Policy};
use serde::{Deserialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::{
    config::Config,
    document::{MAX_DOCUMENT_BYTES, SessionDocument},
};

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
        let token = connection.read_token;
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

/// Read one complete Cloud response for the selected UUID, with no local fallback.
pub(crate) async fn get(session_id: &str, client: CloudClient) -> Result<SessionDocument, String> {
    let session_id = Uuid::parse_str(session_id).map_err(|_| "select a Cloud session UUID")?;
    if session_id.is_nil() {
        return Err("select a non-nil Cloud session UUID".into());
    }
    let body = read_body(client.sessions(Some(session_id)), MAX_DOCUMENT_BYTES).await?;
    let document = SessionDocument::decode_cloud(&body)?;
    document.validate_cloud(client.workspace_id, session_id)?;
    Ok(document)
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

async fn read_json<T: DeserializeOwned>(
    request: RequestBuilder,
    max_bytes: usize,
) -> Result<T, String> {
    let bytes = read_body(request, max_bytes).await?;
    serde_json::from_slice(&bytes).map_err(|_| "Cloud returned an invalid response".into())
}

/// Read only the expected success body. Never put transport or response details in diagnostics.
async fn read_body(request: RequestBuilder, max_bytes: usize) -> Result<Vec<u8>, String> {
    let timeout = "Cloud request timed out; check cloud.request_timeout_secs";
    let mut response = request.send().await.map_err(|error| {
        if !error.is_timeout() {
            "Cloud request failed"
        } else if error.is_connect() {
            "Cloud connection timed out; check cloud.connect_timeout_secs and cloud.request_timeout_secs"
        } else {
            timeout
        }
    })?;
    match response.status() {
        StatusCode::OK => {}
        StatusCode::BAD_REQUEST => return Err("Cloud rejected the request".into()),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            return Err("Cloud access denied".into());
        }
        StatusCode::NOT_FOUND => return Err("Cloud session or document not found".into()),
        StatusCode::PAYLOAD_TOO_LARGE | StatusCode::UNPROCESSABLE_ENTITY => {
            return Err("Cloud cannot read this document format or size".into());
        }
        StatusCode::SERVICE_UNAVAILABLE => return Err("Cloud is unavailable; try again".into()),
        _ => return Err("Cloud request failed".into()),
    }
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err("Cloud response exceeds the size limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        if error.is_timeout() {
            timeout
        } else {
            "Cloud response could not be read"
        }
    })? {
        if chunk.len() > max_bytes - bytes.len() {
            return Err("Cloud response exceeds the size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

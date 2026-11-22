//! Bounded session upload HTTP. API proof stays inside the auth engine.
//! Signed upload locations exist only for one request and never enter storage,
//! renderer state, tracing, or errors.

use super::{auth, engine::Engine};
use crate::store::cloud_capture::SourceKind;
use antiburn_cloud_capture::{
    CaptureIdentity, CaptureManifest, Chunk, ChunkKey, Limits, ServerStatus, Transport,
    TransportError, UploadPlan,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    time::Duration,
};

const MAX_API_BYTES: u64 = 2 * 1024 * 1024;

/// The engine admits only these fixed capture routes on its pinned resource.
#[derive(Clone, Copy, Debug)]
pub(super) enum Route {
    Capabilities,
    Reserve,
    Manifest,
    Status,
    Parts,
    Finalize,
    Renew,
}

impl Route {
    fn path(self, capture: &str) -> String {
        match self {
            Self::Capabilities => "/v1/captures/capabilities".into(),
            Self::Reserve => "/v1/captures/reserve".into(),
            Self::Manifest => format!("/v1/captures/{capture}/manifest"),
            Self::Status => format!("/v1/captures/{capture}/status"),
            Self::Parts => format!("/v1/captures/{capture}/parts"),
            Self::Finalize => format!("/v1/captures/{capture}/finalize"),
            Self::Renew => format!("/v1/captures/{capture}/renew"),
        }
    }
}

/// A bounded response without request URLs, headers, or credentials.
pub(super) struct ApiResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub retry_after: Option<Duration>,
}

/// Send only to the configured API origin. The caller must validate the current
/// grant immediately before this call. No client has cookie storage enabled.
pub(super) fn api_request(
    config: &auth::Config,
    access: &str,
    identity: &CaptureIdentity,
    route: Route,
    body: &impl Serialize,
) -> Result<ApiResponse, TransportError> {
    if !super::capture::valid_request_id(&identity.capture_id) {
        return Err(TransportError::Rejected);
    }
    let mut url = url::Url::parse(&config.resource)
        .and_then(|origin| origin.join(&route.path(&identity.capture_id)))
        .map_err(|_| TransportError::Rejected)?;
    let bytes = serde_json::to_vec(body).map_err(|_| TransportError::Rejected)?;
    if bytes.len() as u64 > MAX_API_BYTES {
        return Err(TransportError::Rejected);
    }
    if matches!(route, Route::Status) {
        let context: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| TransportError::Rejected)?;
        let plan = context["plan_id"]
            .as_str()
            .ok_or(TransportError::Rejected)?;
        let manifest = context["manifest_sha256"]
            .as_str()
            .ok_or(TransportError::Rejected)?;
        url.query_pairs_mut()
            .append_pair("plan_id", plan)
            .append_pair("manifest_sha256", manifest);
    }
    let client = auth::client(&url).map_err(|_| TransportError::Offline)?;
    let request = if matches!(route, Route::Status | Route::Capabilities) {
        client.get(url)
    } else {
        client
            .post(url)
            .header("content-type", "application/json")
            .body(bytes)
    };
    let response = request
        .bearer_auth(access)
        .send()
        .map_err(|_| TransportError::Offline)?;
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(retry_delay);
    let mut body = Vec::new();
    response
        .take(MAX_API_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|_| TransportError::Offline)?;
    if body.len() as u64 > MAX_API_BYTES {
        return Err(TransportError::Rejected);
    }
    Ok(ApiResponse {
        status,
        body,
        retry_after,
    })
}

/// Bound service-directed waits. Invalid hints leave the caller's normal backoff.
fn retry_delay(value: &str) -> Option<Duration> {
    let seconds = value.parse::<u64>().ok().or_else(|| {
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc2822)
            .ok()
            .map(|deadline| {
                deadline
                    .unix_timestamp()
                    .saturating_sub(super::now())
                    .max(1) as u64
            })
    })?;
    Some(Duration::from_secs(seconds.clamp(1, 300)))
}

/// Safe operation outcome, separate from the capture library's closed errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    Backpressure,
    Offline,
    Unauthorized,
    Conflict,
    SourceSuppressed,
    Unavailable,
    Rejected,
}

/// Verified upload bounds. Missing or disabled service support never selects a
/// local default. The profile leaves room for both native companion and metadata.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Capabilities {
    enabled: bool,
    max_expanded_bytes: u64,
    chunk_expanded_bytes: u64,
    source_formats: Vec<String>,
}

/// Validated service bounds and exact format names.
pub(super) struct UploadCapabilities {
    pub limits: Limits,
    formats: BTreeSet<String>,
}

impl UploadCapabilities {
    /// An advertised format must match a local exporter exactly.
    pub(super) fn supports(&self, kind: SourceKind) -> bool {
        self.formats.contains(kind.upload_names().1)
    }
}

impl Capabilities {
    /// Keep capture below the current service bound without clipping source.
    pub(super) fn validate(self) -> Result<UploadCapabilities, Failure> {
        use antiburn_cloud_capture::{CHUNK_BYTES, METADATA_BYTES};
        let format_count = self.source_formats.len();
        let formats = self.source_formats.into_iter().collect::<BTreeSet<_>>();
        if !self.enabled
            || self.max_expanded_bytes > 1024 * 1024 * 1024
            || self.max_expanded_bytes <= 2 * METADATA_BYTES
            || self.chunk_expanded_bytes != CHUNK_BYTES
            || formats.len() != format_count
            || ![
                SourceKind::ClaudeJsonl,
                SourceKind::OpenCodeSqliteV2,
                SourceKind::CodexJsonl,
                SourceKind::PiJsonl,
            ]
            .iter()
            .any(|kind| formats.contains(kind.upload_names().1))
        {
            return Err(Failure::Unavailable);
        }
        Ok(UploadCapabilities {
            limits: Limits {
                source_bytes: (self.max_expanded_bytes - 2 * METADATA_BYTES).min(128 * 1024 * 1024),
                capture_bytes: self.max_expanded_bytes,
                ..Limits::default()
            },
            formats,
        })
    }
}

/// Keep a safe capability failure and its service-directed retry delay.
#[derive(Debug)]
pub(super) struct CapabilityFailure {
    pub failure: Failure,
    pub retry_after: Option<Duration>,
}

/// Keep server backoff with capability failures before any capture job exists.
pub(super) fn capabilities(
    engine: &mut Engine,
    account: &str,
    destination: (&str, Option<&str>),
    account_current: &dyn Fn() -> bool,
    current: &dyn Fn() -> bool,
) -> Result<UploadCapabilities, CapabilityFailure> {
    let identity = CaptureIdentity {
        capture_id: "capabilities".into(),
        account_id: account.into(),
        destination_id: destination.0.into(),
        source_id: "capabilities".into(),
    };
    let origin = engine
        .resource_origin()
        .ok_or(CapabilityFailure {
            failure: Failure::Unavailable,
            retry_after: None,
        })?
        .to_owned();
    let mut transport = HttpTransport::new(engine, account_current, current, &origin, None)
        .with_membership_generation(destination.1);
    let response = transport
        .request(&identity, Route::Capabilities, &())
        .map_err(|_| CapabilityFailure {
            failure: transport.failure.unwrap_or(Failure::Rejected),
            retry_after: transport.retry_after,
        })?;
    let value: Capabilities = transport.decode(response).map_err(|_| CapabilityFailure {
        failure: transport.failure.unwrap_or(Failure::Rejected),
        retry_after: transport.retry_after,
    })?;
    value.validate().map_err(|failure| CapabilityFailure {
        failure,
        retry_after: None,
    })
}

/// Candidate-owned public metadata required by reservation before payload reads.
pub(super) struct Source {
    pub agent: &'static str,
    pub source_format: &'static str,
    pub native_session_id: String,
}

impl Source {
    /// Select exact public provider names, not an installed producer version.
    pub(super) fn from_candidate(candidate: &antiburn_cloud_capture::Candidate) -> Self {
        let (agent, source_format) = SourceKind::from(candidate.source_kind()).upload_names();
        Self {
            agent,
            source_format,
            native_session_id: candidate.native_session_id().into(),
        }
    }
}

/// A short-lived adapter borrows account authority for one capture or resume step.
/// A single controller admission slot bounds API calls and part uploads globally.
pub(super) struct HttpTransport<'a> {
    engine: &'a mut Engine,
    account_current: &'a dyn Fn() -> bool,
    current: &'a dyn Fn() -> bool,
    source: Option<Source>,
    resource_origin: String,
    membership_generation: Option<String>,
    pub failure: Option<Failure>,
    pub retry_after: Option<Duration>,
    /// Status and renewal both refused the exact frozen binding with a conflict.
    pub status_renewal_conflict: bool,
}

impl<'a> HttpTransport<'a> {
    /// Borrow the engine without copying or exposing its private access proof.
    /// `account_current` excludes content pause. `current` checks both account
    /// and content-policy generations before service or object-storage work.
    pub(super) fn new(
        engine: &'a mut Engine,
        account_current: &'a dyn Fn() -> bool,
        current: &'a dyn Fn() -> bool,
        resource_origin: &str,
        source: Option<Source>,
    ) -> Self {
        Self {
            engine,
            account_current,
            current,
            source,
            resource_origin: resource_origin.into(),
            membership_generation: None,
            failure: None,
            retry_after: None,
            status_renewal_conflict: false,
        }
    }

    /// Pin the Team membership from the saved job or explicit folder approval.
    pub(super) fn with_membership_generation(mut self, generation: Option<&str>) -> Self {
        self.membership_generation = generation.map(str::to_owned);
        self
    }

    fn request(
        &mut self,
        identity: &CaptureIdentity,
        route: Route,
        body: &impl Serialize,
    ) -> Result<ApiResponse, TransportError> {
        match self.engine.capture_request(
            identity,
            (&self.resource_origin, self.membership_generation.as_deref()),
            route,
            body,
            self.account_current,
            self.current,
        ) {
            Ok(response) => {
                self.retry_after = response.retry_after;
                let (failure, error) = match response.status {
                    200..=299 => {
                        self.failure = None;
                        return Ok(response);
                    }
                    401 | 403 => (Failure::Unauthorized, TransportError::Unauthorized),
                    409 if matches!(route, Route::Reserve) && source_suppressed(&response.body) => {
                        (Failure::SourceSuppressed, TransportError::Conflict)
                    }
                    409 => (Failure::Conflict, TransportError::Conflict),
                    429 => (
                        Failure::Backpressure,
                        if !matches!(route, Route::Reserve) || capacity_refused(&response.body) {
                            TransportError::Backpressure
                        } else {
                            // An unknown reserve response cannot permit new source reads.
                            TransportError::Offline
                        },
                    ),
                    404 | 405 | 501 => (Failure::Unavailable, TransportError::Rejected),
                    500..=599 => (Failure::Offline, TransportError::Offline),
                    _ => (Failure::Rejected, TransportError::Rejected),
                };
                if failure == Failure::Backpressure {
                    tracing::info!(
                        event = "cloud_capture_api_backpressure",
                        route = ?route,
                        http_status = response.status,
                        failure = ?failure,
                    );
                } else {
                    tracing::warn!(
                        event = "cloud_capture_api_failed",
                        route = ?route,
                        http_status = response.status,
                        failure = ?failure,
                    );
                }
                self.failure = Some(failure);
                Err(error)
            }
            Err(error) => {
                self.failure = Some(match error {
                    TransportError::Backpressure => Failure::Backpressure,
                    TransportError::Offline => Failure::Offline,
                    TransportError::Unauthorized => Failure::Unauthorized,
                    TransportError::Conflict => Failure::Conflict,
                    TransportError::Rejected => Failure::Rejected,
                });
                tracing::warn!(
                    event = "cloud_capture_request_failed",
                    route = ?route,
                    failure = ?self.failure,
                );
                Err(error)
            }
        }
    }

    /// Reuse the same body and binding. Do not replay if renewal fails.
    fn request_with_renewal(
        &mut self,
        identity: &CaptureIdentity,
        route: Route,
        body: &impl Serialize,
        binding: &impl Serialize,
    ) -> Result<ApiResponse, TransportError> {
        match self.request(identity, route, body) {
            Err(_) if self.failure == Some(Failure::Conflict) => {
                tracing::info!(event = "cloud_capture_renewal_started", route = ?route);
                if let Err(error) = self.renew(identity, binding) {
                    self.status_renewal_conflict =
                        matches!(route, Route::Status) && self.failure == Some(Failure::Conflict);
                    tracing::warn!(
                        event = "cloud_capture_renewal_failed",
                        route = ?route,
                        failure = ?self.failure,
                    );
                    return Err(error);
                }
                tracing::info!(event = "cloud_capture_renewal_succeeded", route = ?route);
                self.request(identity, route, body)
            }
            result => result,
        }
    }

    /// Renew only the given reservation and immutable registered plan.
    fn renew(
        &mut self,
        identity: &CaptureIdentity,
        binding: &impl Serialize,
    ) -> Result<(), TransportError> {
        let response = self.request(identity, Route::Renew, binding)?;
        if response.status != 204 || !response.body.is_empty() {
            self.failure = Some(Failure::Rejected);
            return Err(TransportError::Rejected);
        }
        Ok(())
    }

    fn decode<T: DeserializeOwned>(&mut self, response: ApiResponse) -> Result<T, TransportError> {
        if response.status != 200 {
            self.failure = Some(Failure::Rejected);
            return Err(TransportError::Rejected);
        }
        serde_json::from_slice(&response.body).map_err(|_| {
            self.failure = Some(Failure::Rejected);
            TransportError::Rejected
        })
    }

    /// Replay only the saved finalization. Do not try to renew from this path.
    pub(super) fn replay_saved_finalization(
        &mut self,
        reservation: &str,
        manifest: &CaptureManifest,
        plan: &UploadPlan,
    ) -> Result<(), TransportError> {
        let binding = Context::new(reservation, plan);
        let response = self.request(&manifest.identity, Route::Finalize, &binding)?;
        check_finalization_response(&response)
    }
}

/// Only the closed capacity refusal proves that reserve created no reservation.
fn capacity_refused(body: &[u8]) -> bool {
    api_error_is(body, "capture_limit", true)
}

/// Check only a closed API error, not message text from a server or proxy.
fn api_error_is(body: &[u8], code: &str, retryable: bool) -> bool {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ApiError {
        code: String,
        retryable: bool,
        #[serde(rename = "message")]
        _message: String,
        #[serde(rename = "requestId")]
        _request_id: String,
    }
    serde_json::from_slice::<ApiError>(body)
        .is_ok_and(|value| value.code == code && value.retryable == retryable)
}

/// Accept only the API's closed reserve error, never raw storage/provider text.
fn source_suppressed(body: &[u8]) -> bool {
    api_error_is(body, "source_suppressed", false)
}

#[derive(Serialize)]
struct Context<'a> {
    reservation_id: &'a str,
    manifest_sha256: &'a str,
    plan_id: &'a str,
}

impl<'a> Context<'a> {
    fn new(reservation_id: &'a str, plan: &'a UploadPlan) -> Self {
        Self {
            reservation_id,
            manifest_sha256: &plan.manifest_sha256,
            plan_id: &plan.plan_id,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Part {
    url: String,
    headers: BTreeMap<String, String>,
    part_number: u32,
}

/// Validate short-lived signed instructions before reading a saved chunk body.
/// A local service can omit the optional S3 checksum header. Manifest digests and
/// final integrity acceptance remain required.
fn signed_request(
    part: Part,
    chunk: &Chunk,
) -> Result<(url::Url, reqwest::header::HeaderMap), TransportError> {
    if part.part_number != 1
        || part.url.len() > 16_384
        || chunk.compressed_bytes == 0
        || chunk.compressed_bytes > 9 * 1024 * 1024
        || chunk.compressed_sha256.len() != 64
    {
        return Err(TransportError::Rejected);
    }
    let url = url::Url::parse(&part.url).map_err(|_| TransportError::Rejected)?;
    let parts = url
        .query_pairs()
        .filter(|(key, _)| key == "partNumber")
        .collect::<Vec<_>>();
    let uploads = url
        .query_pairs()
        .filter(|(key, _)| key == "uploadId")
        .collect::<Vec<_>>();
    if !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
        || parts.len() != 1
        || parts[0].1 != "1"
        || uploads.len() != 1
        || uploads[0].1.is_empty()
        || uploads[0].1.len() > 1024
    {
        return Err(TransportError::Rejected);
    }
    let mut headers = reqwest::header::HeaderMap::new();
    if part.headers.len() > 16 {
        return Err(TransportError::Rejected);
    }
    for (name, value) in part.headers {
        if name.len() > 128
            || value.len() > 4096
            || !matches!(
                name.to_ascii_lowercase().as_str(),
                "content-type"
                    | "content-length"
                    | "x-amz-checksum-sha256"
                    | "x-amz-content-sha256"
            )
        {
            return Err(TransportError::Rejected);
        }
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| TransportError::Rejected)?;
        let value =
            reqwest::header::HeaderValue::from_str(&value).map_err(|_| TransportError::Rejected)?;
        if headers.insert(name, value).is_some() {
            return Err(TransportError::Rejected);
        }
    }
    use base64::Engine as _;
    let digest = (0..64)
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(
                chunk
                    .compressed_sha256
                    .get(index..index + 2)
                    .ok_or(TransportError::Rejected)?,
                16,
            )
            .map_err(|_| TransportError::Rejected)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let checksum = base64::engine::general_purpose::STANDARD.encode(digest);
    if headers
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        != Some(chunk.compressed_bytes.to_string().as_str())
        || headers
            .get("x-amz-checksum-sha256")
            .is_some_and(|value| value.to_str().ok() != Some(checksum.as_str()))
    {
        return Err(TransportError::Rejected);
    }
    Ok((url, headers))
}

impl Transport for HttpTransport<'_> {
    fn reserve(
        &mut self,
        identity: &CaptureIdentity,
        maximum_expanded_bytes: u64,
    ) -> Result<String, TransportError> {
        let source = self.source.as_ref().ok_or(TransportError::Rejected)?;
        let body = serde_json::json!({
            "identity": identity,
            "maximum_expanded_bytes": maximum_expanded_bytes,
            "agent": source.agent,
            "source_format": source.source_format,
            "native_session_id": source.native_session_id,
            "membership_generation": self.membership_generation,
        });
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Reservation {
            reservation_id: String,
        }
        let response = self.request(identity, Route::Reserve, &body)?;
        let reservation: Reservation = self.decode(response)?;
        Ok(reservation.reservation_id)
    }

    fn negotiate(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
    ) -> Result<UploadPlan, TransportError> {
        let response = self.request_with_renewal(
            &manifest.identity,
            Route::Manifest,
            &serde_json::json!({
                "reservation_id": reservation_id, "manifest": manifest,
            }),
            &serde_json::json!({
                "reservation_id": reservation_id, "manifest_sha256": null, "plan_id": null,
            }),
        )?;
        self.decode(response)
    }

    fn status(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
        plan: &UploadPlan,
    ) -> Result<ServerStatus, TransportError> {
        let binding = Context::new(reservation_id, plan);
        let response =
            self.request_with_renewal(&manifest.identity, Route::Status, &binding, &binding)?;
        self.decode(response)
    }

    fn upload_part(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
        plan: &UploadPlan,
        key: &ChunkKey,
        chunk: &Chunk,
        body: &mut dyn Read,
    ) -> Result<(), TransportError> {
        let binding = Context::new(reservation_id, plan);
        let response = self.request_with_renewal(
            &manifest.identity,
            Route::Parts,
            &serde_json::json!({
                "reservation_id": reservation_id, "manifest_sha256": plan.manifest_sha256,
                "plan_id": plan.plan_id, "key": key,
            }),
            &binding,
        )?;
        let part: Part = self.decode(response)?;
        let (url, headers) = signed_request(part, chunk)?;
        let mut bytes = Vec::with_capacity(chunk.compressed_bytes as usize);
        body.take(chunk.compressed_bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| TransportError::Offline)?;
        if bytes.len() as u64 != chunk.compressed_bytes {
            return Err(TransportError::Rejected);
        }
        if !(self.current)() || self.engine.session().is_none() {
            return Err(TransportError::Unauthorized);
        }
        // This client has no default bearer or cookie storage.
        let response = auth::client(&url)
            .map_err(|_| TransportError::Rejected)?
            .put(url)
            .headers(headers)
            .body(bytes)
            .send()
            .map_err(|_| TransportError::Offline)?;
        if !(self.current)() {
            return Err(TransportError::Unauthorized);
        }
        if response.status().is_success() {
            Ok(())
        } else {
            Err(TransportError::Offline)
        }
    }

    fn complete(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
        plan: &UploadPlan,
    ) -> Result<(), TransportError> {
        let binding = Context::new(reservation_id, plan);
        let response =
            self.request_with_renewal(&manifest.identity, Route::Finalize, &binding, &binding)?;
        check_finalization_response(&response)
    }
}

fn check_finalization_response(response: &ApiResponse) -> Result<(), TransportError> {
    if response.status == 202 {
        Ok(())
    } else {
        Err(TransportError::Rejected)
    }
}

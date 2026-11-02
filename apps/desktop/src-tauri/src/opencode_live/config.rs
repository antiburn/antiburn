//! Read-only current configuration from the verified local OpenCode consumer.

use std::{
    fmt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use reqwest::{Client, RequestBuilder};
use serde_json::Value;

use super::{ServiceRegistration, is_v2_version, read_registration, verify_database_association};

const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RECEIPT_AGE: Duration = Duration::from_secs(10);

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ConfigAuthorityRequest {
    pub registration_path: PathBuf,
    pub database_path: PathBuf,
    pub location: PathBuf,
}

impl fmt::Debug for ConfigAuthorityRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ConfigAuthorityRequest(<private paths>)")
    }
}

fn config_request(
    client: &Client,
    registration: &ServiceRegistration,
    location: &Path,
) -> Result<RequestBuilder, AuthorityError> {
    let directory = location.to_str().ok_or(AuthorityError::InvalidRequest)?;
    let mut url = registration
        .endpoint
        .base_url()
        .join("/api/config")
        .map_err(|_| AuthorityError::InvalidRequest)?;
    url.query_pairs_mut()
        .append_pair("location[directory]", directory);
    super::authorized_get(&registration.endpoint, client, url.as_str())
        .map_err(|_| AuthorityError::InvalidRequest)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthorityError {
    Unavailable,
    ChangedConsumer,
    UnsupportedConsumer,
    InvalidRequest,
    ResponseLimit,
    InvalidResponse,
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "OpenCode current configuration is unavailable: {self:?}"
        )
    }
}

impl std::error::Error for AuthorityError {}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ConfigEntry {
    pub path: Option<PathBuf>,
    pub info: Option<Value>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ConsumerIdentity {
    pid: u32,
    version: String,
    service_id: Option<String>,
    database: PathBuf,
    location: PathBuf,
    database_identity: DatabaseIdentity,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
type DatabaseIdentity = super::unix::FileIdentity;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[derive(Clone, Copy, PartialEq, Eq)]
struct DatabaseIdentity {
    created: std::time::SystemTime,
}

fn database_identity(path: &Path) -> Result<DatabaseIdentity, AuthorityError> {
    let metadata = std::fs::metadata(path).map_err(|_| AuthorityError::Unavailable)?;
    if !metadata.is_file() {
        return Err(AuthorityError::Unavailable);
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        Ok(super::unix::identity(&metadata))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(DatabaseIdentity {
            created: metadata
                .created()
                .map_err(|_| AuthorityError::Unavailable)?,
        })
    }
}

#[derive(Clone)]
struct CaptureStart {
    registration: ServiceRegistration,
    info: super::ServiceInfo,
    database: PathBuf,
    database_identity: DatabaseIdentity,
}

async fn blocking_io<T: Send + 'static>(
    read: impl FnOnce() -> Result<T, AuthorityError> + Send + 'static,
) -> Result<T, AuthorityError> {
    tokio::task::spawn_blocking(read)
        .await
        .map_err(|_| AuthorityError::Unavailable)?
}

pub(crate) async fn canonical_location(location: PathBuf) -> Result<PathBuf, AuthorityError> {
    blocking_io(move || {
        if !location.is_absolute() {
            return Err(AuthorityError::InvalidRequest);
        }
        let location = location
            .canonicalize()
            .map_err(|_| AuthorityError::Unavailable)?;
        if !location.is_dir() {
            return Err(AuthorityError::InvalidRequest);
        }
        Ok(location)
    })
    .await
}

async fn capture_start(
    registration_path: &Path,
    database_path: &Path,
) -> Result<CaptureStart, AuthorityError> {
    let registration_path = registration_path.to_path_buf();
    let database_path = database_path.to_path_buf();
    let snapshot_path = registration_path.clone();
    let (registration, database, database_identity) = blocking_io(move || {
        if !database_path.is_absolute() {
            return Err(AuthorityError::InvalidRequest);
        }
        let registration =
            read_registration(&snapshot_path).map_err(|_| AuthorityError::Unavailable)?;
        let database = database_path
            .canonicalize()
            .map_err(|_| AuthorityError::Unavailable)?;
        let identity = database_identity(&database)?;
        Ok((registration, database, identity))
    })
    .await?;
    let info = verify_database_association(&registration_path, &database, REQUEST_TIMEOUT)
        .await
        .map_err(|_| AuthorityError::Unavailable)?;
    if !is_v2_version(&info.version) || info.pid != registration.pid || registration.id.is_none() {
        return Err(AuthorityError::UnsupportedConsumer);
    }
    Ok(CaptureStart {
        registration,
        info,
        database,
        database_identity,
    })
}

/// A verified consumer has no workspace configuration or location dependency.
pub(crate) struct VerifiedConfigConsumer {
    registration_path: PathBuf,
    proof: CaptureStart,
    initial_proof: Option<Instant>,
}

impl VerifiedConfigConsumer {
    #[cfg(test)]
    pub(crate) async fn registered_async() -> Result<Self, AuthorityError> {
        Err(AuthorityError::Unavailable)
    }

    #[cfg(not(test))]
    pub(crate) async fn registered_async() -> Result<Self, AuthorityError> {
        let started = Instant::now();
        let (registration_path, candidates) = blocking_io(|| {
            let path =
                super::default_registration_path().map_err(|_| AuthorityError::Unavailable)?;
            Ok((path, registered_database_candidates()))
        })
        .await?;
        Self::from_candidates(&registration_path, &candidates, started).await
    }

    pub(crate) async fn from_candidates(
        registration_path: &Path,
        database_candidates: &[PathBuf],
        started: Instant,
    ) -> Result<Self, AuthorityError> {
        let mut bound: Option<CaptureStart> = None;
        for database_path in database_candidates {
            let remaining = REQUEST_TIMEOUT
                .checked_sub(started.elapsed())
                .ok_or(AuthorityError::Unavailable)?;
            if let Ok(proof) =
                tokio::time::timeout(remaining, capture_start(registration_path, database_path))
                    .await
                    .map_err(|_| AuthorityError::Unavailable)?
            {
                if bound
                    .as_ref()
                    .is_some_and(|prior| prior.database != proof.database)
                {
                    return Err(AuthorityError::ChangedConsumer);
                }
                bound = Some(proof);
            }
        }
        Ok(Self {
            registration_path: registration_path.into(),
            proof: bound.ok_or(AuthorityError::Unavailable)?,
            initial_proof: Some(Instant::now()),
        })
    }

    pub(crate) async fn capture_async(
        &mut self,
        location: &Path,
    ) -> Result<CurrentConfigAuthority, AuthorityError> {
        tokio::time::timeout(REQUEST_TIMEOUT, async {
            let proof = if self
                .initial_proof
                .take()
                .is_some_and(|at| at.elapsed() < REQUEST_TIMEOUT)
            {
                self.proof.clone()
            } else {
                let proof = capture_start(&self.registration_path, &self.proof.database).await?;
                if proof.registration != self.proof.registration
                    || proof.info != self.proof.info
                    || proof.database_identity != self.proof.database_identity
                    || proof.database != self.proof.database
                {
                    return Err(AuthorityError::ChangedConsumer);
                }
                proof
            };
            CurrentConfigAuthority::capture_verified(
                &ConfigAuthorityRequest {
                    registration_path: self.registration_path.clone(),
                    database_path: self.proof.database.clone(),
                    location: location.into(),
                },
                proof,
            )
            .await
        })
        .await
        .map_err(|_| AuthorityError::Unavailable)?
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.registration_path
            .as_os_str()
            .len()
            .saturating_add(self.proof.database.as_os_str().len())
            .saturating_mul(2)
            .saturating_add(16 * 1024)
            .saturating_add(std::mem::size_of::<Self>())
            .saturating_add(self.proof.info.version.capacity())
    }

    #[cfg(test)]
    pub(crate) fn fixture(location: &Path) -> Self {
        let receipt = CurrentConfigAuthority::fixture(location, serde_json::json!([]));
        Self {
            registration_path: receipt.request.registration_path,
            proof: CaptureStart {
                registration: super::decode_registration(
                    br#"{"id":"fixture","pid":1,"version":"2.0.32","url":"http://127.0.0.1:3456"}"#,
                )
                .unwrap(),
                info: super::ServiceInfo {
                    pid: 1,
                    version: "2.0.32".into(),
                },
                database: receipt.identity.database,
                database_identity: receipt.identity.database_identity,
            },
            initial_proof: None,
        }
    }
}

/// This receipt is private, short-lived, and never serializable.
#[derive(Clone)]
pub(crate) struct CurrentConfigAuthority {
    request: ConfigAuthorityRequest,
    identity: ConsumerIdentity,
    entries: Vec<ConfigEntry>,
    observed_at: Instant,
}

impl fmt::Debug for CurrentConfigAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CurrentConfigAuthority")
            .field("version", &self.identity.version)
            .field("entries", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl CurrentConfigAuthority {
    #[cfg(test)]
    pub(crate) fn registered(_: &Path) -> Result<Self, AuthorityError> {
        Err(AuthorityError::Unavailable)
    }

    /// Blocks the calling thread. Async callers must use a blocking worker.
    #[cfg(not(test))]
    pub(crate) fn registered(location: &Path) -> Result<Self, AuthorityError> {
        tauri::async_runtime::block_on(Self::registered_async(location))
    }

    #[cfg(test)]
    pub(crate) async fn registered_async(_: &Path) -> Result<Self, AuthorityError> {
        Err(AuthorityError::Unavailable)
    }

    #[cfg(not(test))]
    pub(crate) async fn registered_async(location: &Path) -> Result<Self, AuthorityError> {
        VerifiedConfigConsumer::registered_async()
            .await?
            .capture_async(location)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn registered_from_candidates(
        registration_path: &Path,
        database_candidates: &[PathBuf],
        location: &Path,
        started: Instant,
    ) -> Result<Self, AuthorityError> {
        VerifiedConfigConsumer::from_candidates(registration_path, database_candidates, started)
            .await?
            .capture_async(location)
            .await
    }

    /// Blocks the calling thread. Async callers must use `capture_async`.
    #[cfg(any(not(windows), test))]
    pub(crate) fn capture(request: &ConfigAuthorityRequest) -> Result<Self, AuthorityError> {
        tauri::async_runtime::block_on(Self::capture_async(request))
    }

    pub(crate) async fn capture_async(
        request: &ConfigAuthorityRequest,
    ) -> Result<Self, AuthorityError> {
        tokio::time::timeout(REQUEST_TIMEOUT, async {
            Self::capture_verified(
                request,
                capture_start(&request.registration_path, &request.database_path).await?,
            )
            .await
        })
        .await
        .map_err(|_| AuthorityError::Unavailable)?
    }

    async fn capture_verified(
        request: &ConfigAuthorityRequest,
        proof: CaptureStart,
    ) -> Result<Self, AuthorityError> {
        let CaptureStart {
            registration: before,
            info,
            database,
            database_identity,
        } = proof;
        let location = canonical_location(request.location.clone()).await?;
        validate_capture_files(
            request.clone(),
            before.clone(),
            database.clone(),
            database_identity,
            location.clone(),
        )
        .await?;
        let client = super::local_client()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| AuthorityError::Unavailable)?;
        let entries = fetch_config(&client, &before, &request.location).await?;
        let after_info = verify_database_association(
            &request.registration_path,
            &request.database_path,
            REQUEST_TIMEOUT,
        )
        .await
        .map_err(|_| AuthorityError::ChangedConsumer)?;
        if info != after_info {
            return Err(AuthorityError::ChangedConsumer);
        }
        validate_capture_files(
            request.clone(),
            before.clone(),
            database.clone(),
            database_identity,
            location.clone(),
        )
        .await?;
        Ok(Self {
            request: request.clone(),
            identity: ConsumerIdentity {
                pid: info.pid,
                version: info.version,
                service_id: before.id,
                database,
                location,
                database_identity,
            },
            entries,
            observed_at: Instant::now(),
        })
    }

    /// Blocks the calling thread. Async callers must use a blocking worker.
    #[cfg(not(windows))]
    pub(crate) fn refresh(&self) -> Result<Self, AuthorityError> {
        let fresh = Self::capture(&self.request)?;
        if self.identity != fresh.identity {
            return Err(AuthorityError::ChangedConsumer);
        }
        Ok(fresh)
    }

    /// Blocks the calling thread. Async callers must use a blocking worker.
    pub(crate) fn for_location(&self, location: &Path) -> Result<Self, AuthorityError> {
        tauri::async_runtime::block_on(self.for_location_async(location))
    }

    pub(crate) async fn for_location_async(&self, location: &Path) -> Result<Self, AuthorityError> {
        let mut request = self.request.clone();
        request.location = location.into();
        let fresh = Self::capture_async(&request).await?;
        if self.identity.pid != fresh.identity.pid
            || self.identity.version != fresh.identity.version
            || self.identity.service_id != fresh.identity.service_id
            || self.identity.database != fresh.identity.database
            || self.identity.database_identity != fresh.identity.database_identity
        {
            return Err(AuthorityError::ChangedConsumer);
        }
        Ok(fresh)
    }

    pub(crate) fn entries(&self) -> Result<&[ConfigEntry], AuthorityError> {
        if self.is_expired() {
            return Err(AuthorityError::ChangedConsumer);
        }
        Ok(&self.entries)
    }

    pub(crate) fn is_expired(&self) -> bool {
        self.observed_at.elapsed() > MAX_RECEIPT_AGE
    }

    #[cfg(test)]
    pub(crate) fn expired_fixture(mut self) -> Self {
        self.observed_at = Instant::now() - MAX_RECEIPT_AGE - Duration::from_secs(1);
        self
    }

    pub(crate) fn location(&self) -> &Path {
        &self.identity.location
    }

    pub(crate) fn revalidation_only(&self) -> Self {
        Self {
            request: self.request.clone(),
            identity: self.identity.clone(),
            entries: Vec::new(),
            observed_at: self.observed_at,
        }
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        [
            &self.request.registration_path,
            &self.request.database_path,
            &self.request.location,
            &self.identity.database,
            &self.identity.location,
        ]
        .into_iter()
        .map(|path| path.as_os_str().len().saturating_mul(2))
        .sum::<usize>()
        .saturating_add(self.identity.version.capacity())
        .saturating_add(
            self.identity
                .service_id
                .as_ref()
                .map_or(0, String::capacity),
        )
        .saturating_add(
            self.entries
                .iter()
                .map(|entry| {
                    entry
                        .path
                        .as_ref()
                        .map_or(0, |path| path.as_os_str().len().saturating_mul(2))
                        .saturating_add(
                            entry
                                .info
                                .as_ref()
                                .map_or(0, |info| info.to_string().len().saturating_mul(64)),
                        )
                })
                .sum::<usize>(),
        )
    }

    #[cfg(test)]
    pub(crate) fn fixture(location: &Path, entries: Value) -> Self {
        Self {
            request: ConfigAuthorityRequest {
                registration_path: location.join("service.json"),
                database_path: location.join("source.db"),
                location: location.into(),
            },
            identity: ConsumerIdentity {
                pid: 1,
                version: "2.0.32".into(),
                service_id: Some("fixture".into()),
                database: location.join("source.db"),
                location: location.canonicalize().unwrap(),
                database_identity: DatabaseIdentity {
                    #[cfg(any(target_os = "linux", target_os = "macos"))]
                    device: 1,
                    #[cfg(any(target_os = "linux", target_os = "macos"))]
                    inode: 1,
                    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                    created: std::time::SystemTime::UNIX_EPOCH,
                },
            },
            entries: decode_entries(&serde_json::to_vec(&entries).unwrap()).unwrap(),
            observed_at: Instant::now(),
        }
    }
}

async fn validate_capture_files(
    request: ConfigAuthorityRequest,
    registration: ServiceRegistration,
    database: PathBuf,
    identity: DatabaseIdentity,
    location: PathBuf,
) -> Result<(), AuthorityError> {
    blocking_io(move || {
        if read_registration(&request.registration_path)
            .map_err(|_| AuthorityError::ChangedConsumer)?
            != registration
            || database_identity(&request.database_path)? != identity
            || request
                .database_path
                .canonicalize()
                .map_err(|_| AuthorityError::ChangedConsumer)?
                != database
            || request
                .location
                .canonicalize()
                .map_err(|_| AuthorityError::ChangedConsumer)?
                != location
        {
            return Err(AuthorityError::ChangedConsumer);
        }
        Ok(())
    })
    .await
}

async fn fetch_config(
    client: &Client,
    registration: &ServiceRegistration,
    location: &Path,
) -> Result<Vec<ConfigEntry>, AuthorityError> {
    let request = config_request(client, registration, location)?
        .build()
        .map_err(|_| AuthorityError::InvalidRequest)?;
    validate_request(&request, location)?;
    let response = client
        .execute(request)
        .await
        .map_err(|_| AuthorityError::Unavailable)?;
    if !response.status().is_success() {
        return Err(AuthorityError::Unavailable);
    }
    let bytes = super::read_bounded(response, MAX_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            super::ProbeError::ResponseTooLarge => AuthorityError::ResponseLimit,
            _ => AuthorityError::Unavailable,
        })?;
    decode_entries(&bytes)
}

#[cfg(not(test))]
fn registered_database_candidates() -> Vec<PathBuf> {
    antiburn_local::discovery::agents::opencode::native_database_paths()
        .into_iter()
        .filter(|path| path.is_file())
        .collect()
}

fn validate_request(request: &reqwest::Request, location: &Path) -> Result<(), AuthorityError> {
    let url = request.url();
    let query = url.query_pairs().collect::<Vec<_>>();
    if request.method() != reqwest::Method::GET
        || request.body().is_some()
        || !antiburn_local::opencode_registration::is_loopback_http_url(url)
        || url.path() != "/api/config"
        || query.len() != 1
        || query[0].0 != "location[directory]"
        || query[0].1.as_ref() != location.to_str().ok_or(AuthorityError::InvalidRequest)?
    {
        return Err(AuthorityError::InvalidRequest);
    }
    Ok(())
}

fn decode_entries(bytes: &[u8]) -> Result<Vec<ConfigEntry>, AuthorityError> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(AuthorityError::ResponseLimit);
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| AuthorityError::InvalidResponse)?;
    let items = value
        .as_array()
        .filter(|items| items.len() <= MAX_ENTRIES)
        .ok_or(AuthorityError::InvalidResponse)?;
    items
        .iter()
        .map(|item| {
            let path = match item.get("path") {
                Some(value) => Some(
                    value
                        .as_str()
                        .map(PathBuf::from)
                        .filter(|path| path.is_absolute())
                        .ok_or(AuthorityError::InvalidResponse)?,
                ),
                None => None,
            };
            match item.get("type").and_then(Value::as_str) {
                Some("document") => Ok(ConfigEntry {
                    path,
                    info: Some(
                        item.get("info")
                            .filter(|info| info.is_object())
                            .ok_or(AuthorityError::InvalidResponse)?
                            .clone(),
                    ),
                }),
                Some("directory") if path.is_some() => Ok(ConfigEntry { path, info: None }),
                _ => Err(AuthorityError::InvalidResponse),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn consumer_test_helpers_do_not_discover_a_real_service() {
        assert!(VerifiedConfigConsumer::registered_async().await.is_err());
        let temporary = tempfile::tempdir().unwrap();
        assert!(
            CurrentConfigAuthority::registered_async(temporary.path())
                .await
                .is_err()
        );
        let seed = VerifiedConfigConsumer::fixture(temporary.path());
        assert!(seed.retained_bytes() > 0);
        let expired =
            CurrentConfigAuthority::fixture(temporary.path(), json!([])).expired_fixture();
        assert!(expired.is_expired());
        assert!(expired.entries().is_err());
    }

    fn client() -> Client {
        super::super::local_client()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap()
    }

    #[test]
    fn authenticated_config_request_is_exact_and_credentials_stay_private() {
        let temporary = tempfile::tempdir().unwrap();
        let registration = super::super::decode_registration(
            &serde_json::to_vec(&json!({
                "id":"fixture-service", "version":"2.0.32", "pid":1,
                "url":"http://127.0.0.1:32123", "password":"fixture-private-password"
            }))
            .unwrap(),
        )
        .unwrap();
        let client = client();
        let request = config_request(&client, &registration, temporary.path())
            .unwrap()
            .build()
            .unwrap();
        validate_request(&request, temporary.path()).unwrap();
        assert!(
            request
                .headers()
                .contains_key(reqwest::header::AUTHORIZATION)
        );
        assert!(!request.url().as_str().contains("fixture-private-password"));
        let receipt = CurrentConfigAuthority::fixture(
            temporary.path(),
            json!([{"type":"document","info":{"providers":{"test":{"settings":{"apiKey":"fixture-private-key"}}}}}]),
        );
        assert!(!format!("{receipt:?}").contains("fixture-private"));
    }

    #[test]
    fn malformed_pathless_and_bounded_entries_have_explicit_results() {
        assert!(decode_entries(br#"[{"type":"document","path":"relative","info":{}}]"#).is_err());
        assert!(decode_entries(br#"[{"type":"directory"}]"#).is_err());
        assert!(
            decode_entries(br#"[{"type":"document","info":{}}]"#).unwrap()[0]
                .path
                .is_none()
        );
        assert_eq!(
            decode_entries(&vec![b' '; MAX_RESPONSE_BYTES + 1]).err(),
            Some(AuthorityError::ResponseLimit)
        );
        let many = json!(vec![json!({"type":"document","info":{}}); MAX_ENTRIES + 1]);
        assert!(decode_entries(&serde_json::to_vec(&many).unwrap()).is_err());
    }

    #[test]
    fn request_validation_rejects_wrong_location_routes_and_writes() {
        let temporary = tempfile::tempdir().unwrap();
        let client = client();
        for endpoint in [
            "http://127.0.0.1:32123/api/event",
            "http://example.invalid:32123/api/config",
            "http://127.0.0.1:32123/api/config?location%5Bdirectory%5D=wrong",
        ] {
            assert_eq!(
                validate_request(&client.get(endpoint).build().unwrap(), temporary.path()),
                Err(AuthorityError::InvalidRequest)
            );
        }
        assert!(
            validate_request(
                &client
                    .post("http://127.0.0.1:32123/api/config")
                    .build()
                    .unwrap(),
                temporary.path()
            )
            .is_err()
        );
    }

    #[test]
    fn no_registration_cannot_mint_current_config_authority() {
        let temporary = tempfile::tempdir().unwrap();
        assert!(matches!(
            CurrentConfigAuthority::capture(&ConfigAuthorityRequest {
                registration_path: temporary.path().join("missing.json"),
                database_path: temporary.path().join("missing.db"),
                location: temporary.path().into()
            }),
            Err(AuthorityError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn no_database_candidates_cannot_mint_current_config_authority() {
        let temporary = tempfile::tempdir().unwrap();
        assert_eq!(
            CurrentConfigAuthority::registered_from_candidates(
                &temporary.path().join("missing.json"),
                &[],
                temporary.path(),
                Instant::now(),
            )
            .await
            .err(),
            Some(AuthorityError::Unavailable)
        );
    }

    #[tokio::test]
    async fn config_get_uses_authentication_and_rejects_redirects_and_large_responses() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        for (status, headers, body, expected) in [
            ("200 OK", "", "[{\"type\":\"document\",\"info\":{}}]", None),
            (
                "302 Found",
                "Content-Length: 0\r\nLocation: http://127.0.0.1:9/not-followed\r\n",
                "",
                Some(AuthorityError::Unavailable),
            ),
            (
                "200 OK",
                "Content-Length: 4194305\r\n",
                "",
                Some(AuthorityError::ResponseLimit),
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 16384);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with("GET /api/config?location%5Bdirectory%5D="));
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: basic ")
                );
                stream
                    .write_all(
                        format!("HTTP/1.1 {status}\r\n{headers}Connection: close\r\n\r\n{body}")
                            .as_bytes(),
                    )
                    .unwrap();
            });
            let registration = super::super::decode_registration(&serde_json::to_vec(&json!({"id":"fixture", "pid":1, "version":"2.0.32", "url":endpoint,"password":"fixture-password"})).unwrap()).unwrap();
            let client = client();
            let temporary = tempfile::tempdir().unwrap();
            let result = fetch_config(&client, &registration, temporary.path()).await;
            server.join().unwrap();
            match expected {
                Some(error) => assert_eq!(result.err(), Some(error)),
                None => assert_eq!(result.unwrap().len(), 1),
            }
        }
    }

    #[tokio::test]
    async fn config_response_limits_apply_without_a_content_length() {
        use std::{io::Write, net::TcpListener};

        for chunked in [false, true] {
            for oversized in [false, true] {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let endpoint = format!("http://{}", listener.local_addr().unwrap());
                let server = std::thread::spawn(move || {
                    use std::io::Read;

                    let (mut socket, _) = listener.accept().unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        socket.read_exact(&mut byte).unwrap();
                        request.push(byte[0]);
                        assert!(request.len() < 16 * 1024);
                    }
                    let mut body = String::from("[]");
                    body.push_str(&" ".repeat(MAX_RESPONSE_BYTES - 2 + usize::from(oversized)));
                    let response = if chunked {
                        format!(
                            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
                            body.len()
                        )
                    } else {
                        format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{body}")
                    };
                    let _ = socket.write_all(response.as_bytes());
                });
                let registration = super::super::decode_registration(
                    &serde_json::to_vec(&json!({"pid":1, "url":endpoint})).unwrap(),
                )
                .unwrap();
                let temporary = tempfile::tempdir().unwrap();
                let result = fetch_config(&client(), &registration, temporary.path()).await;
                server.join().unwrap();
                if oversized {
                    assert_eq!(result.err(), Some(AuthorityError::ResponseLimit));
                } else {
                    assert!(result.unwrap().is_empty());
                }
            }
        }
    }
}

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use antiburn_local::analysis::jev::{
    JevError, JevRequest, JevResponse, JevResponseObservation, MAX_REQUEST_TOKENS,
    capabilities::ModelCapabilities, estimate_jev_request_tokens,
};

use crate::jev::client::TypeSafeClient;
use crate::jev::config::{
    SystemOneConnection, SystemOneEndpoint, SystemOneProvider, SystemOneResponseMode,
};

/// Holds only the transport selected before preparation. Never derive Debug.
pub(super) enum DecisionTransport {
    Jev {
        client: TypeSafeClient,
        endpoint: String,
    },
    Ollama(crate::jev_ollama::OllamaClient),
    Cloudflare(crate::jev_cloudflare::CloudflareClient),
    Custom {
        endpoint: String,
        credential: Option<String>,
        mode: SystemOneResponseMode,
    },
}

impl DecisionTransport {
    pub(super) fn from_connection(
        connection: &SystemOneConnection,
        credential: Option<String>,
    ) -> Result<Self, JevError> {
        let endpoint = connection
            .inference_endpoint()
            .map_err(|_| JevError::InvalidRequestSchema)?;
        match (&connection.provider, &connection.endpoint) {
            (SystemOneProvider::Jev, _) => Ok(Self::Jev {
                client: TypeSafeClient::new(credential.ok_or(JevError::AuthenticationRejected)?)?,
                endpoint,
            }),
            (SystemOneProvider::Ollama, SystemOneEndpoint::BaseUrl(base)) => {
                crate::jev_ollama::OllamaClient::new(base, credential)
                    .map(Self::Ollama)
                    .map_err(ollama_error)
            }
            (SystemOneProvider::Cloudflare, SystemOneEndpoint::CloudflareAccount(account)) => {
                crate::jev_cloudflare::CloudflareClient::new(
                    account.clone(),
                    credential.ok_or(JevError::AuthenticationRejected)?,
                    connection.model.clone(),
                )
                .map(Self::Cloudflare)
            }
            (SystemOneProvider::Custom, _) => Ok(Self::Custom {
                endpoint,
                credential,
                mode: connection.response_mode,
            }),
            _ => Err(JevError::InvalidRequestSchema),
        }
    }

    pub(super) fn provider(&self) -> SystemOneProvider {
        match self {
            Self::Jev { .. } => SystemOneProvider::Jev,
            Self::Ollama(_) => SystemOneProvider::Ollama,
            Self::Cloudflare(_) => SystemOneProvider::Cloudflare,
            Self::Custom { .. } => SystemOneProvider::Custom,
        }
    }

    async fn evaluate(
        &self,
        request: &JevRequest,
        capabilities: &ModelCapabilities,
        is_cancelled: &(impl Fn() -> bool + Sync),
        delivery_possible: &AtomicBool,
        record_usage: impl Fn(JevResponseObservation) -> Result<(), JevError>,
    ) -> Result<JevResponse, JevError> {
        match self {
            Self::Jev { client, endpoint } => {
                client
                    .evaluate_at_with_capabilities_and_usage(
                        request,
                        endpoint,
                        capabilities,
                        record_usage,
                    )
                    .await
            }
            Self::Ollama(client) => {
                client
                    .evaluate_with_usage_cancellable(
                        request,
                        capabilities,
                        is_cancelled,
                        || delivery_possible.store(true, Ordering::Release),
                        |usage| {
                            record_usage(JevResponseObservation {
                                usage,
                                served_model: None,
                            })
                        },
                    )
                    .await
            }
            Self::Cloudflare(client) => {
                client
                    .evaluate_with_capabilities_and_usage(request, capabilities, |usage| {
                        record_usage(JevResponseObservation {
                            usage,
                            served_model: None,
                        })
                    })
                    .await
            }
            Self::Custom {
                endpoint,
                credential,
                mode,
            } => {
                super::client::evaluate_custom_with_usage(
                    endpoint,
                    credential.as_deref(),
                    *mode,
                    request,
                    capabilities,
                    |usage| {
                        record_usage(JevResponseObservation {
                            usage,
                            served_model: None,
                        })
                    },
                )
                .await
            }
        }
    }
}

fn ollama_error(error: crate::jev_ollama::OllamaError) -> JevError {
    use crate::jev_ollama::OllamaError;
    match error {
        OllamaError::AuthenticationRejected => JevError::AuthenticationRejected,
        OllamaError::RequestOutcomeUnknown => JevError::RequestOutcomeUnknown,
        OllamaError::ContextRejected => JevError::ContextRejected,
        OllamaError::ResponseDecode => JevError::ResponseDecode,
        OllamaError::ResponseTooLarge => JevError::ResponseTooLarge,
        OllamaError::ProviderUnavailable => JevError::ProviderUnavailable,
        OllamaError::OldVersion | OllamaError::MissingModel | OllamaError::UnsupportedRunner => {
            JevError::UnsupportedModel
        }
        OllamaError::InvalidBaseUrl
        | OllamaError::InvalidRequest
        | OllamaError::RequestBodyTooLarge => JevError::InvalidRequestSchema,
    }
}

pub(super) const GLOBAL_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_DEADLINE: Duration = Duration::from_secs(60);
const GLOBAL_INPUT_TOKENS_PER_SECOND: u64 = 100_000;
// Keep this conservative estimate. Do not infer undocumented provider limits.
const PROVIDER_START_INTERVAL: Duration = Duration::from_millis(25);

pub(super) fn request_bytes() -> &'static tokio::sync::Semaphore {
    static BYTES: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| tokio::sync::Semaphore::new(GLOBAL_REQUEST_BYTES))
}

pub(super) fn request_slots() -> &'static tokio::sync::Semaphore {
    // The engine uses a separate semaphore; desktop request paths share this one.
    static SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| {
        tokio::sync::Semaphore::new(antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS)
    })
}

struct TokenStart {
    reservation_id: String,
    started: tokio::time::Instant,
    tokens: u64,
}

pub(super) struct RollingPacing {
    next_start: tokio::time::Instant,
    cooldown_until: tokio::time::Instant,
    starts: std::collections::VecDeque<TokenStart>,
}

impl RollingPacing {
    fn new(now: tokio::time::Instant) -> Self {
        Self {
            next_start: now,
            cooldown_until: now,
            starts: std::collections::VecDeque::new(),
        }
    }

    pub(super) fn admit(
        &mut self,
        reservation_id: &str,
        estimated_tokens: u64,
        now: tokio::time::Instant,
    ) -> bool {
        while self.starts.front().is_some_and(|start| {
            now.saturating_duration_since(start.started) >= Duration::from_secs(1)
        }) {
            self.starts.pop_front();
        }
        let tokens = self.starts.iter().map(|start| start.tokens).sum::<u64>();
        let estimated_tokens = estimated_tokens.clamp(1, MAX_REQUEST_TOKENS);
        if now < self.cooldown_until
            || now < self.next_start
            || tokens.saturating_add(estimated_tokens) > GLOBAL_INPUT_TOKENS_PER_SECOND
        {
            return false;
        }
        self.starts.push_back(TokenStart {
            reservation_id: reservation_id.to_owned(),
            started: now,
            tokens: estimated_tokens,
        });
        self.next_start = now + PROVIDER_START_INTERVAL;
        true
    }

    pub(super) fn next_eligible(
        &mut self,
        estimated_tokens: u64,
        now: tokio::time::Instant,
    ) -> tokio::time::Instant {
        while self.starts.front().is_some_and(|start| {
            now.saturating_duration_since(start.started) >= Duration::from_secs(1)
        }) {
            self.starts.pop_front();
        }
        let estimated_tokens = estimated_tokens.clamp(1, MAX_REQUEST_TOKENS);
        let mut used_tokens = self.starts.iter().map(|start| start.tokens).sum::<u64>();
        let mut deadline = self.cooldown_until.max(self.next_start);
        for start in &self.starts {
            if used_tokens.saturating_add(estimated_tokens) <= GLOBAL_INPUT_TOKENS_PER_SECOND {
                break;
            }
            used_tokens = used_tokens.saturating_sub(start.tokens);
            deadline = deadline.max(start.started + Duration::from_secs(1));
        }
        deadline.max(now)
    }

    pub(super) fn settle(&mut self, reservation_id: &str, tokens: u64) {
        if let Some(start) = self
            .starts
            .iter_mut()
            .find(|start| start.reservation_id == reservation_id)
        {
            start.tokens = tokens.min(MAX_REQUEST_TOKENS);
        }
    }

    pub(super) fn defer(&mut self, now: tokio::time::Instant, duration: Duration) {
        self.cooldown_until = self.cooldown_until.max(now + duration);
    }
}

pub(super) struct NativePacing {
    next_start: tokio::time::Instant,
    cooldown_until: tokio::time::Instant,
}

impl NativePacing {
    fn new(now: tokio::time::Instant) -> Self {
        Self {
            next_start: now,
            cooldown_until: now,
        }
    }

    fn admit(&mut self, tokens: u64, now: tokio::time::Instant) -> bool {
        if now < self.next_eligible(now) {
            return false;
        }
        let nanos = tokens
            .saturating_mul(1_000_000_000)
            .div_ceil(GLOBAL_INPUT_TOKENS_PER_SECOND);
        let interval = Duration::from_nanos(nanos).max(PROVIDER_START_INTERVAL);
        self.next_start = now + interval;
        true
    }

    fn next_eligible(&self, now: tokio::time::Instant) -> tokio::time::Instant {
        self.next_start.max(self.cooldown_until).max(now)
    }
}

pub(super) enum ProviderPacing {
    Native(NativePacing),
    Standard(RollingPacing),
}

impl ProviderPacing {
    fn new(now: tokio::time::Instant) -> Self {
        Self::Standard(RollingPacing::new(now))
    }

    fn admit(&mut self, id: &str, tokens: u64, now: tokio::time::Instant) -> bool {
        match self {
            Self::Native(pacing) => pacing.admit(tokens, now),
            Self::Standard(pacing) => pacing.admit(id, tokens, now),
        }
    }

    fn next_eligible(&mut self, tokens: u64, now: tokio::time::Instant) -> tokio::time::Instant {
        match self {
            Self::Native(pacing) => pacing.next_eligible(now),
            Self::Standard(pacing) => pacing.next_eligible(tokens, now),
        }
    }

    fn settle(&mut self, id: &str, tokens: u64) {
        if let Self::Standard(pacing) = self {
            pacing.settle(id, tokens);
        }
    }

    fn defer(&mut self, now: tokio::time::Instant, duration: Duration) {
        match self {
            Self::Native(pacing) => {
                pacing.cooldown_until = pacing.cooldown_until.max(now + duration)
            }
            Self::Standard(pacing) => pacing.defer(now, duration),
        }
    }
}

fn native_admission_gate() -> &'static tokio::sync::Semaphore {
    static GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    &GATE
}

struct ProviderPacingSet {
    jev: std::sync::Mutex<ProviderPacing>,
    ollama: std::sync::Mutex<ProviderPacing>,
    cloudflare: std::sync::Mutex<ProviderPacing>,
    custom: std::sync::Mutex<ProviderPacing>,
}

impl ProviderPacingSet {
    fn new(now: tokio::time::Instant) -> Self {
        Self {
            jev: std::sync::Mutex::new(ProviderPacing::Native(NativePacing::new(now))),
            ollama: std::sync::Mutex::new(ProviderPacing::new(now)),
            cloudflare: std::sync::Mutex::new(ProviderPacing::new(now)),
            custom: std::sync::Mutex::new(ProviderPacing::new(now)),
        }
    }
}

pub(super) fn provider_pacing(
    provider: SystemOneProvider,
) -> &'static std::sync::Mutex<ProviderPacing> {
    static PACING: std::sync::OnceLock<ProviderPacingSet> = std::sync::OnceLock::new();
    let pacing = PACING.get_or_init(|| ProviderPacingSet::new(tokio::time::Instant::now()));
    match provider {
        SystemOneProvider::Jev => &pacing.jev,
        SystemOneProvider::Ollama => &pacing.ollama,
        SystemOneProvider::Cloudflare => &pacing.cloudflare,
        SystemOneProvider::Custom => &pacing.custom,
    }
}

pub(super) fn try_admit_provider_request(
    provider: SystemOneProvider,
    reservation_id: &str,
    estimated_tokens: u64,
    now: tokio::time::Instant,
) -> (bool, tokio::time::Instant) {
    let mut pacing = provider_pacing(provider)
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let admitted = pacing.admit(reservation_id, estimated_tokens, now);
    let next_eligible = pacing.next_eligible(estimated_tokens, now);
    (admitted, next_eligible)
}

pub(super) fn apply_provider_cooldown(
    pacing: &mut ProviderPacing,
    error: &JevError,
    now: tokio::time::Instant,
) {
    if let Some(delay) = provider_cooldown_delay(error) {
        pacing.defer(now, delay);
    }
}

pub(super) fn provider_cooldown_delay(error: &JevError) -> Option<Duration> {
    match error {
        JevError::RateLimited { retry_after } | JevError::ProviderOverloaded { retry_after } => {
            Some(retry_after.unwrap_or_default().max(Duration::from_secs(5)))
        }
        _ => None,
    }
}

pub(super) async fn request_with_deadline(
    call: impl Future<Output = Result<JevResponse, JevError>>,
) -> Result<JevResponse, JevError> {
    tokio::time::timeout(REQUEST_DEADLINE, call)
        .await
        .unwrap_or(Err(JevError::RequestOutcomeUnknown))
}

/// Sends one attempt and records known usage before response validation.
/// Known-unsent failures record no usage. Uncertain delivery records unknown usage.
/// Admission and transport each have a separate 60-second shared deadline.
/// The transport deadline includes the Ollama inference-slot wait.
pub(super) async fn dispatch_decision_attempt(
    transport: &DecisionTransport,
    request: &JevRequest,
    capabilities: &ModelCapabilities,
    resident_bytes: usize,
    request_id: &str,
    is_cancelled: impl Fn() -> bool + Sync,
    record_usage: impl Fn(Option<JevResponseObservation>) -> Result<(), JevError> + Sync,
) -> Result<JevResponse, JevError> {
    let byte_count = u32::try_from(resident_bytes).map_err(|_| JevError::RequestSerialization)?;
    if byte_count == 0 || resident_bytes > GLOBAL_REQUEST_BYTES {
        return Err(JevError::RequestTooLarge {
            bytes: resident_bytes,
            maximum: GLOBAL_REQUEST_BYTES,
        });
    }
    let provider = transport.provider();
    let reservation_id = format!("decision:{request_id}");
    let (_slot, _bytes) = tokio::time::timeout(REQUEST_DEADLINE, async {
        let _native_gate = if provider == SystemOneProvider::Jev {
            Some(acquire_neutral_budget(native_admission_gate().acquire(), &is_cancelled).await?)
        } else {
            None
        };
        let acquire = request_slots().acquire();
        #[cfg(test)]
        let acquire = test_hooks::observe_request_slot_pending(request_id, acquire);
        let slot = acquire_neutral_budget(acquire, &is_cancelled).await?;
        let bytes =
            acquire_neutral_budget(request_bytes().acquire_many(byte_count), &is_cancelled).await?;
        let estimated_tokens = if provider == SystemOneProvider::Jev {
            antiburn_local::analysis::jev::validate_jev_request_with_capabilities(
                request,
                capabilities,
            )?;
            estimate_jev_request_tokens(request)?
                .saturating_add(capabilities.rendering_reserve_tokens)
        } else {
            let request_body_bytes =
                antiburn_local::analysis::jev::validate_jev_request_with_capabilities(
                    request,
                    capabilities,
                )?;
            u64::try_from(request_body_bytes.div_ceil(3))
                .unwrap_or(MAX_REQUEST_TOKENS)
                .min(MAX_REQUEST_TOKENS)
        };
        wait_for_provider_neutral(provider, &reservation_id, estimated_tokens, &is_cancelled)
            .await?;
        Ok::<_, JevError>((slot, bytes))
    })
    .await
    .map_err(|_| JevError::ProviderUnavailable)??;
    if is_cancelled() {
        provider_pacing(provider)
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .settle(&reservation_id, 0);
        return Err(JevError::Cancelled);
    }

    let usage_received = AtomicBool::new(false);
    // Only Ollama reports the send boundary after local inference-slot admission.
    let delivery_possible = AtomicBool::new(provider != SystemOneProvider::Ollama);
    let call = request_with_deadline(transport.evaluate(
        request,
        capabilities,
        &is_cancelled,
        &delivery_possible,
        |usage| {
            usage_received.store(true, Ordering::Relaxed);
            provider_pacing(provider)
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .settle(&reservation_id, usage.usage.input_tokens);
            record_usage(Some(usage))
        },
    ));
    let result = wait_for_decision_response(call, &is_cancelled).await;
    let delivery_possible = delivery_possible.load(Ordering::Acquire);
    let result = match result {
        Err(JevError::RequestOutcomeUnknown) if !delivery_possible => Err(if is_cancelled() {
            JevError::Cancelled
        } else {
            JevError::ProviderUnavailable
        }),
        result => result,
    };
    if let Err(error) = &result {
        {
            let mut pacing = provider_pacing(provider)
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            apply_provider_cooldown(&mut pacing, error, tokio::time::Instant::now());
            if !delivery_possible
                || matches!(
                    error,
                    JevError::Cancelled
                        | JevError::RateLimited { .. }
                        | JevError::ProviderOverloaded { .. }
                )
            {
                pacing.settle(&reservation_id, 0);
            }
        }
        if !usage_received.load(Ordering::Relaxed) && usage_is_unknown(error) {
            record_usage(None)?;
        }
    }
    result
}

fn usage_is_unknown(error: &JevError) -> bool {
    matches!(
        error,
        JevError::RequestOutcomeUnknown
            | JevError::ResponseTooLarge
            | JevError::ResponseDecode
            | JevError::ResponseModelMismatch
            | JevError::ResponseAnswerCountMismatch
            | JevError::ResponseAnswerMissing
            | JevError::ResponseAnswerTypeMismatch
            | JevError::InvalidChoiceDistribution
            | JevError::InvalidNoulProbability
            | JevError::InvalidScoreDistribution
            | JevError::InvalidProbabilitySum
            | JevError::ResponseUsageExceeded
    )
}

async fn wait_for_decision_response(
    call: impl Future<Output = Result<JevResponse, JevError>>,
    is_cancelled: &impl Fn() -> bool,
) -> Result<JevResponse, JevError> {
    tokio::pin!(call);
    loop {
        tokio::select! {
            biased;
            result = &mut call => return result,
            () = tokio::time::sleep(Duration::from_millis(100)) => {
                if is_cancelled() {
                    return Err(JevError::RequestOutcomeUnknown);
                }
            }
        }
    }
}

async fn acquire_neutral_budget<T>(
    acquire: impl Future<Output = Result<T, tokio::sync::AcquireError>>,
    is_cancelled: &impl Fn() -> bool,
) -> Result<T, JevError> {
    tokio::pin!(acquire);
    loop {
        if is_cancelled() {
            return Err(JevError::Cancelled);
        }
        tokio::select! {
            permit = &mut acquire => return permit.map_err(|_| JevError::Cancelled),
            () = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    }
}

async fn wait_for_provider_neutral(
    provider: SystemOneProvider,
    reservation_id: &str,
    estimated_tokens: u64,
    is_cancelled: &impl Fn() -> bool,
) -> Result<(), JevError> {
    loop {
        if is_cancelled() {
            return Err(JevError::Cancelled);
        }
        let now = tokio::time::Instant::now();
        let (admitted, next_eligible) =
            try_admit_provider_request(provider, reservation_id, estimated_tokens, now);
        if admitted {
            return Ok(());
        }
        tokio::select! {
            () = tokio::time::sleep_until(next_eligible) => {},
            () = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    }
}

#[cfg(test)]
pub(crate) mod test_hooks {
    use std::future::{Future, poll_fn};
    use std::sync::Arc;

    struct RequestSlotPending {
        request_id: String,
        on_pending: Arc<dyn Fn() + Send + Sync>,
    }

    tokio::task_local! {
        static REQUEST_SLOT_PENDING: RequestSlotPending;
    }

    pub(crate) async fn with_request_slot_pending<T>(
        request_id: String,
        on_pending: impl Fn() + Send + Sync + 'static,
        run: impl Future<Output = T>,
    ) -> T {
        REQUEST_SLOT_PENDING
            .scope(
                RequestSlotPending {
                    request_id,
                    on_pending: Arc::new(on_pending),
                },
                run,
            )
            .await
    }

    pub(super) async fn observe_request_slot_pending<T>(
        request_id: &str,
        acquire: impl Future<Output = T>,
    ) -> T {
        let mut on_pending = REQUEST_SLOT_PENDING
            .try_with(|hook| (hook.request_id == request_id).then(|| Arc::clone(&hook.on_pending)))
            .ok()
            .flatten();
        tokio::pin!(acquire);
        poll_fn(|cx| {
            let result = acquire.as_mut().poll(cx);
            if result.is_pending()
                && let Some(on_pending) = on_pending.take()
            {
                on_pending();
            }
            result
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native_dispatch_request(name: &str, state_bytes: usize) -> JevRequest {
        use antiburn_local::analysis::jev::{JevQuestion, NATIVE_MODEL_ALIAS};
        use std::collections::BTreeMap;

        JevRequest {
            model: NATIVE_MODEL_ALIAS.into(),
            state: serde_json::json!({"id": name, "text": "a".repeat(state_bytes)}),
            questions: BTreeMap::from([(
                "q".into(),
                JevQuestion::Choice {
                    instructions: serde_json::json!("Choose yes or no."),
                    criteria: BTreeMap::from([
                        ("yes".into(), serde_json::json!("Yes")),
                        ("no".into(), serde_json::json!("No")),
                    ]),
                },
            )]),
        }
    }

    fn native_loopback_transport(listener: &std::net::TcpListener) -> DecisionTransport {
        DecisionTransport::Jev {
            client: TypeSafeClient::new("synthetic-key".into()).unwrap(),
            endpoint: format!("http://{}/v1/systemone", listener.local_addr().unwrap()),
        }
    }

    async fn assert_dispatch_pending(mut call: std::pin::Pin<&mut impl Future>) {
        std::future::poll_fn(|cx| {
            assert!(call.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
    }

    fn read_native_dispatch_request(socket: &mut std::net::TcpStream) -> String {
        use std::io::Read;

        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0; 8192];
            let count = socket.read(&mut chunk).unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&chunk[..count]);
            assert!(bytes.len() <= 832 * 1024);
            let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            if bytes.len() < end + 4 + length {
                continue;
            }
            assert!(headers.starts_with("post /v1/systemone http/1.1\r\n"));
            assert!(
                headers
                    .lines()
                    .any(|line| line == "authorization: bearer synthetic-key")
            );
            assert!(
                headers
                    .lines()
                    .any(|line| line == "typesafe-danger-zone-long-context: true")
            );
            let body: serde_json::Value =
                serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
            assert_eq!(
                body["model"],
                antiburn_local::analysis::jev::NATIVE_MODEL_ALIAS
            );
            return body["state"]["id"].as_str().unwrap().to_owned();
        }
    }

    fn write_native_dispatch_response(socket: &mut std::net::TcpStream) {
        use std::io::Write;

        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let body = serde_json::json!({
            "model": "jev-1.14.0",
            "answers": {"q": {"type": "choice", "choice": "yes", "probabilities": {"yes": 0.9, "no": 0.1}, "confidence": 0.8}},
            "usage": {"input_tokens": 12, "output_tokens": 1}
        })
        .to_string();
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    }

    #[tokio::test]
    async fn native_dispatch_releases_gate_before_io_and_preserves_fifo_after_waiter_cancellation()
    {
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;

        #[derive(Default)]
        struct AttemptState {
            cancelled: AtomicBool,
            cancellation_checks: AtomicUsize,
            observations: std::sync::Mutex<Vec<Option<JevResponseObservation>>>,
        }

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
        let (seen, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let (release_first, first_response) = std::sync::mpsc::sync_channel(1);
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut first = None;
            let mut order = Vec::new();
            for index in 0..4 {
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(std::time::Instant::now() < deadline);
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("loopback accept failed: {error}"),
                    }
                };
                let name = read_native_dispatch_request(&mut socket);
                order.push(name.clone());
                seen.send(name).unwrap();
                if index == 0 {
                    first = Some(socket);
                } else {
                    write_native_dispatch_response(&mut socket);
                }
            }
            first_response.recv_timeout(Duration::from_secs(5)).unwrap();
            write_native_dispatch_response(&mut first.unwrap());
            order
        });

        *provider_pacing(SystemOneProvider::Jev).lock().unwrap() =
            ProviderPacing::Native(NativePacing::new(tokio::time::Instant::now()));
        let resident_bytes = 832 * 1024;
        let attempt = |name: &'static str, state_bytes, state: Arc<AttemptState>| {
            let endpoint = endpoint.clone();
            async move {
                let transport = DecisionTransport::Jev {
                    client: TypeSafeClient::new("synthetic-key".into()).unwrap(),
                    endpoint,
                };
                dispatch_decision_attempt(
                    &transport,
                    &native_dispatch_request(name, state_bytes),
                    &ModelCapabilities::jev_native(),
                    resident_bytes,
                    name,
                    || {
                        state.cancellation_checks.fetch_add(1, Ordering::AcqRel);
                        state.cancelled.load(Ordering::Acquire)
                    },
                    |observation| {
                        state.observations.lock().unwrap().push(observation);
                        Ok(())
                    },
                )
                .await
            }
        };
        let first_state = Arc::new(AttemptState::default());
        let first = tokio::spawn(attempt("first", 320_000, first_state.clone()));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), requests.recv())
                .await
                .unwrap()
                .as_deref(),
            Some("first")
        );
        let gate = native_admission_gate()
            .try_acquire()
            .expect("provider I/O must not hold the native gate");
        drop(gate);
        assert!(!first.is_finished());

        let large_state = Arc::new(AttemptState::default());
        let mut large = Box::pin(attempt("large", 60_000, large_state.clone()));
        assert_dispatch_pending(large.as_mut()).await;
        let large = tokio::spawn(large);
        let cancelled_state = Arc::new(AttemptState::default());
        let mut cancelled = Box::pin(attempt("cancelled", 0, cancelled_state.clone()));
        assert_dispatch_pending(cancelled.as_mut()).await;
        let cancelled = tokio::spawn(cancelled);
        let later_large_state = Arc::new(AttemptState::default());
        let mut later_large = Box::pin(attempt("later-large", 60_000, later_large_state.clone()));
        assert_dispatch_pending(later_large.as_mut()).await;
        let later_large = tokio::spawn(later_large);
        let small_state = Arc::new(AttemptState::default());
        let mut small = Box::pin(attempt("small", 0, small_state.clone()));
        assert_dispatch_pending(small.as_mut()).await;
        let small = tokio::spawn(small);

        tokio::time::sleep(Duration::from_millis(350)).await;
        for state in [&cancelled_state, &later_large_state, &small_state] {
            assert!(state.cancellation_checks.load(Ordering::Acquire) >= 4);
        }
        assert!(requests.try_recv().is_err());
        assert_eq!(native_admission_gate().available_permits(), 0);
        assert_eq!(
            request_slots().available_permits(),
            antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS - 2
        );
        assert_eq!(
            request_bytes().available_permits(),
            GLOBAL_REQUEST_BYTES - 2 * resident_bytes
        );
        cancelled_state.cancelled.store(true, Ordering::Release);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), cancelled)
                .await
                .unwrap()
                .unwrap(),
            Err(JevError::Cancelled)
        );
        assert!(cancelled_state.observations.lock().unwrap().is_empty());
        assert_eq!(
            request_slots().available_permits(),
            antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS - 2
        );
        assert_eq!(
            request_bytes().available_permits(),
            GLOBAL_REQUEST_BYTES - 2 * resident_bytes
        );

        for (name, call) in [
            ("large", large),
            ("later-large", later_large),
            ("small", small),
        ] {
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(5), requests.recv())
                    .await
                    .unwrap()
                    .as_deref(),
                Some(name)
            );
            let response = tokio::time::timeout(Duration::from_secs(5), call)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(response.model, "jev-1.14.0");
        }
        assert!(!first.is_finished());
        assert!(first_state.observations.lock().unwrap().is_empty());
        assert_eq!(
            request_slots().available_permits(),
            antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS - 1
        );
        assert_eq!(
            request_bytes().available_permits(),
            GLOBAL_REQUEST_BYTES - resident_bytes
        );
        release_first.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), first)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            server.join().unwrap(),
            ["first", "large", "later-large", "small"]
        );
        for state in [&first_state, &large_state, &later_large_state, &small_state] {
            let observations = state.observations.lock().unwrap();
            assert_eq!(observations.len(), 1);
            assert_eq!(observations[0].as_ref().unwrap().usage.input_tokens, 12);
        }
        assert_eq!(native_admission_gate().available_permits(), 1);
        assert_eq!(
            request_slots().available_permits(),
            antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS
        );
        assert_eq!(request_bytes().available_permits(), GLOBAL_REQUEST_BYTES);
    }

    #[tokio::test(start_paused = true)]
    async fn native_dispatch_rechecks_queued_cooldown_and_releases_permits_after_cancellation() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let transport = native_loopback_transport(&listener);
        let request = native_dispatch_request("cooldown", 0);
        let capabilities = ModelCapabilities::jev_native();
        let now = tokio::time::Instant::now();
        let original_start = now + Duration::from_secs(1);
        *provider_pacing(SystemOneProvider::Jev).lock().unwrap() =
            ProviderPacing::Native(NativePacing {
                next_start: original_start,
                cooldown_until: now,
            });
        let cancelled = AtomicBool::new(false);
        let resident_bytes = 1024;
        let waiting = dispatch_decision_attempt(
            &transport,
            &request,
            &capabilities,
            resident_bytes,
            "cooldown",
            || cancelled.load(Ordering::Acquire),
            |_| panic!("cancelled admission must not record usage"),
        );
        tokio::pin!(waiting);
        assert_dispatch_pending(waiting.as_mut()).await;
        tokio::time::advance(Duration::from_millis(100)).await;
        assert_dispatch_pending(waiting.as_mut()).await;
        apply_provider_cooldown(
            &mut provider_pacing(SystemOneProvider::Jev).lock().unwrap(),
            &JevError::RateLimited { retry_after: None },
            tokio::time::Instant::now(),
        );
        tokio::time::advance(Duration::from_millis(900)).await;
        assert_dispatch_pending(waiting.as_mut()).await;
        tokio::time::advance(Duration::from_secs(4)).await;
        assert_dispatch_pending(waiting.as_mut()).await;
        {
            let pacing = provider_pacing(SystemOneProvider::Jev).lock().unwrap();
            let ProviderPacing::Native(pacing) = &*pacing else {
                panic!("native dispatch must use native pacing");
            };
            assert_eq!(pacing.next_start, original_start);
            assert_eq!(pacing.cooldown_until, now + Duration::from_millis(5100));
        }
        assert_eq!(native_admission_gate().available_permits(), 0);
        assert_eq!(
            request_slots().available_permits(),
            antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS - 1
        );
        assert_eq!(
            request_bytes().available_permits(),
            GLOBAL_REQUEST_BYTES - resident_bytes
        );
        cancelled.store(true, Ordering::Release);
        tokio::time::advance(Duration::from_millis(100)).await;
        assert_eq!(waiting.await, Err(JevError::Cancelled));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(native_admission_gate().available_permits(), 1);
        assert_eq!(
            request_slots().available_permits(),
            antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS
        );
        assert_eq!(request_bytes().available_permits(), GLOBAL_REQUEST_BYTES);
    }

    #[tokio::test(start_paused = true)]
    async fn native_dispatch_shared_admission_deadline_releases_gate_slot_and_resident_bytes() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let transport = native_loopback_transport(&listener);
        let request = native_dispatch_request("deadline", 0);
        let capabilities = ModelCapabilities::jev_native();
        let now = tokio::time::Instant::now();
        *provider_pacing(SystemOneProvider::Jev).lock().unwrap() =
            ProviderPacing::Native(NativePacing {
                next_start: now + 2 * REQUEST_DEADLINE,
                cooldown_until: now,
            });
        let gate = native_admission_gate().acquire().await.unwrap();
        let bytes = request_bytes()
            .acquire_many(GLOBAL_REQUEST_BYTES as u32)
            .await
            .unwrap();
        let resident_bytes = 1024;
        let waiting = dispatch_decision_attempt(
            &transport,
            &request,
            &capabilities,
            resident_bytes,
            "deadline",
            || false,
            |_| panic!("expired admission must not record usage"),
        );
        tokio::pin!(waiting);
        assert_dispatch_pending(waiting.as_mut()).await;
        assert_eq!(
            request_slots().available_permits(),
            antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS
        );
        tokio::time::advance(Duration::from_secs(20)).await;
        drop(gate);
        assert_dispatch_pending(waiting.as_mut()).await;
        assert_eq!(
            request_slots().available_permits(),
            antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS - 1
        );
        assert_eq!(request_bytes().available_permits(), 0);
        tokio::time::advance(Duration::from_secs(20)).await;
        drop(bytes);
        assert_dispatch_pending(waiting.as_mut()).await;
        assert_eq!(
            request_bytes().available_permits(),
            GLOBAL_REQUEST_BYTES - resident_bytes
        );
        tokio::time::advance(Duration::from_secs(19)).await;
        assert_dispatch_pending(waiting.as_mut()).await;
        assert_eq!(native_admission_gate().available_permits(), 0);
        assert_eq!(
            request_slots().available_permits(),
            antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS - 1
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(waiting.await, Err(JevError::ProviderUnavailable));
        assert_eq!(tokio::time::Instant::now(), now + REQUEST_DEADLINE);
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(native_admission_gate().available_permits(), 1);
        assert_eq!(
            request_slots().available_permits(),
            antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS
        );
        assert_eq!(request_bytes().available_permits(), GLOBAL_REQUEST_BYTES);
    }

    #[test]
    fn native_large_charges_use_constant_start_spacing_without_usage_refunds() {
        let now = tokio::time::Instant::now();
        let mut pacing = ProviderPacing::Native(NativePacing::new(now));
        assert!(pacing.admit("first", 262_000, now));
        assert!(!pacing.admit("second", 100_001, now));
        assert_eq!(
            pacing.next_eligible(100_001, now),
            now + Duration::from_millis(2620)
        );
        pacing.settle("first", 0);
        assert_eq!(
            pacing.next_eligible(100_001, now),
            now + Duration::from_millis(2620)
        );
        let second = now + Duration::from_millis(2620);
        assert!(pacing.admit("second", 100_001, second));
        assert_eq!(
            pacing.next_eligible(262_000, second),
            second + Duration::from_nanos(1_000_010_000)
        );
        assert!(pacing.next_eligible(262_000, second) > second);
        let next = pacing.next_eligible(262_000, second);
        assert!(pacing.admit("third", 262_000, next));
        pacing.defer(next, Duration::from_secs(5));
        assert_eq!(pacing.next_eligible(1, next), next + Duration::from_secs(5));
    }

    #[tokio::test(start_paused = true)]
    async fn native_spacing_keeps_maximum_requests_at_least_2620ms_apart() {
        let now = tokio::time::Instant::now();
        let mut pacing = NativePacing::new(now);
        assert!(pacing.admit(262_000, now));
        tokio::time::advance(Duration::from_millis(2619)).await;
        assert!(!pacing.admit(262_000, tokio::time::Instant::now()));
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(pacing.admit(262_000, tokio::time::Instant::now()));
    }

    #[tokio::test(start_paused = true)]
    async fn fifo_admission_keeps_queue_position_across_checks_and_cancelled_waiters() {
        let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let held = gate.acquire().await.unwrap();
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let order = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut waiters = Vec::new();
        for name in ["large", "cancelled", "small"] {
            let gate = gate.clone();
            let cancelled = cancelled.clone();
            let order = order.clone();
            waiters.push(tokio::spawn(async move {
                let permit = acquire_neutral_budget(gate.acquire(), &|| {
                    name == "cancelled" && cancelled.load(Ordering::Acquire)
                })
                .await?;
                order.lock().unwrap().push(name);
                drop(permit);
                Ok::<_, JevError>(())
            }));
            tokio::task::yield_now().await;
        }
        for _ in 0..4 {
            tokio::time::advance(Duration::from_millis(100)).await;
            tokio::task::yield_now().await;
        }
        cancelled.store(true, Ordering::Release);
        tokio::time::advance(Duration::from_millis(100)).await;
        tokio::task::yield_now().await;
        drop(held);
        let mut results = Vec::new();
        for waiter in waiters {
            results.push(waiter.await.unwrap());
        }
        assert_eq!(results, [Ok(()), Err(JevError::Cancelled), Ok(())]);
        assert_eq!(*order.lock().unwrap(), ["large", "small"]);
        assert_eq!(gate.available_permits(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn combined_admission_deadline_includes_fifo_and_resources_and_releases_permits() {
        let gate = tokio::sync::Semaphore::new(1);
        let slots = tokio::sync::Semaphore::new(1);
        let bytes = tokio::sync::Semaphore::new(0);
        let held = gate.acquire().await.unwrap();
        {
            let admission = tokio::time::timeout(REQUEST_DEADLINE, async {
                let _gate = acquire_neutral_budget(gate.acquire(), &|| false).await?;
                let _slot = acquire_neutral_budget(slots.acquire(), &|| false).await?;
                let _bytes = acquire_neutral_budget(bytes.acquire(), &|| false).await?;
                Ok::<_, JevError>(())
            });
            tokio::pin!(admission);
            std::future::poll_fn(|cx| {
                assert!(admission.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            tokio::time::advance(Duration::from_secs(59)).await;
            drop(held);
            std::future::poll_fn(|cx| {
                assert!(admission.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            assert_eq!(slots.available_permits(), 0);
            tokio::time::advance(Duration::from_secs(1)).await;
            assert!(admission.await.is_err());
        }
        assert_eq!(gate.available_permits(), 1);
        assert_eq!(slots.available_permits(), 1);
    }

    #[test]
    fn native_maximum_resident_charge_keeps_the_shared_four_mib_budget() {
        let bytes = tokio::sync::Semaphore::new(GLOBAL_REQUEST_BYTES);
        let charge = 832 * 1024;
        let mut permits = Vec::new();
        for _ in 0..4 {
            permits.push(bytes.try_acquire_many(charge).unwrap());
        }
        assert!(bytes.try_acquire_many(charge).is_err());
        drop(permits);
        assert_eq!(bytes.available_permits(), 4 * 1024 * 1024);
    }

    #[tokio::test]
    async fn request_slot_observer_reports_only_first_pending_for_its_scoped_request() {
        let pending = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = std::sync::Arc::clone(&pending);
        test_hooks::with_request_slot_pending(
            "target".into(),
            move || {
                observed.fetch_add(1, Ordering::AcqRel);
            },
            async {
                let ready = tokio::sync::Semaphore::new(1);
                let permit = test_hooks::observe_request_slot_pending("target", ready.acquire())
                    .await
                    .unwrap();
                assert_eq!(pending.load(Ordering::Acquire), 0);
                drop(permit);

                let blocked = tokio::sync::Semaphore::new(0);
                {
                    let other =
                        test_hooks::observe_request_slot_pending("other", blocked.acquire());
                    tokio::pin!(other);
                    std::future::poll_fn(|cx| {
                        assert!(other.as_mut().poll(cx).is_pending());
                        std::task::Poll::Ready(())
                    })
                    .await;
                    assert_eq!(pending.load(Ordering::Acquire), 0);
                }
                let target = test_hooks::observe_request_slot_pending("target", blocked.acquire());
                tokio::pin!(target);
                for _ in 0..2 {
                    std::future::poll_fn(|cx| {
                        assert!(target.as_mut().poll(cx).is_pending());
                        std::task::Poll::Ready(())
                    })
                    .await;
                    assert_eq!(pending.load(Ordering::Acquire), 1);
                }
                blocked.add_permits(1);
                let permit = target.await.unwrap();
                assert_eq!(pending.load(Ordering::Acquire), 1);
                drop(permit);
            },
        )
        .await;

        let blocked = tokio::sync::Semaphore::new(0);
        let unobserved = test_hooks::observe_request_slot_pending("target", blocked.acquire());
        tokio::pin!(unobserved);
        std::future::poll_fn(|cx| {
            assert!(unobserved.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(pending.load(Ordering::Acquire), 1);
    }

    #[test]
    fn provider_deadline_observes_spacing_cooldown_and_multiple_token_expiries() {
        let now = tokio::time::Instant::now();
        let mut pacing = RollingPacing::new(now);
        assert!(pacing.admit("first", 40_000, now));
        assert_eq!(
            pacing.next_eligible(10, now),
            now + Duration::from_millis(25)
        );
        assert!(pacing.admit("second", 40_000, now + Duration::from_millis(25)));
        assert_eq!(
            pacing.next_eligible(70_000, now + Duration::from_millis(50)),
            now + Duration::from_millis(1025),
        );
        pacing.defer(now, Duration::from_secs(5));
        assert_eq!(
            pacing.next_eligible(70_000, now + Duration::from_millis(50)),
            now + Duration::from_secs(5),
        );
        assert!(pacing.admit("after", 70_000, now + Duration::from_secs(5)));
    }

    #[tokio::test(start_paused = true)]
    async fn provider_admission_waits_for_deadline_and_cancels_on_connection_change() {
        let handle = crate::jev::worker::WorkerHandle::default();
        handle
            .set_system_one_connection(
                crate::jev::config::SystemOneConnection::jev_default(),
                Some("synthetic-key".into()),
            )
            .unwrap();
        let (_, generation) = handle.execution_client().unwrap();
        let now = tokio::time::Instant::now();
        {
            let mut pacing = provider_pacing(SystemOneProvider::Custom).lock().unwrap();
            *pacing = ProviderPacing::new(now);
            assert!(pacing.admit("first", 10, now));
        }
        let is_cancelled = || !handle.key_is_current(generation);
        wait_for_provider_neutral(SystemOneProvider::Custom, "second", 10, &is_cancelled)
            .await
            .unwrap();
        assert_eq!(tokio::time::Instant::now(), now + Duration::from_millis(25));
        provider_pacing(SystemOneProvider::Custom)
            .lock()
            .unwrap()
            .defer(tokio::time::Instant::now(), Duration::from_secs(60));
        let waiting =
            wait_for_provider_neutral(SystemOneProvider::Custom, "cancelled", 10, &is_cancelled);
        tokio::pin!(waiting);
        std::future::poll_fn(|cx| {
            assert!(waiting.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        handle.suspend_system_one();
        assert_eq!(waiting.await, Err(JevError::Cancelled));
    }

    #[tokio::test(start_paused = true)]
    async fn shared_request_deadline_keeps_delivery_unknown() {
        assert_eq!(
            request_with_deadline(std::future::pending()).await,
            Err(JevError::RequestOutcomeUnknown),
        );
    }

    #[tokio::test]
    async fn global_request_budgets_are_shared_and_release_after_cancellation() {
        let handle = crate::jev::worker::WorkerHandle::default();
        handle
            .set_system_one_connection(
                crate::jev::config::SystemOneConnection::jev_default(),
                Some("synthetic-key".into()),
            )
            .unwrap();
        let (_, generation) = handle.execution_client().unwrap();
        assert!(std::ptr::eq(request_bytes(), request_bytes()));
        let held = request_bytes()
            .acquire_many(GLOBAL_REQUEST_BYTES as u32)
            .await
            .unwrap();
        let is_cancelled = || !handle.key_is_current(generation);
        let (outcome, ()) = tokio::join!(
            acquire_neutral_budget(request_bytes().acquire_many(1), &is_cancelled),
            async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                handle.suspend_system_one();
            },
        );
        assert!(matches!(outcome, Err(JevError::Cancelled)));
        drop(held);
        let restored = request_bytes()
            .acquire_many(GLOBAL_REQUEST_BYTES as u32)
            .await
            .unwrap();
        assert_eq!(request_bytes().available_permits(), 0);
        drop(restored);
        assert!(std::ptr::eq(request_slots(), request_slots()));
        let slots = request_slots()
            .acquire_many(antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS as u32)
            .await
            .unwrap();
        assert_eq!(request_slots().available_permits(), 0);
        drop(slots);
    }

    #[tokio::test]
    async fn cancellation_releases_a_blocked_admission_wait() {
        let budget = tokio::sync::Semaphore::new(0);
        let cancelled = AtomicBool::new(false);
        let is_cancelled = || cancelled.load(Ordering::Acquire);
        let (result, ()) = tokio::join!(
            acquire_neutral_budget(budget.acquire(), &is_cancelled),
            async {
                tokio::task::yield_now().await;
                cancelled.store(true, Ordering::Release);
            }
        );
        assert!(matches!(result, Err(JevError::Cancelled)));
        assert_eq!(budget.available_permits(), 0);
    }

    #[tokio::test]
    async fn cancellation_after_transport_starts_is_unknown_and_closes_the_call() {
        struct PendingCall<'a>(&'a AtomicBool);
        impl Drop for PendingCall<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let dropped = AtomicBool::new(false);
        let call = async {
            let _call = PendingCall(&dropped);
            std::future::pending().await
        };
        let result = wait_for_decision_response(call, &|| true).await;
        assert_eq!(result, Err(JevError::RequestOutcomeUnknown));
        assert!(dropped.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn oversized_request_does_not_send_or_record_usage() {
        let transport = DecisionTransport::Jev {
            client: TypeSafeClient::new("synthetic-key".into()).unwrap(),
            endpoint: crate::jev::config::system_one_endpoint(),
        };
        let request = JevRequest {
            model: antiburn_local::analysis::jev::PINNED_MODEL.into(),
            state: serde_json::json!({}),
            questions: Default::default(),
        };
        let result = dispatch_decision_attempt(
            &transport,
            &request,
            &ModelCapabilities::jev_default(),
            GLOBAL_REQUEST_BYTES + 1,
            "oversized",
            || false,
            |_| panic!("an oversized request must not settle provider usage"),
        )
        .await;
        assert!(matches!(result, Err(JevError::RequestTooLarge { .. })));
    }

    #[tokio::test]
    async fn all_selected_transports_cancel_before_admission_without_usage() {
        use crate::jev::config::ContextLimitOverride;
        let connections = [
            SystemOneConnection::jev_default(),
            SystemOneConnection {
                provider: SystemOneProvider::Ollama,
                endpoint: SystemOneEndpoint::BaseUrl("http://127.0.0.1:9".into()),
                model: "synthetic-local".into(),
                credential: None,
                ..SystemOneConnection::jev_default()
            },
            SystemOneConnection {
                provider: SystemOneProvider::Cloudflare,
                endpoint: SystemOneEndpoint::CloudflareAccount("synthetic-account".into()),
                model: "clef".into(),
                response_mode: SystemOneResponseMode::CloudflareEnvelope,
                ..SystemOneConnection::jev_default()
            },
            SystemOneConnection {
                provider: SystemOneProvider::Custom,
                endpoint: SystemOneEndpoint::ExactUrl("http://127.0.0.1:9/exact".into()),
                credential: None,
                context_override: Some(ContextLimitOverride {
                    total_input_tokens: Some(8192),
                    ..Default::default()
                }),
                ..SystemOneConnection::jev_default()
            },
        ];
        for connection in connections {
            let provider = connection.provider;
            let capabilities = connection.capabilities().unwrap();
            let credential = matches!(
                provider,
                SystemOneProvider::Jev | SystemOneProvider::Cloudflare
            )
            .then(|| "synthetic-key".into());
            let transport = DecisionTransport::from_connection(&connection, credential).unwrap();
            assert_eq!(transport.provider(), provider);
            let request = JevRequest {
                model: connection.model,
                state: serde_json::json!({}),
                questions: Default::default(),
            };
            assert_eq!(
                dispatch_decision_attempt(
                    &transport,
                    &request,
                    &capabilities,
                    1024,
                    "cancelled-selected-route",
                    || true,
                    |_| panic!("cancelled admission cannot settle usage"),
                )
                .await,
                Err(JevError::Cancelled)
            );
        }
    }

    #[test]
    fn provider_pacing_reserves_tokens_then_uses_measured_usage_and_shared_backoff() {
        let now = tokio::time::Instant::now();
        let mut pacing = RollingPacing::new(now);
        assert!(pacing.admit("first", 30_000, now));
        assert!(pacing.admit("second", 30_000, now + Duration::from_millis(100)));
        assert!(pacing.admit("third", 30_000, now + Duration::from_millis(200)));
        assert!(!pacing.admit("fourth", 30_000, now + Duration::from_millis(300)));
        pacing.settle("first", 10_000);
        pacing.settle("second", 0);
        pacing.settle("third", 0);
        assert!(pacing.admit("fourth", 20_000, now + Duration::from_millis(300)));
        assert_eq!(
            pacing.starts.iter().map(|start| start.tokens).sum::<u64>(),
            30_000
        );
        pacing.next_start = now + Duration::from_secs(12);
        assert!(!pacing.admit("fifth", 30_000, now + Duration::from_secs(1)));
        assert!(pacing.admit("fifth", 30_000, now + Duration::from_secs(12)));
        assert_eq!(pacing.starts.len(), 1);
        println!(
            "provider pacing estimated request tokens allows bounded concurrency within rolling limit"
        );
    }

    #[test]
    fn provider_pacing_returns_the_next_token_window_deadline() {
        let now = tokio::time::Instant::now();
        let mut pacing = ProviderPacing::new(now);
        assert!(pacing.admit("first", 60_000, now));
        let blocked_at = now + Duration::from_millis(25);
        assert!(!pacing.admit("second", 50_000, blocked_at));
        assert_eq!(
            pacing.next_eligible(50_000, blocked_at),
            now + Duration::from_secs(1)
        );
        assert!(pacing.admit("second", 50_000, now + Duration::from_secs(1)));
    }

    #[test]
    fn provider_pacing_buckets_do_not_share_admission_state() {
        let now = tokio::time::Instant::now();
        let pacing = ProviderPacingSet::new(now);
        let mut jev = pacing.jev.lock().unwrap();
        let mut custom = pacing.custom.lock().unwrap();
        assert!(jev.admit("jev", GLOBAL_INPUT_TOKENS_PER_SECOND, now));
        assert!(!jev.admit("jev-next", 1, now));
        assert!(custom.admit("custom", 1, now));
    }

    #[test]
    fn headerless_rate_limit_and_overload_apply_provider_cooldown() {
        let now = tokio::time::Instant::now();
        for error in [
            JevError::RateLimited { retry_after: None },
            JevError::ProviderOverloaded { retry_after: None },
        ] {
            let mut pacing = ProviderPacing::new(now);
            apply_provider_cooldown(&mut pacing, &error, now);
            assert!(!pacing.admit("during", 10, now + Duration::from_secs(4)));
            assert!(pacing.admit("after", 10, now + Duration::from_secs(5)));
        }
    }

    #[test]
    fn cooldown_is_provider_scoped_and_retry_after_dominates_backoff() {
        let now = tokio::time::Instant::now();
        let mut limited_provider = ProviderPacing::new(now);
        let mut other_provider = ProviderPacing::new(now);
        apply_provider_cooldown(
            &mut limited_provider,
            &JevError::RateLimited {
                retry_after: Some(Duration::from_secs(17)),
            },
            now,
        );

        assert!(!limited_provider.admit("before", 10, now + Duration::from_secs(16)));
        assert!(other_provider.admit("isolated", 10, now));
        assert!(limited_provider.admit("retry-after", 10, now + Duration::from_secs(17)));
    }
}

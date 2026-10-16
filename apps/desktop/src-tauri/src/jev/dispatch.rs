use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use antiburn_local::analysis::jev::{
    JevError, JevRequest, JevResponse, JevUsage, MAX_REQUEST_TOKENS,
    capabilities::ModelCapabilities,
};

use crate::jev::client::TypeSafeClient;
use crate::jev::config::SystemOneProvider;

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

pub(super) struct ProviderPacing {
    next_start: tokio::time::Instant,
    cooldown_until: tokio::time::Instant,
    starts: std::collections::VecDeque<TokenStart>,
}

impl ProviderPacing {
    pub(super) fn new(now: tokio::time::Instant) -> Self {
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

struct ProviderPacingSet {
    jev: std::sync::Mutex<ProviderPacing>,
    ollama: std::sync::Mutex<ProviderPacing>,
    cloudflare: std::sync::Mutex<ProviderPacing>,
    custom: std::sync::Mutex<ProviderPacing>,
}

impl ProviderPacingSet {
    fn new(now: tokio::time::Instant) -> Self {
        Self {
            jev: std::sync::Mutex::new(ProviderPacing::new(now)),
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
pub(super) async fn dispatch_decision_attempt(
    client: &TypeSafeClient,
    request: &JevRequest,
    capabilities: &ModelCapabilities,
    resident_bytes: usize,
    request_id: &str,
    is_cancelled: impl Fn() -> bool + Sync,
    record_usage: impl Fn(Option<JevUsage>) -> Result<(), JevError> + Sync,
) -> Result<JevResponse, JevError> {
    let byte_count = u32::try_from(resident_bytes).map_err(|_| JevError::RequestSerialization)?;
    if byte_count == 0 || resident_bytes > GLOBAL_REQUEST_BYTES {
        return Err(JevError::RequestTooLarge {
            bytes: resident_bytes,
            maximum: GLOBAL_REQUEST_BYTES,
        });
    }
    let request_body_bytes = serde_json::to_vec(request)
        .map_err(|_| JevError::RequestSerialization)?
        .len();
    let estimated_tokens = u64::try_from(request_body_bytes.div_ceil(3))
        .unwrap_or(MAX_REQUEST_TOKENS)
        .min(MAX_REQUEST_TOKENS);
    let reservation_id = format!("decision:{request_id}");
    let (_slot, _bytes) = tokio::time::timeout(REQUEST_DEADLINE, async {
        let slot = acquire_neutral_budget(request_slots().acquire(), &is_cancelled).await?;
        let bytes =
            acquire_neutral_budget(request_bytes().acquire_many(byte_count), &is_cancelled).await?;
        wait_for_provider_neutral(
            SystemOneProvider::Jev,
            &reservation_id,
            estimated_tokens,
            &is_cancelled,
        )
        .await?;
        Ok::<_, JevError>((slot, bytes))
    })
    .await
    .map_err(|_| JevError::ProviderUnavailable)??;
    if is_cancelled() {
        provider_pacing(SystemOneProvider::Jev)
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .settle(&reservation_id, 0);
        return Err(JevError::Cancelled);
    }

    let endpoint = crate::jev::config::system_one_endpoint();
    let usage_received = AtomicBool::new(false);
    let call = request_with_deadline(client.evaluate_at_with_capabilities_and_usage(
        request,
        &endpoint,
        capabilities,
        |usage| {
            usage_received.store(true, Ordering::Relaxed);
            provider_pacing(SystemOneProvider::Jev)
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .settle(&reservation_id, usage.input_tokens);
            record_usage(Some(usage))
        },
    ));
    let result = wait_for_decision_response(call, &is_cancelled).await;
    if let Err(error) = &result {
        {
            let mut pacing = provider_pacing(SystemOneProvider::Jev)
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            apply_provider_cooldown(&mut pacing, error, tokio::time::Instant::now());
            if matches!(
                error,
                JevError::RateLimited { .. } | JevError::ProviderOverloaded { .. }
            ) {
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
mod tests {
    use super::*;

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
        let client = TypeSafeClient::new("synthetic-key".into()).unwrap();
        let request = JevRequest {
            model: antiburn_local::analysis::jev::PINNED_MODEL.into(),
            state: serde_json::json!({}),
            questions: Default::default(),
        };
        let result = dispatch_decision_attempt(
            &client,
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

    #[test]
    fn provider_pacing_reserves_tokens_then_uses_measured_usage_and_shared_backoff() {
        let now = tokio::time::Instant::now();
        let mut pacing = ProviderPacing::new(now);
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

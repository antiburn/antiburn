//! Serialize authorization, rotation, and cleanup under the controller mutex.
//! Closed state survives failure. Neither browser cancellation nor account
//! cleanup owns capture requests or compressed retry bytes.

use super::{
    AccessUpdateStatus, AuthStatus,
    auth::{
        self, AccessUpgrade, Attempt, Callback, Config, Credential, DeviceAction,
        EligibleWorkspace, Failure, ReplacementCredential, Session, TokenRequest, Transport, Vault,
        Workspace, WorkspaceKind,
    },
    loopback::Listener,
    state::{self, Cleanup, Guard, Phase, Record, StateStore},
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Semaphore;

/// A best-effort logout owns only old proofs and their original service.
pub(super) struct RemoteLogout {
    config: Config,
    proofs: Vec<String>,
}

impl RemoteLogout {
    fn new(config: Config, credential: Credential) -> Self {
        Self {
            config,
            proofs: std::iter::once(credential.refresh_token)
                .chain(credential.cleanup_refresh_tokens)
                .chain(
                    credential
                        .replacement
                        .map(|replacement| replacement.refresh_token),
                )
                .collect(),
        }
    }

    /// Try every captured proof once. This operation never touches local state.
    pub(super) fn send(self, transport: &dyn Transport) -> bool {
        let mut confirmed = true;
        for proof in self.proofs {
            confirmed &= transport.revoke(&self.config, &proof).is_ok();
        }
        confirmed
    }
}

struct Access {
    token: String,
    expires_at: i64,
    epoch: String,
    session: Session,
}

#[cfg(unix)]
#[path = "device_broker/grant.rs"]
mod device_grant;

#[cfg(all(test, debug_assertions))]
#[path = "tests/execution_http_integration_tests.rs"]
mod execution_http_tests;

/// Synchronous grant state machine used only inside admitted blocking work.
/// The controller serializes every call; filesystem, vault, and short blocking
/// HTTP operations must not execute on UI or Tokio runtime threads.
pub struct Engine {
    pub config: Option<Config>,
    pub vault: Box<dyn Vault>,
    pub transport: Arc<dyn Transport>,
    remote_logout: Arc<Semaphore>,
    remote_logout_failed: Arc<AtomicBool>,
    notify_remote_logout_failure: Option<Arc<dyn Fn() + Send + Sync>>,
    pub state: StateStore,
    pub attempt: Option<Attempt>,
    pub(super) access_update: Option<AccessUpdateStatus>,
    access: Option<Access>,
    clock: Box<dyn Fn() -> i64 + Send>,
    pub status: AuthStatus,
    pub cleanup: Cleanup,
    pub local_reset_available: bool,
    pub generation: u64,
    cleanup_only: bool,
    sign_out_marker_failed: bool,
}

impl Engine {
    /// Construct memory-only state. Credential/state restoration waits for `refresh`.
    pub fn new(
        config: Option<Config>,
        vault: Box<dyn Vault>,
        transport: Box<dyn Transport>,
        state: StateStore,
    ) -> Self {
        Self {
            config,
            vault,
            transport: transport.into(),
            remote_logout: Arc::new(Semaphore::new(1)),
            remote_logout_failed: Arc::new(AtomicBool::new(false)),
            notify_remote_logout_failure: None,
            state,
            attempt: None,
            access_update: None,
            access: None,
            clock: Box::new(super::now),
            status: AuthStatus::Idle,
            cleanup: Cleanup::None,
            local_reset_available: false,
            generation: 0,
            cleanup_only: false,
            sign_out_marker_failed: false,
        }
    }

    /// Notify the current account views after the worker records a failure.
    pub(super) fn with_remote_logout_notification(
        mut self,
        notify: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        self.notify_remote_logout_failure = Some(Arc::new(notify));
        self
    }

    /// Open an old workspace slot for recovery or revocation only.
    pub(super) fn workspace_engine(
        &self,
        identifier: String,
        account: String,
        workspace: Workspace,
    ) -> Result<Self, Failure> {
        let config = self.config.clone().ok_or(Failure::Unavailable)?;
        let namespace = auth::workspace_namespace(&config, &account, &workspace.id);
        let state = self.state.workspace(&namespace);
        let vault = auth::KeyringVault::new(identifier, &state.workspace_vault_namespace());
        let mut engine = Self::new(
            Some(config),
            Box::new(vault),
            Box::new(auth::HttpTransport),
            state,
        );
        engine.cleanup_only = true;
        engine.remote_logout = self.remote_logout.clone();
        engine.remote_logout_failed = self.remote_logout_failed.clone();
        engine.notify_remote_logout_failure = self.notify_remote_logout_failure.clone();
        Ok(engine)
    }

    /// Read only a team lifecycle marker. A warning never modifies the Personal slot.
    pub(super) fn workspace_cleanup_unconfirmed(
        &self,
        account: &str,
        workspace: &str,
    ) -> Result<bool, Failure> {
        let config = self.config.as_ref().ok_or(Failure::Unavailable)?;
        let state = self
            .state
            .workspace(&auth::workspace_namespace(config, account, workspace));
        let record = state.lock()?.read()?;
        Ok(record.cleanup.blocks_connection()
            || record.cleanup.prior_unconfirmed()
            || record.phase == Phase::Pending)
    }

    /// Use a deterministic clock for synthetic authority tests.
    #[cfg(test)]
    pub(super) fn with_clock(mut self, clock: impl Fn() -> i64 + Send + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// Read unexpired cached identity, not fresh server authority. Refresh before
    /// request actions and enforce the controller's current account generation too.
    pub fn session(&self) -> Option<&Session> {
        self.access
            .as_ref()
            .filter(|access| access.expires_at > (self.clock)())
            .filter(|access| {
                self.config.as_ref().is_some_and(|config| {
                    access.session.resource == config.resource
                        && access.session.iss == config.issuer
                })
            })
            .map(|access| &access.session)
    }

    /// Return the expiry of the cached access proof used for local folder reads.
    pub(super) fn listing_expiry(&self) -> Option<i64> {
        self.session()?;
        self.access.as_ref().map(|access| access.expires_at)
    }

    /// Return the configured canonical API origin, not access authority.
    pub(super) fn resource_origin(&self) -> Option<&str> {
        self.config.as_ref().map(|config| config.resource.as_str())
    }

    /// Fetch a fresh eligible list without exposing the bearer. Missing or changed
    /// Personal anchors fail closed. A rejected bearer closes cached access but
    /// keeps the saved grant. Callers must match each exact Team generation.
    pub(super) fn eligible_workspaces(
        &mut self,
        binding: &super::DestinationBinding,
        current: &dyn Fn() -> bool,
    ) -> Result<Vec<EligibleWorkspace>, Failure> {
        if !current() {
            return Err(Failure::Cancelled);
        }
        self.refresh((self.clock)(), current)?;
        let config = self.config.as_ref().ok_or(Failure::Unavailable)?;
        let access = self.access.as_ref().ok_or(Failure::ReauthRequired)?;
        if !current()
            || !self
                .session()
                .is_some_and(|session| binding.matches(session))
        {
            return Err(Failure::Cancelled);
        }
        let result = self.transport.workspaces(config, &access.token);
        if !current() {
            return Err(Failure::Cancelled);
        }
        if matches!(result, Err(Failure::ReauthRequired)) {
            self.access = None;
            self.status = AuthStatus::ReauthRequired;
            return result;
        }
        let result = result?;
        if self.session().is_none() {
            return Err(Failure::Cancelled);
        }
        if personal_anchor(&result)?.id != access.session.workspace.id {
            return Err(Failure::InvalidResponse);
        }
        Ok(result)
    }

    /// Send one bounded capture API request without returning access proof.
    /// Reject a changed service before credentials are used. Auth rotation uses
    /// only the account fence; content pause blocks dispatch without revocation.
    /// HTTP 401 invalidates cached access. HTTP 403 leaves account access intact.
    pub(super) fn capture_request(
        &mut self,
        identity: &antiburn_cloud_capture::CaptureIdentity,
        binding: (&str, Option<&str>),
        route: super::upload::Route,
        body: &impl serde::Serialize,
        account_current: &dyn Fn() -> bool,
        current: &dyn Fn() -> bool,
    ) -> Result<super::upload::ApiResponse, antiburn_cloud_capture::TransportError> {
        use antiburn_cloud_capture::TransportError;
        let (resource_origin, membership_generation) = binding;
        if !account_current() || !current() || self.resource_origin() != Some(resource_origin) {
            return Err(TransportError::Unauthorized);
        }
        self.refresh((self.clock)(), account_current)
            .map_err(|failure| {
                self.status = failure.into();
                match failure {
                    Failure::Offline | Failure::OfflineBeforeSend => TransportError::Offline,
                    _ => TransportError::Unauthorized,
                }
            })?;
        let config = self.config.as_ref().ok_or(TransportError::Unauthorized)?;
        let access = self.access.as_ref().ok_or(TransportError::Unauthorized)?;
        if !current()
            || access.session.user.id != identity.account_id
            || !access.session.allows(DeviceAction::CaptureWrite)
            || access.expires_at <= (self.clock)()
            || access.session.validate(config, (self.clock)()).is_err()
        {
            return Err(TransportError::Unauthorized);
        }
        if let Some(generation) = membership_generation {
            let eligible = self
                .transport
                .workspaces(config, &access.token)
                .map_err(|error| match error {
                    Failure::Offline | Failure::OfflineBeforeSend => TransportError::Offline,
                    _ => TransportError::Unauthorized,
                })?;
            if !personal_anchor(&eligible)
                .is_ok_and(|anchor| anchor.id == access.session.workspace.id)
                || !eligible.iter().any(|entry| {
                    entry.workspace.kind == WorkspaceKind::Team
                        && entry.workspace.id == identity.destination_id
                        && entry.membership_generation.as_deref() == Some(generation)
                })
            {
                return Err(TransportError::Unauthorized);
            }
        } else if access.session.workspace.id != identity.destination_id {
            return Err(TransportError::Unauthorized);
        }
        if !account_current() || !current() || self.session().is_none() {
            return Err(TransportError::Unauthorized);
        }
        let response = super::upload::api_request(config, &access.token, identity, route, body)?;
        if response.status == 401 {
            self.access = None;
            self.status = AuthStatus::ReauthRequired;
        }
        if !current() || self.session().is_none() {
            return Err(TransportError::Unauthorized);
        }
        Ok(response)
    }

    /// Reject reordered cleanup after a newer account operation owns the generation.
    pub fn allows_account_cleanup(&self, requested: u64, latest: u64) -> bool {
        requested == latest && requested >= self.generation
    }

    /// Clear cached access on state-read failure instead of falling back to memory.
    fn read_state(&mut self) -> Result<(Guard, Record), Failure> {
        let state = self.state.lock().and_then(|mut guard| {
            let record = guard.read()?;
            Ok((guard, record))
        });
        if state.is_err() {
            self.access = None;
        } else if let Ok((_, record)) = &state {
            self.cleanup = record.cleanup;
        }
        state
    }

    /// Restore an active saved grant when local access is missing. Return no
    /// listener after refresh. Only open browser approval for a new connection.
    pub(super) fn begin_account_sign_in(
        &mut self,
        now: i64,
        generation: u64,
        account_current: impl Fn() -> bool,
    ) -> Result<Option<(String, Listener, Attempt)>, Failure> {
        if !self.cleanup_only
            && !self.sign_out_marker_failed
            && self.attempt.is_none()
            && self.session().is_none()
        {
            let (_, record) = self.read_state()?;
            if record.phase == Phase::Active {
                self.refresh(now, account_current)?;
                return Ok(None);
            }
        }
        self.begin(now, generation).map(Some)
    }

    /// Bind a fresh callback before browser launch and persist its pending epoch.
    /// The caller must wait for the returned listener outside the controller lock.
    pub fn begin(
        &mut self,
        now: i64,
        generation: u64,
    ) -> Result<(String, Listener, Attempt), Failure> {
        if self.cleanup_only {
            return Err(Failure::Unavailable);
        }
        let config = self.config.clone().ok_or(Failure::Unavailable)?;
        let (mut guard, mut record) = self.read_state()?;
        if record.phase == Phase::ReauthRequired && record.cleanup.blocks_connection() {
            return Err(Failure::Unavailable);
        }
        if record.phase == Phase::Active
            && !self.sign_out_marker_failed
            && self.status != AuthStatus::ReauthRequired
        {
            return Err(Failure::Unavailable);
        }
        if self.sign_out_marker_failed
            || record.phase != Phase::SignedOut
            || record.cleanup.blocks_connection()
            || self.vault.read()?.is_some()
        {
            drop(guard);
            if let Some(logout) = self.sign_out()? {
                self.try_remote_logout(logout);
            }
            (guard, record) = self.read_state()?;
            if self.sign_out_marker_failed || record.cleanup.blocks_connection() {
                return Err(Failure::CredentialFailed);
            }
        }
        // An explicit sign-in replaces any pending listener, including one lost on restart.
        self.attempt = None;
        self.access_update = None;
        self.vault.write(None)?;
        let listener = Listener::bind()?;
        let record = guard.replace(Phase::Pending, record.cleanup)?;
        self.sign_out_marker_failed = false;
        let (url, listener, attempt) =
            Self::browser_attempt(&config, listener, now, record.epoch, generation, None)?;
        self.access = None;
        self.attempt = Some(attempt.clone());
        self.status = AuthStatus::Opening;
        Ok((url, listener, attempt))
    }

    /// Reuse browser approval without changing active credentials or folder settings.
    /// The caller must claim the browser attempt before starting the update.
    /// Browser cancellation cannot cancel refresh of the current account grant.
    pub(super) fn begin_access_update(
        &mut self,
        now: i64,
        generation: u64,
        actions: &[DeviceAction],
        account_current: impl Fn() -> bool,
    ) -> Result<(String, Listener, Attempt), Failure> {
        self.refresh(now, account_current)?;
        let session = self.session().ok_or(Failure::ReauthRequired)?;
        if self.cleanup_only || !actions.iter().any(|&action| !session.allows(action)) {
            return Err(Failure::Unavailable);
        }
        let config = self.config.as_ref().ok_or(Failure::Unavailable)?;
        let mut scope = auth::scope_names(&session.scope).ok_or(Failure::InvalidResponse)?;
        scope.extend(actions.iter().map(|action| action.name()));
        let upgrade = AccessUpgrade {
            grant_id: session.grant_id.clone(),
            account_id: session.user.id.clone(),
            requested_scope: scope.into_iter().collect::<Vec<_>>().join(" "),
        };
        let epoch = self
            .access
            .as_ref()
            .ok_or(Failure::ReauthRequired)?
            .epoch
            .clone();
        let (url, listener, attempt) = Self::browser_attempt(
            config,
            Listener::bind()?,
            now,
            epoch,
            generation,
            Some(upgrade),
        )?;
        self.attempt = Some(attempt.clone());
        self.access_update = Some(AccessUpdateStatus::Waiting);
        Ok((url, listener, attempt))
    }

    fn browser_attempt(
        config: &Config,
        listener: Listener,
        now: i64,
        epoch: String,
        generation: u64,
        upgrade: Option<AccessUpgrade>,
    ) -> Result<(String, Listener, Attempt), Failure> {
        let attempt = Attempt {
            state: auth::random()?,
            verifier: auth::random()?,
            callback: listener.callback().to_owned(),
            expires_at: now + 600,
            epoch,
            generation,
            upgrade,
        };
        let mut url = config.authorization.clone();
        let device_name = auth::device_name();
        url.query_pairs_mut().extend_pairs([
            ("redirect_uri", attempt.callback.as_str()),
            ("state", attempt.state.as_str()),
            (
                "code_challenge",
                auth::challenge(&attempt.verifier).as_str(),
            ),
            ("code_challenge_method", "S256"),
            ("device_name", device_name.as_str()),
        ]);
        if let Some(upgrade) = &attempt.upgrade {
            url.query_pairs_mut()
                .append_pair("scope", &upgrade.requested_scope)
                .append_pair("account_id", &upgrade.account_id);
        }
        Ok((url.to_string(), listener, attempt))
    }

    /// Clear only this pending attempt when no browser handoff occurred.
    /// Do not use this for an uncertain launch or a lost callback.
    pub fn abort_browser_launch(&mut self, attempt: &Attempt) -> Result<(), Failure> {
        if self.attempt.as_ref().is_none_or(|pending| {
            pending.epoch != attempt.epoch || pending.generation != attempt.generation
        }) {
            return Ok(());
        }
        if attempt.upgrade.is_some() {
            self.attempt = None;
            self.access_update = None;
            return Ok(());
        }
        let (mut guard, record) = self.read_state()?;
        if record.epoch != attempt.epoch || record.phase != Phase::Pending {
            return Ok(());
        }
        guard.replace(Phase::SignedOut, record.cleanup)?;
        self.attempt = None;
        self.cleanup = record.cleanup;
        self.local_reset_available = false;
        self.status = AuthStatus::BrowserFailed;
        Ok(())
    }

    /// Abandon only the eligible browser attempt; a newer attempt or active grant survives.
    pub fn cancel(&mut self, generation: u64) -> Result<(), Failure> {
        if self
            .attempt
            .as_ref()
            .is_some_and(|attempt| attempt.generation > generation)
        {
            return Ok(());
        }
        if let Some(attempt) = self.attempt.take() {
            if attempt.upgrade.is_some() {
                // The browser can create a grant before Desktop receives the callback.
                self.mark_remote_logout_unconfirmed();
                self.access_update = None;
                return Ok(());
            }
            let (mut guard, record) = self.read_state()?;
            if record.epoch == attempt.epoch && record.phase == Phase::Pending {
                return self.abandon_pending(&mut guard);
            }
        }
        if self.access.is_none() && generation > 0 {
            self.status = if self.cleanup.blocks_connection() {
                AuthStatus::RevocationUnconfirmed
            } else {
                AuthStatus::Cancelled
            };
        }
        Ok(())
    }

    /// Lost callback proof cannot establish remote revocation, even before exchange.
    fn abandon_pending(&mut self, guard: &mut Guard) -> Result<(), Failure> {
        self.access = None;
        self.attempt = None;
        let record = guard.replace(Phase::SignedOut, Cleanup::RemoteUnconfirmed)?;
        self.cleanup = record.cleanup;
        self.status = AuthStatus::Cancelled;
        self.local_reset_available = false;
        Ok(())
    }

    /// Exchange only a current pending callback. Persist closed recovery state
    /// before transport, then validate all authority before saving refresh proof.
    /// `started_at` bounds access lifetime before blocking work; recheck the clock
    /// after that work before installing access.
    pub fn callback(
        &mut self,
        attempt: &Attempt,
        callback: Callback,
        started_at: i64,
        current: impl Fn() -> bool,
    ) -> Result<(), Failure> {
        if let Some(upgrade) = &attempt.upgrade {
            return self.complete_access_update(attempt, upgrade, callback, started_at, current);
        }
        let (mut guard, mut record) = self.read_state()?;
        if !current()
            || started_at >= attempt.expires_at
            || record.phase != Phase::Pending
            || record.epoch != attempt.epoch
            || self.attempt.as_ref().is_none_or(|pending| {
                pending.epoch != attempt.epoch || pending.generation != attempt.generation
            })
        {
            return Err(Failure::Cancelled);
        }
        self.attempt = None;
        let Callback::Code(code) = callback else {
            guard.replace(Phase::SignedOut, record.cleanup)?;
            self.status = AuthStatus::Cancelled;
            return Ok(());
        };
        record.phase = Phase::ReauthRequired;
        record.cleanup = Cleanup::Revoke {
            prior_unconfirmed: record.cleanup.prior_unconfirmed(),
        };
        guard.write(&record)?;
        self.cleanup = record.cleanup;
        let config = self.config.as_ref().ok_or(Failure::Unavailable)?;
        let tokens = self.transport.exchange(
            config,
            &TokenRequest::AuthorizationCode {
                code: &code,
                code_verifier: &attempt.verifier,
            },
        )?;
        self.accept(&mut guard, record, tokens, None, started_at, current)
    }

    /// Exchange into a separate candidate. The old proof stays active until the
    /// candidate has passed remote checks, a secure write, and an epoch commit.
    fn complete_access_update(
        &mut self,
        attempt: &Attempt,
        upgrade: &AccessUpgrade,
        callback: Callback,
        started_at: i64,
        current: impl Fn() -> bool,
    ) -> Result<(), Failure> {
        if !current()
            || self.attempt.as_ref().is_none_or(|pending| {
                pending.epoch != attempt.epoch || pending.generation != attempt.generation
            })
        {
            return Err(Failure::Cancelled);
        }
        let preflight = self.read_state().and_then(|(guard, record)| {
            if started_at >= attempt.expires_at
                || record.phase != Phase::Active
                || record.epoch != attempt.epoch
            {
                return Err(Failure::Cancelled);
            }
            Ok((guard, record))
        });
        if !current() {
            return Err(Failure::Cancelled);
        }
        // The listener delivered its only callback after browser launch. Leave
        // no pending attempt on failure so the user can start another update.
        self.attempt = None;
        let (mut guard, record) = preflight.inspect_err(|_| {
            if matches!(callback, Callback::Code(_)) {
                self.mark_remote_logout_unconfirmed();
            }
        })?;
        let Callback::Code(code) = callback else {
            self.access_update = Some(AccessUpdateStatus::Denied);
            return Ok(());
        };
        // Until Desktop has a distinct proof to revoke, a verified callback
        // can fail locally without proving that Cloud did not create a grant.
        let (config, mut saved, prior_workspace, tokens) = (|| {
            let config = self.config.clone().ok_or(Failure::Unavailable)?;
            let saved = self
                .vault
                .read()?
                .and_then(|credential| credential.for_epoch(&record.epoch))
                .filter(|credential| {
                    self.owns_credential(Some(credential)) && credential.account_grant
                })
                .ok_or(Failure::CredentialFailed)?;
            let prior = saved
                .session
                .as_ref()
                .filter(|session| {
                    session.grant_id == upgrade.grant_id && session.user.id == upgrade.account_id
                })
                .ok_or(Failure::Cancelled)?;
            let prior_workspace = prior.workspace.id.clone();
            if !current() {
                return Err(Failure::Cancelled);
            }
            let tokens = self.transport.exchange(
                &config,
                &TokenRequest::AuthorizationCode {
                    code: &code,
                    code_verifier: &attempt.verifier,
                },
            )?;
            // A bad response must not make the old grant a cleanup target.
            if tokens.grant_id == upgrade.grant_id || tokens.refresh_token == saved.refresh_token {
                return Err(Failure::InvalidResponse);
            }
            Ok((config, saved, prior_workspace, tokens))
        })()
        .inspect_err(|_| self.mark_remote_logout_unconfirmed())?;
        let result = (|| {
            let (session, expires_at) =
                self.verify_tokens(&config, &tokens, started_at, &current)?;
            if session.user.id != upgrade.account_id
                || session.workspace.id != prior_workspace
                || auth::scope_names(&session.scope) != auth::scope_names(&upgrade.requested_scope)
            {
                return Err(Failure::InvalidResponse);
            }
            let replacement = Record {
                epoch: auth::random()?,
                phase: Phase::Active,
                // Persist uncertainty before trying to revoke the old grant.
                cleanup: Cleanup::RemoteUnconfirmed,
            };
            saved.replacement = Some(ReplacementCredential {
                epoch: replacement.epoch.clone(),
                refresh_token: tokens.refresh_token.clone(),
                session: session.clone(),
            });
            if !current() || expires_at <= (self.clock)() {
                return Err(Failure::Cancelled);
            }
            // Even if this write reports an error after changing the entry,
            // restart still resolves the unchanged active epoch to the old proof.
            self.vault.write(Some(&saved))?;
            let verified = self
                .vault
                .read()?
                .filter(|credential| {
                    self.owns_credential(Some(credential))
                        && credential.account_grant
                        && credential.epoch == saved.epoch
                        && credential.refresh_token == saved.refresh_token
                })
                .and_then(|credential| credential.replacement)
                .is_some_and(|candidate| {
                    candidate.epoch == replacement.epoch
                        && candidate.refresh_token == tokens.refresh_token
                        && candidate.session.same_grant(&session)
                });
            if !verified {
                return Err(Failure::CredentialFailed);
            }
            if !current() || expires_at <= (self.clock)() {
                return Err(Failure::Cancelled);
            }
            Ok((session, expires_at, replacement))
        })();
        let (session, expires_at, mut replacement) = match result {
            Ok(value) => value,
            Err(error) => {
                self.revoke_access_update_grant(&config, &tokens.refresh_token);
                return Err(error);
            }
        };
        let committed = guard.write(&replacement);
        if committed.is_err() || !current() || expires_at <= (self.clock)() {
            // A rename can succeed before a directory sync fails. If rollback
            // also fails, keep both proofs and let refresh resolve the epoch.
            if let Err(error) = guard.write(&record) {
                self.access = None;
                self.mark_remote_logout_unconfirmed();
                return Err(error);
            }
            self.revoke_access_update_grant(&config, &tokens.refresh_token);
            committed?;
            return Err(Failure::Cancelled);
        }
        if self.revoke_access_update_grant(&config, &saved.refresh_token)
            && current()
            && record.cleanup != replacement.cleanup
        {
            // Restore only the previous marker cleanup under the same lock.
            // Detached workers record their warnings separately.
            replacement.cleanup = record.cleanup;
            if guard.write(&replacement).is_err() {
                replacement.cleanup = Cleanup::RemoteUnconfirmed;
                self.mark_remote_logout_unconfirmed();
            }
        }
        self.access = Some(Access {
            token: tokens.access_token,
            expires_at,
            epoch: replacement.epoch,
            session,
        });
        self.status = AuthStatus::SignedIn;
        self.cleanup = replacement.cleanup;
        self.access_update = None;
        Ok(())
    }

    /// Do not use the detached slot here. Explicit sign-out may need that slot
    /// for the active grant as soon as the access update returns.
    fn revoke_access_update_grant(&self, config: &Config, proof: &str) -> bool {
        if self.transport.revoke(config, proof).is_err() {
            self.mark_remote_logout_unconfirmed();
            return false;
        }
        true
    }

    fn mark_remote_logout_unconfirmed(&self) {
        Self::record_remote_logout_failure(
            &self.state.remote_logout_warning_path(),
            &self.remote_logout_failed,
        );
    }

    /// Verify token authority with independent grant and Personal destination reads.
    fn verify_tokens(
        &self,
        config: &Config,
        tokens: &auth::Tokens,
        started_at: i64,
        current: &impl Fn() -> bool,
    ) -> Result<(Session, i64), Failure> {
        tokens
            .validate(config, started_at)
            .and_then(|()| {
                if started_at.saturating_add(tokens.expires_in as i64) <= (self.clock)() {
                    return Err(Failure::InvalidResponse);
                }
                self.transport.session(config, &tokens.access_token)
            })
            .and_then(|account| {
                if !tokens.matches_session(&account) {
                    return Err(Failure::InvalidResponse);
                }
                if !current() {
                    return Err(Failure::Cancelled);
                }
                let eligible = self.transport.workspaces(config, &tokens.access_token)?;
                let session = account.with_personal(personal_anchor(&eligible)?.clone())?;
                let checked_at = started_at.max((self.clock)());
                session.validate(config, checked_at)?;
                let expires_at = started_at
                    .saturating_add(tokens.expires_in as i64)
                    .min(session.expires_at)
                    .min(session.access_expires_at);
                if expires_at <= checked_at {
                    return Err(Failure::InvalidResponse);
                }
                Ok((session, expires_at))
            })
    }

    /// Accept only matching fixed grant authority. Fence every vault/state write;
    /// rejected or late tokens must take the logout-only cleanup path.
    fn accept(
        &mut self,
        guard: &mut Guard,
        mut record: Record,
        tokens: auth::Tokens,
        previous: Option<&Credential>,
        started_at: i64,
        current: impl Fn() -> bool,
    ) -> Result<(), Failure> {
        let config = self.config.clone().ok_or(Failure::Unavailable)?;
        let session = self
            .verify_tokens(&config, &tokens, started_at, &current)
            .and_then(|(session, expires_at)| {
                if previous.is_some_and(|previous| {
                    previous
                        .session
                        .as_ref()
                        .is_none_or(|saved| !saved.same_grant(&session))
                        || previous.refresh_token == tokens.refresh_token
                }) {
                    Err(Failure::InvalidResponse)
                } else {
                    Ok((session, expires_at))
                }
            });
        if !current() || session.is_err() {
            self.discard_tokens(guard, record, &config, &tokens, previous)?;
            return Err(if current() {
                Failure::ReauthRequired
            } else {
                Failure::Cancelled
            });
        }
        let (session, expires_at) = session?;
        let credential = Credential {
            epoch: record.epoch.clone(),
            issuer: config.issuer.clone(),
            resource: config.resource.clone(),
            refresh_token: tokens.refresh_token.clone(),
            cleanup_refresh_tokens: Vec::new(),
            account_grant: true,
            session: Some(session.clone()),
            replacement: None,
        };
        if self.vault.write(Some(&credential)).is_err() {
            self.discard_tokens(guard, record, &config, &tokens, previous)?;
            return Err(Failure::CredentialFailed);
        }
        if !current() {
            self.discard_tokens(guard, record, &config, &tokens, previous)?;
            return Err(Failure::Cancelled);
        }
        record.phase = Phase::Active;
        record.cleanup = record.cleanup.prior_warning_only();
        let write_result = guard.write(&record);
        // Durable writes can outlast access or overlap account cancellation.
        let cancelled = !current();
        if write_result.is_err() || cancelled || expires_at <= (self.clock)() {
            self.discard_tokens(guard, record, &config, &tokens, previous)?;
            write_result?;
            return Err(if cancelled {
                Failure::Cancelled
            } else {
                Failure::ReauthRequired
            });
        }
        self.cleanup = record.cleanup;
        self.access = Some(Access {
            token: tokens.access_token,
            expires_at,
            epoch: record.epoch,
            session,
        });
        self.status = AuthStatus::SignedIn;
        Ok(())
    }

    /// Retain original and returned proofs before state writes can fail.
    /// Detached revocation does not change the saved recovery state.
    fn discard_tokens(
        &mut self,
        guard: &mut Guard,
        mut record: Record,
        config: &Config,
        tokens: &auth::Tokens,
        previous: Option<&Credential>,
    ) -> Result<(), Failure> {
        self.access = None;
        record.phase = Phase::SignedOut;
        record.cleanup = Cleanup::Revoke {
            prior_unconfirmed: record.cleanup.prior_unconfirmed(),
        };
        // Save logout-only proofs before a state-write failure can discard them.
        let primary = previous.map_or(&tokens.refresh_token, |credential| {
            &credential.refresh_token
        });
        let cleanup = Credential {
            epoch: record.epoch.clone(),
            issuer: config.issuer.clone(),
            resource: config.resource.clone(),
            refresh_token: primary.clone(),
            cleanup_refresh_tokens: if primary == &tokens.refresh_token {
                Vec::new()
            } else {
                vec![tokens.refresh_token.clone()]
            },
            account_grant: false,
            session: None,
            replacement: None,
        };
        let stored = self.vault.write(Some(&cleanup));
        if stored.is_err() {
            record.cleanup = Cleanup::Revoke {
                prior_unconfirmed: true,
            };
        }
        self.cleanup = record.cleanup;
        let marked = guard.write(&record);
        self.try_remote_logout(RemoteLogout::new(config.clone(), cleanup));
        marked?;
        stored
    }

    /// Start at most one detached logout across account and old Team slots.
    /// Skip when occupied. Failures mark only a warning, never the current grant.
    pub(super) fn try_remote_logout(
        &self,
        request: RemoteLogout,
    ) -> Option<tauri::async_runtime::JoinHandle<()>> {
        let warning_path = self.state.remote_logout_warning_path();
        let Ok(permit) = self.remote_logout.clone().try_acquire_owned() else {
            Self::record_remote_logout_failure(&warning_path, &self.remote_logout_failed);
            return None;
        };
        let transport = self.transport.clone();
        let failed = self.remote_logout_failed.clone();
        let notify = self.notify_remote_logout_failure.clone();
        Some(tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            if !request.send(transport.as_ref()) {
                Self::record_remote_logout_failure(&warning_path, &failed);
                if let Some(notify) = notify {
                    notify();
                }
            }
        }))
    }

    fn record_remote_logout_failure(path: &Path, failed: &AtomicBool) {
        failed.store(true, Ordering::Release);
        if state::record_remote_logout_failure(path).is_err() {
            tracing::debug!(event = "cloud_remote_logout_warning_not_saved");
        }
    }

    /// The warning is sticky across later grants and never controls authentication.
    pub(super) fn remote_logout_unconfirmed(&self) -> bool {
        self.cleanup.prior_unconfirmed()
            || self.remote_logout_failed.load(Ordering::Acquire)
            || (self.config.is_some()
                && state::remote_logout_unconfirmed(&self.state.remote_logout_warning_path()))
    }

    #[cfg(test)]
    pub(super) fn wait_remote_logout(&self) {
        tauri::async_runtime::block_on(async {
            let _permit = self.remote_logout.acquire().await.unwrap();
        });
    }

    fn owns_credential(&self, saved: Option<&Credential>) -> bool {
        saved.is_none_or(|credential| {
            self.config.as_ref().is_some_and(|config| {
                credential.issuer == config.issuer && credential.resource == config.resource
            })
        })
    }

    /// Finish local credential deletion. A failed vault deletion or
    /// final state write remains retryable without restoring authenticated access.
    fn finish_local_deletion(
        &mut self,
        guard: &mut Guard,
        mut record: Record,
    ) -> Result<(), Failure> {
        self.cleanup = record.cleanup;
        self.local_reset_available = false;
        if !self.owns_credential(self.vault.read()?.as_ref()) {
            return Err(Failure::CredentialFailed);
        }
        self.vault.write(None)?;
        record.cleanup = record.cleanup.prior_warning_only();
        guard.write(&record)?;
        self.cleanup = record.cleanup;
        self.status = AuthStatus::SignedOut;
        Ok(())
    }

    /// Restore or rotate one fixed grant inside the worker. Persist uncertainty
    /// before exchange; never retry a predecessor after an uncertain response.
    pub fn refresh(&mut self, started_at: i64, current: impl Fn() -> bool) -> Result<(), Failure> {
        let Some(config) = self.config.clone() else {
            return Ok(());
        };
        if self.sign_out_marker_failed {
            return if self.status == AuthStatus::SignedOut {
                Ok(())
            } else {
                Err(Failure::CredentialFailed)
            };
        }
        let (mut guard, mut record) = self.read_state()?;
        let checked_at = started_at.max((self.clock)());
        self.cleanup = record.cleanup;
        self.local_reset_available = false;
        if record.phase != Phase::Active {
            self.access = None;
            if matches!(record.cleanup, Cleanup::DeleteCredential { .. }) {
                self.status = AuthStatus::SignedOut;
                let _ = self.finish_local_deletion(&mut guard, record);
                return Ok(());
            }
            if matches!(record.cleanup, Cleanup::Revoke { .. }) {
                self.local_reset_available = self.vault.read()?.is_none();
            }
            self.status = match record.phase {
                Phase::SignedOut if record.cleanup.blocks_connection() => {
                    AuthStatus::RevocationUnconfirmed
                }
                Phase::SignedOut => AuthStatus::SignedOut,
                Phase::ReauthRequired => AuthStatus::ReauthRequired,
                Phase::Pending
                    if self.attempt.as_ref().is_some_and(|attempt| {
                        attempt.epoch == record.epoch && attempt.expires_at > checked_at
                    }) =>
                {
                    AuthStatus::Waiting
                }
                Phase::Pending => {
                    return self.abandon_pending(&mut guard);
                }
                Phase::Active => unreachable!(),
            };
            return Ok(());
        }
        if !current() {
            self.access = None;
            return Err(Failure::Cancelled);
        }
        if self.access.as_ref().is_some_and(|access| {
            access.epoch == record.epoch
                && access.expires_at > checked_at.saturating_add(30)
                && access.session.validate(&config, checked_at).is_ok()
                && !access.token.is_empty()
        }) {
            self.status = AuthStatus::SignedIn;
            return Ok(());
        }
        self.access = None;
        let prior_cleanup = record.cleanup;
        // Persist this before transport. A crash or an uncertain reply must not reuse a predecessor.
        record.phase = Phase::ReauthRequired;
        record.cleanup = Cleanup::Revoke {
            prior_unconfirmed: record.cleanup.prior_unconfirmed(),
        };
        guard.write(&record)?;
        self.cleanup = record.cleanup;
        let credential = self
            .vault
            .read()?
            .and_then(|credential| credential.for_epoch(&record.epoch))
            .ok_or(Failure::ReauthRequired)?;
        if self.cleanup_only
            || !credential.account_grant
            || credential.epoch != record.epoch
            || !credential.cleanup_refresh_tokens.is_empty()
            || credential.issuer != config.issuer
            || credential.resource != config.resource
            || credential.session.as_ref().is_none_or(|session| {
                session
                    .validate_grant(&config, started_at.max((self.clock)()))
                    .is_err()
            })
        {
            return Err(Failure::ReauthRequired);
        }
        let tokens = match self.transport.exchange(
            &config,
            &TokenRequest::RefreshToken {
                refresh_token: &credential.refresh_token,
            },
        ) {
            Ok(tokens) => tokens,
            Err(Failure::OfflineBeforeSend) if current() => {
                record.phase = Phase::Active;
                record.cleanup = prior_cleanup;
                guard.write(&record)?;
                // A cancelled generation cannot reopen a grant during the state write.
                if !current() {
                    record.phase = Phase::ReauthRequired;
                    record.cleanup = Cleanup::Revoke {
                        prior_unconfirmed: prior_cleanup.prior_unconfirmed(),
                    };
                    guard.write(&record)?;
                    return Err(Failure::Cancelled);
                }
                self.cleanup = record.cleanup;
                return Err(Failure::Offline);
            }
            Err(_) if !current() => return Err(Failure::Cancelled),
            Err(_) => return Err(Failure::ReauthRequired),
        };
        self.accept(
            &mut guard,
            record,
            tokens,
            Some(&credential),
            started_at,
            current,
        )
    }

    /// Close local authority before returning any old proof for optional revocation.
    /// A durable closed marker permits retrying a failed keyring deletion.
    pub(super) fn sign_out(&mut self) -> Result<Option<RemoteLogout>, Failure> {
        self.access = None;
        if self
            .attempt
            .take()
            .is_some_and(|attempt| attempt.upgrade.is_some())
        {
            self.mark_remote_logout_unconfirmed();
        }
        self.access_update = None;
        self.sign_out_marker_failed = true;
        self.status = AuthStatus::CredentialFailed;
        let mut guard = self.state.lock()?;
        let previous = guard.read().unwrap_or(Record {
            phase: Phase::SignedOut,
            cleanup: Cleanup::Revoke {
                prior_unconfirmed: true,
            },
            ..Record::default()
        });
        let cleanup = Cleanup::DeleteCredential {
            prior_unconfirmed: previous.cleanup.prior_unconfirmed()
                || previous.phase == Phase::Pending,
        };
        let mut record = match guard.replace(Phase::SignedOut, cleanup) {
            Ok(record) => record,
            Err(error) => {
                let saved = self.vault.read()?;
                if !self.owns_credential(saved.as_ref()) {
                    return Err(error);
                }
                self.vault.write(None)?;
                self.status = AuthStatus::SignedOut;
                self.cleanup = Cleanup::RemoteUnconfirmed;
                return Ok(self.logout_proof(&previous, saved));
            }
        };
        self.sign_out_marker_failed = false;
        self.status = AuthStatus::SignedOut;
        self.cleanup = record.cleanup;
        self.local_reset_available = false;
        let Ok(saved) = self.vault.read() else {
            if !matches!(previous.cleanup, Cleanup::DeleteCredential { .. }) {
                Self::record_remote_logout_failure(
                    &self.state.remote_logout_warning_path(),
                    &self.remote_logout_failed,
                );
            }
            return Ok(None);
        };
        if !self.owns_credential(saved.as_ref()) {
            return Err(Failure::CredentialFailed);
        }
        if saved.is_none()
            && !matches!(previous.cleanup, Cleanup::DeleteCredential { .. })
            && (previous.phase != Phase::SignedOut || previous.cleanup.blocks_connection())
        {
            record.cleanup = Cleanup::DeleteCredential {
                prior_unconfirmed: true,
            };
        }
        let request = self.logout_proof(&previous, saved);
        let _ = self.finish_local_deletion(&mut guard, record);
        Ok(request)
    }

    fn logout_proof(&self, previous: &Record, saved: Option<Credential>) -> Option<RemoteLogout> {
        if previous.phase == Phase::Pending
            || matches!(previous.cleanup, Cleanup::DeleteCredential { .. })
        {
            return None;
        }
        Some(RemoteLogout::new(self.config.clone()?, saved?))
    }

    fn can_reset_record(&self, record: &Record) -> bool {
        self.attempt.is_none()
            && (matches!(record.cleanup, Cleanup::Revoke { .. })
                || (self.cleanup_only && record.phase == Phase::Pending))
    }

    /// Read team recovery from disk without refreshing or revoking a grant.
    pub(super) fn workspace_recovery(&self) -> Result<super::workspace_grants::Recovery, Failure> {
        let record = self.state.lock()?.read()?;
        let recoverable = self.can_reset_record(&record);
        Ok(super::workspace_grants::Recovery {
            local_reset_available: recoverable
                && self.vault.read().is_ok_and(|proof| proof.is_none()),
            remote_revocation_unconfirmed: record.cleanup.blocks_connection()
                || record.cleanup.prior_unconfirmed()
                || (record.phase == Phase::Pending && self.attempt.is_none()),
        })
    }

    /// Permit reconnect only after explicit confirmation and an empty vault.
    /// Keep uncertainty about older remote connections; never delete source bytes.
    pub fn reset_local(&mut self) -> Result<(), Failure> {
        let mut guard = self.state.lock()?;
        let record = guard.read()?;
        if !self.can_reset_record(&record) || self.vault.read()?.is_some() {
            return Err(Failure::Unavailable);
        }
        guard.replace(Phase::SignedOut, Cleanup::RemoteUnconfirmed)?;
        self.access = None;
        self.attempt = None;
        self.cleanup = Cleanup::RemoteUnconfirmed;
        self.local_reset_available = false;
        self.status = AuthStatus::SignedOut;
        Ok(())
    }
}

/// The Personal adapter requires exactly one eligible Personal workspace.
fn personal_anchor(eligible: &[EligibleWorkspace]) -> Result<&Workspace, Failure> {
    let mut personal = eligible
        .iter()
        .filter(|entry| entry.workspace.kind == WorkspaceKind::Personal);
    let first = personal.next().ok_or(Failure::InvalidResponse)?;
    if personal.next().is_some() || first.membership_generation.is_some() {
        return Err(Failure::InvalidResponse);
    }
    Ok(&first.workspace)
}

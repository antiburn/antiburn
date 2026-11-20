//! Serialize authorization, rotation, and cleanup under the controller mutex.
//! Closed state survives failure. Neither browser cancellation nor account
//! cleanup owns capture requests or compressed retry bytes.

use super::{
    AuthStatus,
    auth::{
        self, Attempt, Callback, Config, Credential, EligibleWorkspace, Failure, Session,
        TokenRequest, Transport, Vault, Workspace, WorkspaceKind,
    },
    loopback::Listener,
    state::{Cleanup, Guard, Phase, Record, StateStore},
};

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
    pub transport: Box<dyn Transport>,
    pub state: StateStore,
    pub attempt: Option<Attempt>,
    access: Option<Access>,
    clock: Box<dyn Fn() -> i64 + Send>,
    pub status: AuthStatus,
    pub cleanup: Cleanup,
    pub local_reset_available: bool,
    pub generation: u64,
    cleanup_only: bool,
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
            transport,
            state,
            attempt: None,
            access: None,
            clock: Box::new(super::now),
            status: AuthStatus::Idle,
            cleanup: Cleanup::None,
            local_reset_available: false,
            generation: 0,
            cleanup_only: false,
        }
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

    /// Return the configured canonical API origin, not access authority.
    pub(super) fn resource_origin(&self) -> Option<&str> {
        self.config.as_ref().map(|config| config.resource.as_str())
    }

    /// Fetch a fresh eligible list without exposing the bearer. Missing or changed
    /// Personal anchors fail closed. Callers must match each exact Team generation.
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
        let result = self.transport.workspaces(config, &access.token)?;
        if !current() || self.session().is_none() {
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
        if response.status == 401 || (response.status == 403 && membership_generation.is_none()) {
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
        let (mut guard, record) = self.read_state()?;
        if record.cleanup.blocks_connection() || record.phase == Phase::Active {
            return Err(Failure::Unavailable);
        }
        if record.phase == Phase::Pending {
            self.abandon_pending(&mut guard, record)?;
            return Err(Failure::ReauthRequired);
        }
        if self.vault.read()?.is_some() {
            return Err(Failure::ReauthRequired);
        }
        // An explicit sign-in replaces any pending listener, including one lost on restart.
        self.attempt = None;
        self.vault.write(None)?;
        let listener = Listener::bind()?;
        let record = guard.replace(Phase::Pending, record.cleanup)?;
        let attempt = Attempt {
            state: auth::random()?,
            verifier: auth::random()?,
            callback: listener.callback().to_owned(),
            expires_at: now + 600,
            epoch: record.epoch,
            generation,
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
        self.access = None;
        self.attempt = Some(attempt.clone());
        self.status = AuthStatus::Opening;
        Ok((url.to_string(), listener, attempt))
    }

    /// Clear only this pending attempt after the opener proves no browser handoff.
    /// Do not use this for an uncertain launch or a lost callback.
    pub fn abort_browser_launch(&mut self, attempt: &Attempt) -> Result<(), Failure> {
        if self.attempt.as_ref().is_none_or(|pending| {
            pending.epoch != attempt.epoch || pending.generation != attempt.generation
        }) {
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
            let (mut guard, record) = self.read_state()?;
            if record.epoch == attempt.epoch && record.phase == Phase::Pending {
                return self.abandon_pending(&mut guard, record);
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
    fn abandon_pending(&mut self, guard: &mut Guard, record: Record) -> Result<(), Failure> {
        self.access = None;
        self.attempt = None;
        let record = guard.replace(
            Phase::SignedOut,
            Cleanup::Revoke {
                prior_unconfirmed: record.cleanup.prior_unconfirmed(),
            },
        )?;
        self.cleanup = record.cleanup;
        self.status = AuthStatus::RevocationUnconfirmed;
        self.local_reset_available = self.vault.read()?.is_none();
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
        let session = tokens
            .validate(&config, started_at)
            .and_then(|()| {
                if started_at.saturating_add(tokens.expires_in as i64) <= (self.clock)() {
                    return Err(Failure::InvalidResponse);
                }
                self.transport.session(&config, &tokens.access_token)
            })
            .and_then(|account| {
                if !tokens.matches_session(&account) {
                    return Err(Failure::InvalidResponse);
                }
                if !current() {
                    return Err(Failure::Cancelled);
                }
                let eligible = self.transport.workspaces(&config, &tokens.access_token)?;
                let session = account.with_personal(personal_anchor(&eligible)?.clone())?;
                let checked_at = started_at.max((self.clock)());
                session.validate(&config, checked_at)?;
                let expires_at = started_at
                    .saturating_add(tokens.expires_in as i64)
                    .min(session.expires_at)
                    .min(session.access_expires_at);
                if expires_at <= checked_at {
                    return Err(Failure::InvalidResponse);
                }
                if previous.is_some_and(|previous| {
                    previous
                        .session
                        .as_ref()
                        .is_none_or(|saved| !saved.same_grant(&session))
                        || previous.refresh_token == tokens.refresh_token
                }) {
                    return Err(Failure::InvalidResponse);
                }
                Ok((session, expires_at))
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
        guard.write(&record)?;
        self.cleanup = record.cleanup;
        // Durable writes can outlast access or overlap account cancellation.
        let cancelled = !current();
        if cancelled || expires_at <= (self.clock)() {
            self.discard_tokens(guard, record, &config, &tokens, previous)?;
            return Err(if cancelled {
                Failure::Cancelled
            } else {
                Failure::ReauthRequired
            });
        }
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
    /// Confirm every proof's revocation before persisting permission to delete them.
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
        };
        if self.vault.write(Some(&cleanup)).is_err() {
            record.cleanup = Cleanup::Revoke {
                prior_unconfirmed: true,
            };
            let marked = guard.write(&record);
            self.cleanup = record.cleanup;
            let _ = self.transport.revoke(config, &tokens.refresh_token);
            marked?;
            return Err(Failure::CredentialFailed);
        }
        self.cleanup = record.cleanup;
        guard.write(&record)?;
        let mut confirmed = true;
        for proof in std::iter::once(&cleanup.refresh_token).chain(&cleanup.cleanup_refresh_tokens)
        {
            confirmed &= self.transport.revoke(config, proof).is_ok();
        }
        if !confirmed {
            return Ok(());
        }
        record.cleanup = Cleanup::DeleteCredential {
            prior_unconfirmed: record.cleanup.prior_unconfirmed(),
        };
        guard.write(&record)?;
        self.finish_local_deletion(guard, record)
    }

    /// Finish previously confirmed remote revocation. A failed vault deletion or
    /// final state write remains retryable without restoring authenticated access.
    fn finish_local_deletion(
        &mut self,
        guard: &mut Guard,
        mut record: Record,
    ) -> Result<(), Failure> {
        self.cleanup = record.cleanup;
        self.local_reset_available = false;
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
        let (mut guard, mut record) = self.read_state()?;
        let checked_at = started_at.max((self.clock)());
        self.cleanup = record.cleanup;
        self.local_reset_available = false;
        if record.phase != Phase::Active {
            self.access = None;
            if matches!(record.cleanup, Cleanup::DeleteCredential { .. }) {
                return self.finish_local_deletion(&mut guard, record);
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
                    return self.abandon_pending(&mut guard, record);
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
        let credential = self.vault.read()?.ok_or(Failure::ReauthRequired)?;
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

    /// Flush a new signed-out epoch before revoking all retained proofs.
    /// Retry secure deletion only after its durable remote-confirmation marker.
    pub fn sign_out(&mut self) -> Result<(), Failure> {
        self.access = None;
        self.attempt = None;
        let mut guard = self.state.lock()?;
        let previous = guard.read().unwrap_or(Record {
            phase: Phase::SignedOut,
            cleanup: Cleanup::Revoke {
                prior_unconfirmed: true,
            },
            ..Record::default()
        });
        // This marker blocks restored credentials even when the keyring or revocation fails.
        let cleanup = if matches!(previous.cleanup, Cleanup::DeleteCredential { .. }) {
            previous.cleanup
        } else {
            Cleanup::Revoke {
                prior_unconfirmed: previous.cleanup.prior_unconfirmed(),
            }
        };
        let mut record = guard.replace(Phase::SignedOut, cleanup)?;
        self.status = AuthStatus::RevocationUnconfirmed;
        self.cleanup = record.cleanup;
        self.local_reset_available = false;
        if matches!(cleanup, Cleanup::DeleteCredential { .. }) {
            return self.finish_local_deletion(&mut guard, record);
        }
        let credential = self.vault.read()?;
        if credential.is_none() {
            if previous.phase == Phase::SignedOut && !previous.cleanup.blocks_connection() {
                record.cleanup = previous.cleanup.prior_warning_only();
                guard.write(&record)?;
                self.cleanup = record.cleanup;
                self.status = AuthStatus::SignedOut;
            } else {
                self.local_reset_available = true;
            }
            return Ok(());
        }
        if let Some(credential) = credential {
            let config = self.config.as_ref().ok_or(Failure::Unavailable)?;
            if credential.issuer != config.issuer || credential.resource != config.resource {
                return Err(Failure::CredentialFailed);
            }
            let mut confirmed = true;
            for proof in
                std::iter::once(&credential.refresh_token).chain(&credential.cleanup_refresh_tokens)
            {
                confirmed &= self.transport.revoke(config, proof).is_ok();
            }
            if !confirmed {
                return Ok(());
            }
        }
        record.cleanup = Cleanup::DeleteCredential {
            prior_unconfirmed: record.cleanup.prior_unconfirmed(),
        };
        guard.write(&record)?;
        self.finish_local_deletion(&mut guard, record)
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

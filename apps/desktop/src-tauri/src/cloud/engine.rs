//! Serialize authorization, rotation, and cleanup under the controller mutex.
//! Closed state survives failure. Neither browser cancellation nor account
//! cleanup owns capture requests or compressed retry bytes.

use super::{
    AuthStatus,
    auth::{
        self, Attempt, Callback, Config, Credential, EligibleDestination, Failure, Session,
        TokenRequest, Transport, Vault,
    },
    loopback::Listener,
    state::{Cleanup, Guard, Phase, Record, StateStore},
};

struct Access {
    token: String,
    expires_at: i64,
    epoch: String,
    session: Session,
    destinations: Option<Vec<EligibleDestination>>,
}

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
    invitation_support: Option<bool>,
    clock: Box<dyn Fn() -> i64 + Send>,
    pub status: AuthStatus,
    pub cleanup: Cleanup,
    sign_out_marker_failed: bool,
    pub generation: u64,
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
            invitation_support: None,
            clock: Box::new(super::now),
            status: AuthStatus::Idle,
            cleanup: Cleanup::None,
            sign_out_marker_failed: false,
            generation: 0,
        }
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

    /// Return view data only while the matching device grant is live.
    pub(super) fn destinations(&self) -> Option<&[EligibleDestination]> {
        self.session()?;
        self.access.as_ref()?.destinations.as_deref()
    }

    /// Match one exact destination for this account. No Personal fallback.
    pub(super) fn destination(
        &self,
        account: &str,
        destination: &str,
    ) -> Option<&EligibleDestination> {
        (self.session()?.user.id == account).then_some(())?;
        self.destinations()?
            .iter()
            .find(|entry| entry.workspace.id == destination)
    }

    /// Update the setup view from the saved route or from Personal before any route is saved.
    pub(super) fn select_destination_view(&mut self, destination: Option<&str>) {
        let Some(access) = &mut self.access else {
            return;
        };
        access.session.workspace = access.destinations.as_ref().and_then(|entries| {
            entries
                .iter()
                .find(|entry| {
                    destination.map_or(
                        entry.workspace.kind == auth::WorkspaceKind::Personal,
                        |id| entry.workspace.id == id,
                    )
                })
                .map(|entry| entry.workspace.clone())
        });
    }

    /// Probe once when a saved session restores without a browser sign-in.
    pub(super) fn check_invitation_support(&mut self) {
        if self.invitation_support.is_none() && self.session().is_some() {
            self.invitation_support = Some(
                self.config
                    .as_ref()
                    .is_some_and(|config| self.transport.supports_invitations(config)),
            );
        }
    }

    /// Only a live account and a positive development capability enable invitations.
    pub(super) fn invitations_enabled(&self) -> bool {
        self.invitation_support == Some(true) && self.session().is_some()
    }

    /// Run an invitation request with current account authority. Keep bearer
    /// proof in Rust and reject stale renderer actions before dispatch.
    pub(super) fn with_invitation_access<T>(
        &mut self,
        account_id: &str,
        current: impl Fn() -> bool,
        request: impl FnOnce(&Config, &str, &dyn Fn() -> bool) -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        if !current() || self.invitation_support != Some(true) {
            return Err(Failure::Unavailable);
        }
        self.refresh((self.clock)(), &current)?;
        let config = self.config.as_ref().ok_or(Failure::Unavailable)?;
        let access = self.access.as_ref().ok_or(Failure::ReauthRequired)?;
        if !current() || access.session.user.id != account_id {
            return Err(Failure::Cancelled);
        }
        access.session.validate(config, (self.clock)())?;
        let can_send = || current() && access.expires_at > (self.clock)();
        let result = request(config, &access.token, &can_send);
        if !current() {
            return Err(Failure::Cancelled);
        }
        result
    }

    /// Send one bounded capture API request without returning access proof.
    /// Reject a changed service before credentials are used. Auth rotation uses
    /// only the account fence; content pause blocks dispatch without revocation.
    pub(super) fn capture_request(
        &mut self,
        identity: &antiburn_cloud_capture::CaptureIdentity,
        destination_scope: (&str, Option<&str>),
        route: super::upload::Route,
        body: &impl serde::Serialize,
        account_current: &dyn Fn() -> bool,
        current: &dyn Fn() -> bool,
    ) -> Result<super::upload::ApiResponse, antiburn_cloud_capture::TransportError> {
        use antiburn_cloud_capture::TransportError;
        let (resource_origin, membership_generation) = destination_scope;
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
            || self
                .destination(&identity.account_id, &identity.destination_id)
                .is_none_or(|entry| entry.membership_generation.as_deref() != membership_generation)
            || access.expires_at <= (self.clock)()
            || access.session.validate(config, (self.clock)()).is_err()
        {
            return Err(TransportError::Unauthorized);
        }
        let response = super::upload::api_request(config, &access.token, identity, route, body)?;
        if response.status == 401 {
            self.access = None;
            self.status = if account_current()
                && serde_json::from_slice::<auth::DeviceAuthError>(&response.body)
                    .is_ok_and(|error| error.needs_sign_in())
            {
                self.sign_out()
                    .err()
                    .map_or(AuthStatus::ReauthRequired, AuthStatus::from)
            } else {
                AuthStatus::ReauthRequired
            };
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
        let config = self.config.clone().ok_or(Failure::Unavailable)?;
        let (mut guard, record) = match self.read_state() {
            Ok(state)
                if !self.sign_out_marker_failed
                    && state.1.phase == Phase::Active
                    && self.session().is_some() =>
            {
                return Err(Failure::Unavailable);
            }
            Ok(state)
                if !self.sign_out_marker_failed
                    && state.1.phase == Phase::SignedOut
                    && !state.1.cleanup.blocks_connection()
                    && (!state.1.epoch.is_empty() || self.vault.read()?.is_none()) =>
            {
                state
            }
            _ => {
                self.sign_out()?;
                let prior_unconfirmed = self.cleanup.prior_unconfirmed();
                let (guard, mut record) = self.read_state()?;
                if prior_unconfirmed {
                    record.cleanup = Cleanup::RemoteUnconfirmed;
                }
                (guard, record)
            }
        };
        // An explicit sign-in replaces any pending listener, including one lost on restart.
        self.attempt = None;
        let listener = Listener::bind()?;
        let record = guard.replace(Phase::Pending, record.cleanup.prior_warning_only())?;
        self.sign_out_marker_failed = false;
        self.cleanup = record.cleanup;
        let attempt = Attempt {
            state: auth::random()?,
            verifier: auth::random()?,
            callback: listener.callback().to_owned(),
            expires_at: now + 600,
            epoch: record.epoch,
            generation,
        };
        let mut url = config.authorization.clone();
        url.query_pairs_mut().extend_pairs([
            ("redirect_uri", attempt.callback.as_str()),
            ("state", attempt.state.as_str()),
            (
                "code_challenge",
                auth::challenge(&attempt.verifier).as_str(),
            ),
            ("code_challenge_method", "S256"),
            ("device_name", auth::DEVICE_NAME),
        ]);
        let supports_invitations = self.transport.supports_invitations(&config);
        self.invitation_support = Some(supports_invitations);
        if supports_invitations {
            url.query_pairs_mut()
                .append_pair("native_invitations", "true");
        }
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
                return self.abandon_pending(&mut guard);
            }
        }
        if self.access.is_none() && generation > 0 {
            self.status = AuthStatus::Cancelled;
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
        let tokens = self
            .transport
            .exchange(
                config,
                &TokenRequest::AuthorizationCode {
                    code: &code,
                    code_verifier: &attempt.verifier,
                },
            )
            .map_err(|failure| match failure {
                Failure::OfflineBeforeSend => Failure::Offline,
                other => other,
            })?;
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
            .and_then(|session| {
                let checked_at = started_at.max((self.clock)());
                session.validate(&config, checked_at)?;
                let expires_at = started_at
                    .saturating_add(tokens.expires_in as i64)
                    .min(session.expires_at)
                    .min(session.access_expires_at);
                if expires_at <= checked_at {
                    return Err(Failure::InvalidResponse);
                }
                if !tokens.matches_session(&session) {
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
        let (mut session, expires_at) = session?;
        session.workspace = None;
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
            destinations: None,
        });
        self.status = AuthStatus::SignedIn;
        let _ = self.refresh_destinations(&current);
        self.select_destination_view(None);
        Ok(())
    }

    /// Update upload eligibility separately from account authentication.
    pub(super) fn refresh_destinations(
        &mut self,
        current: &dyn Fn() -> bool,
    ) -> Result<(), Failure> {
        if !current() {
            return Err(Failure::Cancelled);
        }
        let config = self.config.as_ref().ok_or(Failure::Unavailable)?;
        let access = self.access.as_mut().ok_or(Failure::ReauthRequired)?;
        let selected = access
            .session
            .workspace
            .as_ref()
            .map(|workspace| workspace.id.clone());
        let result = self.transport.destinations(config, &access.token);
        access.destinations = None;
        access.session.workspace = None;
        if !current() || access.expires_at <= (self.clock)() {
            return Err(Failure::Cancelled);
        }
        access.destinations = Some(result?);
        self.select_destination_view(selected.as_deref());
        Ok(())
    }

    /// Retain original and returned proofs before state writes can fail.
    /// Keep any proof Cloud may still accept for revocation.
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
        if !self.revoke_saved(config, &cleanup) {
            return Ok(());
        }
        record.cleanup = Cleanup::DeleteCredential {
            prior_unconfirmed: record.cleanup.prior_unconfirmed(),
        };
        guard.write(&record)?;
        self.finish_local_deletion(guard, record)
    }

    /// Ask Cloud to revoke each captured proof. Failure never confirms revocation.
    fn revoke_saved(&self, config: &Config, credential: &Credential) -> bool {
        let mut confirmed = true;
        for proof in
            std::iter::once(&credential.refresh_token).chain(&credential.cleanup_refresh_tokens)
        {
            confirmed &= self.transport.revoke(config, proof).is_ok();
        }
        confirmed
    }

    /// Do not send or remove another service origin's saved proof.
    fn owns_credential(&self, credential: Option<&Credential>) -> bool {
        credential.is_none_or(|credential| {
            self.config.as_ref().is_some_and(|config| {
                credential.issuer == config.issuer && credential.resource == config.resource
            })
        })
    }

    /// Finish authorized local deletion. A failed vault deletion or
    /// final state write remains retryable without restoring authenticated access.
    fn finish_local_deletion(
        &mut self,
        guard: &mut Guard,
        mut record: Record,
    ) -> Result<(), Failure> {
        self.cleanup = record.cleanup;
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
                Err(Failure::SignOutFailed)
            };
        }
        let (mut guard, mut record) = self.read_state()?;
        let checked_at = started_at.max((self.clock)());
        self.cleanup = record.cleanup;
        if record.phase != Phase::Active {
            self.access = None;
            if matches!(record.cleanup, Cleanup::DeleteCredential { .. }) {
                self.status = AuthStatus::SignedOut;
                if self
                    .vault
                    .read()
                    .is_ok_and(|value| self.owns_credential(value.as_ref()))
                {
                    let _ = self.finish_local_deletion(&mut guard, record);
                }
                return Ok(());
            }
            self.status = match record.phase {
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
        let credential = self.vault.read()?.ok_or(Failure::ReauthRequired)?;
        if !credential.account_grant
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
                return Err(Failure::OfflineBeforeSend);
            }
            Err(_) if !current() => return Err(Failure::Cancelled),
            Err(Failure::ReauthRequired) => {
                drop(guard);
                self.sign_out()?;
                return Err(Failure::ReauthRequired);
            }
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

    /// Close local access without waiting for Cloud. Explicit sign-out may send the
    /// returned proof later. Neither the proof nor its outcome can change local state.
    pub fn sign_out(&mut self) -> Result<Option<auth::RemoteLogout>, Failure> {
        self.access = None;
        self.attempt = None;
        self.sign_out_marker_failed = true;
        let mut guard = self.state.lock().map_err(|_| Failure::SignOutFailed)?;
        let previous = guard.read().unwrap_or(Record {
            phase: Phase::SignedOut,
            cleanup: Cleanup::Revoke {
                prior_unconfirmed: true,
            },
            ..Record::default()
        });
        let already_closed = matches!(previous.cleanup, Cleanup::DeleteCredential { .. });
        let prior_unconfirmed =
            previous.cleanup.prior_unconfirmed() || previous.phase == Phase::Pending;
        let cleanup = Cleanup::DeleteCredential {
            prior_unconfirmed: prior_unconfirmed || !already_closed,
        };
        let mut record = match guard.replace(Phase::SignedOut, cleanup) {
            Ok(record) => record,
            Err(_) => {
                if let Ok(saved) = self.vault.read()
                    && self.owns_credential(saved.as_ref())
                    && self.vault.write(None).is_ok()
                {
                    self.status = AuthStatus::SignedOut;
                    self.cleanup = Cleanup::RemoteUnconfirmed;
                    return Ok(self.logout_proof(&previous, saved));
                }
                return Err(Failure::SignOutFailed);
            }
        };
        self.sign_out_marker_failed = false;
        self.status = AuthStatus::SignedOut;
        self.cleanup = record.cleanup;
        let Ok(saved) = self.vault.read() else {
            return Ok(None);
        };
        if !self.owns_credential(saved.as_ref()) {
            return Ok(None);
        }
        if saved.is_none()
            && previous.phase == Phase::SignedOut
            && !previous.cleanup.blocks_connection()
        {
            record.cleanup = Cleanup::DeleteCredential { prior_unconfirmed };
        }
        let logout = self.logout_proof(&previous, saved);
        let _ = self.finish_local_deletion(&mut guard, record);
        Ok(logout)
    }

    /// Never send proof left from an earlier closed epoch as part of a browser attempt.
    fn logout_proof(
        &self,
        previous: &Record,
        saved: Option<Credential>,
    ) -> Option<auth::RemoteLogout> {
        if previous.phase == Phase::Pending
            || matches!(previous.cleanup, Cleanup::DeleteCredential { .. })
        {
            return None;
        }
        Some(auth::RemoteLogout::from_credential(
            self.config.clone()?,
            saved?,
        ))
    }
}

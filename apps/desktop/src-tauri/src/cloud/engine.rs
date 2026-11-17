use super::{
    AuthStatus,
    auth::{
        self, Attempt, Callback, Config, Credential, Failure, Session, TokenRequest, Transport,
        Vault,
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

pub struct Engine {
    pub config: Option<Config>,
    pub vault: Box<dyn Vault>,
    pub transport: Box<dyn Transport>,
    pub state: StateStore,
    pub attempt: Option<Attempt>,
    access: Option<Access>,
    pub status: AuthStatus,
    pub cleanup: Cleanup,
    pub local_reset_available: bool,
    pub generation: u64,
}

impl Engine {
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
            status: AuthStatus::Idle,
            cleanup: Cleanup::None,
            local_reset_available: false,
            generation: 0,
        }
    }

    pub fn session(&self) -> Option<&Session> {
        self.access.as_ref().map(|access| &access.session)
    }

    pub fn allows_account_cleanup(&self, requested: u64, latest: u64) -> bool {
        requested == latest && requested >= self.generation
    }

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

    pub fn begin(
        &mut self,
        now: i64,
        generation: u64,
    ) -> Result<(String, Listener, Attempt), Failure> {
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
        self.access = None;
        self.attempt = Some(attempt.clone());
        self.status = AuthStatus::Opening;
        Ok((url.to_string(), listener, attempt))
    }

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

    pub fn callback(
        &mut self,
        attempt: &Attempt,
        callback: Callback,
        now: i64,
        current: impl Fn() -> bool,
    ) -> Result<(), Failure> {
        let (mut guard, mut record) = self.read_state()?;
        if !current()
            || now >= attempt.expires_at
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
        self.accept(&mut guard, record, tokens, None, now, current)
    }

    fn accept(
        &mut self,
        guard: &mut Guard,
        mut record: Record,
        tokens: auth::Tokens,
        previous: Option<&Credential>,
        now: i64,
        current: impl Fn() -> bool,
    ) -> Result<(), Failure> {
        let config = self.config.clone().ok_or(Failure::Unavailable)?;
        let session = tokens
            .validate(&config, now)
            .and_then(|()| self.transport.session(&config, &tokens.access_token))
            .and_then(|session| {
                session.validate(&config, now)?;
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
                Ok(session)
            });
        if !current() || session.is_err() {
            self.discard_tokens(guard, record, &config, &tokens, previous)?;
            return Err(if current() {
                Failure::ReauthRequired
            } else {
                Failure::Cancelled
            });
        }
        let session = session?;
        let credential = Credential {
            epoch: record.epoch.clone(),
            issuer: config.issuer.clone(),
            resource: config.resource.clone(),
            refresh_token: tokens.refresh_token.clone(),
            cleanup_refresh_tokens: Vec::new(),
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
        // Check again after durable writes. Logout can invalidate work during an OS-vault call.
        if !current() {
            self.discard_tokens(guard, record, &config, &tokens, previous)?;
            return Err(Failure::Cancelled);
        }
        self.access = Some(Access {
            token: tokens.access_token,
            expires_at: now
                .saturating_add(tokens.expires_in as i64)
                .min(session.expires_at)
                .min(session.access_expires_at),
            epoch: record.epoch,
            session,
        });
        self.status = AuthStatus::SignedIn;
        Ok(())
    }

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

    pub fn refresh(&mut self, now: i64, current: impl Fn() -> bool) -> Result<(), Failure> {
        let Some(config) = self.config.clone() else {
            return Ok(());
        };
        let (mut guard, mut record) = self.read_state()?;
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
                        attempt.epoch == record.epoch && attempt.expires_at > now
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
                && access.expires_at > now + 30
                && access.session.expires_at > now
                && !access.token.is_empty()
        }) {
            self.status = AuthStatus::SignedIn;
            return Ok(());
        }
        self.access = None;
        // Persist this before transport. A crash or an uncertain reply must not reuse a predecessor.
        record.phase = Phase::ReauthRequired;
        record.cleanup = Cleanup::Revoke {
            prior_unconfirmed: record.cleanup.prior_unconfirmed(),
        };
        guard.write(&record)?;
        self.cleanup = record.cleanup;
        let credential = self.vault.read()?.ok_or(Failure::ReauthRequired)?;
        if credential.epoch != record.epoch
            || !credential.cleanup_refresh_tokens.is_empty()
            || credential.issuer != config.issuer
            || credential.resource != config.resource
            || credential
                .session
                .as_ref()
                .is_none_or(|session| session.validate_grant(&config, now).is_err())
        {
            return Err(Failure::ReauthRequired);
        }
        let tokens = self
            .transport
            .exchange(
                &config,
                &TokenRequest::RefreshToken {
                    refresh_token: &credential.refresh_token,
                },
            )
            .map_err(|_| Failure::ReauthRequired)?;
        self.accept(&mut guard, record, tokens, Some(&credential), now, current)
    }

    pub fn sign_out(&mut self) -> Result<(), Failure> {
        self.access = None;
        self.attempt = None;
        let mut guard = self.state.lock()?;
        let previous = guard.read().unwrap_or(Record {
            phase: Phase::SignedOut,
            cleanup: Cleanup::Revoke {
                prior_unconfirmed: false,
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

    pub fn reset_local(&mut self) -> Result<(), Failure> {
        self.access = None;
        self.attempt = None;
        let mut guard = self.state.lock()?;
        let record = guard.read()?;
        if !matches!(record.cleanup, Cleanup::Revoke { .. }) || self.vault.read()?.is_some() {
            return Err(Failure::Unavailable);
        }
        guard.replace(Phase::SignedOut, Cleanup::RemoteUnconfirmed)?;
        self.cleanup = Cleanup::RemoteUnconfirmed;
        self.local_reset_available = false;
        self.status = AuthStatus::SignedOut;
        Ok(())
    }
}

//! Shared provider credentials, generation fences, and bounded preparation.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::Duration;

#[cfg(test)]
use crate::jev::client::TypeSafeClient;
use crate::jev::config::{SystemOneConnection, SystemOneProvider};
use crate::store::Store;

const PREPARATION_WORKERS: usize = 4;

#[derive(Clone, Copy)]
pub(crate) enum BlockingPreparationStage {
    WorkProbe,
    PreparationStart,
}

impl BlockingPreparationStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::WorkProbe => "work_probe",
            Self::PreparationStart => "preparation_start",
        }
    }
}

fn trace_blocking_preparation(
    stage: BlockingPreparationStage,
    slot_wait: Duration,
    elapsed: Duration,
    succeeded: bool,
) {
    let stage = stage.as_str();
    let slot_wait_ms = slot_wait.as_millis();
    let elapsed_ms = elapsed.as_millis();
    if slot_wait >= Duration::from_secs(1) || elapsed >= Duration::from_secs(1) {
        ::tracing::debug!(
            event = "burn_check_blocking_preparation",
            stage,
            slot_wait_ms,
            elapsed_ms,
            succeeded,
        );
    } else {
        ::tracing::trace!(
            event = "burn_check_blocking_preparation",
            stage,
            slot_wait_ms,
            elapsed_ms,
            succeeded,
        );
    }
}

pub(crate) async fn run_blocking_preparation<T: Send + 'static>(
    stage: BlockingPreparationStage,
    prepare: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    let waiting = std::time::Instant::now();
    static SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    let permit = SLOTS
        .get_or_init(|| tokio::sync::Semaphore::new(PREPARATION_WORKERS))
        .acquire()
        .await?;
    let slot_wait = waiting.elapsed();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let started = std::time::Instant::now();
        let result = prepare();
        trace_blocking_preparation(stage, slot_wait, started.elapsed(), result.is_ok());
        result
    })
    .await?
}

pub(crate) struct WorkerHandle {
    request_admission: Mutex<()>,
    system_one: RwLock<(SystemOneConnection, Option<String>)>,
    key_generation: AtomicU64,
    runtime_enabled: AtomicBool,
    authentication_rejected: AtomicBool,
}

pub(crate) struct ExecutionSnapshot {
    pub(crate) connection: SystemOneConnection,
    pub(crate) credential: Option<String>,
    pub(crate) generation: u64,
}

impl WorkerHandle {
    pub fn set_system_one_connection(
        &self,
        connection: SystemOneConnection,
        credential: Option<String>,
    ) -> Result<(), crate::jev::config::ConnectionValidationError> {
        self.install_system_one_connection(connection, credential, true)
    }

    pub(crate) fn install_system_one_connection(
        &self,
        connection: SystemOneConnection,
        credential: Option<String>,
        enabled: bool,
    ) -> Result<(), crate::jev::config::ConnectionValidationError> {
        connection.validate()?;
        let _admission = self
            .request_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut active = self
            .system_one
            .write()
            .unwrap_or_else(|error| error.into_inner());
        if active.0 != connection
            || active.1 != credential
            || self.runtime_enabled.load(Ordering::Acquire) != enabled
        {
            *active = (connection, credential);
            self.runtime_enabled.store(enabled, Ordering::Release);
            self.key_generation.fetch_add(1, Ordering::AcqRel);
            self.authentication_rejected.store(false, Ordering::Release);
        }
        Ok(())
    }

    /// Disable dispatch and clear the secret. Keep the selected connection.
    pub(crate) fn suspend_system_one(&self) {
        let _admission = self
            .request_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut active = self
            .system_one
            .write()
            .unwrap_or_else(|error| error.into_inner());
        active.1 = None;
        self.runtime_enabled.store(false, Ordering::Release);
        self.key_generation.fetch_add(1, Ordering::AcqRel);
        self.authentication_rejected.store(false, Ordering::Release);
    }

    /// Return the active connection without its resolved credential.
    #[cfg(test)]
    pub(crate) fn system_one_connection(&self) -> SystemOneConnection {
        self.system_one
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .0
            .clone()
    }

    pub(crate) fn execution_snapshot(&self) -> Option<ExecutionSnapshot> {
        let active = self
            .system_one
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let (connection, credential) = &*active;
        if !self.runtime_enabled.load(Ordering::Acquire) || self.authentication_rejected() {
            return None;
        }
        let generation = self.key_generation.load(Ordering::Acquire);
        if matches!(
            connection.provider,
            SystemOneProvider::Jev | SystemOneProvider::Cloudflare
        ) && credential.is_none()
        {
            return None;
        }
        connection.validate().ok()?;
        Some(ExecutionSnapshot {
            connection: connection.clone(),
            credential: credential.clone(),
            generation,
        })
    }

    #[cfg(test)]
    pub(crate) fn execution_client(&self) -> Option<(TypeSafeClient, u64)> {
        let snapshot = self.execution_snapshot()?;
        let client =
            TypeSafeClient::new(snapshot.credential.unwrap_or_else(|| "unused".to_owned())).ok()?;
        Some((client, snapshot.generation))
    }

    pub(crate) fn with_current_generation<T>(
        &self,
        generation: u64,
        action: impl FnOnce() -> T,
    ) -> Option<T> {
        let _active = self
            .system_one
            .read()
            .unwrap_or_else(|error| error.into_inner());
        (self.runtime_enabled.load(Ordering::Acquire)
            && self.key_generation.load(Ordering::Acquire) == generation)
            .then(action)
    }

    pub(crate) fn key_is_current(&self, generation: u64) -> bool {
        let active = self
            .system_one
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let credential_required = matches!(
            active.0.provider,
            SystemOneProvider::Jev | SystemOneProvider::Cloudflare
        );
        self.key_generation.load(Ordering::Acquire) == generation
            && self.runtime_enabled.load(Ordering::Acquire)
            && (!credential_required || active.1.is_some())
            && !self.authentication_rejected()
    }

    pub(crate) fn is_available(&self) -> bool {
        let active = self
            .system_one
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let credential_required = matches!(
            active.0.provider,
            SystemOneProvider::Jev | SystemOneProvider::Cloudflare
        );
        active.0.validate().is_ok()
            && self.runtime_enabled.load(Ordering::Acquire)
            && (!credential_required || active.1.is_some())
            && !self.authentication_rejected()
    }

    pub(crate) fn authentication_rejected(&self) -> bool {
        self.authentication_rejected.load(Ordering::Acquire)
    }

    pub(crate) fn reject_authentication(
        &self,
        store: &Store,
        generation: u64,
    ) -> anyhow::Result<bool> {
        let _admission = self
            .request_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.with_current_generation(generation, || {
            self.authentication_rejected.store(true, Ordering::Release);
            store.set_internal_value_checked("internal:typesafeAuthRejectedV1", "true")?;
            Ok(true)
        })
        .unwrap_or(Ok(false))
    }
}

impl Default for WorkerHandle {
    fn default() -> Self {
        Self {
            request_admission: Mutex::new(()),
            system_one: RwLock::new((SystemOneConnection::jev_default(), None)),
            key_generation: AtomicU64::new(0),
            runtime_enabled: AtomicBool::new(false),
            authentication_rejected: AtomicBool::new(false),
        }
    }
}

#[cfg(test)]
mod tests;

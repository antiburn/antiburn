use super::*;
use crate::cloud::{
    auth::{AccountSession, CAPTURE_SCOPE, Config, EligibleWorkspace, TokenRequest, Tokens},
    engine::Access,
    state::{Cleanup, Phase, StateStore},
    tests::{MemoryVault, session},
};
use std::sync::Arc;

struct NoRequests;

impl crate::cloud::auth::Transport for NoRequests {
    fn exchange(&self, _: &Config, _: &TokenRequest<'_>) -> Result<Tokens, Failure> {
        panic!("a cached, unexpired token does not need to rotate")
    }

    fn session(&self, _: &Config, _: &str) -> Result<AccountSession, Failure> {
        panic!("a cached, unexpired token does not need to rotate")
    }

    fn workspaces(&self, _: &Config, _: &str) -> Result<Vec<EligibleWorkspace>, Failure> {
        panic!("the broker uses the verified Personal workspace without another request")
    }

    fn revoke(&self, _: &Config, _: &str) -> Result<(), Failure> {
        panic!("a broker request does not revoke a grant")
    }
}

fn engine(directory: &std::path::Path, scope: &str) -> Engine {
    let mut session = session();
    session.scope = scope.into();
    let config = Config::parse(&session.iss, &session.resource).unwrap();
    let state = StateStore::new(directory);
    let record = state
        .lock()
        .unwrap()
        .replace(Phase::Active, Cleanup::None)
        .unwrap();
    let mut engine = Engine::new(
        Some(config),
        Box::new(Arc::new(MemoryVault::default())),
        Box::new(NoRequests),
        state,
    )
    .with_clock(|| 1);
    engine.access = Some(Access {
        token: "synthetic-access".into(),
        expires_at: session.access_expires_at,
        epoch: record.epoch,
        session,
    });
    engine
}

#[test]
fn returns_the_verified_personal_workspace_without_another_request() {
    let directory = tempfile::tempdir().unwrap();
    let mut engine = engine(directory.path(), CAPTURE_AND_SESSION_SCOPE);
    let (origin, bearer, personal) = engine.cli_device_session(&|| true).unwrap();
    assert_eq!(origin, "https://api.example");
    assert_eq!(bearer, "synthetic-access");
    assert_eq!(personal, "synthetic-personal");
}

#[test]
fn rejects_capture_only_grants_and_cancelled_reads() {
    for (scope, current, expected) in [
        (CAPTURE_SCOPE, true, Failure::Unavailable),
        (CAPTURE_AND_SESSION_SCOPE, false, Failure::Cancelled),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut engine = engine(directory.path(), scope);
        assert!(matches!(
            engine.cli_device_session(&|| current),
            Err(error) if error == expected
        ));
    }
}

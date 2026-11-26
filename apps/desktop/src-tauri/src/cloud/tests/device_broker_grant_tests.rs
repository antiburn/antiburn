use super::*;
use crate::cloud::{
    auth::{
        AccountSession, CAPTURE_AND_SESSION_SCOPE, CAPTURE_SCOPE,
        CAPTURE_SESSION_AND_TEAM_JOIN_SCOPE, Config, EligibleWorkspace, TokenRequest, Tokens,
    },
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
#[cfg(debug_assertions)]
fn returns_only_the_derived_read_token_for_a_read_enabled_device() {
    use crate::cloud::http_tests::server;
    for scope in [
        CAPTURE_AND_SESSION_SCOPE,
        CAPTURE_SESSION_AND_TEAM_JOIN_SCOPE,
    ] {
        let read_token = "x".repeat(180);
        let response = serde_json::json!({ "access_token": read_token }).to_string();
        let (config, worker) = server(200, &response);
        let directory = tempfile::tempdir().unwrap();
        let mut engine = engine(directory.path(), scope);
        engine.config = Some(config);
        let (origin, bearer, personal) = engine.cli_device_session(&|| true).unwrap();
        assert_eq!(origin, "https://api.example");
        assert_eq!(bearer, read_token);
        assert_eq!(personal, "synthetic-personal");
        let request = worker.join().unwrap();
        assert_eq!(
            request.line,
            "POST /v1/devices/grants/current/read-token HTTP/1.1"
        );
        assert_eq!(
            request.header("authorization"),
            Some("Bearer synthetic-access")
        );
        assert_eq!(request.header("cookie"), None);
        assert_eq!(request.header("origin"), None);
        assert!(request.body.is_empty());
    }
}

#[test]
#[cfg(debug_assertions)]
fn never_falls_back_to_device_proof_when_read_token_is_unavailable() {
    use crate::cloud::http_tests::server;
    for (status, reply, expected) in [
        (404, "{}", Failure::Unavailable),
        (401, "{}", Failure::ReauthRequired),
        (
            200,
            r#"{"access_token":"synthetic-access"}"#,
            Failure::InvalidResponse,
        ),
        (200, r#"{"access_token":""}"#, Failure::InvalidResponse),
    ] {
        let (config, worker) = server(status, reply);
        let directory = tempfile::tempdir().unwrap();
        let mut engine = engine(directory.path(), CAPTURE_SESSION_AND_TEAM_JOIN_SCOPE);
        engine.config = Some(config);
        assert!(matches!(
            engine.cli_device_session(&|| true),
            Err(error) if error == expected
        ));
        let request = worker.join().unwrap();
        assert_eq!(
            request.line,
            "POST /v1/devices/grants/current/read-token HTTP/1.1"
        );
        assert_eq!(
            request.header("authorization"),
            Some("Bearer synthetic-access")
        );
    }
}

#[test]
fn rejects_capture_only_grants_and_cancelled_reads() {
    for (scope, current, expected) in [
        (CAPTURE_SCOPE, true, Failure::Unavailable),
        (CAPTURE_AND_SESSION_SCOPE, false, Failure::Cancelled),
        (
            CAPTURE_SESSION_AND_TEAM_JOIN_SCOPE,
            false,
            Failure::Cancelled,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut engine = engine(directory.path(), scope);
        assert!(matches!(
            engine.cli_device_session(&|| current),
            Err(error) if error == expected
        ));
    }
}

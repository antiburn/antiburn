//! Checked transactions for account-bound requests and stable local root scopes.
//! Request IDs are idempotency keys. Cancellation keeps the record and does not
//! delete spool bytes or remote data.

use super::Store;
use antiburn_cloud_capture::CaptureIdentity;
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub(crate) const CONSENT_VERSION: &str = "native-full-content-v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RootScope {
    pub path: String,
    pub source_scope_id: String,
}

/// Frozen selection identity, not pinned source bytes or future capture authority.
/// A saved path cannot replace the library's admitted source handle during capture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SelectedSource {
    pub identity: CaptureIdentity,
    pub transcript_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CaptureRequest {
    pub request_id: String,
    pub account_id: String,
    pub destination_id: String,
    pub consent_version: String,
    pub roots: Vec<RootScope>,
    pub sources: Vec<SelectedSource>,
    pub discovery_complete: bool,
    pub cancelled: bool,
}

fn read_in(connection: &Connection, id: &str) -> Result<Option<CaptureRequest>> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT record_json FROM cloud_capture_request WHERE request_id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
        .transpose()
}

fn encoded(request: &CaptureRequest) -> Result<String> {
    let raw = serde_json::to_string(request)?;
    ensure!(
        raw.len() <= 1_048_576,
        "Cloud request exceeds its size limit"
    );
    Ok(raw)
}

impl Store {
    pub(crate) fn prepare_cloud_capture(
        &self,
        request_id: &str,
        account: &str,
        destination: &str,
        roots: &[String],
        now: i64,
    ) -> Result<CaptureRequest> {
        ensure!(
            !roots.is_empty() && roots.len() <= 100,
            "Choose bounded Cloud sources"
        );
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        if let Some(saved) = read_in(&transaction, request_id)? {
            ensure!(
                saved.account_id == account
                    && saved.destination_id == destination
                    && saved.roots.iter().map(|root| &root.path).eq(roots),
                "Cloud request is already bound to another selection"
            );
            return Ok(saved);
        }
        let count: u64 =
            transaction.query_row("SELECT count(*) FROM cloud_capture_request", [], |row| {
                row.get(0)
            })?;
        ensure!(count < 128, "Cloud request storage is full");
        let mut scopes = Vec::new();
        for path in roots {
            let saved: Option<String> = transaction.query_row(
                "SELECT source_scope_id FROM cloud_source_scope WHERE account_id = ?1 AND root_path = ?2",
                params![account, path], |row| row.get(0),
            ).optional()?;
            let source_scope_id = match saved {
                Some(saved) => saved,
                None => {
                    let count: u64 = transaction.query_row(
                        "SELECT count(*) FROM cloud_source_scope",
                        [],
                        |row| row.get(0),
                    )?;
                    ensure!(count < 1024, "Cloud source storage is full");
                    let mut bytes = [0; 32];
                    getrandom::fill(&mut bytes)
                        .map_err(|_| anyhow::anyhow!("Cloud source key is unavailable"))?;
                    let id = URL_SAFE_NO_PAD.encode(bytes);
                    transaction.execute(
                        "INSERT INTO cloud_source_scope(account_id, root_path, source_scope_id) VALUES (?1, ?2, ?3)",
                        params![account, path, id],
                    )?;
                    id
                }
            };
            scopes.push(RootScope {
                path: path.clone(),
                source_scope_id,
            });
        }
        let request = CaptureRequest {
            request_id: request_id.into(),
            account_id: account.into(),
            destination_id: destination.into(),
            consent_version: CONSENT_VERSION.into(),
            roots: scopes,
            sources: Vec::new(),
            discovery_complete: false,
            cancelled: false,
        };
        transaction.execute(
            "INSERT INTO cloud_capture_request(request_id, account_id, destination_id, created_at, record_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![request_id, account, destination, now, encoded(&request)?],
        )?;
        transaction.commit()?;
        Ok(request)
    }

    pub(crate) fn cloud_capture_request(
        &self,
        id: &str,
        account: &str,
        destination: &str,
    ) -> Result<CaptureRequest> {
        let request = read_in(&self.lock(), id)?
            .ok_or_else(|| anyhow::anyhow!("Cloud request is unavailable"))?;
        ensure!(
            request.account_id == account && request.destination_id == destination,
            "Cloud request is unavailable"
        );
        Ok(request)
    }

    pub(crate) fn current_cloud_capture_request(
        &self,
        account: &str,
        destination: &str,
        roots: &[String],
    ) -> Result<Option<CaptureRequest>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT record_json FROM cloud_capture_request WHERE account_id = ?1 AND destination_id = ?2
             ORDER BY created_at DESC, request_id DESC LIMIT 128",
        )?;
        for row in
            statement.query_map(params![account, destination], |row| row.get::<_, String>(0))?
        {
            let request: CaptureRequest = serde_json::from_str(&row?)?;
            if request.roots.iter().map(|root| &root.path).eq(roots) {
                return Ok(Some(request));
            }
        }
        Ok(None)
    }

    pub(crate) fn finish_cloud_discovery(
        &self,
        request: &CaptureRequest,
        sources: Vec<SelectedSource>,
    ) -> Result<CaptureRequest> {
        ensure!(
            sources.len() <= 512,
            "Cloud source selection exceeds its limit"
        );
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut saved = read_in(&transaction, &request.request_id)?
            .ok_or_else(|| anyhow::anyhow!("Cloud request is unavailable"))?;
        ensure!(
            saved.account_id == request.account_id
                && saved.destination_id == request.destination_id
                && saved.roots == request.roots,
            "Cloud request changed"
        );
        if saved.discovery_complete || saved.cancelled {
            return Ok(saved);
        }
        saved.sources = sources;
        saved.discovery_complete = true;
        transaction.execute(
            "UPDATE cloud_capture_request SET record_json = ?2 WHERE request_id = ?1",
            params![saved.request_id, encoded(&saved)?],
        )?;
        transaction.commit()?;
        Ok(saved)
    }

    pub(crate) fn cancel_cloud_capture_request(
        &self,
        request: &CaptureRequest,
    ) -> Result<CaptureRequest> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut saved = read_in(&transaction, &request.request_id)?
            .ok_or_else(|| anyhow::anyhow!("Cloud request is unavailable"))?;
        ensure!(
            saved.account_id == request.account_id
                && saved.destination_id == request.destination_id,
            "Cloud request is unavailable"
        );
        saved.cancelled = true;
        transaction.execute(
            "UPDATE cloud_capture_request SET record_json = ?2 WHERE request_id = ?1",
            params![saved.request_id, encoded(&saved)?],
        )?;
        transaction.commit()?;
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_request_ids_and_account_root_scopes_survive_restart_and_grant_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        let roots = vec!["/Example/Native".to_owned()];
        let first = store
            .prepare_cloud_capture("request-one", "account-one", "personal-one", &roots, 1)
            .unwrap();
        let next = store
            .prepare_cloud_capture("request-two", "account-one", "personal-one", &roots, 2)
            .unwrap();
        assert_eq!(first.roots, next.roots);
        let another = store
            .prepare_cloud_capture("request-three", "account-two", "personal-two", &roots, 3)
            .unwrap();
        assert_ne!(first.roots, another.roots);
        assert!(
            store
                .prepare_cloud_capture("request-one", "account-two", "personal-two", &roots, 4)
                .is_err()
        );
        assert!(
            store
                .prepare_cloud_capture(
                    "request-one",
                    "account-one",
                    "another-destination",
                    &roots,
                    4
                )
                .is_err()
        );
        assert!(
            store
                .prepare_cloud_capture(
                    "request-one",
                    "account-one",
                    "personal-one",
                    &["/Example/Other".into()],
                    4
                )
                .is_err()
        );
        store.cancel_cloud_capture_request(&first).unwrap();
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        let repeated = store
            .prepare_cloud_capture("request-one", "account-one", "personal-one", &roots, 10)
            .unwrap();
        assert!(repeated.cancelled);
        assert_eq!(repeated.roots, first.roots);
        assert!(
            store
                .cloud_capture_request("request-one", "account-two", "personal-two")
                .is_err()
        );
    }

    #[test]
    fn cloud_request_insert_failure_rolls_back_new_scope_keys() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        store
            .lock()
            .execute_batch(
                "CREATE TRIGGER reject_cloud_request BEFORE INSERT ON cloud_capture_request
             BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
            )
            .unwrap();
        assert!(
            store
                .prepare_cloud_capture(
                    "request",
                    "account",
                    "personal",
                    &["/Example/Native".into()],
                    1
                )
                .is_err()
        );
        let scopes: u64 = store
            .lock()
            .query_row("SELECT count(*) FROM cloud_source_scope", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(scopes, 0);
    }
}

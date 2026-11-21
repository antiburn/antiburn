//! Store folder drafts by internal account and personal destination. Drafts are
//! preferences, not content consent. Failed reads and writes stay visible.

use super::Store;
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};

impl Store {
    /// Create feature-owned tables in one checked transaction on admitted blocking work.
    /// Call before Cloud table access; failure can retry without marking setup ready.
    pub(crate) fn initialize_cloud_setup(&self) -> Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS cloud_setup (
                account_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                draft_json TEXT NOT NULL CHECK(length(draft_json) <= 524288),
                PRIMARY KEY (account_id, workspace_id)
            );
            CREATE TABLE IF NOT EXISTS cloud_source_scope (
                account_id TEXT NOT NULL,
                root_path TEXT NOT NULL,
                source_scope_id TEXT NOT NULL UNIQUE,
                PRIMARY KEY (account_id, root_path)
            );
            CREATE TABLE IF NOT EXISTS cloud_capture_request (
                request_id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                destination_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                record_json TEXT NOT NULL CHECK(length(record_json) <= 1048576)
            );
            CREATE INDEX IF NOT EXISTS cloud_capture_request_owner
                ON cloud_capture_request(account_id, destination_id, created_at);
            CREATE TABLE IF NOT EXISTS cloud_sync_policy (
                account_id TEXT NOT NULL,
                destination_id TEXT NOT NULL,
                record_json TEXT NOT NULL CHECK(length(record_json) <= 524288),
                PRIMARY KEY(account_id, destination_id)
            );
            CREATE TABLE IF NOT EXISTS cloud_sync_scope (
                account_id TEXT NOT NULL,
                destination_id TEXT NOT NULL,
                source_kind TEXT NOT NULL,
                root_path TEXT NOT NULL,
                source_scope_id TEXT NOT NULL UNIQUE,
                PRIMARY KEY(account_id, destination_id, source_kind, root_path)
            );
            CREATE TABLE IF NOT EXISTS cloud_sync_service_scope (
                account_id TEXT NOT NULL,
                destination_id TEXT NOT NULL,
                resource_origin TEXT NOT NULL,
                source_kind TEXT NOT NULL,
                root_path TEXT NOT NULL,
                source_scope_id TEXT NOT NULL UNIQUE,
                PRIMARY KEY(account_id, destination_id, resource_origin, source_kind, root_path)
            );
            CREATE TABLE IF NOT EXISTS cloud_sync_job (
                capture_id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                destination_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                record_json TEXT NOT NULL CHECK(length(record_json) <= 16384)
            );
            CREATE INDEX IF NOT EXISTS cloud_sync_job_owner
                ON cloud_sync_job(account_id, destination_id, source_id);
            CREATE INDEX IF NOT EXISTS cloud_sync_job_active
                ON cloud_sync_job(account_id, destination_id)
                WHERE json_extract(record_json, '$.state') NOT IN ('accepted', 'suppressed', 'abandoned');
            CREATE TABLE IF NOT EXISTS cloud_sync_source (
                account_id TEXT NOT NULL,
                destination_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                record_json TEXT NOT NULL CHECK(length(record_json) <= 16384),
                PRIMARY KEY(account_id, destination_id, source_id)
            );",
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Read only the requested account/destination preferences. Missing data is
    /// not permission or consent; database failures stay distinct from absence.
    pub(crate) fn cloud_draft(&self, account: &str, workspace: &str) -> Result<Option<String>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT draft_json FROM cloud_setup WHERE account_id = ?1 AND workspace_id = ?2",
                params![account, workspace],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Atomically replace bounded preferences without touching requests or source scopes.
    /// The caller must validate current authority and run outside UI/runtime threads.
    pub(crate) fn save_cloud_draft(
        &self,
        account: &str,
        workspace: &str,
        draft: &str,
    ) -> Result<()> {
        ensure!(
            !account.is_empty() && !workspace.is_empty(),
            "Cloud account is required"
        );
        ensure!(draft.len() <= 524_288, "Cloud setup exceeds its size limit");
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO cloud_setup (account_id, workspace_id, draft_json) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, workspace_id) DO UPDATE SET draft_json = excluded.draft_json",
            params![account, workspace, draft],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_setup_is_account_and_destination_bound_and_survives_restart() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        store
            .save_cloud_draft("account-a", "personal-a", "saved")
            .unwrap();
        assert_eq!(store.cloud_draft("account-b", "personal-a").unwrap(), None);
        assert_eq!(store.cloud_draft("account-a", "personal-b").unwrap(), None);
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        assert_eq!(
            store
                .cloud_draft("account-a", "personal-a")
                .unwrap()
                .as_deref(),
            Some("saved")
        );
    }

    #[test]
    fn cloud_setup_reports_failed_reads_and_writes_without_replacing_saved_choices() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        assert!(store.cloud_draft("account", "personal").is_err());
        store.initialize_cloud_setup().unwrap();
        store
            .save_cloud_draft("account", "personal", "saved")
            .unwrap();
        store
            .lock()
            .execute_batch(
                "CREATE TRIGGER reject_cloud_setup BEFORE UPDATE ON cloud_setup
             BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END;",
            )
            .unwrap();
        assert!(
            store
                .save_cloud_draft("account", "personal", "replacement")
                .is_err()
        );
        assert_eq!(
            store.cloud_draft("account", "personal").unwrap().as_deref(),
            Some("saved")
        );
    }
}

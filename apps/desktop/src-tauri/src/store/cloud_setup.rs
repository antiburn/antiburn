//! Store folder drafts by internal account and personal destination. Drafts are
//! preferences, not content consent. Failed reads and writes stay visible.

use super::Store;
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};

impl Store {
    /// Upgrade feature-owned tables in one transaction on admitted blocking work.
    /// Retired request tables and their rows stay intact but do not grant sync.
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
            CREATE TABLE IF NOT EXISTS cloud_sync_policy (
                account_id TEXT NOT NULL,
                destination_id TEXT NOT NULL,
                record_json TEXT NOT NULL CHECK(length(record_json) <= 524288),
                PRIMARY KEY(account_id, destination_id)
            );
            CREATE TABLE IF NOT EXISTS cloud_device_upload (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                enabled INTEGER NOT NULL CHECK(enabled IN (0, 1))
            );
            INSERT INTO cloud_device_upload (id, enabled)
                SELECT 1, NOT EXISTS(
                    SELECT 1 FROM cloud_sync_policy
                    WHERE CASE WHEN json_valid(record_json)
                        THEN COALESCE(json_extract(record_json, '$.paused'), 1)
                        ELSE 1 END
                )
                WHERE EXISTS (SELECT 1 FROM cloud_sync_policy)
                ON CONFLICT(id) DO NOTHING;
             CREATE TABLE IF NOT EXISTS cloud_workspace_slot (
                 issuer TEXT NOT NULL,
                 resource TEXT NOT NULL,
                 account_id TEXT NOT NULL,
                 workspace_id TEXT NOT NULL,
                 anchor_grant_id TEXT,
                 PRIMARY KEY (issuer, resource, account_id, workspace_id)
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
             );
             CREATE INDEX IF NOT EXISTS cloud_session_cwd
                 ON session(environment_key, cwd, agent)
                 WHERE cwd IS NOT NULL AND cwd <> '' AND length(CAST(cwd AS BLOB)) <= 4096;
             CREATE INDEX IF NOT EXISTS cloud_opencode_cwd
                 ON session(environment_key, agent, cwd, session_id)
                 WHERE environment_key = 'native' AND wsl_distro IS NULL
                   AND agent = 'opencode' AND source_kind = 'providerDb';
             CREATE INDEX IF NOT EXISTS cloud_claude_source
                 ON session(environment_key, agent, source_label)
                 WHERE environment_key = 'native' AND wsl_distro IS NULL
                   AND agent = 'claude-code' AND source_kind = 'file';",
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Add a synthetic legacy slot for cleanup tests. New connections never create slots.
    #[cfg(test)]
    pub(crate) fn register_cloud_workspace_slot(
        &self,
        issuer: &str,
        resource: &str,
        account: &str,
        workspace: &str,
    ) -> Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let existing: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM cloud_workspace_slot
             WHERE issuer=?1 AND resource=?2 AND account_id=?3 AND workspace_id=?4)",
            params![issuer, resource, account, workspace],
            |row| row.get(0),
        )?;
        if !existing {
            let count: usize = transaction.query_row(
                "SELECT count(*) FROM cloud_workspace_slot WHERE issuer=?1 AND resource=?2",
                params![issuer, resource],
                |row| row.get(0),
            )?;
            ensure!(count < 16, "Too many saved workspace connections");
            transaction.execute(
                "INSERT INTO cloud_workspace_slot VALUES (?1, ?2, ?3, ?4, NULL)",
                params![issuer, resource, account, workspace],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Return the exact slots for this installation and service, including slots
    /// that still need cleanup when no account or membership list can be read.
    pub(crate) fn cloud_workspace_slots(
        &self,
        issuer: &str,
        resource: &str,
    ) -> Result<Vec<(String, String)>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT account_id, workspace_id FROM cloud_workspace_slot
             WHERE issuer=?1 AND resource=?2 ORDER BY account_id, workspace_id LIMIT 17",
        )?;
        let slots = statement
            .query_map(params![issuer, resource], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ensure!(
            slots.len() <= 16
                && slots.iter().all(|(account, workspace)| {
                    !account.is_empty()
                        && account.len() <= 256
                        && !workspace.is_empty()
                        && workspace.len() <= 256
                }),
            "Invalid saved workspace connections"
        );
        Ok(slots)
    }

    /// Remove only a slot whose explicit close has already succeeded.
    pub(crate) fn remove_cloud_workspace_slot(
        &self,
        issuer: &str,
        resource: &str,
        account: &str,
        workspace: &str,
    ) -> Result<()> {
        self.lock().execute(
            "DELETE FROM cloud_workspace_slot WHERE issuer=?1 AND resource=?2 AND account_id=?3 AND workspace_id=?4",
            params![issuer, resource, account, workspace],
        )?;
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
#[path = "tests/cloud_setup_upgrade_integration_tests.rs"]
mod upgrade_tests;

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

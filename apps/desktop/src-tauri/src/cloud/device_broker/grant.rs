//! Check the current account grant before the local broker can use its bearer.
//! This module is private to the engine so no caller can bypass these checks.

use super::{Engine, personal_anchor};
use crate::cloud::auth::{CAPTURE_AND_SESSION_SCOPE, Failure, WorkspaceKind};

/// Return only the four fields that the private broker can send to the CLI.
type DeviceGrant = (String, String, String, Vec<String>);

impl Engine {
    /// Restore through the existing rotation path and fetch the current workspace
    /// list. Call only in admitted Cloud work with the account generation fence.
    /// Existing capture-only grants do not authorize this handoff.
    pub(in crate::cloud) fn cli_device_session(
        &mut self,
        current: &dyn Fn() -> bool,
    ) -> Result<DeviceGrant, Failure> {
        if !current() {
            return Err(Failure::Cancelled);
        }
        if let Err(error) = self.refresh((self.clock)(), current) {
            self.status = error.into();
            return Err(error);
        }
        let config = self.config.as_ref().ok_or(Failure::Unavailable)?;
        let access = self.access.as_ref().ok_or(Failure::ReauthRequired)?;
        if !current() {
            return Err(Failure::Cancelled);
        }
        let session = self.session().ok_or(Failure::ReauthRequired)?;
        if session.scope != CAPTURE_AND_SESSION_SCOPE
            || session.workspace.kind != WorkspaceKind::Personal
        {
            return Err(Failure::Unavailable);
        }
        let eligible = self.transport.workspaces(config, &access.token)?;
        if !current() || self.session().is_none() {
            return Err(Failure::Cancelled);
        }
        let personal = personal_anchor(&eligible)?;
        if personal.id != session.workspace.id {
            return Err(Failure::InvalidResponse);
        }
        Ok((
            config.resource.clone(),
            access.token.clone(),
            personal.id.clone(),
            eligible
                .into_iter()
                .map(|entry| entry.workspace.id)
                .collect(),
        ))
    }
}

#[cfg(test)]
#[path = "../tests/device_broker_grant_tests.rs"]
mod tests;

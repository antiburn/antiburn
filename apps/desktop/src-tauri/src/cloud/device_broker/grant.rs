//! Check the current account grant before the local broker can use its bearer.
//! This module is private to the engine so no caller can bypass these checks.

use super::Engine;
use crate::cloud::auth::{CAPTURE_AND_SESSION_SCOPE, Failure, WorkspaceKind};

/// Return only the three fields that the private broker can send to the CLI.
type DeviceGrant = (String, String, String);

impl Engine {
    /// Restore through the existing rotation path and use the verified Personal
    /// workspace. Call only in admitted Cloud work with the account fence.
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
        Ok((
            config.resource.clone(),
            access.token.clone(),
            session.workspace.id.clone(),
        ))
    }
}

#[cfg(test)]
#[path = "../tests/device_broker_grant_tests.rs"]
mod tests;

//! Check the current account grant and fetch a read-only token for the CLI.
//! This module is private to the engine so no caller can bypass these checks.

use super::Engine;
use crate::cloud::AuthStatus;
use crate::cloud::auth::{self, Config, DeviceAction, Failure, WorkspaceKind};
use reqwest::StatusCode;
use serde::Deserialize;

/// Return only the three fields that the private broker can send to the CLI.
type DeviceGrant = (String, String, String);

impl Engine {
    /// Restore through the existing rotation path and use the verified Personal
    /// workspace. Call only in admitted Cloud work with the account fence.
    /// Send only the derived read token. Do not cache it or return full proof
    /// if the service denies the request or does not support this route.
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
        if !(session.allows(DeviceAction::SessionList) || session.allows(DeviceAction::SessionRead))
            || session.workspace.kind != WorkspaceKind::Personal
        {
            return Err(Failure::Unavailable);
        }
        let result = read_token(config, &access.token);
        if !current() {
            return Err(Failure::Cancelled);
        }
        if matches!(result, Err(Failure::ReauthRequired)) {
            self.access = None;
            self.status = AuthStatus::ReauthRequired;
            return Err(Failure::ReauthRequired);
        }
        if self.session().is_none() {
            return Err(Failure::Cancelled);
        }
        Ok((
            config.resource.clone(),
            result?,
            session.workspace.id.clone(),
        ))
    }
}

/// Keep the device bearer on the resource origin and reject unusable replies.
fn read_token(config: &Config, access: &str) -> Result<String, Failure> {
    let url = config
        .session
        .join("/v1/devices/grants/current/read-token")
        .map_err(|_| Failure::Unavailable)?;
    let response = auth::client(&url)?
        .post(url)
        .bearer_auth(access)
        .send()
        .map_err(|_| Failure::Offline)?;
    match response.status() {
        StatusCode::OK => {}
        StatusCode::UNAUTHORIZED => return Err(Failure::ReauthRequired),
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => return Err(Failure::Unavailable),
        _ => return Err(Failure::Offline),
    }
    #[derive(Deserialize)]
    struct ReadToken {
        access_token: String,
    }
    let ReadToken { access_token } = auth::decode_json(response)?;
    if access_token == access
        || access_token.is_empty()
        || access_token.len() > 16_384
        || !access_token.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(Failure::InvalidResponse);
    }
    Ok(access_token)
}

#[cfg(test)]
#[path = "../tests/device_broker_grant_tests.rs"]
mod tests;

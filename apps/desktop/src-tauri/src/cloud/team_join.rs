//! Device-only Team join requests. Pasted links never become request URLs.

use super::auth::{self, Config, Failure};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub(super) const ATTEMPTS_PATH: &str = "/v1/devices/team-join-attempts";

/// Renderer input for exactly one step. No variant supplies an origin or proof
/// for the signed-in account.
#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum Action {
    Start {
        link: String,
    },
    Resume {
        attempt_id: String,
    },
    SelectEmail {
        attempt_id: String,
        address: String,
        #[serde(default)]
        resend: bool,
    },
    Claim {
        attempt_id: String,
        email_id: String,
    },
}

/// Decode the API status and expose only the fields the join view needs.
/// The renderer keeps attempt handles in memory and cannot treat them as proof.
#[derive(Deserialize, Serialize)]
#[serde(
    tag = "status",
    rename_all(serialize = "camelCase", deserialize = "snake_case"),
    rename_all_fields(serialize = "camelCase")
)]
pub(crate) enum Step {
    Started {
        attempt_id: String,
    },
    NeedsEmail {
        #[serde(skip_deserializing)]
        attempt_id: String,
        team_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        email: Option<String>,
    },
    VerificationPending {
        #[serde(skip_deserializing)]
        attempt_id: String,
        team_name: String,
        email: String,
    },
    Ready {
        #[serde(skip_deserializing)]
        attempt_id: String,
        team_name: String,
        email: String,
        email_id: String,
    },
    Joined {
        team_name: String,
        in_setup: bool,
    },
    Removed,
    BillingInactive,
    BillingStale,
    Full,
}

/// Closed failure codes. Do not send server messages or raw input through IPC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JoinError {
    Cancelled,
    ReauthRequired,
    Unavailable,
    InvalidInvite,
    InactiveInvite,
    InvalidEmail,
    ReapprovalRequired,
    RateLimited,
    Unconfirmed,
}

impl JoinError {
    pub(super) fn ipc_code(self) -> String {
        match self {
            Self::Cancelled => "team_join_account_changed",
            Self::ReauthRequired => "team_join_sign_in_required",
            Self::Unavailable => "team_join_unavailable",
            Self::InvalidInvite => "team_join_invalid_invite",
            Self::InactiveInvite => "team_join_inactive_invite",
            Self::InvalidEmail => "team_join_invalid_email",
            Self::ReapprovalRequired => "team_join_reapproval_required",
            Self::RateLimited => "team_join_rate_limited",
            Self::Unconfirmed => "team_join_unconfirmed",
        }
        .into()
    }
}

impl From<Failure> for JoinError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Cancelled => Self::Cancelled,
            Failure::ReauthRequired | Failure::CredentialFailed => Self::ReauthRequired,
            Failure::Rejected | Failure::Unavailable => Self::Unavailable,
            _ => Self::Unconfirmed,
        }
    }
}

/// Send one request with current device proof. The caller checks account
/// authority before dispatch and before it uses the reply.
pub(super) fn send(config: &Config, access: &str, action: Action) -> Result<Step, JoinError> {
    let mut url = config.team_join_attempts.clone();
    let (method, body, attempt_id, suffix) = match &action {
        Action::Start { link } => {
            let token = invite_token(config, link).ok_or(JoinError::InvalidInvite)?;
            (Method::POST, Some(json!({ "token": token })), None, "")
        }
        Action::Resume { attempt_id } => (Method::POST, None, Some(attempt_id), "resume"),
        Action::SelectEmail {
            attempt_id,
            address,
            resend,
        } => {
            let address = address.trim();
            if address.is_empty() || address.len() > 254 || address.chars().any(char::is_control) {
                return Err(JoinError::InvalidEmail);
            }
            let body = if *resend {
                json!({ "address": address, "resend": true })
            } else {
                json!({ "address": address })
            };
            (Method::PUT, Some(body), Some(attempt_id), "email")
        }
        Action::Claim {
            attempt_id,
            email_id,
        } => {
            if !valid_id(email_id) {
                return Err(JoinError::InvalidEmail);
            }
            (
                Method::POST,
                Some(json!({ "email_id": email_id })),
                Some(attempt_id),
                "claim",
            )
        }
    };
    if let Some(id) = attempt_id {
        if !valid_id(id) {
            return Err(JoinError::InactiveInvite);
        }
        url.path_segments_mut()
            .map_err(|()| JoinError::Unavailable)?
            .push(id)
            .push(suffix);
    }
    let mut request = auth::client(&url)?.request(method, url).bearer_auth(access);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().map_err(|_| JoinError::Unconfirmed)?;
    match response.status() {
        StatusCode::OK => decode_step(response, attempt_id),
        StatusCode::UNAUTHORIZED => Err(JoinError::ReauthRequired),
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => Err(JoinError::Unavailable),
        StatusCode::TOO_MANY_REQUESTS => Err(JoinError::RateLimited),
        StatusCode::BAD_REQUEST | StatusCode::CONFLICT => {
            #[derive(Deserialize)]
            struct ErrorBody {
                code: String,
            }
            match auth::decode_json::<ErrorBody>(response)?.code.as_str() {
                "invite_link_inactive" => Err(JoinError::InactiveInvite),
                "invalid_email_address" => Err(JoinError::InvalidEmail),
                _ => Err(JoinError::Unconfirmed),
            }
        }
        _ => Err(JoinError::Unconfirmed),
    }
}

fn decode_step(
    response: reqwest::blocking::Response,
    attempt_id: Option<&String>,
) -> Result<Step, JoinError> {
    let Some(attempt_id) = attempt_id else {
        #[derive(Deserialize)]
        struct Started {
            attempt_id: String,
        }
        let Started { attempt_id } = auth::decode_json(response)?;
        if !valid_id(&attempt_id) {
            return Err(JoinError::Unconfirmed);
        }
        return Ok(Step::Started { attempt_id });
    };
    let mut step: Step = auth::decode_json(response)?;
    match &mut step {
        Step::Ready { email_id, .. } if !valid_id(email_id) => return Err(JoinError::Unconfirmed),
        Step::NeedsEmail { attempt_id: id, .. }
        | Step::VerificationPending { attempt_id: id, .. }
        | Step::Ready { attempt_id: id, .. } => id.clone_from(attempt_id),
        Step::Started { .. } => return Err(JoinError::Unconfirmed),
        _ => {}
    }
    Ok(step)
}

fn valid_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
            }
        })
}

/// Accept copied and emailed links at the exact configured dashboard.
/// Return only the Team invite token. The recipient proof belongs in the browser.
fn invite_token<'a>(config: &Config, input: &'a str) -> Option<&'a str> {
    if input.len() > 4096 {
        return None;
    }
    let expected = config.dashboard.as_ref()?.join("/join").ok()?;
    let (address, fragment) = input.trim().split_once("#token=")?;
    if address != expected.as_str() {
        return None;
    }
    let token = match fragment.split_once("&proof=") {
        Some((token, proof)) if valid_invite_secret(proof) => token,
        Some(_) => return None,
        None => fragment,
    };
    valid_invite_secret(token).then_some(token)
}

fn valid_invite_secret(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_links_require_the_exact_dashboard_path_and_token() {
        let mut config = Config::parse("https://issuer.example", "https://api.example").unwrap();
        let token = "a".repeat(43);
        let invite = format!("https://dashboard.example/join#token={token}");
        assert_eq!(invite_token(&config, &invite), None);
        config.dashboard = Some("https://dashboard.example".parse().unwrap());
        assert_eq!(
            invite_token(&config, &format!("  {invite}\n")),
            Some(token.as_str())
        );
        let proof = "_".repeat(43);
        assert_eq!(
            invite_token(&config, &format!("{invite}&proof={proof}")),
            Some(token.as_str())
        );
        for address in [
            "https://other.example/join",
            "https://api.example/join",
            "https://user@dashboard.example/join",
            "http://dashboard.example/join",
            "https://dashboard.example:443/join",
            "https://dashboard.example/emails/verify",
            "https://dashboard.example/join/",
            "https://dashboard.example/join?next=/",
        ] {
            assert_eq!(
                invite_token(&config, &format!("{address}#token={token}")),
                None
            );
        }
        for invalid in [
            "too-short".to_owned(),
            format!("{token}%2f"),
            format!("{token}&token=duplicate"),
            format!("{token}&proof=too-short"),
            format!("{token}&proof={proof}&proof={proof}"),
            format!("{token}&proof={proof}&other={token}"),
            format!("{token}&proof={proof}%2f"),
            "a".repeat(4096),
        ] {
            assert_eq!(
                invite_token(
                    &config,
                    &format!("https://dashboard.example/join#token={invalid}")
                ),
                None
            );
        }
    }
}

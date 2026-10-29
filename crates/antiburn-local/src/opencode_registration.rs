//! Decode the bounded registration file for a local OpenCode service.

use std::fmt;

use serde_json::Value;
use url::Url;

pub const MAX_REGISTRATION_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceRegistration {
    pub id: Option<String>,
    pub version: Option<String>,
    pub endpoint: LocalEndpoint,
    pub pid: u32,
}

#[derive(Clone, PartialEq, Eq)]
pub struct LocalEndpoint {
    base_url: Url,
    password: Option<String>,
}

impl fmt::Debug for LocalEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalEndpoint")
            .field("base_url", &self.base_url)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Reject names that can resolve outside this machine and URLs with credentials.
pub fn is_loopback_http_url(url: &Url) -> bool {
    let loopback = match url.host() {
        Some(url::Host::Ipv4(host)) => host.is_loopback(),
        Some(url::Host::Ipv6(host)) => host.is_loopback(),
        _ => false,
    };
    url.scheme() == "http"
        && loopback
        && url.port().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

impl LocalEndpoint {
    pub fn parse(url: &str, password: Option<String>) -> Result<Self, RegistrationError> {
        let base_url = Url::parse(url).map_err(|_| RegistrationError::InvalidEndpoint)?;
        if !is_loopback_http_url(&base_url) || base_url.query().is_some() {
            return Err(RegistrationError::InvalidEndpoint);
        }
        if password
            .as_ref()
            .is_some_and(|value| value.len() > 4096 || value.contains('\0'))
        {
            return Err(RegistrationError::InvalidField);
        }
        Ok(Self {
            base_url,
            password: password.filter(|value| !value.is_empty()),
        })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationError {
    Io,
    TooLarge,
    Malformed,
    InvalidField,
    InvalidEndpoint,
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Io => "service registration could not be read",
            Self::TooLarge => "service registration exceeds the size limit",
            Self::Malformed => "service registration is malformed",
            Self::InvalidField => "service registration has invalid fields",
            Self::InvalidEndpoint => "service endpoint is not a valid local endpoint",
        })
    }
}

impl std::error::Error for RegistrationError {}

pub fn decode_registration(bytes: &[u8]) -> Result<ServiceRegistration, RegistrationError> {
    if bytes.len() > MAX_REGISTRATION_BYTES {
        return Err(RegistrationError::TooLarge);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| RegistrationError::Malformed)?;
    let object = value.as_object().ok_or(RegistrationError::Malformed)?;
    let url = object
        .get("url")
        .and_then(Value::as_str)
        .ok_or(RegistrationError::InvalidField)?;
    let pid = object
        .get("pid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0 && *pid <= u32::MAX as u64)
        .ok_or(RegistrationError::InvalidField)? as u32;
    Ok(ServiceRegistration {
        id: optional_string(object.get("id"), 256)?,
        version: optional_string(object.get("version"), 128)?,
        endpoint: LocalEndpoint::parse(url, optional_string(object.get("password"), 4096)?)?,
        pid,
    })
}

fn optional_string(
    value: Option<&Value>,
    max_len: usize,
) -> Result<Option<String>, RegistrationError> {
    let Some(value) = value else { return Ok(None) };
    let text = value.as_str().ok_or(RegistrationError::InvalidField)?;
    if text.len() > max_len || text.contains('\0') {
        return Err(RegistrationError::InvalidField);
    }
    Ok(Some(text.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decoder_accepts_numeric_loopback_and_keeps_password_private() {
        for host in ["127.0.0.1", "127.10.20.30", "[::1]"] {
            let registration = decode_registration(
                &serde_json::to_vec(&json!({
                    "url": format!("http://{host}:3456"), "pid": 1, "password": "private-password"
                }))
                .unwrap(),
            )
            .unwrap();
            assert_eq!(registration.endpoint.password(), Some("private-password"));
            assert!(!format!("{registration:?}").contains("private-password"));
        }
    }

    #[test]
    fn decoder_rejects_unsafe_endpoints_and_invalid_fields() {
        for url in [
            "http://localhost:3456",
            "https://127.0.0.1:3456",
            "http://192.0.2.1:3456",
            "http://127.0.0.1",
            "http://u:p@127.0.0.1:3456",
            "http://127.0.0.1:3456/?q=1",
        ] {
            assert!(LocalEndpoint::parse(url, None).is_err(), "{url}");
        }
        for value in [
            json!({"url":"http://127.0.0.1:3456","pid":0}),
            json!({"url":"http://127.0.0.1:3456","pid":1,"password":null}),
            json!({"url":"http://127.0.0.1:3456","pid":1,"id":"bad\0id"}),
        ] {
            assert!(decode_registration(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        assert_eq!(
            decode_registration(&vec![b' '; MAX_REGISTRATION_BYTES + 1]),
            Err(RegistrationError::TooLarge)
        );
    }
}

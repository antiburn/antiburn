// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One HTTP client, and one error mapping, shared by every direct-fetch
//! source.
//!
//! A provider's endpoint gets asked the same way no matter which source is
//! asking: the same timeout, the same refusal to follow a redirect
//! (this application decides which host it is trusting a credential to, not
//! a `Location` header), and the same cap on how much of a response it will
//! ever hold in memory. Two provider integrations gave two easy ways for that
//! agreement to drift; one shared client is the one way it cannot.

use std::io::Read as _;
use std::time::Duration;

use crate::provider_usage::live::model::ProviderUsageError;

/// How long a single request may take before it counts as unreachable.
const TIMEOUT: Duration = Duration::from_secs(15);

/// A response body larger than this is refused rather than read — no
/// provider usage payload is anywhere near this large, so a response this
/// size is not one this module was written to trust.
const MAX_RESPONSE_BYTES: u64 = 512 * 1024;

/// The client every direct-fetch source shares.
///
/// Built once per process: `reqwest::blocking::Client` owns a connection pool
/// worth keeping warm, and every source here talks to at most a couple of
/// hosts. The `rustls-no-provider` feature this crate builds `reqwest`
/// against leaves the process-wide crypto provider unset, so the first call
/// here installs it — see the `rustls` dependency comment in `Cargo.toml`.
///
/// Blocking, and only ever to be called where blocking is allowed. A caller
/// holding a runtime has to hand the work to a blocking thread first, and
/// both callers do: `crate::usage_alerts::blocking::run` for the background
/// monitor, `crate::commands::get_live_usage` for the popover.
///
/// The cost of getting that wrong differs by profile, and both are worth
/// knowing. In release, a blocking call parks the calling thread until the
/// response or [`TIMEOUT`] — bad enough on a runtime worker, worse on the
/// main thread. In debug, `reqwest` additionally builds and drops a `tokio`
/// runtime on the calling thread purely to assert it is not inside one
/// (`reqwest::blocking::wait::enter`, `#[cfg(debug_assertions)]`), and
/// dropping a runtime inside an asynchronous context panics outright. So a
/// debug build fails loudly where a release build only stalls: the rule is
/// the same either way, and only the symptom of breaking it changes.
pub fn client() -> &'static reqwest::blocking::Client {
    static CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::blocking::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("a client with no custom TLS material always builds")
    })
}

/// Read a response body under the shared cap, or fail closed.
///
/// Checked twice: `Content-Length`, when the server sent one, rejects an
/// oversized body before a single byte is read; counting what is actually
/// read catches a server that lied about — or omitted — that header.
pub fn read_capped_body(
    response: reqwest::blocking::Response,
) -> Result<String, ProviderUsageError> {
    if response
        .content_length()
        .is_some_and(|len| len > MAX_RESPONSE_BYTES)
    {
        return Err(ProviderUsageError::Unavailable);
    }
    let mut limited = response.take(MAX_RESPONSE_BYTES + 1);
    let mut bytes = Vec::new();
    limited
        .read_to_end(&mut bytes)
        .map_err(|_| ProviderUsageError::Unavailable)?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(ProviderUsageError::Unavailable);
    }
    String::from_utf8(bytes).map_err(|_| ProviderUsageError::Unavailable)
}

/// Map an HTTP status onto this module's error taxonomy. `None` is success.
///
/// Deliberately coarse, matching [`ProviderUsageError`]'s own four
/// categories: a reader can act on "sign in again" or "try later", and
/// nothing downstream needs to know which of the many non-2xx statuses a
/// provider actually sent.
pub fn status_error(status: reqwest::StatusCode) -> Option<ProviderUsageError> {
    if status.is_success() {
        return None;
    }
    Some(match status.as_u16() {
        401 | 403 => ProviderUsageError::Authentication,
        429 => ProviderUsageError::RateLimited,
        _ => ProviderUsageError::Unavailable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_statuses_map_to_no_error() {
        assert_eq!(status_error(reqwest::StatusCode::OK), None);
        assert_eq!(status_error(reqwest::StatusCode::NO_CONTENT), None);
    }

    #[test]
    fn authentication_statuses_are_grouped_together() {
        assert_eq!(
            status_error(reqwest::StatusCode::UNAUTHORIZED),
            Some(ProviderUsageError::Authentication)
        );
        assert_eq!(
            status_error(reqwest::StatusCode::FORBIDDEN),
            Some(ProviderUsageError::Authentication)
        );
    }

    #[test]
    fn a_rate_limit_status_is_distinguished_from_every_other_failure() {
        assert_eq!(
            status_error(reqwest::StatusCode::TOO_MANY_REQUESTS),
            Some(ProviderUsageError::RateLimited)
        );
    }

    #[test]
    fn every_other_failure_status_is_unavailable() {
        assert_eq!(
            status_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            Some(ProviderUsageError::Unavailable)
        );
        assert_eq!(
            status_error(reqwest::StatusCode::NOT_FOUND),
            Some(ProviderUsageError::Unavailable)
        );
    }

    #[test]
    fn the_shared_client_builds_exactly_once() {
        // `OnceLock` guarantees the identity, not just the value; this is
        // mostly a smoke test that construction does not panic.
        assert!(std::ptr::eq(client(), client()));
    }
}

//! Where analytics events go — and, in every build of this repository as it
//! stands, the fact that they go nowhere.
//!
//! The endpoint and the operator's display name are injected at build time and
//! appear nowhere in this tree.
//!
//! It also buys a property worth more than the secrecy. A clone of this
//! repository, built by anyone, has no endpoint — so it transmits nothing at
//! all, and the Privacy pane can say so truthfully rather than describing a
//! capability the reader cannot verify. [`configured`] is what every surface
//! downstream asks, and it answers from the build it is actually running in.

/// The collector's base URL; request paths are appended to it. `None` in
/// every build that did not inject one.
///
/// Named to match the variable the first-party collector's own clients read,
/// so one release pipeline configures both without a translation step. The
/// word is `ANALYTICS` rather than `TELEMETRY` deliberately: the operator
/// reserves "telemetry" for a device snapshot antiburn does not send, and an
/// env var that says otherwise would mislead whoever wires up the build.
const ENDPOINT: Option<&str> = option_env!("ANTIBURN_ANALYTICS_URL");

/// Who receives the events, in the reader's own words, for the Privacy pane.
const OPERATOR: Option<&str> = option_env!("ANTIBURN_ANALYTICS_OPERATOR");

/// Whether this build can transmit at all.
///
/// Three things must all be true, and the `cfg!` is deliberately one of them:
/// `updates.rs` learned the hard way that a compile-time answer alone will
/// claim a capability a misconfigured build does not have, so this pairs the
/// build flag with the configuration it depends on.
pub fn configured() -> bool {
    cfg!(feature = "distribution") && endpoint().is_some()
}

/// The endpoint, if this build has a usable one.
///
/// Plain `http` is refused except on loopback, which exists so the manual
/// verification in the plan can point at a local listener without a
/// certificate. Anything else travelling unencrypted would make "anonymised"
/// depend on nobody being on the same network.
pub fn endpoint() -> Option<&'static str> {
    let raw = ENDPOINT?.trim();
    if raw.is_empty() {
        return None;
    }
    let origin = origin_of(raw);
    is_permitted(origin).then_some(origin)
}

/// Reduce a configured URL to its bare origin — scheme, host, optional port —
/// discarding any path, query, or trailing slash.
///
/// The collector's contract is a base that request paths are composed onto,
/// and its own client normalises the same way. Without this, injecting
/// `https://host/v1/` would post to `https://host/v1/v1/track` and every
/// event would 404 — a configuration mistake that produces no error the
/// reader or the operator would ever see, because delivery failures here are
/// silent by design.
///
/// Done by hand rather than with a URL crate: the input is a single
/// build-time constant and the whole rule is "cut at the first slash after
/// the authority".
fn origin_of(raw: &str) -> &str {
    let Some(scheme_end) = raw.find("://") else {
        return raw;
    };
    let authority_start = scheme_end + 3;
    match raw[authority_start..].find('/') {
        Some(offset) => &raw[..authority_start + offset],
        None => raw,
    }
}

/// Whether a configured URL may be used: TLS anywhere, plaintext on loopback
/// only.
///
/// Prefix-matching the host alone would accept `http://localhost.example.com`,
/// which is a plaintext endpoint on somebody else's machine wearing a loopback
/// name. The authority has to *end* at the host, so only a port or a path may
/// follow it.
fn is_permitted(raw: &str) -> bool {
    if raw.starts_with("https://") {
        return true;
    }
    ["http://127.0.0.1", "http://localhost"].iter().any(|host| {
        raw.strip_prefix(host)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(':') || rest.starts_with('/'))
    })
}

/// The operator's display name, for copy that names who receives the events.
pub fn operator() -> Option<&'static str> {
    let raw = OPERATOR?.trim();
    (!raw.is_empty()).then_some(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A base with a path composes onto, rather than replaces, the request
    /// path — so the configured value has to be cut back to its origin before
    /// `/v1/track` is appended.
    #[test]
    fn a_configured_base_is_reduced_to_its_origin() {
        assert_eq!(origin_of("https://example.com/v1/"), "https://example.com");
        assert_eq!(origin_of("https://example.com/"), "https://example.com");
        assert_eq!(origin_of("https://example.com"), "https://example.com");
        assert_eq!(
            origin_of("http://127.0.0.1:8787/v1"),
            "http://127.0.0.1:8787"
        );
        // No scheme separator at all: nothing to cut, and `is_permitted`
        // rejects it a moment later anyway.
        assert_eq!(origin_of("example.com/v1"), "example.com/v1");
    }

    /// The loopback exemption exists so a local listener can be used for the
    /// manual verification. It must not become a way to ship plaintext to a
    /// host that merely starts with a loopback name.
    #[test]
    fn the_loopback_exemption_stops_at_the_host() {
        for accepted in [
            "http://127.0.0.1",
            "http://127.0.0.1:8787",
            "http://localhost/v1",
            "https://example.com",
        ] {
            assert!(is_permitted(accepted), "{accepted}");
        }
        for rejected in [
            "http://localhost.example.com",
            "http://127.0.0.1.example.com",
            "http://example.com",
        ] {
            assert!(!is_permitted(rejected), "{rejected}");
        }
    }
}

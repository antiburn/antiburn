//! Match repository rules against the configured local fetch origin.
//! This parser does not read Git configuration or resolve URL rewrites.

use std::net::{Ipv4Addr, Ipv6Addr};

const MAX_FETCH_URL_BYTES: usize = 4096;
const EXACT_ENDPOINT_PREFIX: &str = "endpoint-v2:";

/// A credential-free label for one local fetch origin. Compare labels exactly.
/// A match does not verify repository ownership or access to a remote.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RepositoryOrigin(String);

impl RepositoryOrigin {
    /// Parse the NUL-delimited output of `git config --null --get-all
    /// remote.origin.url`. Pass only successful, complete output. Reject more
    /// than one entry, even if both entries would produce the same label.
    pub(super) fn from_fetch_urls(output: &[u8]) -> Option<Self> {
        let value = output.strip_suffix(b"\0")?;
        if value.len() > MAX_FETCH_URL_BYTES || !value.iter().all(|byte| byte.is_ascii_graphic()) {
            return None;
        }
        parse_fetch_url(std::str::from_utf8(value).ok()?, LabelFormat::Current)
    }

    /// Use this exact label for storage and matching. Only recognized hosted
    /// endpoints can share an HTTPS and SSH label. Other SSH labels retain the
    /// login and distinguish relative SCP paths from absolute SSH paths.
    pub(super) fn as_label(&self) -> &str {
        &self.0
    }
}

/// Accept current and legacy labels so saved rules remain readable and revocable.
/// A legacy generic label never matches fresh proof. Never use labels as URLs.
pub(crate) fn valid_repository_origin_label(label: &str) -> bool {
    if label.is_empty() || label.len() > MAX_FETCH_URL_BYTES {
        return false;
    }
    let (endpoint, suffix, format) = match label.strip_prefix(EXACT_ENDPOINT_PREFIX) {
        Some(endpoint) => (endpoint, "", LabelFormat::Current),
        None => (label, ".git", LabelFormat::Legacy),
    };
    fetch_url_for_label(endpoint, suffix)
        .and_then(|url| parse_fetch_url(&url, format))
        .is_some_and(|origin| origin.as_label() == label)
}

/// Map a current exact endpoint to the former label for an OFF choice only.
/// The result does not identify an approved repository. Never use it for
/// approval, grant lookup, or capture admission.
pub(crate) fn legacy_repository_origin_for_off_choice(label: &str) -> Option<String> {
    let endpoint = label.strip_prefix(EXACT_ENDPOINT_PREFIX)?;
    if label.len() > MAX_FETCH_URL_BYTES {
        return None;
    }
    let url = fetch_url_for_label(endpoint, "")?;
    if parse_fetch_url(&url, LabelFormat::Current)?.as_label() != label {
        return None;
    }
    parse_fetch_url(&url, LabelFormat::Legacy).map(|origin| origin.0)
}

/// Rebuild parser input from a stored label. Do not use this value for I/O.
fn fetch_url_for_label(endpoint: &str, suffix: &str) -> Option<String> {
    Some(if let Some(relative) = endpoint.strip_prefix("scp://") {
        let (authority, path) = relative.split_once('/')?;
        format!("{authority}:{path}{suffix}")
    } else if endpoint.starts_with("ssh://") || endpoint.starts_with("https://") {
        format!("{endpoint}{suffix}")
    } else {
        format!("https://{endpoint}{suffix}")
    })
}

enum LabelFormat {
    Current,
    Legacy,
}

#[derive(Clone, Copy)]
enum Endpoint<'a> {
    Https,
    Ssh { user: &'a str, relative: bool },
}

fn parse_fetch_url(value: &str, format: LabelFormat) -> Option<RepositoryOrigin> {
    let (host, endpoint, port, path) = if let Some((scheme, rest)) = value.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        let (user, authority) = match authority.split_once('@') {
            Some((user, host)) => (Some(user), host),
            None => (None, authority),
        };
        let endpoint = match scheme {
            "https" if user.is_none_or(valid_https_user_info) => Endpoint::Https,
            "ssh" => Endpoint::Ssh {
                user: user.filter(|user| valid_ssh_user(user))?,
                relative: false,
            },
            _ => return None,
        };
        let (host, port) = split_host(authority);
        let port = if port.is_empty() {
            None
        } else {
            let port = port.strip_prefix(':')?;
            if port.starts_with('0') || !port.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            Some(port.parse::<u16>().ok()?)
        };
        (host, endpoint, port, path)
    } else {
        let (user, authority) = value.split_once('@')?;
        if !valid_ssh_user(user) {
            return None;
        }
        let (host, path) = split_host(authority);
        (
            host,
            Endpoint::Ssh {
                user,
                relative: true,
            },
            None,
            path.strip_prefix(':')?,
        )
    };
    let host = canonical_host(host)?;
    let without_suffix = path.strip_suffix(".git").unwrap_or(path);
    if !valid_repository_path(without_suffix) {
        return None;
    }
    let default_port = match endpoint {
        Endpoint::Https => 443,
        Endpoint::Ssh { .. } => 22,
    };
    let standard_endpoint = port.is_none_or(|port| port == default_port)
        && matches!(
            endpoint,
            Endpoint::Https | Endpoint::Ssh { user: "git", .. }
        )
        && standard_hosted_path(&host, path);
    let exact_endpoint = !standard_endpoint && matches!(format, LabelFormat::Current);
    // An SSH client can choose a different port when the origin omits one.
    let port = port.filter(|&port| {
        port != default_port || (exact_endpoint && matches!(endpoint, Endpoint::Ssh { .. }))
    });
    let path = if exact_endpoint { path } else { without_suffix };
    let label = match (endpoint, port) {
        (Endpoint::Https, None) => format!("{host}/{path}"),
        (Endpoint::Https, Some(port)) => format!("https://{host}:{port}/{path}"),
        (Endpoint::Ssh { user: "git", .. }, None) if standard_endpoint => {
            format!("{host}/{path}")
        }
        (
            Endpoint::Ssh {
                user,
                relative: true,
            },
            None,
        ) => format!("scp://{user}@{host}/{path}"),
        (Endpoint::Ssh { user, .. }, None) => format!("ssh://{user}@{host}/{path}"),
        (Endpoint::Ssh { user, .. }, Some(port)) => format!("ssh://{user}@{host}:{port}/{path}"),
    };
    let label = if exact_endpoint {
        // The old label cannot prove which spelling the user approved.
        format!("{EXACT_ENDPOINT_PREFIX}{label}")
    } else {
        label
    };
    (label.len() <= MAX_FETCH_URL_BYTES).then_some(RepositoryOrigin(label))
}

/// Limit cross-transport matches to known services with a shared Git login and
/// a repository namespace that does not depend on the SSH account.
fn standard_hosted_path(host: &str, path: &str) -> bool {
    let components = path.split('/').count();
    match host {
        "github.com" | "bitbucket.org" => components == 2,
        "gitlab.com" => components >= 2,
        _ => false,
    }
}

/// Split off the port or SCP path. Keep colons inside an IPv6 host intact.
fn split_host(authority: &str) -> (&str, &str) {
    if authority.starts_with('[') {
        return authority
            .find(']')
            .map_or((authority, ""), |end| authority.split_at(end + 1));
    }
    authority
        .find(':')
        .map_or((authority, ""), |end| authority.split_at(end))
}

fn canonical_host(host: &str) -> Option<String> {
    if let Some(ip) = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
    {
        let ip = ip.parse::<Ipv6Addr>().ok()?;
        return Some(format!("[{ip}]"));
    }
    if host.parse::<Ipv4Addr>().is_ok() {
        return Some(host.to_owned());
    }
    let last = host.rsplit('.').next()?;
    let numeric_suffix = last.bytes().all(|byte| byte.is_ascii_digit())
        || last
            .get(..2)
            .filter(|prefix| prefix.eq_ignore_ascii_case("0x"))
            .is_some_and(|_| last[2..].bytes().all(|byte| byte.is_ascii_hexdigit()));
    // Reject alternate numeric IP spellings and names that URL parsers may
    // interpret as numeric addresses.
    if host.len() > 253
        || numeric_suffix
        || !host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

fn valid_https_user_info(user_info: &str) -> bool {
    let mut bytes = user_info.bytes();
    if user_info.is_empty() {
        return false;
    }
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            if !bytes.next().is_some_and(|byte| byte.is_ascii_hexdigit())
                || !bytes.next().is_some_and(|byte| byte.is_ascii_hexdigit())
            {
                return false;
            }
        } else if !byte.is_ascii_alphanumeric() && !b"-._~!$&'()*+,;=:".contains(&byte) {
            return false;
        }
    }
    true
}

fn valid_ssh_user(user: &str) -> bool {
    !user.is_empty() && !user.starts_with('-') && user.bytes().all(valid_path_byte)
}

/// Check the path without decoding it or resolving its components.
fn valid_repository_path(path: &str) -> bool {
    path.split('/')
        .all(|part| !part.is_empty() && !part.ends_with('.') && part.bytes().all(valid_path_byte))
}

fn valid_path_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(url: &str) -> Option<RepositoryOrigin> {
        RepositoryOrigin::from_fetch_urls(format!("{url}\0").as_bytes())
    }

    #[test]
    fn normal_endpoints_match_without_credentials() {
        let expected = origin("https://gitlab.com/Group/Nested/Repo").unwrap();
        assert_eq!(expected.as_label(), "gitlab.com/Group/Nested/Repo");
        for url in [
            "https://user:secret%40value@GITLAB.COM:443/Group/Nested/Repo.git",
            "ssh://git@gitlab.com:22/Group/Nested/Repo.git",
            "git@GITLAB.COM:Group/Nested/Repo.git",
        ] {
            assert_eq!(origin(url), Some(expected.clone()));
        }
        for url in [
            "https://gitlab.com/group/Nested/Repo",
            "https://gitlab.com/Group/Repo",
            "https://mirror.example.org/Group/Nested/Repo",
            "https://gitlab.com/Group/Nested/Repo.GIT",
            "ssh://alice@gitlab.com/Group/Nested/Repo",
            "ssh://git@gitlab.com:2222/Group/Nested/Repo",
        ] {
            assert_ne!(origin(url).unwrap(), expected);
        }
    }

    #[test]
    fn generic_ssh_keeps_the_user_and_path_kind() {
        let alice = origin("ssh://alice@git.example.org/Group/Repo").unwrap();
        assert_eq!(
            alice.as_label(),
            "endpoint-v2:ssh://alice@git.example.org/Group/Repo"
        );
        for distinct in [
            "ssh://alice@git.example.org:22/Group/Repo",
            "ssh://bob@git.example.org/Group/Repo",
            "alice@git.example.org:Group/Repo",
            "https://git.example.org/Group/Repo",
        ] {
            assert_ne!(origin(distinct).unwrap(), alice);
        }
        assert_ne!(
            origin("ssh://git@git.example.org/Group/Repo"),
            origin("https://git.example.org/Group/Repo")
        );
        assert!(origin("ssh://git.example.org/Group/Repo").is_none());
        assert!(origin("git.example.org:Group/Repo").is_none());
        assert_ne!(
            origin("ssh://git@github.com/Unexpected/Group/Repo"),
            origin("https://github.com/Unexpected/Group/Repo")
        );
    }

    #[test]
    fn custom_endpoints_keep_the_transport_and_port() {
        let normal = origin("git@git.example.org:Group/Repo").unwrap();
        let https = origin("https://synthetic-user@git.example.org:8443/Group/Repo.git").unwrap();
        let ssh = origin("ssh://git@git.example.org:8443/Group/Repo").unwrap();
        assert_eq!(
            https.as_label(),
            "endpoint-v2:https://git.example.org:8443/Group/Repo.git"
        );
        assert_eq!(
            ssh.as_label(),
            "endpoint-v2:ssh://git@git.example.org:8443/Group/Repo"
        );
        assert_ne!(
            origin("ssh://alice@git.example.org:8443/Group/Repo"),
            origin("ssh://bob@git.example.org:8443/Group/Repo")
        );
        assert_ne!(https, ssh);
        assert_ne!(
            origin("https://git.example.org:22/Group/Repo").unwrap(),
            normal
        );
        assert_ne!(
            origin("ssh://git@git.example.org:443/Group/Repo").unwrap(),
            normal
        );
        assert_ne!(
            origin("ssh://git@git.example.org:2222/Group/Repo").unwrap(),
            ssh
        );
    }

    #[test]
    fn generic_endpoint_labels_keep_the_suffix_and_do_not_match_legacy_grants() {
        for (url, legacy) in [
            (
                "https://git.example.org/Team/App",
                "git.example.org/Team/App",
            ),
            (
                "ssh://git@git.example.org/Team/App",
                "ssh://git@git.example.org/Team/App",
            ),
            (
                "git@git.example.org:Team/App",
                "scp://git@git.example.org/Team/App",
            ),
            (
                "https://github.com:8443/Team/App",
                "https://github.com:8443/Team/App",
            ),
            (
                "ssh://alice@github.com/Team/App",
                "ssh://alice@github.com/Team/App",
            ),
        ] {
            let bare = origin(url).unwrap();
            let suffixed = origin(&format!("{url}.git")).unwrap();
            assert_eq!(bare.as_label(), format!("endpoint-v2:{legacy}"));
            assert_eq!(suffixed.as_label(), format!("endpoint-v2:{legacy}.git"));
            assert!(valid_repository_origin_label(bare.as_label()));
            assert!(valid_repository_origin_label(suffixed.as_label()));
            assert!(valid_repository_origin_label(legacy));
            assert_eq!(
                legacy_repository_origin_for_off_choice(bare.as_label()).as_deref(),
                Some(legacy)
            );
            assert_eq!(
                legacy_repository_origin_for_off_choice(suffixed.as_label()).as_deref(),
                Some(legacy)
            );
        }
    }

    #[test]
    fn explicit_generic_ssh_default_port_keeps_its_identity_but_shares_legacy_off_choice() {
        let url = "ssh://git@git.example.org:22/Team/App.git";
        let label = origin(url).unwrap();
        assert_eq!(
            label.as_label(),
            "endpoint-v2:ssh://git@git.example.org:22/Team/App.git"
        );
        assert_ne!(
            label,
            origin("ssh://git@git.example.org/Team/App.git").unwrap()
        );
        assert!(valid_repository_origin_label(label.as_label()));
        assert_eq!(
            legacy_repository_origin_for_off_choice(label.as_label()).as_deref(),
            Some("ssh://git@git.example.org/Team/App")
        );
        for label in [
            "ssh://git@git.example.org/Team/App",
            "github.com/Team/App",
            "endpoint-v2:github.com/Team/App",
            "endpoint-v2:ssh://git@git.example.org:022/Team/App",
        ] {
            assert_eq!(legacy_repository_origin_for_off_choice(label), None);
        }
    }

    #[test]
    fn standard_hosted_endpoints_keep_the_existing_label() {
        for host in ["github.com", "gitlab.com", "bitbucket.org"] {
            let label = format!("{host}/Team/App");
            for url in [
                format!("https://{label}"),
                format!("https://{label}.git"),
                format!("ssh://git@{label}.git"),
                format!("ssh://git@{host}:22/Team/App.git"),
                format!("git@{host}:Team/App.git"),
            ] {
                assert_eq!(origin(&url).unwrap().as_label(), label);
            }
            assert!(valid_repository_origin_label(&label));
        }
    }

    #[test]
    fn accepts_canonical_ip_hosts() {
        assert_eq!(
            origin("https://192.0.2.4/Group/Repo").unwrap().as_label(),
            "endpoint-v2:192.0.2.4/Group/Repo"
        );
        let ipv6 = origin("git@[2001:DB8:0::1]:Group/Repo").unwrap();
        assert_eq!(
            ipv6.as_label(),
            "endpoint-v2:scp://git@[2001:db8::1]/Group/Repo"
        );
        assert_ne!(origin("https://[2001:db8::1]:443/Group/Repo"), Some(ipv6));
    }

    #[test]
    fn rejects_local_or_unsupported_origins() {
        for url in [
            "/home/me/Group/Repo",
            "../Group/Repo",
            "Group/Repo",
            "C:/Group/Repo",
            "C:Group/Repo",
            r"\\server\share\Repo",
            "file:///Group/Repo",
            "file:Group/Repo",
            "http://git.example.org/Group/Repo",
            "git://git.example.org/Group/Repo",
            "git+ssh://git.example.org/Group/Repo",
            "ext::git.example.org/Group/Repo",
        ] {
            assert!(origin(url).is_none(), "{url}");
        }
    }

    #[test]
    fn rejects_ambiguous_or_malformed_addresses() {
        for url in [
            "https://git.example.org/Group/../Repo",
            "https://git.example.org/Group/./Repo",
            "https://git.example.org/Group/Repo/...",
            "https://git.example.org/Group/.git",
            "https://git.example.org/Group/Repo..git",
            "https://git.example.org//Group/Repo",
            "https://git.example.org/Group/Repo/",
            "https://git.example.org/Group//Repo",
            "https://git.example.org/Group/%2e%2e/Repo",
            "https://git.example.org/Group%2FRepo",
            "https://git.example.org/Group/R%65po",
            r"https://git.example.org/Group\Repo",
            "https://git.example.org/Group/Repo?x=y",
            "https://git.example.org/Group/Repo#fragment",
            "https://git.example.org/Group/Répo",
            "https://git.example.org/Group/Repo\n",
            " https://git.example.org/Group/Repo",
            "https:///Group/Repo",
            "https://git.example.org./Group/Repo",
            "https://git..example.org/Group/Repo",
            "https://git.example.org:/Group/Repo",
            "https://git.example.org:0443/Group/Repo",
            "https://git.example.org:0/Group/Repo",
            "https://git.example.org:+443/Group/Repo",
            "https://git.example.org:65536/Group/Repo",
            "https://0192.0.2.4/Group/Repo",
            "https://0xc0000204/Group/Repo",
            "https://192.4/Group/Repo",
            "https://[2001:db8::1]other/Group/Repo",
            "https://a@b@git.example.org/Group/Repo",
            "https://user:%2G@git.example.org/Group/Repo",
            "ssh://-user@git.example.org/Group/Repo",
            "ssh://git@git.example.org/~/Group/Repo",
            "git@git.example.org:/Group/Repo",
            "git@git.example.org:2222/Group:Repo",
        ] {
            assert!(origin(url).is_none(), "{url}");
        }
    }

    #[test]
    fn needs_exactly_one_complete_fetch_value() {
        for output in [
            &b""[..],
            b"\0",
            b"https://git.example.org/Group/Repo",
            b"https://git.example.org/Group/Repo\0trailing",
            b"https://git.example.org/Group/Repo\0\0",
            b"https://git.example.org/Group/Repo\0https://git.example.org/Group/Repo\0",
            b"https://git.example.org/Group/\xff\0",
        ] {
            assert!(RepositoryOrigin::from_fetch_urls(output).is_none());
        }
        assert!(origin(&format!("https://git.example.org/{}", "a".repeat(4096))).is_none());
    }
}

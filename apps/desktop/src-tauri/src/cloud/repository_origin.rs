//! Match repository rules against the configured local fetch origin.
//! This parser does not read Git configuration or resolve URL rewrites.

use std::net::{Ipv4Addr, Ipv6Addr};

const MAX_FETCH_URL_BYTES: usize = 4096;

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
        parse_fetch_url(std::str::from_utf8(value).ok()?)
    }

    /// Use this exact label for storage and matching. Only recognized hosted
    /// endpoints can share an HTTPS and SSH label. Other SSH labels retain the
    /// login and distinguish relative SCP paths from absolute SSH paths.
    pub(super) fn as_label(&self) -> &str {
        &self.0
    }
}

/// Accept only labels this parser can produce. Never treat a stored label as
/// an address for a network request.
pub(crate) fn valid_repository_origin_label(label: &str) -> bool {
    if label.is_empty() || label.len() > MAX_FETCH_URL_BYTES {
        return false;
    }
    let url = if let Some(relative) = label.strip_prefix("scp://") {
        let Some((authority, path)) = relative.split_once('/') else {
            return false;
        };
        format!("{authority}:{path}.git")
    } else if label.starts_with("ssh://") || label.starts_with("https://") {
        format!("{label}.git")
    } else {
        format!("https://{label}.git")
    };
    parse_fetch_url(&url).is_some_and(|origin| origin.as_label() == label)
}

enum Endpoint<'a> {
    Https,
    Ssh { user: &'a str, relative: bool },
}

fn parse_fetch_url(value: &str) -> Option<RepositoryOrigin> {
    let (host, endpoint, port, path) = if let Some((scheme, rest)) = value.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        let (user, authority) = match authority.split_once('@') {
            Some((user, host)) => (Some(user), host),
            None => (None, authority),
        };
        let (endpoint, default_port) = match scheme {
            "https" if user.is_none_or(valid_https_user_info) => (Endpoint::Https, 443),
            "ssh" => (
                Endpoint::Ssh {
                    user: user.filter(|user| valid_ssh_user(user))?,
                    relative: false,
                },
                22,
            ),
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
            let port = port.parse::<u16>().ok()?;
            (port != default_port).then_some(port)
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
    let path = canonical_path(path)?;
    let label = match (endpoint, port) {
        (Endpoint::Https, None) => format!("{host}/{path}"),
        (Endpoint::Https, Some(port)) => format!("https://{host}:{port}/{path}"),
        (Endpoint::Ssh { user: "git", .. }, None) if standard_hosted_path(&host, path) => {
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
    Some(RepositoryOrigin(label))
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

/// Keep all path components and their case. Strip one terminal `.git` suffix.
/// Reject empty components and trailing dots. Do not decode or resolve paths.
fn canonical_path(path: &str) -> Option<&str> {
    let path = path.strip_suffix(".git").unwrap_or(path);
    path.split('/')
        .all(|part| !part.is_empty() && !part.ends_with('.') && part.bytes().all(valid_path_byte))
        .then_some(path)
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
        let alice = origin("ssh://alice@git.example.org:22/Group/Repo").unwrap();
        assert_eq!(alice.as_label(), "ssh://alice@git.example.org/Group/Repo");
        for distinct in [
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
        let https = origin("https://me:secret@git.example.org:8443/Group/Repo.git").unwrap();
        let ssh = origin("ssh://git@git.example.org:8443/Group/Repo").unwrap();
        assert_eq!(https.as_label(), "https://git.example.org:8443/Group/Repo");
        assert_eq!(ssh.as_label(), "ssh://git@git.example.org:8443/Group/Repo");
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
    fn accepts_canonical_ip_hosts() {
        assert_eq!(
            origin("https://192.0.2.4/Group/Repo").unwrap().as_label(),
            "192.0.2.4/Group/Repo"
        );
        let ipv6 = origin("git@[2001:DB8:0::1]:Group/Repo").unwrap();
        assert_eq!(ipv6.as_label(), "scp://git@[2001:db8::1]/Group/Repo");
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

//! Shared codecs for "workspace key → real filesystem path" decoding.
//!
//! Several agents serialize a workspace's absolute path into a hyphen-flattened
//! directory or file name (e.g. Claude's `~/.claude/projects/-Users-x-repo/`,
//! Cursor's `workspaceStorage/-Users-x-repo`). Recovering the original path
//! from such a name is ambiguous because the segment separator collides with
//! hyphens inside path components — both decoders walk the candidate space
//! depth-first, validating each prefix against the real filesystem.
//!
//! The two algorithms are structurally similar but agent-specific in their
//! Windows drive handling and input-shape pre-checks. They live here together
//! so the next agent that needs one knows where to add it and where to look
//! for prior art.

use std::path::{MAIN_SEPARATOR, Path, PathBuf};

/// Bounds the exponential candidate space the decoders walk before giving up.
/// At 65 536 candidates a typical 5–8 segment path resolves in microseconds;
/// pathological inputs short-circuit instead of blocking the runtime.
const PATH_DECODE_MAX_CANDIDATES: usize = 65_536;

/// True if `candidate` is either inside `boundary` or an ancestor on the
/// way down to it. Both decoders walk from `/` (or a Windows drive root)
/// toward a leaf, so intermediate prefixes like `/Users` legitimately need
/// to be probed; we accept them while still refusing to stat anywhere that
/// can't possibly reach `boundary`.
fn within_boundary(candidate: &Path, boundary: &Path) -> bool {
    candidate.starts_with(boundary) || boundary.starts_with(candidate)
}

fn initial_hyphenated_path_state(trimmed: &str) -> (PathBuf, usize) {
    if MAIN_SEPARATOR == '\\' {
        let bytes = trimmed.as_bytes();
        if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() {
            let drive = char::from(bytes[0]);
            if bytes[1] == b':' && bytes[2] == b'-' {
                return (PathBuf::from(format!("{drive}:{MAIN_SEPARATOR}")), 3);
            }
            if bytes[1] == b'-' && bytes[2] == b'-' {
                return (PathBuf::from(format!("{drive}:{MAIN_SEPARATOR}")), 3);
            }
        }
    }

    (PathBuf::from(MAIN_SEPARATOR.to_string()), 0)
}

/// Decode a Claude-flavored hyphenated path under `home_boundary`.
///
/// Encoded inputs come from session-log content, so the decoder MUST refuse
/// to probe paths outside the user's home — otherwise a crafted workspace key
/// (`-etc-passwd`, `-proc-self-mem`, …) turns `tokio::fs::try_exists` into a
/// filesystem-existence oracle. `home_boundary` is the user's home dir; only
/// candidates inside it (or ancestors of it on the walk down) are stat'd.
pub async fn decode_hyphenated_absolute_path(
    encoded: &str,
    home_boundary: &Path,
) -> Option<PathBuf> {
    let trimmed = encoded.strip_prefix('-')?;
    if trimmed.is_empty() {
        return None;
    }

    let mut stack = vec![initial_hyphenated_path_state(trimmed)];
    let mut checked_candidates = 0usize;
    while let Some((prefix, start_idx)) = stack.pop() {
        if start_idx >= trimmed.len() {
            return Some(prefix);
        }

        let mut next = Vec::new();
        let bytes = trimmed.as_bytes();
        for end_idx in (start_idx + 1..=trimmed.len()).rev() {
            if end_idx != trimmed.len() && bytes[end_idx] != b'-' {
                continue;
            }

            // SAFETY: `end_idx` is either `trimmed.len()` or a position where
            // `bytes[end_idx] == b'-'`. `-` (0x2D) is single-byte ASCII and
            // cannot appear as a UTF-8 continuation byte, so `end_idx` is
            // always at a codepoint boundary. `start_idx` was initialized
            // either at 0 (or 3, after a Windows drive prefix consisting of
            // ASCII bytes only) or at a previous boundary-validated
            // `end_idx + 1` (line below). Both endpoints are valid; the
            // direct slice cannot panic. If a future edit adds a new source
            // for `start_idx` or `end_idx`, re-verify this invariant.
            let segment = &trimmed[start_idx..end_idx];
            if segment.is_empty() {
                continue;
            }

            let candidate = prefix.join(segment);
            if !within_boundary(&candidate, home_boundary) {
                continue;
            }

            checked_candidates += 1;
            if checked_candidates > PATH_DECODE_MAX_CANDIDATES {
                return None;
            }

            if tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
                let next_idx = if end_idx == trimmed.len() {
                    end_idx
                } else {
                    end_idx + 1
                };
                next.push((candidate, next_idx));
            }
        }
        stack.extend(next);
    }

    None
}

fn initial_cursor_path_state(trimmed: &str) -> (PathBuf, usize) {
    if MAIN_SEPARATOR == '\\' {
        let bytes = trimmed.as_bytes();
        if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() {
            let drive = char::from(bytes[0]);
            if bytes[1] == b'-' && bytes[2] == b'-' {
                return (PathBuf::from(format!("{drive}:{MAIN_SEPARATOR}")), 3);
            }
        }
    }

    (PathBuf::from(MAIN_SEPARATOR.to_string()), 0)
}

/// Decode a Cursor workspace key under `home_boundary`. See
/// [`decode_hyphenated_absolute_path`] for why the boundary is required.
pub async fn decode_cursor_workspace_path_buf(
    encoded: &str,
    home_boundary: &Path,
) -> Option<PathBuf> {
    if encoded.trim().is_empty() {
        return None;
    }

    let mut stack = vec![initial_cursor_path_state(encoded)];
    let mut checked_candidates = 0usize;
    while let Some((prefix, start_idx)) = stack.pop() {
        if start_idx >= encoded.len() {
            return Some(prefix);
        }

        let mut next = Vec::new();
        let bytes = encoded.as_bytes();
        for end_idx in (start_idx + 1..=encoded.len()).rev() {
            if end_idx != encoded.len() && bytes[end_idx] != b'-' {
                continue;
            }

            // SAFETY: same invariant as `decode_hyphenated_absolute_path` —
            // `end_idx` is always at a `-` codepoint boundary or at the end
            // of the string; `start_idx` is always at a previously
            // boundary-validated position. See the longer comment in the
            // Claude decoder above.
            let segment = &encoded[start_idx..end_idx];
            if segment.is_empty() {
                continue;
            }

            let candidate = prefix.join(segment);
            if !within_boundary(&candidate, home_boundary) {
                continue;
            }

            checked_candidates += 1;
            if checked_candidates > PATH_DECODE_MAX_CANDIDATES {
                return None;
            }

            if tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
                let next_idx = if end_idx == encoded.len() {
                    end_idx
                } else {
                    end_idx + 1
                };
                next.push((candidate, next_idx));
            }
        }

        stack.extend(next);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test(flavor = "current_thread")]
    async fn cursor_decoder_resolves_hyphenated_segments_via_filesystem() {
        // Locks in the "ambiguous split disambiguates by filesystem probe"
        // contract for the Cursor codec — mirrors the Claude codec's test
        // below, so the next change to either function trips one.
        let dir = TempDir::new().unwrap();
        let repo_root = dir.path().join("work").join("project-with-dashes");
        tokio::fs::create_dir_all(&repo_root).await.unwrap();

        let encoded = encode_cursor(&repo_root);
        let decoded = decode_cursor_workspace_path_buf(&encoded, dir.path()).await;
        assert_eq!(decoded, Some(repo_root));
    }

    /// Replicate Claude's project-name encoding for tests across platforms.
    /// Unix: `/Users/x/repo` → `-Users-x-repo` (the leading `/` becomes `-`).
    /// Windows: `C:\Users\x\repo` → `-C--Users-x-repo` (drive's `:` becomes
    /// a second `-`, separators become `-`, prepend `-`). The decoder accepts
    /// both `C:-` and `C--` drive prefixes; we emit the filesystem-safe form.
    fn encode_claude(path: &Path) -> String {
        let s = path.to_string_lossy();
        if MAIN_SEPARATOR == '\\' {
            let replaced = s.replace('\\', "-").replacen(':', "-", 1);
            format!("-{replaced}")
        } else {
            s.replace('/', "-")
        }
    }

    /// Replicate Cursor's workspace-key encoding for tests across platforms.
    /// Cursor keys are unprefixed (no leading `-`). Unix: drop leading `/`,
    /// collapse internal `/` to `-`. Windows: drop drive's `:`, collapse `\`
    /// to `-` so the result starts with the drive letter and matches the
    /// `C--…` shape the Cursor decoder recognises.
    fn encode_cursor(path: &Path) -> String {
        let s = path.to_string_lossy();
        if MAIN_SEPARATOR == '\\' {
            s.replace('\\', "-").replacen(':', "-", 1)
        } else {
            s.trim_start_matches('/').replace('/', "-")
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn claude_decoder_resolves_hyphenated_segments_via_filesystem() {
        let dir = TempDir::new().unwrap();
        let repo_root = dir.path().join("work").join("git--context-explorer");
        tokio::fs::create_dir_all(&repo_root).await.unwrap();

        let decoded = decode_hyphenated_absolute_path(&encode_claude(&repo_root), dir.path()).await;
        assert_eq!(decoded, Some(repo_root));
    }

    #[test]
    fn initial_hyphenated_path_state_handles_windows_drive_prefix() {
        let (root, start_idx) = initial_hyphenated_path_state("C:-Users-avery");
        if MAIN_SEPARATOR == '\\' {
            assert_eq!(root, PathBuf::from(r"C:\"));
            assert_eq!(start_idx, 3);
        } else {
            assert_eq!(root, PathBuf::from("/"));
            assert_eq!(start_idx, 0);
        }
    }

    #[test]
    fn initial_hyphenated_path_state_handles_windows_safe_drive_prefix() {
        let (root, start_idx) = initial_hyphenated_path_state("C--Users-avery");
        if MAIN_SEPARATOR == '\\' {
            assert_eq!(root, PathBuf::from(r"C:\"));
            assert_eq!(start_idx, 3);
        } else {
            assert_eq!(root, PathBuf::from("/"));
            assert_eq!(start_idx, 0);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn claude_decoder_refuses_paths_outside_home_boundary() {
        // Regression: a crafted workspace key must not stat files outside
        // the user's home, even if the path exists. We point the boundary
        // at one TempDir but craft an encoded path that would resolve into
        // a sibling TempDir; the decoder must return None.
        let inside = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("etc");
        tokio::fs::create_dir_all(&target).await.unwrap();

        let decoded = decode_hyphenated_absolute_path(&encode_claude(&target), inside.path()).await;
        assert_eq!(decoded, None);
    }
}

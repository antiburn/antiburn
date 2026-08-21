// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::ffi::OsString;
use std::path::PathBuf;

pub(crate) enum Os {
    Mac,
    Windows,
    Linux,
}

pub(crate) fn resolve_log_dir(directory_name: &str) -> Option<PathBuf> {
    let os = if cfg!(target_os = "macos") {
        Os::Mac
    } else if cfg!(target_os = "windows") {
        Os::Windows
    } else {
        Os::Linux
    };
    log_dir_with(os, &|name| std::env::var_os(name), directory_name)
}

fn non_empty_env_path(env: &dyn Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    env(name)
        .filter(|value| !value.as_os_str().is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn is_safe_directory_name(directory_name: &str) -> bool {
    !directory_name.is_empty()
        && directory_name != "."
        && directory_name != ".."
        && directory_name
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, '.' | '-' | '_'))
}

pub(crate) fn log_dir_with(
    os: Os,
    env: &dyn Fn(&str) -> Option<OsString>,
    directory_name: &str,
) -> Option<PathBuf> {
    if !is_safe_directory_name(directory_name) {
        return None;
    }
    match os {
        Os::Mac => non_empty_env_path(env, "HOME")
            .map(|home| home.join("Library/Logs").join(directory_name)),
        Os::Windows => non_empty_env_path(env, "LOCALAPPDATA")
            .map(|base| base.join(directory_name).join("logs")),
        Os::Linux => non_empty_env_path(env, "XDG_STATE_HOME")
            .or_else(|| non_empty_env_path(env, "HOME").map(|home| home.join(".local/state")))
            .map(|base| base.join(directory_name).join("logs")),
    }
}

#[cfg(test)]
mod tests {
    use super::{Os, log_dir_with};
    use std::collections::HashMap;
    use std::ffi::OsString;

    fn env(values: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let values: HashMap<String, OsString> = values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
            .collect();
        move |name| values.get(name).cloned()
    }

    #[test]
    fn resolves_each_platform_directory() {
        let root = std::env::temp_dir().join("antiburn-path-tests");
        let home = root.join("home");
        let local = root.join("local");
        let state = root.join("state");
        let home_text = home.to_string_lossy();
        let local_text = local.to_string_lossy();
        let state_text = state.to_string_lossy();
        assert_eq!(
            log_dir_with(Os::Mac, &env(&[("HOME", &home_text)]), "app.id"),
            Some(home.join("Library/Logs/app.id"))
        );
        assert_eq!(
            log_dir_with(
                Os::Windows,
                &env(&[("LOCALAPPDATA", &local_text)]),
                "app.id"
            ),
            Some(local.join("app.id/logs"))
        );
        assert_eq!(
            log_dir_with(
                Os::Linux,
                &env(&[("XDG_STATE_HOME", &state_text)]),
                "app.id"
            ),
            Some(state.join("app.id/logs"))
        );
        assert_eq!(
            log_dir_with(Os::Linux, &env(&[("HOME", &home_text)]), "app.id"),
            Some(home.join(".local/state/app.id/logs"))
        );
    }

    #[test]
    fn unresolved_and_empty_environment_values_return_none() {
        assert_eq!(log_dir_with(Os::Mac, &env(&[]), "app.id"), None);
        assert_eq!(
            log_dir_with(Os::Windows, &env(&[("LOCALAPPDATA", "")]), "app.id"),
            None
        );
        assert_eq!(
            log_dir_with(
                Os::Linux,
                &env(&[("XDG_STATE_HOME", ""), ("HOME", "")]),
                "app.id"
            ),
            None
        );
        assert_eq!(
            log_dir_with(
                Os::Linux,
                &env(&[("XDG_STATE_HOME", "relative"), ("HOME", "also-relative")]),
                "app.id"
            ),
            None
        );
    }

    #[test]
    fn names_select_distinct_directories() {
        let state = std::env::temp_dir().join("antiburn-path-tests/state");
        let state_text = state.to_string_lossy();
        let environment = [("XDG_STATE_HOME", state_text.as_ref())];
        let values = env(&environment);
        let release = log_dir_with(Os::Linux, &values, "antiburn");
        let debug = log_dir_with(Os::Linux, &values, "antiburn-debug");
        assert_ne!(release, debug);
    }

    #[test]
    fn path_like_names_return_none() {
        let state = std::env::temp_dir().join("antiburn-path-tests/state");
        let state_text = state.to_string_lossy();
        let environment = [("XDG_STATE_HOME", state_text.as_ref())];
        let values = env(&environment);
        for directory_name in [
            "",
            ".",
            "..",
            "../escape",
            "/absolute",
            "nested/path",
            "nested\\path",
            "C:escape",
        ] {
            assert_eq!(log_dir_with(Os::Linux, &values, directory_name), None);
        }
    }
}

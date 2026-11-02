//! Program outcomes stay separate from the tool's terminal result status.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellState {
    Running,
    Completed,
    Exited,
    Timeout,
    Killed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkState {
    Running,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolOutcome {
    Shell {
        state: Option<ShellState>,
        exit: Option<i64>,
        signal: Option<String>,
        timed_out: Option<bool>,
        shell_id: Option<String>,
    },
    CodeMode {
        error: Option<bool>,
    },
    Subagent {
        session_id: String,
        state: WorkState,
    },
}

// Postcard needs enum tags instead of JSON's kind field.
mod outcome_serde {
    use super::{NativeToolOutcome, ShellState, WorkState};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(remote = "NativeToolOutcome", tag = "kind", rename_all = "snake_case")]
    enum HumanReadable {
        Shell {
            state: Option<ShellState>,
            exit: Option<i64>,
            signal: Option<String>,
            timed_out: Option<bool>,
            shell_id: Option<String>,
        },
        CodeMode {
            error: Option<bool>,
        },
        Subagent {
            session_id: String,
            state: WorkState,
        },
    }

    #[derive(Serialize, Deserialize)]
    #[serde(remote = "NativeToolOutcome")]
    enum Binary {
        Shell {
            state: Option<ShellState>,
            exit: Option<i64>,
            signal: Option<String>,
            timed_out: Option<bool>,
            shell_id: Option<String>,
        },
        CodeMode {
            error: Option<bool>,
        },
        Subagent {
            session_id: String,
            state: WorkState,
        },
    }

    impl Serialize for NativeToolOutcome {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            if serializer.is_human_readable() {
                HumanReadable::serialize(self, serializer)
            } else {
                Binary::serialize(self, serializer)
            }
        }
    }

    impl<'de> Deserialize<'de> for NativeToolOutcome {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            if deserializer.is_human_readable() {
                HumanReadable::deserialize(deserializer)
            } else {
                Binary::deserialize(deserializer)
            }
        }
    }
}

impl NativeToolOutcome {
    pub(crate) fn retained_string_bytes(&self) -> usize {
        match self {
            Self::Shell {
                signal, shell_id, ..
            } => signal.as_ref().map_or(0, String::len) + shell_id.as_ref().map_or(0, String::len),
            Self::CodeMode { .. } => 0,
            Self::Subagent { session_id, .. } => session_id.len(),
        }
    }

    pub(crate) fn opencode_v2_tool(
        name: &str,
        metadata: Option<&Value>,
        provider_executed: Option<bool>,
        failed: bool,
    ) -> Option<Self> {
        if provider_executed == Some(true) {
            return None;
        }
        let metadata = metadata?.as_object()?;
        match name {
            "shell" => {
                let state = match metadata.get("status").and_then(Value::as_str) {
                    Some("running") => Some(ShellState::Running),
                    Some("completed") => Some(ShellState::Completed),
                    _ => None,
                };
                let exit = metadata.get("exit").and_then(Value::as_i64);
                let signal = bounded_string(metadata.get("signal"));
                let timed_out = metadata.get("timeout").and_then(Value::as_bool);
                let shell_id = bounded_string(metadata.get("shellID"));
                (state.is_some()
                    || exit.is_some()
                    || signal.is_some()
                    || timed_out.is_some()
                    || shell_id.is_some())
                .then_some(Self::Shell {
                    state,
                    exit,
                    signal,
                    timed_out,
                    shell_id,
                })
            }
            "execute" => Some(Self::CodeMode {
                error: metadata.get("error").and_then(Value::as_bool),
            }),
            "subagent" => {
                // A failed terminal can retain earlier progress metadata.
                if failed {
                    return None;
                }
                let session_id = bounded_string(metadata.get("sessionID"))?;
                if !session_id.starts_with("ses_") {
                    return None;
                }
                let state = match metadata.get("status")?.as_str()? {
                    "running" => WorkState::Running,
                    "completed" => WorkState::Completed,
                    _ => return None,
                };
                Some(Self::Subagent { session_id, state })
            }
            _ => None,
        }
    }

    pub(crate) fn opencode_v2_shell_message(value: &Value) -> Option<Self> {
        let state = match value.get("status")?.as_str()? {
            "running" => ShellState::Running,
            "exited" => ShellState::Exited,
            "timeout" => ShellState::Timeout,
            "killed" => ShellState::Killed,
            _ => return None,
        };
        Some(Self::Shell {
            state: Some(state),
            exit: value.get("exit").and_then(Value::as_i64),
            // The pinned shell message projection does not retain a signal.
            signal: None,
            timed_out: (state == ShellState::Timeout).then_some(true),
            shell_id: bounded_string(value.get("shellID")),
        })
    }
}

fn bounded_string(value: Option<&Value>) -> Option<String> {
    value?
        .as_str()
        .filter(|text| {
            !text.is_empty()
                && text.len() <= crate::analysis::EVIDENCE_STRING_CAP
                && !text.contains('\0')
        })
        .map(str::to_owned)
}

#[derive(Clone, Copy)]
pub(super) enum Stage {
    Resolve,
    SwitchPreflight,
    RuntimeRestore,
    StartupLegacy,
    LegacyUpdate,
    CredentialRemoval,
}

impl Stage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Resolve => "resolve",
            Self::SwitchPreflight => "switch_preflight",
            Self::RuntimeRestore => "runtime_restore",
            Self::StartupLegacy => "startup_legacy",
            Self::LegacyUpdate => "legacy_update",
            Self::CredentialRemoval => "credential_removal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Category {
    Missing,
    Malformed,
    InvalidReference,
    Access,
    Cancelled,
    Unavailable,
    Ambiguous,
    Platform,
}

impl Category {
    fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Malformed => "malformed",
            Self::InvalidReference => "invalid_reference",
            Self::Access => "access",
            Self::Cancelled => "cancelled",
            Self::Unavailable => "unavailable",
            Self::Ambiguous => "ambiguous",
            Self::Platform => "platform",
        }
    }

    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Missing => "The saved credential is missing. Use Edit → Change to add it again.",
            Self::Malformed => {
                "The saved credential has an invalid format. Use Edit → Change to replace it."
            }
            Self::InvalidReference => "The connection credential reference is invalid.",
            Self::Access => {
                "The system did not allow credential access. Check app access and retry."
            }
            Self::Cancelled => {
                "Credential access was cancelled. Use the connection again when ready."
            }
            Self::Unavailable => "Credential storage is unavailable.",
            Self::Ambiguous => {
                "Credential storage matched more than one entry. Check the system credential store before retrying."
            }
            Self::Platform => {
                "Could not read the saved credential. Retry, or use Edit → Change to replace it."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Failure {
    category: Category,
    os_status: Option<i32>,
}

impl Failure {
    pub(super) fn new(category: Category) -> Self {
        Self {
            category,
            os_status: None,
        }
    }

    pub(super) fn from_keyring(error: &keyring::Error) -> Self {
        let os_status = macos_status(error);
        let category = match os_status {
            Some(-128) => Category::Cancelled,
            Some(-25293 | -25308) => Category::Access,
            _ => match error {
                keyring::Error::NoEntry => Category::Missing,
                keyring::Error::BadEncoding(_) => Category::Malformed,
                keyring::Error::Invalid(_, _) | keyring::Error::TooLong(_, _) => {
                    Category::InvalidReference
                }
                keyring::Error::NoStorageAccess(_) => Category::Unavailable,
                keyring::Error::Ambiguous(_) => Category::Ambiguous,
                _ => Category::Platform,
            },
        };
        Self {
            category,
            os_status,
        }
    }

    pub(super) fn report(self, stage: Stage) -> &'static str {
        ::tracing::warn!(
            event = "system_one_credential_read_failed",
            stage = stage.as_str(),
            category = self.category.as_str(),
            os_status = self.os_status,
        );
        self.category.message()
    }
}

#[cfg(target_os = "macos")]
fn macos_status(error: &keyring::Error) -> Option<i32> {
    match error {
        keyring::Error::PlatformFailure(source) | keyring::Error::NoStorageAccess(source) => source
            .downcast_ref::<security_framework::base::Error>()
            .map(|error| error.code()),
        _ => None,
    }
}

#[cfg(not(target_os = "macos"))]
fn macos_status(_: &keyring::Error) -> Option<i32> {
    None
}

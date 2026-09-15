use super::{VendorConfig, VendorPolicy};
use crate::agent_config::{ConfigSetting, ConfigUnavailableReason};

pub(super) struct Antigravity;

pub(super) static ANTIGRAVITY: Antigravity = Antigravity;

impl VendorConfig for Antigravity {
    fn policy(&self, _: ConfigSetting) -> VendorPolicy {
        VendorPolicy::Unsupported(ConfigUnavailableReason::UnsupportedAgent)
    }
}

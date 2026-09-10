use antiburn_local::analysis::SourceFormat;
use antiburn_local::model::AgentKind;

use super::{ActionSupport, RemediationAction, VendorRemediationPolicy};

pub(super) static POLICY: AntigravityPolicy = AntigravityPolicy;

pub(super) struct AntigravityPolicy;

impl VendorRemediationPolicy for AntigravityPolicy {
    fn agent(&self) -> AgentKind {
        AgentKind::Antigravity
    }

    fn action_support(&self, _action: RemediationAction, _source: SourceFormat) -> ActionSupport {
        ActionSupport::Unsupported
    }
}

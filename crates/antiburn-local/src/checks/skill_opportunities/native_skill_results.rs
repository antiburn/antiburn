use crate::analysis::jev_evidence::{
    JevRecordedSkillIdentityKind, JevRecordedSkillResult, JevRecordedSkillStatus,
};

use super::recorded_use::hash;
use super::{RecordedSkillIdentity, SkillUseLifecycle};

pub(super) fn identity(fact: &JevRecordedSkillResult) -> RecordedSkillIdentity {
    match (fact.identity, fact.name.as_ref(), fact.location.as_ref()) {
        (JevRecordedSkillIdentityKind::Name, Some(name), None) => {
            RecordedSkillIdentity::Name { name: name.clone() }
        }
        (JevRecordedSkillIdentityKind::InferredName, Some(name), None) => {
            RecordedSkillIdentity::InferredName { name: name.clone() }
        }
        (JevRecordedSkillIdentityKind::Document, Some(name), Some(location)) => {
            RecordedSkillIdentity::Document {
                name: name.clone(),
                path_digest: hash(location.as_bytes()),
            }
        }
        _ => RecordedSkillIdentity::Unknown,
    }
}

pub(super) fn lifecycle(fact: &JevRecordedSkillResult) -> SkillUseLifecycle {
    match fact.status {
        JevRecordedSkillStatus::Requested => SkillUseLifecycle::Requested,
        JevRecordedSkillStatus::DocumentSelected => SkillUseLifecycle::DocumentSelected,
        JevRecordedSkillStatus::Failed => SkillUseLifecycle::Failed,
        JevRecordedSkillStatus::Unknown => SkillUseLifecycle::Unknown,
    }
}

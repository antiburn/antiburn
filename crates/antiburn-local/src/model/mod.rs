// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local-only value types shared across the engine: agent identity and
//! skill-invocation details.

pub mod agent;
pub mod skill;

pub use agent::AgentKind;
pub use skill::{LocalSkillDetails, SkillDetail, SkillScope, SkillUse};

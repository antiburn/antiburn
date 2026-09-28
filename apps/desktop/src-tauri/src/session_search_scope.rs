//! A fixed activity window shared by metadata and retained-content search.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SessionSearchScope {
    pub days: u32,
    pub from_epoch: i64,
    pub through_epoch: i64,
    pub time_zone: String,
}

impl SessionSearchScope {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!((1..=365).contains(&self.days), "invalid search days");
        ensure!(
            self.from_epoch > 0 && self.through_epoch >= self.from_epoch,
            "invalid search window"
        );
        ensure!(
            self.through_epoch - self.from_epoch <= i64::from(self.days + 1) * 86_400,
            "search window exceeds selected days"
        );
        ensure!(
            !self.time_zone.is_empty() && self.time_zone.len() <= 128,
            "invalid search timezone"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_windows() {
        let valid = SessionSearchScope {
            days: 7,
            from_epoch: 100,
            through_epoch: 200,
            time_zone: "Australia/Brisbane".into(),
        };
        assert!(valid.validate().is_ok());
        for bad in [
            SessionSearchScope {
                days: 0,
                ..valid.clone()
            },
            SessionSearchScope {
                through_epoch: 99,
                ..valid.clone()
            },
            SessionSearchScope {
                through_epoch: 900_000,
                ..valid.clone()
            },
            SessionSearchScope {
                time_zone: "".into(),
                ..valid
            },
        ] {
            assert!(bad.validate().is_err());
        }
    }
}

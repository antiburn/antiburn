use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const KEY: &str = "internal:smartChecksDisplayNumbersV1";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DisplayNumbers {
    assignments: BTreeMap<String, Assignment>,
    counters: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Assignment {
    provider: SystemOneProvider,
    number: u32,
}

fn provider_key(provider: SystemOneProvider) -> &'static str {
    match provider {
        SystemOneProvider::Jev => "jev",
        SystemOneProvider::Ollama => "ollama",
        SystemOneProvider::Cloudflare => "cloudflare",
        SystemOneProvider::Custom => "custom",
    }
}

impl DisplayNumbers {
    pub(super) fn read(store: &Store) -> Result<Option<Self>, &'static str> {
        store
            .internal_value(KEY)
            .map(|raw| {
                serde_json::from_str(&raw)
                    .map_err(|_| "Saved connection display numbers are invalid.")
            })
            .transpose()
    }

    pub(super) fn encode(&self) -> Result<String, &'static str> {
        serde_json::to_string(self).map_err(|_| "Could not save connection display numbers.")
    }

    fn validate(&self) -> Result<(), &'static str> {
        let mut used = BTreeSet::new();
        if self
            .counters
            .keys()
            .any(|key| !matches!(key.as_str(), "jev" | "ollama" | "cloudflare" | "custom"))
        {
            return Err("Saved connection display counters are invalid.");
        }
        for assignment in self.assignments.values() {
            let provider = provider_key(assignment.provider);
            if assignment.number == 0
                || !used.insert((provider, assignment.number))
                || self.counters.get(provider).copied().unwrap_or(0) < assignment.number
            {
                return Err("Saved connection display numbers are invalid.");
            }
        }
        Ok(())
    }

    pub(super) fn allocate(
        &mut self,
        id: &str,
        provider: SystemOneProvider,
    ) -> Result<(), &'static str> {
        self.validate()?;
        if self.assignments.contains_key(id) {
            return Err("This connection display identity was already used.");
        }
        let key = provider_key(provider);
        let number = self
            .counters
            .get(key)
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("The connection display number limit was reached.")?;
        self.assignments
            .insert(id.to_owned(), Assignment { provider, number });
        self.counters.insert(key.to_owned(), number);
        Ok(())
    }

    pub(super) fn migrate(
        &mut self,
        inventory: &BTreeMap<String, SystemOneConnection>,
    ) -> Result<(), &'static str> {
        self.validate()?;
        let mut cursors = self.counters.clone();
        let mut hints = BTreeMap::<(&str, u32), usize>::new();
        for (id, connection) in inventory {
            if let Some(number) = legacy_number(id, connection.provider) {
                *hints
                    .entry((provider_key(connection.provider), number))
                    .or_default() += 1;
            }
        }
        for (id, connection) in inventory {
            if let Some(assignment) = self.assignments.get(id) {
                if assignment.provider != connection.provider {
                    return Err("The saved connection display provider does not match.");
                }
                continue;
            }
            let key = provider_key(connection.provider);
            let preserved = legacy_number(id, connection.provider).filter(|number| {
                hints.get(&(key, *number)) == Some(&1)
                    && !self.assignments.values().any(|saved| {
                        saved.provider == connection.provider && saved.number == *number
                    })
            });
            if let Some(number) = preserved {
                self.assignments.insert(
                    id.clone(),
                    Assignment {
                        provider: connection.provider,
                        number,
                    },
                );
                let counter = self.counters.entry(key.to_owned()).or_default();
                *counter = (*counter).max(number);
            }
        }
        for (id, connection) in inventory {
            if self.assignments.contains_key(id) {
                continue;
            }
            let key = provider_key(connection.provider);
            let cursor = cursors.entry(key.to_owned()).or_default();
            let number =
                loop {
                    *cursor = cursor
                        .checked_add(1)
                        .ok_or("The connection display number limit was reached.")?;
                    if !self.assignments.values().any(|saved| {
                        saved.provider == connection.provider && saved.number == *cursor
                    }) {
                        break *cursor;
                    }
                };
            self.assignments.insert(
                id.clone(),
                Assignment {
                    provider: connection.provider,
                    number,
                },
            );
            let counter = self.counters.entry(key.to_owned()).or_default();
            *counter = (*counter).max(number);
        }
        self.validate()
    }

    pub(super) fn public(
        &self,
        profiles: &ConnectionProfiles,
    ) -> Result<BTreeMap<String, u32>, &'static str> {
        self.validate()?;
        profiles
            .profiles
            .iter()
            .map(|(id, connection)| {
                let assignment = self
                    .assignments
                    .get(id)
                    .filter(|assignment| assignment.provider == connection.provider)
                    .ok_or("The saved connection display number is missing.")?;
                Ok((id.clone(), assignment.number))
            })
            .collect()
    }
}

fn legacy_number(id: &str, provider: SystemOneProvider) -> Option<u32> {
    let suffix = id.strip_prefix(provider_key(provider))?.strip_prefix('-')?;
    let (number, identity) = suffix.split_once('-')?;
    (!identity.is_empty()).then_some(())?;
    number.parse::<u32>().ok().filter(|number| *number > 0)
}

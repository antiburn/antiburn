//! A non-evicting identity summary. False positives remove links, never create them.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const FILTER_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "Vec<u8>", into = "Vec<u8>")]
pub(crate) struct IdentityFilter {
    bits: Vec<u8>,
}

impl TryFrom<Vec<u8>> for IdentityFilter {
    type Error = &'static str;

    fn try_from(bits: Vec<u8>) -> Result<Self, Self::Error> {
        if bits.len() != FILTER_BYTES {
            return Err("invalid native identity filter length");
        }
        Ok(Self { bits })
    }
}

impl From<IdentityFilter> for Vec<u8> {
    fn from(filter: IdentityFilter) -> Self {
        filter.bits
    }
}

impl Default for IdentityFilter {
    fn default() -> Self {
        Self {
            bits: vec![0; FILTER_BYTES],
        }
    }
}

impl IdentityFilter {
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.positions(id)
            .all(|index| self.bits[index / 8] & (1 << (index % 8)) != 0)
    }

    pub(crate) fn insert(&mut self, id: &str) {
        let positions: Vec<_> = self.positions(id).collect();
        for index in positions {
            self.bits[index / 8] |= 1 << (index % 8);
        }
    }

    fn positions(&self, id: &str) -> impl Iterator<Item = usize> {
        let digest = Sha256::digest(id.as_bytes());
        let bits = self.bits.len() * 8;
        digest
            .chunks_exact(4)
            .map(move |chunk| {
                let number = u32::from_le_bytes(chunk.try_into().expect("four digest bytes"));
                number as usize % bits
            })
            .collect::<Vec<_>>()
            .into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_identities_survive_large_streams_and_restart() {
        let mut filter = IdentityFilter::default();
        for index in 0..100_000 {
            filter.insert(&format!("call-{index}"));
        }
        let bytes = postcard::to_allocvec(&filter).unwrap();
        assert!(bytes.len() < FILTER_BYTES + 16);
        let restored: IdentityFilter = postcard::from_bytes(&bytes).unwrap();
        for index in 0..100_000 {
            assert!(restored.contains(&format!("call-{index}")));
        }
    }
}

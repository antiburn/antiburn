use super::*;

pub(super) const RESPONSE_CACHE_KEY: &str = "internal:burnCheckResponseCacheV1";
const MAX_RESPONSE_CACHE_ENTRIES: usize = 128;
const MAX_RESPONSE_CACHE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedAssessmentResponse {
    pub provider: String,
    pub request_digest: String,
    pub returned_model: String,
    pub response_json: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub created_at_epoch: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ResponseCache {
    entries: Vec<CachedAssessmentResponse>,
}

impl Store {
    /// Atomically settle usage and cache a successful typed response.
    pub fn record_burn_check_response(
        &self,
        reservation_id: &str,
        entry: CachedAssessmentResponse,
    ) -> anyhow::Result<()> {
        validate_cache_entry(&entry)?;
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let raw_usage = internal_value_in(&transaction, USAGE_LEDGER_KEY)?;
        let mut ledger = raw_usage
            .as_deref()
            .map(serde_json::from_str::<UsageLedger>)
            .transpose()?
            .unwrap_or_default();
        ledger
            .reservations
            .retain(|reservation| reservation.expires_at_epoch > entry.created_at_epoch);
        let Some(reservation) = ledger
            .reservations
            .iter_mut()
            .find(|reservation| reservation.id == reservation_id)
        else {
            anyhow::bail!("Burn Check usage reservation is missing");
        };
        reservation.input_tokens = entry.input_tokens;

        let raw_cache = internal_value_in(&transaction, RESPONSE_CACHE_KEY)?;
        let mut cache = raw_cache
            .as_deref()
            .map(serde_json::from_str::<ResponseCache>)
            .transpose()?
            .unwrap_or_default();
        insert_cache_entry(&mut cache, entry)?;
        write_json_setting(&transaction, USAGE_LEDGER_KEY, &ledger)?;
        write_json_setting(&transaction, RESPONSE_CACHE_KEY, &cache)?;
        transaction.commit()?;
        Ok(())
    }

    /// Read an exact-payload response from the bounded shared cache.
    pub fn cached_assessment_response(
        &self,
        provider: &str,
        request_digest: &str,
        now_epoch: i64,
    ) -> anyhow::Result<Option<CachedAssessmentResponse>> {
        let connection = self.lock();
        let raw = internal_value_in(&connection, RESPONSE_CACHE_KEY)?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let mut cache: ResponseCache = serde_json::from_str(&raw)?;
        cache
            .entries
            .retain(|entry| entry.created_at_epoch > now_epoch.saturating_sub(7 * 24 * 60 * 60));
        Ok(cache
            .entries
            .into_iter()
            .find(|entry| entry.provider == provider && entry.request_digest == request_digest))
    }

    /// Cache only a typed response for its exact request digest.
    pub fn cache_assessment_response(&self, entry: CachedAssessmentResponse) -> anyhow::Result<()> {
        validate_cache_entry(&entry)?;
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let raw = internal_value_in(&transaction, RESPONSE_CACHE_KEY)?;
        let mut cache = raw
            .as_deref()
            .map(serde_json::from_str::<ResponseCache>)
            .transpose()?
            .unwrap_or_default();
        insert_cache_entry(&mut cache, entry)?;
        write_json_setting(&transaction, RESPONSE_CACHE_KEY, &cache)?;
        transaction.commit()?;
        Ok(())
    }
}

fn validate_cache_entry(entry: &CachedAssessmentResponse) -> anyhow::Result<()> {
    if entry.provider.is_empty()
        || entry.request_digest.is_empty()
        || entry.returned_model.is_empty()
        || entry.response_json.len() > 64 * 1024
        || serde_json::from_str::<Value>(&entry.response_json).is_err()
    {
        anyhow::bail!("Burn Check response cache entry is invalid");
    }
    Ok(())
}

fn insert_cache_entry(
    cache: &mut ResponseCache,
    entry: CachedAssessmentResponse,
) -> anyhow::Result<()> {
    cache.entries.retain(|cached| {
        cached.provider != entry.provider || cached.request_digest != entry.request_digest
    });
    cache.entries.push(entry);
    cache.entries.sort_by_key(|cached| cached.created_at_epoch);
    while cache.entries.len() > MAX_RESPONSE_CACHE_ENTRIES
        || serde_json::to_vec(cache)
            .map(|bytes| bytes.len() > MAX_RESPONSE_CACHE_BYTES)
            .unwrap_or(true)
    {
        if cache.entries.is_empty() {
            break;
        }
        cache.entries.remove(0);
    }
    Ok(())
}

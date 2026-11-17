use crate::contract::{digest, safe_id};
use crate::*;
use std::collections::BTreeSet;

pub(crate) fn stream_spec(
    index: usize,
    count: usize,
) -> (&'static str, &'static str, &'static str) {
    if index == 0 {
        ("transcript", "native_transcript", "claude-jsonl")
    } else if index + 1 == count {
        (
            "capture-metadata",
            "capture_metadata",
            "antiburn-capture-metadata-v1",
        )
    } else {
        (
            "native-companion",
            "native_companion",
            "claude-agent-meta-json",
        )
    }
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn validate_manifest(manifest: &CaptureManifest, limits: &Limits) -> Result<()> {
    if manifest.capture_version != CAPTURE_VERSION
        || manifest.compression != "gzip-mtime0-os255-level6"
        || manifest.chunk_expanded_bytes != CHUNK_BYTES
        || !valid_hash(&manifest.metadata_fingerprint)
        || !(2..=3).contains(&manifest.streams.len())
        || manifest.source_fingerprint != manifest.calculate_source_fingerprint()?
    {
        return Err(Error::CorruptSpool);
    }
    let mut total = 0u64;
    for (index, stream) in manifest.streams.iter().enumerate() {
        let (id, role, schema) = stream_spec(index, manifest.streams.len());
        let maximum = if index == 0 {
            limits.source_bytes
        } else {
            METADATA_BYTES
        };
        if stream.stream_id != id
            || stream.role != role
            || stream.schema != schema
            || stream.expanded_bytes > maximum
            || !valid_hash(&stream.expanded_sha256)
            || stream.chunks.len() > 128
            || (index + 1 == manifest.streams.len() && stream.expanded_bytes == 0)
        {
            return Err(Error::CorruptSpool);
        }
        validate_stream(stream)?;
        total = total
            .checked_add(stream.expanded_bytes)
            .ok_or(Error::CorruptSpool)?;
    }
    if total > limits.capture_bytes {
        return Err(Error::CorruptSpool);
    }
    Ok(())
}

fn validate_stream(stream: &CapturedStream) -> Result<()> {
    if stream.expanded_bytes == 0 {
        return if stream.chunks.is_empty() && stream.expanded_sha256 == digest(b"") {
            Ok(())
        } else {
            Err(Error::CorruptSpool)
        };
    }
    if stream.chunks.is_empty() {
        return Err(Error::CorruptSpool);
    }
    let mut offset = 0u64;
    for (index, chunk) in stream.chunks.iter().enumerate() {
        if chunk.index as usize != index
            || chunk.expanded_offset != offset
            || chunk.expanded_bytes == 0
            || chunk.expanded_bytes > CHUNK_BYTES
            || (index + 1 != stream.chunks.len() && chunk.expanded_bytes != CHUNK_BYTES)
            || chunk.compressed_bytes == 0
            || chunk.compressed_bytes > CHUNK_BYTES + CHUNK_BYTES / 100 + 1024
            || !valid_hash(&chunk.expanded_sha256)
            || !valid_hash(&chunk.compressed_sha256)
        {
            return Err(Error::CorruptSpool);
        }
        offset = offset
            .checked_add(chunk.expanded_bytes)
            .ok_or(Error::CorruptSpool)?;
    }
    if offset != stream.expanded_bytes {
        return Err(Error::CorruptSpool);
    }
    Ok(())
}

pub(crate) fn validate_plan(plan: &UploadPlan, manifest: &CaptureManifest) -> Result<()> {
    if plan.identity != manifest.identity
        || plan.manifest_sha256 != manifest.sha256()?
        || !safe_id(&plan.plan_id)
        || plan.chunks.len() != manifest.chunks().count()
    {
        return Err(Error::InvalidServerStatus);
    }
    let mut transfer = 0u64;
    let mut reused = 0u64;
    for (planned, (key, chunk)) in plan.chunks.iter().zip(manifest.chunks()) {
        if planned.key != key || planned.compressed_sha256 != chunk.compressed_sha256 {
            return Err(Error::InvalidServerStatus);
        }
        match planned.disposition {
            ChunkDisposition::Upload => transfer += chunk.compressed_bytes,
            ChunkDisposition::Reuse => reused += chunk.compressed_bytes,
        }
    }
    if transfer != plan.required_transfer_bytes || reused != plan.reused_bytes {
        return Err(Error::InvalidServerStatus);
    }
    Ok(())
}

pub(crate) fn validate_status(
    status: &ServerStatus,
    manifest: &CaptureManifest,
    plan: &UploadPlan,
) -> Result<BTreeSet<ChunkKey>> {
    if status.identity != manifest.identity
        || status.manifest_sha256 != manifest.sha256()?
        || status.plan_id != plan.plan_id
        || status.uploaded_parts.len() > plan.chunks.len()
    {
        return Err(Error::InvalidServerStatus);
    }
    let mut confirmed = BTreeSet::new();
    for receipt in &status.uploaded_parts {
        let decision = plan
            .chunks
            .iter()
            .find(|part| part.key == receipt.key)
            .ok_or(Error::InvalidServerStatus)?;
        if decision.disposition != ChunkDisposition::Upload
            || decision.compressed_sha256 != receipt.compressed_sha256
            || !confirmed.insert(receipt.key.clone())
        {
            return Err(Error::InvalidServerStatus);
        }
    }
    let required_count = plan
        .chunks
        .iter()
        .filter(|chunk| chunk.disposition == ChunkDisposition::Upload)
        .count();
    if status.integrity_accepted && confirmed.len() != required_count {
        return Err(Error::InvalidServerStatus);
    }
    Ok(confirmed)
}

pub(crate) fn validate_status_progress(
    previous: Option<&ServerStatus>,
    current: &ServerStatus,
) -> Result<()> {
    let Some(previous) = previous else {
        return Ok(());
    };
    if (previous.integrity_accepted && !current.integrity_accepted)
        || previous
            .uploaded_parts
            .iter()
            .any(|part| !current.uploaded_parts.contains(part))
    {
        return Err(Error::InvalidServerStatus);
    }
    Ok(())
}

pub(crate) fn entry_progress(entry: &QueueEntry) -> Result<TransferProgress> {
    let Some(manifest) = &entry.manifest else {
        return Ok(TransferProgress::default());
    };
    let mut progress = TransferProgress {
        total_compressed_bytes: manifest
            .chunks()
            .map(|(_, chunk)| chunk.compressed_bytes)
            .sum(),
        ..TransferProgress::default()
    };
    let Some(plan) = &entry.upload_plan else {
        if entry.server_status.is_some() {
            return Err(Error::CorruptSpool);
        }
        return Ok(progress);
    };
    validate_plan(plan, manifest)?;
    progress.negotiated = true;
    progress.required_transfer_bytes = plan.required_transfer_bytes;
    progress.reused_bytes = plan.reused_bytes;
    if let Some(status) = &entry.server_status {
        for key in validate_status(status, manifest, plan)? {
            progress.confirmed_upload_bytes += manifest
                .chunk(&key)
                .ok_or(Error::CorruptSpool)?
                .compressed_bytes;
        }
    }
    Ok(progress)
}

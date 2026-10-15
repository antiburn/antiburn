use super::{Decision, ExplanationBasis, ReadRelationship, Reason};
use crate::analysis::jev::JevEvidenceRole;
use crate::analysis::jev_evidence::{JevReadResultKind, JevReadStatus};
use serde_json::Value;

impl Decision {
    pub fn task_evidence_is_bound_to(
        &self,
        evidence: &[crate::analysis::jev::JevEvidenceReference],
    ) -> bool {
        self.has_valid_explanation()
            && !self.task_evidence.is_empty()
            && self
                .task_evidence
                .iter()
                .all(|binding| evidence.contains(binding))
    }

    pub fn selected_source_snippets(
        &self,
    ) -> Option<impl Iterator<Item = &super::ReadSourceSnippet>> {
        if !self.has_valid_explanation() {
            return None;
        }
        let basis = self.explanation.as_ref()?;
        Some(basis.snippets.iter().chain(basis.purpose.iter()))
    }

    pub fn has_valid_explanation(&self) -> bool {
        let Some(basis) = &self.explanation else {
            return false;
        };
        if basis.version != 3
            || basis.reads.len() != self.reads.len()
            || basis.reads.is_empty()
            || basis.task.evidence != self.task_evidence
            || !super::episodes::task_context_is_complete(&basis.task)
            || !purpose_is_bound(basis)
            || basis.reads.iter().zip(&self.reads).any(|(read, binding)| {
                read.request.reference_id != binding.request_id
                    || read.request.truncated
                    || read.request.paths.len() != 1
                    || read
                        .request
                        .paths
                        .iter()
                        .any(|path| path.is_empty() || path.len() > 192)
                    || read.result.as_ref().is_none_or(|result| {
                        Some(result.reference_id.as_str()) != binding.result_id.as_deref()
                            || Some(&result.recorded_output_digest)
                                != binding.output_digest.as_ref()
                            || result.request_reference_id.as_deref()
                                != Some(binding.request_id.as_str())
                            || result.status != JevReadStatus::Success
                            || result.kind != JevReadResultKind::File
                            || result.truncated
                    })
            })
        {
            return false;
        }
        let first = &basis.reads[0];
        let result = first.result.as_ref().expect("validated result");
        match self.reason {
            Reason::ExcessiveWithinFileReading => {
                let redundant = basis
                    .reads
                    .iter()
                    .skip(1)
                    .filter_map(|read| read.result.as_ref())
                    .fold(0_u64, |sum, result| {
                        sum.saturating_add(result.recorded_output_bytes)
                    });
                basis.relationship == ReadRelationship::LaterReadRepeatsEarlier
                    && basis.whole_output_equal == Some(true)
                    && basis.reads.len() >= 2
                    && redundant >= 8192
                    && super::has_known_returned_extent(result)
                    && (first.request.cwd.is_some()
                        || super::is_absolute_source_path(&first.request.paths[0]))
                    && basis.reads.iter().all(|read| {
                        read.request.paths == first.request.paths
                            && read.request.cwd == first.request.cwd
                            && read.request.extent == first.request.extent
                            && read.request.native_extent == first.request.native_extent
                            && read.result.as_ref().is_some_and(|later| {
                                later.returned_extent == result.returned_extent
                                    && later.recorded_output_digest == result.recorded_output_digest
                            })
                    })
                    && basis.compared.fields["repeat"]["count"].as_u64()
                        == u64::try_from(basis.reads.len()).ok()
                    && basis.compared.fields["repeat"]["redundant_output_bytes"] == redundant
                    && basis.compared.fields["repeat"]["path"] == first.request.paths[0]
                    && basis.compared.fields["repeat"]["extent"]
                        == serde_json::json!(result.returned_extent)
                    && basis.compared.fields["repeat"]["complete_outputs_equal"] == true
                    && basis.compared.fields["repeat"]["earlier_result_available"] == true
                    && basis.compared.fields["repeat"]["complete_successful_file_results"] == true
                    && basis.compared.fields["repeat"]["intervening_work"] == false
                    && (basis.compared.fields["repeat"].get("sample_only").is_none()
                        || basis.compared.fields["repeat"]["sample_only"] == true)
                    && basis.compared.fields["repeat"]["starts_at_file_beginning"]
                        == super::episodes::read_starts_at_file_beginning(result)
                    && source_sample_is_bound(
                        basis,
                        "/repeat/header_sample",
                        "repeat.header_sample.text",
                        &result.reference_id,
                        false,
                    )
            }
            Reason::UnrelatedFiles => {
                basis.relationship == ReadRelationship::SelectedReadUnrelated
                    && basis.reads.len() == 1
                    && result.recorded_output_bytes >= 8192
                    && basis.whole_output_equal.is_none()
                    && basis.compared.fields["read"]["path"] == first.request.paths[0]
                    && basis.compared.fields["read"]["returned_bytes"]
                        == result.recorded_output_bytes
                    && basis.compared.fields["read"]["sample_only"] == true
                    && basis.compared.fields["read"]["starts_at_file_beginning"]
                        == super::episodes::read_starts_at_file_beginning(result)
                    && source_sample_is_bound(
                        basis,
                        "/read/sample",
                        "read.sample.text",
                        &result.reference_id,
                        true,
                    )
            }
            Reason::ExcessiveFileBreadth => false,
        }
    }
}

fn purpose_is_bound(basis: &ExplanationBasis) -> bool {
    let purpose = &basis.compared.fields["purpose"];
    let references = basis
        .compared
        .evidence
        .iter()
        .filter(|reference| reference.part_id == "purpose.text")
        .collect::<Vec<_>>();
    match (
        purpose.is_null(),
        basis.purpose.as_ref(),
        references.as_slice(),
    ) {
        (true, None, []) => true,
        (false, Some(snippet), [reference]) => {
            let text = purpose["text"].as_str();
            let range = purpose["range"].as_array();
            text.is_some_and(|text| {
                !text.trim().is_empty()
                    && text.len() <= 192
                    && snippet.text == text
                    && snippet.range == (0, text.len())
                    && snippet.reference.id == reference.source_id
                    && snippet.kind == reference.content_kind
                    && reference.role == JevEvidenceRole::SupportingContext
                    && snippet.reference.stable
                    && !snippet.partial
                    && !snippet.source_digest.is_empty()
                    && range.is_some_and(|range| {
                        range.len() == 2 && range[0] == 0 && range[1] == text.len()
                    })
            })
        }
        _ => false,
    }
}

fn source_sample_is_bound(
    basis: &ExplanationBasis,
    field_pointer: &str,
    part_id: &str,
    result_id: &str,
    required: bool,
) -> bool {
    let references = basis
        .compared
        .evidence
        .iter()
        .filter(|reference| reference.part_id == part_id)
        .collect::<Vec<_>>();
    let Some(sample) = basis.compared.fields.pointer(field_pointer) else {
        return !required && basis.snippets.is_empty() && references.is_empty();
    };
    if sample.is_null() {
        return !required && basis.snippets.is_empty() && references.is_empty();
    }
    let [reference] = references.as_slice() else {
        return false;
    };
    let Some(snippet) = basis.snippets.first() else {
        return false;
    };
    let Some(text) = sample["text"].as_str() else {
        return false;
    };
    let Some(range) = sample["range"].as_array() else {
        return false;
    };
    let Some(start) = range.first().and_then(Value::as_u64) else {
        return false;
    };
    let Some(end) = range.get(1).and_then(Value::as_u64) else {
        return false;
    };
    let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
        return false;
    };
    basis.snippets.len() == 1
        && reference.source_id == result_id
        && reference.content_kind == "tool_result"
        && reference.role == JevEvidenceRole::Candidate
        && snippet.reference.id == result_id
        && snippet.reference.stable
        && snippet.range == (start, end)
        && snippet.text == text
        && text.len() <= 512
        && text.chars().any(char::is_alphabetic)
        && start < end
        && end - start == text.len()
        && !snippet.source_digest.is_empty()
        && snippet.partial
}

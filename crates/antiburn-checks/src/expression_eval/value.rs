use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use super::{EvaluationBudget, EvaluationError, EvaluationValue, PathSegment};

/// The source evidence selected by an expression.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct EvaluationTrace {
    pub dependencies: Vec<EvaluationDependency>,
}

/// A selected source field or a collection's membership.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvaluationDependency {
    pub path: Vec<PathSegment>,
    pub kind: EvaluationDependencyKind,
}

/// Membership does not select unused fields inside collection items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum EvaluationDependencyKind {
    Value,
    Membership,
}

impl EvaluationTrace {
    /// Merges dependencies without changing their first-read order.
    pub fn merge(&mut self, other: &Self) {
        for dependency in &other.dependencies {
            if !self.dependencies.contains(dependency) {
                self.dependencies.push(dependency.clone());
            }
        }
    }

    fn add(&mut self, path: Vec<PathSegment>, kind: EvaluationDependencyKind) {
        self.merge(&Self {
            dependencies: vec![EvaluationDependency { path, kind }],
        });
    }
}

/// Available data, selected evidence, and private frozen-value identity.
#[derive(Clone, Debug, PartialEq)]
pub struct EvaluatedValue {
    pub value: EvaluationValue,
    pub trace: EvaluationTrace,
    pub(super) metadata: ValueMetadata,
}

/// Local map aliases retain the selected value's source identity.
pub type EvaluationBindings = BTreeMap<String, EvaluatedValue>;

impl EvaluatedValue {
    /// Creates an authored or generated JSON value without source evidence.
    #[must_use]
    pub fn available(value: Value) -> Self {
        let metadata = ValueMetadata::generated(&value);
        Self {
            value: EvaluationValue::Available(value),
            trace: EvaluationTrace::default(),
            metadata,
        }
    }

    /// Selects an item for a map alias and charges its iteration visit.
    pub fn array_item(
        &self,
        index: usize,
        budget: &mut EvaluationBudget,
    ) -> Result<Self, EvaluationError> {
        budget.visit_items(1)?;
        let EvaluationValue::Available(Value::Array(items)) = &self.value else {
            return Ok(Self::unavailable(self.membership_trace()));
        };
        let Some(item) = items.get(index) else {
            return Ok(Self::unavailable(self.membership_trace()));
        };
        let mut metadata = self.metadata.child(&PathSegment::Index(index));
        metadata.dependencies.merge(&self.membership_trace());
        let value = super::copy_value(item, budget, 1)?;
        let trace = metadata.trace();
        Ok(Self {
            value: EvaluationValue::Available(value),
            trace,
            metadata,
        })
    }

    /// Returns membership evidence without selecting every item field.
    #[must_use]
    pub fn membership_trace(&self) -> EvaluationTrace {
        self.metadata.membership_trace()
    }

    /// Retains dependencies when an evaluated part contributes to generated text.
    pub fn merge_text_provenance(&mut self, source: &Self) {
        self.trace.merge(&source.trace);
        self.metadata.dependencies.merge(&source.trace);
        self.metadata
            .text_sources
            .merge(&source.metadata.text_source_trace());
        self.metadata.encoded_provenance = None;
    }

    pub(super) fn unavailable(trace: EvaluationTrace) -> Self {
        Self {
            value: EvaluationValue::Unavailable,
            metadata: ValueMetadata::derived(trace.clone()),
            trace,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum ValueIdentity {
    Frozen(Vec<PathSegment>),
    Generated {
        root: Arc<()>,
        path: Vec<PathSegment>,
    },
}

impl PartialEq for ValueIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.same(other)
    }
}

impl ValueIdentity {
    fn child(&self, segment: &PathSegment) -> Self {
        match self {
            Self::Frozen(path) => Self::Frozen(append(path, segment)),
            Self::Generated { root, path } => Self::Generated {
                root: root.clone(),
                path: append(path, segment),
            },
        }
    }

    pub(super) fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Frozen(left), Self::Frozen(right)) => left == right,
            (
                Self::Generated {
                    root: left,
                    path: left_path,
                },
                Self::Generated {
                    root: right,
                    path: right_path,
                },
            ) => Arc::ptr_eq(left, right) && left_path == right_path,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ValueProvenance {
    origin: Option<Vec<PathSegment>>,
    items: Vec<Self>,
    text_sources: EvaluationTrace,
    membership_sources: EvaluationTrace,
    encoded_provenance: Option<Box<Self>>,
}

impl ValueProvenance {
    pub fn child(&self, segment: &PathSegment) -> Self {
        if let PathSegment::Index(index) = segment
            && let Some(item) = self.items.get(*index)
        {
            let mut child = item.clone();
            child.membership_sources.merge(&self.membership_sources);
            return child;
        }
        Self {
            origin: self.origin.as_ref().map(|path| append(path, segment)),
            membership_sources: self.membership_sources.clone(),
            ..Self::default()
        }
    }

    pub fn collect_sources(&self, value: &Value, sources: &mut EvaluationTrace) {
        sources.merge(&self.text_sources);
        sources.merge(&self.membership_sources);
        if matches!(value, Value::Array(_) | Value::Object(_))
            && let Some(origin) = &self.origin
        {
            sources.add(origin.clone(), EvaluationDependencyKind::Membership);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ValueMetadata {
    pub identity: Option<ValueIdentity>,
    pub origin: Option<Vec<PathSegment>>,
    pub dependencies: EvaluationTrace,
    pub items: Vec<Self>,
    /// JSON conversion keeps source shape for membership, not value identity.
    pub provenance: Option<ValueProvenance>,
    pub encoded_provenance: Option<Box<ValueProvenance>>,
    pub text_sources: EvaluationTrace,
    pub membership_sources: EvaluationTrace,
}

impl ValueMetadata {
    pub fn generated(value: &Value) -> Self {
        let identity =
            matches!(value, Value::Array(_) | Value::Object(_)).then(|| ValueIdentity::Generated {
                root: Arc::new(()),
                path: Vec::new(),
            });
        Self {
            identity,
            ..Self::default()
        }
    }

    pub fn frozen(path: Vec<PathSegment>) -> Self {
        Self {
            identity: Some(ValueIdentity::Frozen(path.clone())),
            origin: Some(path),
            ..Self::default()
        }
    }

    pub fn derived(dependencies: EvaluationTrace) -> Self {
        Self {
            dependencies,
            ..Self::default()
        }
    }

    pub fn child(&self, segment: &PathSegment) -> Self {
        if let PathSegment::Index(index) = segment
            && let Some(item) = self.items.get(*index)
        {
            let mut metadata = item.clone();
            metadata.dependencies.merge(&self.dependencies);
            metadata.membership_sources.merge(&self.membership_sources);
            return metadata;
        }
        let provenance = self
            .provenance
            .as_ref()
            .map(|provenance| provenance.child(segment));
        let mut metadata = Self {
            identity: self
                .identity
                .as_ref()
                .map(|identity| identity.child(segment)),
            origin: self.origin.as_ref().map(|path| append(path, segment)),
            dependencies: self.dependencies.clone(),
            membership_sources: self.membership_sources.clone(),
            ..Self::default()
        };
        if let Some(provenance) = provenance {
            metadata.restore_provenance(provenance);
            metadata.membership_sources.merge(&self.membership_sources);
        }
        metadata
    }

    pub fn trace(&self) -> EvaluationTrace {
        let mut trace = self.dependencies.clone();
        if let Some(origin) = &self.origin {
            trace.add(origin.clone(), EvaluationDependencyKind::Value);
        }
        for item in &self.items {
            trace.merge(&item.trace());
        }
        trace
    }

    pub fn membership_trace(&self) -> EvaluationTrace {
        let mut trace = self.dependencies.clone();
        trace.merge(&self.membership_sources);
        if let Some(origin) = self.origin.as_ref().or_else(|| {
            self.provenance
                .as_ref()
                .and_then(|provenance| provenance.origin.as_ref())
        }) {
            trace.add(origin.clone(), EvaluationDependencyKind::Membership);
        }
        trace
    }

    pub fn source_provenance(&self) -> ValueProvenance {
        let mut provenance = self.provenance.clone().unwrap_or_else(|| ValueProvenance {
            origin: self.origin.clone(),
            items: self.items.iter().map(Self::source_provenance).collect(),
            ..ValueProvenance::default()
        });
        provenance.text_sources = self.text_sources.clone();
        provenance.membership_sources = self.membership_sources.clone();
        provenance.encoded_provenance = self.encoded_provenance.clone();
        provenance
    }

    pub fn restore_provenance(&mut self, provenance: ValueProvenance) {
        self.text_sources = provenance.text_sources.clone();
        self.membership_sources = provenance.membership_sources.clone();
        self.encoded_provenance = provenance.encoded_provenance.clone();
        self.provenance = Some(provenance);
    }

    pub fn text_source_trace(&self) -> EvaluationTrace {
        let mut sources = self.text_sources.clone();
        sources.merge(&self.membership_sources);
        sources
    }
}

pub(super) struct RuntimeValue<'a> {
    pub value: Option<Cow<'a, Value>>,
    pub metadata: ValueMetadata,
}

impl<'a> RuntimeValue<'a> {
    pub fn owned(value: Value, dependencies: EvaluationTrace) -> Self {
        let mut metadata = ValueMetadata::generated(&value);
        metadata.dependencies = dependencies;
        Self {
            value: Some(Cow::Owned(value)),
            metadata,
        }
    }

    pub fn unavailable(metadata: ValueMetadata) -> Self {
        Self {
            value: None,
            metadata,
        }
    }

    pub fn trace(&self) -> EvaluationTrace {
        self.metadata.trace()
    }

    pub fn finish(
        self,
        budget: &mut EvaluationBudget,
        depth: usize,
    ) -> Result<EvaluatedValue, EvaluationError> {
        let trace = self.trace();
        let value = match self.value {
            Some(Cow::Borrowed(value)) => {
                EvaluationValue::Available(super::copy_value(value, budget, depth)?)
            }
            Some(Cow::Owned(value)) => EvaluationValue::Available(value),
            None => EvaluationValue::Unavailable,
        };
        Ok(EvaluatedValue {
            value,
            trace,
            metadata: self.metadata,
        })
    }
}

fn append(path: &[PathSegment], segment: &PathSegment) -> Vec<PathSegment> {
    let mut path = path.to_vec();
    path.push(segment.clone());
    path
}

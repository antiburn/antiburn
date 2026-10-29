use super::{CompiledCheck, Condition, InputTemplate, StatusResultTemplate, TemplatePart};
use crate::expression::{AccessSegment, Expression, ExpressionLiteral, PathSegment};

/// Capture points for current files. Neither flag requires complete inventories.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CurrentInventoryDemand {
    /// The pre-selection condition reads current inventories.
    pub condition: bool,
    /// Decision inputs or status actions read current inventories.
    pub candidate: bool,
}

pub(super) fn inspect(check: &CompiledCheck) -> CurrentInventoryDemand {
    CurrentInventoryDemand {
        condition: condition_reads(check.condition.as_ref(), &[]),
        candidate: check.decisions.iter().any(|decision| template_reads(&decision.input, &mut Vec::new()))
            || check.actions.iter().any(|action| {
                condition_reads(action.condition.as_ref(), &[])
                    || matches!(&action.result, StatusResultTemplate::Expression(value) if expression_reads(value, &[]))
                    || action.message.as_ref().is_some_and(|value| template_reads(value, &mut Vec::new()))
            }),
    }
}

fn condition_reads(condition: Option<&Condition>, aliases: &[&str]) -> bool {
    matches!(condition, Some(Condition::Expression(value)) if expression_reads(value, aliases))
}

fn template_reads<'a>(template: &'a InputTemplate, aliases: &mut Vec<&'a str>) -> bool {
    match template {
        InputTemplate::Literal(_) => false,
        InputTemplate::Expression(value) => expression_reads(value, aliases),
        InputTemplate::Text(parts) => parts.iter().any(|part| matches!(part, TemplatePart::Expression(value) if expression_reads(value, aliases))),
        InputTemplate::Sequence(items) => items.iter().any(|item| template_reads(item, aliases)),
        InputTemplate::Object(items) => items.iter().any(|(_, item)| template_reads(item, aliases)),
        InputTemplate::Map { collection, alias, condition, value } => {
            let collection_reads = expression_reads(collection, aliases);
            aliases.push(alias);
            let body_reads = condition_reads(condition.as_ref(), aliases) || template_reads(value, aliases);
            aliases.pop();
            collection_reads || body_reads
        }
    }
}

enum ScopeMember<'a> {
    Whole,
    Named(&'a str),
    Unknown,
}

fn scope_path<'a>(
    expression: &'a Expression,
    aliases: &[&str],
) -> Option<(&'a str, ScopeMember<'a>)> {
    match expression {
        Expression::Path(path) => {
            let (PathSegment::Property(root), rest) = path.split_first()? else {
                return None;
            };
            if aliases.contains(&root.as_str()) || !matches!(root.as_str(), "global" | "repo") {
                return None;
            }
            let first = match rest.first() {
                Some(PathSegment::Property(property)) => ScopeMember::Named(property),
                None => ScopeMember::Whole,
                _ => ScopeMember::Unknown,
            };
            Some((root, first))
        }
        Expression::Access { base, segments } => {
            let (root, property) = scope_path(base, aliases)?;
            if !matches!(property, ScopeMember::Whole) {
                return Some((root, property));
            }
            let property = match segments.first() {
                Some(AccessSegment::Property(property)) => ScopeMember::Named(property),
                Some(AccessSegment::Dynamic(value)) => match value.as_ref() {
                    Expression::Literal(ExpressionLiteral::String(property)) => {
                        ScopeMember::Named(property)
                    }
                    _ => ScopeMember::Unknown,
                },
                _ => ScopeMember::Unknown,
            };
            Some((root, property))
        }
        _ => None,
    }
}

fn expression_reads(expression: &Expression, aliases: &[&str]) -> bool {
    if let Some((root, property)) = scope_path(expression, aliases) {
        let dynamic_reads = match expression {
            Expression::Access { base, segments } => access_keys_read(base, aliases) || segments.iter().any(|segment| matches!(segment, AccessSegment::Dynamic(value) if expression_reads(value, aliases))),
            _ => false,
        };
        return dynamic_reads
            || root == "global"
            || !matches!(property, ScopeMember::Named("root"));
    }
    match expression {
        Expression::Literal(_) | Expression::Path(_) => false,
        Expression::Access { base, segments } => expression_reads(base, aliases) || segments.iter().any(|segment| matches!(segment, AccessSegment::Dynamic(value) if expression_reads(value, aliases))),
        Expression::Call { arguments, .. } => arguments.iter().any(|value| expression_reads(value, aliases)),
        Expression::Not(value) => expression_reads(value, aliases),
        Expression::Binary { left, right, .. } => expression_reads(left, aliases) || expression_reads(right, aliases),
    }
}

fn access_keys_read(expression: &Expression, aliases: &[&str]) -> bool {
    match expression {
        Expression::Access { base, segments } => access_keys_read(base, aliases) || segments.iter().any(|segment| matches!(segment, AccessSegment::Dynamic(value) if expression_reads(value, aliases))),
        _ => false,
    }
}

#[cfg(test)]
mod tests;

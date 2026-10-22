use super::*;

use std::collections::BTreeSet;

#[cfg(test)]
#[path = "source_selection_tests.rs"]
mod tests;

pub(super) fn candidate_id(log: &SessionLog) -> Option<&str> {
    if !log.environment.is_native() || log.agent_type != AgentKind::OpenCode {
        return None;
    }
    let id = match &log.source {
        SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            session_id,
            source_format:
                Some(
                    crate::analysis::SourceFormat::OpenCodeSqliteV1
                    | crate::analysis::SourceFormat::OpenCodeSqliteV2,
                ),
            ..
        } => session_id.as_str(),
        SessionSource::Inline { label, .. } => label.strip_prefix("opencode:")?,
        _ => return None,
    };
    valid_opencode_v2_session_id(id).then_some(id)
}

pub(super) fn select(
    roots: &[PathBuf],
    ids: HashSet<String>,
    cancelled: Arc<AtomicBool>,
) -> Result<HashMap<String, Option<SessionLog>>, &'static str> {
    select_with_path_resolver(roots, ids, cancelled, database_paths_for_root)
}

struct PendingRoot {
    requested: String,
    current: String,
    seen: HashSet<String>,
}

fn select_with_path_resolver(
    roots: &[PathBuf],
    ids: HashSet<String>,
    cancelled: Arc<AtomicBool>,
    paths_for_root: fn(&Path) -> Result<Vec<PathBuf>, &'static str>,
) -> Result<HashMap<String, Option<SessionLog>>, &'static str> {
    let mut ids: Vec<_> = ids.into_iter().collect();
    ids.sort();
    let mut pending: Vec<_> = ids
        .into_iter()
        .map(|id| PendingRoot {
            requested: id.clone(),
            current: id.clone(),
            seen: HashSet::from([id]),
        })
        .collect();
    let mut selected = HashMap::new();
    let mut searched = HashSet::new();
    let mut sources = HashMap::new();
    while !pending.is_empty() {
        if cancelled.load(Ordering::Acquire) {
            return Err("cancelled");
        }
        let unresolved: BTreeSet<_> = pending
            .iter()
            .map(|candidate| &candidate.current)
            .filter(|id| !searched.contains(*id))
            .cloned()
            .collect();
        if !unresolved.is_empty() {
            sources.extend(select_direct_sources(
                roots,
                &unresolved,
                cancelled.clone(),
                paths_for_root,
            )?);
            searched.extend(unresolved);
        }
        pending.retain_mut(|candidate| {
            let Some(source) = sources.get(&candidate.current) else {
                if candidate.current != candidate.requested {
                    selected.insert(candidate.requested.clone(), None);
                }
                return false;
            };
            let Some(log) = source else {
                selected.insert(candidate.requested.clone(), None);
                return false;
            };
            let Some(root_id) = candidate_id(log) else {
                selected.insert(candidate.requested.clone(), None);
                return false;
            };
            if root_id == candidate.current {
                selected.insert(candidate.requested.clone(), Some(log.clone()));
                return false;
            }
            if candidate.seen.len() > MAX_V2_PARENT_DEPTH
                || !candidate.seen.insert(root_id.to_owned())
            {
                selected.insert(candidate.requested.clone(), None);
                return false;
            }
            candidate.current = root_id.to_owned();
            true
        });
    }
    Ok(selected)
}

fn select_direct_sources(
    roots: &[PathBuf],
    ids: &BTreeSet<String>,
    cancelled: Arc<AtomicBool>,
    paths_for_root: fn(&Path) -> Result<Vec<PathBuf>, &'static str>,
) -> Result<HashMap<String, Option<SessionLog>>, &'static str> {
    let mut selected = HashMap::new();
    if let Err(stage) = visit_databases(roots, ids, &cancelled, paths_for_root, &mut selected) {
        if cancelled.load(Ordering::Acquire) {
            return Err("cancelled");
        }
        tracing::warn!(event = "opencode_sqlite_source_selection_failed", stage);
        // An unreadable earlier source prevents safe fallback for unresolved IDs.
        for id in ids {
            selected.entry(id.clone()).or_insert(None);
        }
    }
    Ok(selected)
}

fn visit_databases(
    roots: &[PathBuf],
    ids: &BTreeSet<String>,
    cancelled: &Arc<AtomicBool>,
    paths_for_root: fn(&Path) -> Result<Vec<PathBuf>, &'static str>,
    selected: &mut HashMap<String, Option<SessionLog>>,
) -> Result<(), &'static str> {
    for root in roots {
        if selected.len() == ids.len() {
            break;
        }
        for path in paths_for_root(root)? {
            if selected.len() == ids.len() {
                break;
            }
            if cancelled.load(Ordering::Acquire) {
                return Err("cancelled");
            }
            let connection = match Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            ) {
                Ok(connection) => connection,
                Err(_)
                    if std::fs::metadata(&path)
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
                {
                    continue;
                }
                Err(_) => return Err("source_selection_open"),
            };
            connection
                .busy_timeout(Duration::from_millis(250))
                .map_err(|_| "source_selection_timeout")?;
            let budget = QueryBudget::new(&connection, cancelled.clone());
            let snapshot = connection
                .unchecked_transaction()
                .map_err(|_| "source_selection_snapshot")?;
            let result = select_from_database(&snapshot, &path, ids, selected, &budget);
            if let Err(stage) = result {
                return Err(budget.failure(stage));
            }
        }
    }
    Ok(())
}

fn select_from_database(
    connection: &Connection,
    path: &Path,
    ids: &BTreeSet<String>,
    selected: &mut HashMap<String, Option<SessionLog>>,
    budget: &QueryBudget<'_>,
) -> Result<(), &'static str> {
    budget.check()?;
    let schema = opencode_sqlite_schema(connection).map_err(|_| "source_selection_schema")?;
    if schema == OpenCodeSqliteSchema::Malformed {
        return Err("source_selection_schema");
    }
    if !schema.has_v2() {
        return Ok(());
    }
    let mut query = connection
        .prepare(
            "SELECT CASE WHEN length(CAST(parent_id AS BLOB)) <= 256 THEN parent_id END,
                parent_id IS NOT NULL AND length(CAST(parent_id AS BLOB)) > 256,
                MAX(time_created, time_updated)
           FROM session_v2 WHERE id = ?1",
        )
        .map_err(|_| "source_selection_query")?;
    for id in ids {
        budget.check()?;
        if selected.contains_key(id) {
            continue;
        }
        match select_session_source(&mut query, path, id, budget) {
            Ok(Some(log)) => {
                selected.insert(id.clone(), Some(log));
            }
            Ok(None) => {}
            Err(stage) => {
                budget.check()?;
                tracing::warn!(event = "opencode_sqlite_source_binding_failed", stage);
                selected.insert(id.clone(), None);
            }
        }
    }
    Ok(())
}

fn select_session_source(
    query: &mut rusqlite::Statement<'_>,
    path: &Path,
    id: &str,
    budget: &QueryBudget<'_>,
) -> Result<Option<SessionLog>, &'static str> {
    let mut session_id = id.to_owned();
    let mut seen = HashSet::from([session_id.clone()]);
    for depth in 0..=MAX_V2_PARENT_DEPTH {
        budget.check()?;
        let row = query
            .query_row([&session_id], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .optional()
            .map_err(|_| "source_selection_row")?;
        budget.check()?;
        let Some((parent, oversized, updated)) = row else {
            return if depth == 0 {
                Ok(None)
            } else {
                Err("source_selection_hierarchy")
            };
        };
        if oversized {
            return Err("source_selection_metadata_limit");
        }
        let Some(parent) = parent else {
            return Ok(Some(SessionLog {
                environment: Default::default(),
                agent_type: AgentKind::OpenCode,
                source: SessionSource::ProviderDb {
                    agent: AgentKind::OpenCode,
                    db_path: path.to_path_buf(),
                    session_id,
                    source_format: Some(crate::analysis::SourceFormat::OpenCodeSqliteV2),
                },
                updated_at: normalize_db_epoch(Some(updated)),
            }));
        };
        if !valid_opencode_v2_session_id(&parent) || !seen.insert(parent.clone()) {
            return Err("source_selection_hierarchy");
        }
        session_id = parent;
    }
    Err("source_selection_hierarchy")
}

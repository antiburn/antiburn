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
    valid_session_id(id).then_some(id)
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

#[derive(Clone, Copy)]
enum SelectionTarget {
    Transcript,
    Metadata,
}

fn source_databases(paths: &[PathBuf]) -> Vec<SourceDatabase> {
    paths
        .iter()
        .map(|path| SourceDatabase {
            path: path.clone(),
            state: DatabaseState::Pending,
        })
        .collect()
}

pub(super) fn select_metadata_at_paths(
    paths: &[PathBuf],
    ids: HashSet<String>,
    cancelled: Arc<AtomicBool>,
) -> Result<HashMap<String, Option<SessionLog>>, &'static str> {
    select_direct_sources(
        &mut source_databases(paths),
        &ids.into_iter().collect(),
        cancelled,
        SelectionTarget::Metadata,
    )
}

fn select_with_path_resolver(
    roots: &[PathBuf],
    ids: HashSet<String>,
    cancelled: Arc<AtomicBool>,
    paths_for_root: impl Fn(&Path) -> Result<Vec<PathBuf>, &'static str>,
) -> Result<HashMap<String, Option<SessionLog>>, &'static str> {
    if cancelled.load(Ordering::Acquire) {
        return Err("cancelled");
    }
    let paths = roots
        .iter()
        .flat_map(|root| match paths_for_root(root) {
            Ok(paths) => paths,
            Err(stage) => {
                tracing::warn!(event = "opencode_sqlite_paths_failed", stage);
                Vec::new()
            }
        })
        .collect::<Vec<_>>();
    select_at_paths(&paths, ids, cancelled)
}

pub(super) fn select_at_paths(
    paths: &[PathBuf],
    ids: HashSet<String>,
    cancelled: Arc<AtomicBool>,
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
    let mut databases = source_databases(paths);
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
                &mut databases,
                &unresolved,
                cancelled.clone(),
                SelectionTarget::Transcript,
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
    databases: &mut [SourceDatabase],
    ids: &BTreeSet<String>,
    cancelled: Arc<AtomicBool>,
    target: SelectionTarget,
) -> Result<HashMap<String, Option<SessionLog>>, &'static str> {
    let mut selected = HashMap::new();
    visit_databases(databases, ids, &cancelled, &mut selected, target)?;
    Ok(selected)
}

struct SourceDatabase {
    path: PathBuf,
    state: DatabaseState,
}

enum DatabaseState {
    Pending,
    Ready(Connection, OpenCodeSqliteSchema),
    Failed,
}

fn is_v2_selection(selection: Option<&Option<SessionLog>>) -> bool {
    matches!(
        selection,
        Some(Some(SessionLog {
            source: SessionSource::ProviderDb {
                source_format: Some(crate::analysis::SourceFormat::OpenCodeSqliteV2),
                ..
            },
            ..
        }))
    )
}

fn visit_databases(
    databases: &mut [SourceDatabase],
    ids: &BTreeSet<String>,
    cancelled: &Arc<AtomicBool>,
    selected: &mut HashMap<String, Option<SessionLog>>,
    target: SelectionTarget,
) -> Result<(), &'static str> {
    for database in databases {
        if ids.iter().all(|id| is_v2_selection(selected.get(id))) {
            break;
        }
        if cancelled.load(Ordering::Acquire) {
            return Err("cancelled");
        }
        if matches!(database.state, DatabaseState::Pending) {
            database.state = match open_database(&database.path, cancelled.clone()) {
                Ok(Some((connection, schema))) => DatabaseState::Ready(connection, schema),
                Ok(None) => DatabaseState::Failed,
                Err("cancelled") => return Err("cancelled"),
                Err(stage) => {
                    tracing::warn!(event = "opencode_sqlite_source_selection_failed", stage);
                    DatabaseState::Failed
                }
            };
        }
        let DatabaseState::Ready(connection, schema) = &database.state else {
            continue;
        };
        let unresolved = ids
            .iter()
            .filter(|id| !is_v2_selection(selected.get(*id)))
            .cloned()
            .collect();
        let result = select_database(
            connection,
            &database.path,
            *schema,
            &unresolved,
            cancelled.clone(),
            target,
        );
        match result {
            Ok(found) => {
                for (id, source) in found {
                    if is_v2_selection(selected.get(&id)) {
                        continue;
                    }
                    if is_v2_selection(Some(&source))
                        || selected.get(&id).is_none_or(Option::is_none)
                    {
                        selected.insert(id, source);
                    }
                }
            }
            Err("cancelled") => return Err("cancelled"),
            Err(stage) => {
                tracing::warn!(event = "opencode_sqlite_source_selection_failed", stage);
                database.state = DatabaseState::Failed;
            }
        }
    }
    Ok(())
}

fn open_database(
    path: &Path,
    cancelled: Arc<AtomicBool>,
) -> Result<Option<(Connection, OpenCodeSqliteSchema)>, &'static str> {
    let connection = match Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(connection) => connection,
        Err(_) if path.try_exists().is_ok_and(|exists| !exists) => return Ok(None),
        Err(_) => return Err("source_selection_open"),
    };
    connection
        .busy_timeout(Duration::from_millis(250))
        .map_err(|_| "source_selection_timeout")?;
    connection
        .execute_batch("BEGIN")
        .map_err(|_| "source_selection_snapshot")?;
    let schema = {
        let budget = QueryBudget::new(&connection, cancelled);
        let schema = opencode_sqlite_schema(&connection)
            .map_err(|_| budget.failure("source_selection_schema"))?;
        budget.check()?;
        schema
    };
    if schema == OpenCodeSqliteSchema::Malformed {
        return Err("source_selection_schema");
    }
    Ok(Some((connection, schema)))
}

fn select_database(
    connection: &Connection,
    path: &Path,
    schema: OpenCodeSqliteSchema,
    ids: &BTreeSet<String>,
    cancelled: Arc<AtomicBool>,
    target: SelectionTarget,
) -> Result<HashMap<String, Option<SessionLog>>, &'static str> {
    let mut selected = HashMap::new();
    let ids: Vec<_> = ids.iter().cloned().collect();
    for ids in ids.chunks(256) {
        let budget = QueryBudget::new(connection, cancelled.clone());
        select_from_database(
            connection,
            path,
            ids,
            &mut selected,
            schema,
            &budget,
            target,
        )
        .map_err(|stage| budget.failure(stage))?;
        budget.check()?;
    }
    Ok(selected)
}

fn select_from_database(
    connection: &Connection,
    path: &Path,
    ids: &[String],
    selected: &mut HashMap<String, Option<SessionLog>>,
    schema: OpenCodeSqliteSchema,
    budget: &QueryBudget<'_>,
    target: SelectionTarget,
) -> Result<(), &'static str> {
    budget.check()?;
    let mut query = schema
        .has_v2()
        .then(|| connection.prepare(V2_PARENT_QUERY))
        .transpose()
        .map_err(|_| "source_selection_query")?;
    let mut v1_query = schema
        .has_v1()
        .then(|| {
            let created = if db_table_has_column(connection, "session", "time_created") {
                "time_created"
            } else {
                "NULL"
            };
            let updated = if db_table_has_column(connection, "session", "time_updated") {
                "time_updated"
            } else {
                "NULL"
            };
            connection.prepare(&format!(
                "SELECT {created}, {updated} FROM session WHERE id = ?1"
            ))
        })
        .transpose()
        .map_err(|_| "source_selection_query")?;
    for id in ids {
        budget.check()?;
        let source = if let Some(query) = query.as_mut()
            && valid_opencode_v2_session_id(id)
        {
            select_session_source(query, path, id, budget, target)
        } else {
            Ok(None)
        };
        match source {
            Ok(Some(log)) => {
                selected.insert(id.clone(), Some(log));
            }
            Ok(None) => {
                if let Some(query) = v1_query.as_mut()
                    && let Some((created, updated)) = query
                        .query_row([id], |row| {
                            Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?))
                        })
                        .optional()
                        .map_err(|_| "source_selection_row")?
                {
                    selected.insert(
                        id.clone(),
                        Some(SessionLog {
                            environment: Default::default(),
                            agent_type: AgentKind::OpenCode,
                            source: SessionSource::ProviderDb {
                                agent: AgentKind::OpenCode,
                                db_path: path.to_path_buf(),
                                session_id: id.clone(),
                                source_format: Some(
                                    crate::analysis::SourceFormat::OpenCodeSqliteV1,
                                ),
                            },
                            updated_at: normalize_db_epoch(created)
                                .max(normalize_db_epoch(updated)),
                        }),
                    );
                }
            }
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
    target: SelectionTarget,
) -> Result<Option<SessionLog>, &'static str> {
    let row = match target {
        SelectionTarget::Transcript => v2_root_session(query, id, budget)?,
        SelectionTarget::Metadata => query
            .query_row([id], |row| row.get::<_, i64>(2))
            .optional()
            .map_err(|_| "source_selection_row")?
            .map(|updated| (id.to_owned(), updated)),
    };
    let Some((session_id, updated)) = row else {
        return Ok(None);
    };
    Ok(Some(SessionLog {
        environment: Default::default(),
        agent_type: AgentKind::OpenCode,
        source: SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path: path.to_path_buf(),
            session_id,
            source_format: Some(crate::analysis::SourceFormat::OpenCodeSqliteV2),
        },
        updated_at: normalize_db_epoch(Some(updated)),
    }))
}

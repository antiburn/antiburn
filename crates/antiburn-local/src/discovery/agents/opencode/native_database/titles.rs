use super::*;

pub(super) fn read(
    path: &Path,
    ids: &[String],
    cancelled: Arc<AtomicBool>,
) -> Vec<(String, Option<String>, bool)> {
    if ids.len() > 256 {
        return Vec::new();
    }
    let Ok(connection) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    if connection.busy_timeout(Duration::from_millis(250)).is_err() {
        return Vec::new();
    }
    let budget = QueryBudget::new(&connection, cancelled);
    let Ok(snapshot) = connection.unchecked_transaction() else {
        return Vec::new();
    };
    let Ok(schema) = opencode_sqlite_schema(&snapshot) else {
        return Vec::new();
    };
    let mut titles = Vec::new();
    let mut bytes = 0;
    for id in ids.iter().filter(|id| valid_session_id(id)) {
        if budget.check().is_err() {
            break;
        }
        let Some(format) = db_session_source_format_for_schema_connection(&snapshot, schema, id)
        else {
            continue;
        };
        let v2 = format == crate::analysis::SourceFormat::OpenCodeSqliteV2;
        let table = if v2 { "session_v2" } else { "session" };
        let remaining = MAX_V2_ADMISSION_BYTES.saturating_sub(bytes);
        let query = format!(
            "SELECT CASE WHEN length(CAST(title AS BLOB)) <= {remaining} THEN title END FROM {table} WHERE id = ?1"
        );
        let Ok(title) = snapshot.query_row(&query, [id], |row| row.get::<_, Option<String>>(0))
        else {
            continue;
        };
        bytes += title.as_ref().map_or(0, String::len);
        let title = title
            .map(|title| title.trim().to_owned())
            .filter(|title| !title.is_empty());
        titles.push((id.clone(), title, v2));
    }
    titles
}

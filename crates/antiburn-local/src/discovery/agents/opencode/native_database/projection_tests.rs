use super::*;

#[test]
fn projections_do_not_return_individually_small_fields_from_an_oversized_row() {
    for (field_bytes, limit) in [
        (600_000_u64, MAX_V2_ADMISSION_BYTES as u64),
        (6 * 1024 * 1024, MAX_V2_FINGERPRINT_ROW_BYTES),
    ] {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(&format!(
            "CREATE TABLE session_v2 (directory TEXT,title TEXT,metadata TEXT);
             INSERT INTO session_v2 VALUES (CAST(zeroblob({field_bytes}) AS TEXT), CAST(zeroblob({field_bytes}) AS TEXT), CAST(zeroblob({field_bytes}) AS TEXT));"
        )).unwrap();
        let _budget = QueryBudget::strict(&connection);
        let (selected, size) = bounded_v2_columns(
            &connection,
            "session_v2",
            &["directory", "title", "metadata"],
            limit,
        );
        let values = connection
            .query_row(
                &format!("SELECT {selected}, {size} FROM session_v2"),
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, u64>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(values, (None, None, None, 3 * field_bytes));
    }
}

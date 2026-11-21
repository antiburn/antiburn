use sqlx::Connection;
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    let a: String = db
        .query_row("SELECT sqlite_source_id()", [], |row| row.get(0))
        .unwrap();
    let mut db = sqlx::SqliteConnection::connect("sqlite::memory:")
        .await
        .unwrap();
    let b: String = sqlx::query_scalar("SELECT sqlite_source_id()")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(a, b);
    println!("rusqlite 0.32.1 and SQLx 0.8.6 share SQLite: {a}");
}

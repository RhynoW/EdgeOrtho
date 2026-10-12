use super::model::ShareRecord;
use rusqlite::{params, Connection};

/// 跟 `tracks::store::init_db` 共用同一個 SQLite 連線/檔案，這裡只負責在
/// 那個既有連線上補一張表，不另外開檔案。
pub fn init_db(conn: &Connection) -> anyhow::Result<()> {
    conn.execute(
        r#"CREATE TABLE IF NOT EXISTS shares (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at TEXT NOT NULL,
            ip         TEXT NOT NULL,
            url        TEXT NOT NULL,
            message    TEXT
        )"#,
        [],
    )?;
    Ok(())
}

pub fn insert_share(conn: &Connection, ip: &str, url: &str, message: Option<&str>) -> anyhow::Result<i64> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO shares (created_at, ip, url, message) VALUES (?1, ?2, ?3, ?4)",
        params![now, ip, url, message],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_shares(conn: &Connection) -> anyhow::Result<Vec<ShareRecord>> {
    let mut stmt = conn.prepare("SELECT id, created_at, ip, url, message FROM shares ORDER BY id DESC")?;
    let rows = stmt.query_map([], |row| {
        Ok(ShareRecord {
            id: row.get(0)?,
            created_at: row.get(1)?,
            ip: row.get(2)?,
            url: row.get(3)?,
            message: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

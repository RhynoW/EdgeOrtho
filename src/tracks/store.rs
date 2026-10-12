use super::model::{Feature, TrackPointProperties, TrackPointRow, TrackSummary};
use rusqlite::{params, Connection};

pub fn init_db(path: &str) -> anyhow::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute(
        r#"CREATE TABLE IF NOT EXISTS track_points (
            session_id TEXT NOT NULL,
            seq        INTEGER NOT NULL,
            drone_id   TEXT NOT NULL,
            timestamp  TEXT NOT NULL,
            geometry   TEXT NOT NULL,
            properties TEXT NOT NULL,
            inserted_at TEXT NOT NULL,
            PRIMARY KEY (session_id, seq)
        )"#,
        [],
    )?;
    Ok(conn)
}

/// 以 (session_id, seq) 為唯一鍵冪等寫入；已存在的 seq 會被 INSERT OR IGNORE 忽略，
/// 藉此讓斷網後的自動同步（Auto-Sync）可以安全重送而不產生重複資料。
pub fn insert_points(
    conn: &Connection,
    session_id: &str,
    features: &[Feature],
) -> anyhow::Result<(usize, usize)> {
    let mut accepted = 0usize;
    let mut duplicate = 0usize;
    let now = chrono::Utc::now().to_rfc3339();

    for f in features {
        let geometry_json = serde_json::to_string(&f.geometry)?;
        let properties_json = serde_json::to_string(&f.properties)?;
        let changed = conn.execute(
            r#"INSERT OR IGNORE INTO track_points
               (session_id, seq, drone_id, timestamp, geometry, properties, inserted_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            params![
                session_id,
                f.properties.seq as i64,
                f.properties.drone_id,
                f.properties.timestamp,
                geometry_json,
                properties_json,
                now,
            ],
        )?;
        if changed == 1 {
            accepted += 1;
        } else {
            duplicate += 1;
        }
    }
    Ok((accepted, duplicate))
}

pub fn list_points(conn: &Connection, session_id: &str) -> anyhow::Result<Vec<TrackPointRow>> {
    let mut stmt = conn.prepare(
        "SELECT seq, geometry, properties FROM track_points WHERE session_id = ?1 ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![session_id], |row| {
        let seq: i64 = row.get(0)?;
        let geometry_json: String = row.get(1)?;
        let properties_json: String = row.get(2)?;
        Ok((seq, geometry_json, properties_json))
    })?;

    let mut out = Vec::new();
    for r in rows {
        let (seq, geometry_json, properties_json) = r?;
        let geometry: serde_json::Value = serde_json::from_str(&geometry_json)?;
        let properties: TrackPointProperties = serde_json::from_str(&properties_json)?;
        out.push(TrackPointRow { seq: seq as u64, geometry, properties });
    }
    Ok(out)
}

pub fn list_sessions(conn: &Connection) -> anyhow::Result<Vec<TrackSummary>> {
    let mut stmt = conn.prepare(
        r#"SELECT session_id,
                  MIN(drone_id),
                  COUNT(*),
                  MIN(timestamp),
                  MAX(timestamp),
                  MAX(inserted_at)
           FROM track_points
           GROUP BY session_id
           ORDER BY MAX(inserted_at) DESC"#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(TrackSummary {
            session_id: row.get(0)?,
            drone_id: row.get(1)?,
            point_count: row.get(2)?,
            first_timestamp: row.get(3)?,
            last_timestamp: row.get(4)?,
            updated_at: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

//! 稽核紀錄（SQLite `audit_log`）：驗證失敗（401）、來源網域被拒（403）、限流（429）、影像上傳結果（ok／duplicate／rejected／failed）、稽核清除。
//!
//! 設計取捨：
//! - **成功的驗證不記錄**（每張圖磚請求都記會把資料庫灌爆，也沒有稽核價值）；只記「被拒絕的存取」與「有副作用的操作」。
//! - **失敗紀錄要節流**：攻擊者可以用大量壞請求灌爆稽核表。同一個（來源 IP, 結果:原因代碼）在 10 秒視窗內只寫一筆，
//!   視窗內被略過的次數併入下一筆的 detail（`suppressed=N`），所以不會漏算次數、也不會無限成長。
//! - **永遠不寫入密鑰**：未知的 API Key 只記「unknown API key」，不記值；JWT 只記驗證錯誤原因。
//! - **來源 IP**：只有 `server.behind_reverse_proxy = true` 時才信任 `X-Forwarded-For`；否則一律用連線的對端位址，避免被偽造。
//!
//! 防竄改（雜湊鏈）：
//! 每筆紀錄存 `prev_hash` 與 `hash = SHA-256(prev_hash ‖ 各欄位)`（欄位以長度前綴編碼，避免欄位邊界被移動的歧義）。
//! 竄改任何一筆、刪除中間任何一筆、調換順序，`GET /api/v1/audit/verify` 都會在該處斷鏈。
//! **限制（務必知道）**：能直接寫資料庫檔案的人可以重算整條鏈而不被發現。所以要把「鏈頭」（`head_id` 與 `head_hash`，匯出清單與驗證回應都有）
//! **定期存到資料庫之外**（另一台機器、唯讀儲存、簽章後寄出），驗證時帶 `expected_id`＋`expected_hash` 比對：重算過的鏈在該筆的雜湊會不同。
//!
//! 保存期限：`[audit] retention_days`（預設 0＝不自動刪除；稽核資料不宜默默消失）。啟用時，清除前先把被清除的最後一筆的雜湊記進 `audit_anchor`，
//! 清除後鏈仍可驗證；清除動作本身也寫一筆稽核。**清除前請先匯出**。

use crate::error::AppError;
use crate::state::SharedState;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub fn init_db(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        r#"CREATE TABLE IF NOT EXISTS audit_log (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            ts        TEXT NOT NULL,
            actor     TEXT NOT NULL,
            ip        TEXT,
            action    TEXT NOT NULL,
            resource  TEXT,
            outcome   TEXT NOT NULL,
            detail    TEXT,
            prev_hash TEXT,
            hash      TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_audit_outcome ON audit_log (outcome, id);
        CREATE TABLE IF NOT EXISTS audit_anchor (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            upto_id     INTEGER NOT NULL,
            upto_hash   TEXT NOT NULL,
            pruned_rows INTEGER NOT NULL,
            pruned_at   TEXT NOT NULL
        );"#,
    )?;
    // 舊版資料庫（沒有雜湊欄位）：補欄位；舊紀錄 hash 為 NULL，驗證時列為「未串鏈」。
    let cols: Vec<String> = conn.prepare("PRAGMA table_info(audit_log)")?.query_map([], |r| r.get::<_, String>(1))?.filter_map(|r| r.ok()).collect();
    for c in ["prev_hash", "hash"] {
        if !cols.iter().any(|x| x == c) {
            conn.execute(&format!("ALTER TABLE audit_log ADD COLUMN {c} TEXT"), [])?;
        }
    }
    Ok(())
}

/// 雜湊內容：前一筆雜湊＋各欄位（每個欄位前加 4 位元組長度）。
pub fn row_hash(prev: &str, ts: &str, actor: &str, ip: &str, action: &str, resource: &str, outcome: &str, detail: &str) -> String {
    let mut h = Sha256::new();
    for f in [prev, ts, actor, ip, action, resource, outcome, detail] {
        h.update((f.len() as u32).to_le_bytes());
        h.update(f.as_bytes());
    }
    format!("{:x}", h.finalize())
}

/// 寫一筆稽核並串進雜湊鏈。呼叫端必須持有資料庫連線的獨占權（`state.db` 的鎖），鏈頭才不會競爭。
pub fn log(conn: &Connection, actor: &str, ip: &str, action: &str, resource: &str, outcome: &str, detail: &str) {
    let ts = chrono::Utc::now().to_rfc3339();
    let prev: String = conn
        .query_row("SELECT hash FROM audit_log WHERE hash IS NOT NULL ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
        .or_else(|_| conn.query_row("SELECT upto_hash FROM audit_anchor WHERE id = 1", [], |r| r.get::<_, String>(0)))
        .unwrap_or_default();
    let hash = row_hash(&prev, &ts, actor, ip, action, resource, outcome, detail);
    let _ = conn.execute(
        "INSERT INTO audit_log (ts, actor, ip, action, resource, outcome, detail, prev_hash, hash) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![ts, actor, ip, action, resource, outcome, detail, prev, hash],
    );
}

#[derive(Debug, Clone, PartialEq)]
pub struct VerifyReport {
    pub ok: bool,
    pub rows_checked: i64,
    pub unchained_rows: i64,
    pub first_bad_id: Option<i64>,
    pub reason: Option<String>,
    pub head_id: Option<i64>,
    pub head_hash: Option<String>,
    pub anchor_match: Option<bool>,
}

/// 逐筆驗證雜湊鏈。`expected`＝(id, hash)：呼叫端另外保存的「鏈上某一筆」，不符代表整條鏈被重算過。
pub fn verify_chain(conn: &Connection, expected: Option<(i64, String)>) -> rusqlite::Result<VerifyReport> {
    // 起點：若曾清除過，第一筆剩下的紀錄的 prev_hash 必須等於錨點的 upto_hash；否則必須是空字串（鏈的起點）
    let anchor: Option<(i64, String)> = conn.query_row("SELECT upto_id, upto_hash FROM audit_anchor WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).ok();
    let mut expect_prev = anchor.as_ref().map(|a| a.1.clone()).unwrap_or_default();
    let mut st = conn.prepare("SELECT id, ts, actor, ip, action, resource, outcome, detail, prev_hash, hash FROM audit_log ORDER BY id")?;
    let mut rows = st.query([])?;
    let mut rep = VerifyReport { ok: true, rows_checked: 0, unchained_rows: 0, first_bad_id: None, reason: None, head_id: None, head_hash: None, anchor_match: None };
    let mut bad = |rep: &mut VerifyReport, id: i64, why: String| {
        if rep.ok {
            rep.ok = false;
            rep.first_bad_id = Some(id);
            rep.reason = Some(why);
        }
    };
    let mut seen_expected = false;
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        let g = |i: usize| -> rusqlite::Result<String> { Ok(r.get::<_, Option<String>>(i)?.unwrap_or_default()) };
        let (ts, actor, ip, action, resource, outcome, detail) = (g(1)?, g(2)?, g(3)?, g(4)?, g(5)?, g(6)?, g(7)?);
        let (prev_hash, hash): (Option<String>, Option<String>) = (r.get(8)?, r.get(9)?);
        let Some(hash) = hash else {
            rep.unchained_rows += 1; // 舊版留下、沒有雜湊的紀錄：不在鏈上
            continue;
        };
        let prev_hash = prev_hash.unwrap_or_default();
        if prev_hash != expect_prev {
            bad(&mut rep, id, format!("prev_hash 與前一筆不連續（可能有紀錄被刪除或調換）"));
        }
        let recomputed = row_hash(&prev_hash, &ts, &actor, &ip, &action, &resource, &outcome, &detail);
        if recomputed != hash {
            bad(&mut rep, id, "紀錄內容與其雜湊不符（內容被修改）".into());
        }
        rep.rows_checked += 1;
        if let Some((eid, ehash)) = &expected {
            if *eid == id {
                seen_expected = true;
                rep.anchor_match = Some(&hash == ehash);
            }
        }
        expect_prev = hash.clone();
        rep.head_id = Some(id);
        rep.head_hash = Some(hash);
    }
    if let Some((eid, _)) = &expected {
        if !seen_expected {
            // 錨點那一筆已不存在：可能被刪除；若它早於清除錨點則屬正常（已清除）
            rep.anchor_match = Some(anchor.as_ref().map(|a| *eid <= a.0).unwrap_or(false));
        }
        if rep.anchor_match == Some(false) {
            bad(&mut rep, *eid, "與外部保存的鏈上錨點不符（整條鏈可能被重算或該筆被刪除）".into());
        }
    }
    Ok(rep)
}

/// 刪除早於 `cutoff` 的紀錄，先把被刪的最後一筆的雜湊寫進錨點，使剩下的鏈仍可驗證。回傳刪除筆數。
/// 只能刪「鏈頭之前的連續前段」（以 id 為界），所以不會在中間留下洞。
pub fn prune_before(conn: &Connection, cutoff_rfc3339: &str) -> rusqlite::Result<i64> {
    let last: Option<(i64, Option<String>)> = conn
        .query_row("SELECT id, hash FROM audit_log WHERE ts < ?1 ORDER BY id DESC LIMIT 1", params![cutoff_rfc3339], |r| Ok((r.get(0)?, r.get(1)?)))
        .ok();
    let Some((upto_id, upto_hash)) = last else { return Ok(0) };
    // 舊版沒有雜湊的紀錄不能當錨點：往前找最後一筆有雜湊的
    let upto_hash = match upto_hash {
        Some(h) => (upto_id, h),
        None => match conn.query_row("SELECT id, hash FROM audit_log WHERE id <= ?1 AND hash IS NOT NULL ORDER BY id DESC LIMIT 1", params![upto_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))) {
            Ok(x) => x,
            Err(_) => return conn.execute("DELETE FROM audit_log WHERE id <= ?1", params![upto_id]).map(|n| n as i64),
        },
    };
    let tx = conn.unchecked_transaction()?;
    let n = tx.execute("DELETE FROM audit_log WHERE id <= ?1", params![upto_id])? as i64;
    tx.execute(
        "INSERT INTO audit_anchor (id, upto_id, upto_hash, pruned_rows, pruned_at) VALUES (1, ?1, ?2, ?3, ?4) \
         ON CONFLICT(id) DO UPDATE SET upto_id = ?1, upto_hash = ?2, pruned_rows = pruned_rows + ?3, pruned_at = ?4",
        params![upto_hash.0, upto_hash.1, n, chrono::Utc::now().to_rfc3339()],
    )?;
    tx.commit()?;
    Ok(n)
}

/// 背景保存期限：每 6 小時清除一次早於 `retention_days` 的紀錄；清除動作本身寫一筆稽核。
pub async fn run_retention(state: SharedState, retention_days: u32) {
    loop {
        {
            let db = state.db.lock().await;
            let cutoff = (chrono::Utc::now() - chrono::Duration::days(retention_days as i64)).to_rfc3339();
            match prune_before(&db, &cutoff) {
                Ok(n) if n > 0 => log(&db, "system", "", "audit_prune", "audit_log", "ok", &format!("pruned={n}; retention_days={retention_days}; cutoff={cutoff}")),
                Ok(_) => {}
                Err(e) => log(&db, "system", "", "audit_prune", "audit_log", "failed", &e.to_string()),
            }
        }
        tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
    }
}

/// 來源 IP：見模組說明。
pub fn client_ip(headers: &HeaderMap, addr: Option<&SocketAddr>, behind_proxy: bool) -> String {
    if behind_proxy {
        if let Some(v) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|v| v.split(',').next()) {
            let v = v.trim();
            if !v.is_empty() && v.len() <= 64 {
                return v.to_string();
            }
        }
    }
    addr.map(|a| a.ip().to_string()).unwrap_or_else(|| "unknown".into())
}

/// 失敗紀錄的節流器：同一個 key 在 `window` 內只放行第一次，其餘累計次數，下次放行時一併回報。
pub struct Throttle {
    window: Duration,
    max_keys: usize,
    map: HashMap<(String, String), (Instant, u32)>,
}

impl Throttle {
    pub fn new(window: Duration, max_keys: usize) -> Self {
        Throttle { window, max_keys, map: HashMap::new() }
    }

    /// 回傳 `Some(被略過的次數)` 表示這一筆要寫入；`None` 表示被節流（已累計）。
    pub fn check(&mut self, ip: &str, outcome: &str, now: Instant) -> Option<u32> {
        if self.map.len() >= self.max_keys {
            // 防止被大量不同 IP 撐爆記憶體：直接清空（最壞情況是多放行一輪紀錄）
            self.map.clear();
        }
        let e = self.map.entry((ip.to_string(), outcome.to_string())).or_insert((now - self.window - Duration::from_secs(1), 0));
        if now.duration_since(e.0) >= self.window {
            let suppressed = e.1;
            *e = (now, 0);
            Some(suppressed)
        } else {
            e.1 += 1;
            None
        }
    }
}

static THROTTLE: Mutex<Option<Throttle>> = Mutex::new(None);

fn throttle_check(ip: &str, kind: &str) -> Option<u32> {
    let mut g = THROTTLE.lock().unwrap();
    g.get_or_insert_with(|| Throttle::new(Duration::from_secs(10), 5000)).check(ip, kind, Instant::now())
}

/// 記錄一筆「被拒絕的存取」（401／403／429）。非同步寫入，不阻塞回應。
/// `reason` 是固定的原因代碼（如 `unknown_api_key`），節流以（IP［已驗證者再加身分］, 結果:原因代碼）為單位；
/// `detail` 可含使用者可控的內容（如 Origin 字串），所以**不能**拿來當節流 key，否則攻擊者換個字串就能繞過節流。
pub fn deny(state: &SharedState, actor: &str, ip: &str, method: &str, path: &str, outcome: &'static str, reason: &'static str, detail: &str) {
    // 已通過驗證的拒絕（權限不足、限流）以「IP＋身分」節流，否則同一個 IP 上不同金鑰的拒絕會互相吞掉；匿名的只看 IP。
    let tkey = if actor == "anonymous" { ip.to_string() } else { format!("{ip}|{actor}") };
    let Some(suppressed) = throttle_check(&tkey, &format!("{outcome}:{reason}")) else { return };
    let state = state.clone();
    let (actor, ip, resource) = (actor.to_string(), ip.to_string(), format!("{method} {path}"));
    let mut detail: String = format!("{reason}: {detail}").chars().take(220).collect();
    if suppressed > 0 {
        detail.push_str(&format!("; suppressed={suppressed}"));
    }
    tokio::spawn(async move {
        let db = state.db.lock().await;
        log(&db, &actor, &ip, "access", &resource, outcome, &detail);
    });
}

#[derive(Deserialize)]
pub struct AuditQuery {
    limit: Option<u32>,
    outcome: Option<String>,
    action: Option<String>,
}

fn row_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({ "id": r.get::<_, i64>(0)?, "ts": r.get::<_, String>(1)?, "actor": r.get::<_, String>(2)?, "ip": r.get::<_, Option<String>>(3)?,
               "action": r.get::<_, String>(4)?, "resource": r.get::<_, Option<String>>(5)?, "outcome": r.get::<_, String>(6)?, "detail": r.get::<_, Option<String>>(7)?,
               "prev_hash": r.get::<_, Option<String>>(8)?, "hash": r.get::<_, Option<String>>(9)? }))
}

/// 稽核紀錄（最新在前）。可用 `outcome=`、`action=` 過濾。
pub async fn get_audit(State(state): State<SharedState>, Query(q): Query<AuditQuery>) -> Result<Json<Value>, AppError> {
    let db = state.db.lock().await;
    let map = |e: rusqlite::Error| AppError::Internal(e.to_string());
    let rows: Vec<Value> = db
        .prepare(
            "SELECT id, ts, actor, ip, action, resource, outcome, detail, prev_hash, hash FROM audit_log \
             WHERE (?2 IS NULL OR outcome = ?2) AND (?3 IS NULL OR action = ?3) ORDER BY id DESC LIMIT ?1",
        )
        .map_err(map)?
        .query_map(params![q.limit.unwrap_or(100).min(1000), q.outcome, q.action], row_json)
        .map_err(map)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(Json(json!({ "entries": rows })))
}

fn open_readonly(state: &SharedState) -> Result<Connection, AppError> {
    Connection::open_with_flags(&state.config.server.db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| AppError::Internal(e.to_string()))
}

#[derive(Deserialize)]
pub struct VerifyQuery {
    /// 外部保存的鏈上某一筆：`expected_id` 與 `expected_hash` 必須成對給。
    expected_id: Option<i64>,
    expected_hash: Option<String>,
}

fn report_json(r: &VerifyReport) -> Value {
    json!({ "ok": r.ok, "rows_checked": r.rows_checked, "unchained_rows": r.unchained_rows, "first_bad_id": r.first_bad_id, "reason": r.reason,
            "head_id": r.head_id, "head_hash": r.head_hash, "anchor_match": r.anchor_match,
            "note": "head_id／head_hash 請定期存到資料庫之外（另一台機器或唯讀儲存），驗證時以 expected_id／expected_hash 帶入；否則能寫資料庫的人可以重算整條鏈而不被發現。" })
}

/// 驗證雜湊鏈（用另開的唯讀連線，不佔用寫入鎖）。
pub async fn verify(State(state): State<SharedState>, Query(q): Query<VerifyQuery>) -> Result<Json<Value>, AppError> {
    let expected = match (q.expected_id, q.expected_hash) {
        (Some(i), Some(h)) => Some((i, h)),
        (None, None) => None,
        _ => return Err(AppError::BadRequest("expected_id 與 expected_hash 必須成對提供".into())),
    };
    let conn = open_readonly(&state)?;
    let rep = tokio::task::spawn_blocking(move || verify_chain(&conn, expected)).await.map_err(|e| AppError::Internal(e.to_string()))?.map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(report_json(&rep)))
}

#[derive(Deserialize)]
pub struct ExportQuery {
    /// 起訖時間（含起、不含迄），RFC 3339 或 YYYY-MM-DD；不給就是全部。
    from: Option<String>,
    to: Option<String>,
    outcome: Option<String>,
}

const EXPORT_MAX_ROWS: i64 = 200_000;

/// 匯出 JSON Lines：每筆一行，最後一行是 `{"manifest": …}`（筆數、範圍、鏈頭、鏈是否完整、匯出時間）。
/// 匯出本身也寫一筆稽核（誰在什麼時候匯出了什麼範圍）。
pub async fn export(State(state): State<SharedState>, axum::Extension(identity): axum::Extension<crate::auth::Identity>, Query(q): Query<ExportQuery>) -> Result<Response, AppError> {
    let conn = open_readonly(&state)?;
    let (from, to, outcome) = (q.from.clone(), q.to.clone(), q.outcome.clone());
    let (lines, rep, count, first_id, last_id) = tokio::task::spawn_blocking(move || -> rusqlite::Result<(String, VerifyReport, i64, Option<i64>, Option<i64>)> {
        let rep = verify_chain(&conn, None)?;
        let mut st = conn.prepare(
            "SELECT id, ts, actor, ip, action, resource, outcome, detail, prev_hash, hash FROM audit_log \
             WHERE (?1 IS NULL OR ts >= ?1) AND (?2 IS NULL OR ts < ?2) AND (?3 IS NULL OR outcome = ?3) ORDER BY id LIMIT ?4",
        )?;
        let mut out = String::new();
        let (mut n, mut first, mut last) = (0i64, None, None);
        for r in st.query_map(params![from, to, outcome, EXPORT_MAX_ROWS], row_json)? {
            let v = r?;
            if first.is_none() { first = v["id"].as_i64(); }
            last = v["id"].as_i64();
            out.push_str(&v.to_string());
            out.push('\n');
            n += 1;
        }
        Ok((out, rep, n, first, last))
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .map_err(|e| AppError::Internal(e.to_string()))?;
    let manifest = json!({ "manifest": {
        "rows": count, "first_id": first_id, "last_id": last_id, "truncated_at_max_rows": count >= EXPORT_MAX_ROWS,
        "range": { "from": q.from, "to": q.to, "outcome": q.outcome },
        "chain": { "ok": rep.ok, "head_id": rep.head_id, "head_hash": rep.head_hash, "unchained_rows": rep.unchained_rows, "first_bad_id": rep.first_bad_id },
        "exported_at": chrono::Utc::now().to_rfc3339(),
    }});
    {
        let db = state.db.lock().await;
        let actor = identity.actor();
        log(&db, &actor, "", "audit_export", "audit_log", "ok", &format!("rows={count}; first_id={first_id:?}; last_id={last_id:?}; chain_ok={}", rep.ok));
    }
    let body = format!("{lines}{manifest}\n");
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson; charset=utf-8"), (header::CONTENT_DISPOSITION, "attachment; filename=\"audit_log.jsonl\"")], body).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        init_db(&c).unwrap();
        c
    }

    fn fill(c: &Connection, n: usize) {
        for i in 0..n {
            log(c, "anonymous", "1.2.3.4", "access", &format!("GET /x/{i}"), "unauthorized", &format!("no_credentials: detail {i}"));
        }
    }

    #[test]
    fn throttle_passes_first_then_counts_then_reports() {
        let mut t = Throttle::new(Duration::from_secs(10), 100);
        let t0 = Instant::now();
        assert_eq!(t.check("1.2.3.4", "unauthorized", t0), Some(0));
        assert_eq!(t.check("1.2.3.4", "unauthorized", t0 + Duration::from_secs(1)), None);
        assert_eq!(t.check("1.2.3.4", "unauthorized", t0 + Duration::from_secs(9)), None);
        // 不同 IP、不同結果各自獨立
        assert_eq!(t.check("5.6.7.8", "unauthorized", t0 + Duration::from_secs(2)), Some(0));
        assert_eq!(t.check("1.2.3.4", "rate_limited", t0 + Duration::from_secs(2)), Some(0));
        // 視窗過後放行，並帶出被略過的 2 次
        assert_eq!(t.check("1.2.3.4", "unauthorized", t0 + Duration::from_secs(11)), Some(2));
        assert_eq!(t.check("1.2.3.4", "unauthorized", t0 + Duration::from_secs(12)), None);
    }

    #[test]
    fn throttle_memory_is_bounded() {
        let mut t = Throttle::new(Duration::from_secs(10), 50);
        let t0 = Instant::now();
        for i in 0..500 {
            t.check(&format!("10.0.{}.{}", i / 250, i % 250), "unauthorized", t0);
        }
        assert!(t.map.len() <= 50);
    }

    #[test]
    fn forwarded_for_only_trusted_behind_proxy() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "9.9.9.9, 10.0.0.1".parse().unwrap());
        let addr: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        assert_eq!(client_ip(&h, Some(&addr), true), "9.9.9.9");
        assert_eq!(client_ip(&h, Some(&addr), false), "127.0.0.1");
        assert_eq!(client_ip(&HeaderMap::new(), None, false), "unknown");
    }

    #[test]
    fn hash_encoding_is_not_ambiguous_across_field_boundaries() {
        // 把 "ab"|"c" 與 "a"|"bc" 當成不同的欄位切法，雜湊必須不同（長度前綴）
        assert_ne!(row_hash("", "t", "ab", "c", "a", "r", "o", "d"), row_hash("", "t", "a", "bc", "a", "r", "o", "d"));
    }

    #[test]
    fn intact_chain_verifies() {
        let c = mem();
        fill(&c, 20);
        let r = verify_chain(&c, None).unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!((r.rows_checked, r.unchained_rows, r.head_id), (20, 0, Some(20)));
    }

    #[test]
    fn modified_row_is_detected_at_that_row() {
        let c = mem();
        fill(&c, 10);
        c.execute("UPDATE audit_log SET outcome = 'ok' WHERE id = 6", []).unwrap();
        let r = verify_chain(&c, None).unwrap();
        assert!(!r.ok);
        assert_eq!(r.first_bad_id, Some(6));
        assert!(r.reason.unwrap().contains("內容被修改"));
    }

    #[test]
    fn deleted_middle_row_is_detected() {
        let c = mem();
        fill(&c, 10);
        c.execute("DELETE FROM audit_log WHERE id = 4", []).unwrap();
        let r = verify_chain(&c, None).unwrap();
        assert!(!r.ok);
        assert_eq!(r.first_bad_id, Some(5)); // 第 5 筆的 prev_hash 對不上（前一筆是 3）
    }

    #[test]
    fn recomputed_chain_is_caught_only_by_external_anchor() {
        let c = mem();
        fill(&c, 8);
        let head = verify_chain(&c, None).unwrap();
        let anchor = (head.head_id.unwrap(), head.head_hash.clone().unwrap());
        assert_eq!(verify_chain(&c, Some(anchor.clone())).unwrap().anchor_match, Some(true));
        // 攻擊者改了第 3 筆，並把之後的整條鏈都重算（沒有外部錨點就看不出來）
        c.execute("UPDATE audit_log SET detail = 'forged' WHERE id = 3", []).unwrap();
        let mut prev: String = c.query_row("SELECT hash FROM audit_log WHERE id = 2", [], |r| r.get(0)).unwrap();
        let rows: Vec<(i64, String, String, String, String, String, String, String)> = c
            .prepare("SELECT id, ts, actor, ip, action, resource, outcome, detail FROM audit_log WHERE id >= 3 ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))
            .unwrap()
            .filter_map(|x| x.ok())
            .collect();
        for (id, ts, a, ip, ac, rs, o, d) in rows {
            let h = row_hash(&prev, &ts, &a, &ip, &ac, &rs, &o, &d);
            c.execute("UPDATE audit_log SET prev_hash = ?1, hash = ?2 WHERE id = ?3", params![prev, h, id]).unwrap();
            prev = h;
        }
        assert!(verify_chain(&c, None).unwrap().ok, "鏈本身內部一致，單看資料庫發現不了");
        let r = verify_chain(&c, Some(anchor)).unwrap();
        assert!(!r.ok);
        assert_eq!(r.anchor_match, Some(false));
    }

    #[test]
    fn prune_keeps_chain_verifiable_via_anchor() {
        let c = mem();
        fill(&c, 12);
        // 讓前 5 筆「很舊」
        c.execute("UPDATE audit_log SET ts = '2020-01-01T00:00:00+00:00' WHERE id <= 5", []).unwrap();
        // 注意：改了 ts 會破壞雜湊（這只是為了測試清除邏輯）——先重算鏈
        let mut prev = String::new();
        let rows: Vec<(i64, String, String, String, String, String, String, String)> = c
            .prepare("SELECT id, ts, actor, ip, action, resource, outcome, detail FROM audit_log ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))
            .unwrap()
            .filter_map(|x| x.ok())
            .collect();
        for (id, ts, a, ip, ac, rs, o, d) in rows {
            let h = row_hash(&prev, &ts, &a, &ip, &ac, &rs, &o, &d);
            c.execute("UPDATE audit_log SET prev_hash = ?1, hash = ?2 WHERE id = ?3", params![prev, h, id]).unwrap();
            prev = h;
        }
        assert!(verify_chain(&c, None).unwrap().ok);
        let n = prune_before(&c, "2021-01-01T00:00:00+00:00").unwrap();
        assert_eq!(n, 5);
        let r = verify_chain(&c, None).unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.rows_checked, 7);
        // 清除後繼續寫入，鏈仍連續
        fill(&c, 3);
        assert!(verify_chain(&c, None).unwrap().ok);
        // 外部錨點指向已被清除的舊紀錄：視為正常；指向仍存在的紀錄但雜湊不符：失敗
        assert_eq!(verify_chain(&c, Some((3, "whatever".into()))).unwrap().anchor_match, Some(true));
        assert!(!verify_chain(&c, Some((8, "wrong".into()))).unwrap().ok);
        // 清除之後有人把剩下的第一筆刪掉：斷鏈
        c.execute("DELETE FROM audit_log WHERE id = 6", []).unwrap();
        assert!(!verify_chain(&c, None).unwrap().ok);
    }

    #[test]
    fn legacy_rows_without_hash_are_reported_but_not_failures() {
        let c = Connection::open_in_memory().unwrap();
        // 舊版表格：沒有 prev_hash / hash 欄位
        c.execute_batch("CREATE TABLE audit_log (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL, actor TEXT NOT NULL, ip TEXT, action TEXT NOT NULL, resource TEXT, outcome TEXT NOT NULL, detail TEXT);
                         INSERT INTO audit_log (ts, actor, ip, action, resource, outcome, detail) VALUES ('2026-01-01T00:00:00+00:00','a','1.1.1.1','access','GET /','unauthorized','old');")
            .unwrap();
        init_db(&c).unwrap(); // 補欄位
        fill(&c, 3);
        let r = verify_chain(&c, None).unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!((r.unchained_rows, r.rows_checked), (1, 3));
    }
}

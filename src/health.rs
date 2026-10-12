//! 健康檢查與可用率報表（對應採購規範「API 可用率 ≥ 99.5%（月）」）。
//!
//! - `GET /healthz`：存活檢查（既有，永遠 200「ok」）。
//! - `GET /readyz`（免驗證）：就緒檢查，實際寫一筆資料庫、檢查圖磚來源；失敗回 503。給負載平衡器／監控用，不回傳路徑等細節。
//! - `GET /api/v1/availability?month=YYYY-MM&tz=8`（需驗證）：月可用率報表。
//!
//! 可用率怎麼算（務必讀懂這個定義）：
//! 背景工作每 10 秒自我探測一次（資料庫可寫、至少一個圖磚來源可讀），並累加進當分鐘的一筆 `uptime_samples`
//! （探測次數、失敗次數、請求數、5xx 數；每次探測都寫，所以行程在某分鐘中途被關掉，那一分鐘仍有紀錄）。**一分鐘算「可用」的條件**：當分鐘有紀錄、探測全部成功，
//! 且 5xx 比例 ≤ 5%（沒有請求時不看比例）。**沒有紀錄的分鐘算「不可用」**——行程沒在跑就寫不出紀錄，
//! 這正是要抓的停機。分母從該期間「第一筆紀錄」開始算，所以新部署的第一天不會被扣掉部署之前的時間。
//!
//! 這是「服務自己量的」可用率，**有兩個盲點**：整台機器或網路斷線時外部看得到、內部看不到的「連不上」，
//! 這裡只會呈現成「沒有紀錄的分鐘」（也就是停機，方向是對的），但無法區分原因；而且它無法偵測
//! 反向代理或 DNS 層的故障。正式驗收應另外用外部探測（例如每分鐘對 `/readyz` 取樣）交叉比對。

use crate::config::AppConfig;
use crate::error::AppError;
use crate::state::SharedState;
use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub const TARGET_PCT: f64 = 99.5;
/// 一分鐘內 5xx 比例超過這個百分比就視為「不健康」。
pub const ERR_RATE_LIMIT_PCT: i64 = 5;
const PROBE_INTERVAL: Duration = Duration::from_secs(10);

static REQUESTS: AtomicU64 = AtomicU64::new(0);
static ERRORS_5XX: AtomicU64 = AtomicU64::new(0);
static STARTED_AT: std::sync::OnceLock<chrono::DateTime<chrono::Utc>> = std::sync::OnceLock::new();

pub fn init_db(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        r#"CREATE TABLE IF NOT EXISTS uptime_samples (
            minute     INTEGER PRIMARY KEY,   -- Unix 分鐘（秒 / 60）
            probes     INTEGER NOT NULL,
            probe_fail INTEGER NOT NULL,
            requests   INTEGER NOT NULL,
            errors_5xx INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS uptime_events (
            ts    TEXT NOT NULL,
            event TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS health_probe (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            ts TEXT NOT NULL
        );"#,
    )?;
    conn.execute("INSERT INTO uptime_events (ts, event) VALUES (?1, 'start')", params![chrono::Utc::now().to_rfc3339()])?;
    let _ = STARTED_AT.set(chrono::Utc::now());
    Ok(())
}

/// 計算請求數與 5xx 數的最外層中介層（`/healthz`、`/readyz` 不計，避免監控探測稀釋比例）。
pub async fn count_requests(req: Request, next: Next) -> Response {
    let skip = matches!(req.uri().path(), "/healthz" | "/readyz");
    let resp = next.run(req).await;
    if !skip {
        REQUESTS.fetch_add(1, Ordering::Relaxed);
        if resp.status().is_server_error() {
            ERRORS_5XX.fetch_add(1, Ordering::Relaxed);
        }
    }
    resp
}

pub struct ProbeResult {
    pub database: bool,
    pub tiles_ok: usize,
    pub tiles_total: usize,
}

impl ProbeResult {
    /// 就緒＝資料庫可寫，且（沒有設定圖磚來源，或至少一個來源可讀）。
    pub fn ready(&self) -> bool {
        self.database && (self.tiles_total == 0 || self.tiles_ok > 0)
    }
}

fn tile_sources(cfg: &AppConfig) -> (usize, usize) {
    let (mut ok, mut total) = (0, 0);
    for l in &cfg.layers {
        if let Some(g) = &l.gpkg {
            total += 1;
            ok += std::path::Path::new(&g.path).is_file() as usize;
        } else if let Some(d) = &l.tile_dir {
            total += 1;
            ok += std::fs::metadata(d).map(|m| m.is_dir()).unwrap_or(false) as usize;
        }
    }
    (ok, total)
}

pub async fn probe(state: &SharedState) -> ProbeResult {
    // 資料庫：2 秒內拿得到連線並寫入成功才算正常（鎖被卡住也算不正常）
    let database = match tokio::time::timeout(Duration::from_secs(2), state.db.lock()).await {
        Ok(db) => db
            .execute("INSERT OR REPLACE INTO health_probe (id, ts) VALUES (1, ?1)", params![chrono::Utc::now().to_rfc3339()])
            .is_ok(),
        Err(_) => false,
    };
    let cfg = state.config.clone();
    let (tiles_ok, tiles_total) = tokio::task::spawn_blocking(move || tile_sources(&cfg)).await.unwrap_or((0, 0));
    ProbeResult { database, tiles_ok, tiles_total }
}

/// 就緒檢查（免驗證）。
pub async fn readyz(State(state): State<SharedState>) -> Response {
    let p = probe(&state).await;
    let up = STARTED_AT.get().map(|t| (chrono::Utc::now() - *t).num_seconds()).unwrap_or(0);
    let body = json!({
        "status": if p.ready() { "ok" } else { "unavailable" },
        "checks": { "database": p.database, "tile_sources": { "ok": p.tiles_ok, "total": p.tiles_total } },
        "uptime_seconds": up,
    });
    (if p.ready() { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE }, Json(body)).into_response()
}

fn now_minute() -> i64 {
    chrono::Utc::now().timestamp() / 60
}

/// 把一次探測的結果累加進當分鐘的紀錄（`ON CONFLICT DO UPDATE` 累加，不是覆蓋）。
/// 每次探測都寫，所以行程在某分鐘中途被關掉時，那一分鐘仍留有紀錄；重啟後在同一分鐘內再寫也不會蓋掉先前的紀錄。
async fn record(state: &SharedState, minute: i64, probes: i64, fail: i64, req: i64, err: i64) {
    let db = state.db.lock().await;
    let _ = db.execute(
        "INSERT INTO uptime_samples (minute, probes, probe_fail, requests, errors_5xx) VALUES (?1,?2,?3,?4,?5) \
         ON CONFLICT(minute) DO UPDATE SET probes = probes + ?2, probe_fail = probe_fail + ?3, requests = requests + ?4, errors_5xx = errors_5xx + ?5",
        params![minute, probes, fail, req, err],
    );
}

/// 背景取樣：每 10 秒探測一次並累加進當分鐘的紀錄。
///
/// 重啟的那一分鐘：若行程是在該分鐘的第 10 秒之後才起來，代表那一分鐘有一段時間沒在服務，記一次探測失敗（算不健康）。
/// 全新部署（資料庫裡還沒有任何紀錄）不套用這條，否則第一分鐘永遠被扣。
pub async fn run_sampler(state: SharedState) {
    let had_history: bool = {
        let db = state.db.lock().await;
        db.query_row("SELECT COUNT(*) FROM uptime_samples", [], |r| r.get::<_, i64>(0)).map(|n| n > 0).unwrap_or(false)
    };
    let (mut req0, mut err0) = (REQUESTS.load(Ordering::Relaxed) as i64, ERRORS_5XX.load(Ordering::Relaxed) as i64);
    let mut first = true;
    loop {
        let ok = probe(&state).await.ready();
        let now = chrono::Utc::now();
        let (req1, err1) = (REQUESTS.load(Ordering::Relaxed) as i64, ERRORS_5XX.load(Ordering::Relaxed) as i64);
        let mut fail = if ok { 0 } else { 1 };
        if first && had_history && now.timestamp() % 60 >= 10 {
            fail += 1;
        }
        first = false;
        record(&state, now.timestamp() / 60, 1, fail, req1 - req0, err1 - err0).await;
        req0 = req1;
        err0 = err1;
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub minute: i64,
    pub probes: i64,
    pub probe_fail: i64,
    pub requests: i64,
    pub errors_5xx: i64,
}

impl Sample {
    pub fn up(&self) -> bool {
        self.probes > 0 && self.probe_fail == 0 && (self.requests == 0 || self.errors_5xx * 100 <= self.requests * ERR_RATE_LIMIT_PCT)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Incident {
    pub start_minute: i64,
    pub minutes: i64,
    /// `no_data`＝沒有紀錄（行程沒在跑）；`unhealthy`＝有紀錄但探測失敗或 5xx 過高；`mixed`＝兩者連續出現。
    pub kind: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub measured_from: Option<i64>,
    pub measured_to: i64,
    pub total_minutes: i64,
    pub up_minutes: i64,
    pub unhealthy_minutes: i64,
    pub no_data_minutes: i64,
    pub availability_pct: Option<f64>,
    pub allowed_downtime_minutes: f64,
    pub incidents: Vec<Incident>,
}

/// 計算 [from, to) 這段 Unix 分鐘內的可用率。`now_min` 之前的最後一個分鐘（可能還沒寫入）不計入，避免誤判。
pub fn compute(samples: &[Sample], from: i64, to: i64, now_min: i64) -> Report {
    compute_with(samples, from, to, now_min, false)
}

/// `strict = true`：嚴格從 `from` 起算（`from` 之後沒有紀錄的分鐘一律算停機），用在呼叫端明確指定的比對期間；
/// `false`：從期間內第一筆紀錄起算（月報表用，部署之前的時間不扣）。
pub fn compute_with(samples: &[Sample], from: i64, to: i64, now_min: i64, strict: bool) -> Report {
    let end = to.min(now_min - 1).max(from);
    let by: HashMap<i64, &Sample> = samples.iter().filter(|s| s.minute >= from && s.minute < end).map(|s| (s.minute, s)).collect();
    let first_opt = if strict { (end > from).then_some(from) } else { by.keys().min().copied() };
    let Some(first) = first_opt else {
        return Report { measured_from: None, measured_to: end, total_minutes: 0, up_minutes: 0, unhealthy_minutes: 0, no_data_minutes: 0, availability_pct: None, allowed_downtime_minutes: 0.0, incidents: vec![] };
    };
    let (mut up, mut unhealthy, mut nodata) = (0i64, 0i64, 0i64);
    let mut incidents: Vec<Incident> = Vec::new();
    let mut run: Option<(i64, i64, i64, i64)> = None; // (start, minutes, unhealthy_in_run, nodata_in_run)
    let close = |run: &mut Option<(i64, i64, i64, i64)>, incidents: &mut Vec<Incident>| {
        if let Some((start, minutes, u, n)) = run.take() {
            incidents.push(Incident { start_minute: start, minutes, kind: if u == 0 { "no_data" } else if n == 0 { "unhealthy" } else { "mixed" } });
        }
    };
    for m in first..end {
        match by.get(&m) {
            Some(s) if s.up() => {
                up += 1;
                close(&mut run, &mut incidents);
            }
            other => {
                let is_nodata = other.is_none();
                if is_nodata { nodata += 1 } else { unhealthy += 1 }
                let r = run.get_or_insert((m, 0, 0, 0));
                r.1 += 1;
                if is_nodata { r.3 += 1 } else { r.2 += 1 }
            }
        }
    }
    close(&mut run, &mut incidents);
    let total = end - first;
    Report {
        measured_from: Some(first),
        measured_to: end,
        total_minutes: total,
        up_minutes: up,
        unhealthy_minutes: unhealthy,
        no_data_minutes: nodata,
        availability_pct: Some(((up as f64 / total as f64) * 100.0 * 10_000.0).round() / 10_000.0),
        allowed_downtime_minutes: (total as f64 * (100.0 - TARGET_PCT) / 100.0 * 100.0).round() / 100.0,
        incidents,
    }
}

/// 每一分鐘的狀態（與 `compute` 同一套判定）：`up`／`unhealthy`／`no_data`。範圍從第一筆紀錄算起，同樣排除 `now_min - 1` 之後。
pub fn minute_states(samples: &[Sample], from: i64, to: i64, now_min: i64) -> Vec<(i64, &'static str)> {
    minute_states_with(samples, from, to, now_min, false)
}

pub fn minute_states_with(samples: &[Sample], from: i64, to: i64, now_min: i64, strict: bool) -> Vec<(i64, &'static str)> {
    let end = to.min(now_min - 1).max(from);
    let by: HashMap<i64, &Sample> = samples.iter().filter(|s| s.minute >= from && s.minute < end).map(|s| (s.minute, s)).collect();
    let first_opt = if strict { (end > from).then_some(from) } else { by.keys().min().copied() };
    let Some(first) = first_opt else { return vec![] };
    (first..end).map(|m| (m, match by.get(&m) { Some(s) if s.up() => "up", Some(_) => "unhealthy", None => "no_data" })).collect()
}

/// "YYYY-MM"＋時區（小時）→ (起始 Unix 分鐘, 結束 Unix 分鐘)，半開區間。
pub fn month_range(month: &str, tz_hours: i32) -> Option<(i64, i64)> {
    let (y, m) = month.split_once('-')?;
    let (y, m): (i32, u32) = (y.parse().ok()?, m.parse().ok()?);
    let start = chrono::NaiveDate::from_ymd_opt(y, m, 1)?;
    let next = if m == 12 { chrono::NaiveDate::from_ymd_opt(y + 1, 1, 1)? } else { chrono::NaiveDate::from_ymd_opt(y, m + 1, 1)? };
    let to_min = |d: chrono::NaiveDate| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp() / 60 - tz_hours as i64 * 60;
    Some((to_min(start), to_min(next)))
}

#[derive(Deserialize)]
pub struct AvailQuery {
    month: Option<String>,
    tz: Option<i32>,
    /// 自訂期間（本地時間，`YYYY-MM-DDTHH:MM`，半開區間）；給了就不看 `month`。
    from: Option<String>,
    to: Option<String>,
    /// 期間不超過 2 天時，附上逐分鐘狀態（供外部探測交叉比對）。
    detail: Option<bool>,
    /// 只對自訂期間（from／to）有效：嚴格從 `from` 起算，沒有紀錄的分鐘算停機（預設從第一筆紀錄起算）。
    strict: Option<bool>,
}

fn parse_local_minute(v: &str, tz: i32) -> Option<i64> {
    let d = chrono::NaiveDateTime::parse_from_str(v, "%Y-%m-%dT%H:%M").ok()?;
    Some(d.and_utc().timestamp() / 60 - tz as i64 * 60)
}

fn fmt_min(m: i64, tz: i32) -> String {
    chrono::DateTime::from_timestamp((m + tz as i64 * 60) * 60, 0).map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

pub async fn get_availability(State(state): State<SharedState>, Query(q): Query<AvailQuery>) -> Result<Json<Value>, AppError> {
    let tz = q.tz.unwrap_or(8).clamp(-12, 14);
    let month = q.month.unwrap_or_else(|| (chrono::Utc::now() + chrono::Duration::hours(tz as i64)).format("%Y-%m").to_string());
    let (from, to) = match (&q.from, &q.to) {
        (Some(f), Some(t)) => {
            let (f, t) = (parse_local_minute(f, tz), parse_local_minute(t, tz));
            match (f, t) {
                (Some(f), Some(t)) if t > f => (f, t),
                _ => return Err(AppError::BadRequest("from／to 格式為 YYYY-MM-DDTHH:MM，且 to 必須晚於 from".into())),
            }
        }
        _ => month_range(&month, tz).ok_or_else(|| AppError::BadRequest("month 格式為 YYYY-MM".into()))?,
    };
    let samples: Vec<Sample> = {
        let db = state.db.lock().await;
        let map = |e: rusqlite::Error| AppError::Internal(e.to_string());
        let v = db
            .prepare("SELECT minute, probes, probe_fail, requests, errors_5xx FROM uptime_samples WHERE minute >= ?1 AND minute < ?2 ORDER BY minute")
            .map_err(map)?
            .query_map(params![from, to], |r| Ok(Sample { minute: r.get(0)?, probes: r.get(1)?, probe_fail: r.get(2)?, requests: r.get(3)?, errors_5xx: r.get(4)? }))
            .map_err(map)?
            .filter_map(|r| r.ok())
            .collect();
        v
    };
    let now = now_minute();
    let strict = q.from.is_some() && q.strict.unwrap_or(false);
    let r = compute_with(&samples, from, to, now, strict);
    let minutes: Option<Vec<Value>> = (q.detail.unwrap_or(false) && to - from <= 2 * 24 * 60)
        .then(|| minute_states_with(&samples, from, to, now, strict).into_iter().map(|(m, st)| json!({ "t": fmt_min(m, tz), "minute": m, "state": st })).collect());
    let meets = r.availability_pct.map(|p| p >= TARGET_PCT);
    let down = r.unhealthy_minutes + r.no_data_minutes;
    Ok(Json(json!({
        "month": month, "timezone_offset_hours": tz,
        "period": { "from": fmt_min(from, tz), "to": fmt_min(to, tz) },
        "measured_from": r.measured_from.map(|m| fmt_min(m, tz)), "measured_to": fmt_min(r.measured_to, tz),
        "total_minutes": r.total_minutes, "up_minutes": r.up_minutes, "down_minutes": down,
        "down_breakdown": { "no_data": r.no_data_minutes, "unhealthy": r.unhealthy_minutes },
        "availability_pct": r.availability_pct, "target_pct": TARGET_PCT, "meets_target": meets,
        "allowed_downtime_minutes": r.allowed_downtime_minutes,
        "downtime_budget_remaining_minutes": ((r.allowed_downtime_minutes - down as f64) * 100.0).round() / 100.0,
        "minutes": minutes,
        "incidents_total": r.incidents.len(),
        "incidents": r.incidents.iter().take(50).map(|i| json!({ "start": fmt_min(i.start_minute, tz), "minutes": i.minutes, "kind": i.kind })).collect::<Vec<_>>(),
        "definition": format!("一分鐘為「可用」＝有紀錄、自我探測全部成功、5xx 比例 ≤ {ERR_RATE_LIMIT_PCT}%；沒有紀錄的分鐘算不可用。服務自己量測，正式驗收需搭配外部探測（對 /readyz 取樣）。"),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(m: i64) -> Sample {
        Sample { minute: m, probes: 6, probe_fail: 0, requests: 100, errors_5xx: 0 }
    }

    #[test]
    fn all_up_is_100_percent_and_meets_target() {
        let s: Vec<_> = (1000..1100).map(up).collect();
        let r = compute(&s, 1000, 1100, 1200);
        assert_eq!((r.total_minutes, r.up_minutes), (100, 100));
        assert_eq!(r.availability_pct, Some(100.0));
        assert!(r.incidents.is_empty());
        assert_eq!(r.allowed_downtime_minutes, 0.5);
    }

    #[test]
    fn missing_minutes_count_as_down_and_form_incidents() {
        let mut s: Vec<_> = (1000..1200).map(up).collect();
        s.retain(|x| !(1050..1053).contains(&x.minute) && !(1100..1101).contains(&x.minute)); // 兩段停機：3 分鐘與 1 分鐘
        let r = compute(&s, 1000, 1200, 1300);
        assert_eq!((r.total_minutes, r.up_minutes, r.no_data_minutes), (200, 196, 4));
        assert_eq!(r.availability_pct, Some(98.0));
        assert_eq!(r.incidents, vec![Incident { start_minute: 1050, minutes: 3, kind: "no_data" }, Incident { start_minute: 1100, minutes: 1, kind: "no_data" }]);
    }

    #[test]
    fn probe_failures_and_high_5xx_rate_are_unhealthy() {
        let mut s: Vec<_> = (0..20).map(up).collect();
        s[5].probe_fail = 1; // 探測失敗
        s[6].errors_5xx = 6; // 6% > 5%
        s[7].errors_5xx = 5; // 恰好 5%：仍算可用
        s[8].requests = 0; // 沒有請求：不看比例
        s[8].errors_5xx = 0;
        let r = compute(&s, 0, 20, 100);
        assert_eq!((r.up_minutes, r.unhealthy_minutes), (18, 2));
        assert_eq!(r.incidents.len(), 1);
        assert_eq!(r.incidents[0], Incident { start_minute: 5, minutes: 2, kind: "unhealthy" });
    }

    #[test]
    fn mixed_run_and_trailing_minutes() {
        let mut s: Vec<_> = (0..30).map(up).collect();
        s[10].probe_fail = 2;
        s.retain(|x| x.minute != 11 && x.minute != 12);
        let r = compute(&s, 0, 30, 31);
        // now_min=31：最後一個分鐘（30 之前的 29）仍計入；10 為不健康、11–12 無紀錄 → 同一事件、混合
        assert_eq!(r.incidents, vec![Incident { start_minute: 10, minutes: 3, kind: "mixed" }]);
        assert_eq!(r.total_minutes, 30);
    }

    #[test]
    fn measured_from_first_sample_and_current_minute_excluded() {
        let s: Vec<_> = (500..510).map(up).collect();
        // 期間從 0 開始，但服務 500 才開始有紀錄：不扣掉部署之前的時間
        let r = compute(&s, 0, 10_000, 511);
        assert_eq!(r.measured_from, Some(500));
        assert_eq!(r.total_minutes, 10); // end = now-1 = 510
        assert_eq!(r.availability_pct, Some(100.0));
        // 沒有任何紀錄
        let e = compute(&[], 0, 100, 200);
        assert_eq!((e.total_minutes, e.availability_pct), (0, None));
    }

    #[test]
    fn meets_target_boundary_995() {
        // 1000 分鐘、5 分鐘停機＝99.5%，恰好達標
        let s: Vec<_> = (0..1000).filter(|m| !(100..105).contains(m)).map(up).collect();
        let r = compute(&s, 0, 1000, 5000);
        assert_eq!(r.availability_pct, Some(99.5));
        assert!(r.availability_pct.unwrap() >= TARGET_PCT);
        assert_eq!(r.allowed_downtime_minutes, 5.0);
    }

    #[test]
    fn minute_states_matches_compute_classification() {
        let mut s: Vec<_> = (0..10).map(up).collect();
        s[3].probe_fail = 1;
        s.retain(|x| x.minute != 6);
        let st = minute_states(&s, 0, 10, 100);
        assert_eq!(st.iter().map(|x| x.1).collect::<Vec<_>>(), ["up", "up", "up", "unhealthy", "up", "up", "no_data", "up", "up", "up"]);
        assert!(minute_states(&[], 0, 10, 100).is_empty());
    }

    #[test]
    fn strict_counts_minutes_before_first_sample_as_down() {
        let s: Vec<_> = (105..110).map(up).collect();
        let loose = compute_with(&s, 100, 110, 200, false);
        assert_eq!((loose.total_minutes, loose.no_data_minutes), (5, 0));
        let strict = compute_with(&s, 100, 110, 200, true);
        assert_eq!((strict.total_minutes, strict.up_minutes, strict.no_data_minutes), (10, 5, 5));
        assert_eq!(strict.incidents, vec![Incident { start_minute: 100, minutes: 5, kind: "no_data" }]);
        assert_eq!(minute_states_with(&s, 100, 110, 200, true).iter().filter(|x| x.1 == "no_data").count(), 5);
        // 完全沒有紀錄：嚴格模式整段都是停機；寬鬆模式沒有可量測的資料
        assert_eq!(compute_with(&[], 100, 110, 200, true).no_data_minutes, 10);
        assert_eq!(compute_with(&[], 100, 110, 200, false).total_minutes, 0);
    }

    #[test]
    fn parse_local_minute_applies_timezone() {
        // 2026-10-10 12:00 +08:00 = 04:00 UTC
        let m = parse_local_minute("2026-10-10T12:00", 8).unwrap();
        assert_eq!(chrono::DateTime::from_timestamp(m * 60, 0).unwrap().format("%H:%M").to_string(), "04:00");
        assert!(parse_local_minute("2026-10-10 12:00", 8).is_none());
    }

    #[test]
    fn month_range_handles_timezone_and_year_end() {
        let (a, b) = month_range("2026-10", 8).unwrap();
        assert_eq!(b - a, 31 * 24 * 60);
        // 2026-10-01 00:00 +08:00 = 2026-09-30 16:00 UTC
        assert_eq!(chrono::DateTime::from_timestamp(a * 60, 0).unwrap().format("%Y-%m-%d %H:%M").to_string(), "2026-09-30 16:00");
        let (c, d) = month_range("2026-12", 0).unwrap();
        assert_eq!(d - c, 31 * 24 * 60);
        assert!(month_range("2026-13", 8).is_none());
        assert!(month_range("bad", 8).is_none());
    }
}

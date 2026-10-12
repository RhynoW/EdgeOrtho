//! 無人機影像上傳與增量鑲嵌（選用功能 `ingest`）。
//!
//! `POST /api/v1/ingest/frame?session_id=…`：body 是原始 JPEG（含 EXIF GPS 與 DJI 風格 XMP 姿態）。
//! 伺服器讀出位置與姿態 → 以 TM2 幾何投影到設定的目標圖磚格網（EPSG:3857 與 EPSG:3826）→ 只重寫受影響的圖磚。
//! - 冪等：以 JPEG 的 SHA-256 當 frame id；同一張重送回 `duplicate`，不會重複鑲嵌。
//! - 序列化：鑲嵌是「讀舊圖磚 → 混合 → 寫回」，用單一鎖依序處理，避免兩張影像同時改同一塊圖磚。
//! - 稽核：每次上傳（成功、重複、拒絕、失敗）都經 `crate::audit` 寫入 SQLite `audit_log`，含身分、來源 IP、結果（驗證失敗與限流由 auth 中介層記錄）。
//! - 即時圖層的 `Cache-Control` 要縮短（見 LayerConfig.cache_seconds），否則瀏覽器會一直看舊圖磚。

pub mod geo;
pub mod mosaic;
pub mod pose;

use crate::auth::Identity;
use crate::error::AppError;
use crate::state::SharedState;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Extension, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Debug, Deserialize, Clone)]
pub struct IngestTarget {
    pub tms: String,
    pub zoom: u32,
    /// 往下（較粗的縮放級別）重算幾級金字塔；0＝不做。
    #[serde(default)]
    pub pyramid: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct IngestConfig {
    /// 輸出圖層 id；該圖層的 `tile_dir` 就是圖磚寫入位置。
    pub layer: String,
    /// 地面解析度 = 相對高度 × gsd_per_agl（相機焦距與像元大小的比值）。
    pub gsd_per_agl: f64,
    #[serde(default = "default_feather")]
    pub feather_px: f64,
    #[serde(default = "default_max_bytes")]
    pub max_frame_bytes: usize,
    /// 同時處理的影像數（每張影像內部還會用最多 8 條執行緒處理圖磚）。預設＝核心數 ÷ 4（至少 2）。
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// 等待處理的影像上限；超過就回 503＋`Retry-After`，讓客戶端退避，而不是無限排隊。預設 64。
    #[serde(default = "default_max_queue")]
    pub max_queue: usize,
    pub targets: Vec<IngestTarget>,
}

/// 預設並行度：CPU 核心數 ÷ 4（每張影像內部最多用 8 條執行緒），至少 2。實測（32 核、22 架過載）吞吐量：1 路 2.65、2 路 3.57、4 路 4.66、8 路 5.49、16 路 5.20 張/s。
fn default_concurrency() -> usize {
    (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8) / 4).max(2)
}
fn default_max_queue() -> usize {
    64
}

fn default_feather() -> f64 {
    64.0
}
fn default_max_bytes() -> usize {
    40 * 1024 * 1024
}

/// 並行度（同時處理的影像數）：第一次使用時依設定建立。
static PERMITS: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> = std::sync::OnceLock::new();
/// 目前在等待處理名額的影像數、被入場控制拒絕的總數（自行程啟動起算）。
static WAITING: AtomicUsize = AtomicUsize::new(0);
static BUSY_TOTAL: AtomicUsize = AtomicUsize::new(0);
/// 處理中的 frame id：同一張影像（相同 SHA-256）不會被同時處理兩次。
static IN_FLIGHT: std::sync::Mutex<Option<HashSet<String>>> = std::sync::Mutex::new(None);

struct InFlight(String);

impl InFlight {
    fn try_new(id: &str) -> Option<InFlight> {
        let mut g = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(HashSet::new).insert(id.to_string()).then(|| InFlight(id.to_string()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Some(s) = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            s.remove(&self.0);
        }
    }
}

pub fn init_db(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        r#"CREATE TABLE IF NOT EXISTS ingest_frames (
            frame_id    TEXT PRIMARY KEY,
            session_id  TEXT NOT NULL,
            actor       TEXT NOT NULL,
            received_at TEXT NOT NULL,
            captured_at TEXT,
            lat REAL, lon REAL, agl REAL,
            bytes       INTEGER NOT NULL,
            tiles       INTEGER NOT NULL,
            queue_ms    INTEGER NOT NULL,
            process_ms  INTEGER NOT NULL
        );
        "#,
    )?;
    Ok(())
}

async fn reject(state: &SharedState, actor: &str, ip: &str, frame_id: &str, session: &str, msg: &str, outcome: &str) {
    let db = state.db.lock().await;
    crate::audit::log(&db, actor, ip, "ingest_frame", frame_id, outcome, &format!("session={session}; {msg}"));
}

#[derive(Deserialize)]
pub struct FrameQuery {
    session_id: Option<String>,
}

pub async fn post_frame(
    State(state): State<SharedState>,
    Extension(identity): Extension<Identity>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<FrameQuery>,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    let t_recv = Instant::now();
    let cfg = state.config.ingest.clone().ok_or_else(|| AppError::NotFound("ingest is not enabled".into()))?;
    let (actor, ip) = (identity.actor(), crate::audit::client_ip(&headers, Some(&addr), state.config.server.behind_reverse_proxy));
    let session = q.session_id.unwrap_or_else(|| "default".into());
    let frame_id = format!("{:x}", Sha256::digest(&body));
    if session.is_empty() || session.len() > 64 || !session.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
        reject(&state, &actor, &ip, &frame_id, &session.chars().take(64).collect::<String>(), "invalid session_id", "rejected").await;
        return Err(AppError::BadRequest("session_id 只允許英數、-、_、.，長度 1–64".into()));
    }
    // 先做便宜的檢查（大小、EXIF／XMP），無效的上傳不佔用處理名額
    if body.len() > cfg.max_frame_bytes {
        reject(&state, &actor, &ip, &frame_id, &session, &format!("too large: {} bytes", body.len()), "rejected").await;
        return Err(AppError::BadRequest(format!("影像超過上限 {} bytes", cfg.max_frame_bytes)));
    }
    let pose = match pose::parse(&body) {
        Ok(p) => p,
        Err(e) => {
            reject(&state, &actor, &ip, &frame_id, &session, &e, "rejected").await;
            return Err(AppError::BadRequest(e));
        }
    };

    // 已處理過 → 直接回 duplicate（不佔名額）
    {
        let db = state.db.lock().await;
        let dup: Option<(String, i64)> = db
            .query_row("SELECT session_id, tiles FROM ingest_frames WHERE frame_id = ?1", params![frame_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .ok();
        if let Some((s, tiles)) = dup {
            crate::audit::log(&db, &actor, &ip, "ingest_frame", &frame_id, "duplicate", &format!("session={session}; first_session={s}"));
            return Ok(Json(json!({ "status": "duplicate", "frame_id": frame_id, "session_id": s, "tiles_written": tiles, "queue_ms": 0 })));
        }
    }
    // 同一張影像正在處理中（客戶端逾時後重送）→ 409，不重複處理
    let Some(_in_flight) = InFlight::try_new(&frame_id) else {
        crate::audit::deny(&state, &actor, &ip, "POST", "/api/v1/ingest/frame", "conflict", "in_progress", &format!("frame={frame_id}"));
        return Err(AppError::Conflict("這張影像正在處理中，稍後重送會得到 duplicate".into()));
    };
    // 入場控制：等待處理名額的影像超過上限就回 503（客戶端依 Retry-After 退避）
    let waiting = WAITING.fetch_add(1, Ordering::SeqCst) + 1;
    if waiting > cfg.max_queue {
        WAITING.fetch_sub(1, Ordering::SeqCst);
        BUSY_TOTAL.fetch_add(1, Ordering::Relaxed);
        crate::audit::deny(&state, &actor, &ip, "POST", "/api/v1/ingest/frame", "busy", "queue_full", &format!("waiting={waiting}; max_queue={}", cfg.max_queue));
        return Err(AppError::Busy(format!("處理佇列已滿（{} 張在等待），請稍後重試", cfg.max_queue)));
    }
    let permits = PERMITS.get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(cfg.concurrency.max(1)))).clone();
    let _permit = permits.acquire_owned().await.map_err(|e| AppError::Internal(e.to_string()));
    WAITING.fetch_sub(1, Ordering::SeqCst);
    let _permit = _permit?;
    let queue_ms = t_recv.elapsed().as_millis() as u64;
    let layer = state.config.layer(&cfg.layer).ok_or_else(|| AppError::Internal(format!("ingest.layer '{}' 不存在", cfg.layer)))?;
    let tile_dir = layer.tile_dir.clone().ok_or_else(|| AppError::Internal("ingest 圖層沒有 tile_dir".into()))?;

    let t0 = Instant::now();
    let work = {
        let state = state.clone();
        let (cfg, pose, body) = (cfg.clone(), pose.clone(), body.clone());
        tokio::task::spawn_blocking(move || -> Result<(Vec<(String, usize, usize)>, (f64, f64), f64), String> {
            let img = image::load_from_memory_with_format(&body, image::ImageFormat::Jpeg).map_err(|e| format!("JPEG 解碼失敗：{e}"))?.to_rgb8();
            let g = mosaic::Geom::new(&pose, cfg.gsd_per_agl, img.width(), img.height());
            let mut per = Vec::new();
            for t in &cfg.targets {
                let tms = state.config.tilematrixset(&t.tms).ok_or_else(|| format!("未知 TileMatrixSet {}", t.tms))?;
                let changed = mosaic::mosaic_into(&img, &g, tms, t.zoom, &tile_dir, cfg.feather_px)?;
                let pyr = if t.pyramid > 0 { mosaic::build_pyramid(tms, t.zoom, &changed, t.pyramid, &tile_dir)? } else { 0 };
                per.push((format!("{}/{}", t.tms, t.zoom), changed.len(), pyr));
            }
            Ok((per, g.footprint_center(), g.gsd()))
        })
        .await
    };
    let (per, center, gsd) = match work {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            reject(&state, &actor, &ip, &frame_id, &session, &e, "failed").await;
            return Err(AppError::BadRequest(e));
        }
        Err(e) => {
            reject(&state, &actor, &ip, &frame_id, &session, &e.to_string(), "failed").await;
            return Err(AppError::Internal(e.to_string()));
        }
    };
    let process_ms = t0.elapsed().as_millis() as u64;
    let tiles: usize = per.iter().map(|(_, n, p)| n + p).sum();
    {
        let db = state.db.lock().await;
        db.execute(
            "INSERT OR IGNORE INTO ingest_frames (frame_id, session_id, actor, received_at, captured_at, lat, lon, agl, bytes, tiles, queue_ms, process_ms) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![frame_id, session, actor, chrono::Utc::now().to_rfc3339(), pose.captured_at, pose.lat, pose.lon, pose.agl, body.len() as i64, tiles as i64, queue_ms as i64, process_ms as i64],
        )
        .map_err(|e| AppError::Internal(e.to_string()))?;
        crate::audit::log(&db, &actor, &ip, "ingest_frame", &frame_id, "ok", &format!("session={session}; tiles={tiles}; process_ms={process_ms}"));
    }
    Ok(Json(json!({
        "status": "processed", "frame_id": frame_id, "session_id": session,
        "tiles_written": tiles, "per_target": per.iter().map(|(k, n, p)| json!({ "target": k, "tiles": n, "pyramid_tiles": p })).collect::<Vec<_>>(),
        "footprint_center_tm2": [center.0, center.1], "gsd_m": gsd,
        "queue_ms": queue_ms, "process_ms": process_ms, "total_ms": t_recv.elapsed().as_millis() as u64,
    })))
}

fn percentile(sorted: &[i64], p: f64) -> i64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[(((sorted.len() - 1) as f64) * p).round() as usize]
}

/// 處理統計：張數、圖磚數、延遲分位數、重複與拒絕次數。
pub async fn get_stats(State(state): State<SharedState>) -> Result<Json<Value>, AppError> {
    let db = state.db.lock().await;
    let map = |e: rusqlite::Error| AppError::Internal(e.to_string());
    let (frames, tiles): (i64, i64) = db.query_row("SELECT COUNT(*), COALESCE(SUM(tiles),0) FROM ingest_frames", [], |r| Ok((r.get(0)?, r.get(1)?))).map_err(map)?;
    let mut ms: Vec<i64> = db.prepare("SELECT process_ms FROM ingest_frames ORDER BY received_at DESC LIMIT 2000").map_err(map)?.query_map([], |r| r.get(0)).map_err(map)?.filter_map(|r| r.ok()).collect();
    ms.sort_unstable();
    let outcome = |o: &str| -> i64 { db.query_row("SELECT COUNT(*) FROM audit_log WHERE action='ingest_frame' AND outcome=?1", params![o], |r| r.get(0)).unwrap_or(0) };
    let (first, last): (Option<String>, Option<String>) = db.query_row("SELECT MIN(received_at), MAX(received_at) FROM ingest_frames", [], |r| Ok((r.get(0)?, r.get(1)?))).map_err(map)?;
    Ok(Json(json!({
        "frames": frames, "tiles_written": tiles, "duplicates": outcome("duplicate"), "rejected": outcome("rejected"), "failed": outcome("failed"),
        "process_ms": { "p50": percentile(&ms, 0.5), "p95": percentile(&ms, 0.95), "max": ms.last().copied().unwrap_or(0), "n": ms.len() },
        "first_received_at": first, "last_received_at": last,
        "admission": {
            "concurrency": state.config.ingest.as_ref().map(|c| c.concurrency).unwrap_or(0),
            "max_queue": state.config.ingest.as_ref().map(|c| c.max_queue).unwrap_or(0),
            "waiting_now": WAITING.load(Ordering::SeqCst),
            "busy_rejections_total": BUSY_TOTAL.load(Ordering::Relaxed),
        },
    })))
}

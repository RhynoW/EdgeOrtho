//! Overpass 相容查詢端點：`GET/POST /api/interpreter`，後端是 DuckDB + spatial R-tree。
//! 設計見 docs/overpass-duckdb-design.md。

pub mod parser;
pub mod search;

use crate::error::AppError;
use crate::state::SharedState;
use axum::extract::{Query as AxQuery, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use parser::{Filter, Op, OutMode, Program};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::time::Duration;

#[derive(Debug, Deserialize, Clone)]
pub struct OverpassConfig {
    pub db_path: String,
    /// 單次查詢 bbox 面積上限（度²）。
    #[serde(default = "d_area")]
    pub max_bbox_deg2: f64,
    #[serde(default = "d_results")]
    pub max_results: usize,
    #[serde(default = "d_timeout")]
    pub timeout_seconds: u64,
    /// spatial 擴充的本機檔案路徑（離線部署用）；未設定則 `INSTALL spatial` 自網路取得。
    #[serde(default)]
    pub spatial_extension: Option<String>,
}
fn d_area() -> f64 { 4.0 }
fn d_results() -> usize { 1000 }
fn d_timeout() -> u64 { 10 }

pub fn open(cfg: &OverpassConfig) -> anyhow::Result<duckdb::Connection> {
    let conn = duckdb::Connection::open_with_flags(
        &cfg.db_path,
        duckdb::Config::default().access_mode(duckdb::AccessMode::ReadOnly)?,
    )?;
    match &cfg.spatial_extension {
        Some(p) => conn.execute_batch(&format!("LOAD '{}';", p.replace('\'', "''")))?,
        None => conn.execute_batch("INSTALL spatial; LOAD spatial;")?,
    }
    Ok(conn)
}

/// 產生參數化 SQL。標籤 key/value 一律走綁定參數，不做字串拼接。
fn build_sql(prog: &Program, cfg: &OverpassConfig) -> Result<(String, Vec<String>), AppError> {
    let mut parts = Vec::new();
    // 參數順序必須跟 ? 出現順序一致。
    let mut order: Vec<String> = Vec::new();
    for q in &prog.queries {
        let bb = q.bbox.or(prog.bbox).ok_or_else(|| AppError::BadRequest("a bounding box is required ([bbox:s,w,n,e] or (s,w,n,e))".into()))?;
        let area = (bb[2] - bb[0]).abs() * (bb[3] - bb[1]).abs();
        if area > cfg.max_bbox_deg2 {
            return Err(AppError::RateLimited);
        }
        // bbox 以數值字面量內嵌（f64 的 Display 只會是數字，無注入風險），比對的是外框欄位 x0/y0/x1/y1：
        // 資料依 Hilbert 排序，row group 的 zonemap 能裁掉絕大部分區塊。實測（全台 1200 萬筆、
        // ~0.02° bbox + 標籤）：ST_Intersects 走 R-tree 再回表取 tags 需 3.5–5 秒，外框欄位約 0.1 秒。
        // R-tree 索引仍保留在資料庫中，供純幾何查詢或之後的細部相交判斷使用。
        if !bb.iter().all(|v| v.is_finite()) { return Err(AppError::BadRequest("invalid bbox".into())); }
        let mut w = format!("x1 >= {} AND x0 <= {} AND y1 >= {} AND y0 <= {}", bb[1], bb[3], bb[0], bb[2]);
        let types: Vec<String> = q.types.iter().map(|t| format!("'{t}'")).collect();
        if q.types.len() < 3 {
            w.push_str(&format!(" AND typ IN ({})", types.join(",")));
        }
        for Filter { key, op } in &q.filters {
            match op {
                Op::Exists => { w.push_str(" AND map_contains(tags, ?)"); order.push((key.clone())); }
                Op::NotExists => { w.push_str(" AND NOT map_contains(tags, ?)"); order.push((key.clone())); }
                Op::Eq(v) => { w.push_str(" AND tags[?] = ?"); order.extend([(key.clone()), (v.clone())]); }
                Op::Ne(v) => { w.push_str(" AND (NOT map_contains(tags, ?) OR tags[?] <> ?)"); order.extend([(key.clone()), (key.clone()), (v.clone())]); }
                Op::Re(v, i) => { w.push_str(" AND regexp_matches(tags[?], ?, ?)"); order.extend([(key.clone()), (v.clone()), (if *i { "i" } else { "" }.into())]); }
                Op::NotRe(v, i) => { w.push_str(" AND NOT coalesce(regexp_matches(tags[?], ?, ?), false)"); order.extend([(key.clone()), (v.clone()), (if *i { "i" } else { "" }.into())]); }
            }
        }
        parts.push(format!(
            "SELECT typ, id, CAST(to_json(tags) AS VARCHAR) AS tags, CAST(ST_AsGeoJSON(geom) AS VARCHAR) AS gj, \
             ST_X(ST_Centroid(geom)) AS cx, ST_Y(ST_Centroid(geom)) AS cy FROM osm_element WHERE {w}"
        ));
    }
    let limit = prog.limit.unwrap_or(cfg.max_results).min(cfg.max_results) + 1;
    let sql = format!("SELECT * FROM ({}) ORDER BY typ, id LIMIT {limit}", parts.join(" UNION "));
    let params: Vec<String> = order;
    Ok((sql, params))
}


fn run(conn: &duckdb::Connection, prog: &Program, cfg: &OverpassConfig) -> Result<(Vec<Value>, bool), AppError> {
    let (sql, params) = build_sql(prog, cfg)?;
    let mut stmt = conn.prepare(&sql).map_err(|e| AppError::Internal(e.to_string()))?;
    let mut rows = stmt
        .query(duckdb::params_from_iter(params.iter()))
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let max = prog.limit.unwrap_or(cfg.max_results).min(cfg.max_results);
    let mut out = Vec::new();
    let mut truncated = false;
    while let Some(r) = rows.next().map_err(|e| AppError::Internal(e.to_string()))? {
        if out.len() >= max { truncated = true; break; }
        let typ: String = r.get(0).map_err(ie)?;
        let id: i64 = r.get(1).map_err(ie)?;
        let tags: Option<String> = r.get(2).map_err(ie)?;
        let gj: Option<String> = r.get(3).map_err(ie)?;
        let cx: Option<f64> = r.get(4).map_err(ie)?;
        let cy: Option<f64> = r.get(5).map_err(ie)?;
        out.push(element(&typ, id, tags, gj, cx, cy, prog.out));
    }
    Ok((out, truncated))
}
fn ie(e: duckdb::Error) -> AppError { AppError::Internal(e.to_string()) }

fn element(typ: &str, id: i64, tags: Option<String>, gj: Option<String>, cx: Option<f64>, cy: Option<f64>, mode: OutMode) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), json!(typ));
    m.insert("id".into(), json!(id));
    if mode == OutMode::Ids { return Value::Object(m); }
    let g: Value = gj.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    let pt = |c: &Value| json!({"lat": c[1], "lon": c[0]});
    match (typ, mode) {
        ("node", _) => { m.insert("lat".into(), g["coordinates"][1].clone()); m.insert("lon".into(), g["coordinates"][0].clone()); }
        (_, OutMode::Geom) => {
            let ring = match g["type"].as_str() {
                Some("Polygon") => &g["coordinates"][0],
                _ => &g["coordinates"],
            };
            let pts: Vec<Value> = ring.as_array().map(|a| a.iter().map(pt).collect()).unwrap_or_default();
            let (mut s, mut w, mut n, mut e) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for p in &pts {
                let (la, lo) = (p["lat"].as_f64().unwrap_or(0.0), p["lon"].as_f64().unwrap_or(0.0));
                s = s.min(la); n = n.max(la); w = w.min(lo); e = e.max(lo);
            }
            if !pts.is_empty() { m.insert("bounds".into(), json!({"minlat": s, "minlon": w, "maxlat": n, "maxlon": e})); }
            m.insert("geometry".into(), Value::Array(pts));
        }
        (_, OutMode::Center) => { if let (Some(x), Some(y)) = (cx, cy) { m.insert("center".into(), json!({"lat": y, "lon": x})); } }
        _ => {}
    }
    if let Some(t) = tags.and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        if t.as_object().map(|o| !o.is_empty()).unwrap_or(false) { m.insert("tags".into(), t); }
    }
    Value::Object(m)
}

#[derive(Deserialize)]
pub struct DataParam { data: Option<String> }

pub async fn interpreter_get(State(st): State<SharedState>, AxQuery(q): AxQuery<DataParam>) -> Result<Response, AppError> {
    handle(st, q.data.unwrap_or_default()).await
}

pub async fn interpreter_post(State(st): State<SharedState>, headers: HeaderMap, body: String) -> Result<Response, AppError> {
    let is_form = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(|v| v.starts_with("application/x-www-form-urlencoded")).unwrap_or(false);
    let data = if is_form || body.starts_with("data=") {
        body.split('&').find_map(|kv| kv.strip_prefix("data=")).map(url_decode).unwrap_or_default()
    } else { body };
    handle(st, data).await
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut o = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => o.push(b' '),
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) { o.push(v); i += 2; } else { o.push(b'%'); }
            }
            c => o.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&o).into_owned()
}

async fn handle(st: SharedState, data: String) -> Result<Response, AppError> {
    let (Some(cfg), Some(base)) = (st.config.overpass.clone(), st.osm.as_ref()) else {
        return Err(AppError::NotFound("overpass endpoint is not enabled".into()));
    };
    if data.trim().is_empty() { return Err(AppError::BadRequest("missing 'data' parameter".into())); }
    let prog = parser::parse(&data).map_err(AppError::BadRequest)?;
    let conn = base.lock().map_err(|_| AppError::Internal("db lock poisoned".into()))?.try_clone().map_err(ie)?;
    let handle = conn.interrupt_handle();
    let tmo = Duration::from_secs(prog.timeout.unwrap_or(cfg.timeout_seconds).min(cfg.timeout_seconds));
    let cfg2 = cfg.clone();
    let job = tokio::task::spawn_blocking(move || {
        let r = run(&conn, &prog, &cfg2);
        (r, prog.out)
    });
    let (res, _) = match tokio::time::timeout(tmo, job).await {
        Ok(j) => j.map_err(|e| AppError::Internal(e.to_string()))?,
        Err(_) => {
            handle.interrupt();
            return Ok((StatusCode::GATEWAY_TIMEOUT, "runtime error: Query timed out").into_response());
        }
    };
    let (elements, truncated) = res?;
    let mut body = json!({
        "version": 0.6,
        "generator": "rust-wmts overpass-duckdb",
        "osm3s": {"copyright": "The data included in this document is from www.openstreetmap.org. The data is made available under ODbL."},
        "elements": elements,
    });
    if truncated { body["remark"] = json!(format!("result truncated to {} elements", cfg.max_results)); }
    Ok(([(header::CACHE_CONTROL, "public, max-age=300")], axum::Json(body)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> OverpassConfig {
        OverpassConfig { db_path: String::new(), max_bbox_deg2: 4.0, max_results: 1000, timeout_seconds: 10, spatial_extension: None }
    }

    #[test]
    fn sql_is_parameterized() {
        let p = parser::parse(r#"[bbox:25,121,25.1,121.1];nwr["leisure"="x'; DROP TABLE osm_element;--"];out;"#).unwrap();
        let (sql, params) = build_sql(&p, &cfg()).unwrap();
        assert!(!sql.contains("DROP"));
        assert!(params.iter().any(|s| s.contains("DROP")));
        assert_eq!(sql.matches('?').count(), params.len());
    }

    #[test]
    fn bbox_required_and_capped() {
        let p = parser::parse("node[a=b];out;").unwrap();
        assert!(matches!(build_sql(&p, &cfg()), Err(AppError::BadRequest(_))));
        let p = parser::parse("[bbox:20,100,30,130];node[a=b];out;").unwrap();
        assert!(matches!(build_sql(&p, &cfg()), Err(AppError::RateLimited)));
    }
}

//! 名稱搜尋：`GET /api/v1/search`，查 DuckDB 的 `osm_name` 表（scripts/osm_build_names.py 建立）。
//!
//! 為什麼不用 FTS／n-gram 索引：全台 1,218 萬筆只有約 68.5 萬筆有名稱，獨立成表後 `contains()` 約 0.01 秒、
//! 編輯距離模糊比對約 0.07 秒（實測），不需要索引。
//!
//! 正規化（`norm`）必須與 scripts/osm_build_names.py 的 `norm()` 完全一致：全形 ASCII → 半形、全形空白 → 空白、
//! 小寫、空白折疊、簡繁逐字對照（t2s.tsv，由 OpenCC t2s 產生）。所以輸入「臺北」「台北」「台北」「Starbucks」「ＳＴＡＲＢＵＣＫＳ」都能互搜。

use crate::error::AppError;
use crate::state::SharedState;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

const T2S_TSV: &str = include_str!("t2s.tsv");
pub const MAX_Q_CHARS: usize = 40;
pub const MAX_LIMIT: usize = 100;

fn t2s() -> &'static HashMap<char, char> {
    static M: OnceLock<HashMap<char, char>> = OnceLock::new();
    M.get_or_init(|| {
        T2S_TSV
            .lines()
            .filter_map(|l| {
                let (a, b) = l.split_once('\t')?;
                Some((a.chars().next()?, b.chars().next()?))
            })
            .collect()
    })
}

pub fn norm(s: &str) -> String {
    let mapped: String = s
        .chars()
        .map(|c| match c as u32 {
            0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            0x3000 => ' ',
            _ => c,
        })
        .collect();
    let lower = mapped.to_lowercase();
    let folded = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    folded.chars().map(|c| *t2s().get(&c).unwrap_or(&c)).collect()
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
    /// s,w,n,e
    bbox: Option<String>,
    lat: Option<f64>,
    lon: Option<f64>,
    limit: Option<usize>,
    /// 精確搜尋沒結果時是否改用編輯距離模糊比對（預設 true）
    fuzzy: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct SearchParams {
    pub q: String,                     // 已正規化
    pub bbox: Option<[f64; 4]>,        // s,w,n,e
    pub near: Option<(f64, f64)>,      // lat,lon
    pub limit: usize,
    pub fuzzy: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub typ: String,
    pub id: i64,
    pub lat: f64,
    pub lon: f64,
    pub name: String,
    pub kind: String,
    pub cat: Option<String>,
}

pub fn parse_params(q: &SearchQuery) -> Result<SearchParams, AppError> {
    let raw = q.q.clone().unwrap_or_default();
    if raw.chars().count() > MAX_Q_CHARS {
        return Err(AppError::BadRequest(format!("q too long (max {MAX_Q_CHARS} characters)")));
    }
    let n = norm(&raw);
    if n.is_empty() {
        return Err(AppError::BadRequest("missing q".into()));
    }
    let bbox = match &q.bbox {
        None => None,
        Some(b) => {
            let v: Vec<f64> = b.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if v.len() != 4 || !v.iter().all(|x| x.is_finite()) || v[0] >= v[2] || v[1] >= v[3] || v[0] < -90.0 || v[2] > 90.0 {
                return Err(AppError::BadRequest("bbox must be s,w,n,e".into()));
            }
            Some([v[0], v[1], v[2], v[3]])
        }
    };
    let near = match (q.lat, q.lon) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() && (-90.0..=90.0).contains(&a) => Some((a, b)),
        (None, None) => None,
        _ => return Err(AppError::BadRequest("lat and lon must be given together".into())),
    };
    Ok(SearchParams { q: n, bbox, near, limit: q.limit.unwrap_or(30).clamp(1, MAX_LIMIT), fuzzy: q.fuzzy.unwrap_or(true) })
}

fn where_extra(p: &SearchParams) -> String {
    // 數值以字面量內嵌（f64 的 Display 只會是數字，且已驗證有限）
    match p.bbox {
        Some(b) => format!(" AND lat >= {} AND lat <= {} AND lon >= {} AND lon <= {}", b[0], b[2], b[1], b[3]),
        None => String::new(),
    }
}
fn dist_expr(p: &SearchParams) -> String {
    match p.near {
        Some((la, lo)) => {
            let c = la.to_radians().cos();
            format!("(lat - {la}) * (lat - {la}) + ((lon - {lo}) * {c}) * ((lon - {lo}) * {c})")
        }
        None => "lat * 0.0".into(), // 不能寫常數 0：ORDER BY 的整數常數會被當成欄位序號
    }
}

/// 回傳 (結果, 模式)：mode 為 "exact"（子字串）或 "fuzzy"（編輯距離）。
pub fn run_search(conn: &duckdb::Connection, p: &SearchParams) -> Result<(Vec<Hit>, &'static str), AppError> {
    let ie = |e: duckdb::Error| {
        let m = e.to_string();
        if m.contains("osm_name") { AppError::NotFound("name index not built (run scripts/osm_build_names.py)".into()) } else { AppError::Internal(m) }
    };
    let cols = "typ, id, lat, lon, coalesce(name, ''), kind, cat";
    let exact = format!(
        "SELECT {cols} FROM osm_name WHERE contains(hay, ?){} \
         ORDER BY CASE WHEN nname = ? THEN 0 WHEN starts_with(nname, ?) THEN 1 ELSE 2 END, {}, length(nname), id LIMIT {}",
        where_extra(p), dist_expr(p), p.limit
    );
    let mut hits = query(conn, &exact, &[&p.q, &p.q, &p.q]).map_err(ie)?;
    let mut mode = "exact";
    let n = p.q.chars().count();
    if hits.is_empty() && p.fuzzy && (2..=12).contains(&n) {
        // DuckDB 的 levenshtein 是以「位元組」計算（一個 CJK 字替換＝2～3），不能直接當字元距離用。
        // 做法：SQL 先用位元組距離 <= 4×容許字數 取候選（一個字元編輯最多改 4 個位元組，所以是安全的超集），
        // 再在這裡以字元為單位精算並排序。
        let maxd = if n >= 5 { 2 } else { 1 };
        // 預過濾（鴿籠原理）：把查詢切成 maxd+1 段，編輯距離 <= maxd 的名稱至少會完整含有其中一段，
        // 先用向量化的 contains() 篩掉絕大多數資料列，再對剩下的算 levenshtein（全表算約 100～500 ms，預過濾後約 30 ms）。
        let qc: Vec<char> = p.q.chars().collect();
        let pieces: Vec<String> = (0..=maxd).map(|i| qc[i * n / (maxd + 1)..(i + 1) * n / (maxd + 1)].iter().collect()).collect();
        let piece_cond = vec!["contains(nname, ?)"; pieces.len()].join(" OR ");
        let fuzzy = format!(
            "SELECT {cols}, nname FROM osm_name WHERE abs(length(nname) - {n}) <= {maxd} AND ({piece_cond}) AND levenshtein(nname, ?) <= {} {}              ORDER BY {}, id LIMIT 2000",
            4 * maxd, where_extra(p), dist_expr(p)
        );
        let mut args: Vec<&String> = pieces.iter().collect();
        args.push(&p.q);
        let mut cand = query_fuzzy(conn, &fuzzy, &args).map_err(ie)?;
        let mut scored: Vec<(usize, Hit)> = cand.into_iter()
            .filter_map(|(h, nn)| { let nc: Vec<char> = nn.chars().collect(); let d = lev_chars(&nc, &qc, maxd); (d <= maxd).then_some((d, h)) })
            .collect();
        scored.sort_by_key(|(d, _)| *d); // 穩定排序：距離相同者維持「近的在前」
        hits = scored.into_iter().map(|(_, h)| h).take(p.limit).collect();
        mode = "fuzzy";
    }
    Ok((hits, mode))
}

/// 以字元（不是位元組）計算的編輯距離。
pub fn char_lev(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    lev_chars(&a, &b, usize::MAX)
}

/// 編輯距離（字元陣列版）。`limit`：只關心距離是否 <= limit 時傳入，某一列最小值已超過就提早回傳 limit+1。
fn lev_chars(a: &[char], b: &[char], limit: usize) -> usize {
    if a.len().abs_diff(b.len()) > limit { return limit.saturating_add(1); }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
            row_min = row_min.min(cur[j]);
        }
        if row_min > limit { return limit.saturating_add(1); }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn query_fuzzy(conn: &duckdb::Connection, sql: &str, args: &[&String]) -> Result<Vec<(Hit, String)>, duckdb::Error> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(duckdb::params_from_iter(args.iter().map(|s| s.as_str())))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let h = Hit { typ: r.get(0)?, id: r.get(1)?, lat: r.get(2)?, lon: r.get(3)?, name: r.get(4)?, kind: r.get::<_, Option<String>>(5)?.unwrap_or_default(), cat: r.get(6)? };
        out.push((h, r.get::<_, Option<String>>(7)?.unwrap_or_default()));
    }
    Ok(out)
}

fn query(conn: &duckdb::Connection, sql: &str, params: &[&String]) -> Result<Vec<Hit>, duckdb::Error> {
    query_with(conn, sql, params)
}
fn query_with(conn: &duckdb::Connection, sql: &str, params: &[&String]) -> Result<Vec<Hit>, duckdb::Error> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(duckdb::params_from_iter(params.iter().map(|s| s.as_str())))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(Hit { typ: r.get(0)?, id: r.get(1)?, lat: r.get(2)?, lon: r.get(3)?, name: r.get(4)?, kind: r.get::<_, Option<String>>(5)?.unwrap_or_default(), cat: r.get(6)? });
    }
    Ok(out)
}

pub fn hits_json(q: &str, normalized: &str, mode: &str, hits: &[Hit], took_ms: u128) -> Value {
    json!({
        "query": q, "normalized": normalized, "mode": mode, "count": hits.len(), "took_ms": took_ms,
        "elements": hits.iter().map(|h| json!({"type": h.typ, "id": h.id, "lat": h.lat, "lon": h.lon, "name": h.name, "kind": h.kind, "cat": h.cat})).collect::<Vec<_>>(),
        "attribution": "© OpenStreetMap contributors (ODbL)",
    })
}

pub async fn search_get(State(st): State<SharedState>, Query(q): Query<SearchQuery>) -> Result<Response, AppError> {
    let (Some(cfg), Some(base)) = (st.config.overpass.clone(), st.osm.as_ref()) else {
        return Err(AppError::NotFound("search endpoint is not enabled".into()));
    };
    let p = parse_params(&q)?;
    let raw = q.q.clone().unwrap_or_default();
    let conn = base.lock().map_err(|_| AppError::Internal("db lock poisoned".into()))?.try_clone().map_err(|e| AppError::Internal(e.to_string()))?;
    let handle = conn.interrupt_handle();
    let tmo = Duration::from_secs(cfg.timeout_seconds.max(1));
    let norm_q = p.q.clone();
    let t0 = std::time::Instant::now();
    let job = tokio::task::spawn_blocking(move || run_search(&conn, &p));
    let (hits, mode) = match tokio::time::timeout(tmo, job).await {
        Ok(j) => j.map_err(|e| AppError::Internal(e.to_string()))??,
        Err(_) => {
            handle.interrupt();
            return Err(AppError::Internal("search timed out".into()));
        }
    };
    let body = hits_json(&raw, &norm_q, mode, &hits, t0.elapsed().as_millis());
    Ok(([(header::CACHE_CONTROL, "public, max-age=300")], axum::Json(body)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_fullwidth_case_space_and_t2s() {
        assert_eq!(norm("ＳＴＡＲＢＵＣＫＳ　Coffee"), "starbucks coffee");
        assert_eq!(norm("  7-ELEVEN   大安店 "), "7-eleven 大安店");
        assert_eq!(norm("臺北車站"), norm("台北車站"));           // 臺→台（OpenCC）
        assert_eq!(norm("學校"), norm("学校"));
        assert_eq!(norm("咖啡館"), norm("咖啡馆"));
        assert_eq!(norm("台北"), "台北");
        assert_eq!(norm("a\nb"), "a b");                         // 換行折疊，查詢不能跨欄位比對
    }

    /// 與 scripts/test_mapsearch_core.js（normName）、scripts/test_name_norm.py（norm）使用同一組樣本與期望值。
    #[test]
    fn norm_matches_python_and_js_samples() {
        for (input, want) in [
            ("ＳＴＡＲＢＵＣＫＳ　Coffee", "starbucks coffee"), ("  7-ELEVEN   大安店 ", "7-eleven 大安店"), ("臺北車站", "台北车站"),
            ("學校", "学校"), ("咖啡馆", "咖啡馆"), ("a
b", "a b"), ("Ｔｅｓｔ１２３", "test123"), ("", ""),
        ] {
            assert_eq!(norm(input), want, "{input:?}");
        }
    }

    fn db() -> duckdb::Connection {
        let c = duckdb::Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE osm_name (typ VARCHAR, id BIGINT, lon DOUBLE, lat DOUBLE, name VARCHAR, nname VARCHAR, hay VARCHAR, kind VARCHAR, cat VARCHAR);").unwrap();
        let rows = [
            ("node", 1, 121.5360, 25.0336, "星巴克", "starbucks coffee", "amenity=cafe", Some("cafe")),
            ("node", 2, 121.5400, 25.0340, "星巴克大安門市", "", "amenity=cafe", Some("cafe")),
            ("node", 3, 121.5000, 25.0000, "老星巴克咖啡館", "", "amenity=cafe", Some("cafe")),
            ("way", 4, 121.5365, 25.0338, "臺北市立圖書館", "", "amenity=library", Some("library")),
            ("node", 5, 120.3000, 22.6000, "星巴克", "", "amenity=cafe", Some("cafe")),   // 高雄
            ("node", 6, 121.5361, 25.0337, "x'; DROP TABLE osm_name;--", "", "", None),
        ];
        for (t, id, lon, lat, name, en, kind, cat) in rows {
            let hay = format!("{}\n{}", norm(name), norm(en));
            let cat_sql = cat.map(|c| format!("'{c}'")).unwrap_or("NULL".into());
            c.execute_batch(&format!(
                "INSERT INTO osm_name VALUES ('{t}', {id}, {lon}, {lat}, '{}', '{}', '{}', '{kind}', {cat_sql});",
                name.replace('\'', "''"), norm(name).replace('\'', "''"), hay.replace('\'', "''")
            )).unwrap();
        }
        c
    }
    fn p(q: &str) -> SearchParams {
        parse_params(&SearchQuery { q: Some(q.into()), bbox: None, lat: None, lon: None, limit: None, fuzzy: None }).unwrap()
    }

    #[test]
    fn ranks_exact_then_prefix_then_contains_then_distance() {
        let c = db();
        let mut sp = p("星巴克");
        sp.near = Some((25.0336, 121.5360));
        let (h, mode) = run_search(&c, &sp).unwrap();
        assert_eq!(mode, "exact");
        let ids: Vec<i64> = h.iter().map(|x| x.id).collect();
        assert_eq!(ids, vec![1, 5, 2, 3], "完全相同（近的在前）→ 前綴 → 包含");
    }

    #[test]
    fn traditional_simplified_fullwidth_and_english_variants_match() {
        let c = db();
        assert_eq!(run_search(&c, &p("台北市立图书馆")).unwrap().0[0].id, 4);   // 簡體輸入找繁體資料
        assert_eq!(run_search(&c, &p("臺北市立圖書館")).unwrap().0[0].id, 4);
        assert_eq!(run_search(&c, &p("ＳＴＡＲＢＵＣＫＳ")).unwrap().0.iter().map(|x| x.id).next(), Some(1)); // 全形英文、name:en
        assert_eq!(run_search(&c, &p("咖啡馆")).unwrap().0[0].id, 3);
    }

    #[test]
    fn bbox_filters_and_limit() {
        let c = db();
        let mut sp = p("星巴克");
        sp.bbox = Some([25.0, 121.5, 25.1, 121.6]);
        let (h, _) = run_search(&c, &sp).unwrap();
        assert!(h.iter().all(|x| x.lat >= 25.0) && !h.iter().any(|x| x.id == 5));
        sp.limit = 1;
        assert_eq!(run_search(&c, &sp).unwrap().0.len(), 1);
    }

    #[test]
    fn fuzzy_only_when_exact_is_empty() {
        let c = db();
        let (h, mode) = run_search(&c, &p("星巴可")).unwrap();   // 打錯一個字
        assert_eq!(mode, "fuzzy");
        assert!(h.iter().any(|x| x.id == 1));
        let mut off = p("星巴可"); off.fuzzy = false;
        assert!(run_search(&c, &off).unwrap().0.is_empty());
        assert_eq!(run_search(&c, &p("星巴克")).unwrap().1, "exact");
    }

    #[test]
    fn char_level_edit_distance() {
        assert_eq!(char_lev("星巴克", "星巴可"), 1);
        assert_eq!(char_lev("星巴克", "星克"), 1);
        assert_eq!(char_lev("", "abc"), 3);
        assert_eq!(char_lev("kitten", "sitting"), 3);
    }

    #[test]
    fn parameters_are_bound_not_interpolated() {
        let c = db();
        let sp = p("x'; DROP TABLE osm_name;--");
        let (h, _) = run_search(&c, &sp).unwrap();
        assert_eq!(h[0].id, 6);
        assert_eq!(run_search(&c, &p("星巴克")).unwrap().0.len() > 0, true, "資料表仍在");
    }

    #[test]
    fn parse_params_validation() {
        let mk = |q: &str, bbox: Option<&str>, lat: Option<f64>, lon: Option<f64>| SearchQuery { q: Some(q.into()), bbox: bbox.map(String::from), lat, lon, limit: Some(9999), fuzzy: None };
        assert!(parse_params(&mk("  ", None, None, None)).is_err());
        assert!(parse_params(&mk(&"x".repeat(41), None, None, None)).is_err());
        assert!(parse_params(&mk("a", Some("1,2,3"), None, None)).is_err());
        assert!(parse_params(&mk("a", Some("5,2,3,4"), None, None)).is_err());          // s >= n
        assert!(parse_params(&mk("a", Some("1,2,NaN,4"), None, None)).is_err());
        assert!(parse_params(&mk("a", None, Some(25.0), None)).is_err());
        let ok = parse_params(&mk("A", Some("25,121,26,122"), Some(25.5), Some(121.5))).unwrap();
        assert_eq!(ok.limit, MAX_LIMIT); assert_eq!(ok.q, "a");
    }
}

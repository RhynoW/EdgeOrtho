use super::{capabilities, get_tile, TileRequest};
use crate::state::SharedState;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;

/// WMTS KVP 介面：`GET /wmts?SERVICE=WMTS&REQUEST=...`
/// OGC KVP 參數名稱大小寫不區分，故手動解析並統一轉為大寫 key 比對，
/// 不採用 axum 的強型別 Query extractor（避免因客戶端大小寫差異導致解析失敗）。
pub async fn handler(State(state): State<SharedState>, RawQuery(raw): RawQuery) -> Response {
    let params = parse_kvp(raw.as_deref().unwrap_or(""));
    let get = |k: &str| params.get(k).cloned();

    match get("REQUEST").as_deref() {
        Some("GetCapabilities") => {
            let xml = capabilities::build_capabilities_xml(&state.config);
            (StatusCode::OK, [("Content-Type", "text/xml")], xml).into_response()
        }
        Some("GetTile") => {
            let layer = match get("LAYER") {
                Some(v) => v,
                None => return crate::error::ows_exception(StatusCode::BAD_REQUEST, "MissingParameterValue", "LAYER"),
            };
            let tms = match get("TILEMATRIXSET") {
                Some(v) => v,
                None => {
                    return crate::error::ows_exception(StatusCode::BAD_REQUEST, "MissingParameterValue", "TILEMATRIXSET")
                }
            };
            let tile_matrix = match get("TILEMATRIX") {
                Some(v) => v,
                None => {
                    return crate::error::ows_exception(StatusCode::BAD_REQUEST, "MissingParameterValue", "TILEMATRIX")
                }
            };
            let row = match get("TILEROW").and_then(|v| v.parse::<u32>().ok()) {
                Some(v) => v,
                None => return crate::error::ows_exception(StatusCode::BAD_REQUEST, "MissingParameterValue", "TILEROW"),
            };
            let col = match get("TILECOL").and_then(|v| v.parse::<u32>().ok()) {
                Some(v) => v,
                None => return crate::error::ows_exception(StatusCode::BAD_REQUEST, "MissingParameterValue", "TILECOL"),
            };
            let format = get("FORMAT");

            get_tile(
                &state,
                TileRequest {
                    layer,
                    tile_matrix_set: tms,
                    tile_matrix,
                    tile_row: row,
                    tile_col: col,
                    format,
                },
            )
            .await
        }
        _ => crate::error::ows_exception(
            StatusCode::BAD_REQUEST,
            "InvalidParameterValue",
            "REQUEST must be GetCapabilities or GetTile",
        ),
    }
}

fn parse_kvp(raw: &str) -> HashMap<String, String> {
    raw.split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            let mut it = pair.splitn(2, '=');
            let k = it.next()?;
            let v = it.next().unwrap_or("");
            let decoded_v = urlencoding_decode(v);
            Some((k.to_uppercase(), decoded_v))
        })
        .collect()
}

fn urlencoding_decode(s: &str) -> String {
    percent_decode(s.replace('+', " ").as_bytes())
}

fn percent_decode(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

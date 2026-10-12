pub mod capabilities;
pub mod kvp;
pub mod restful;

use crate::state::SharedState;
use crate::tilematrix::{resolve_levels, validate_tile_index};
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::path::PathBuf;

pub struct TileRequest {
    pub layer: String,
    pub tile_matrix_set: String,
    pub tile_matrix: String,
    pub tile_row: u32,
    pub tile_col: u32,
    pub format: Option<String>,
}

/// GetTile 的共用實作，供 KVP 與 RESTful 兩種介面呼叫。
/// 圖磚檔案存放慣例：`{layer.tile_dir}/{tilematrixset}/{tile_matrix}/{tile_col}/{tile_row}.png`
pub async fn get_tile(state: &SharedState, req: TileRequest) -> Response {
    let layer = match state.config.layer(&req.layer) {
        Some(l) => l,
        None => return ows_error(StatusCode::NOT_FOUND, "InvalidParameterValue", "unknown layer"),
    };

    if !layer.tilematrixsets.iter().any(|t| t == &req.tile_matrix_set) {
        return ows_error(
            StatusCode::BAD_REQUEST,
            "InvalidParameterValue",
            "layer does not support requested TileMatrixSet",
        );
    }

    if let Some(fmt) = &req.format {
        if fmt != &layer.format {
            return ows_error(StatusCode::BAD_REQUEST, "InvalidParameterValue", "unsupported FORMAT");
        }
    }

    let tms = match state.config.tilematrixset(&req.tile_matrix_set) {
        Some(t) => t,
        None => return ows_error(StatusCode::NOT_FOUND, "InvalidParameterValue", "unknown TileMatrixSet"),
    };
    let levels = resolve_levels(tms);
    let matrix = match validate_tile_index(&levels, &req.tile_matrix, req.tile_row, req.tile_col) {
        Some(m) => m,
        None => return ows_error(StatusCode::BAD_REQUEST, "TileOutOfRange", "row/col outside TileMatrix bounds"),
    };

    let bytes = if let Some(gpkg) = &layer.gpkg {
        let gpkg = gpkg.clone();
        let zoom: u32 = req.tile_matrix.parse().unwrap_or(0);
        let (row, col) = (req.tile_row, req.tile_col);
        // WMTS 請求帶的是 TileMatrixSet 定義的全域 row/col；GeoPackage 裡存的
        // 是該檔案自己的局部 row/col（見 GpkgSourceConfig 文件），兩者要換算。
        let tile_size = matrix.tile_width as f64 * matrix.resolution;
        let offset_col = ((gpkg.local_origin_x - matrix.top_left_corner[0]) / tile_size).floor() as i64;
        let offset_row = ((matrix.top_left_corner[1] - gpkg.local_origin_y) / tile_size).floor() as i64;
        let local_col = col as i64 - offset_col;
        let local_row = row as i64 - offset_row;
        if local_col < 0 || local_row < 0 {
            None
        } else {
            let (local_col, local_row) = (local_col as u32, local_row as u32);
            tokio::task::spawn_blocking(move || read_gpkg_tile(&gpkg, zoom, local_row, local_col))
                .await
                .ok()
                .and_then(|r| r.ok())
        }
    } else if let Some(tile_dir) = &layer.tile_dir {
        let path: PathBuf = [
            tile_dir.as_str(),
            req.tile_matrix_set.as_str(),
            req.tile_matrix.as_str(),
            req.tile_col.to_string().as_str(),
            &format!("{}.png", req.tile_row),
        ]
        .iter()
        .collect();
        tokio::fs::read(&path).await.ok()
    } else {
        tracing::error!("layer '{}' has neither tile_dir nor gpkg configured", layer.identifier);
        None
    };

    let cache_control = format!("public, max-age={}", layer.cache_seconds.unwrap_or(86400));
    match bytes {
        Some(bytes) => (
            StatusCode::OK,
            [
                ("Content-Type", "image/png"),
                ("Cache-Control", cache_control.as_str()),
            ],
            Body::from(bytes),
        )
            .into_response(),
        None => ows_error(StatusCode::NOT_FOUND, "TileNotFound", "requested tile does not exist"),
    }
}

/// 從 GeoPackage 的 tiles extension 資料表讀一張圖磚。row/col 直接對應
/// GeoPackage 的 tile_row/tile_column，兩者都是「row 0 在北（上）」的慣例
/// （已用 gpkg/Xiamen.gpkg 實測驗證，見 config.rs 的 GpkgSourceConfig 文件）。
/// 每次請求開一個唯讀連線：GeoPackage 是本機檔案、索引查詢很快，這支功能
/// 目前定位是示範/中量流量用途，還沒做連線池化，見 ARCHITECTURE.md。
fn read_gpkg_tile(gpkg: &crate::config::GpkgSourceConfig, zoom: u32, row: u32, col: u32) -> anyhow::Result<Vec<u8>> {
    let conn = rusqlite::Connection::open_with_flags(&gpkg.path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // gpkg.table 來自設定檔（管理者可信輸入，不是 HTTP 請求參數），沒有 SQL
    // injection 風險；圖磚座標本身走參數化綁定。
    let sql = format!("SELECT tile_data FROM {} WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3", gpkg.table);
    let bytes: Vec<u8> = conn.query_row(&sql, rusqlite::params![zoom, col, row], |r| r.get(0))?;
    Ok(bytes)
}

fn ows_error(status: StatusCode, code: &str, text: &str) -> Response {
    crate::error::ows_exception(status, code, text)
}

use super::{capabilities, get_tile, TileRequest};
use crate::state::SharedState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub async fn get_capabilities_xml(State(state): State<SharedState>) -> Response {
    let xml = capabilities::build_capabilities_xml(&state.config);
    (StatusCode::OK, [("Content-Type", "text/xml")], xml).into_response()
}

pub async fn get_capabilities_json(State(state): State<SharedState>) -> Response {
    axum::Json(capabilities::build_capabilities_json(&state.config)).into_response()
}

/// RESTful GetTile：`/wmts/1.0.0/{Layer}/{Style}/{TileMatrixSet}/{TileMatrix}/{TileRow}/{TileCol}.png`
/// Style 目前僅支援 "default"，保留於路徑中以符合 OGC RESTful 樣板規範。
pub async fn get_tile_restful(
    State(state): State<SharedState>,
    Path((layer, _style, tms, tile_matrix, tile_row, tile_col_ext)): Path<(String, String, String, String, u32, String)>,
) -> Response {
    let Some(tile_col_str) = tile_col_ext.strip_suffix(".png") else {
        return crate::error::ows_exception(
            StatusCode::BAD_REQUEST,
            "InvalidParameterValue",
            "tile resource must end with .png",
        );
    };
    let Ok(tile_col) = tile_col_str.parse::<u32>() else {
        return crate::error::ows_exception(StatusCode::BAD_REQUEST, "InvalidParameterValue", "invalid TileCol");
    };

    get_tile(
        &state,
        TileRequest {
            layer,
            tile_matrix_set: tms,
            tile_matrix,
            tile_row,
            tile_col,
            format: Some("image/png".to_string()),
        },
    )
    .await
}

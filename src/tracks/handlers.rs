use super::export;
use super::model::{FeatureCollection, UploadSummary};
use super::store;
use crate::error::AppError;
use crate::state::SharedState;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct FormatQuery {
    format: Option<String>,
}

/// 無人機飛行軌跡上傳端點：`POST /api/v1/tracks/{session_id}/points`
/// body 為 RFC 7946 GeoJSON FeatureCollection。手持遙控器可於斷網時本機緩存，
/// 網路恢復後對同一 session_id 重送（含先前已送過的點），伺服器以 seq 去重，
/// 不會產生重複資料，符合「一鍵自動同步」情境下的冪等性要求。
pub async fn upload_points(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
    Json(body): Json<FeatureCollection>,
) -> Result<Json<UploadSummary>, AppError> {
    if body.type_ != "FeatureCollection" {
        return Err(AppError::BadRequest("body must be a GeoJSON FeatureCollection".into()));
    }
    if body.features.is_empty() {
        return Err(AppError::BadRequest("features must not be empty".into()));
    }

    let conn = state.db.lock().await;
    let (accepted, duplicate) = store::insert_points(&conn, &session_id, &body.features)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(UploadSummary { session_id, accepted, duplicate }))
}

/// 取回單一飛行工作階段軌跡，預設回傳 GeoJSON；`?format=kml` 或 `?format=gpx`
/// 可取得標準格式匯出，供不支援 GeoJSON 的下游系統使用。
pub async fn get_track(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
    Query(q): Query<FormatQuery>,
) -> Result<Response, AppError> {
    let conn = state.db.lock().await;
    let points = store::list_points(&conn, &session_id).map_err(|e| AppError::Internal(e.to_string()))?;
    drop(conn);

    if points.is_empty() {
        return Err(AppError::NotFound(format!("no track found for session_id '{session_id}'")));
    }

    match q.format.as_deref() {
        None | Some("geojson") => {
            let features: Vec<_> = points
                .iter()
                .map(|p| {
                    json!({
                        "type": "Feature",
                        "geometry": p.geometry,
                        "properties": p.properties,
                    })
                })
                .collect();
            Ok(Json(json!({ "type": "FeatureCollection", "features": features })).into_response())
        }
        Some("kml") => {
            let kml = export::to_kml(&session_id, &points);
            Ok((axum::http::StatusCode::OK, [("Content-Type", "application/vnd.google-earth.kml+xml")], kml).into_response())
        }
        Some("gpx") => {
            let gpx = export::to_gpx(&session_id, &points);
            Ok((axum::http::StatusCode::OK, [("Content-Type", "application/gpx+xml")], gpx).into_response())
        }
        Some(other) => Err(AppError::BadRequest(format!("unsupported format '{other}', use geojson|kml|gpx"))),
    }
}

pub async fn list_tracks(State(state): State<SharedState>) -> Result<Response, AppError> {
    let conn = state.db.lock().await;
    let sessions = store::list_sessions(&conn).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(sessions).into_response())
}

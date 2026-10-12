use super::model::{ShareAck, ShareRequest};
use super::store;
use crate::error::AppError;
use crate::state::SharedState;
use axum::extract::{ConnectInfo, State};
use axum::http::HeaderMap;
use axum::Json;
use std::net::SocketAddr;

const MAX_URL_LEN: usize = 4096;
const MAX_MESSAGE_LEN: usize = 2000;

/// 連線位址取自 TCP socket peer；`behind_reverse_proxy = true` 部署時那會是
/// 前方 nginx/Caddy 自己的位址，不是真正訪客 IP，此時改信任反代加上的
/// `X-Forwarded-For`（取第一個，即最原始來源）。這個端點只是留存紀錄用，
/// 不是拿 IP 做存取控制判斷，相信 X-Forwarded-For 在這裡不構成安全疑慮。
fn client_ip(headers: &HeaderMap, socket_addr: SocketAddr) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| socket_addr.ip().to_string())
}

/// `POST /api/v1/shares`：前端「分享目前畫面」按鈕呼叫，留存時間／連線IP／
/// 分享網址／留言。刻意不放進 `require_auth` 保護的路由群組——這是給
/// 匿名訪客也能用的檢視器小工具（跟座標複製/量距一樣），要求先申請 JWT/API
/// Key 才能分享網址不合理。因此也不含存取控制，只做基本長度驗證，避免
/// 顯著的濫用/塞爆資料庫。
pub async fn create_share(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<ShareRequest>,
) -> Result<Json<ShareAck>, AppError> {
    let url = body.url.trim();
    if url.is_empty() {
        return Err(AppError::BadRequest("url must not be empty".into()));
    }
    if url.len() > MAX_URL_LEN {
        return Err(AppError::BadRequest(format!("url too long (max {MAX_URL_LEN} bytes)")));
    }
    let message = body.message.as_deref().map(str::trim).filter(|m| !m.is_empty());
    if let Some(m) = message {
        if m.len() > MAX_MESSAGE_LEN {
            return Err(AppError::BadRequest(format!("message too long (max {MAX_MESSAGE_LEN} bytes)")));
        }
    }

    let ip = client_ip(&headers, addr);
    let conn = state.db.lock().await;
    let id = store::insert_share(&conn, &ip, url, message).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(ShareAck { id }))
}

/// `GET /api/v1/shares`：瀏覽留存紀錄（時間/連線IP/分享網址/留言），跟寫入端點
/// 不同，這個掛在 `require_auth` 保護的路由群組下（見 main.rs），需要 JWT
/// 或 API Key 才能讀取——連線 IP 屬於個資，不能讓任何人匿名查看。
pub async fn list_shares(State(state): State<SharedState>) -> Result<Json<Vec<super::model::ShareRecord>>, AppError> {
    let conn = state.db.lock().await;
    let records = store::list_shares(&conn).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(records))
}

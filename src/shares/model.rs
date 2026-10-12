use serde::{Deserialize, Serialize};

/// `POST /api/v1/shares` 的請求 body：前端「分享目前畫面」按鈕產生分享連結時
/// 一併送出，`url` 是完整分享網址（含 lat/lng/z/layer/pins 參數），`message`
/// 是使用者選填的留言。
#[derive(Debug, Deserialize)]
pub struct ShareRequest {
    pub url: String,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ShareAck {
    pub id: i64,
}

/// `GET /api/v1/shares` 回傳的單筆紀錄；這個端點走 `require_auth`（JWT 或
/// API Key），跟寫入端點（匿名可用）刻意分開權限。
#[derive(Debug, Serialize)]
pub struct ShareRecord {
    pub id: i64,
    pub created_at: String,
    pub ip: String,
    pub url: String,
    pub message: Option<String>,
}

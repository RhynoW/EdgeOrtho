use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 上傳格式為 RFC 7946 GeoJSON FeatureCollection，每個 Feature 代表一筆遙測點。
/// `properties.seq` 為客戶端於本次飛行工作階段(session)內產生的單調遞增序號，
/// 是離線緩存 / 網路恢復後自動同步（Auto-Sync）能夠冪等（重送不重複）的關鍵：
/// 伺服器以 (session_id, seq) 為唯一鍵去重，斷線重連後重送已上傳過的點不會造成
/// 重複資料，也不會因為分批上傳順序不同而遺漏。
#[derive(Debug, Deserialize)]
pub struct FeatureCollection {
    #[serde(rename = "type")]
    pub type_: String,
    pub features: Vec<Feature>,
}

#[derive(Debug, Deserialize)]
pub struct Feature {
    #[serde(rename = "type")]
    pub type_: String,
    pub geometry: Value,
    pub properties: TrackPointProperties,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TrackPointProperties {
    pub seq: u64,
    pub drone_id: String,
    /// RFC 3339 時間戳。
    pub timestamp: String,
    pub heading_deg: Option<f64>,
    pub attitude: Option<Attitude>,
    pub battery_percent: Option<f64>,
    pub rtk_status: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Attitude {
    pub roll_deg: f64,
    pub pitch_deg: f64,
    pub yaw_deg: f64,
}

#[derive(Debug, Serialize)]
pub struct UploadSummary {
    pub session_id: String,
    pub accepted: usize,
    pub duplicate: usize,
}

#[derive(Debug, Serialize)]
pub struct TrackPointRow {
    pub seq: u64,
    pub geometry: Value,
    pub properties: TrackPointProperties,
}

#[derive(Debug, Serialize)]
pub struct TrackSummary {
    pub session_id: String,
    pub drone_id: String,
    pub point_count: i64,
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
    pub updated_at: String,
}

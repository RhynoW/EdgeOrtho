use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub bind: String,
    /// 若在 nginx/Caddy 等反向代理後方運行（由代理層負責 TLS 1.3 termination），
    /// 設為 true 則本服務只綁定明文 HTTP，tls_cert/tls_key 會被忽略。
    #[serde(default)]
    pub behind_reverse_proxy: bool,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub public_base_url: String,
    #[serde(default = "default_db_path")]
    pub db_path: String,
}

fn default_db_path() -> String {
    "wmts_tracks.sqlite3".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct SecurityConfig {
    pub jwt_secret_env: String,
    pub jwt_issuer: String,
    pub token_ttl_seconds: i64,
    pub allowed_origins: Vec<String>,
    pub allowed_referers: Vec<String>,
    pub api_key_rate_limit_per_minute: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ApiKeyConfig {
    pub key_env: String,
    pub label: String,
    /// `client`（預設）／`uploader`／`admin`，見 auth::Role。
    #[serde(default)]
    pub role: crate::auth::Role,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TileMatrixSetConfig {
    pub identifier: String,
    pub crs: String,
    pub well_known_scale_set: Option<String>,
    pub tile_width: u32,
    pub tile_height: u32,
    pub top_left_corner: [f64; 2],
    /// GoogleMapsCompatible 系列可用 min_zoom/max_zoom 以公式推導。
    pub min_zoom: Option<u32>,
    pub max_zoom: Option<u32>,
    /// 非公式格網（如 NLSC TWD97/TM2、GeoPackage 匯入的資料集）須明列每一
    /// zoom level 的 [resolution, matrix_width, matrix_height]（單位跟著
    /// `crs` 走：投影座標系用公尺，地理座標系如 EPSG:4326 用度；後者目前
    /// GetCapabilities 算出來的 ScaleDenominator 不是嚴格正確的度量單位換算，
    /// 只是沿用同一個公式，屬已知限制——不影響圖磚讀取本身）。
    pub levels: Option<Vec<[f64; 3]>>,
    /// `levels` 陣列第一筆對應的 zoom level；預設 0。像 GeoPackage 匯入的
    /// 資料集常常只有 z14 以上有圖磚，不需要為 z0-z13 塞一堆沒有資料的
    /// 佔位項目。
    #[serde(default)]
    pub start_zoom: u32,
}

/// 圖磚來源是 GeoPackage（OGC 標準的 tiles extension，本質是 SQLite）而非
/// 一般攤平檔案時使用；見 examples/read_gpkg.rs 的探索性驗證與
/// ARCHITECTURE.md 對應章節。row/col 直接對應 GeoPackage 的 tile_row/
/// tile_column，兩者都是「row 0 在北（上）」的慣例，不用做座標翻轉
/// （已用 gpkg/Xiamen.gpkg 實測驗證：tile_row=0 對應資料集北側的廈門高崎
/// 機場一帶，不是南側）。
///
/// `local_origin_x`/`local_origin_y`：GDAL/QGIS 匯出 GeoPackage 圖磚時，即使
/// CRS/解析度跟 WMTS 的 TileMatrixSet 公式完全一致（例如都用標準 Web Mercator
/// 全球格網公式），`tile_column`/`tile_row` 通常還是從 0 開始的**局部**索引
/// （只涵蓋該資料集實際範圍那一小塊），不是 TileMatrixSet 定義的**全域**
/// TileCol/TileRow（已用 gpkg/Xiamen_3857.gpkg 實測驗證：z17 存的
/// tile_column 範圍是 0..17，但這塊地理範圍對應的全域 Web Mercator column
/// 其實是 108538..108794）。這兩個欄位記錄該 GeoPackage 局部 (0,0) 圖磚
/// 左上角對應的真實世界座標（直接抄 `gpkg_contents` 的 min_x/max_y），
/// 讀取時用來換算「WMTS 請求的全域 row/col」→「這個 GeoPackage 檔案裡的
/// 局部 row/col」，讓它能正確疊在標準格網的其他圖層上，不用像
/// EPSG:4326、非標準格網（`gpkg/Xiamen.gpkg`）那樣得另外開自訂 CRS 面板。
#[derive(Debug, Deserialize, Clone)]
pub struct GpkgSourceConfig {
    pub path: String,
    /// gpkg_contents 裡 data_type='tiles' 的 table_name（例如 "tile_data"）。
    pub table: String,
    pub local_origin_x: f64,
    pub local_origin_y: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LayerConfig {
    pub identifier: String,
    pub title: String,
    pub abstract_text: Option<String>,
    pub tilematrixsets: Vec<String>,
    pub format: String,
    /// 攤平檔案來源：{tile_dir}/{tms}/{z}/{col}/{row}.png。跟 `gpkg` 二選一，
    /// 有設定 `gpkg` 的話優先用 `gpkg`，這個欄位會被忽略。
    #[serde(default)]
    pub tile_dir: Option<String>,
    /// GeoPackage 來源；設定了就優先於 `tile_dir`。
    #[serde(default)]
    pub gpkg: Option<GpkgSourceConfig>,
    pub bbox_wgs84: [f64; 4],
    /// GetTile 回應的 Cache-Control max-age（秒）；預設 86400。會被持續更新的即時圖層
    /// （如 ingest 輸出）要設很小，否則瀏覽器與 CDN 會一直拿到舊圖磚。
    #[serde(default)]
    pub cache_seconds: Option<u64>,
}

/// 稽核紀錄設定。`retention_days = 0`（預設）＝不自動清除；稽核資料不宜默默消失，要清除請明確設定並先匯出。
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AuditConfig {
    #[serde(default)]
    pub retention_days: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub security: SecurityConfig,
    #[serde(default)]
    pub api_keys: Vec<ApiKeyConfig>,
    pub tilematrixsets: Vec<TileMatrixSetConfig>,
    pub layers: Vec<LayerConfig>,
    #[serde(default)]
    pub audit: AuditConfig,
    /// 可選：Overpass 相容查詢端點（DuckDB + R-tree），未設定則不啟用。
    #[cfg(feature = "overpass")]
    #[serde(default)]
    pub overpass: Option<crate::overpass::OverpassConfig>,
    /// 可選：無人機影像上傳與增量鑲嵌（cargo feature `ingest`），未設定則不啟用。
    #[cfg(feature = "ingest")]
    #[serde(default)]
    pub ingest: Option<crate::ingest::IngestConfig>,
}

impl AppConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = fs::read_to_string(path)?;
        let mut cfg: AppConfig = toml::from_str(&raw)?;
        // 允許用環境變數覆寫 public_base_url：部署到 HF Space 等平台時，實際對外網址
        // 要到建立 Space 後才知道，不想為此重新編譯映像檔或改設定檔再重新部署。
        // 同時覆寫 allowed_origins/allowed_referers（見 README.md 的說明）：
        // static/index.html 這個內建輕量檢視器是從這個 base_url 同源載入的，
        // 瀏覽器對 /wmts 發出的 fetch() 會帶這個網址當 Origin/Referer，
        // 設定檔內殘留的佔位網址（如 layers.hfspace.toml 的 example.hf.space）
        // 會被 auth::check_origin_and_referer 擋下來，所以兩者要跟著一起換。
        if let Ok(base_url) = std::env::var("WMTS_PUBLIC_BASE_URL") {
            cfg.server.public_base_url = base_url.clone();
            cfg.security.allowed_origins = vec![base_url.clone()];
            cfg.security.allowed_referers = vec![base_url];
        }
        Ok(cfg)
    }

    pub fn tilematrixset(&self, identifier: &str) -> Option<&TileMatrixSetConfig> {
        self.tilematrixsets.iter().find(|t| t.identifier == identifier)
    }

    pub fn layer(&self, identifier: &str) -> Option<&LayerConfig> {
        self.layers.iter().find(|l| l.identifier == identifier)
    }
}

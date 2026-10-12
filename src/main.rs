mod audit;
mod auth;
mod config;
mod error;
mod health;
#[cfg(feature = "ingest")]
mod ingest;
#[cfg(feature = "overpass")]
mod overpass;
mod shares;
mod state;
mod tilematrix;
mod tracks;
mod wmts;
mod xmlutil;

use axum::extract::Request;
use axum::http::{HeaderValue, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{middleware, Router};
use config::AppConfig;
use state::{AppState, SharedState};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    // axum-server 使用 tls-rustls-no-provider（見 Cargo.toml 註解），需在啟動時
    // 手動安裝 rustls 的加密後端；選用 ring 而非預設 aws-lc-rs，避免建置時依賴
    // 本機未安裝的 cmake/nasm。
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("failed to install rustls ring crypto provider"))?;

    let config_path = std::env::var("WMTS_CONFIG").unwrap_or_else(|_| "config/layers.toml".to_string());
    let config = AppConfig::load(&PathBuf::from(&config_path))?;

    let jwt_secret = std::env::var(&config.security.jwt_secret_env).map_err(|_| {
        anyhow::anyhow!(
            "environment variable '{}' (security.jwt_secret_env) is not set",
            config.security.jwt_secret_env
        )
    })?;

    let mut api_keys: HashMap<String, String> = HashMap::new();
    let mut api_key_roles: HashMap<String, auth::Role> = HashMap::new();
    for k in &config.api_keys {
        match std::env::var(&k.key_env) {
            Ok(v) => {
                api_key_roles.insert(v.clone(), k.role);
                api_keys.insert(v, k.label.clone());
            }
            Err(_) => {
                tracing::warn!("api key env '{}' not set, skipping ({})", k.key_env, k.label);
            }
        }
    }

    let db = tracks::store::init_db(&config.server.db_path)?;
    shares::store::init_db(&db)?;
    audit::init_db(&db)?;
    health::init_db(&db)?;
    #[cfg(feature = "ingest")]
    ingest::init_db(&db)?;
    #[cfg(feature = "overpass")]
    let osm = match &config.overpass {
        Some(c) => Some(overpass::open(c)?),
        None => None,
    };
    #[cfg(not(feature = "overpass"))]
    let osm: state::OsmArg = None;
    let state: SharedState = AppState::new(config.clone(), jwt_secret, api_keys, api_key_roles, db, osm);

    let app = build_router(state.clone());
    // 可用率取樣：每 10 秒自我探測、每分鐘寫一筆（見 health.rs）
    tokio::spawn(health::run_sampler(state.clone()));
    if config.audit.retention_days > 0 {
        tokio::spawn(audit::run_retention(state.clone(), config.audit.retention_days));
    }

    let addr: SocketAddr = config.server.bind.parse()?;

    if config.server.behind_reverse_proxy {
        tracing::info!("binding plain HTTP on {addr} (TLS terminated upstream by reverse proxy)");
        axum_server::bind(addr)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;
    } else {
        let cert = config
            .server
            .tls_cert
            .clone()
            .ok_or_else(|| anyhow::anyhow!("server.tls_cert is required unless behind_reverse_proxy = true"))?;
        let key = config
            .server
            .tls_key
            .clone()
            .ok_or_else(|| anyhow::anyhow!("server.tls_key is required unless behind_reverse_proxy = true"))?;
        tracing::info!("binding HTTPS on {addr} (TLS 1.3 only)");
        let tls_config = build_tls13_only_config(&cert, &key)?;
        axum_server::bind_rustls(addr, tls_config)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;
    }

    Ok(())
}

/// 需求規範「服務傳輸須採用 HTTPS TLS 1.3（含）以上」：`axum_server::tls_rustls::RustlsConfig::from_pem_file`
/// 用的是 rustls 預設支援的協定版本集合，光靠 Cargo.toml 拿掉 "tls12" feature
/// 還不夠明確表態，這裡改成手動組 `rustls::ServerConfig`，用
/// `builder_with_protocol_versions(&[&rustls::version::TLS13])` 明確只允許
/// TLS 1.3，安全性需求不是靠隱含的 crate feature 選項滿足。
fn build_tls13_only_config(cert_path: &str, key_path: &str) -> anyhow::Result<axum_server::tls_rustls::RustlsConfig> {
    let cert_file = &mut std::io::BufReader::new(std::fs::File::open(cert_path)?);
    let certs = rustls_pemfile::certs(cert_file).collect::<Result<Vec<_>, _>>()?;

    let key_file = &mut std::io::BufReader::new(std::fs::File::open(key_path)?);
    let key = rustls_pemfile::private_key(key_file)?
        .ok_or_else(|| anyhow::anyhow!("no private key found in '{key_path}'"))?;

    let server_config = rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    Ok(axum_server::tls_rustls::RustlsConfig::from_config(std::sync::Arc::new(
        server_config,
    )))
}

/// `nest_service("/viewer", ServeDir::new(...))` 這種掛法，對「/viewer」這個
/// 不帶結尾斜線的精確路徑會直接回 200（把它當成目錄本身，回傳
/// index.html），不會自動補一個導向「/viewer/」的重新導向；實測在同一個
/// Router 上加 `.route("/viewer", ...)` 想蓋掉這個行為，不管加在 nest_service
/// 前面還是後面都沒有效果（沒有深究 axum 內部為什麼，只是試過兩種順序都不行）。
///
/// 這件事本身不是空談——static/index.html 裡所有相對路徑（fetch 圖磚/
/// GeoJSON、"ndu60/"／"ilovemyhome/" 這類主題地圖連結）都是相對於目前頁面
/// 網址算的；訪客如果打「.../viewer」（少一條結尾斜線）進來，這些相對路徑
/// 全部會少算一層目錄、404 到網站根目錄去（實際在 HF Space demo 上重現過：
/// OSM 疊圖層跟主題地圖連結全部因為這個原因壞掉）。
///
/// 解法是在路由分派「之前」就攔截、直接導向補斜線版本，不靠 Router 本身的
/// 路徑匹配去蓋 nest_service：這裡用 `middleware::from_fn` 包住整個
/// Router，執行順序在所有 `.route()`/`.nest_service()` 匹配之前。
async fn redirect_missing_trailing_slash(req: Request, next: Next) -> Response {
    match req.uri().path() {
        "/viewer" => Redirect::temporary("/viewer/").into_response(),
        "/terrain3d" => Redirect::temporary("/terrain3d/").into_response(),
        _ => next.run(req).await,
    }
}

fn build_router(state: SharedState) -> Router {
    let cors_origins: Vec<HeaderValue> = state
        .config
        .security
        .allowed_origins
        .iter()
        .filter_map(|o| o.parse::<HeaderValue>().ok())
        .collect();

    let cors = CorsLayer::new()
        .allow_origin(cors_origins)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(tower_http::cors::Any);

    let protected = Router::new()
        // WMTS KVP：GetCapabilities + GetTile 走同一入口，由 REQUEST 參數分派。
        .route("/wmts", get(wmts::kvp::handler))
        // WMTS RESTful。
        .route("/wmts/1.0.0/WMTSCapabilities.xml", get(wmts::restful::get_capabilities_xml))
        .route("/wmts/1.0.0/WMTSCapabilities.json", get(wmts::restful::get_capabilities_json))
        .route(
            "/wmts/1.0.0/:layer/:style/:tms/:tile_matrix/:tile_row/:tile_col",
            get(wmts::restful::get_tile_restful),
        )
        // 無人機飛行軌跡 API。
        .route("/api/v1/tracks", get(tracks::handlers::list_tracks))
        .route("/api/v1/tracks/:session_id/points", post(tracks::handlers::upload_points))
        .route("/api/v1/tracks/:session_id", get(tracks::handlers::get_track))
        // 分享連結留存紀錄的「讀取」端點：連線 IP 屬於個資，瀏覽紀錄需要
        // JWT/API Key（跟同路徑但走匿名的 POST 寫入端點刻意分開權限，
        // 見下方 outer router 註解跟 shares::handlers::list_shares）。
        .route("/api/v1/shares", get(shares::handlers::list_shares))
        ;
    // 必須在 route_layer 之前加入，route_layer 只套用到當下已存在的路由。
    #[cfg(feature = "overpass")]
    let protected = protected.route("/api/interpreter", get(overpass::interpreter_get).post(overpass::interpreter_post));
    #[cfg(feature = "overpass")]
    let protected = protected.route("/api/v1/search", get(overpass::search::search_get));
    // 影像上傳端點：預設 body 上限只有 2 MB，原始航照要放寬到 ingest.max_frame_bytes。
    // 角色分組（見 auth::Role）：這兩組各自用 route_layer 掛角色檢查，再併入 protected；
    // 最後 protected 的 route_layer(require_auth) 會包住所有路由，所以順序是「先驗證身分、再檢查角色」。
    #[cfg(feature = "ingest")]
    let protected = {
        let max = state.config.ingest.as_ref().map(|c| c.max_frame_bytes).unwrap_or(2 * 1024 * 1024);
        let uploader = Router::new()
            .route("/api/v1/ingest/frame", post(ingest::post_frame).layer(axum::extract::DefaultBodyLimit::max(max)))
            .route("/api/v1/ingest/stats", get(ingest::get_stats))
            .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_uploader));
        protected.merge(uploader)
    };
    // 稽核紀錄（讀取、驗證、匯出）與月可用率報表：admin 專用。
    let admin = Router::new()
        .route("/api/v1/audit", get(audit::get_audit))
        .route("/api/v1/audit/verify", get(audit::verify))
        .route("/api/v1/audit/export", get(audit::export))
        .route("/api/v1/availability", get(health::get_availability));
    #[cfg(feature = "ingest")]
    let admin = admin.route("/api/v1/ingest/audit", get(audit::get_audit));
    let protected = protected.merge(admin.route_layer(middleware::from_fn_with_state(state.clone(), auth::require_admin)));
    let protected = protected.route_layer(middleware::from_fn_with_state(state.clone(), auth::require_auth));

    // /viewer：內建的輕量瀏覽器端 WMTS 圖磚檢視器（見 static/viewer.html），
    // 純靜態檔案、不需驗證即可載入；頁面內對 /wmts 端點發出的實際圖磚請求
    // 仍會帶 X-API-Key，一樣要通過下方 protected router 的 require_auth。
    let viewer = ServeDir::new("static");
    // /terrain3d：3D Tiles（Cesium 相容）測試資料集，驗證 STK AgTerrain2Tiles
    // 轉檔管線可行性用（Copernicus GLO-30 DEM，見 terrain3d/README.md 的授權
    // 說明）。純靜態檔案，不含任何敏感資訊，不需驗證。
    let terrain3d = ServeDir::new("terrain3d");

    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        // 就緒檢查（免驗證）：實際寫資料庫、檢查圖磚來源；失敗回 503。見 health.rs。
        .route("/readyz", get(health::readyz))
        // 方便直接開啟 Space/伺服器根網址就進到內建輕量檢視器（見 README.md）。
        .route("/", get(|| async { Redirect::temporary("/viewer/") }))
        .nest_service("/viewer", viewer)
        .nest_service("/terrain3d", terrain3d)
        // 分享連結留存紀錄的「寫入」端點：刻意不掛 require_auth，跟座標複製/
        // 量距一樣是給匿名檢視器訪客用的小工具，見 shares::handlers::create_share
        // 註解。讀取（GET，同一路徑）改掛在下面 protected router 需要驗證。
        .route("/api/v1/shares", post(shares::handlers::create_share))
        .merge(protected)
        .layer(cors)
        // 見 redirect_missing_trailing_slash 說明；要放在最後一個 .layer()，
        // Tower 的 layer 是後加的在外層，這樣才會在路由比對「之前」執行。
        .layer(middleware::from_fn(redirect_missing_trailing_slash))
        // 最外層：統計請求數與 5xx 數，供可用率報表使用（/healthz、/readyz 不計）。
        .layer(middleware::from_fn(health::count_requests))
        .with_state(state)
}

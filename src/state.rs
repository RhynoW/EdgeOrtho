use crate::config::AppConfig;
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg(feature = "overpass")]
pub type OsmArg = Option<duckdb::Connection>;
#[cfg(not(feature = "overpass"))]
pub type OsmArg = Option<()>;

pub type ApiKeyLimiter = DefaultDirectRateLimiter;

pub struct AppState {
    pub config: AppConfig,
    pub jwt_secret: String,
    /// api_key -> label
    pub api_keys: HashMap<String, String>,
    /// api_key -> 角色；沒列出的視為 client。
    pub api_key_roles: HashMap<String, crate::auth::Role>,
    /// api_key -> 該 key 專屬的 rate limiter（每分鐘配額，來自 security.api_key_rate_limit_per_minute）。
    pub api_key_limiters: HashMap<String, Arc<ApiKeyLimiter>>,
    pub db: Mutex<rusqlite::Connection>,
    /// Overpass 端點用的 DuckDB 唯讀連線（每個請求 try_clone 取得獨立連線）。
    #[cfg(feature = "overpass")]
    pub osm: Option<std::sync::Mutex<duckdb::Connection>>,
}

impl AppState {
    pub fn new(config: AppConfig, jwt_secret: String, api_keys: HashMap<String, String>, api_key_roles: HashMap<String, crate::auth::Role>, db: rusqlite::Connection, osm: OsmArg) -> Arc<Self> {
        let quota = Quota::per_minute(
            std::num::NonZeroU32::new(config.security.api_key_rate_limit_per_minute.max(1)).unwrap(),
        );
        let api_key_limiters = api_keys
            .keys()
            .map(|k| (k.clone(), Arc::new(RateLimiter::direct(quota))))
            .collect();
        Arc::new(Self {
            config,
            jwt_secret,
            api_keys,
            api_key_roles,
            api_key_limiters,
            db: Mutex::new(db),
            #[cfg(feature = "overpass")]
            osm: osm.map(std::sync::Mutex::new),
        })
    }
}

pub type SharedState = Arc<AppState>;

pub mod jwt;

use crate::error::AppError;
use crate::state::SharedState;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use serde::Deserialize;
use std::net::SocketAddr;

/// 角色（由低到高，高的涵蓋低的）：
/// - `client`：讀取（WMTS 圖磚、軌跡、搜尋、Overpass 相容查詢等）——**預設**，最小權限。
/// - `uploader`：client＋上傳無人機影像（`POST /api/v1/ingest/frame`）與讀取處理統計。
/// - `admin`：uploader＋稽核紀錄（讀取、驗證、匯出）與可用率報表。
///
/// 為什麼要分：稽核含來源 IP 與被拒絕的存取，可用率報表含營運資訊；能讀圖磚的金鑰（可能發給很多客戶端）不應該能讀這些，
/// 也不應該能寫入即時圖層。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    Client,
    Uploader,
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Client => "client",
            Role::Uploader => "uploader",
            Role::Admin => "admin",
        }
    }

    pub fn from_claim(v: Option<&str>) -> Role {
        match v {
            Some("admin") => Role::Admin,
            Some("uploader") => Role::Uploader,
            _ => Role::Client,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Identity {
    Jwt { subject: String, role: Role },
    ApiKey { label: String, role: Role },
}

impl Identity {
    pub fn role(&self) -> Role {
        match self {
            Identity::Jwt { role, .. } | Identity::ApiKey { role, .. } => *role,
        }
    }

    /// 稽核用的身分字串（不含任何密鑰）。
    pub fn actor(&self) -> String {
        match self {
            Identity::Jwt { subject, .. } => format!("jwt:{subject}"),
            Identity::ApiKey { label, .. } => format!("apikey:{label}"),
        }
    }
}

/// 角色檢查中介層：必須放在 `require_auth` 之內（先驗證身分、再檢查角色）。權限不足回 403，並寫入稽核（`forbidden:insufficient_role`）。
async fn require_role(needed: Role, state: SharedState, req: Request, next: Next) -> Result<Response, AppError> {
    let Some(id) = req.extensions().get::<Identity>().cloned() else {
        return Err(AppError::Unauthorized("no identity".into()));
    };
    if id.role() >= needed {
        return Ok(next.run(req).await);
    }
    let headers = req.headers();
    let ip = crate::audit::client_ip(headers, req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| &c.0), state.config.server.behind_reverse_proxy);
    crate::audit::deny(&state, &id.actor(), &ip, req.method().as_str(), req.uri().path(), "forbidden", "insufficient_role", &format!("role={} needs={}", id.role().as_str(), needed.as_str()));
    Err(AppError::Forbidden(format!("this endpoint requires role '{}'", needed.as_str())))
}

pub async fn require_admin(State(state): State<SharedState>, req: Request, next: Next) -> Result<Response, AppError> {
    require_role(Role::Admin, state, req, next).await
}

pub async fn require_uploader(State(state): State<SharedState>, req: Request, next: Next) -> Result<Response, AppError> {
    require_role(Role::Uploader, state, req, next).await
}

/// 驗證中介層：接受 `Authorization: Bearer <JWT>` 或 `X-API-Key: <key>` 其中之一。
/// 通過後將 Identity 放進 request extensions 供後續 handler 取用（如寫入軌跡上傳的來源標記）。
pub async fn require_auth(
    State(state): State<SharedState>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let headers = req.headers().clone();
    // 稽核：被拒絕的存取（401／403／429）都記錄；密鑰值本身永遠不寫入（見 audit.rs 說明）。
    let (method, path) = (req.method().to_string(), req.uri().path().to_string());
    let ip = crate::audit::client_ip(&headers, req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| &c.0), state.config.server.behind_reverse_proxy);
    let deny = |actor: &str, outcome: &'static str, reason: &'static str, detail: &str| crate::audit::deny(&state, actor, &ip, &method, &path, outcome, reason, detail);

    if let Err(e) = check_origin_and_referer(&state, &headers) {
        deny("anonymous", "forbidden", "origin_or_referer", &e.to_string());
        return Err(e);
    }

    if let Some(token) = bearer_token(&headers) {
        let claims = match jwt::verify(&token, &state.jwt_secret, &state.config.security.jwt_issuer) {
            Ok(c) => c,
            Err(msg) => {
                deny("anonymous", "unauthorized", "invalid_bearer", &msg);
                return Err(AppError::Unauthorized(msg));
            }
        };
        let role = Role::from_claim(claims.role.as_deref());
        req.extensions_mut().insert(Identity::Jwt { subject: claims.sub, role });
        return Ok(next.run(req).await);
    }

    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        let Some(label) = state.api_keys.get(key).cloned() else {
            deny("anonymous", "unauthorized", "unknown_api_key", "unknown API key");
            return Err(AppError::Unauthorized("unknown API key".into()));
        };
        let limiter = state
            .api_key_limiters
            .get(key)
            .expect("limiter must exist for every configured api key");
        if limiter.check().is_err() {
            deny(&format!("apikey:{label}"), "rate_limited", "api_key_rate_limit", "api key rate limit exceeded");
            return Err(AppError::RateLimited);
        }
        let role = state.api_key_roles.get(key).copied().unwrap_or_default();
        req.extensions_mut().insert(Identity::ApiKey { label, role });
        return Ok(next.run(req).await);
    }

    deny("anonymous", "unauthorized", "no_credentials", "missing credentials");
    Err(AppError::Unauthorized(
        "missing Authorization Bearer token or X-API-Key header".into(),
    ))
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    value.strip_prefix("Bearer ").map(|s| s.to_string())
}

/// 來源網域限制：CORS 由 tower-http CorsLayer 處理瀏覽器端的預檢與回應標頭；
/// 這裡額外針對帶有 Origin/Referer 的請求做白名單比對，防止圖磚遭其他網站熱連結盜用。
/// 伺服器對伺服器（GIS 後端、SDK）呼叫通常不帶這兩個標頭，故僅在其存在時強制檢查，
/// 實際存取控制仍以 JWT / API Key 驗證為主。
fn check_origin_and_referer(state: &SharedState, headers: &HeaderMap) -> Result<(), AppError> {
    let allowed_origins = &state.config.security.allowed_origins;
    let allowed_referers = &state.config.security.allowed_referers;

    if let Some(origin) = headers.get(axum::http::header::ORIGIN).and_then(|v| v.to_str().ok()) {
        if !allowed_origins.iter().any(|o| o == origin) {
            return Err(AppError::Forbidden(format!("origin '{origin}' not allowed")));
        }
    }
    if let Some(referer) = headers.get(axum::http::header::REFERER).and_then(|v| v.to_str().ok()) {
        if !allowed_referers.iter().any(|r| referer.starts_with(r.as_str())) {
            return Err(AppError::Forbidden(format!("referer '{referer}' not allowed")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_ordering_is_client_uploader_admin() {
        assert!(Role::Admin > Role::Uploader && Role::Uploader > Role::Client);
        assert!(Role::Admin >= Role::Uploader && Role::Uploader >= Role::Client);
        assert_eq!(Role::default(), Role::Client);
    }

    #[test]
    fn jwt_role_claim_defaults_to_least_privilege() {
        assert_eq!(Role::from_claim(Some("admin")), Role::Admin);
        assert_eq!(Role::from_claim(Some("uploader")), Role::Uploader);
        assert_eq!(Role::from_claim(Some("client")), Role::Client);
        assert_eq!(Role::from_claim(None), Role::Client);
        assert_eq!(Role::from_claim(Some("root")), Role::Client); // 未知值不升權
        assert_eq!(Role::from_claim(Some("ADMIN")), Role::Client); // 大小寫不符也不升權
    }

    #[test]
    fn config_role_parses_and_defaults() {
        #[derive(Deserialize)]
        struct K {
            #[serde(default)]
            role: Role,
        }
        assert_eq!(toml::from_str::<K>("role = \"admin\"").unwrap().role, Role::Admin);
        assert_eq!(toml::from_str::<K>("").unwrap().role, Role::Client);
        assert!(toml::from_str::<K>("role = \"root\"").is_err());
    }

    #[test]
    fn identity_actor_never_contains_secrets() {
        assert_eq!(Identity::ApiKey { label: "x".into(), role: Role::Admin }.actor(), "apikey:x");
        assert_eq!(Identity::Jwt { subject: "u1".into(), role: Role::Client }.actor(), "jwt:u1");
    }
}

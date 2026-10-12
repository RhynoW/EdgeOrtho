use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub iss: String,
    pub exp: i64,
    pub iat: i64,
    /// 可選的角色（`client`／`uploader`／`admin`）；沒有就是 `client`（最小權限）。
    #[serde(default)]
    pub role: Option<String>,
}

/// 驗證 Bearer JWT：簽章、issuer、exp（時效防護）皆由 jsonwebtoken 內建檢查完成。
pub fn verify(token: &str, secret: &str, issuer: &str) -> Result<Claims, String> {
    let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.set_issuer(&[issuer]);
    // jsonwebtoken 預設即驗證 exp；此處明確設定以避免未來調整 Validation 時遺漏。
    validation.validate_exp = true;

    let data = decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation)
        .map_err(|e| format!("invalid token: {e}"))?;
    Ok(data.claims)
}

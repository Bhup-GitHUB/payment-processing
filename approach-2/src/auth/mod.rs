use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use tonic::metadata::MetadataMap;

use crate::config::{AuthConfig, ClientConfig};
use crate::error::PropellerError;

#[derive(Debug, Clone)]
pub struct AuthContext {
    #[allow(dead_code)]
    pub subject: String,
    pub tenant_id: String,
    pub client_id: String,
    #[allow(dead_code)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AuthService {
    config: AuthConfig,
}

#[derive(Debug, serde::Deserialize)]
struct JwtClaims {
    sub: Option<String>,
    exp: Option<usize>,
    nbf: Option<usize>,
    iss: Option<String>,
    aud: Option<serde_json::Value>,
    client_id: Option<String>,
    scope: Option<String>,
    scopes: Option<Vec<String>>,
    #[serde(flatten)]
    extra: HashMap<String, serde_json::Value>,
}

impl AuthService {
    pub fn new(config: AuthConfig) -> Self {
        Self { config }
    }

    pub fn authenticate_http(
        &self,
        headers: &HeaderMap,
        client_config: &ClientConfig,
    ) -> Result<(AuthContext, bool), PropellerError> {
        if !self.config.enabled {
            return self.legacy_context_from_http(headers, client_config).map(|ctx| (ctx, false));
        }

        if let Some(token) = extract_http_bearer(headers) {
            let ctx = self.decode_jwt(token)?;
            return Ok((ctx, false));
        }

        if self.config.legacy_header_mode {
            return self.legacy_context_from_http(headers, client_config).map(|ctx| (ctx, true));
        }

        Err(PropellerError::Unauthenticated(
            "missing Authorization bearer token".into(),
        ))
    }

    pub fn authenticate_grpc(
        &self,
        metadata: &MetadataMap,
        client_config: &ClientConfig,
    ) -> Result<(AuthContext, bool), PropellerError> {
        if !self.config.enabled {
            return self.legacy_context_from_grpc(metadata, client_config).map(|ctx| (ctx, false));
        }

        if let Some(token) = extract_grpc_bearer(metadata) {
            let ctx = self.decode_jwt(token)?;
            return Ok((ctx, false));
        }

        if self.config.legacy_header_mode {
            return self.legacy_context_from_grpc(metadata, client_config).map(|ctx| (ctx, true));
        }

        Err(PropellerError::Unauthenticated(
            "missing authorization bearer token".into(),
        ))
    }

    pub fn authorize_api_key_grpc(&self, metadata: &MetadataMap) -> Result<(), PropellerError> {
        if !self.config.enabled {
            return Ok(());
        }
        if self.config.api_keys.is_empty() {
            return Err(PropellerError::Unauthenticated(
                "publisher api keys are not configured".into(),
            ));
        }

        let key = metadata
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| PropellerError::Unauthenticated("missing x-api-key".into()))?;

        if self.config.api_keys.iter().any(|candidate| candidate == key) {
            return Ok(());
        }

        Err(PropellerError::PermissionDenied("invalid api key".into()))
    }

    fn decode_jwt(&self, token: &str) -> Result<AuthContext, PropellerError> {
        let claims = self.decode_and_validate_hs256(token)?;
        let tenant_id = claims
            .extra
            .get(&self.config.tenant_claim)
            .and_then(value_to_string)
            .ok_or_else(|| {
                PropellerError::Unauthenticated(format!(
                    "missing tenant claim: {}",
                    self.config.tenant_claim
                ))
            })?;
        let client_id = claims
            .client_id
            .or_else(|| claims.sub.clone())
            .ok_or_else(|| PropellerError::Unauthenticated("missing client identity".into()))?;
        let subject = claims.sub.unwrap_or_else(|| client_id.clone());
        let scopes = parse_scopes(claims.scope, claims.scopes);
        Ok(AuthContext {
            subject,
            tenant_id,
            client_id,
            scopes,
        })
    }

    fn legacy_context_from_http(
        &self,
        headers: &HeaderMap,
        client_config: &ClientConfig,
    ) -> Result<AuthContext, PropellerError> {
        let client_id = headers
            .get(&client_config.header)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        legacy_context(client_id, &self.config.legacy_tenant)
    }

    fn legacy_context_from_grpc(
        &self,
        metadata: &MetadataMap,
        client_config: &ClientConfig,
    ) -> Result<AuthContext, PropellerError> {
        let client_id = metadata
            .get(&client_config.header)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        legacy_context(client_id, &self.config.legacy_tenant)
    }
    fn decode_and_validate_hs256(&self, token: &str) -> Result<JwtClaims, PropellerError> {
        let secret = self
            .config
            .jwt
            .hmac_secret
            .as_deref()
            .ok_or_else(|| PropellerError::Internal("jwt secret not configured".into()))?;
        let mut parts = token.split('.');
        let header_part = parts
            .next()
            .ok_or_else(|| PropellerError::Unauthenticated("invalid jwt format".into()))?;
        let payload_part = parts
            .next()
            .ok_or_else(|| PropellerError::Unauthenticated("invalid jwt format".into()))?;
        let sig_part = parts
            .next()
            .ok_or_else(|| PropellerError::Unauthenticated("invalid jwt format".into()))?;
        if parts.next().is_some() {
            return Err(PropellerError::Unauthenticated("invalid jwt format".into()));
        }

        let signing_input = format!("{}.{}", header_part, payload_part);
        let expected_sig = hmac_sha256(secret.as_bytes(), signing_input.as_bytes());
        let actual_sig = URL_SAFE_NO_PAD
            .decode(sig_part.as_bytes())
            .map_err(|_| PropellerError::Unauthenticated("invalid jwt signature encoding".into()))?;
        if !constant_time_eq(&expected_sig, &actual_sig) {
            return Err(PropellerError::Unauthenticated("jwt signature verification failed".into()));
        }

        let payload_bytes = URL_SAFE_NO_PAD
            .decode(payload_part.as_bytes())
            .map_err(|_| PropellerError::Unauthenticated("invalid jwt payload".into()))?;
        let claims: JwtClaims = serde_json::from_slice(&payload_bytes)
            .map_err(|_| PropellerError::Unauthenticated("invalid jwt claims".into()))?;
        self.validate_claims(&claims)?;
        Ok(claims)
    }

    fn validate_claims(&self, claims: &JwtClaims) -> Result<(), PropellerError> {
        if claims.iss.as_deref() != Some(self.config.jwt.issuer.as_str()) {
            return Err(PropellerError::Unauthenticated("invalid jwt issuer".into()));
        }
        if !aud_matches(claims.aud.as_ref(), &self.config.jwt.audience) {
            return Err(PropellerError::Unauthenticated("invalid jwt audience".into()));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| PropellerError::Internal("system time error".into()))?
            .as_secs() as usize;
        let leeway = self.config.jwt.clock_skew_secs as usize;

        if let Some(nbf) = claims.nbf {
            if now + leeway < nbf {
                return Err(PropellerError::Unauthenticated("jwt not active yet".into()));
            }
        }
        if let Some(exp) = claims.exp {
            if now > exp + leeway {
                return Err(PropellerError::Unauthenticated("jwt expired".into()));
            }
        } else {
            return Err(PropellerError::Unauthenticated("jwt exp missing".into()));
        }
        Ok(())
    }
}

fn legacy_context(client_id: String, legacy_tenant: &str) -> Result<AuthContext, PropellerError> {
    if client_id.is_empty() {
        return Err(PropellerError::Unauthenticated(
            "missing client identity".into(),
        ));
    }
    Ok(AuthContext {
        subject: client_id.clone(),
        tenant_id: legacy_tenant.to_string(),
        client_id,
        scopes: Vec::new(),
    })
}

fn parse_scopes(scope: Option<String>, scopes: Option<Vec<String>>) -> Vec<String> {
    let mut out = HashSet::new();
    if let Some(scope_val) = scope {
        for item in scope_val.split_whitespace() {
            out.insert(item.to_string());
        }
    }
    if let Some(scopes_val) = scopes {
        for item in scopes_val {
            out.insert(item);
        }
    }
    out.into_iter().collect()
}

fn value_to_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(v) => Some(v.clone()),
        serde_json::Value::Number(v) => Some(v.to_string()),
        _ => None,
    }
}

fn extract_http_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_bearer)
}

fn extract_grpc_bearer(metadata: &MetadataMap) -> Option<&str> {
    metadata
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(extract_bearer)
}

fn extract_bearer(value: &str) -> Option<&str> {
    let mut parts = value.splitn(2, ' ');
    match (parts.next(), parts.next()) {
        (Some(scheme), Some(token)) if scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() => {
            Some(token)
        }
        _ => None,
    }
}

fn constant_time_eq(lhs: &[u8], rhs: &[u8]) -> bool {
    if lhs.len() != rhs.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in lhs.iter().zip(rhs.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let mut hasher = Sha256::new();
        hasher.update(key);
        let digest = hasher.finalize();
        key_block[..32].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    let digest = outer.finalize();

    let mut output = [0u8; 32];
    output.copy_from_slice(&digest);
    output
}

fn aud_matches(aud: Option<&serde_json::Value>, expected: &str) -> bool {
    match aud {
        Some(serde_json::Value::String(single)) => single == expected,
        Some(serde_json::Value::Array(list)) => list
            .iter()
            .any(|entry| entry.as_str().is_some_and(|value| value == expected)),
        _ => false,
    }
}

pub fn client_channel(tenant_id: &str, client_id: &str) -> String {
    format!("tenant:{}:client:{}", tenant_id, client_id)
}

pub fn device_channel(tenant_id: &str, client_id: &str, device_id: &str) -> String {
    format!(
        "tenant:{}:client:{}:device:{}",
        tenant_id, client_id, device_id
    )
}

pub fn topic_channel(tenant_id: &str, topic: &str) -> String {
    format!("tenant:{}:topic:{}", tenant_id, topic)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthConfig, ClientConfig, JwtConfig};
    use axum::http::{HeaderMap, HeaderValue};
    use serde_json::json;

    #[test]
    fn channel_namespaces_are_canonical() {
        assert_eq!(client_channel("t1", "u1"), "tenant:t1:client:u1");
        assert_eq!(
            device_channel("t1", "u1", "d1"),
            "tenant:t1:client:u1:device:d1"
        );
        assert_eq!(topic_channel("t1", "payment"), "tenant:t1:topic:payment");
    }

    #[test]
    fn jwt_auth_success() {
        let auth = AuthService::new(test_auth_config());
        let token = build_jwt(
            "secret",
            json!({
                "iss": "issuer",
                "aud": "audience",
                "exp": 4_102_444_800u64,
                "sub": "user_1",
                "tenant_id": "tenant_1"
            }),
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
        );
        let (ctx, legacy) = auth
            .authenticate_http(&headers, &test_client_config())
            .expect("jwt should authenticate");
        assert!(!legacy);
        assert_eq!(ctx.client_id, "user_1");
        assert_eq!(ctx.tenant_id, "tenant_1");
    }

    #[test]
    fn jwt_auth_rejects_bad_audience() {
        let auth = AuthService::new(test_auth_config());
        let token = build_jwt(
            "secret",
            json!({
                "iss": "issuer",
                "aud": "wrong",
                "exp": 4_102_444_800u64,
                "sub": "user_1",
                "tenant_id": "tenant_1"
            }),
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
        );
        let err = auth
            .authenticate_http(&headers, &test_client_config())
            .expect_err("audience mismatch should fail");
        assert!(err.to_string().contains("audience"));
    }

    fn test_auth_config() -> AuthConfig {
        AuthConfig {
            enabled: true,
            legacy_header_mode: false,
            tenant_claim: "tenant_id".to_string(),
            legacy_tenant: "legacy".to_string(),
            api_keys: vec!["dev-key".to_string()],
            jwt: JwtConfig {
                issuer: "issuer".to_string(),
                audience: "audience".to_string(),
                clock_skew_secs: 0,
                hmac_secret: Some("secret".to_string()),
                jwks_url: None,
            },
        }
    }

    fn test_client_config() -> ClientConfig {
        ClientConfig {
            header: "x-client-id".to_string(),
            device_header: "x-device-id".to_string(),
            enable_device_support: true,
            device_attribute_headers: vec!["x-os".to_string()],
        }
    }

    fn build_jwt(secret: &str, payload: serde_json::Value) -> String {
        let header = json!({"alg":"HS256","typ":"JWT"});
        let header_encoded = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&header).expect("header encode"));
        let payload_encoded = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).expect("payload encode"));
        let signing_input = format!("{}.{}", header_encoded, payload_encoded);
        let sig = hmac_sha256(secret.as_bytes(), signing_input.as_bytes());
        let sig_encoded = URL_SAFE_NO_PAD.encode(sig);
        format!("{}.{}.{}", header_encoded, payload_encoded, sig_encoded)
    }
}

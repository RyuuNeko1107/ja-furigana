//! 認証ミドルウェア + CORS 設定

use super::types::AppState;
use crate::config::Config;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::net::SocketAddr;
use subtle::ConstantTimeEq;
use tower_http::cors::{Any, CorsLayer};

/// log 用に token を識別可能だが秘匿性は保つ形 (先頭 4 文字 + length) で短縮
fn token_log_repr(token: &str) -> String {
    let prefix: String = token.chars().take(4).collect();
    format!("{}…(len={})", prefix, token.len())
}

/// 認証失敗時の共通 log + metrics
fn record_auth_failure(state: &AppState, req: &Request, presented: Option<&str>, scope: &str) {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.to_string())
        .unwrap_or_else(|| "?".to_string());
    let path = req.uri().path();
    let token_repr = presented
        .map(token_log_repr)
        .unwrap_or_else(|| "<none>".to_string());
    tracing::warn!(
        peer = %peer,
        path = %path,
        scope = %scope,
        token = %token_repr,
        "auth failure"
    );
    state.metrics.record_auth_failure();
}

/// **timing-safe** な token 比較。
///
/// 単純 `==` で比較すると secret length / 一致 prefix 長 が処理時間差に漏れて
/// 攻撃者が char-by-char で token を推測できる。 `subtle::ConstantTimeEq` で
/// 全 byte を見比べた結果に縮約して時間差を消す。 length 不一致は早期 false。
fn tokens_match(allowed: &[String], presented: &str) -> bool {
    let presented_bytes = presented.as_bytes();
    let mut found = subtle::Choice::from(0u8);
    for t in allowed {
        let t_bytes = t.as_bytes();
        if t_bytes.len() != presented_bytes.len() {
            continue;
        }
        found |= t_bytes.ct_eq(presented_bytes);
    }
    bool::from(found)
}

/// 一般エンドポイントの認証判定 (pure)。
///
/// `tokens` が空なら認証不要 (= 誰でも許可)。 設定済みなら `presented` が
/// メンバーと一致したときのみ許可。 副作用 (log / metrics / next.run) は呼び出し側。
fn user_auth_allows(tokens: &[String], presented: Option<&str>) -> bool {
    tokens.is_empty() || presented.is_some_and(|t| tokens_match(tokens, t))
}

/// admin エンドポイントの認証判定 (pure)。
///
/// `admin_tokens` が空なら `None` (= 機能 disabled、 503)。 設定済みなら
/// `Some(allow)` で、 `presented` が一致したときのみ `Some(true)`。
fn admin_auth_outcome(admin_tokens: &[String], presented: Option<&str>) -> Option<bool> {
    if admin_tokens.is_empty() {
        return None;
    }
    Some(presented.is_some_and(|t| tokens_match(admin_tokens, t)))
}

/// `state.tokens` が空でない時、`/furigana` へのリクエストに認証を要求する
///
/// `X-API-Key` を優先、無ければ `Authorization: Bearer` を読む。
pub(super) async fn require_token(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let presented = extract_token(&req);
    if user_auth_allows(&state.tokens, presented.as_deref()) {
        Ok(next.run(req).await)
    } else {
        record_auth_failure(&state, &req, presented.as_deref(), "user");
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// `/admin/*` 用の認証ミドルウェア
///
/// `state.admin_tokens` が空なら 503 (admin 機能 disabled)。
/// 空でなければ `X-API-Key` または `Authorization: Bearer` を厳密に照合。
/// 認証は常に必須 (一般 tokens では通らない)。
pub(super) async fn require_admin_token(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let presented = extract_token(&req);
    match admin_auth_outcome(&state.admin_tokens, presented.as_deref()) {
        None => Err(StatusCode::SERVICE_UNAVAILABLE),
        Some(true) => Ok(next.run(req).await),
        Some(false) => {
            record_auth_failure(&state, &req, presented.as_deref(), "admin");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// `X-API-Key` 優先、無ければ `Authorization: Bearer <token>` を読む
fn extract_token(req: &Request) -> Option<String> {
    if let Some(v) = req.headers().get("x-api-key").and_then(|v| v.to_str().ok()) {
        let s = v.trim();
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    let auth = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())?;
    let stripped = auth.strip_prefix("Bearer ")?;
    Some(stripped.to_string())
}

/// `config.server.cors_origins` から `CorsLayer` を組み立てる。空なら Any 許可。
pub(super) fn build_cors(cfg: &Config) -> CorsLayer {
    let methods = [Method::GET, Method::POST, Method::OPTIONS];
    if cfg.server.cors_origins.is_empty() {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(methods)
            .allow_headers(Any)
    } else {
        let origins: Vec<HeaderValue> = cfg
            .server
            .cors_origins
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(methods)
            .allow_headers(Any)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn req_with(headers: &[(&str, &str)]) -> Request {
        let mut b = Request::builder().uri("/furigana");
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(Body::empty()).unwrap()
    }

    #[test]
    fn tokens_match_accepts_only_exact_member() {
        let allowed = vec!["alpha-secret".to_string(), "bravo-secret".to_string()];
        assert!(tokens_match(&allowed, "alpha-secret"));
        assert!(tokens_match(&allowed, "bravo-secret"));
        assert!(
            !tokens_match(&allowed, "charlie-secret"),
            "非メンバーは拒否"
        );
        // length 不一致 (prefix / 余分) は早期 false。timing-safe 比較でも結果は正しいこと。
        assert!(
            !tokens_match(&allowed, "alpha-secre"),
            "短い prefix を通さない"
        );
        assert!(
            !tokens_match(&allowed, "alpha-secrett"),
            "長い superstring を通さない"
        );
        assert!(!tokens_match(&allowed, ""), "空 token を通さない");
        assert!(!tokens_match(&[], "anything"), "allowed 空なら誰も通さない");
    }

    #[test]
    fn extract_token_prefers_x_api_key_over_bearer() {
        let r = req_with(&[("x-api-key", "KEY1"), ("authorization", "Bearer KEY2")]);
        assert_eq!(extract_token(&r).as_deref(), Some("KEY1"));
    }

    #[test]
    fn extract_token_empty_x_api_key_falls_back_to_bearer() {
        // 空 / 空白のみの X-API-Key は skip して Authorization を読む。
        let r = req_with(&[("x-api-key", "   "), ("authorization", "Bearer KEY2")]);
        assert_eq!(extract_token(&r).as_deref(), Some("KEY2"));
    }

    #[test]
    fn extract_token_bearer_prefix_is_strict() {
        assert_eq!(
            extract_token(&req_with(&[("authorization", "Bearer K")])).as_deref(),
            Some("K")
        );
        // 小文字 bearer / 別 scheme / prefix 無しは None (誤受理しない)。
        assert_eq!(
            extract_token(&req_with(&[("authorization", "bearer K")])),
            None
        );
        assert_eq!(
            extract_token(&req_with(&[("authorization", "Basic K")])),
            None
        );
        assert_eq!(extract_token(&req_with(&[("authorization", "K")])), None);
    }

    #[test]
    fn extract_token_none_when_absent() {
        assert_eq!(extract_token(&req_with(&[])), None);
    }

    #[test]
    fn user_auth_gate_logic() {
        // tokens 未設定 → 認証不要 (= 誰でも許可)。 ここが退行すると認証が黙って無効化される。
        assert!(user_auth_allows(&[], None));
        assert!(user_auth_allows(&[], Some("anything")));
        // 設定済み: 一致のみ許可、 不一致 / 欠落は拒否
        let toks = vec!["secret".to_string()];
        assert!(user_auth_allows(&toks, Some("secret")));
        assert!(!user_auth_allows(&toks, Some("wrong")));
        assert!(!user_auth_allows(&toks, None));
    }

    #[test]
    fn admin_auth_gate_logic() {
        // admin_tokens 未設定 → None (= 503 disabled、 一般 token では絶対通らない)
        assert_eq!(admin_auth_outcome(&[], Some("x")), None);
        assert_eq!(admin_auth_outcome(&[], None), None);
        // 設定済み: 一致のみ Some(true)、 不一致 / 欠落は Some(false)
        let toks = vec!["adm".to_string()];
        assert_eq!(admin_auth_outcome(&toks, Some("adm")), Some(true));
        assert_eq!(admin_auth_outcome(&toks, Some("nope")), Some(false));
        assert_eq!(admin_auth_outcome(&toks, None), Some(false));
    }
}

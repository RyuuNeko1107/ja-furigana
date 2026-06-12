//! `serve` の HTTP 型 + AppState + エラーヘルパ

use super::metrics::ServerMetrics;
use crate::paths::Paths;
use axum::http::StatusCode;
use axum::Json;
use furigana::{AccentResult, AnalyzeResult, Furigana};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;

/// 1 リクエストあたりの最大入力文字数 (公開 API 仕様)
pub(super) const MAX_TEXT_LEN: usize = 10_000;

/// `short_pause` / `long_pause` の最大文字数。
///
/// pause は句読点ごとに出力へ挿入されるため、 長い pause × 大量句読点で出力が
/// 増幅する (REPORT-001)。 ポーズ記号 (空白 / 短いマーカー) の用途では十分な上限。
pub(super) const MAX_PAUSE_LEN: usize = 16;

/// `max_segment_len` の許容範囲 (両端含む)。
///
/// 0 は分割器 (`slice::chunks`) を panic させる (REPORT-002)。 上限は入力長
/// ([`MAX_TEXT_LEN`]) と揃える (それ以上の分割幅は無意味)。
pub(super) const MIN_SEGMENT_LEN: usize = 1;
pub(super) const MAX_SEGMENT_LEN: usize = MAX_TEXT_LEN;

/// total_ms がこの閾値を超えると WARN log + slow_requests counter increment
pub(super) const SLOW_REQUEST_MS: f64 = 100.0;

/// HTTP サーバーの共有状態
///
/// `furigana` は `RwLock<Arc<...>>` でホットリロード対応:
/// - read 側 (lookup): `read().await.clone()` で `Arc` を即取り出して lock を即解放
/// - write 側 (reload): `write().await` で中の `Arc` を差し替え
///
/// `paths` は reload 時に `build_furigana` を再実行するため保持。
/// `metrics` は process-wide な counter / histogram、 `/metrics` で export。
#[derive(Clone)]
pub(super) struct AppState {
    pub(super) furigana: Arc<RwLock<Arc<Furigana>>>,
    pub(super) tokens: Arc<Vec<String>>,
    pub(super) admin_tokens: Arc<Vec<String>>,
    pub(super) paths: Arc<Paths>,
    pub(super) metrics: Arc<ServerMetrics>,
}

/// `/furigana` のクエリ / body パラメータ
#[derive(Debug, Deserialize)]
pub(super) struct FuriganaParams {
    #[serde(default)]
    pub(super) text: Option<String>,
    #[serde(default)]
    pub(super) text_b64: Option<String>,
    #[serde(default = "default_mode")]
    pub(super) mode: String,
    #[serde(default = "default_short_pause")]
    pub(super) short_pause: String,
    #[serde(default = "default_long_pause")]
    pub(super) long_pause: String,
    #[serde(default = "default_true")]
    pub(super) keep_period: bool,
    #[serde(default)]
    pub(super) segmented: bool,
    #[serde(default = "default_max_seg")]
    pub(super) max_segment_len: usize,
    #[serde(default)]
    pub(super) debug: bool,
}

pub(super) fn default_mode() -> String {
    "tts".to_string()
}
pub(super) fn default_short_pause() -> String {
    " ".to_string()
}
pub(super) fn default_long_pause() -> String {
    "   ".to_string()
}
pub(super) fn default_true() -> bool {
    true
}
pub(super) fn default_max_seg() -> usize {
    60
}

/// `/furigana` の正常レスポンス
#[derive(Debug, Serialize)]
pub(super) struct FuriganaResponse {
    pub(super) result: String,
    pub(super) mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) segments: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) timings_ms: Option<Value>,
    /// `mode=analyze` の時のみ入る Smart engine debug 詳細 (★F1)。
    /// `result` には採択 path の reading 連結が入る。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) analyze: Option<AnalyzeResult>,
    /// `mode=accent` の時のみ入る accent 中立 JSON (0.2.0)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) accent: Option<AccentResult>,
}

/// `/furigana` のエラーレスポンス (`{"error": "..."}` + HTTP ステータス)
pub(super) type ApiError = (StatusCode, Json<Value>);

/// エラーレスポンスを生成するショートカット
pub(super) fn error(code: StatusCode, msg: impl Into<String>) -> ApiError {
    (code, Json(json!({ "error": msg.into() })))
}

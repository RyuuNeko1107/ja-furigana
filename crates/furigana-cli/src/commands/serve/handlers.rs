//! HTTP ハンドラ + 変換ロジック

use super::types::{
    default_mode, error, ApiError, AppState, FuriganaParams, FuriganaResponse, MAX_PAUSE_LEN,
    MAX_SEGMENT_LEN, MAX_TEXT_LEN, MIN_SEGMENT_LEN, SLOW_REQUEST_MS,
};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use furigana::{Furigana, RomajiStyle, TtsOptions};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Instant;

/// reload trigger 元 (= log + metrics 用)
///
/// `Startup` は予約 (= 起動時 reload はまだ do_reload 経由しない)、
/// `Sighup` は Unix のみ使われる。 cross-platform 互換のため全 variant を保持。
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(super) enum ReloadSource {
    Startup,
    Admin,
    AutoUpdate,
    Sighup,
}

impl ReloadSource {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Admin => "admin",
            Self::AutoUpdate => "auto_update",
            Self::Sighup => "sighup",
        }
    }
}

/// `GET /healthz`
pub(super) async fn healthz(State(state): State<AppState>) -> Json<Value> {
    let f = state.furigana.read().await;
    Json(json!({
        "status": "ok",
        "dict_size": f.dict_size(),
    }))
}

/// `GET /metrics` — Prometheus 互換 text exposition format で metrics を返す
pub(super) async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let body = state.metrics.render();
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        body,
    )
}

/// `GET /furigana?text=...`
pub(super) async fn furigana_get(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(params): Query<FuriganaParams>,
) -> Result<Json<FuriganaResponse>, ApiError> {
    let f = state.furigana.read().await.clone();
    let user_agent = ua(&headers);
    process(f.as_ref(), &params, &state, peer, user_agent.as_deref())
}

/// `POST /furigana` (JSON body)
pub(super) async fn furigana_post(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(params): Json<FuriganaParams>,
) -> Result<Json<FuriganaResponse>, ApiError> {
    let f = state.furigana.read().await.clone();
    let user_agent = ua(&headers);
    process(f.as_ref(), &params, &state, peer, user_agent.as_deref())
}

/// `POST /admin/reload` — `<data_dir>` から辞書を再ロードして state を swap
pub(super) async fn admin_reload(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let dict_size = do_reload(&state, ReloadSource::Admin).await.map_err(|e| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("reload failed: {e}"),
        )
    })?;
    Ok(Json(json!({
        "status": "reloaded",
        "dict_size": dict_size,
    })))
}

/// 辞書を再ビルド → state.furigana を差し替え。`POST /admin/reload` と SIGHUP の共通実装。
///
/// build 自体は CPU bound + I/O 込みなので `spawn_blocking` で逃がす。
/// 戻り値は新 dict のサイズ。
pub(super) async fn do_reload(state: &AppState, source: ReloadSource) -> Result<usize, String> {
    let old_size = state.furigana.read().await.dict_size();
    let paths = state.paths.clone();
    let estimate_accent = state.estimate_accent;
    let new = tokio::task::spawn_blocking(move || -> Result<furigana::Furigana, String> {
        // 起動 flag (estimate_accent) を reload 後も維持する
        let f = crate::commands::furigana_builder(&paths)
            .estimate_accent(estimate_accent)
            .build()
            .map_err(|e| format!("build_furigana failed: {e}"))?;
        // reload 直後の最初の request が同期 analyzer init コストを払わない /
        // init 失敗でその 1 request が panic しないよう、 swap 前に eager init する
        // (起動時 preload と挙動を揃える)。 build と同じ spawn_blocking 内なので
        // executor を塞がない。 失敗時は swap せず旧 dict を温存し reload を atomic に保つ。
        f.preload()
            .map_err(|e| format!("preload after reload failed: {e}"))?;
        Ok(f)
    })
    .await
    .map_err(|e| format!("reload task join error: {e}"))??;
    let new_arc = std::sync::Arc::new(new);
    let dict_size = new_arc.dict_size();
    *state.furigana.write().await = new_arc;
    state.metrics.record_reload(dict_size);
    tracing::info!(
        source = source.as_str(),
        old_size,
        new_size = dict_size,
        delta = dict_size as i64 - old_size as i64,
        "dict reload"
    );
    Ok(dict_size)
}

/// HeaderMap から User-Agent 取り出し (= debug log 用、 無ければ None)
fn ua(headers: &HeaderMap) -> Option<String> {
    headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// パラメータをデコード → モード別変換 → JSON レスポンス組み立て
fn process(
    f: &Furigana,
    params: &FuriganaParams,
    state: &AppState,
    peer: SocketAddr,
    user_agent: Option<&str>,
) -> Result<Json<FuriganaResponse>, ApiError> {
    let text = decode_text(params)?;
    validate_length(&text)?;
    validate_params(params)?;
    let mode = normalize_mode(&params.mode);

    tracing::debug!(
        peer = %peer,
        user_agent = user_agent.unwrap_or("-"),
        text = %text,
        mode = %mode,
        "request"
    );

    let t_start = Instant::now();

    // analyze mode は tokenize 経路ではなく Smart engine analyze() を直接呼ぶ。
    // 既存 mode (tts/ruby/...) は従来通り tokenize → 変換。
    if mode == "analyze" {
        let analyze_start = Instant::now();
        let analyze_result = f.analyze(&text);
        let t_convert_ms = analyze_start.elapsed().as_secs_f64() * 1000.0;
        let t_total_ms = t_start.elapsed().as_secs_f64() * 1000.0;

        // result には採択 path の reading を連結 (= Smart engine が決めた reading sequence)、
        // 詳細 candidate / boundary は analyze field 経由で参照。
        let result: String = analyze_result
            .tokens
            .iter()
            .map(|t| t.reading.as_str())
            .collect();

        let timings_ms = if params.debug {
            Some(json!({
                "total": round1(t_total_ms),
                "tokenize": 0.0, // analyze は tokenize 経由しないため 0
                "convert": round1(t_convert_ms),
            }))
        } else {
            None
        };

        let token_dump = format_analyze_tokens(&analyze_result);
        state.metrics.record_request(&mode, t_total_ms);
        let degraded = detect_degraded(&mode, &text, &result);
        if degraded {
            state.metrics.record_failed_resolution();
            tracing::warn!(
                peer = %peer,
                text = %text,
                result = %result,
                mode = %mode,
                "reading resolution degraded (= empty or identity to input)"
            );
        }
        if t_total_ms > SLOW_REQUEST_MS {
            state.metrics.record_slow_request();
            tracing::warn!(
                peer = %peer,
                mode = %mode,
                text_len = text.chars().count(),
                total_ms = round1(t_total_ms),
                "slow request"
            );
        }
        tracing::debug!(
            peer = %peer,
            result = %result,
            tokens = %token_dump,
            n_tokens = analyze_result.tokens.len(),
            total_ms = round1(t_total_ms),
            convert_ms = round1(t_convert_ms),
            "response (analyze)"
        );

        return Ok(Json(FuriganaResponse {
            result,
            mode,
            segments: None,
            timings_ms,
            analyze: Some(analyze_result),
            accent: None,
        }));
    }

    if mode == "accent" {
        let accent_start = Instant::now();
        let accent_result = f.to_accent(&text);
        let t_convert_ms = accent_start.elapsed().as_secs_f64() * 1000.0;
        let t_total_ms = t_start.elapsed().as_secs_f64() * 1000.0;

        let result: String = accent_result
            .tokens
            .iter()
            .map(|t| t.reading.as_str())
            .collect();

        let timings_ms = if params.debug {
            Some(json!({
                "total": round1(t_total_ms),
                "tokenize": 0.0,
                "convert": round1(t_convert_ms),
            }))
        } else {
            None
        };

        state.metrics.record_request(&mode, t_total_ms);

        return Ok(Json(FuriganaResponse {
            result,
            mode,
            segments: None,
            timings_ms,
            analyze: None,
            accent: Some(accent_result),
        }));
    }

    if mode == "voicevox-aques" || mode == "aquestalk" {
        // TTS engine 固有の記号列 (ADR-0001 adapter crate 経由)。
        // - voicevox-aques: そのまま POST /accent_phrases?is_kana=true へ渡せる kana 記法
        // - aquestalk: 本家 AquesTalk (AquesTalk2 / AquesTalk10) の音声記号列
        let convert_start = Instant::now();
        let accent = f.to_accent(&text);
        // aquestalk は engine 側に記号列長の上限があるので、 `segmented=true` の時は
        // アクセント句境界で分割した列も `segments` に載せる (pause 記号は保持される)。
        let (result, segments) = if mode == "aquestalk" {
            let symbols = ja_furigana_aquestalk::to_aquestalk_with(
                &accent,
                ja_furigana_aquestalk::Options {
                    devoice: params.devoice,
                    trailing_period: params.keep_period,
                },
            );
            let segments = params.segmented.then(|| {
                let max_len = params.max_len.unwrap_or(ja_furigana_aquestalk::MAX_LEN);
                ja_furigana_aquestalk::split_for_aquestalk(&symbols, max_len)
            });
            (symbols, segments)
        } else {
            (ja_furigana_voicevox::to_aques_kana(&accent), None)
        };
        let t_convert_ms = convert_start.elapsed().as_secs_f64() * 1000.0;
        let t_total_ms = t_start.elapsed().as_secs_f64() * 1000.0;

        let timings_ms = if params.debug {
            Some(json!({
                "total": round1(t_total_ms),
                "tokenize": 0.0,
                "convert": round1(t_convert_ms),
            }))
        } else {
            None
        };

        state.metrics.record_request(&mode, t_total_ms);

        return Ok(Json(FuriganaResponse {
            result,
            mode,
            segments,
            timings_ms,
            analyze: None,
            accent: None,
        }));
    }

    let tokens_start = Instant::now();
    let tokens = f.tokenize(&text);
    let t_tokenize_ms = tokens_start.elapsed().as_secs_f64() * 1000.0;

    let convert_start = Instant::now();
    let result = match mode.as_str() {
        "kanji" => text.clone(),
        "ruby" => furigana::tokens_to_ruby(&tokens),
        "hiragana" => furigana::tokens_to_hiragana(&tokens),
        "romaji" => {
            let hira = furigana::tokens_to_hiragana(&tokens);
            furigana::hiragana_to_romaji(&hira, RomajiStyle::Hepburn)
        }
        "romaji-kunrei" => {
            let hira = furigana::tokens_to_hiragana(&tokens);
            furigana::hiragana_to_romaji(&hira, RomajiStyle::Kunrei)
        }
        _ => {
            // tts (default)
            let opts = TtsOptions {
                short_pause: params.short_pause.clone(),
                long_pause: params.long_pause.clone(),
                keep_period: params.keep_period,
                silence_symbols: params.silence_symbols,
            };
            // **Furigana::to_tts をそのまま呼ぶ**。 自前で tokenize → normalize_for_tts を
            // 組み直すと、 token filter (silence_symbols) や tts 用 postprocess ルールの
            // 適用漏れが HTTP 経路だけで起きる (実際に両方やらかした)。
            // tokens は debug 出力 / segments 用にそのまま残す。
            f.to_tts(&text, &opts)
        }
    };
    let t_convert_ms = convert_start.elapsed().as_secs_f64() * 1000.0;
    let t_total_ms = t_start.elapsed().as_secs_f64() * 1000.0;

    let segments = if params.segmented && (mode == "tts" || mode == "hiragana") {
        Some(furigana::tts::segment_for_tts(
            &result,
            params.max_segment_len,
        ))
    } else {
        None
    };

    let timings_ms = if params.debug {
        Some(json!({
            "total": round1(t_total_ms),
            "tokenize": round1(t_tokenize_ms),
            "convert": round1(t_convert_ms),
        }))
    } else {
        None
    };

    let token_dump = format_tokens(&tokens);
    let n_segments = segments.as_ref().map(|s| s.len()).unwrap_or(0);
    state.metrics.record_request(&mode, t_total_ms);
    // silence_symbols で 「絵文字だけのコメント」 が空になるのは仕様どおりで障害ではない。
    // ここを degraded 扱いすると failed_resolution counter と WARN log が実害と
    // 区別できないレベルで増える (配信コメントは絵文字のみが日常的に来る)。
    let silenced_to_empty = params.silence_symbols && result.is_empty();
    let degraded = !silenced_to_empty && detect_degraded(&mode, &text, &result);
    if degraded {
        state.metrics.record_failed_resolution();
        tracing::warn!(
            peer = %peer,
            text = %text,
            result = %result,
            mode = %mode,
            "reading resolution degraded (= empty or identity to input)"
        );
    }
    if t_total_ms > SLOW_REQUEST_MS {
        state.metrics.record_slow_request();
        tracing::warn!(
            peer = %peer,
            mode = %mode,
            text_len = text.chars().count(),
            total_ms = round1(t_total_ms),
            tokenize_ms = round1(t_tokenize_ms),
            convert_ms = round1(t_convert_ms),
            "slow request"
        );
    }
    tracing::debug!(
        peer = %peer,
        result = %result,
        tokens = %token_dump,
        n_tokens = tokens.len(),
        n_segments,
        total_ms = round1(t_total_ms),
        tokenize_ms = round1(t_tokenize_ms),
        convert_ms = round1(t_convert_ms),
        "response"
    );

    Ok(Json(FuriganaResponse {
        result,
        mode,
        segments,
        timings_ms,
        analyze: None,
        accent: None,
    }))
}

/// 読み解決が退化 (= empty / kanji 通過扱い / input = output) しているか判定。
///
/// `mode="kanji"` は input そのまま返すのが仕様なので除外。 そうでない mode で
/// result が空 / input と同一 / 全部 None-reading の状態は dict 未収録 or
/// engine の path 解決失敗を示唆する debug 用 signal。
fn detect_degraded(mode: &str, text: &str, result: &str) -> bool {
    if mode == "kanji" {
        return false;
    }
    if result.is_empty() {
        return true;
    }
    // input 中に漢字を含むのに result が input と完全一致 = reading 解決されてない
    let has_kanji = text.chars().any(|c| {
        // CJK Unified Ideographs の主要範囲のみで近似 (= 詳細は furigana::kana 側)
        ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3400}'..='\u{4DBF}').contains(&c)
    });
    has_kanji && result == text
}

/// debug log 用に tokens を `surface[reading]|surface[reading]|...` 形式に整形
fn format_tokens(tokens: &[furigana::ReadingToken]) -> String {
    tokens
        .iter()
        .map(|t| format!("{}[{}]", t.surface, t.reading.as_deref().unwrap_or("")))
        .collect::<Vec<_>>()
        .join("|")
}

/// debug log 用に analyze tokens を `surface[reading]|...` 形式に整形
fn format_analyze_tokens(result: &furigana::AnalyzeResult) -> String {
    result
        .tokens
        .iter()
        .map(|t| format!("{}[{}]", t.surface, t.reading))
        .collect::<Vec<_>>()
        .join("|")
}

/// `text` または `text_b64` から本文を取り出す。両方無ければ 400。
fn decode_text(params: &FuriganaParams) -> Result<String, ApiError> {
    if let Some(b64) = params.text_b64.as_ref() {
        let decoded = URL_SAFE_NO_PAD
            .decode(b64.trim_end_matches('='))
            .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid base64 in text_b64"))?;
        String::from_utf8(decoded).map_err(|_| {
            error(
                StatusCode::BAD_REQUEST,
                "text_b64 decoded bytes are not valid UTF-8",
            )
        })
    } else if let Some(t) = params.text.as_ref() {
        Ok(t.clone())
    } else {
        Err(error(StatusCode::BAD_REQUEST, "no text provided"))
    }
}

/// 入力長制限を確認
fn validate_length(text: &str) -> Result<(), ApiError> {
    if text.is_empty() {
        return Err(error(StatusCode::BAD_REQUEST, "no text provided"));
    }
    let nchars = text.chars().count();
    if nchars > MAX_TEXT_LEN {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!("text too long: {nchars} chars (max {MAX_TEXT_LEN})"),
        ));
    }
    Ok(())
}

/// pause 長 / segment 幅などの数量パラメータを検証する。
///
/// - `short_pause` / `long_pause`: 句読点ごとに出力へ挿入されるため、 長すぎる
///   pause × 大量句読点で出力が増幅する (REPORT-001)。 [`MAX_PAUSE_LEN`] で上限。
/// - `max_segment_len`: 0 は分割器を panic させ (REPORT-002)、 lib 側で 1 に clamp
///   されるが、 API としては明示的に 400 を返す。 [`MIN_SEGMENT_LEN`]..=[`MAX_SEGMENT_LEN`]。
fn validate_params(params: &FuriganaParams) -> Result<(), ApiError> {
    let short_len = params.short_pause.chars().count();
    if short_len > MAX_PAUSE_LEN {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!("short_pause too long: {short_len} chars (max {MAX_PAUSE_LEN})"),
        ));
    }
    let long_len = params.long_pause.chars().count();
    if long_len > MAX_PAUSE_LEN {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!("long_pause too long: {long_len} chars (max {MAX_PAUSE_LEN})"),
        ));
    }
    // max_segment_len は segmented 時のみ使われるが、 不正値は常に早期 reject する。
    if params.segmented && !(MIN_SEGMENT_LEN..=MAX_SEGMENT_LEN).contains(&params.max_segment_len) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!(
                "max_segment_len out of range: {} (allowed {MIN_SEGMENT_LEN}..={MAX_SEGMENT_LEN})",
                params.max_segment_len
            ),
        ));
    }
    // max_len (aquestalk の記号列分割幅) も同様に 0 / 過大値を reject する。
    if let Some(max_len) = params.max_len {
        if !(MIN_SEGMENT_LEN..=MAX_SEGMENT_LEN).contains(&max_len) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                format!(
                    "max_len out of range: {max_len} (allowed {MIN_SEGMENT_LEN}..={MAX_SEGMENT_LEN})"
                ),
            ));
        }
    }
    Ok(())
}

/// 不正な mode は静かに `tts` (= default) に fallback
fn normalize_mode(mode: &str) -> String {
    match mode {
        "tts" | "hiragana" | "ruby" | "kanji" | "romaji" | "romaji-kunrei" | "analyze"
        | "accent" | "voicevox-aques" | "aquestalk" => mode.to_string(),
        // alias: 棒読みちゃん系へ流すテキストは tts と同一、 voicevox は正式名へ寄せる
        "bouyomi" => "tts".to_string(),
        "voicevox" => "voicevox-aques".to_string(),
        _ => default_mode(),
    }
}

fn round1(ms: f64) -> f64 {
    (ms * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::serve::types::{default_long_pause, default_max_seg, default_short_pause};

    fn base_params() -> FuriganaParams {
        FuriganaParams {
            text: Some("猫".into()),
            text_b64: None,
            mode: default_mode(),
            short_pause: default_short_pause(),
            long_pause: default_long_pause(),
            keep_period: true,
            silence_symbols: false,
            segmented: false,
            max_segment_len: default_max_seg(),
            devoice: true,
            max_len: None,
            debug: false,
        }
    }

    #[test]
    fn validate_params_rejects_out_of_range_max_len() {
        // aquestalk の分割幅: 0 / 過大値は 400
        let mut p = base_params();
        p.max_len = Some(0);
        assert!(validate_params(&p).is_err());
        p.max_len = Some(MAX_SEGMENT_LEN + 1);
        assert!(validate_params(&p).is_err());
        p.max_len = Some(255);
        assert!(validate_params(&p).is_ok());
    }

    #[test]
    fn validate_params_accepts_defaults() {
        assert!(validate_params(&base_params()).is_ok());
    }

    #[test]
    fn validate_params_rejects_oversized_short_pause() {
        // REPORT-001 回帰: pause × 大量句読点の出力増幅を入口で弾く
        let p = FuriganaParams {
            short_pause: "x".repeat(MAX_PAUSE_LEN + 1),
            ..base_params()
        };
        assert!(validate_params(&p).is_err());
    }

    #[test]
    fn validate_params_rejects_oversized_long_pause() {
        let p = FuriganaParams {
            long_pause: "x".repeat(MAX_PAUSE_LEN + 1),
            ..base_params()
        };
        assert!(validate_params(&p).is_err());
    }

    #[test]
    fn validate_params_accepts_pause_at_limit() {
        let p = FuriganaParams {
            short_pause: "x".repeat(MAX_PAUSE_LEN),
            long_pause: "y".repeat(MAX_PAUSE_LEN),
            ..base_params()
        };
        assert!(validate_params(&p).is_ok());
    }

    #[test]
    fn validate_params_rejects_zero_segment_len_when_segmented() {
        // REPORT-002 回帰: max_segment_len=0 (chunks(0) panic 経路) を 400 で弾く
        let p = FuriganaParams {
            segmented: true,
            max_segment_len: 0,
            ..base_params()
        };
        assert!(validate_params(&p).is_err());
    }

    #[test]
    fn validate_params_ignores_segment_len_when_not_segmented() {
        // segmented=false なら max_segment_len は未使用 → 値を検証しない
        let p = FuriganaParams {
            segmented: false,
            max_segment_len: 0,
            ..base_params()
        };
        assert!(validate_params(&p).is_ok());
    }

    #[test]
    fn validate_params_rejects_segment_len_above_max() {
        let p = FuriganaParams {
            segmented: true,
            max_segment_len: MAX_SEGMENT_LEN + 1,
            ..base_params()
        };
        assert!(validate_params(&p).is_err());
    }

    // ─── decode_text ──────────────────────────────────────────────────────────

    #[test]
    fn decode_text_plain_text() {
        let p = FuriganaParams {
            text: Some("猫".into()),
            text_b64: None,
            ..base_params()
        };
        assert_eq!(decode_text(&p).unwrap(), "猫");
    }

    #[test]
    fn decode_text_b64_decodes_and_takes_precedence() {
        // "54yr" (URL_SAFE_NO_PAD base64) = "猫"。text より text_b64 が優先。
        let p = FuriganaParams {
            text: Some("犬".into()),
            text_b64: Some("54yr".into()),
            ..base_params()
        };
        assert_eq!(decode_text(&p).unwrap(), "猫");
    }

    #[test]
    fn decode_text_invalid_base64_is_400() {
        let p = FuriganaParams {
            text: None,
            text_b64: Some("!!!not base64!!!".into()),
            ..base_params()
        };
        assert_eq!(decode_text(&p).unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn decode_text_invalid_utf8_is_400() {
        // "_w" = URL_SAFE_NO_PAD base64 of [0xFF] = 不正 UTF-8。
        let p = FuriganaParams {
            text: None,
            text_b64: Some("_w".into()),
            ..base_params()
        };
        assert_eq!(decode_text(&p).unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn decode_text_missing_both_is_400() {
        let p = FuriganaParams {
            text: None,
            text_b64: None,
            ..base_params()
        };
        assert_eq!(decode_text(&p).unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    // ─── validate_length ──────────────────────────────────────────────────────

    #[test]
    fn validate_length_boundaries() {
        assert!(validate_length("").is_err(), "空は 400");
        assert!(validate_length("あ").is_ok());
        assert!(
            validate_length(&"あ".repeat(MAX_TEXT_LEN)).is_ok(),
            "上限ちょうどは OK"
        );
        assert_eq!(
            validate_length(&"あ".repeat(MAX_TEXT_LEN + 1))
                .unwrap_err()
                .0,
            StatusCode::BAD_REQUEST,
            "上限+1 は 400"
        );
    }

    // ─── normalize_mode ───────────────────────────────────────────────────────

    #[test]
    fn normalize_mode_known_through_unknown_to_default() {
        for m in [
            "tts",
            "hiragana",
            "ruby",
            "kanji",
            "romaji",
            "romaji-kunrei",
            "analyze",
            "accent",
            "voicevox-aques",
            "aquestalk",
        ] {
            assert_eq!(normalize_mode(m), m, "known mode はそのまま");
        }
        // alias
        assert_eq!(normalize_mode("bouyomi"), "tts");
        assert_eq!(normalize_mode("voicevox"), "voicevox-aques");
        assert_eq!(normalize_mode("bogus"), default_mode());
        assert_eq!(normalize_mode(""), default_mode());
    }

    // ─── detect_degraded ──────────────────────────────────────────────────────

    #[test]
    fn detect_degraded_logic() {
        assert!(
            !detect_degraded("kanji", "猫", "猫"),
            "kanji mode は常に false"
        );
        assert!(
            detect_degraded("hiragana", "猫", ""),
            "空 result は degraded"
        );
        assert!(
            detect_degraded("hiragana", "猫", "猫"),
            "漢字含みで result==input は degraded"
        );
        assert!(
            !detect_degraded("hiragana", "猫", "ねこ"),
            "解決済は not degraded"
        );
        assert!(
            !detect_degraded("hiragana", "ねこ", "ねこ"),
            "漢字なしなら result==input でも not degraded"
        );
    }
}

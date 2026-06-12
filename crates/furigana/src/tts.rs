//! TTS (音声合成) 向けテキスト整形
//!
//! `tokens_to_hiragana` で得た「全部ひらがな」のテキストを、VOICEVOX 等の
//! TTS エンジンが読み上げやすい形に正規化する。
//!
//! - 句読点統一・重複除去
//! - `、` の後に `short_pause`、`。！？` の後に `long_pause` を挿入
//! - 連続スペース圧縮
//! - `keep_period = false` で `。` を全削除
//!
//! さらに `segment_for_tts` で長文を文末・読点ベースで分割し、TTS エンジンの
//! リクエスト長制限に対応する。

use once_cell::sync::Lazy;
use regex::Regex;

/// TTS 整形オプション
#[derive(Debug, Clone)]
pub struct TtsOptions {
    /// `、` (読点) の後に挿入する文字列
    pub short_pause: String,
    /// `。！？!?` (句点等) の後に挿入する文字列
    pub long_pause: String,
    /// `。` を出力に残すか (false で削除)
    pub keep_period: bool,
}

impl Default for TtsOptions {
    /// デフォルト: short_pause=" ", long_pause="   " (スペース 3), keep_period=true
    fn default() -> Self {
        Self {
            short_pause: " ".to_string(),
            long_pause: "   ".to_string(),
            keep_period: true,
        }
    }
}

/// TTS 向けテキスト正規化
///
/// 1. 全角スペース→半角、連続スペース圧縮
/// 2. 句読点統一 (`,，` → `、`, `.．` → `。`)
/// 3. 句読点前後の空白除去
/// 4. 同一句読点の重複圧縮
/// 5. `、` の後に `short_pause`、`。！？!?` の後に `long_pause` 挿入
/// 6. 再度連続スペース圧縮 + trim
/// 7. `keep_period = false` の場合 `。` 削除
#[must_use]
pub fn normalize_for_tts(text: &str, opts: &TtsOptions) -> String {
    if text.is_empty() {
        return String::new();
    }

    let mut s = text.to_string();

    // 全角スペース → 半角
    s = s.replace('\u{3000}', " ");

    // 連続スペース圧縮 + trim
    static MULTI_SPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
    s = MULTI_SPACE.replace_all(&s, " ").trim().to_string();

    // 句読点統一
    static COMMA: Lazy<Regex> = Lazy::new(|| Regex::new(r"[，,]+").unwrap());
    static PERIOD: Lazy<Regex> = Lazy::new(|| Regex::new(r"[。．\.]+").unwrap());
    s = COMMA.replace_all(&s, "、").to_string();
    s = PERIOD.replace_all(&s, "。").to_string();

    // 句読点前後の空白除去
    static PUNCT_SPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*([、。！？!?])\s*").unwrap());
    s = PUNCT_SPACE.replace_all(&s, "$1").to_string();

    // 同一句読点の重複除去
    static DUP_COMMA: Lazy<Regex> = Lazy::new(|| Regex::new(r"(、)\s*(?:、)+").unwrap());
    static DUP_PERIOD: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"([。！？!?])\s*(?:[。！？!?])+").unwrap());
    s = DUP_COMMA.replace_all(&s, "$1").to_string();
    s = DUP_PERIOD.replace_all(&s, "$1").to_string();

    // ポーズ挿入
    s = insert_pause_after(&s, &['、'], &opts.short_pause);
    s = insert_pause_after(&s, &['。', '！', '？', '!', '?'], &opts.long_pause);

    // 連続スペース再圧縮
    static MULTI_SPACE2: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s{2,}").unwrap());
    s = MULTI_SPACE2.replace_all(&s, " ").trim().to_string();

    if !opts.keep_period {
        s = s.replace('。', "");
    }

    s
}

/// 指定文字セットの直後 (空白が続かない場合) にポーズを挿入
fn insert_pause_after(text: &str, targets: &[char], pause: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() * 2);
    for (i, &c) in chars.iter().enumerate() {
        out.push(c);
        if targets.contains(&c) {
            if let Some(next) = chars.get(i + 1) {
                if !next.is_whitespace() {
                    out.push_str(pause);
                }
            }
        }
    }
    out
}

/// TTS 向けテキスト分割
///
/// 文末記号で一次分割 → 各文が `max_len` 超なら `、` で再分割 (貪欲詰め込み)
/// → それでも超える場合は固定長 chunk 分割。
/// 空・句読点のみのセグメントは除去。
///
/// `max_len = 0` は `slice::chunks(0)` が panic するため内部で 1 に clamp する
/// (= 「1 文字ずつ分割」)。 caller (HTTP handler 等) は妥当な下限を別途 validate
/// してよいが、 本関数単体でも入力値によって panic しないことを保証する。
#[must_use]
pub fn segment_for_tts(text: &str, max_len: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    // chunks(0) panic 回避の自己防御。
    let max_len = max_len.max(1);

    // 句読点前後正規化
    static PUNCT_SPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*([、。！？!?])\s*").unwrap());
    let s = PUNCT_SPACE.replace_all(text, "$1").to_string();

    // 文末記号で一次分割
    static SENT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^。！？!?]+[。！？!?]?").unwrap());
    let sentences: Vec<String> = SENT_RE
        .find_iter(&s)
        .map(|m| m.as_str().to_string())
        .collect();

    let mut segments = Vec::new();
    for sent in &sentences {
        let sent = sent.trim();
        if sent.is_empty() {
            continue;
        }

        if sent.chars().count() <= max_len {
            segments.push(sent.to_string());
            continue;
        }

        // 「、」で再分割
        let parts: Vec<&str> = sent.split('、').collect();
        let mut buf = String::new();
        for (i, p) in parts.iter().enumerate() {
            let frag = if i < parts.len() - 1 {
                format!("{p}、")
            } else {
                (*p).to_string()
            };
            if buf.chars().count() + frag.chars().count() <= max_len {
                buf.push_str(&frag);
            } else {
                if !buf.is_empty() {
                    segments.push(buf.clone());
                }
                buf = frag;
            }
        }
        if !buf.is_empty() {
            segments.push(buf);
        }
    }

    // 句読点のみ / 空を除去
    segments.retain(|seg| !seg.is_empty() && seg != "。" && seg != "、");

    // 残りで max 超は固定長で chunk
    let mut final_segs = Vec::new();
    for seg in &segments {
        if seg.chars().count() <= max_len {
            final_segs.push(seg.clone());
        } else {
            let chars: Vec<char> = seg.chars().collect();
            for chunk in chars.chunks(max_len) {
                let s: String = chunk.iter().collect();
                if !s.is_empty() && s != "。" && s != "、" {
                    final_segs.push(s);
                }
            }
        }
    }

    final_segs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options() {
        let o = TtsOptions::default();
        assert_eq!(o.short_pause, " ");
        assert_eq!(o.long_pause, "   ");
        assert!(o.keep_period);
    }

    #[test]
    fn normalize_empty() {
        let o = TtsOptions::default();
        assert_eq!(normalize_for_tts("", &o), "");
    }

    #[test]
    fn normalize_inserts_pauses() {
        let o = TtsOptions::default();
        let result = normalize_for_tts("こんにちは。きょうははれ、あしたはあめ。", &o);
        // default は short=" ", long="   " だが MULTI_SPACE2 で全て 1 スペースに圧縮される
        // (区別したい場合は非空白マーカーを使う)
        assert_eq!(result, "こんにちは。 きょうははれ、 あしたはあめ。");
    }

    #[test]
    fn normalize_unifies_punct() {
        let o = TtsOptions::default();
        let result = normalize_for_tts("はい，よろしく.", &o);
        // ，→ 、 、 . → 。
        assert!(result.starts_with("はい、"));
        assert!(result.ends_with("よろしく。"));
    }

    #[test]
    fn normalize_compresses_duplicate_punct() {
        let o = TtsOptions::default();
        let result = normalize_for_tts("わー。。。すごい！！！", &o);
        // 。。。→ 。、！！！→ ！
        assert!(result.contains("わー。"));
        assert!(result.contains("すごい！"));
        // 重複が残っていないこと
        assert!(!result.contains("。。"));
        assert!(!result.contains("！！"));
    }

    #[test]
    fn normalize_collapses_whitespace() {
        let o = TtsOptions::default();
        // 全角スペース + 連続スペース
        let result = normalize_for_tts("  こんにちは\u{3000}\u{3000}せかい  ", &o);
        assert_eq!(result, "こんにちは せかい");
    }

    #[test]
    fn normalize_drops_period_when_disabled() {
        let opts = TtsOptions {
            keep_period: false,
            ..TtsOptions::default()
        };
        let result = normalize_for_tts("こんにちは。", &opts);
        assert!(!result.contains('。'));
    }

    #[test]
    fn normalize_custom_pauses() {
        let opts = TtsOptions {
            short_pause: "<s>".to_string(),
            long_pause: "<l>".to_string(),
            keep_period: true,
        };
        let result = normalize_for_tts("こんにちは。さよなら、また。", &opts);
        // <l> と <s> が挿入される (末尾の。後ろは MULTI_SPACE2 + trim で消える可能性がある)
        assert!(result.contains("こんにちは。<l>"));
        assert!(result.contains("さよなら、<s>"));
    }

    #[test]
    fn segment_short_text_returns_one() {
        let segs = segment_for_tts("こんにちは。", 60);
        assert_eq!(segs, vec!["こんにちは。"]);
    }

    #[test]
    fn segment_splits_on_sentence_boundary() {
        let segs = segment_for_tts("ぶん1。ぶん2。ぶん3。", 60);
        assert_eq!(segs, vec!["ぶん1。", "ぶん2。", "ぶん3。"]);
    }

    #[test]
    fn segment_falls_back_to_comma_when_too_long() {
        // max=10、3 文を 1 文ずつだと収まらないが「、」で再分割で詰める
        let segs = segment_for_tts("a、b、c、d、e、f、g、h、i", 5);
        // 各 chunk が 5 文字以内
        assert!(segs.iter().all(|s| s.chars().count() <= 5));
    }

    #[test]
    fn segment_empty_input() {
        assert_eq!(segment_for_tts("", 60), Vec::<String>::new());
    }

    #[test]
    fn segment_max_len_zero_does_not_panic() {
        // max_len=0 は内部で 1 に clamp される (slice::chunks(0) panic 回避)。
        // REPORT-002 回帰: 非空文を 0 で分割しても panic せず 1 文字ずつになる。
        let segs = segment_for_tts("あいう。", 0);
        assert!(!segs.is_empty());
        assert!(segs
            .iter()
            .all(|s| s.chars().count() <= 1 || s.ends_with('。')));
    }

    #[test]
    fn segment_filters_punct_only() {
        let segs = segment_for_tts("。！？", 60);
        assert!(segs.is_empty() || segs.iter().all(|s| s != "。" && s != "、"));
    }
}

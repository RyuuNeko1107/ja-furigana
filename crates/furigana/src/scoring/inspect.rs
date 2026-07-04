//! `AnalyzeResult` の inspection helper (★alpha.19、 辞書改善 log 用途)。
//!
//! Smart engine の出力 (= [`AnalyzeResult`]) を caller が triage できるよう、
//! 「dict 未登録 / Lindera fallback でしか reading 取れなかった surface」 を
//! 周辺 context 込みで抽出する pure 関数群。
//!
//! lib 自体は telemetry / 自動 log を行わない (= scoring-engine.md §2.4
//! 「OSS ローカル完結方針」)。 server / app 等の caller が本 module の関数で
//! candidate を抽出、 caller 側で:
//! - dict 改善 PR の triage 入力
//! - production traffic から 「未登録 surface」 ranking を作る
//! - debug 用に低 confidence reading を log
//!
//! 等の用途に流す想定。
//!
//! ## 主要関数
//!
//! - [`token_band`]: 採択 path 上の token N 番目が選んだ candidate の band 値
//! - [`surface_with_context`]: input 上の byte range の前後 N char を含む context window
//! - [`extract_dict_gap_candidates`]: 採択 path 中で band threshold 以下 (= dict 未登録疑い)
//!   の token を context window 込みで列挙

use crate::scoring::analyze::AnalyzeResult;
use serde::Serialize;
use std::ops::Range;

/// surface 周辺の文字 context (= dict 改善 log で 「どんな文脈で出たか」 を残す)。
#[derive(Debug, Clone, Serialize)]
pub struct ContextWindow {
    /// surface の **前** にあった context (= 最大 `context_chars` 字、 input 先頭で truncate)
    pub before: String,
    /// 注目している surface 自体
    pub surface: String,
    /// surface の **後** にあった context (= 最大 `context_chars` 字、 input 末尾で truncate)
    pub after: String,
}

/// dict に登録すべき候補 = Smart engine が低 band で reading を選んだ surface。
///
/// 「band ≤ threshold」 (= 通常 100 以下、 unihan per-char or Lindera fallback) で
/// 漢字を含む surface を抽出する。 これらは:
/// - jukugo dict に未登録 (band 1000 で勝てなかった)
/// - 専門用語 / 固有名詞 / 人名 が多い (= dict 改善 PR の input)
///
/// `#[non_exhaustive]`: 0.2.0+ で field 追加余地 (= alternative readings、 confidence
/// score 等)、 SemVer 互換維持のため caller の literal struct 構築は禁止。
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct DictGapCandidate {
    /// 採択 surface (= input の該当範囲)
    pub surface: String,
    /// 採択 reading (= Lindera / unihan から借りた推定)
    pub reading: String,
    /// input 上の byte range
    pub range: Range<usize>,
    /// 採択 candidate の band 値 (50 = Lindera only、 100 = unihan kanji per-char)
    pub band: u16,
    /// surface 周辺 context (= dict 改善 PR で 「何の文脈で出たか」 確認用)
    pub context: ContextWindow,
}

/// 採択 path 上の `token_idx` 番目の token が選んだ candidate の band 値を返す。
///
/// `tokens[i]` と range が一致する candidate を `candidates[i]` から探して band を返す。
/// 整合性が崩れていれば (= 一致 candidate 不在) `None`。
#[must_use]
pub fn token_band(result: &AnalyzeResult, token_idx: usize) -> Option<u16> {
    let token = result.tokens.get(token_idx)?;
    let candidates = result.candidates.get(token_idx)?;
    candidates
        .iter()
        .find(|c| c.range == token.range)
        .map(|c| c.score.band)
}

/// `input` の byte range 周辺の **N 文字 context window** を切り出す。
///
/// 「前 N 字 + surface + 後 N 字」 の 3 段で構成。 input 先頭 / 末尾近くで N 字
/// 取れない場合は取れる分まで。
///
/// 文字単位 (= char) で数えるので、 UTF-8 multi-byte 安全。
#[must_use]
pub fn surface_with_context(
    input: &str,
    range: &Range<usize>,
    context_chars: usize,
) -> ContextWindow {
    let surface = input.get(range.clone()).unwrap_or("").to_string();
    let before_slice = input.get(..range.start).unwrap_or("");
    let after_slice = input.get(range.end..).unwrap_or("");

    // 前 N 文字 (= 末尾から context_chars 字を取る)
    let before: String = {
        let chars: Vec<char> = before_slice.chars().collect();
        let start = chars.len().saturating_sub(context_chars);
        chars[start..].iter().collect()
    };
    // 後 N 文字 (= 先頭から context_chars 字を取る)
    let after: String = after_slice.chars().take(context_chars).collect();

    ContextWindow {
        before,
        surface,
        after,
    }
}

/// `AnalyzeResult` から **dict 改善候補** (= 低 band reading の token) を抽出する。
///
/// `band_threshold` 以下の band を採用した token を周辺 context 込みで列挙。
/// 漢字を含まない token (= 助詞 / okurigana 等) は除外 (= dict 改善対象は漢字のみ)。
///
/// caller 想定: production traffic で本関数を呼んで結果を log に蓄積、 後で
/// surface 頻度 ranking して dict 改善 PR の input に。
///
/// ## 引数
///
/// - `result`: [`crate::Furigana::analyze`] の戻り値
/// - `input`: analyze に渡した元 input string (= context window 抽出に必要)
/// - `context_chars`: 前後 何文字 を context に含めるか (例: 3 → 前後 3 字ずつ)
/// - `band_threshold`: この band **以下** の採択 candidate を「未登録疑い」 とみなす
///   (推奨: 100 = unihan per-char + Lindera fallback 両方を含む)
#[must_use]
pub fn extract_dict_gap_candidates(
    result: &AnalyzeResult,
    input: &str,
    context_chars: usize,
    band_threshold: u16,
) -> Vec<DictGapCandidate> {
    let mut out = Vec::new();
    for (i, token) in result.tokens.iter().enumerate() {
        // 漢字を含まない surface は skip (= 助詞 / okurigana / 数字 / 記号 等は dict 改善対象外)
        if !token.surface.chars().any(crate::kana::is_kanji_char) {
            continue;
        }
        let Some(band) = token_band(result, i) else {
            continue;
        };
        if band > band_threshold {
            continue;
        }
        let context = surface_with_context(input, &token.range, context_chars);
        out.push(DictGapCandidate {
            surface: token.surface.clone(),
            reading: token.reading.clone(),
            range: token.range.clone(),
            band,
            context,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::analyze::Token;
    use crate::scoring::candidate::{Candidate, Score};

    fn dummy_result() -> (AnalyzeResult, &'static str) {
        // 「灰桜の道」 を path: 灰桜 (band 1000、 dict) + の (band 50、 Lindera) + 道 (band 100、 unihan) で
        // 採択した模擬 AnalyzeResult を作る
        let input = "灰桜の道";
        let tokens = vec![
            Token {
                surface: "灰桜".into(),
                reading: "ハイザクラ".into(),
                range: 0..6,
                accent_phrases: Vec::new(),
                ambiguous: false,
                alternatives: Vec::new(),
                is_name: false,
            },
            Token {
                surface: "の".into(),
                reading: "ノ".into(),
                range: 6..9,
                accent_phrases: Vec::new(),
                ambiguous: false,
                alternatives: Vec::new(),
                is_name: false,
            },
            Token {
                surface: "道".into(),
                reading: "ミチ".into(),
                range: 9..12,
                accent_phrases: Vec::new(),
                ambiguous: false,
                alternatives: Vec::new(),
                is_name: false,
            },
        ];
        let candidates = vec![
            vec![Candidate::new(
                "灰桜",
                "ハイザクラ",
                0..6,
                Score::dict_exact(2),
            )],
            vec![Candidate::new("の", "ノ", 6..9, Score::lindera(1))],
            vec![Candidate::new("道", "ミチ", 9..12, Score::kanji(1))],
        ];
        (
            AnalyzeResult {
                tokens,
                candidates,
                path_indices: vec![0, 6, 9],
                boundary_regions: vec![],
            },
            input,
        )
    }

    #[test]
    fn token_band_returns_band_for_each_token() {
        let (r, _) = dummy_result();
        assert_eq!(token_band(&r, 0), Some(1000)); // 灰桜
        assert_eq!(token_band(&r, 1), Some(50)); //   の
        assert_eq!(token_band(&r, 2), Some(100)); //  道
        assert_eq!(token_band(&r, 3), None); // 範囲外
    }

    #[test]
    fn surface_with_context_extracts_window() {
        let input = "あいうえおかきくけこ";
        // 「かきく」 (= byte 15..24) の前後 2 字
        let ctx = surface_with_context(input, &(15..24), 2);
        assert_eq!(ctx.before, "えお");
        assert_eq!(ctx.surface, "かきく");
        assert_eq!(ctx.after, "けこ");
    }

    #[test]
    fn surface_with_context_truncates_at_boundaries() {
        let input = "あいう";
        // 「あ」 (= byte 0..3) の前後 5 字 (= 前は 0 字、 後は 2 字)
        let ctx = surface_with_context(input, &(0..3), 5);
        assert_eq!(ctx.before, "");
        assert_eq!(ctx.surface, "あ");
        assert_eq!(ctx.after, "いう");
    }

    #[test]
    fn extract_dict_gap_filters_non_kanji_and_high_band() {
        let (r, input) = dummy_result();
        // band ≤ 100 + 漢字含む token を抽出
        let gaps = extract_dict_gap_candidates(&r, input, 3, 100);
        // 灰桜 (band 1000、 漢字あり) → threshold 超で除外
        // の (band 50、 漢字なし) → 除外
        // 道 (band 100、 漢字あり) → 該当
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].surface, "道");
        assert_eq!(gaps[0].band, 100);
        assert_eq!(gaps[0].context.before, "灰桜の");
        assert_eq!(gaps[0].context.after, "");
    }

    #[test]
    fn extract_dict_gap_with_strict_threshold_returns_lindera_only() {
        let (r, input) = dummy_result();
        // band ≤ 50 + 漢字含む token → 該当なし (の は kana で除外)
        let gaps = extract_dict_gap_candidates(&r, input, 3, 50);
        assert_eq!(gaps.len(), 0);
    }
}

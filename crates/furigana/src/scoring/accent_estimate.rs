//! Rule-based accent 推定 (opt-in、 ADR-0007)。
//!
//! dict bracket notation (真値) を持たない token に対して、 **frozen rule** で
//! accent 核位置を推定し `AccentPhrase { estimated: true }` を埋める。
//! [`crate::FuriganaBuilder`] の `estimate_accent(true)` でのみ有効
//! (default off = ADR-0002 の 「不明は None」 挙動そのまま)。
//!
//! ## 推定 rule (phase 1)
//!
//! | 対象 | 判定 | rule |
//! |---|---|---|
//! | 人名 | `Token::is_name` (Lindera 固有名詞/人名 or NameBoundaryPass) | 4 mora 以上 = 平板 (0)、 3 mora 以下 = -3 rule。 ロー/ロウ 終わりは長さ問わず -3 (太郎系) |
//! | 外来語 | surface 全カタカナ、 or 英字 surface + カタカナ loanword 読み | -3 rule (後ろから 3 mora 目、 特殊拍は左シフト) |
//!
//! それ以外 (和語/漢語の一般名詞・動詞・形容詞) は強い規則が無いため **推定しない**
//! (= `accent_phrases` 空のまま、 None は正直な出力)。
//!
//! ## 決定論
//!
//! 入力 token 列のみから計算する pure function (lookup 無し / 確率無し / 外部通信無し)。
//! 同 input → 同 output を常に満たす ([[feedback_deterministic_runtime]] 準拠)。

use crate::analyzer::Analyzer;
use crate::kana::{hira_to_kata, is_kanji_char};
use crate::scoring::analyze::Token;
use crate::scoring::bracket::{is_combining_small_kana, AccentPhrase};
use crate::scoring::names::SUFFIXES;

/// 核を担えない特殊拍 (促音 / 撥音 / 長音)。 -3 rule でここに核が落ちたら左シフト。
fn is_special_mora(mora: &str) -> bool {
    matches!(mora, "ッ" | "ン" | "ー")
}

/// カタカナ or 長音符 (外来語 surface / 読み判定用)。 中黒 「・」 は含まない。
fn is_katakana_or_prolonged(c: char) -> bool {
    matches!(c, 'ァ'..='ヶ' | 'ー')
}

/// reading を mora 単位に分割 (拗音 / 小書き母音は直前と合算)。
fn mora_split(reading: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in reading.chars() {
        if is_combining_small_kana(c) {
            if let Some(last) = out.last_mut() {
                last.push(c);
                continue;
            }
        }
        out.push(c.to_string());
    }
    out
}

/// -3 rule: 後ろから 3 mora 目に核。 特殊拍 (ッ/ン/ー) なら核になれないので左へシフト。
/// 2 mora 以下は頭高 (1)。 戻り値は 1-based 核位置。
fn antepenultimate(morae: &[String]) -> u8 {
    let n = morae.len();
    let mut idx = if n >= 3 { n - 2 } else { 1 };
    while idx > 1 && is_special_mora(&morae[idx - 1]) {
        idx -= 1;
    }
    u8::try_from(idx).unwrap_or(u8::MAX)
}

/// 推定対象の分類。
enum EstimateKind {
    /// 人名 (IPADIC 固有名詞/人名 or NameBoundaryPass 由来)
    Name,
    /// 外来語 (カタカナ surface / 英字 surface + loanword カタカナ読み)
    Loanword,
}

fn classify(token: &Token) -> Option<EstimateKind> {
    if token.is_name {
        return Some(EstimateKind::Name);
    }
    let surface_is_katakana =
        !token.surface.is_empty() && token.surface.chars().all(is_katakana_or_prolonged);
    if surface_is_katakana {
        return Some(EstimateKind::Loanword);
    }
    // AlphabetPassthroughProvider の loanword hit (例: Aim → エイム)。
    // miss は reading == surface (ASCII) なので下の katakana 読み判定で弾かれる。
    let surface_is_ascii_alpha =
        !token.surface.is_empty() && token.surface.chars().all(|c| c.is_ascii_alphabetic());
    if surface_is_ascii_alpha {
        return Some(EstimateKind::Loanword);
    }
    None
}

/// 人名 rule: 4 mora 以上 = 平板 (0)、 3 mora 以下 = -3 rule (窪薗の一般化)。
/// ロー/ロウ 終わり (太郎/次郎系) は長さ問わず -3。
fn name_accent(morae: &[String]) -> u8 {
    let n = morae.len();
    let ro_ending = n >= 2 && morae[n - 2] == "ロ" && matches!(morae[n - 1].as_str(), "ー" | "ウ");
    if ro_ending || n < 4 {
        antepenultimate(morae)
    } else {
        0
    }
}

/// 敬称文脈 (次 token = さん/ちゃん/くん/君/様/氏) の漢字 2〜3 字 token を、
/// standalone IPADIC 照会で人名か判定し `is_name` を立てる。
///
/// 大谷さん / 田中さん のように **dict jukugo や Lindera compound が band 勝ちして
/// 人名 flag なしで採択された** token を accent 推定の対象に拾うための補完
/// (読みは変えない = 読みの人名切替は dict match rule の担当)。
/// 単漢字は dict `[[kanji]]` block 人名 suffix match の領分なので触らない。
fn flag_suffix_context_names(tokens: &mut [Token], analyzer: &Analyzer) {
    for i in 0..tokens.len().saturating_sub(1) {
        let t = &tokens[i];
        if t.is_name || !t.accent_phrases.is_empty() {
            continue;
        }
        let char_count = t.surface.chars().count();
        if !(2..=3).contains(&char_count) || !t.surface.chars().all(is_kanji_char) {
            continue;
        }
        let next = &tokens[i + 1];
        if t.range.end != next.range.start || !SUFFIXES.iter().any(|(s, _)| *s == next.surface) {
            continue;
        }
        let morphs = analyzer.tokenize(&t.surface);
        let is_name = matches!(
            morphs.as_slice(),
            [m] if m.pos.as_deref() == Some("名詞")
                && m.pos_detail.as_deref() == Some("固有名詞")
                && m.pos_detail2.as_deref() == Some("人名")
        );
        if is_name {
            tokens[i].is_name = true;
        }
    }
}

/// `tokens` の accent 未付与 token に rule-based 推定を適用する (in-place)。
///
/// - dict bracket 由来の `accent_phrases` (真値) がある token は触らない
/// - 推定できない token も触らない (= `accent_phrases` 空のまま)
/// - 推定 phrase は `estimated: true` で明示 (ADR-0007 provenance)
/// - 敬称文脈の人名は [`flag_suffix_context_names`] で補完してから推定
pub(crate) fn estimate(tokens: &mut [Token], analyzer: &Analyzer) {
    flag_suffix_context_names(tokens, analyzer);
    estimate_flagged(tokens);
}

/// [`estimate`] の本体 (analyzer 非依存部)。 unit test はこちらを直接叩く。
fn estimate_flagged(tokens: &mut [Token]) {
    for token in tokens {
        if !token.accent_phrases.is_empty() {
            continue;
        }
        let Some(kind) = classify(token) else {
            continue;
        };
        let reading = hira_to_kata(&token.reading);
        if reading.is_empty() || !reading.chars().all(is_katakana_or_prolonged) {
            continue;
        }
        let morae = mora_split(&reading);
        if morae.is_empty() {
            continue;
        }
        let accent = match kind {
            EstimateKind::Name => name_accent(&morae),
            EstimateKind::Loanword => antepenultimate(&morae),
        };
        let mora = u8::try_from(morae.len()).unwrap_or(u8::MAX);
        token.accent_phrases.push(AccentPhrase {
            reading,
            mora,
            accent: Some(accent),
            estimated: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::analyze::Token;
    use crate::scoring::candidate::{Candidate, Score};

    fn token(surface: &str, reading: &str) -> Token {
        Token::from_candidate(&Candidate::new(
            surface,
            reading,
            0..surface.len(),
            Score::dict_exact(1),
        ))
    }

    fn name_token(surface: &str, reading: &str) -> Token {
        Token::from_candidate(
            &Candidate::new(surface, reading, 0..surface.len(), Score::lindera(1))
                .with_name_flag(true),
        )
    }

    fn estimated_accent(t: &Token) -> Option<u8> {
        t.accent_phrases.first().and_then(|p| p.accent)
    }

    // ─── mora_split ──────────────────────────────────────────────────────────

    #[test]
    fn mora_split_groups_small_kana() {
        assert_eq!(mora_split("キョウト"), vec!["キョ", "ウ", "ト"]);
        assert_eq!(mora_split("ファン"), vec!["ファ", "ン"]);
        assert_eq!(mora_split("カーテン"), vec!["カ", "ー", "テ", "ン"]);
    }

    #[test]
    fn mora_split_leading_small_kana_stands_alone() {
        // 異常系: 先頭小書き (通常来ないが panic しない)
        assert_eq!(mora_split("ャア"), vec!["ャ", "ア"]);
    }

    // ─── antepenultimate (-3 rule) ───────────────────────────────────────────

    #[test]
    fn antepenultimate_standard() {
        // クリスマス (5 mora) → 3
        assert_eq!(antepenultimate(&mora_split("クリスマス")), 3);
        // バナナ (3 mora) → 1
        assert_eq!(antepenultimate(&mora_split("バナナ")), 1);
    }

    #[test]
    fn antepenultimate_short_words_atamadaka() {
        // パン (2 mora) → 1、 ア (1 mora) → 1
        assert_eq!(antepenultimate(&mora_split("パン")), 1);
        assert_eq!(antepenultimate(&mora_split("ア")), 1);
    }

    #[test]
    fn antepenultimate_shifts_left_past_special_mora() {
        // カーテン: -3 = 「ー」 → 左シフト → 1 (カ]ーテン)
        assert_eq!(antepenultimate(&mora_split("カーテン")), 1);
        // サッカー: -3 = 「ッ」 → 左シフト → 1 (サ]ッカー)
        assert_eq!(antepenultimate(&mora_split("サッカー")), 1);
        // エレベーター: -3 = 「ー」 → 左シフト → 3 (エレベ]ーター)
        assert_eq!(antepenultimate(&mora_split("エレベーター")), 3);
    }

    // ─── name rule ───────────────────────────────────────────────────────────

    #[test]
    fn name_four_mora_is_heiban() {
        // タナカ… 4 mora 姓は平板が支配的 (ワタナベ)
        assert_eq!(name_accent(&mora_split("ワタナベ")), 0);
        assert_eq!(name_accent(&mora_split("ヤマモト")), 0);
    }

    #[test]
    fn name_three_mora_antepenultimate() {
        // 3 mora 姓 → -3 = 頭高 (キ]ムラ)
        assert_eq!(name_accent(&mora_split("キムラ")), 1);
        assert_eq!(name_accent(&mora_split("モリ")), 1);
    }

    #[test]
    fn name_rou_ending_antepenultimate_regardless_of_length() {
        // 太郎系: タ]ロウ (3 mora → 1)、 シンタ]ロウ (5 mora → 3、 平板にしない)
        assert_eq!(name_accent(&mora_split("タロウ")), 1);
        assert_eq!(name_accent(&mora_split("シンタロウ")), 3);
        assert_eq!(name_accent(&mora_split("タロー")), 1);
    }

    // ─── estimate (統合) ─────────────────────────────────────────────────────

    #[test]
    fn estimate_katakana_surface_loanword() {
        let mut tokens = vec![token("カーテン", "カーテン")];
        estimate_flagged(&mut tokens);
        let p = &tokens[0].accent_phrases[0];
        assert_eq!(p.reading, "カーテン");
        assert_eq!(p.mora, 4);
        assert_eq!(p.accent, Some(1));
        assert!(p.estimated);
    }

    #[test]
    fn estimate_ascii_loanword_hit() {
        // AlphabetPassthroughProvider hit 形: surface 英字 + カタカナ読み
        let mut tokens = vec![token("Aim", "エイム")];
        estimate_flagged(&mut tokens);
        assert_eq!(estimated_accent(&tokens[0]), Some(1)); // エ]イム (3 mora → -3 = 1)
    }

    #[test]
    fn estimate_ascii_passthrough_miss_skipped() {
        // miss 形: reading == surface (ASCII) → カタカナ読み判定で skip
        let mut tokens = vec![token("xyzzy", "xyzzy")];
        estimate_flagged(&mut tokens);
        assert!(tokens[0].accent_phrases.is_empty());
    }

    #[test]
    fn estimate_name_token() {
        let mut tokens = vec![name_token("渡辺", "ワタナベ")];
        estimate_flagged(&mut tokens);
        let p = &tokens[0].accent_phrases[0];
        assert_eq!(p.accent, Some(0)); // 4 mora 姓 = 平板
        assert!(p.estimated);
    }

    #[test]
    fn estimate_kanji_common_noun_stays_none() {
        // 和語/漢語 一般名詞は推定しない (強い規則なし)
        let mut tokens = vec![token("学校", "ガッコウ")];
        estimate_flagged(&mut tokens);
        assert!(tokens[0].accent_phrases.is_empty());
    }

    #[test]
    fn estimate_does_not_touch_dict_brackets() {
        // dict bracket 由来 (真値) は触らない
        let mut tokens = vec![token("雨", "ア]メ")];
        assert_eq!(tokens[0].accent_phrases.len(), 1);
        assert!(!tokens[0].accent_phrases[0].estimated);
        estimate_flagged(&mut tokens);
        assert_eq!(tokens[0].accent_phrases.len(), 1);
        assert_eq!(tokens[0].accent_phrases[0].accent, Some(1));
        assert!(!tokens[0].accent_phrases[0].estimated);
    }

    #[test]
    fn estimate_hiragana_reading_normalized_to_katakana() {
        // 読みがひらがなで来ても カタカナ に正規化して推定
        let mut tokens = vec![name_token("森", "もり")];
        estimate_flagged(&mut tokens);
        let p = &tokens[0].accent_phrases[0];
        assert_eq!(p.reading, "モリ");
        assert_eq!(p.accent, Some(1));
    }

    #[test]
    fn estimate_is_deterministic() {
        let build = || {
            let mut tokens = vec![
                token("カーテン", "カーテン"),
                name_token("渡辺", "ワタナベ"),
            ];
            estimate_flagged(&mut tokens);
            tokens
                .iter()
                .map(|t| t.accent_phrases.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(build(), build());
    }
}

//! Bracket notation parser — reading 内の `[` `]` `/` から accent phrase を抽出。
//!
//! 仕様: ADR-0003 (bracket notation spec) / `docs/PROPOSALS/intonation.md` §4
//!
//! ## 記法
//!
//! | 記号 | 意味 |
//! |---|---|
//! | `[` | accent phrase 開始 / L→H rise |
//! | `]` | accent 核 (H→L drop) |
//! | `/` | phrase 区切り (deprecated per ADR-0003、 backward compat 維持) |
//!
//! ## 解釈規則 (ADR-0003)
//!
//! - `[カ]ミテ` — accent = Some(1)
//! - `[カミテ` — accent = Some(0) (flat)
//! - `カミテ` — accent = None (unknown)
//! - `カ]ミテ` (no `[`) — implicit `[` at start、 accent = Some(1)
//! - `[トウキョウ][ト]リツ` — 2 phrases (consecutive `[` = phrase boundary)

use serde::Serialize;

// ─── AccentPhrase ───────────────────────────────────────────────────────────

/// 1 つの accent phrase (読み + mora 数 + accent 核位置)。
///
/// `accent`: `None` = 不明、 `Some(0)` = 平板、 `Some(1..N)` = 核位置
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct AccentPhrase {
    pub reading: String,
    pub mora: u8,
    pub accent: Option<u8>,
    /// 推定 accent flag (ADR-0007)。 dict bracket 由来 (= 真値) は `false` で
    /// serialize されない = 既存 JSON 出力は不変。 `estimate_accent` opt-in 時の
    /// rule-based 推定のみ `true`。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub estimated: bool,
}

/// `parse_bracket_notation` の戻り値。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedReading {
    pub reading: String,
    pub accent_phrases: Vec<AccentPhrase>,
}

// ─── mora ───────────────────────────────────────────────────────────────────

pub(crate) fn is_combining_small_kana(c: char) -> bool {
    // dict reading は 訓=ひらがな / 音=カタカナ の混在慣習なので両 script を見る
    // (ひらがな側を見落とすと "[ちゃわん" 等の拗音入りひらがな reading で mora が狂う)
    matches!(
        c,
        'ャ' | 'ュ'
            | 'ョ'
            | 'ァ'
            | 'ィ'
            | 'ゥ'
            | 'ェ'
            | 'ォ'
            | 'ゃ'
            | 'ゅ'
            | 'ょ'
            | 'ぁ'
            | 'ぃ'
            | 'ぅ'
            | 'ぇ'
            | 'ぉ'
    )
}

/// カタカナ reading の mora 数を数える。
///
/// 拗音 (ャュョ) / 小書き母音 (ァィゥェォ) は直前と合算で 1 mora。
/// 促音 (ッ) / 撥音 (ン) / 長音 (ー) は各 1 mora。
#[allow(dead_code)] // 0.2.0 output modes で使用予定
#[must_use]
pub fn count_mora(reading: &str) -> u8 {
    let mut count: u8 = 0;
    for c in reading.chars() {
        if !is_combining_small_kana(c) {
            count = count.saturating_add(1);
        }
    }
    count
}

// ─── parser ─────────────────────────────────────────────────────────────────

/// bracket notation 付き reading をパースして stripped reading + accent phrases を返す。
///
/// `[` or `]` なしの reading → `accent_phrases` 空 (= accent 不明)、 reading はそのまま返す。
/// deprecated `/` のみの reading も未パース (ADR-0003: `/` は 0.2.0 で意味を持たない)。
#[must_use]
pub fn parse_bracket_notation(raw: &str) -> ParsedReading {
    let has_brackets = raw.chars().any(|c| matches!(c, '[' | ']'));
    if !has_brackets {
        return ParsedReading {
            reading: raw.to_string(),
            accent_phrases: Vec::new(),
        };
    }

    let mut phrases = Vec::new();
    let mut kana = String::new();
    let mut mora: u8 = 0;
    let mut accent_pos: Option<u8> = None;
    let mut has_open = false;
    let mut open_count: u32 = 0;

    for c in raw.chars() {
        match c {
            '[' => {
                open_count += 1;
                if open_count > 1 && !kana.is_empty() {
                    flush_phrase(
                        &mut phrases,
                        &mut kana,
                        &mut mora,
                        &mut accent_pos,
                        has_open,
                    );
                }
                has_open = true;
            }
            '/' => {
                if !kana.is_empty() {
                    flush_phrase(
                        &mut phrases,
                        &mut kana,
                        &mut mora,
                        &mut accent_pos,
                        has_open,
                    );
                    has_open = false;
                }
                open_count = 0;
            }
            ']' => {
                accent_pos = Some(mora);
                if !has_open {
                    has_open = true;
                }
            }
            _ => {
                kana.push(c);
                if !is_combining_small_kana(c) {
                    mora = mora.saturating_add(1);
                }
            }
        }
    }

    if !kana.is_empty() {
        flush_phrase(
            &mut phrases,
            &mut kana,
            &mut mora,
            &mut accent_pos,
            has_open,
        );
    }

    let reading = strip_intonation_markers(raw);
    ParsedReading {
        reading,
        accent_phrases: phrases,
    }
}

fn flush_phrase(
    phrases: &mut Vec<AccentPhrase>,
    kana: &mut String,
    mora: &mut u8,
    accent_pos: &mut Option<u8>,
    has_open: bool,
) {
    let accent = match *accent_pos {
        Some(pos) => Some(pos),
        None if has_open => Some(0),
        None => None,
    };
    phrases.push(AccentPhrase {
        reading: std::mem::take(kana),
        mora: *mora,
        accent,
        estimated: false,
    });
    *mora = 0;
    *accent_pos = None;
}

// ─── strip / detect (0.1.0 既存) ────────────────────────────────────────────

/// reading 文字列から intonation bracket marker (`[`, `]`, `/`) を除去。
#[must_use]
pub fn strip_intonation_markers(reading: &str) -> String {
    reading
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | '/'))
        .collect()
}

/// reading に intonation bracket marker (`[`, `]`, `/`) が含まれているか判定。
#[allow(dead_code)] // test + debug 用途
#[must_use]
pub fn has_intonation_markers(reading: &str) -> bool {
    reading.chars().any(|c| matches!(c, '[' | ']' | '/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── count_mora ─────────────────────────────────────────────────────────

    #[test]
    fn mora_normal_kana() {
        assert_eq!(count_mora("ネコ"), 2);
        assert_eq!(count_mora("サクラ"), 3);
    }

    #[test]
    fn mora_youon() {
        assert_eq!(count_mora("キョウト"), 3); // キョ(1) ウ(2) ト(3)
        assert_eq!(count_mora("ヒャク"), 2); // ヒャ(1) ク(2)
    }

    #[test]
    fn mora_sokuon_hatsuon_chouon() {
        assert_eq!(count_mora("アッサリ"), 4); // ア ッ サ リ
        assert_eq!(count_mora("カーテン"), 4); // カ ー テ ン
        assert_eq!(count_mora("パン"), 2); // パ ン
    }

    #[test]
    fn mora_gairaigo_small_vowel() {
        assert_eq!(count_mora("ファン"), 2); // ファ(1) ン(2)
        assert_eq!(count_mora("ティ"), 1); // ティ(1)
        assert_eq!(count_mora("フォーク"), 3); // フォ(1) ー(2) ク(3)
    }

    #[test]
    fn mora_hiragana_youon() {
        // dict reading は訓=ひらがな慣習: ひらがな拗音も 1 mora 合算
        assert_eq!(count_mora("ちゃわん"), 3); // ちゃ(1) わ(2) ん(3)
        assert_eq!(count_mora("きょう"), 2); // きょ(1) う(2)
    }

    #[test]
    fn parse_hiragana_reading_with_brackets() {
        // ひらがな reading + bracket でも mora / accent が正しい
        let r = parse_bracket_notation("[ちゃ]わん");
        assert_eq!(r.reading, "ちゃわん");
        assert_eq!(r.accent_phrases[0].mora, 3);
        assert_eq!(r.accent_phrases[0].accent, Some(1));
    }

    #[test]
    fn mora_empty() {
        assert_eq!(count_mora(""), 0);
    }

    // ─── parse_bracket_notation ─────────────────────────────────────────────

    #[test]
    fn parse_no_markers_yields_empty_phrases() {
        let r = parse_bracket_notation("ジョウズ");
        assert_eq!(r.reading, "ジョウズ");
        assert!(r.accent_phrases.is_empty());
    }

    #[test]
    fn parse_atamadaka() {
        // ア]メ → 1型 (implicit [ at start per ADR-0003)
        let r = parse_bracket_notation("ア]メ");
        assert_eq!(r.reading, "アメ");
        assert_eq!(r.accent_phrases.len(), 1);
        assert_eq!(r.accent_phrases[0].reading, "アメ");
        assert_eq!(r.accent_phrases[0].mora, 2);
        assert_eq!(r.accent_phrases[0].accent, Some(1));
    }

    #[test]
    fn parse_heiban() {
        // キ[リサメ → 0型 (flat)
        let r = parse_bracket_notation("キ[リサメ");
        assert_eq!(r.reading, "キリサメ");
        assert_eq!(r.accent_phrases.len(), 1);
        assert_eq!(r.accent_phrases[0].reading, "キリサメ");
        assert_eq!(r.accent_phrases[0].mora, 4);
        assert_eq!(r.accent_phrases[0].accent, Some(0));
    }

    #[test]
    fn parse_nakadaka() {
        // サ[ク]ラ → 中高 accent=2
        let r = parse_bracket_notation("サ[ク]ラ");
        assert_eq!(r.reading, "サクラ");
        assert_eq!(r.accent_phrases.len(), 1);
        assert_eq!(r.accent_phrases[0].mora, 3);
        assert_eq!(r.accent_phrases[0].accent, Some(2));
    }

    #[test]
    fn parse_odaka() {
        // ハ[ナ] → 尾高 accent=2 (= mora count)
        let r = parse_bracket_notation("ハ[ナ]");
        assert_eq!(r.reading, "ハナ");
        assert_eq!(r.accent_phrases.len(), 1);
        assert_eq!(r.accent_phrases[0].mora, 2);
        assert_eq!(r.accent_phrases[0].accent, Some(2));
    }

    #[test]
    fn parse_odaka_three_mora() {
        // コ[コロ] → 尾高 accent=3
        let r = parse_bracket_notation("コ[コロ]");
        assert_eq!(r.reading, "ココロ");
        assert_eq!(r.accent_phrases.len(), 1);
        assert_eq!(r.accent_phrases[0].mora, 3);
        assert_eq!(r.accent_phrases[0].accent, Some(3));
    }

    #[test]
    fn parse_youon_accent() {
        // キョ]ウト → 1型、 キョ = 1 mora
        let r = parse_bracket_notation("キョ]ウト");
        assert_eq!(r.reading, "キョウト");
        assert_eq!(r.accent_phrases[0].mora, 3);
        assert_eq!(r.accent_phrases[0].accent, Some(1));
    }

    #[test]
    fn parse_heiban_youon() {
        // ヒャ[ク → 0型
        let r = parse_bracket_notation("ヒャ[ク");
        assert_eq!(r.reading, "ヒャク");
        assert_eq!(r.accent_phrases[0].mora, 2);
        assert_eq!(r.accent_phrases[0].accent, Some(0));
    }

    // ─── multi-phrase ───────────────────────────────────────────────────────

    #[test]
    fn parse_multi_phrase_slash() {
        // ハ[クレイ/レ[イム → 2 phrases (deprecated / separator)
        let r = parse_bracket_notation("ハ[クレイ/レ[イム");
        assert_eq!(r.reading, "ハクレイレイム");
        assert_eq!(r.accent_phrases.len(), 2);
        assert_eq!(r.accent_phrases[0].reading, "ハクレイ");
        assert_eq!(r.accent_phrases[0].mora, 4);
        assert_eq!(r.accent_phrases[0].accent, Some(0));
        assert_eq!(r.accent_phrases[1].reading, "レイム");
        assert_eq!(r.accent_phrases[1].mora, 3);
        assert_eq!(r.accent_phrases[1].accent, Some(0));
    }

    #[test]
    fn parse_multi_phrase_consecutive_brackets() {
        // 入力 = [トウキョウ] [ト] リツ → 連続 [ で 2 phrase に分かれる。
        // phrase 1 = 「[トウキョウ]」: 末尾に ] があり核は mora 4 → accent Some(4) (尾高)。
        //   (cf. parse_explicit_open_flat の「[カミテ」 は ] 無しなので Some(0) 平板。
        //    ここは ] があるので尾高で正しい。)
        // phrase 2 = 「[ト]リツ」: ] は mora 1 の後 → accent Some(1)。
        let r = parse_bracket_notation("[トウキョウ][ト]リツ");
        assert_eq!(r.reading, "トウキョウトリツ");
        assert_eq!(r.accent_phrases.len(), 2);
        assert_eq!(r.accent_phrases[0].reading, "トウキョウ");
        assert_eq!(r.accent_phrases[0].mora, 4);
        assert_eq!(r.accent_phrases[0].accent, Some(4)); // 尾高 (] が末尾 mora)
        assert_eq!(r.accent_phrases[1].reading, "トリツ");
        assert_eq!(r.accent_phrases[1].mora, 3);
        assert_eq!(r.accent_phrases[1].accent, Some(1));
    }

    #[test]
    fn parse_multi_phrase_slash_with_accents() {
        // イ[イズナマル/メ[グム → 2 phrases
        let r = parse_bracket_notation("イ[イズナマル/メ[グム");
        assert_eq!(r.reading, "イイズナマルメグム");
        assert_eq!(r.accent_phrases.len(), 2);
        assert_eq!(r.accent_phrases[0].reading, "イイズナマル");
        assert_eq!(r.accent_phrases[0].mora, 6);
        assert_eq!(r.accent_phrases[0].accent, Some(0));
        assert_eq!(r.accent_phrases[1].reading, "メグム");
        assert_eq!(r.accent_phrases[1].mora, 3);
        assert_eq!(r.accent_phrases[1].accent, Some(0));
    }

    #[test]
    fn parse_explicit_open_at_start() {
        // [カ]ミテ → 1型
        let r = parse_bracket_notation("[カ]ミテ");
        assert_eq!(r.reading, "カミテ");
        assert_eq!(r.accent_phrases.len(), 1);
        assert_eq!(r.accent_phrases[0].mora, 3);
        assert_eq!(r.accent_phrases[0].accent, Some(1));
    }

    #[test]
    fn parse_explicit_open_flat() {
        // [カミテ → 0型
        let r = parse_bracket_notation("[カミテ");
        assert_eq!(r.reading, "カミテ");
        assert_eq!(r.accent_phrases.len(), 1);
        assert_eq!(r.accent_phrases[0].mora, 3);
        assert_eq!(r.accent_phrases[0].accent, Some(0));
    }

    #[test]
    fn parse_multi_phrase_first_phrase_without_bracket_is_unknown() {
        // bracket 情報の無い phrase は accent 不明 = None (平板 Some(0) ではない)。
        // multi-phrase の先頭が bracket 無しのケースを固定 (flush_phrase の
        // `None if has_open => Some(0)` / `None => None` 分岐の後者を pin)。
        let r = parse_bracket_notation("カミ/[テ]");
        assert_eq!(r.reading, "カミテ");
        assert_eq!(r.accent_phrases.len(), 2);
        assert_eq!(r.accent_phrases[0].reading, "カミ");
        assert_eq!(r.accent_phrases[0].accent, None); // bracket 無し = 不明
        assert_eq!(r.accent_phrases[1].reading, "テ");
        assert_eq!(r.accent_phrases[1].accent, Some(1));
    }

    // ─── intonation.md §4.1 examples ────────────────────────────────────────

    #[test]
    fn parse_spec_examples() {
        // 既存形式 (accent 無し)
        let r = parse_bracket_notation("セッサタクマ");
        assert!(r.accent_phrases.is_empty());

        // 0型 明示
        let r = parse_bracket_notation("ア[メ");
        assert_eq!(r.accent_phrases[0].accent, Some(0));
        assert_eq!(r.accent_phrases[0].mora, 2);

        // 1型
        let r = parse_bracket_notation("ネ]コ");
        assert_eq!(r.accent_phrases[0].accent, Some(1));

        // 1型
        let r = parse_bracket_notation("マ]クラ");
        assert_eq!(r.accent_phrases[0].accent, Some(1));
        assert_eq!(r.accent_phrases[0].mora, 3);
    }

    #[test]
    fn parse_single_mora_atamadaka() {
        // ジョ]ウズ → 拗音で 1 mora 目、 accent=1
        let r = parse_bracket_notation("ジョ]ウズ");
        assert_eq!(r.accent_phrases[0].mora, 3);
        assert_eq!(r.accent_phrases[0].accent, Some(1));
    }

    // ─── edge cases ─────────────────────────────────────────────────────────

    #[test]
    fn parse_empty_string() {
        let r = parse_bracket_notation("");
        assert_eq!(r.reading, "");
        assert!(r.accent_phrases.is_empty());
    }

    #[test]
    fn parse_only_markers() {
        let r = parse_bracket_notation("[]/");
        assert_eq!(r.reading, "");
        assert!(r.accent_phrases.is_empty());
    }

    #[test]
    fn parse_preserves_non_marker_punctuation() {
        let r = parse_bracket_notation("オン・ザ・ロック");
        assert_eq!(r.reading, "オン・ザ・ロック");
        assert!(r.accent_phrases.is_empty());
    }

    // ─── strip_intonation_markers (既存) ────────────────────────────────────

    #[test]
    fn strip_no_markers_returns_unchanged() {
        assert_eq!(strip_intonation_markers("ジョウズ"), "ジョウズ");
        assert_eq!(strip_intonation_markers("マリサ"), "マリサ");
    }

    #[test]
    fn strip_single_close_bracket_for_initial_high() {
        assert_eq!(strip_intonation_markers("ア]メ"), "アメ");
        assert_eq!(strip_intonation_markers("ジョ]ウズ"), "ジョウズ");
    }

    #[test]
    fn strip_single_open_bracket_for_flat() {
        assert_eq!(strip_intonation_markers("キ[リサメ"), "キリサメ");
        assert_eq!(strip_intonation_markers("ア[メ"), "アメ");
    }

    #[test]
    fn strip_open_and_close_for_middle_high() {
        assert_eq!(strip_intonation_markers("サ[ク]ラ"), "サクラ");
        assert_eq!(strip_intonation_markers("コ[コロ]"), "ココロ");
    }

    #[test]
    fn strip_phrase_separator() {
        assert_eq!(
            strip_intonation_markers("ハ[クレイ/レ[イム"),
            "ハクレイレイム"
        );
    }

    #[test]
    fn strip_multiple_phrases() {
        assert_eq!(
            strip_intonation_markers("イ[イズナマル/メ[グム"),
            "イイズナマルメグム"
        );
    }

    #[test]
    fn strip_empty_string() {
        assert_eq!(strip_intonation_markers(""), "");
    }

    #[test]
    fn strip_only_markers_yields_empty() {
        assert_eq!(strip_intonation_markers("[]/"), "");
        assert_eq!(strip_intonation_markers("[/]/"), "");
    }

    #[test]
    fn strip_preserves_non_marker_punctuation() {
        assert_eq!(
            strip_intonation_markers("オン・ザ・ロック"),
            "オン・ザ・ロック"
        );
        assert_eq!(strip_intonation_markers("ハ／シ"), "ハ／シ");
    }

    // ─── has_intonation_markers (既存) ───────────────────────────────────────

    #[test]
    fn has_markers_false_for_clean_reading() {
        assert!(!has_intonation_markers("ジョウズ"));
        assert!(!has_intonation_markers(""));
    }

    #[test]
    fn has_markers_true_for_any_marker() {
        assert!(has_intonation_markers("ア]メ"));
        assert!(has_intonation_markers("キ[リサメ"));
        assert!(has_intonation_markers("ハ[クレイ/レ[イム"));
        assert!(has_intonation_markers("/"));
        assert!(has_intonation_markers("["));
        assert!(has_intonation_markers("]"));
    }

    // ─── round-trip property ────────────────────────────────────────────────

    #[test]
    fn strip_then_has_markers_yields_false() {
        let inputs = [
            "ジョウズ",
            "ジョ]ウズ",
            "ハ[クレイ/レ[イム",
            "サ[ク]ラ",
            "",
            "[/]",
        ];
        for input in inputs {
            let stripped = strip_intonation_markers(input);
            assert!(
                !has_intonation_markers(&stripped),
                "strip 後に marker 残ってる: input={input:?}, stripped={stripped:?}"
            );
        }
    }

    #[test]
    fn parse_reading_matches_strip() {
        let inputs = [
            "ジョウズ",
            "ジョ]ウズ",
            "ハ[クレイ/レ[イム",
            "サ[ク]ラ",
            "コ[コロ]",
            "[トウキョウ][ト]リツ",
            "ア]メ",
            "キ[リサメ",
            "",
        ];
        for input in inputs {
            let parsed = parse_bracket_notation(input);
            let stripped = strip_intonation_markers(input);
            assert_eq!(
                parsed.reading, stripped,
                "parse と strip で reading 不一致: input={input:?}"
            );
        }
    }
}

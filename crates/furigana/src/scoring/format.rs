//! 0.1.0 stable 用 dict format (新 format) の struct 定義 + Deserialize。
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §3 dict format
//!
//! ## 構造
//!
//! - [`Entry`] — `[entries]` 内の各 entry 値、 untagged enum で 3 形式併存
//!   (省略形 `String` / inline `EntryDetail` / expanded sub-table も同 struct)
//! - [`EntryDetail`] — 完全形 entry の本体 (default reading + match block 配列)
//! - [`MatchBlock`] — `[[entries."x".match]]` の 1 要素 (conditions + reading)
//! - [`MatchCondition`] — matcher 条件 (literal + char_type、 **品詞 不採用**)
//! - [`CharType`] — 文字種列挙
//! - [`KanjiBlock`] — `[[kanji]]` first-class candidate generator
//! - [`EntriesData`] / [`KanjiData`] — 各 TOML file 全体の wrapper
//!
//! ## 既存 alpha era format との関係
//!
//! 既存 [`crate::rules::context::ContextRule`] / `ContextMatch` は alpha era 仕様、
//! 0.1.0 stable で本 format に置き換わる。 既存 ContextMatch にあった
//! `prev_pos` / `next_pos` (品詞) は **採用しない** (Lindera 撤廃路線)、
//! `prev_ends` / `prev_month` / `next_starts_any` / `next_digit` / `next2_starts`
//! 等の拡張 matcher は migration 時に entry 化や別 logic で吸収する。
//!
//! ## bracket forward compat (0.1.0 から、 0.2.0 で活用)
//!
//! reading 内の bracket `[` `]` `/` (intonation 用) は本 format の Deserialize
//! 段では受け入れる、 lib 側は別途 strip して reading 部分のみ使用する
//! (詳細は scoring/special.rs で実装予定)。

use serde::Deserialize;
#[cfg(test)]
use std::collections::HashMap;

/// `[entries]` 内の各 entry 値。 untagged enum で省略形 / 完全形を併存。
///
/// ## 例
///
/// 省略形:
/// ```toml
/// [entries]
/// "魔理沙" = "マリサ"
/// ```
///
/// inline 完全形:
/// ```toml
/// [entries]
/// "上手" = { reading = "ジョウズ", match = [
///   { next_eq = "から", reading = "カミテ" },
/// ]}
/// ```
///
/// expanded 完全形 (sub-table、 多 match block 時に推奨):
/// ```toml
/// [entries."上手"]
/// reading = "ジョウズ"
///
/// [[entries."上手".match]]
/// next_eq = "から"
/// reading = "カミテ"
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Entry {
    /// 省略形 — string 1 つ (= default reading のみ、 match block なし)
    Simple(String),
    /// 完全形 — default reading + match block 配列
    Detailed(EntryDetail),
}

impl Entry {
    /// この entry の default reading を返す。
    #[must_use]
    pub fn default_reading(&self) -> &str {
        match self {
            Entry::Simple(s) => s.as_str(),
            Entry::Detailed(d) => d.reading.as_str(),
        }
    }

    /// この entry の match blocks を返す (省略形は空)。
    #[must_use]
    pub fn matches(&self) -> &[MatchBlock] {
        match self {
            Entry::Simple(_) => &[],
            Entry::Detailed(d) => &d.matches,
        }
    }

    /// この entry の代替読み候補を返す (省略形は空)。
    #[must_use]
    pub fn alternatives(&self) -> &[Alternative] {
        match self {
            Entry::Simple(_) => &[],
            Entry::Detailed(d) => &d.alt,
        }
    }
}

/// 完全形 entry の本体。 default reading (必須) + match block 配列 (空可) + alt 候補 (空可)。
///
/// `reading` field は必須、 不在は parse error。 全 match block が miss した場合の
/// fallback としてもこの reading が使われる。
#[derive(Debug, Clone, Deserialize)]
pub struct EntryDetail {
    /// 全 match miss 時の default reading (必須)
    pub reading: String,
    /// 文脈 match block (TOML 順、 第一 hit 採用、 全 miss なら default 採用)
    #[serde(default, rename = "match")]
    pub matches: Vec<MatchBlock>,
    /// 代替読み候補 (ADR-0004)。消費側が ML 等で選択する用途。
    #[serde(default)]
    pub alt: Vec<Alternative>,
}

/// `[[entries."x".match]]` 配列の 1 要素。 matcher conditions + match 時の reading。
///
/// 同 block 内の condition は **AND** (全条件 hit で match 成立)、
/// 複数 block は TOML 順で **第一 hit** 採用。
#[derive(Debug, Clone, Deserialize)]
pub struct MatchBlock {
    /// match 時の reading
    pub reading: String,
    /// matcher conditions (flatten で MatchCondition の field を inline 受け付け)
    #[serde(flatten)]
    pub condition: MatchCondition,
}

/// matcher 条件。 literal (exact / prefix / suffix) + char_type + 述語 のみ、
/// 品詞 (Lindera POS) は **採用しない**。
///
/// 全 field が `None` / 空配列 / `false` (= 条件なし) の場合は無条件 match
/// (= default 等価)。
///
/// ## vocabulary (proposal §3.3)
///
/// | 軸 | prev 側 | next 側 | next-after-next |
/// |---|---|---|---|
/// | literal exact | `prev_eq` | `next_eq` | — |
/// | literal exact いずれか | `prev_eq_any` | `next_eq_any` | — |
/// | literal suffix | `prev_ends_any` | — | — |
/// | literal prefix | — | `next_starts` / `next_starts_any` | `next2_starts_any` |
/// | 文字種 | `prev_char_type` | `next_char_type` | — |
/// | 述語 | `prev_month` | `next_digit` | — |
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MatchCondition {
    // ─── literal 完全一致 ───
    /// 直前 token の surface 完全一致
    #[serde(default)]
    pub prev_eq: Option<String>,
    /// 直前 token の surface がいずれかに一致
    #[serde(default)]
    pub prev_eq_any: Vec<String>,
    /// 直後 token の surface 完全一致
    #[serde(default)]
    pub next_eq: Option<String>,
    /// 直後 token の surface がいずれかに一致
    #[serde(default)]
    pub next_eq_any: Vec<String>,

    // ─── literal suffix / prefix ───
    /// 直前 token の surface 末尾がいずれかに一致 (= ends_with any of)
    #[serde(default)]
    pub prev_ends_any: Vec<String>,
    /// 直後 token の surface 先頭が指定文字列に一致 (= starts_with)
    #[serde(default)]
    pub next_starts: Option<String>,
    /// 直後 token の surface 先頭がいずれかに一致 (= starts_with any of)
    #[serde(default)]
    pub next_starts_any: Vec<String>,
    /// 「直後の更に直後」 (= idx+2) の token surface 先頭がいずれかに一致。
    /// 「人気が無い」 → idx+1=「が」、 idx+2=「無」 のような 1 飛ばし参照用。
    #[serde(default)]
    pub next2_starts_any: Vec<String>,

    // ─── 文字種一致 ───
    /// 直前文字 (= 直前 token の末尾文字) の文字種一致
    #[serde(default)]
    pub prev_char_type: Option<CharType>,
    /// 直後文字 (= 直後 token の先頭文字) の文字種一致
    #[serde(default)]
    pub next_char_type: Option<CharType>,

    // ─── 述語 (boolean predicate) ───
    /// 直前 token の surface が月名 (`一月` 〜 `十二月` / `1月` 〜 `12月` /
    /// 全角数字含む) で終わるか
    #[serde(default)]
    pub prev_month: bool,
    /// 直後 token の surface が数字 (半角 / 全角) で始まるか
    #[serde(default)]
    pub next_digit: bool,
}

/// 代替読み候補 (ADR-0004)。同形異音語の曖昧解決用。
///
/// ## 例
///
/// ```toml
/// [[entries."上手".alt]]
/// reading = "カミテ"
/// sense = "stage-left"
/// weight = 30
/// next_eq = "から"
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct Alternative {
    /// 代替読み
    pub reading: String,
    /// 意味ラベル (消費側が ML に渡すヒント)
    #[serde(default)]
    pub sense: Option<String>,
    /// 相対頻度 (0–100、 default reading は暗黙 100)
    #[serde(default = "default_weight")]
    pub weight: u8,
    /// 文脈条件 (条件 hit 時のみこの alt を候補に含める。空 = 無条件)
    #[serde(flatten)]
    pub condition: MatchCondition,
}

fn default_weight() -> u8 {
    50
}

/// 文字種列挙 (matcher の `prev_char_type` / `next_char_type` の値型)。
///
/// TOML では文字列で書く: `"漢字"` / `"ひらがな"` / `"カタカナ"` / `"英数"` / `"記号"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub enum CharType {
    /// 漢字 (CJK Unified Ideographs)
    #[serde(rename = "漢字")]
    Kanji,
    /// ひらがな
    #[serde(rename = "ひらがな")]
    Hiragana,
    /// カタカナ (全角・半角)
    #[serde(rename = "カタカナ")]
    Katakana,
    /// 英数 (ASCII / 全角英数)
    #[serde(rename = "英数")]
    Alphanumeric,
    /// 記号 (句読点 / 括弧 / その他記号)
    #[serde(rename = "記号")]
    Symbol,
}

/// `[[kanji]]` block — 単漢字 first-class candidate generator。
///
/// 各単漢字に default reading + 文脈 match を持つ。 alpha era の `core/single/`
/// (= 旧 `single_overrides::SingleOverrides`、 alpha.15 で削除済) を統合した形。
///
/// ## 例
///
/// ```toml
/// [[kanji]]
/// char = "生"
/// default = "セイ"
///
/// [[kanji.match]]
/// next_eq = "じる"
/// reading = "ショウ"
///
/// [[kanji.match]]
/// next_char_type = "ひらがな"
/// reading = "ナマ"
/// ```
///
/// `char` field は単漢字 1 文字、 `default` field は必須。
/// `match` block の文法は entry inline match と完全同一。
#[derive(Debug, Clone, Deserialize)]
pub struct KanjiBlock {
    /// 対象漢字 (1 文字、 [`Self::validate`] で確認可能)
    pub char: String,
    /// 全 match miss 時の default reading (必須)
    pub default: String,
    /// 文脈 match (= entry inline match と同 vocabulary)
    #[serde(default, rename = "match")]
    pub matches: Vec<MatchBlock>,
    /// 代替読み候補 (ADR-0004)
    #[serde(default)]
    pub alt: Vec<Alternative>,
}

impl KanjiBlock {
    /// `char` field が単漢字 1 文字であることを確認する。
    ///
    /// `Ok(())` なら valid、 `Err(message)` なら invalid (1 文字でない / 空 / 漢字でない)。
    /// caller (loader) 側で deserialize 後に呼ぶことで、 validate.py 相当の check ができる。
    pub fn validate(&self) -> Result<(), String> {
        let chars: Vec<char> = self.char.chars().collect();
        if chars.is_empty() {
            return Err("kanji block: char field is empty".to_string());
        }
        if chars.len() != 1 {
            return Err(format!(
                "kanji block: char must be single character, got {:?} ({} chars)",
                self.char,
                chars.len()
            ));
        }
        // 注: 「漢字」 の判定は別 module (kana::has_kanji 等) でやる方針、
        // ここでは文字数のみ check (後で kana::is_kanji 等で拡張可能)
        Ok(())
    }
}

/// `[entries]` block 全体 (= surface → Entry の HashMap)
///
/// TOML 上の `[entries]` table を deserialize する。
#[cfg(test)]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EntriesData {
    /// surface → Entry mapping
    #[serde(default)]
    pub entries: HashMap<String, Entry>,
}

/// `[[kanji]]` block 全体 (= 配列で複数 KanjiBlock)
///
/// TOML 上の `[[kanji]]` array of tables を deserialize する。
#[cfg(test)]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct KanjiData {
    /// kanji block 配列
    #[serde(default, rename = "kanji")]
    pub blocks: Vec<KanjiBlock>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Entry (untagged enum) deserialize ───────────────────────────────────

    #[test]
    fn entry_simple_form_deserializes_as_string() {
        let toml_str = r#"
            [entries]
            "魔理沙" = "マリサ"
        "#;
        let data: EntriesData = toml::from_str(toml_str).unwrap();
        let entry = data.entries.get("魔理沙").unwrap();
        assert!(matches!(entry, Entry::Simple(_)));
        assert_eq!(entry.default_reading(), "マリサ");
        assert!(entry.matches().is_empty());
    }

    #[test]
    fn entry_inline_form_deserializes_as_detailed() {
        let toml_str = r#"
            [entries]
            "上手" = { reading = "ジョウズ", match = [
              { next_eq = "から", reading = "カミテ" }
            ]}
        "#;
        let data: EntriesData = toml::from_str(toml_str).unwrap();
        let entry = data.entries.get("上手").unwrap();
        assert!(matches!(entry, Entry::Detailed(_)));
        assert_eq!(entry.default_reading(), "ジョウズ");
        assert_eq!(entry.matches().len(), 1);
        assert_eq!(entry.matches()[0].reading, "カミテ");
        assert_eq!(
            entry.matches()[0].condition.next_eq.as_deref(),
            Some("から")
        );
    }

    #[test]
    fn entry_expanded_form_with_subtable() {
        let toml_str = r#"
            [entries."上手"]
            reading = "ジョウズ"

            [[entries."上手".match]]
            next_eq = "から"
            reading = "カミテ"

            [[entries."上手".match]]
            prev_eq = "下"
            reading = "シタテ"
        "#;
        let data: EntriesData = toml::from_str(toml_str).unwrap();
        let entry = data.entries.get("上手").unwrap();
        assert_eq!(entry.default_reading(), "ジョウズ");
        assert_eq!(entry.matches().len(), 2);
        assert_eq!(entry.matches()[0].reading, "カミテ");
        assert_eq!(entry.matches()[1].condition.prev_eq.as_deref(), Some("下"));
    }

    #[test]
    fn entry_simple_and_detailed_can_coexist() {
        let toml_str = r#"
            [entries]
            "魔理沙" = "マリサ"

            [entries."上手"]
            reading = "ジョウズ"

            [[entries."上手".match]]
            next_eq = "から"
            reading = "カミテ"
        "#;
        let data: EntriesData = toml::from_str(toml_str).unwrap();
        assert_eq!(data.entries.len(), 2);
        assert_eq!(
            data.entries.get("魔理沙").unwrap().default_reading(),
            "マリサ"
        );
        assert_eq!(
            data.entries.get("上手").unwrap().default_reading(),
            "ジョウズ"
        );
    }

    // ─── MatchCondition fields ───────────────────────────────────────────────

    #[test]
    fn match_condition_all_fields_deserialize() {
        let toml_str = r#"
            [[match]]
            prev_eq = "前"
            prev_eq_any = ["a", "b"]
            next_eq = "後"
            next_eq_any = ["x", "y"]
            prev_char_type = "漢字"
            next_char_type = "ひらがな"
            reading = "test"
        "#;
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(rename = "match")]
            matches: Vec<MatchBlock>,
        }
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        let m = &w.matches[0];
        assert_eq!(m.condition.prev_eq.as_deref(), Some("前"));
        assert_eq!(m.condition.prev_eq_any, vec!["a", "b"]);
        assert_eq!(m.condition.next_eq.as_deref(), Some("後"));
        assert_eq!(m.condition.next_eq_any, vec!["x", "y"]);
        assert_eq!(m.condition.prev_char_type, Some(CharType::Kanji));
        assert_eq!(m.condition.next_char_type, Some(CharType::Hiragana));
    }

    #[test]
    fn match_condition_all_fields_optional() {
        let toml_str = r#"
            [[match]]
            reading = "test"
        "#;
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(rename = "match")]
            matches: Vec<MatchBlock>,
        }
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        let m = &w.matches[0];
        assert!(m.condition.prev_eq.is_none());
        assert!(m.condition.prev_eq_any.is_empty());
        assert!(m.condition.next_eq.is_none());
        assert!(m.condition.next_eq_any.is_empty());
        assert!(m.condition.prev_char_type.is_none());
        assert!(m.condition.next_char_type.is_none());
    }

    // ─── CharType ────────────────────────────────────────────────────────────

    #[test]
    fn char_type_all_variants_deserialize() {
        let cases = [
            ("漢字", CharType::Kanji),
            ("ひらがな", CharType::Hiragana),
            ("カタカナ", CharType::Katakana),
            ("英数", CharType::Alphanumeric),
            ("記号", CharType::Symbol),
        ];
        for (s, expected) in cases {
            let toml_str = format!(r#"value = "{}""#, s);
            #[derive(Deserialize)]
            struct Wrapper {
                value: CharType,
            }
            let w: Wrapper = toml::from_str(&toml_str).unwrap();
            assert_eq!(w.value, expected, "case: {s}");
        }
    }

    #[test]
    fn char_type_unknown_value_errors() {
        let toml_str = r#"value = "其他""#;
        #[derive(Deserialize)]
        struct Wrapper {
            #[allow(dead_code)]
            value: CharType,
        }
        let result: Result<Wrapper, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    // ─── KanjiBlock ──────────────────────────────────────────────────────────

    #[test]
    fn kanji_block_basic_deserializes() {
        let toml_str = r#"
            [[kanji]]
            char = "生"
            default = "セイ"

            [[kanji.match]]
            next_eq = "じる"
            reading = "ショウ"

            [[kanji.match]]
            next_char_type = "ひらがな"
            reading = "ナマ"
        "#;
        let data: KanjiData = toml::from_str(toml_str).unwrap();
        assert_eq!(data.blocks.len(), 1);
        let kanji = &data.blocks[0];
        assert_eq!(kanji.char, "生");
        assert_eq!(kanji.default, "セイ");
        assert_eq!(kanji.matches.len(), 2);
        assert_eq!(kanji.matches[0].reading, "ショウ");
        assert_eq!(kanji.matches[1].reading, "ナマ");
        assert!(kanji.validate().is_ok());
    }

    #[test]
    fn kanji_block_multiple_kanji_in_array() {
        let toml_str = r#"
            [[kanji]]
            char = "生"
            default = "セイ"

            [[kanji]]
            char = "下"
            default = "シタ"

            [[kanji.match]]
            prev_eq_any = ["階段", "段"]
            reading = "オリ"
        "#;
        let data: KanjiData = toml::from_str(toml_str).unwrap();
        assert_eq!(data.blocks.len(), 2);
        assert_eq!(data.blocks[0].char, "生");
        assert_eq!(data.blocks[1].char, "下");
        assert_eq!(data.blocks[1].matches.len(), 1);
    }

    #[test]
    fn kanji_block_validate_rejects_multi_char() {
        let block = KanjiBlock {
            char: "魔理沙".to_string(),
            default: "マリサ".to_string(),
            matches: vec![],
            alt: vec![],
        };
        let err = block.validate().unwrap_err();
        assert!(err.contains("single character"), "err: {err}");
    }

    #[test]
    fn kanji_block_validate_rejects_empty() {
        let block = KanjiBlock {
            char: String::new(),
            default: "".to_string(),
            matches: vec![],
            alt: vec![],
        };
        let err = block.validate().unwrap_err();
        assert!(err.contains("empty"), "err: {err}");
    }

    #[test]
    fn kanji_block_validate_accepts_single_kanji() {
        let block = KanjiBlock {
            char: "生".to_string(),
            default: "セイ".to_string(),
            matches: vec![],
            alt: vec![],
        };
        assert!(block.validate().is_ok());
    }

    // ─── EntriesData / KanjiData empty / default ─────────────────────────────

    #[test]
    fn entries_data_empty_deserializes_to_default() {
        let toml_str = "";
        let data: EntriesData = toml::from_str(toml_str).unwrap();
        assert!(data.entries.is_empty());
    }

    #[test]
    fn kanji_data_empty_deserializes_to_default() {
        let toml_str = "";
        let data: KanjiData = toml::from_str(toml_str).unwrap();
        assert!(data.blocks.is_empty());
    }

    // ─── 完全形 entry: reading 必須 ──────────────────────────────────────────

    #[test]
    fn entry_detail_reading_field_is_required() {
        let toml_str = r#"
            [entries."上手"]
            # reading field 不在 → parse error 期待

            [[entries."上手".match]]
            next_eq = "から"
            reading = "カミテ"
        "#;
        // detail form で reading 不在は untagged enum でも 受け止められない、
        // 結果的に parse error になる (Simple は string のみ受ける、
        // Detailed は reading required)
        let result: Result<EntriesData, _> = toml::from_str(toml_str);
        assert!(
            result.is_err(),
            "expected parse error for missing reading field"
        );
    }

    // ─── bracket forward compat (0.2.0 で活用、 0.1.0 では受け入れるだけ) ──

    #[test]
    fn entry_with_bracket_in_reading_is_accepted_at_parse_time() {
        // 0.1.0 の format parser は bracket を含む reading を受け入れる、
        // strip / 無視は別 layer (scoring/special.rs 予定) で実施。
        let toml_str = r#"
            [entries]
            "上手" = "ジョ]ウズ"
        "#;
        let data: EntriesData = toml::from_str(toml_str).unwrap();
        assert_eq!(
            data.entries.get("上手").unwrap().default_reading(),
            "ジョ]ウズ"
        );
    }

    #[test]
    fn kanji_block_with_bracket_in_default_is_accepted() {
        let toml_str = r#"
            [[kanji]]
            char = "上"
            default = "ジョ]ウ"
        "#;
        let data: KanjiData = toml::from_str(toml_str).unwrap();
        assert_eq!(data.blocks[0].default, "ジョ]ウ");
    }

    // ─── Alternative (ADR-0004) ─────────────────────────────────────────────

    #[test]
    fn entry_with_alt_candidates_deserializes() {
        let toml_str = r#"
            [entries."上手"]
            reading = "ジョウズ"

            [[entries."上手".alt]]
            reading = "カミテ"
            sense = "stage-left"
            weight = 30

            [[entries."上手".alt]]
            reading = "ウワテ"
            sense = "superior"
            weight = 20
            prev_ends_any = ["より"]
        "#;
        let data: EntriesData = toml::from_str(toml_str).unwrap();
        let entry = data.entries.get("上手").unwrap();
        assert_eq!(entry.default_reading(), "ジョウズ");
        let alts = entry.alternatives();
        assert_eq!(alts.len(), 2);
        assert_eq!(alts[0].reading, "カミテ");
        assert_eq!(alts[0].sense.as_deref(), Some("stage-left"));
        assert_eq!(alts[0].weight, 30);
        assert_eq!(alts[1].reading, "ウワテ");
        assert_eq!(alts[1].weight, 20);
        assert_eq!(alts[1].condition.prev_ends_any, vec!["より"]);
    }

    #[test]
    fn entry_without_alt_has_empty_alternatives() {
        let toml_str = r#"
            [entries]
            "猫" = "ネコ"
        "#;
        let data: EntriesData = toml::from_str(toml_str).unwrap();
        assert!(data.entries.get("猫").unwrap().alternatives().is_empty());
    }

    #[test]
    fn alt_default_weight_is_50() {
        let toml_str = r#"
            [entries."橋"]
            reading = "ハシ"

            [[entries."橋".alt]]
            reading = "キョウ"
        "#;
        let data: EntriesData = toml::from_str(toml_str).unwrap();
        let alts = data.entries.get("橋").unwrap().alternatives();
        assert_eq!(alts[0].weight, 50);
        assert!(alts[0].sense.is_none());
    }

    #[test]
    fn kanji_block_with_alt_deserializes() {
        let toml_str = r#"
            [[kanji]]
            char = "生"
            default = "セイ"

            [[kanji.alt]]
            reading = "ナマ"
            sense = "raw"
            weight = 40

            [[kanji.alt]]
            reading = "イ"
            sense = "live"
            weight = 20
        "#;
        let data: KanjiData = toml::from_str(toml_str).unwrap();
        let block = &data.blocks[0];
        assert_eq!(block.alt.len(), 2);
        assert_eq!(block.alt[0].reading, "ナマ");
        assert_eq!(block.alt[1].reading, "イ");
    }
}

//! 絵文字の読み (emoji.toml)
//!
//! 絵文字 1 つ (肌色・ZWJ・国旗を含む 1 まとまり) → 読み。 読みが空文字なら 「読まない」。
//! 載っていない絵文字は従来どおり表記のまま通す。 文脈で読みが変わるものは、 漢字の辞書と同じ形の
//! `[[entries."X".match]]` で書く。
//!
//! ## 例
//! ```toml
//! [meta]
//! role = "emoji"
//!
//! [entries]
//! "⏰" = "めざましどけい|目覚まし|時計"   # 名詞の読み | 別表記
//! "😂" = ""                              # 読まない
//! "🍣" = "!すし"                          # 先頭 ! = 常に読む
//!
//! # 文脈で変わる読み。 match を上から見て、 最初に当たったものの reading を使う (空なら読まない)。
//! # どれにも当たらなければ上の reading (無ければ読まない) の規則に戻る
//! [entries."🙏"]
//! reading = ""                # 省略すると、 当たらない時は辞書に載っていないのと同じ (表記のまま)
//! [[entries."🙏".match]]
//! alone = true                  # 発言がこの絵文字だけの時
//! reading = "ありがとう"
//! ```
//!
//! キーは VS16 (U+FE0F) と肌色 (U+1F3FB..1F3FF) を除いた形で引く。 肌色つきの絵文字だけに別の読みを
//! 付けたい時は、 肌色まで書いた完全一致のキーが先に当たる。
//!
//! ## match の条件 (全部書いた条件を満たした時に当たる。 1 つは必要)
//! - `prev_ends_any`: 直前の文字列がどれかで終わる
//! - `next_starts_any`: 直後の文字列がどれかで始まる
//! - `input_contains_any`: 文のどこかにどれかを含む
//! - `alone = true`: 発言が (空白を除いて) この絵文字だけ

use serde::Deserialize;
use std::collections::HashMap;

/// 絵文字 1 つの項目: 読みだけの形と、 文脈で変わる形
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum EmojiEntry {
    /// `"⏰" = "めざましどけい|目覚まし"`
    Simple(String),
    /// `[entries."🙏"]` + `[[entries."🙏".match]]`
    Detailed {
        /// どの match にも当たらなかった時の読み (規則は Simple と同じ)。 省略 = 辞書に載っていない絵文字と同じ (表記のまま)
        #[serde(default)]
        reading: Option<String>,
        /// 文脈で変わる読み (上から順に見る)
        #[serde(default, rename = "match")]
        matches: Vec<EmojiMatch>,
    },
}

impl EmojiEntry {
    /// 文脈に関係なく使う読み (Simple の値、 または Detailed の既定)。 無ければ `None` (= 辞書に載っていないのと同じ)
    #[must_use]
    pub fn base(&self) -> Option<&str> {
        match self {
            Self::Simple(s) => Some(s),
            Self::Detailed { reading, .. } => reading.as_deref(),
        }
    }

    /// 文脈で当たる match の読み。 無ければ `None`
    #[must_use]
    pub fn matched(&self, prev: &str, after: &str, input: &str) -> Option<&str> {
        match self {
            Self::Simple(_) => None,
            Self::Detailed { matches, .. } => matches
                .iter()
                .find(|m| m.is_match(prev, after, input))
                .map(|m| m.reading.as_str()),
        }
    }
}

/// 文脈で変わる読み 1 つ
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmojiMatch {
    /// 直前の文字列がどれかで終わる
    #[serde(default)]
    pub prev_ends_any: Vec<String>,
    /// 直後の文字列がどれかで始まる
    #[serde(default)]
    pub next_starts_any: Vec<String>,
    /// 文のどこかにどれかを含む
    #[serde(default)]
    pub input_contains_any: Vec<String>,
    /// 発言が (空白を除いて) この絵文字だけ
    #[serde(default)]
    pub alone: bool,
    /// 当たった時の読み (空 = 読まない)
    #[serde(default)]
    pub reading: String,
}

impl EmojiMatch {
    /// 条件を 1 つ以上書いていて、 書いた条件が全部成り立つか
    #[must_use]
    pub fn is_match(&self, prev: &str, after: &str, input: &str) -> bool {
        let any_condition = !self.prev_ends_any.is_empty()
            || !self.next_starts_any.is_empty()
            || !self.input_contains_any.is_empty()
            || self.alone;
        any_condition
            && (self.prev_ends_any.is_empty()
                || self
                    .prev_ends_any
                    .iter()
                    .any(|w| prev.ends_with(w.as_str())))
            && (self.next_starts_any.is_empty()
                || self
                    .next_starts_any
                    .iter()
                    .any(|w| after.starts_with(w.as_str())))
            && (self.input_contains_any.is_empty()
                || self
                    .input_contains_any
                    .iter()
                    .any(|w| input.contains(w.as_str())))
            && (!self.alone || (prev.trim().is_empty() && after.trim().is_empty()))
    }
}

/// emoji.toml 全体 (絵文字 → 読み)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct EmojiData {
    /// 絵文字 → 項目
    #[serde(default)]
    pub entries: HashMap<String, EmojiEntry>,
}

impl EmojiData {
    /// 別 file の内容を足す。 同じキーは後勝ち。
    pub fn merge(&mut self, other: Self) {
        self.entries.extend(other.entries);
    }

    /// 件数
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 空判定
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// VS16 と肌色を除いた形 (引く時のキー)。 ZWJ は残す。
#[must_use]
pub fn strip_variation(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\u{FE0F}' | '\u{1F3FB}'..='\u{1F3FF}'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_and_detailed_entries_and_merge() {
        let mut a: EmojiData = toml::from_str(
            "[entries]\n\"⏰\" = \"めざましどけい\"\n[entries.\"🙏\"]\nreading = \"\"\n[[entries.\"🙏\".match]]\nalone = true\nreading = \"ありがとう\"\n",
        )
        .unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a.entries["⏰"].base(), Some("めざましどけい"));
        assert_eq!(a.entries["🙏"].base(), Some(""));
        let b: EmojiData = toml::from_str("[entries]\n\"⏰\" = \"とけい\"").unwrap();
        a.merge(b);
        assert_eq!(a.entries["⏰"].base(), Some("とけい"));
    }

    #[test]
    fn match_conditions_are_anded_and_need_at_least_one() {
        let e: EmojiData = toml::from_str(
            "[entries.\"🙏\"]\nreading = \"\"\n[[entries.\"🙏\".match]]\nalone = true\nreading = \"ありがとう\"\n[[entries.\"🙏\".match]]\nprev_ends_any = [\"お願い\"]\nnext_starts_any = [\"。\"]\nreading = \"x\"\n[[entries.\"🙏\".match]]\nreading = \"never\"\n",
        )
        .unwrap();
        let p = &e.entries["🙏"];
        // 絵文字だけの発言
        assert_eq!(p.matched("", "", "🙏"), Some("ありがとう"));
        assert_eq!(p.matched(" ", "\n", " 🙏\n"), Some("ありがとう"));
        // 他の文字がある時は alone に当たらない。 条件の無い match は当たらない
        assert_eq!(p.matched("お願い", "です", "お願い🙏です"), None);
        // 2 つの条件が両方成り立つ時だけ
        assert_eq!(
            p.matched("よろしくお願い", "。", "よろしくお願い🙏。"),
            Some("x")
        );
        assert_eq!(p.matched("よろしくお願い", "!", "よろしくお願い🙏!"), None);
    }

    #[test]
    fn strip_variation_removes_vs16_and_skin_tone_but_keeps_zwj() {
        assert_eq!(strip_variation("⏰\u{FE0F}"), "⏰");
        assert_eq!(strip_variation("👍\u{1F3FD}"), "👍");
        assert_eq!(strip_variation("👨\u{200D}👩"), "👨\u{200D}👩");
    }
}

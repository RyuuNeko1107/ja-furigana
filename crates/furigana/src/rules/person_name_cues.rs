//! 人名の手がかり (person_name_cues.toml)
//!
//! 人名の専用経路 ([`crate::scoring::person_names::PersonNamePass`]) が、 漢字の並びを人名と
//! 判断する手がかりの語。 辞書側のファイルが **正** で、 lib には既定値を持たない
//! (ファイルが無ければ手がかりは空 = 人名の専用経路は何もしない)。
//!
//! ```toml
//! [cues]
//! honorifics      = ["さん", "ちゃん", "くん", "氏"]   # 敬称 (肩書きより強い手がかり)
//! titles          = ["選手", "監督"]                    # 肩書き・職名
//! not_given_names = ["新人", "初代", "勝利"]            # 名の表にあっても名として扱わない語
//! compound_heads  = ["会", "団"]                        # 肩書き + この字で始まる語 = 別の複合語
//! ```
//!
//! 複数のファイルに分けて置いた場合は、 語が合算される (重複は除く)。

use serde::Deserialize;

/// 1 語の最大文字数。
pub const MAX_CUE_CHARS: usize = 16;
/// 1 つの表の最大語数。
pub const MAX_CUE_WORDS: usize = 1000;

/// `[cues]` の 4 つの表。
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
pub struct PersonCueLists {
    /// 敬称。 肩書きより強い手がかり
    #[serde(default)]
    pub honorifics: Vec<String>,
    /// 肩書き・職名
    #[serde(default)]
    pub titles: Vec<String>,
    /// 名の表にあっても名として扱わない語
    #[serde(default)]
    pub not_given_names: Vec<String>,
    /// 肩書きの直後にこの字で始まる token が続く時は、 別の複合語 (医師会 / 記者会見) で肩書きではない。
    /// 1 字ずつ
    #[serde(default)]
    pub compound_heads: Vec<String>,
}

/// person_name_cues.toml 全体。
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
pub struct PersonNameCuesData {
    /// 手がかりの表
    #[serde(default)]
    pub cues: PersonCueLists,
}

fn extend_unique(into: &mut Vec<String>, from: Vec<String>) {
    for w in from {
        if !into.contains(&w) {
            into.push(w);
        }
    }
}

impl PersonNameCuesData {
    /// 別 file の内容を足す。 重複は除く (並びは先に出た順)。
    pub fn merge(&mut self, other: Self) {
        let (a, b) = (&mut self.cues, other.cues);
        extend_unique(&mut a.honorifics, b.honorifics);
        extend_unique(&mut a.titles, b.titles);
        extend_unique(&mut a.not_given_names, b.not_given_names);
        extend_unique(&mut a.compound_heads, b.compound_heads);
    }

    /// 語の書式を検査する (load 時に、 file ごとと合算後に呼ぶ)。
    ///
    /// 辞書由来の値なので縛る: 空・空白だけ・空白や制御文字を含む語、 長すぎる語 (16 字超)、
    /// 多すぎる語数 (1 つの表で 1,000 語超)、 1 字でない `compound_heads` は不可。
    ///
    /// # Errors
    /// 上のいずれかに当たる時
    pub fn validate(&self) -> Result<(), String> {
        let c = &self.cues;
        for (name, list) in [
            ("honorifics", &c.honorifics),
            ("titles", &c.titles),
            ("not_given_names", &c.not_given_names),
            ("compound_heads", &c.compound_heads),
        ] {
            if list.len() > MAX_CUE_WORDS {
                return Err(format!(
                    "cues.{name} が {} 語 (上限 {MAX_CUE_WORDS})",
                    list.len()
                ));
            }
            for w in list {
                if w.trim().is_empty() {
                    return Err(format!("cues.{name} に空の語がある"));
                }
                if w.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
                    return Err(format!("cues.{name} {w:?} に空白・制御文字がある"));
                }
                if w.chars().count() > MAX_CUE_CHARS {
                    return Err(format!(
                        "cues.{name} {w:?} が {MAX_CUE_CHARS} 字を超えている"
                    ));
                }
                if name == "compound_heads" && w.chars().count() != 1 {
                    return Err(format!("cues.compound_heads {w:?} は 1 字にしてください"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> PersonNameCuesData {
        toml::from_str(s).unwrap()
    }

    #[test]
    fn merge_appends_without_duplicates() {
        let mut a = parse("[cues]\nhonorifics = [\"さん\"]\ntitles = [\"選手\"]");
        a.merge(parse(
            "[cues]\nhonorifics = [\"さん\", \"くん\"]\ntitles = [\"監督\"]\ncompound_heads = [\"会\"]",
        ));
        assert_eq!(a.cues.honorifics, ["さん", "くん"]);
        assert_eq!(a.cues.titles, ["選手", "監督"]);
        assert_eq!(a.cues.compound_heads, ["会"]);
        assert!(a.validate().is_ok());
    }

    #[test]
    fn missing_sections_are_empty() {
        let d = parse("");
        assert!(d.cues.honorifics.is_empty() && d.cues.titles.is_empty());
        assert!(d.validate().is_ok());
    }

    #[test]
    fn unknown_key_is_ignored_for_forward_compatibility() {
        // 新しい辞書が表を足しても、 古い lib が読み込みごと失敗しない
        let d = parse(
            "[cues]
honorifics = [\"さん\"]
future_list = [\"x\"]
[other]
a = 1",
        );
        assert_eq!(d.cues.honorifics, ["さん"]);
    }

    #[test]
    fn validate_rejects_bad_words() {
        let bad = |body: &str| parse(&format!("[cues]\n{body}")).validate().is_err();
        assert!(bad("titles = [\" \"]"));
        assert!(bad("titles = [\"選 手\"]"));
        assert!(bad("titles = [\"選\\n手\"]"));
        assert!(bad(&format!(
            "titles = [\"{}\"]",
            "あ".repeat(MAX_CUE_CHARS + 1)
        )));
        assert!(!bad(&format!(
            "titles = [\"{}\"]",
            "あ".repeat(MAX_CUE_CHARS)
        )));
        assert!(bad("compound_heads = [\"会団\"]"));
        assert!(!bad("compound_heads = [\"会\", \"団\"]"));
    }

    #[test]
    fn validate_rejects_too_many_words() {
        let mut d = PersonNameCuesData::default();
        d.cues.titles = (0..=MAX_CUE_WORDS).map(|i| format!("語{i}")).collect();
        assert!(d.validate().is_err());
        d.cues.titles.pop();
        assert!(d.validate().is_ok());
    }
}

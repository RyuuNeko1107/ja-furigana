//! 絵文字の読み (emoji.toml)
//!
//! 絵文字 1 つ (肌色・ZWJ・国旗を含む 1 まとまり) → 読み。 読みが空文字なら 「読まない」。
//! 載っていない絵文字は従来どおり表記のまま通す。
//!
//! ## 例
//! ```toml
//! [meta]
//! role = "emoji"
//!
//! [entries]
//! "⏰" = "めざましどけい"
//! "😂" = ""          # 読まない
//! "♪" = ""
//! ```
//!
//! キーは VS16 (U+FE0F) と肌色 (U+1F3FB..1F3FF) を除いた形で引く。 肌色つきの絵文字だけに別の読みを
//! 付けたい時は、 肌色まで書いた完全一致のキーが先に当たる。

use serde::Deserialize;
use std::collections::HashMap;

/// emoji.toml 全体 (絵文字 → 読み)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct EmojiData {
    /// 絵文字 → 読み (空文字 = 読まない)
    #[serde(default)]
    pub entries: HashMap<String, String>,
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
    fn parse_and_merge() {
        let mut a: EmojiData = toml::from_str("[entries]\n\"⏰\" = \"めざましどけい\"").unwrap();
        let b: EmojiData = toml::from_str("[entries]\n\"😂\" = \"\"\n\"⏰\" = \"とけい\"").unwrap();
        a.merge(b);
        assert_eq!(a.len(), 2);
        assert_eq!(a.entries["⏰"], "とけい");
        assert_eq!(a.entries["😂"], "");
    }

    #[test]
    fn strip_variation_removes_vs16_and_skin_tone_but_keeps_zwj() {
        assert_eq!(strip_variation("⏰\u{FE0F}"), "⏰");
        assert_eq!(strip_variation("👍\u{1F3FD}"), "👍");
        assert_eq!(strip_variation("👨\u{200D}👩"), "👨\u{200D}👩");
    }
}

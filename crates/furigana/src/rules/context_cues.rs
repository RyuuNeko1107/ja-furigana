//! 文脈の手がかり語 (context_cues.toml)
//!
//! 数字や記号の読みが文脈で変わるもの (`N/M` が日付か分数か) を、 手がかりになる語の一覧で
//! 辞書側から足せるようにする。 lib に組み込んだ手がかりに **足される** (置き換えない)。
//!
//! ```toml
//! [slash_pair]
//! date_after      = ["予約開始"]   # N/M の直後にあれば日付
//! date_before     = ["締切"]       # N/M の直前にあれば日付
//! date_topic      = ["ライブ"]     # 文のどこかにあれば日付の話題
//! fraction_after  = ["くらい"]     # N/M の直後にあれば分数
//! fraction_before = ["HP"]         # N/M の直前にあれば分数
//! fraction_topic  = ["スコア"]     # 文のどこかにあれば分数の話題 (日付の話題より優先)
//! ```

use serde::Deserialize;

/// `N/M` の手がかり語。
#[derive(Debug, Default, Clone, Deserialize)]
pub struct SlashPairCues {
    /// 直後にあれば日付
    #[serde(default)]
    pub date_after: Vec<String>,
    /// 直前にあれば日付
    #[serde(default)]
    pub date_before: Vec<String>,
    /// 文のどこかにあれば日付の話題
    #[serde(default)]
    pub date_topic: Vec<String>,
    /// 直後にあれば分数
    #[serde(default)]
    pub fraction_after: Vec<String>,
    /// 直前にあれば分数
    #[serde(default)]
    pub fraction_before: Vec<String>,
    /// 文のどこかにあれば分数の話題
    #[serde(default)]
    pub fraction_topic: Vec<String>,
}

/// 伏せ字 (`■さん` / `△△様`) の手がかり語。
#[derive(Debug, Default, Clone, Deserialize)]
pub struct PlaceholderCues {
    /// 図形記号の直後に付く敬称・肩書き (`■さん` のように 1 つだけでも伏せ字として読む)
    #[serde(default)]
    pub honorifics: Vec<String>,
}

/// context_cues.toml 全体。
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ContextCuesData {
    /// `N/M` (日付か分数か)
    #[serde(default)]
    pub slash_pair: SlashPairCues,
    /// 伏せ字の図形記号
    #[serde(default)]
    pub placeholder: PlaceholderCues,
}

impl ContextCuesData {
    /// 別 file の内容を足す。
    pub fn merge(&mut self, other: Self) {
        let (a, b) = (&mut self.slash_pair, other.slash_pair);
        a.date_after.extend(b.date_after);
        a.date_before.extend(b.date_before);
        a.date_topic.extend(b.date_topic);
        a.fraction_after.extend(b.fraction_after);
        a.fraction_before.extend(b.fraction_before);
        a.fraction_topic.extend(b.fraction_topic);
        self.placeholder
            .honorifics
            .extend(other.placeholder.honorifics);
    }

    /// 空の語が無いかを検査する (load 時に呼ぶ)。
    ///
    /// # Errors
    /// 空文字の語がある時 (空の語は全ての文に当たってしまう)
    pub fn validate(&self) -> Result<(), String> {
        let s = &self.slash_pair;
        for (name, list) in [
            ("date_after", &s.date_after),
            ("date_before", &s.date_before),
            ("date_topic", &s.date_topic),
            ("fraction_after", &s.fraction_after),
            ("fraction_before", &s.fraction_before),
            ("fraction_topic", &s.fraction_topic),
        ] {
            if list.iter().any(|w| w.trim().is_empty()) {
                return Err(format!("slash_pair.{name} に空の語がある"));
            }
        }
        if self
            .placeholder
            .honorifics
            .iter()
            .any(|w| w.trim().is_empty())
        {
            return Err("placeholder.honorifics に空の語がある".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_appends_and_validate_rejects_empty_words() {
        let mut a: ContextCuesData =
            toml::from_str("[slash_pair]\ndate_topic = [\"配信\"]").unwrap();
        let b: ContextCuesData = toml::from_str(
            "[slash_pair]\ndate_topic = [\"ライブ\"]\nfraction_after = [\"くらい\"]",
        )
        .unwrap();
        a.merge(b);
        assert_eq!(a.slash_pair.date_topic, ["配信", "ライブ"]);
        assert_eq!(a.slash_pair.fraction_after, ["くらい"]);
        assert!(a.validate().is_ok());
        let bad: ContextCuesData = toml::from_str("[slash_pair]\ndate_after = [\" \"]").unwrap();
        assert!(bad.validate().is_err());
        // 伏せ字の敬称も足され、 空の語は弾く
        let mut p: ContextCuesData =
            toml::from_str("[placeholder]\nhonorifics = [\"さん\"]").unwrap();
        p.merge(toml::from_str("[placeholder]\nhonorifics = [\"師匠\"]").unwrap());
        assert_eq!(p.placeholder.honorifics, ["さん", "師匠"]);
        assert!(p.validate().is_ok());
        let bad: ContextCuesData = toml::from_str("[placeholder]\nhonorifics = [\" \"]").unwrap();
        assert!(bad.validate().is_err());
    }
}

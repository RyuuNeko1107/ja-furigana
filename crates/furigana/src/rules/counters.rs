//! 助数詞ルール (counters.toml)
//!
//! 助数詞ごとの連濁 / 促音化 / 数字依存特殊読みを TOML で表現する。
//!
//! ## 例
//! ```toml
//! [simple]
//! "円" = "エン"
//! "点" = "テン"
//!
//! [counter."本"]
//! default = "ホン"
//!
//! [[counter."本".rules]]
//! last_digit = [1, 6, 8, 0]
//! suffix = "ポン"
//! sokuonize = true
//!
//! [[counter."本".rules]]
//! last_digit = [3]
//! suffix = "ボン"
//!
//! [counter."月"]
//! default = "ガツ"
//! specials = { "4" = "シガツ", "7" = "シチガツ", "9" = "クガツ" }
//!
//! [counter."目"]
//! mode = "recursive"
//! suffix = "メ"
//! ```

use serde::Deserialize;
use std::collections::HashMap;

/// counters.toml 全体
#[derive(Debug, Default, Clone, Deserialize)]
pub struct CountersData {
    /// 単純サフィックス: 数値の読み + ここに書いた文字列を連結するだけ
    #[serde(default)]
    pub simple: HashMap<String, String>,

    /// 連濁・促音化等のルールを伴う助数詞
    #[serde(default)]
    pub counter: HashMap<String, CounterRule>,
}

impl CountersData {
    /// 別の `CountersData` をマージする (other の同名 key は self を上書き)
    ///
    /// counters/ サブディレクトリ配下の複数 TOML を 1 つの構造体に
    /// 統合する用途で [`crate::loader::load_rules_dir`] 内部から呼ばれる。
    pub fn merge(&mut self, other: Self) {
        self.simple.extend(other.simple);
        self.counter.extend(other.counter);
    }
}

/// 助数詞 1 件の振る舞い
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CounterRule {
    /// デフォルト suffix (例: 本→「ホン」)。
    /// `mode = "recursive"` の場合は不要 (None 可)。
    #[serde(default)]
    pub default: Option<String>,

    /// 末尾数字に応じた連濁・促音化
    #[serde(default)]
    pub rules: Vec<EuphonicRule>,

    /// 数値そのもの (string キー) に対する特殊読み (例: 月で 4→シガツ)。
    /// マッチした場合は `default` の suffix 連結すらせず、
    /// この値をそのまま読みとして採用する (フル上書き)。
    #[serde(default)]
    pub specials: HashMap<String, String>,

    /// 数値カナの末尾置換 (時の 4→ヨン→ヨ, 7→ナナ→シチ, 9→キュウ→ク 等)。
    /// `last_digit` が一致した時、数値カナが `from` で終わっていれば
    /// `to` に置換し、その後 default suffix を付ける。
    #[serde(default)]
    pub replacements: Vec<KanaReplacement>,

    /// 「目」のように既存助数詞末尾に再帰的に付く形式の場合のモード
    #[serde(default)]
    pub mode: Option<CounterMode>,

    /// `mode = "recursive"` の際に末尾連結する suffix (例: 目→「メ」)
    #[serde(default)]
    pub suffix: Option<String>,

    /// **漢数字 (五/七/十 等) を bare で counter 化するか** (default false)。
    ///
    /// false (従来挙動): bare 漢数字 + 助数詞 (= 「五匹」「一日」) は counter candidate に
    /// せず Lindera / chunker に委ねる。「一日中」 の 「一日」 が誤って counter 化するのを防ぐ。
    /// true: 「五匹→ごひき」「三羽→さんば」 等を NumberProvider 側で連濁・促音込みで読む。
    /// counter 用法が支配的で非 counter 文脈との衝突が無い助数詞 (匹/羽/頭/杯 等) のみ opt-in する。
    #[serde(default)]
    pub kanji_numeral: bool,
}

/// 数値カナの末尾置換ルール
///
/// 例: 時の 4 (ヨン→ヨ) — 4時 = ヨジ、14時 = ジュウヨジ
#[derive(Debug, Clone, Deserialize)]
pub struct KanaReplacement {
    /// このルールが適用される末尾数字 (0〜9)
    pub last_digit: Vec<u32>,
    /// 置換対象の末尾文字列 (例: "ヨン")
    pub from: String,
    /// 置換後の文字列 (例: "ヨ")
    pub to: String,
}

/// 特殊な助数詞処理モード
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CounterMode {
    /// 末尾再帰: ベース助数詞 (例: 「個目」の「個」) を解決した後、
    /// `suffix` (例: 「メ」) を末尾に連結する
    Recursive,
}

/// 末尾数字に依存する連濁・促音化ルール
#[derive(Debug, Clone, Deserialize)]
pub struct EuphonicRule {
    /// このルールが適用される末尾数字 (0〜9)
    pub last_digit: Vec<u32>,

    /// 連結する suffix (例: 本→「ポン」、匹→「ピキ」)
    pub suffix: String,

    /// 直前のカタカナ末尾を促音化するか (例: イチ→イッ)
    #[serde(default)]
    pub sokuonize: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_section() {
        let toml_str = r#"
            [simple]
            "円" = "エン"
            "点" = "テン"
        "#;
        let data: CountersData = toml::from_str(toml_str).unwrap();
        assert_eq!(data.simple.get("円").map(String::as_str), Some("エン"));
        assert_eq!(data.simple.get("点").map(String::as_str), Some("テン"));
    }

    #[test]
    fn parses_complex_counter_with_euphonic_rules() {
        let toml_str = r#"
            [counter."本"]
            default = "ホン"

            [[counter."本".rules]]
            last_digit = [1, 6, 8, 0]
            suffix = "ポン"
            sokuonize = true

            [[counter."本".rules]]
            last_digit = [3]
            suffix = "ボン"
        "#;
        let data: CountersData = toml::from_str(toml_str).unwrap();
        let hon = data.counter.get("本").expect("counter.本 が無い");
        assert_eq!(hon.default.as_deref(), Some("ホン"));
        assert_eq!(hon.rules.len(), 2);

        let pon = &hon.rules[0];
        assert_eq!(pon.last_digit, vec![1, 6, 8, 0]);
        assert_eq!(pon.suffix, "ポン");
        assert!(pon.sokuonize);

        let bon = &hon.rules[1];
        assert_eq!(bon.last_digit, vec![3]);
        assert_eq!(bon.suffix, "ボン");
        assert!(!bon.sokuonize);
    }

    #[test]
    fn parses_specials() {
        let toml_str = r#"
            [counter."月"]
            default = "ガツ"
            specials = { "4" = "シガツ", "7" = "シチガツ", "9" = "クガツ" }
        "#;
        let data: CountersData = toml::from_str(toml_str).unwrap();
        let tsuki = data.counter.get("月").unwrap();
        assert_eq!(tsuki.specials.get("4").map(String::as_str), Some("シガツ"));
        assert_eq!(
            tsuki.specials.get("7").map(String::as_str),
            Some("シチガツ")
        );
        assert_eq!(tsuki.specials.get("9").map(String::as_str), Some("クガツ"));
    }

    #[test]
    fn parses_recursive_mode() {
        let toml_str = r#"
            [counter."目"]
            mode = "recursive"
            suffix = "メ"
        "#;
        let data: CountersData = toml::from_str(toml_str).unwrap();
        let me = data.counter.get("目").unwrap();
        assert_eq!(me.mode, Some(CounterMode::Recursive));
        assert_eq!(me.suffix.as_deref(), Some("メ"));
        assert!(me.default.is_none());
    }

    #[test]
    fn empty_input_yields_default() {
        let data: CountersData = toml::from_str("").unwrap();
        assert!(data.simple.is_empty());
        assert!(data.counter.is_empty());
    }

    #[test]
    fn parses_kana_replacements() {
        let toml_str = r#"
            [counter."時"]
            default = "ジ"
            specials = { "0" = "レイジ" }

            [[counter."時".replacements]]
            last_digit = [4]
            from = "ヨン"
            to = "ヨ"

            [[counter."時".replacements]]
            last_digit = [7]
            from = "ナナ"
            to = "シチ"

            [[counter."時".replacements]]
            last_digit = [9]
            from = "キュウ"
            to = "ク"
        "#;
        let data: CountersData = toml::from_str(toml_str).unwrap();
        let h = data.counter.get("時").unwrap();
        assert_eq!(h.default.as_deref(), Some("ジ"));
        assert_eq!(h.replacements.len(), 3);
        assert_eq!(h.replacements[0].from, "ヨン");
        assert_eq!(h.replacements[0].to, "ヨ");
        assert_eq!(h.specials.get("0").map(String::as_str), Some("レイジ"));
    }
}

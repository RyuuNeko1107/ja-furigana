//! 型番パターン (models.toml)
//!
//! PC 部品・端末の型番 (`RTX 4070 Ti` / `Ryzen 7 5800X3D` / `i9-13900K` / `iPhone 15 Pro`) を、
//! 正規表現 + 読みの雛形で宣言する。 数字の読み方も英字の読み方も語ごとに違うので、
//! 型番ごとに 「どの部分をどう読むか」 を書く。
//!
//! ```toml
//! [parts]            # 接尾辞の読み (大文字小文字は区別しない)。 無い部分は英字 1 字ずつ
//! "Ti" = "ティーアイ"
//!
//! [[model]]
//! pattern = 'RTX\s?([0-9]{4})(?:\s?(Ti|Super))?'
//! reading = "アールティーエックス{1:each}{2:part}"
//! ```
//!
//! - `pattern`: 先頭一致の正規表現 (大文字小文字は区別しない)。 取りたい部分は括弧で囲む
//! - `reading`: カタカナ + `{N}` / `{N:style}` (N は括弧の番号、 マッチしなかった括弧は空)
//!   - style: `place` / `each` / `each_en` / `english` / `pair` / `pair_en` (数字の読み方、
//!     `[digits]` と同じ)、 `dotted` (5.0 = ゴテンゼロ)、 `part` (`[parts]` の読み、 無ければ `spell`)、
//!     `spell` (英字を 1 字ずつ、 数字は英語)
//!   - 省略時: 数字だけなら `each`、 それ以外は `part`
//! - `first`: 先頭になりうる英字 (省略時は `pattern` の頭の英字から取る)。 検索の絞り込み用

use serde::Deserialize;
use std::collections::HashMap;

/// 型番 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModelSpec {
    /// 先頭一致の正規表現
    pub pattern: String,
    /// 読みの雛形
    pub reading: String,
    /// 先頭になりうる英字 (省略時は pattern の頭から取る)
    #[serde(default)]
    pub first: Option<String>,
}

/// models.toml 全体。
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ModelsData {
    /// 部品 (接尾辞) の読み
    #[serde(default)]
    pub parts: HashMap<String, String>,
    /// 型番パターン
    #[serde(default)]
    pub model: Vec<ModelSpec>,
}

impl ModelsData {
    /// 別 file の内容を足す。
    pub fn merge(&mut self, other: Self) {
        self.parts.extend(other.parts);
        self.model.extend(other.model);
    }

    /// 全 pattern / 雛形を検査する (load 時に呼ぶ)。 最初の誤りを返す。
    ///
    /// # Errors
    /// 正規表現が不正 / 雛形の `{N}` が括弧の数を超える / style が未知 / 絞り込み用の英字が取れない時
    pub fn validate(&self) -> Result<(), String> {
        for spec in &self.model {
            crate::scoring::model_number::CompiledModel::compile(spec)
                .map_err(|e| format!("model '{}': {e}", spec.pattern))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_parts_and_models() {
        let d: ModelsData = toml::from_str(
            r#"
            [parts]
            "Ti" = "ティーアイ"
            [[model]]
            pattern = 'RTX\s?([0-9]{4})'
            reading = "アールティーエックス{1:each}"
            "#,
        )
        .unwrap();
        assert_eq!(d.parts["Ti"], "ティーアイ");
        assert_eq!(d.model.len(), 1);
        assert_eq!(d.model[0].first, None);
        assert!(d.validate().is_ok());
    }

    #[test]
    fn merge_appends_both() {
        let mut a: ModelsData =
            toml::from_str("[parts]\n\"a\"=\"エー\"\n[[model]]\npattern='RTX'\nreading='ア'")
                .unwrap();
        let b: ModelsData =
            toml::from_str("[parts]\n\"b\"=\"ビー\"\n[[model]]\npattern='GTX'\nreading='イ'")
                .unwrap();
        a.merge(b);
        assert_eq!(a.parts.len(), 2);
        assert_eq!(a.model.len(), 2);
    }

    #[test]
    fn validate_rejects_bad_models() {
        let bad = |toml_str: &str| -> String {
            let d: ModelsData = toml::from_str(toml_str).unwrap();
            d.validate().unwrap_err()
        };
        // 不正な正規表現
        assert!(bad("[[model]]\npattern='RTX(['\nreading='ア'").contains("RTX(["));
        // 括弧より多い番号
        assert!(bad("[[model]]\npattern='RTX([0-9])'\nreading='ア{2}'").contains("{2}"));
        // 未知の style
        assert!(bad("[[model]]\npattern='RTX([0-9])'\nreading='ア{1:nope}'").contains("nope"));
        // 先頭の英字が取れず first も無い
        assert!(bad("[[model]]\npattern='([A-Z])[0-9]'\nreading='ア{1}'").contains("first"));
        // 閉じていない {
        assert!(bad("[[model]]\npattern='RTX'\nreading='ア{1'").contains('{'));
    }
}

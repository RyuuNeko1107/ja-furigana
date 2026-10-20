//! 後処理ルール (regex ベースの mode 別置換)
//!
//! [`crate::Furigana::to_hiragana`] / `to_ruby` / `to_tts` / `to_romaji` の
//! 出力直前に適用される regex 置換ルール (Step 7 の正規表現ベース置換)。
//!
//! ## scoring engine との関係 (★C4)
//!
//! 本 layer は **scoring engine の score / candidate logic とは独立**:
//!
//! - scoring engine (`crate::scoring`) は input → candidate path 確定 までを担当
//! - postprocess は path 確定後の **mode 別 output 生成 layer**、 文字列 regex 置換のみ
//! - したがって [`crate::Furigana::analyze`] (= scoring engine debug API) は
//!   postprocess を **適用しない**、 caller は raw token reading を見る
//! - [`crate::Furigana::to_hiragana`] 等の本流 method のみで postprocess が走る
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §5.6 (「scoring-engine とは別軸 で維持」)
//!
//! ## ユースケース
//!
//! - 文字列レベルの最終調整 (「ゴジュウパー → ゴジュッパー」等の促音化補正)
//! - mode 別の挙動分岐 (`tts` のみで句読点後の半角スペース除去 等)
//! - 辞書 / context rule で表現しづらい conjugation の手当て
//!
//! ## TOML 形式
//!
//! ```toml
//! [[rule]]
//! pattern = "ジュウパー"
//! replacement = "ジュッパー"
//! modes = ["hiragana", "tts", "romaji"]   # 空 or 省略 = すべての mode
//!
//! [[rule]]
//! pattern = "(\\d+)\\s*ヶ"
//! replacement = "$1カ"   # キャプチャグループ参照可
//! ```
//!
//! ## ステータス
//!
//! Phase 1: 構造定義 + apply 実装。`furigana-dict/rules/postprocess.toml` が
//! 存在しなければ no-op。Phase 2 で具体的な rule を seed 投入する。

use serde::Deserialize;

/// `replacement` 1 件の長さ上限 (byte)。 空マッチする規則と組み合わさると、 入力の文字数 ×
/// replacement の長さの出力ができるため。 実際の規則は数 byte。
const MAX_REPLACEMENT_BYTES: usize = 1024;
/// 規則の件数上限。 規則は上から順に全部走るので、 件数がそのまま 1 変換あたりの処理量になる。
const MAX_RULES: usize = 10_000;
/// 出力が入力の何倍まで伸びてよいか (+ [`GROWTH_SLACK_BYTES`])。 超えたら以降の規則を適用しない。
const MAX_GROWTH_FACTOR: usize = 8;
const GROWTH_SLACK_BYTES: usize = 4096;

/// 後処理ルール TOML 全体
///
/// `[[rule]]` array をそのまま受ける defensive な型。pattern parse は
/// 構築時に実行 ([`Self::from_spec`])、生 string のまま `apply` で都度
/// コンパイルしないことで重複コストを避ける。
#[derive(Debug, Default, Clone)]
pub struct PostProcessData {
    rules: Vec<CompiledRule>,
}

/// コンパイル済みの単一ルール (内部表現)
#[derive(Debug, Clone)]
struct CompiledRule {
    re: regex::Regex,
    replacement: String,
    /// 空なら全 mode に適用、空でなければ含まれる mode にのみ適用
    modes: Vec<String>,
}

/// TOML から直接読む型 (deserialize 用)
#[derive(Debug, Default, Deserialize)]
pub struct PostProcessSpec {
    /// `[[rule]]` の集合。 loader が複数 file を merge する際に直接 extend する用途で公開。
    #[serde(default, rename = "rule")]
    pub rules: Vec<PostProcessRuleSpec>,
}

/// TOML の `[[rule]]` 1 件分
#[derive(Debug, Deserialize)]
pub struct PostProcessRuleSpec {
    pub pattern: String,
    pub replacement: String,
    #[serde(default)]
    pub modes: Vec<String>,
}

impl PostProcessData {
    /// `PostProcessSpec` から構築 (regex pre-compile、不正パターンは Err)
    ///
    /// # Errors
    /// regex パターンのコンパイル失敗時。
    pub fn from_spec(spec: PostProcessSpec) -> Result<Self, regex::Error> {
        if spec.rules.len() > MAX_RULES {
            return Err(regex::Error::Syntax(format!(
                "postprocess の規則が多すぎます: {} 件 (上限 {MAX_RULES})",
                spec.rules.len()
            )));
        }
        let rules = spec
            .rules
            .into_iter()
            .map(|r| {
                if r.replacement.len() > MAX_REPLACEMENT_BYTES {
                    return Err(regex::Error::Syntax(format!(
                        "postprocess の replacement が長すぎます: {} bytes (上限 {MAX_REPLACEMENT_BYTES})",
                        r.replacement.len()
                    )));
                }
                // size_limit で巨大 regex (compile 後 DFA / NFA size) を拒否。
                // postprocess.toml の pattern は dist (or user) 制御の data なので、
                // 攻撃者が regex bomb を仕込んで memory を食わせるのを防御。
                // 10 MB は通常 pattern (数百 byte) を十分カバー。
                let re = regex::RegexBuilder::new(&r.pattern)
                    .size_limit(10 * 1024 * 1024)
                    .build()?;
                Ok(CompiledRule {
                    re,
                    replacement: r.replacement,
                    modes: r.modes,
                })
            })
            .collect::<Result<Vec<_>, regex::Error>>()?;
        Ok(Self { rules })
    }

    /// 該当 mode の rule を上から順に適用
    ///
    /// rules が空なら text をそのまま返す (no-op)。
    /// `mode` は `"hiragana"` / `"ruby"` / `"tts"` / `"romaji"` 等の文字列。
    #[must_use]
    pub fn apply(&self, text: &str, mode: &str) -> String {
        if self.rules.is_empty() {
            return text.to_string();
        }
        let limit = text
            .len()
            .saturating_mul(MAX_GROWTH_FACTOR)
            .saturating_add(GROWTH_SLACK_BYTES);
        let mut current = text.to_string();
        for r in &self.rules {
            if r.modes.is_empty() || r.modes.iter().any(|m| m == mode) {
                let next =
                    r.re.replace_all(&current, r.replacement.as_str())
                        .into_owned();
                // 空マッチする規則の連鎖で出力が膨らむのを止める (その規則の結果は採らない)
                if next.len() > limit {
                    tracing::warn!(
                        "postprocess: 出力が入力の {MAX_GROWTH_FACTOR} 倍を超えるため、 以降の規則を適用しません"
                    );
                    return current;
                }
                current = next;
            }
        }
        current
    }

    /// rule 件数 (debug 用)
    #[must_use]
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// 空判定
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_no_op() {
        let p = PostProcessData::default();
        assert_eq!(p.apply("こんにちは", "hiragana"), "こんにちは");
    }

    #[test]
    fn applies_simple_replacement() {
        let spec = PostProcessSpec {
            rules: vec![PostProcessRuleSpec {
                pattern: r"ジュウパー".to_string(),
                replacement: "ジュッパー".to_string(),
                modes: vec![],
            }],
        };
        let p = PostProcessData::from_spec(spec).unwrap();
        assert_eq!(
            p.apply("ゴジュウパーセント", "hiragana"),
            "ゴジュッパーセント"
        );
    }

    #[test]
    fn mode_filter_excludes_other_modes() {
        let spec = PostProcessSpec {
            rules: vec![PostProcessRuleSpec {
                pattern: r"AAA".to_string(),
                replacement: "BBB".to_string(),
                modes: vec!["tts".to_string()],
            }],
        };
        let p = PostProcessData::from_spec(spec).unwrap();
        assert_eq!(p.apply("AAA", "tts"), "BBB");
        assert_eq!(p.apply("AAA", "hiragana"), "AAA"); // mode 違うので no-op
    }

    #[test]
    fn capture_group_reference_works() {
        let spec = PostProcessSpec {
            rules: vec![PostProcessRuleSpec {
                pattern: r"(\d+)ヶ".to_string(),
                replacement: "$1カ".to_string(),
                modes: vec![],
            }],
        };
        let p = PostProcessData::from_spec(spec).unwrap();
        assert_eq!(p.apply("3ヶ月", "hiragana"), "3カ月");
    }

    #[test]
    fn invalid_pattern_errors() {
        let spec = PostProcessSpec {
            rules: vec![PostProcessRuleSpec {
                pattern: r"[invalid".to_string(),
                replacement: "X".to_string(),
                modes: vec![],
            }],
        };
        assert!(PostProcessData::from_spec(spec).is_err());
    }

    fn rule(pattern: &str, replacement: &str) -> PostProcessRuleSpec {
        PostProcessRuleSpec {
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            modes: vec![],
        }
    }

    #[test]
    fn rejects_oversize_replacement_and_too_many_rules() {
        let long = "x".repeat(MAX_REPLACEMENT_BYTES + 1);
        let spec = PostProcessSpec {
            rules: vec![rule("a", &long)],
        };
        assert!(PostProcessData::from_spec(spec).is_err());
        let ok = "x".repeat(MAX_REPLACEMENT_BYTES);
        let spec = PostProcessSpec {
            rules: vec![rule("a", &ok)],
        };
        assert!(PostProcessData::from_spec(spec).is_ok());
        let spec = PostProcessSpec {
            rules: (0..=MAX_RULES).map(|_| rule("a", "b")).collect(),
        };
        assert!(PostProcessData::from_spec(spec).is_err());
    }

    #[test]
    fn chained_empty_match_rules_cannot_blow_up_the_output() {
        // 空マッチ × 100 byte の規則 3 連: 80 字の入力が約 100 MB になっていた
        let spec = PostProcessSpec {
            rules: (0..3).map(|_| rule("", &"x".repeat(100))).collect(),
        };
        let p = PostProcessData::from_spec(spec).unwrap();
        let input = "今日は天気がいい".repeat(10);
        let out = p.apply(&input, "tts");
        assert!(
            out.len() <= input.len() * MAX_GROWTH_FACTOR + GROWTH_SLACK_BYTES,
            "出力は {} bytes",
            out.len()
        );
        // 止めた規則の結果は採らない = 上限を超えた時点の直前の文字列
        assert!(out.starts_with('今'));
    }

    #[test]
    fn normal_rules_are_unaffected_by_the_growth_limit() {
        let spec = PostProcessSpec {
            rules: vec![rule("a", "bbbb"), rule("b", "cc")],
        };
        let p = PostProcessData::from_spec(spec).unwrap();
        assert_eq!(p.apply("aaa", "tts"), "c".repeat(24));
    }
}

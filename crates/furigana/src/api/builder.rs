//! [`FuriganaBuilder`] と辞書・規則のローダ

use super::Furigana;
use crate::dict::Dict;
use crate::error::Result;
use crate::rules::CompatData;
use crate::scoring::numbers::NumberCandidateProvider;
use crate::scoring::special::normalize_alphabet;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// [`Furigana`] を段階的に構築する builder
///
/// 全フィールド optional。指定しなければデフォルト (空) が使われる。
/// Dict は core → user → overrides → add_entry の順にマージされ、
/// 後のものが優先 (override) される。
#[derive(Debug, Default)]
pub struct FuriganaBuilder {
    rules_dir: Option<PathBuf>,
    core_dict_dirs: Vec<PathBuf>,
    user_dict_dirs: Vec<PathBuf>,
    overrides_files: Vec<PathBuf>,
    extra_entries: Vec<(String, String)>,
    estimate_accent: bool,
    /// 読まない dict path (dict dir からの相対、 `exclude_dict_path`)
    dict_excludes: Vec<String>,
    /// アクセント専用の表を読まない (`accent_lexicon(false)`)。 Default を false = 読む にするため否定形で持つ
    no_accent_lexicon: bool,
    cost_engine: bool,
}

impl FuriganaBuilder {
    /// 空の builder を作る
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// ルール TOML をディレクトリから読み込む (デフォルト空を上書き)
    #[must_use]
    pub fn rules_dir(mut self, p: impl AsRef<Path>) -> Self {
        self.rules_dir = Some(p.as_ref().to_path_buf());
        self
    }

    /// core 辞書ディレクトリを追加 (複数指定可、優先度: 低)
    #[must_use]
    pub fn core_dict_dir(mut self, p: impl AsRef<Path>) -> Self {
        self.core_dict_dirs.push(p.as_ref().to_path_buf());
        self
    }

    /// user 辞書ディレクトリを追加 (複数指定可、優先度: 中)
    #[must_use]
    pub fn user_dict_dir(mut self, p: impl AsRef<Path>) -> Self {
        self.user_dict_dirs.push(p.as_ref().to_path_buf());
        self
    }

    /// overrides TOML ファイルを追加 (複数指定可、優先度: 高)
    #[must_use]
    pub fn overrides_file(mut self, p: impl AsRef<Path>) -> Self {
        self.overrides_files.push(p.as_ref().to_path_buf());
        self
    }

    /// 個別エントリをコード上で追加 (優先度: 最高)
    #[must_use]
    pub fn add_entry(mut self, surface: impl Into<String>, reading: impl Into<String>) -> Self {
        self.extra_entries.push((surface.into(), reading.into()));
        self
    }

    /// rule-based accent 推定を有効化する (opt-in、 ADR-0007。 default: off)。
    ///
    /// 有効時、 dict bracket notation を持たない token に **決定論的な frozen rule**
    /// (外来語 -3 rule / 人名 rule) で accent を推定し、 `AccentPhrase` の
    /// `estimated: true` で真値 (dict 由来) と区別できる形で埋める。
    /// 推定できない token は従来どおり `accent_phrases` 空 (= accent 不明)。
    ///
    /// 読み出力 (`to_hiragana` / `to_ruby` 等) には影響しない。
    #[must_use]
    pub fn estimate_accent(mut self, enabled: bool) -> Self {
        self.estimate_accent = enabled;
        self
    }

    /// core / user dict dir のうち、 この相対 path 配下 (または この file) を読まない。
    ///
    /// 用途に合わない分野の辞書を外すためのもの。 pattern は各 dict dir からの相対 path を `/` 区切りで書き、
    /// dir なら配下全部、 file なら 1 file (`.toml` は省略可)。 何度でも呼べる (全部外す)。
    ///
    /// ```no_run
    /// # use furigana::Furigana;
    /// // ニュース読み上げ: 作品・VTuber 名と麻雀用語を外す
    /// let f = Furigana::builder()
    ///     .core_dict_dir("furigana-dict/core")
    ///     .exclude_dict_path("works")
    ///     .exclude_dict_path("jukugo/society/mahjong")
    ///     .build()?;
    /// # Ok::<(), furigana::FuriganaError>(())
    /// ```
    ///
    /// 読み (jukugo / kanji / unihan / works) / 外来語 / 異体字 / アクセント表の全部に効く。
    /// rules_dir と overrides file には効かない。
    #[must_use]
    pub fn exclude_dict_path(mut self, path: impl Into<String>) -> Self {
        self.dict_excludes.push(path.into());
        self
    }

    /// dict のアクセント専用の表 (`role = "accent"`) を使うか (default: on)。
    ///
    /// on の時、 dict bracket notation を持たない token に、 表記 + 読みが一致する表の accent を付ける
    /// (適用順は dict bracket → この表 → [`Self::estimate_accent`] の推定)。 表は数万件規模なので、
    /// accent を使わない (読みだけ欲しい) 用途は off にすると読み込み時間と常駐メモリを節約できる。
    /// 読み出力には影響しない。
    #[must_use]
    pub fn accent_lexicon(mut self, enabled: bool) -> Self {
        self.no_accent_lexicon = !enabled;
        self
    }

    /// コスト lattice engine (ADR-0011) を使う (実験中の opt-in)。
    ///
    /// IPADIC の語コスト + 連接コストと dict 候補を 1 本の lattice に並べ、
    /// コスト最小 path を選ぶ。 環境変数 `FURIGANA_COST_ENGINE=1` でも有効になる。
    #[must_use]
    pub fn cost_engine(mut self, enabled: bool) -> Self {
        self.cost_engine = enabled;
        self
    }

    /// [`Furigana`] を構築
    ///
    /// 形態素解析器 (Lindera + IPADIC) は **lazy init** — 構築時には初期化せず、
    /// 最初の `tokenize` / `to_*` 呼び出し時に 1 度だけ初期化される。サーバー
    /// 起動時に init コストを払いたい場合は [`Furigana::preload`] を呼ぶ。
    ///
    /// # Errors
    /// - ルールファイルパース失敗 ([`crate::FuriganaError::Toml`])
    /// - 辞書ファイル/ディレクトリ I/O 失敗
    pub fn build(self) -> Result<Furigana> {
        let mut rules = match self.rules_dir.as_ref() {
            Some(dir) => crate::loader::load_rules_dir(dir)?,
            None => crate::embedded::rules()?,
        };

        // 異体字正規化 (compat) は `load_rules_dir` が rules_dir しか走査しないが、
        // ja-furigana-dict 配布では `compat.toml` が **core/ に同梱**される。 production の
        // wrapper のように rules_dir と core_dict_dir を別 path で mount すると compat が
        // 一切読まれず 髙→高 / 﨑→崎 が無効化される (構造的 trap)。 ここで core / user dict
        // 配下の `role = "compat"` も拾って rules.compat に補完する (rules_dir 由来を優先)。
        for d in &self.core_dict_dirs {
            load_compat_into(&mut rules.compat, d, &self.dict_excludes)?;
        }
        for d in &self.user_dict_dirs {
            load_compat_into(&mut rules.compat, d, &self.dict_excludes)?;
        }

        let mut dict = Dict::new();
        for d in &self.core_dict_dirs {
            dict.merge(Dict::from_toml_dir_excluding(d, &self.dict_excludes)?);
        }
        for d in &self.user_dict_dirs {
            dict.merge(Dict::from_toml_dir_excluding(d, &self.dict_excludes)?);
        }
        for f in &self.overrides_files {
            dict.merge(Dict::from_toml_file(f)?);
        }
        for (s, r) in self.extra_entries {
            dict.insert(s, r);
        }

        // Smart engine 用の数字系 provider (★C3): rules を pre-compile して保持。
        // analyze() 呼び出しごとに rebuild すると regex compile cost が乗るので一度作る。
        let number_provider = NumberCandidateProvider::new(&rules);

        // ★alpha.21: loanwords を再統合。 core_dict_dirs / user_dict_dirs 配下から
        // `role = "loanwords"` file を集めて lookup map に。 AlphabetPassthroughProvider
        // に渡して band 1000 dict hit を実現する。
        let mut loanwords_map: HashMap<String, String> = HashMap::new();
        for d in &self.core_dict_dirs {
            load_loanwords_into(&mut loanwords_map, d, &self.dict_excludes)?;
        }
        for d in &self.user_dict_dirs {
            load_loanwords_into(&mut loanwords_map, d, &self.dict_excludes)?;
        }
        // アクセント専用の表 (role = "accent") と 連結規則の表 (role = "accent_chain")。 core → user の順
        // (同じ表記 + 読みは先勝ち)。 有効なバイナリキャッシュがあればそれを読む ([`crate::tablecache`])。
        let (accent_lexicon, chain_table) = if self.no_accent_lexicon {
            (
                crate::scoring::accent_lexicon::AccentLexicon::default(),
                crate::chain_phrases::ChainTable::default(),
            )
        } else {
            let dirs: Vec<std::path::PathBuf> = self
                .core_dict_dirs
                .iter()
                .chain(self.user_dict_dirs.iter())
                .cloned()
                .collect();
            crate::tablecache::load_tables(&dirs, &self.dict_excludes)?
        };

        let compat_chars: crate::fxmap::FxSet<char> = rules
            .compat
            .map
            .keys()
            .filter_map(|k| {
                let mut cs = k.chars();
                match (cs.next(), cs.next()) {
                    (Some(c), None) => Some(c),
                    _ => None,
                }
            })
            .collect();
        Ok(Furigana {
            analyzer: OnceLock::new(),
            compat_chars,
            rules,
            dict,
            number_provider,
            loanwords: Arc::new(loanwords_map),
            accent_lexicon,
            chain_table,
            estimate_accent: self.estimate_accent,
            // 実験中の cost lattice engine (ADR-0011)。 環境変数で切り替えて A/B する。
            cost_engine: self.cost_engine
                || std::env::var("FURIGANA_COST_ENGINE").is_ok_and(|v| v == "1"),
        })
    }
}

/// `dir` 配下を再帰 scan し、 `role = "compat"` の TOML file (`compat.toml`) から
/// 異体字 → 標準字 map を `out` に補完する。
///
/// 既に `out` に存在する key (= rules_dir 由来) は上書きしない (rules_dir 優先)。
/// 複数 dir / file に同 variant があれば最初に読んだ方を残す。
fn load_compat_into(out: &mut CompatData, dir: &Path, excludes: &[String]) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    crate::loader::for_each_toml_in_dir_roles(
        dir,
        excludes,
        |role| role == Some("compat"),
        |content, from, role| {
            if role != Some("compat") {
                return Ok(());
            }
            let parsed: CompatData = crate::loader::parse_toml(content, from)?;
            for (variant, canonical) in parsed.map {
                out.map.entry(variant).or_insert(canonical);
            }
            Ok(())
        },
    )
}

/// `dir` 配下を再帰 scan し、 `role = "loanwords"` の TOML file から
/// `[entries]` table の `surface = reading` map を `out` に取り込む。
///
/// surface は [`normalize_alphabet`] で正規化 (= ASCII lowercase + 全角→半角)。
/// 同 surface に複数 reading が現れた場合、 後勝ち (= file 名 sort 順で merge 後勝ち)。
pub(super) fn load_loanwords_into(
    out: &mut HashMap<String, String>,
    dir: &Path,
    excludes: &[String],
) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    // walk + schema 検証 + role 解決は `for_each_toml_in_dir` が共通担当。
    crate::loader::for_each_toml_in_dir_roles(
        dir,
        excludes,
        |role| role == Some("loanwords"),
        |content, from, role| {
            if role != Some("loanwords") {
                return Ok(());
            }
            // [entries] table を parse (= role 別 toml 構造、 jukugo と同形式)
            #[derive(serde::Deserialize, Default)]
            struct LoanwordsToml {
                #[serde(default)]
                entries: HashMap<String, String>,
                /// 語 → くっついた数字の読み方。 `"Windows" = "english"` か、 読みを上書きする
                /// `"No" = { style = "place", reading = "ナンバー" }`。
                #[serde(default)]
                digits: HashMap<String, DigitsValue>,
                /// 英文の中でだけ使う語 (a / is / to / you …)。 単独では引かないので、 日本語の中の
                /// `to` や `so` を壊さない。 空白区切りの英字 / 空白なしの連結 (thankyou) の部分として使う。
                #[serde(default)]
                sentence_words: HashMap<String, String>,
            }
            #[derive(serde::Deserialize)]
            #[serde(untagged)]
            enum DigitsValue {
                Style(String),
                Detail {
                    style: String,
                    reading: Option<String>,
                },
            }
            let parsed: LoanwordsToml = crate::loader::parse_toml(content, from)?;
            for (surface, reading) in parsed.entries {
                out.insert(normalize_alphabet(&surface), reading);
            }
            for (word, reading) in parsed.sentence_words {
                out.insert(
                    format!(
                        "{}{}",
                        crate::scoring::special::SENTENCE_KEY_PREFIX,
                        normalize_alphabet(&word)
                    ),
                    reading,
                );
            }
            for (surface, value) in parsed.digits {
                let (style, reading) = match value {
                    DigitsValue::Style(s) => (s, None),
                    DigitsValue::Detail { style, reading } => (style, reading),
                };
                if crate::scoring::digit_reading::DigitStyle::parse(&style).is_none() {
                    return Err(crate::FuriganaError::Validation(format!(
                        "{from}: [digits] '{surface}' の読み方 '{style}' は未知 \
                     (place / each / each_en / english / english10 / pair / pair_en)"
                    )));
                }
                // 「読み方」 と (あれば) 「語 + 数字の時だけ使う読み」 を 1 文字列に畳む。
                let encoded = match reading {
                    Some(r) => format!("{style}{}{r}", crate::scoring::special::DIGITS_SEP),
                    None => style,
                };
                out.insert(
                    format!(
                        "{}{}",
                        crate::scoring::special::DIGITS_KEY_PREFIX,
                        normalize_alphabet(&surface)
                    ),
                    encoded,
                );
            }
            Ok(())
        },
    )
}

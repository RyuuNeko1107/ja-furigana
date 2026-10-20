//! データローダー (TOML → [`RulesData`])
//!
//! ## ファイル名規約
//!
//! ディレクトリ単位で読み込む場合、以下のファイル名で配置する:
//!
//! | ファイル名 | 型 |
//! |---|---|
//! | `counters.toml` | [`CountersData`] |
//! | `context.toml` | [`ContextData`] |
//! | `days.toml` | [`DaysData`] |
//! | `scales.toml` | [`ScalesData`] |
//! | `units.toml` | [`UnitsData`] |
//! | `symbols.toml` | [`SymbolsData`] |
//! | `numeric_phrases.toml` | [`NumericPhrasesData`] |
//! | `compat_map.toml` | [`CompatData`] |
//!
//! 個別ファイルが存在しない場合はその型のデフォルト値が返る。
//! ただし [`load_rules_dir`] は **ディレクトリ自体が存在しない** 場合はエラーを返す。
//!
//! ### 細分化サポート (counters / context)
//!
//! 大きくなりがちな counters.toml / context.toml は、それぞれ
//! `counters/*.toml` / `context/*.toml` のサブディレクトリに分割して
//! 配置することもできる。サブディレクトリ配下の `*.toml` はファイル名
//! ソート順で読み込まれ、[`CountersData::merge`] / [`ContextData::merge`]
//! で 1 つにまとめられる。
//!
//! 単一ファイル `counters.toml` とサブディレクトリ `counters/` が両方
//! ある場合は単一ファイルが優先される (混在防止のため、いずれか一方に
//! 統一することを推奨)。
//!
//! ## API
//!
//! 単一ファイル系は generic で 1 つに集約してある:
//! - [`parse_toml`]      : 文字列 → 任意の `Deserialize` 型
//! - [`load_or_default`] : ファイル → 任意の `Deserialize + Default` 型
//!   (ファイルが無ければ `Default::default()` を返す)
//!
//! ディレクトリ全体を読み込む高レベル API は [`load_rules_dir`]。

use crate::error::{FuriganaError, Result};
use crate::rules::{
    ContextCuesData, CountersData, ModelsData, PostProcessData, PostProcessSpec, RulesData,
};
use serde::de::DeserializeOwned;
use std::path::Path;

// ─── ファイル名定数 ───────────────────────────────────────────────────────────

/// 助数詞ルール
pub const COUNTERS_FILE: &str = "counters.toml";
/// 文脈ルール
pub const CONTEXT_FILE: &str = "context.toml";
/// 日付特殊読み
pub const DAYS_FILE: &str = "days.toml";
/// 大数スケール
pub const SCALES_FILE: &str = "scales.toml";
/// SI 単位
pub const UNITS_FILE: &str = "units.toml";
/// 記号読み
pub const SYMBOLS_FILE: &str = "symbols.toml";
/// 慣用語句
pub const NUMERIC_PHRASES_FILE: &str = "numeric_phrases.toml";
/// 異体字マップ。 配布物 (ja-furigana-dict release tar) 内のファイル名と一致させる。
/// 旧 alpha 系で「`compat_map.toml`」 にしていた時代があったが、 dict 側は当初
/// から `compat.toml` で配布しており、 lib が探していたファイル名と乖離して
/// **異体字正規化が無効化されていた** (R15 で corpus 「檜風呂に入る」 が追加される
/// まで気付かれなかった構造的 bug)。
///
/// **2026-06-14**: dict 配布での配置を `core/compat.toml` → `rules/compat.toml` に移動。
/// `load_rules_dir` は `rules_dir` しか走査せず role="compat" を読むため、 wrapper が
/// `rules_dir` / `core_dict_dir` を別 path で mount する production 構成では `core/` 配下の
/// compat が一切読まれず異体字正規化が dead だった (NFKC/IVS は `CompatData` 非依存で
/// 動作)。 compat は辞書 entry でなく入力正規化ルールなので `rules/` が本来の home。
/// 加えて [`crate::api::FuriganaBuilder::build`] が core/user dict 配下の role="compat" も
/// 補完読みするので、 どちらの配置でも拾える (defense-in-depth)。
pub const COMPAT_FILE: &str = "compat.toml";
/// 後処理ルール (Step 7 (mode 別後処理 regex))
pub const POSTPROCESS_FILE: &str = "postprocess.toml";

// ─── 汎用 parse / load ──────────────────────────────────────────────────────

/// TOML 文字列を任意の型にパース
///
/// 失敗時は `file` をエラーメッセージに含めた [`FuriganaError::Toml`] を返す。
///
/// ```no_run
/// use furigana::loader::parse_toml;
/// use furigana::rules::CountersData;
///
/// let data: CountersData = parse_toml(r#"[simple]"#, "counters.toml").unwrap();
/// ```
pub fn parse_toml<T: DeserializeOwned>(content: &str, file: &str) -> Result<T> {
    toml::from_str(content).map_err(|e| FuriganaError::Toml {
        file: file.to_string(),
        source: e,
    })
}

/// ファイルから TOML を読み込む (存在しなければ `Default::default()` を返す)
///
/// ```no_run
/// use furigana::loader::load_or_default;
/// use furigana::rules::CountersData;
///
/// let data: CountersData = load_or_default("path/to/counters.toml").unwrap();
/// ```
pub fn load_or_default<T: DeserializeOwned + Default>(path: impl AsRef<Path>) -> Result<T> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(T::default());
    }
    let content = std::fs::read_to_string(path)?;
    parse_toml(&content, &path.display().to_string())
}

// ─── ディレクトリ全体 ────────────────────────────────────────────────────────

/// 指定ディレクトリから全ルールファイルを読み込んで [`RulesData`] を構築する。
///
/// - **ディレクトリ自体が存在しない**: [`FuriganaError::Validation`] でエラー
/// - **個別ファイルが存在しない**: その型のデフォルト値で埋める
/// - **個別ファイルがパース失敗**: そのファイル名付きでエラー
pub fn load_rules_dir<P: AsRef<Path>>(dir: P) -> Result<RulesData> {
    let dir = dir.as_ref();
    if !dir.exists() {
        return Err(FuriganaError::Validation(format!(
            "rules directory not found: {}",
            dir.display()
        )));
    }
    if !dir.is_dir() {
        return Err(FuriganaError::Validation(format!(
            "rules path is not a directory: {}",
            dir.display()
        )));
    }

    // 全 *.toml を再帰 walk して、 各 file の role で振り分け (walk + schema 検証 +
    // role 解決は `for_each_toml_in_dir` が共通担当)。 role tag 無い file は file 名
    // から推定 (`infer_role_from_path` 経由)。 これで rule file は dir 構造に依存せず
    // 配置自由 (例 `rules/text/postprocess.toml` のような階層化が可能)。
    //
    // ★A1b: rules dir 配下の TOML は **全 file** に `[meta] schema_version = "2"` を
    // 必須化 (alpha.10〜)。 役割不明 / dict 系混在 file も含めて validation される
    // (= dir 配下の TOML は全部新 format に揃える方針、 旧 alpha era format を silent
    // skip させない)。 これは `for_each_toml_in_dir` 内の `validate_schema_version`。
    let mut data = RulesData::default();
    let mut merged_spec = PostProcessSpec::default();
    for_each_toml_in_dir(dir, |content, from, role| {
        match role {
            Some("counters") => {
                let part: CountersData = parse_toml(content, from)?;
                data.counters.merge(part);
            }
            Some("context") => {
                // ★alpha.15: context.toml は dict 側 [entries."X".match] / [[kanji]]
                // block で代替され、 lib 側では使われない。 古い release dict 互換のため
                // role を認識するが load せず silent skip。
            }
            Some("days") => data.days = parse_toml(content, from)?,
            Some("scales") => data.scales = parse_toml(content, from)?,
            Some("units") => data.units = parse_toml(content, from)?,
            Some("symbols") => data.symbols = parse_toml(content, from)?,
            // emoji_context は文脈読み (match) だけを別 file にしたもの。 旧 lib (0.5.19〜21) は未知の role として無視するので、
            // 簡単な読みを emoji.toml、 文脈読みを emoji_context.toml に分ければ旧 lib でも辞書が壊れない
            Some("emoji" | "emoji_context") => {
                let part: crate::rules::EmojiData = parse_toml(content, from)?;
                data.emoji.merge(part);
            }
            Some("models") => {
                let part: ModelsData = parse_toml(content, from)?;
                part.validate()
                    .map_err(|e| FuriganaError::Validation(format!("{from}: {e}")))?;
                data.models.merge(part);
            }
            Some("context_cues") => {
                let part: ContextCuesData = parse_toml(content, from)?;
                part.validate()
                    .map_err(|e| FuriganaError::Validation(format!("{from}: {e}")))?;
                data.context_cues.merge(part);
            }
            Some("person_name_cues") => {
                let part: crate::rules::PersonNameCuesData = parse_toml(content, from)?;
                part.validate()
                    .map_err(|e| FuriganaError::Validation(format!("{from}: {e}")))?;
                data.person_name_cues.merge(part);
            }
            Some("numeric_phrases") => data.numeric_phrases = parse_toml(content, from)?,
            Some("compat") => data.compat = parse_toml(content, from)?,
            Some("postprocess") => {
                // regex compile は最後にまとめて (複数 file を merge した後)
                let spec: PostProcessSpec = parse_toml(content, from)?;
                merged_spec.rules.extend(spec.rules);
            }
            _ => {
                // role 不明 / 認識外 / dict 系 (jukugo/unihan/works/loanwords/
                // single_overrides) は rules には不要、 silent skip。
            }
        }
        Ok(())
    })?;
    // 合算後にも検査する (複数 file に分けて語数の上限を超える書き方を弾く)
    data.person_name_cues
        .validate()
        .map_err(|e| FuriganaError::Validation(format!("person_name_cues: {e}")))?;
    // postprocess を merge して compile
    data.postprocess = PostProcessData::from_spec(merged_spec)
        .map_err(|e| FuriganaError::Validation(format!("postprocess regex compile failed: {e}")))?;
    Ok(data)
}

mod meta;
mod walk;

pub use meta::{
    parse_meta_role, parse_meta_schema_version, resolve_role, validate_schema_version,
    SUPPORTED_SCHEMA_VERSIONS,
};
pub(crate) use walk::{
    for_each_toml_in_dir, for_each_toml_in_dir_roles, is_excluded, walk_toml_files,
};

#[cfg(test)]
mod tests;

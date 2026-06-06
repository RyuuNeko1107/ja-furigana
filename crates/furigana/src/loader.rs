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
use crate::rules::{CountersData, PostProcessData, PostProcessSpec, RulesData};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::path::{Path, PathBuf};

// ─── [meta] tag の取得 ───────────────────────────────────────────────────────
//
// 各 dict / rule TOML の冒頭に `[meta]` block を declare できる:
//
// ```toml
// [meta]
// schema_version = "2"      # 0.1.0 から必須 (新 format)、 alpha era は不在 or "1" (旧)
// role = "entries"          # role 駆動 dispatch tag
// description = "..."
// ```
//
// lib loader は schema_version で format version を判定、 role で dispatch。
// schema_version "2" のみ accept、 それ以外 (旧 alpha era / 不在) は明確 error。

#[derive(Deserialize, Default)]
struct MetaWrapper {
    #[serde(default)]
    meta: Option<MetaTag>,
}

#[derive(Deserialize, Default)]
struct MetaTag {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    schema_version: Option<String>,
}

/// 本 lib が accept する dict schema version の list。
///
/// alpha.10 以降は `"2"` のみ。 旧 alpha era 用 dict (= schema_version 不在 or `"1"`)
/// は受け付けない、 [`validate_schema_version`] で明確 error reject。
pub const SUPPORTED_SCHEMA_VERSIONS: &[&str] = &["2"];

/// TOML 内の `[meta] role` を返す (無ければ None)。 失敗は None 扱い。
#[must_use]
pub fn parse_meta_role(content: &str) -> Option<String> {
    toml::from_str::<MetaWrapper>(content)
        .ok()
        .and_then(|w| w.meta)
        .and_then(|m| m.role)
}

/// TOML 内の `[meta] schema_version` を返す (無ければ None)。 失敗は None 扱い。
#[must_use]
pub fn parse_meta_schema_version(content: &str) -> Option<String> {
    toml::from_str::<MetaWrapper>(content)
        .ok()
        .and_then(|w| w.meta)
        .and_then(|m| m.schema_version)
}

/// TOML 内の `[meta] schema_version` を validate。 [`SUPPORTED_SCHEMA_VERSIONS`] のみ accept。
///
/// - `schema_version = "2"` → `Ok(())`
/// - 不在 (= 構文 valid だが `[meta] schema_version` field 無し) → `Err(Validation)`
///   (legacy format pre-0.1.0、 migration 要求)
/// - その他 (= `"1"` 含む) → `Err(Validation)` (= unsupported version)
/// - **TOML 構文 invalid** → `Ok(())` (= 後段の `parse_toml` が proper な
///   [`FuriganaError::Toml`] を返すことを期待、 validation 段階では silent pass)
///
/// alpha.10 以降 lib は新 format (= schema_version "2") のみ受け付ける。
/// 旧 format dict を読み込んだ場合は明確に reject、 caller に migration を促す。
///
/// # 使用例
///
/// ```ignore
/// let content = std::fs::read_to_string("path/to/dict.toml")?;
/// validate_schema_version(&content, "path/to/dict.toml")?;
/// ```
pub fn validate_schema_version(content: &str, file: &str) -> Result<()> {
    // TOML 構文 invalid なら silent pass (= 後段 parser が proper な Toml error を出す責務)。
    // この early-return が無いと 「壊れた TOML」 が "missing schema_version" Validation
    // error として扱われ、 caller の error UX が劣化する (parse error の方が診断的に有用)。
    let Ok(wrapper) = toml::from_str::<MetaWrapper>(content) else {
        return Ok(());
    };
    let version = wrapper.meta.and_then(|m| m.schema_version);
    match version.as_deref() {
        Some(v) if SUPPORTED_SCHEMA_VERSIONS.contains(&v) => Ok(()),
        Some(v) => Err(FuriganaError::Validation(format!(
            "{}: dict schema version {:?} not supported by ja-furigana 0.1.x \
             (expected: {:?}). Migrate dict using `furigana-dict/tools/migrate_v2.py` \
             or upgrade dict to v0.1.0+",
            file,
            v,
            SUPPORTED_SCHEMA_VERSIONS.join(", ")
        ))),
        None => Err(FuriganaError::Validation(format!(
            "{}: missing [meta] schema_version field (= legacy format pre-0.1.0). \
             Migrate dict using `furigana-dict/tools/migrate_v2.py` \
             or upgrade dict to v0.1.0+",
            file
        ))),
    }
}

/// file path から role を推定 (role tag 無い file の互換 fallback)。
///
/// 既知の hardcoded path / 名 から「これは何の role か」 を推定する:
/// - `<dir>/days.toml` → "days"
/// - `<dir>/scales.toml` → "scales"
/// - `<dir>/counters.toml` or `<dir>/counters/*.toml` → "counters"
/// - `<dir>/context.toml` or `<dir>/context/*.toml` → "context"
/// - 等
///
/// 不明 path は None。
fn infer_role_from_path(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    // subdir 親で counters / context を識別
    if let Some(parent) = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    {
        match parent {
            "counters" => return Some("counters"),
            "context" => return Some("context"),
            _ => {}
        }
    }
    match name {
        "counters.toml" => Some("counters"),
        "context.toml" => Some("context"),
        "days.toml" => Some("days"),
        "scales.toml" => Some("scales"),
        "units.toml" => Some("units"),
        "symbols.toml" => Some("symbols"),
        "numeric_phrases.toml" => Some("numeric_phrases"),
        "compat.toml" | "compat_map.toml" => Some("compat"),
        "postprocess.toml" => Some("postprocess"),
        _ => None,
    }
}

/// dict 系 file の role を推定 (role tag 無い file の互換 fallback)。
///
/// 既知の hardcoded path / 名 から「これは何の role か」 を推定する:
/// - `<dir>/single_overrides.toml` → "single_overrides"
/// - `<dir>/compat.toml` or `compat_map.toml` → "compat"
/// - path 中に `loanwords/` を含む → "loanwords"
/// - path 中に `jukugo/` を含む → "jukugo"
/// - path 中に `unihan/` を含む → "unihan"
/// - path 中に `works/` を含む → "works"
///
/// 不明 path は None。 caller 側で 「role 不明 = jukugo (back-compat)」 のように
/// 扱うかどうかを決める。
fn infer_dict_role_from_path(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    if name == "single_overrides.toml" {
        return Some("single_overrides");
    }
    if name == "compat.toml" || name == "compat_map.toml" {
        return Some("compat");
    }
    // path 中の dir 名で識別
    for component in path.components() {
        if let Some(s) = component.as_os_str().to_str() {
            match s {
                "loanwords" => return Some("loanwords"),
                "jukugo" => return Some("jukugo"),
                "unihan" => return Some("unihan"),
                "works" => return Some("works"),
                _ => {}
            }
        }
    }
    None
}

/// dict / rule 系 file の role を `[meta] role` から取得、 失敗時は path 推定。
///
/// `parse_meta_role` で `[meta] role` を読み、 無ければ
/// [`infer_role_from_path`] (rules) → [`infer_dict_role_from_path`] (dict) の
/// 順で path-based 推定を試す。 どちらにも該当しなければ None。
///
/// dict 側 loader (`Dict::from_toml_dir` / `Loanwords::from_toml_dir`) からは
/// この関数を経由して role を解決し、 role 値で dispatch する。
#[must_use]
pub fn resolve_role(content: &str, path: &Path) -> Option<String> {
    if let Some(role) = parse_meta_role(content) {
        return Some(role);
    }
    if let Some(role) = infer_role_from_path(path) {
        return Some(role.to_string());
    }
    if let Some(role) = infer_dict_role_from_path(path) {
        return Some(role.to_string());
    }
    None
}

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
/// から `core/compat.toml` で配布しており、 lib が探していたファイル名と乖離して
/// **異体字正規化が無効化されていた** (reading::tokenize_text Step 1 が no-op
/// になり、 「髙橋」 / 「檜風呂」 等が compat 経由で標準字に変換されないまま
/// chunker / Lindera に流れていた)。 R15 で corpus に「檜風呂に入る」 が追加され
/// るまで気付かれなかった構造的 bug の修正。
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
    // postprocess を merge して compile
    data.postprocess = PostProcessData::from_spec(merged_spec)
        .map_err(|e| FuriganaError::Validation(format!("postprocess regex compile failed: {e}")))?;
    Ok(data)
}

/// `dir` 配下の *.toml を再帰 walk して file path を昇順で列挙する。
///
/// `_genre.toml` (STATS sub-section meta) と `*.test.toml` (CI 専用) は除外。
/// rules / dict / loanwords いずれの dir scan もこの 1 関数を共有する
/// (= かつて loader / dict / api に 3 重実装されていた walk を統合)。
pub(crate) fn walk_toml_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk_toml_files_inner(dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_toml_files_inner(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "toml") {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "_genre.toml" || name.ends_with(".test.toml") {
                continue;
            }
            out.push(path);
        } else if path.is_dir() {
            walk_toml_files_inner(&path, out)?;
        }
    }
    Ok(())
}

/// `dir` 配下の全 TOML file を walk し、 各 file を schema_version 検証 + role 解決
/// した上で `visit(content, from, role)` に渡す共有 loader。
///
/// rules ([`load_rules_dir`]) / dict (`Dict::from_toml_dir`) / loanwords
/// (`load_loanwords_into`) の 3 経路が共有する 「walk → read → schema 検証 →
/// `resolve_role` → role 別 dispatch」 の前段を 1 箇所に集約する。 role 別の
/// 取り込み方 (= どの role を何のデータ構造へ load するか) だけが caller 固有で、
/// `visit` closure として渡す。
///
/// `from` は `path.display()` 文字列 (= parse error メッセージ用)。 `dir` 不在時の
/// 挙動 (エラー / default) は caller が事前に判断する前提で、 本関数は walk のみ行う。
pub(crate) fn for_each_toml_in_dir<F>(dir: &Path, mut visit: F) -> Result<()>
where
    F: FnMut(&str, &str, Option<&str>) -> Result<()>,
{
    for path in walk_toml_files(dir)? {
        let content = std::fs::read_to_string(&path)?;
        let from = path.display().to_string();
        validate_schema_version(&content, &from)?;
        let role = resolve_role(&content, &path);
        visit(&content, &from, role.as_deref())?;
    }
    Ok(())
}

// (旧 load_counters / load_context / load_postprocess / list_toml_files_sorted は
// load_rules_dir の再帰 walk + role 駆動 dispatch に統合された。
// hardcoded path / file 名定数 は infer_role_from_path の fallback で使われる。)

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::CountersData;

    #[test]
    fn parses_counters_toml() {
        let toml_str = r#"
            [counter."本"]
            default = "ホン"

            [[counter."本".rules]]
            last_digit = [1, 6, 8, 0]
            suffix = "ポン"
            sokuonize = true
        "#;
        let data: CountersData = parse_toml(toml_str, "counters.toml").unwrap();
        let hon = data.counter.get("本").unwrap();
        assert_eq!(hon.default.as_deref(), Some("ホン"));
        assert_eq!(hon.rules[0].suffix, "ポン");
    }

    #[test]
    fn invalid_toml_error_includes_file_name() {
        let err = parse_toml::<CountersData>("[invalid", "counters.toml").unwrap_err();
        match err {
            FuriganaError::Toml { file, .. } => assert_eq!(file, "counters.toml"),
            other => panic!("expected Toml error, got {other:?}"),
        }
    }

    #[test]
    fn parses_days_toml_lookup_by_int() {
        let toml_str = r#"
            [entries]
            "1" = "ツイタチ"
            "20" = "ハツカ"
        "#;
        let data: crate::rules::DaysData = parse_toml(toml_str, "days.toml").unwrap();
        assert_eq!(data.get(1), Some("ツイタチ"));
        assert_eq!(data.get(20), Some("ハツカ"));
        assert_eq!(data.get(15), None);
    }

    #[test]
    fn parses_scales_toml_with_array_of_tables() {
        let toml_str = r#"
            [[entry]]
            kanji = "万"
            kana = "マン"
            [[entry]]
            kanji = "億"
            kana = "オク"
        "#;
        let data: crate::rules::ScalesData = parse_toml(toml_str, "scales.toml").unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data.lookup("億"), Some("オク"));
    }

    #[test]
    fn parses_units_toml_with_inline_tables() {
        let toml_str = r#"
            [entries]
            "km" = { kana = "キロメートル" }
            "L"  = { kana = "リットル", ci = true }
        "#;
        let data: crate::rules::UnitsData = parse_toml(toml_str, "units.toml").unwrap();
        assert_eq!(data.lookup("km"), Some("キロメートル"));
        assert_eq!(data.lookup("l"), Some("リットル"));
    }

    fn fresh_temp_dir(suffix: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "furigana_loader_{}_{}_{}",
            std::process::id(),
            suffix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn load_rules_dir_errors_when_dir_missing() {
        let nonexistent = std::env::temp_dir().join("furigana_does_not_exist_xyz123");
        std::fs::remove_dir_all(&nonexistent).ok();
        let err = load_rules_dir(&nonexistent).unwrap_err();
        assert!(matches!(err, FuriganaError::Validation(_)));
    }

    #[test]
    fn load_rules_dir_with_no_files_yields_default() {
        let dir = fresh_temp_dir("empty");
        let data = load_rules_dir(&dir).unwrap();
        assert!(data.counters.simple.is_empty());
        assert!(data.scales.is_empty());
        assert!(data.compat.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_rules_dir_loads_files_when_present() {
        let dir = fresh_temp_dir("present");
        std::fs::write(
            dir.join(SCALES_FILE),
            "[meta]\nschema_version = \"2\"\n\n[[entry]]\nkanji = \"万\"\nkana = \"マン\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(COUNTERS_FILE),
            "[meta]\nschema_version = \"2\"\n\n[counter.\"本\"]\ndefault = \"ホン\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(COMPAT_FILE),
            "[meta]\nschema_version = \"2\"\n\n[map]\n\"髙\" = \"高\"\n",
        )
        .unwrap();

        let data = load_rules_dir(&dir).unwrap();
        assert_eq!(data.scales.lookup("万"), Some("マン"));
        assert_eq!(
            data.counters
                .counter
                .get("本")
                .and_then(|c| c.default.as_deref()),
            Some("ホン")
        );
        assert_eq!(data.compat.lookup("髙"), Some("高"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_rules_dir_propagates_parse_errors() {
        let dir = fresh_temp_dir("parse_err");
        // 壊れた TOML は validate_schema_version が silent pass し、 後段の
        // parse_toml が proper な Toml error を返す (= ★A1b の UX 設計)。
        std::fs::write(dir.join(COUNTERS_FILE), "壊れた").unwrap();
        let err = load_rules_dir(&dir).unwrap_err();
        assert!(matches!(err, FuriganaError::Toml { .. }));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_rules_dir_merges_counters_subdir() {
        let dir = fresh_temp_dir("counters_subdir");
        let sub = dir.join("counters");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("01_simple.toml"),
            "[meta]\nschema_version = \"2\"\n\n[simple]\n\"円\" = \"エン\"\n",
        )
        .unwrap();
        std::fs::write(
            sub.join("02_objects.toml"),
            "[meta]\nschema_version = \"2\"\n\n[counter.\"本\"]\ndefault = \"ホン\"\n",
        )
        .unwrap();
        std::fs::write(
            sub.join("03_time.toml"),
            "[meta]\nschema_version = \"2\"\n\n[counter.\"時\"]\ndefault = \"ジ\"\n",
        )
        .unwrap();

        let data = load_rules_dir(&dir).unwrap();
        assert_eq!(
            data.counters.simple.get("円").map(String::as_str),
            Some("エン")
        );
        assert_eq!(
            data.counters
                .counter
                .get("本")
                .and_then(|c| c.default.as_deref()),
            Some("ホン")
        );
        assert_eq!(
            data.counters
                .counter
                .get("時")
                .and_then(|c| c.default.as_deref()),
            Some("ジ")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_rules_dir_merges_single_file_and_subdir() {
        // 新 loader (role 駆動 + 再帰 walk) は単一 file と subdir 配下を **両方 merge**
        // する。 旧 loader では「単一 file 優先で subdir は ignore」 だったが、
        // role tag 駆動なら同 role の file が複数 dir に存在しても自然に merge できる。
        let dir = fresh_temp_dir("counters_merge");
        let sub = dir.join("counters");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            dir.join(COUNTERS_FILE),
            "[meta]\nschema_version = \"2\"\n\n[counter.\"本\"]\ndefault = \"ホン\"\n",
        )
        .unwrap();
        std::fs::write(
            sub.join("more.toml"),
            "[meta]\nschema_version = \"2\"\n\n[counter.\"匹\"]\ndefault = \"ヒキ\"\n",
        )
        .unwrap();

        let data = load_rules_dir(&dir).unwrap();
        assert!(data.counters.counter.contains_key("本"));
        assert!(data.counters.counter.contains_key("匹"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_rules_dir_role_tag_overrides_filename() {
        // file 名から推定できない場所に置いても [meta] role があれば認識される。
        let dir = fresh_temp_dir("role_tag_override");
        std::fs::create_dir_all(dir.join("custom")).unwrap();
        std::fs::write(
            dir.join("custom").join("anywhere.toml"),
            "[meta]\nschema_version = \"2\"\nrole = \"counters\"\n\n[counter.\"枚\"]\ndefault = \"マイ\"\n",
        )
        .unwrap();

        let data = load_rules_dir(&dir).unwrap();
        assert!(data.counters.counter.contains_key("枚"));
        std::fs::remove_dir_all(&dir).ok();
    }

    // ─── schema_version validation tests ─────────────────────────────────────

    #[test]
    fn parse_meta_schema_version_returns_value_when_present() {
        let content = "[meta]\nschema_version = \"2\"\nrole = \"entries\"\n";
        assert_eq!(parse_meta_schema_version(content).as_deref(), Some("2"));
    }

    #[test]
    fn parse_meta_schema_version_returns_none_when_absent() {
        let content = "[meta]\nrole = \"entries\"\n";
        assert!(parse_meta_schema_version(content).is_none());
    }

    #[test]
    fn parse_meta_schema_version_returns_none_when_no_meta_block() {
        let content = "[entries]\n\"猫\" = \"ネコ\"\n";
        assert!(parse_meta_schema_version(content).is_none());
    }

    #[test]
    fn validate_schema_version_accepts_v2() {
        let content = "[meta]\nschema_version = \"2\"\n";
        assert!(validate_schema_version(content, "test.toml").is_ok());
    }

    #[test]
    fn validate_schema_version_rejects_v1_explicitly() {
        let content = "[meta]\nschema_version = \"1\"\n";
        let err = validate_schema_version(content, "test.toml").unwrap_err();
        match err {
            FuriganaError::Validation(msg) => {
                assert!(msg.contains("not supported"), "msg: {msg}");
                assert!(msg.contains("\"1\""), "msg: {msg}");
                assert!(msg.contains("test.toml"), "msg: {msg}");
                assert!(msg.contains("migrate_v2.py"), "msg: {msg}");
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn validate_schema_version_rejects_missing_field_as_legacy() {
        let content = "[meta]\nrole = \"entries\"\n";
        let err = validate_schema_version(content, "old_dict.toml").unwrap_err();
        match err {
            FuriganaError::Validation(msg) => {
                assert!(msg.contains("legacy format pre-0.1.0"), "msg: {msg}");
                assert!(msg.contains("missing"), "msg: {msg}");
                assert!(msg.contains("old_dict.toml"), "msg: {msg}");
                assert!(msg.contains("migrate_v2.py"), "msg: {msg}");
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn validate_schema_version_rejects_unknown_version() {
        let content = "[meta]\nschema_version = \"99\"\n";
        let err = validate_schema_version(content, "test.toml").unwrap_err();
        match err {
            FuriganaError::Validation(msg) => {
                assert!(msg.contains("not supported"), "msg: {msg}");
                assert!(msg.contains("\"99\""), "msg: {msg}");
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn validate_schema_version_rejects_no_meta_block() {
        let content = "[entries]\n\"猫\" = \"ネコ\"\n";
        let err = validate_schema_version(content, "no_meta.toml").unwrap_err();
        match err {
            FuriganaError::Validation(msg) => {
                assert!(msg.contains("legacy format"), "msg: {msg}");
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    // ─── A1b: caller-side schema_version 強制 tests ─────────────────────────

    #[test]
    fn load_rules_dir_rejects_legacy_file_without_meta() {
        // alpha.10〜 lib は schema_version = "2" を必須化 (★A1b)。
        // [meta] 不在 = 旧 alpha era format → Validation error で reject。
        let dir = fresh_temp_dir("a1b_legacy");
        std::fs::write(
            dir.join(SCALES_FILE),
            "[[entry]]\nkanji = \"万\"\nkana = \"マン\"\n",
        )
        .unwrap();
        let err = load_rules_dir(&dir).unwrap_err();
        match err {
            FuriganaError::Validation(msg) => {
                assert!(
                    msg.contains("schema_version"),
                    "expected schema_version error: {msg}"
                );
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_rules_dir_rejects_v1_format() {
        let dir = fresh_temp_dir("a1b_v1");
        std::fs::write(
            dir.join(SCALES_FILE),
            "[meta]\nschema_version = \"1\"\n\n[[entry]]\nkanji = \"万\"\nkana = \"マン\"\n",
        )
        .unwrap();
        let err = load_rules_dir(&dir).unwrap_err();
        assert!(matches!(err, FuriganaError::Validation(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn supported_schema_versions_contains_v2() {
        assert!(SUPPORTED_SCHEMA_VERSIONS.contains(&"2"));
    }
}

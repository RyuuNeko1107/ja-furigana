//! `[meta]` block (role / schema_version) の取得と、 パスからの role 推定

use crate::error::{FuriganaError, Result};
use serde::Deserialize;
use std::path::Path;

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
pub(super) struct MetaWrapper {
    #[serde(default)]
    pub(super) meta: Option<MetaTag>,
}

#[derive(Deserialize, Default)]
pub(super) struct MetaTag {
    #[serde(default)]
    pub(super) role: Option<String>,
    #[serde(default)]
    pub(super) schema_version: Option<String>,
}

/// 本 lib が accept する dict schema version の list。
///
/// alpha.10 以降は `"2"` のみ。 旧 alpha era 用 dict (= schema_version 不在 or `"1"`)
/// は受け付けない、 [`validate_schema_version`] で明確 error reject。
pub const SUPPORTED_SCHEMA_VERSIONS: &[&str] = &["2"];

/// `[meta]` table だけを切り出す (`[meta]` 行から次の table 見出しの手前まで)。 見つからなければ None。
///
/// role / schema_version の判定のたびに file 全体を TOML 解析すると、 10 万件規模の表 (アクセント表 等) で
/// 起動が遅くなる (loader が 1 file を役割判定のために複数回読む)。 `[meta]` は file 先頭に置く規約なので
/// その部分だけ解析する。 切り出せない / 解析できない file は従来通り全体を解析する (2026-09-27)
pub(super) fn meta_section(content: &str) -> Option<&str> {
    let mut begin = None;
    let mut off = 0usize;
    for l in content.split_inclusive('\n') {
        let t = l.trim();
        match begin {
            None if t == "[meta]" => begin = Some(off),
            Some(b) if t.starts_with('[') => return Some(&content[b..off]),
            _ => {}
        }
        off += l.len();
    }
    begin.map(|b| &content[b..])
}

/// `[meta]` を解析する (切り出した部分 → 失敗なら file 全体)。
fn parse_meta(content: &str) -> Option<MetaWrapper> {
    if let Some(sec) = meta_section(content) {
        if let Ok(w) = toml::from_str::<MetaWrapper>(sec) {
            if w.meta.is_some() {
                return Some(w);
            }
        }
    }
    toml::from_str::<MetaWrapper>(content).ok()
}

/// TOML 内の `[meta] role` を返す (無ければ None)。 失敗は None 扱い。
#[must_use]
pub fn parse_meta_role(content: &str) -> Option<String> {
    parse_meta(content)
        .and_then(|w| w.meta)
        .and_then(|m| m.role)
}

/// TOML 内の `[meta] schema_version` を返す (無ければ None)。 失敗は None 扱い。
#[must_use]
pub fn parse_meta_schema_version(content: &str) -> Option<String> {
    parse_meta(content)
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
    let Some(wrapper) = parse_meta(content) else {
        return Ok(());
    };
    check_schema_version(wrapper.meta.and_then(|m| m.schema_version).as_deref(), file)
}

/// `[meta] schema_version` の値 (無ければ `None`) が対応する version か調べる。
pub(super) fn check_schema_version(version: Option<&str>, file: &str) -> Result<()> {
    match version {
        Some(v) if SUPPORTED_SCHEMA_VERSIONS.contains(&v) => Ok(()),
        Some(v) => Err(FuriganaError::Validation(format!(
            "{}: dict schema version {:?} not supported by ja-furigana 0.1.x              (expected: {:?}). Migrate dict using `furigana-dict/tools/migrate_v2.py`              or upgrade dict to v0.1.0+",
            file,
            v,
            SUPPORTED_SCHEMA_VERSIONS.join(", ")
        ))),
        None => Err(FuriganaError::Validation(format!(
            "{}: missing [meta] schema_version field (= legacy format pre-0.1.0).              Migrate dict using `furigana-dict/tools/migrate_v2.py`              or upgrade dict to v0.1.0+",
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
pub(super) fn infer_role_from_path(path: &Path) -> Option<&'static str> {
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
        "models.toml" => Some("models"),
        "context_cues.toml" => Some("context_cues"),
        "symbols.toml" => Some("symbols"),
        "emoji.toml" => Some("emoji"),
        "emoji_context.toml" => Some("emoji_context"),
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
pub(super) fn infer_dict_role_from_path(path: &Path) -> Option<&'static str> {
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
/// `infer_role_from_path` (rules) → `infer_dict_role_from_path` (dict) の
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

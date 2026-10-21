//! ディレクトリ配下の *.toml の列挙と、 role で絞った読み込み (rules / dict / loanwords で共有)

use super::meta::check_schema_version;
use super::meta::infer_dict_role_from_path;
use super::meta::infer_role_from_path;
use super::meta::meta_section;
use super::meta::resolve_role;
use super::meta::validate_schema_version;
use super::meta::MetaTag;
use super::meta::MetaWrapper;
use crate::error::Result;
use std::path::{Path, PathBuf};

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
pub(crate) fn for_each_toml_in_dir<F>(dir: &Path, visit: F) -> Result<()>
where
    F: FnMut(&str, &str, Option<&str>) -> Result<()>,
{
    for_each_toml_in_dir_excluding(dir, &[], visit)
}

/// `dir` からの相対 path が `excludes` のどれかに当たるか。
///
/// pattern は `/` 区切りの相対 path で、 dir (`works` / `works/vtuber`) なら配下全部、
/// file (`jukugo/society/mahjong.toml`、 `.toml` は省略可) ならその file だけ。
pub(crate) fn is_excluded(dir: &Path, path: &Path, excludes: &[String]) -> bool {
    if excludes.is_empty() {
        return false;
    }
    let Ok(rel) = path.strip_prefix(dir) else {
        return false;
    };
    let rel: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let rel = rel.join("/");
    let stem = rel.strip_suffix(".toml").unwrap_or(&rel);
    excludes.iter().any(|p| {
        let p = p.trim_matches('/').replace('\\', "/");
        let p = p.strip_suffix(".toml").unwrap_or(&p);
        !p.is_empty() && (stem == p || rel.strip_prefix(p).is_some_and(|r| r.starts_with('/')))
    })
}

/// [`for_each_toml_in_dir`] の除外付き版 ([`is_excluded`] に当たる file は読まない)
pub(crate) fn for_each_toml_in_dir_excluding<F>(
    dir: &Path,
    excludes: &[String],
    mut visit: F,
) -> Result<()>
where
    F: FnMut(&str, &str, Option<&str>) -> Result<()>,
{
    for path in walk_toml_files(dir)? {
        if is_excluded(dir, &path, excludes) {
            continue;
        }
        let content = std::fs::read_to_string(&path)?;
        let from = path.display().to_string();
        validate_schema_version(&content, &from)?;
        let role = resolve_role(&content, &path);
        visit(&content, &from, role.as_deref())?;
    }
    Ok(())
}

/// 先頭から読む最大 byte 数。 `[meta]` は file 先頭に置く規約なので、 役割の判定と schema 検証はここで足りる
pub(super) const HEAD_BYTES: usize = 16 * 1024;

/// file の先頭だけ読む。 file 全体が収まったなら `(全体, true)`。 末尾が UTF-8 の途中で切れたら手前までにする
pub(super) fn read_head(path: &Path) -> Result<(String, bool)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; HEAD_BYTES];
    let mut n = 0;
    while n < buf.len() {
        let r = f.read(&mut buf[n..])?;
        if r == 0 {
            break;
        }
        n += r;
    }
    buf.truncate(n);
    let complete = n < HEAD_BYTES;
    match String::from_utf8(buf) {
        Ok(s) => Ok((s, complete)),
        Err(e) if !complete => {
            let valid = e.utf8_error().valid_up_to();
            let mut bytes = e.into_bytes();
            bytes.truncate(valid);
            Ok((String::from_utf8(bytes).unwrap_or_default(), false))
        }
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "stream did not contain valid UTF-8",
        )
        .into()),
    }
}

/// 先頭 (`head`) だけで `[meta]` が決まるなら解析結果を返す。 `[meta]` が先頭に収まらない /
/// 解析できない時は `None` (呼び出し側が file 全体を読んで従来どおり判定する)
pub(super) fn head_meta(head: &str, complete: bool) -> Option<MetaTag> {
    let sec = meta_section(head)?;
    let unterminated = sec.as_ptr() as usize + sec.len() == head.as_ptr() as usize + head.len();
    if unterminated && !complete {
        return None;
    }
    toml::from_str::<MetaWrapper>(sec).ok()?.meta
}

/// [`for_each_toml_in_dir_excluding`] の、 役割で絞る版。
///
/// 各 file は先頭だけ読んで schema 検証と role 解決をし (検証は絞らず全 file に掛かる)、
/// `wants(role)` が真の file だけ全体を読んで `visit` に渡す。 アクセントの表のような巨大な file
/// (辞書全体の 3 分の 2) を、 要らない walk で何度も読まずに済む。 `[meta]` が先頭に収まらない file は
/// 従来どおり全体を読んで判定する。
pub(crate) fn for_each_toml_in_dir_roles<W, F>(
    dir: &Path,
    excludes: &[String],
    wants: W,
    mut visit: F,
) -> Result<()>
where
    W: Fn(Option<&str>) -> bool,
    F: FnMut(&str, &str, Option<&str>) -> Result<()>,
{
    for path in walk_toml_files(dir)? {
        if is_excluded(dir, &path, excludes) {
            continue;
        }
        let from = path.display().to_string();
        let (head, complete) = read_head(&path)?;
        if let Some(tag) = head_meta(&head, complete) {
            check_schema_version(tag.schema_version.as_deref(), &from)?;
            let role = tag
                .role
                .or_else(|| infer_role_from_path(&path).map(str::to_string))
                .or_else(|| infer_dict_role_from_path(&path).map(str::to_string));
            if !wants(role.as_deref()) {
                continue;
            }
            let content = if complete {
                head
            } else {
                std::fs::read_to_string(&path)?
            };
            visit(&content, &from, role.as_deref())?;
        } else {
            let content = if complete {
                head
            } else {
                std::fs::read_to_string(&path)?
            };
            validate_schema_version(&content, &from)?;
            let role = resolve_role(&content, &path);
            if wants(role.as_deref()) {
                visit(&content, &from, role.as_deref())?;
            }
        }
    }
    Ok(())
}

// (旧 load_counters / load_context / load_postprocess / list_toml_files_sorted は
// load_rules_dir の再帰 walk + role 駆動 dispatch に統合された。
// hardcoded path / file 名定数 は infer_role_from_path の fallback で使われる。)

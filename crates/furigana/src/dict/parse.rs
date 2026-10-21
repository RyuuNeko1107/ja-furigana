//! 辞書 TOML の解析 (`[entries]` の取り出し・入れ子の hoist・1 行ずつの高速経路)

use crate::scoring::format::KanjiBlock;
use serde::Deserialize;
use std::collections::HashMap;

/// TOML ファイルの `[entries]` セクションを受ける defensive な型。
///
/// value を `toml::Value` で受けて、後段で「string 値だけを拾う」フィルタを
/// かける。これにより rules 系ファイル (例: `units.toml` の `[entries]` は
/// `{ kana = "..." }` の inline table) と同じディレクトリに置かれていても
/// silent skip できる。core 辞書と rules を `data/` 1 階層に flat 配置する
/// ユースケース (paths::Paths::dict_core_dir == rules_dir) のための防御。
#[derive(Debug, Default, Deserialize)]
pub(super) struct DictFile {
    #[serde(default)]
    pub(super) entries: HashMap<String, toml::Value>,
    /// ★A2 alpha.12: `[[kanji]]` block 配列 (= core/kanji/*.toml の new format)
    /// 単漢字単独 default reading + 文脈分岐を持つ 「first-class candidate generator」
    #[serde(default)]
    pub(super) kanji: Vec<KanjiBlock>,
}

/// entry value tree (table / array) を再帰走査し、 **非 ASCII の string key**
/// を `(surface, reading)` として収集する (= TOML footgun で detailed entry の
/// match/alt block に吸収された bare simple entry の救済用)。
///
/// matcher 条件 field (`prev_eq` / `next_starts_any` / `reading` 等) は全て ASCII
/// identifier なので `key.is_ascii()` で除外され、 残る非 ASCII key = 吸収された
/// 日本語 surface のみが拾われる。 reading 等の **値** (非 ASCII) は key ではないので
/// 対象外。
pub(super) fn collect_hoisted_entries(v: &toml::Value, out: &mut Vec<(String, String)>) {
    match v {
        toml::Value::Table(tbl) => {
            for (k, vv) in tbl {
                if !k.is_ascii() {
                    if let Some(s) = vv.as_str() {
                        out.push((k.clone(), s.to_string()));
                    }
                }
                collect_hoisted_entries(vv, out);
            }
        }
        toml::Value::Array(arr) => {
            for vv in arr {
                collect_hoisted_entries(vv, out);
            }
        }
        _ => {}
    }
}

/// `[meta]` と `[entries]` に `"表層" = "読み"` (行末コメント可) しか無い TOML なら、 その組を返す。
/// 1 行でも外れる書き方があれば `None` (= 呼び出し側は toml crate で読む)。 表層・読みは
/// エスケープ (`\`) を含まない basic string のみ。 同じキーが 2 回出たら TOML としてエラーなので `None`
/// (toml 経路に任せてエラーを出させる)。
pub(super) fn parse_simple_entries(content: &str) -> Option<Vec<(&str, &str)>> {
    /// `"..."` を先頭から読み、 (中身, 残り) を返す。 エスケープや改行を含むものは None
    fn quoted(s: &str) -> Option<(&str, &str)> {
        let rest = s.strip_prefix('"')?;
        let end = rest.find('"')?;
        let inner = &rest[..end];
        if inner.contains('\\') {
            return None;
        }
        Some((inner, &rest[end + 1..]))
    }
    /// 値の後ろ: 空白 + (無し | `# コメント`) のみ許す
    fn tail_ok(s: &str) -> bool {
        let t = s.trim_start_matches([' ', '\t']);
        t.is_empty() || t.starts_with('#')
    }
    let mut section = "";
    let mut pairs = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for raw in content.lines() {
        let line = raw.trim_matches([' ', '\t']);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let (head, rest) = line.split_at(line.find(']')? + 1);
            if !tail_ok(rest) {
                return None;
            }
            match head {
                "[meta]" if section != "entries" => section = "meta",
                "[entries]" => section = "entries",
                _ => return None,
            }
            continue;
        }
        match section {
            "meta" => {
                // `key = "value"` だけ許す (中身は Dict では使わない)
                let (key, rest) = line.split_once('=')?;
                if !key
                    .trim()
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    return None;
                }
                let (_, after) = quoted(rest.trim_start())?;
                if !tail_ok(after) {
                    return None;
                }
            }
            "entries" => {
                let (k, rest) = quoted(line)?;
                let rest = rest.trim_start_matches([' ', '\t']).strip_prefix('=')?;
                let (v, after) = quoted(rest.trim_start_matches([' ', '\t']))?;
                if !tail_ok(after) || !seen.insert(k) {
                    return None;
                }
                pairs.push((k, v));
            }
            _ => return None, // root 直下の key 等
        }
    }
    Some(pairs)
}

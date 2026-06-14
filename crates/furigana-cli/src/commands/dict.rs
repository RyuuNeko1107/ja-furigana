//! `furigana dict ...` サブコマンド一式
//!
//! - `add <surface> <reading>` : user dict (`cli-added.toml`) に 1 件追加
//! - `list [--limit N]`         : 辞書状態のサマリ
//! - `remove <surface>`         : `cli-added.toml` から削除
//! - `import <path>`            : TOML ファイルを user dict にコピー
//! - `pull [--version v...]`    : core 辞書を GitHub Release から取得 (未実装)

use crate::config::Config;
use crate::paths::Paths;
use anyhow::{anyhow, bail, Context, Result};
use clap::{Args as ClapArgs, Subcommand};
use furigana::Dict;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// CLI 経由で追加されたエントリの保存先 (user dict 配下)
const CLI_DICT_FILENAME: &str = "cli-added.toml";

#[derive(ClapArgs, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub action: Action,
}

#[derive(Subcommand, Debug)]
pub enum Action {
    /// user 辞書に 1 件追加 (重複は上書き)
    Add {
        /// 表層形 (漢字を含む語)
        surface: String,
        /// カタカナ読み
        reading: String,
    },

    /// 現在の辞書状態をサマリ表示
    List {
        /// 表示するエントリ数の上限
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },

    /// user 辞書から削除 (`cli-added.toml` のみ対象)
    Remove {
        /// 削除する表層形
        surface: String,
    },

    /// TOML ファイルを user 辞書にコピー
    Import {
        /// インポート元 TOML
        path: PathBuf,
    },

    /// GitHub Release から core 辞書 + rules を取得して `<data_dir>` に展開
    Pull {
        /// ピン留めバージョン (例: `v0.1.0`)。未指定で最新。
        #[arg(long)]
        version: Option<String>,
    },
}

pub fn run(args: Args, paths: &Paths, _cfg: &Config) -> Result<()> {
    match args.action {
        Action::Add { surface, reading } => add(paths, &surface, &reading),
        Action::List { limit } => list(paths, limit),
        Action::Remove { surface } => remove(paths, &surface),
        Action::Import { path } => import(paths, &path),
        Action::Pull { version } => super::dict_pull::run(paths, version.as_deref()),
    }
}

// ─── add ─────────────────────────────────────────────────────────────────────

fn add(paths: &Paths, surface: &str, reading: &str) -> Result<()> {
    if surface.is_empty() || reading.is_empty() {
        bail!("surface と reading は必須です");
    }
    // TOML basic string に格納できない制御文字 (NULL や U+0001..U+0008、 U+000B
    // 等) を reject。 toml_escape は `"` `\` `\n` `\r` `\t` のみ escape するので、
    // 他の制御文字が混入すると parse 時に file 全体壊れる (self-DoS)。
    validate_user_input("surface", surface)?;
    validate_user_input("reading", reading)?;
    let user_dir = paths.dict_user_dir();
    fs::create_dir_all(&user_dir)
        .with_context(|| format!("user dict ディレクトリ作成失敗: {}", user_dir.display()))?;
    let cli_file = user_dir.join(CLI_DICT_FILENAME);

    let mut entries = read_cli_dict(&cli_file)?;
    let prev = entries.insert(surface.to_string(), reading.to_string());

    write_cli_dict(&cli_file, &entries)?;

    if let Some(p) = prev {
        println!("更新: {surface} ({p} → {reading})");
    } else {
        println!("追加: {surface} → {reading}");
    }
    println!("保存先: {}", cli_file.display());
    Ok(())
}

// ─── list ────────────────────────────────────────────────────────────────────

fn list(paths: &Paths, limit: usize) -> Result<()> {
    let f = super::build_furigana(paths)?;
    let total = f.dict_size();
    println!("辞書エントリ数: {total}");

    let cli_file = paths.dict_user_dir().join(CLI_DICT_FILENAME);
    if cli_file.exists() {
        let entries = read_cli_dict(&cli_file)?;
        if !entries.is_empty() {
            println!("\n[cli-added.toml の最初 {} 件]", limit.min(entries.len()));
            for (s, r) in entries.iter().take(limit) {
                println!("  {s}\t{r}");
            }
        }
    }

    print_files_in_dir("core", &paths.dict_core_dir())?;
    print_files_in_dir("user", &paths.dict_user_dir())?;

    let overrides = paths.overrides_file();
    if overrides.exists() {
        println!("\n[overrides] {}", overrides.display());
    }

    Ok(())
}

fn print_files_in_dir(label: &str, dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let mut files: Vec<_> = fs::read_dir(dir)?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Ok(());
    }
    println!("\n[{label}/ 配下 *.toml]");
    for f in files {
        let size = fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
        println!("  {} ({size} bytes)", f.display());
    }
    Ok(())
}

// ─── remove ──────────────────────────────────────────────────────────────────

fn remove(paths: &Paths, surface: &str) -> Result<()> {
    let cli_file = paths.dict_user_dir().join(CLI_DICT_FILENAME);
    if !cli_file.exists() {
        bail!(
            "{} が存在しません。`furigana dict add` で追加した語のみ削除できます。",
            cli_file.display()
        );
    }
    let mut entries = read_cli_dict(&cli_file)?;
    if entries.remove(surface).is_none() {
        bail!("'{surface}' は cli-added.toml に見つかりません");
    }
    write_cli_dict(&cli_file, &entries)?;
    println!("削除: {surface}");
    Ok(())
}

// ─── import ──────────────────────────────────────────────────────────────────

fn import(paths: &Paths, src: &Path) -> Result<()> {
    if !src.exists() {
        bail!("ファイルが見つかりません: {}", src.display());
    }
    if !src.is_file() {
        bail!("ファイルではありません: {}", src.display());
    }

    // 先にバリデーション (パース失敗ならコピーしない)
    let validated = Dict::from_toml_file(src).with_context(|| {
        format!(
            "TOML パース失敗: {} ([entries] セクション + key=value 形式が必要)",
            src.display()
        )
    })?;

    let user_dir = paths.dict_user_dir();
    fs::create_dir_all(&user_dir)?;

    let dest_name = src
        .file_name()
        .ok_or_else(|| anyhow!("ファイル名が取得できません: {}", src.display()))?;
    let dest = user_dir.join(dest_name);

    fs::copy(src, &dest)?;
    println!("インポート完了 ({} 件)", validated.len());
    println!("  {} → {}", src.display(), dest.display());
    Ok(())
}

// ─── pull は dict_pull.rs に切り出し ─────────────────────────────────────────

// ─── 内部ヘルパー: cli-added.toml の read/write ───────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct CliDictFile {
    #[serde(default)]
    entries: BTreeMap<String, String>,
}

/// `cli-added.toml` を BTreeMap として読む (キー昇順で扱う)
fn read_cli_dict(path: &Path) -> Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let content = fs::read_to_string(path)?;
    let parsed: CliDictFile =
        toml::from_str(&content).with_context(|| format!("{} のパース失敗", path.display()))?;
    Ok(parsed.entries)
}

/// 整形済みの `cli-added.toml` を書き出す
fn write_cli_dict(path: &Path, entries: &BTreeMap<String, String>) -> Result<()> {
    let mut out = String::from(
        "# `furigana dict add/remove` で更新される CLI 管理エントリ\n\
         # surface = reading の TOML inline map\n\
         \n\
         [entries]\n",
    );
    for (s, r) in entries {
        // TOML 文字列リテラル化 (基本的に basic string で OK、escape も対応)
        out.push('"');
        out.push_str(&toml_escape(s));
        out.push_str("\" = \"");
        out.push_str(&toml_escape(r));
        out.push_str("\"\n");
    }
    fs::write(path, out)?;
    Ok(())
}

/// 制御文字 (`\t` `\n` `\r` 以外の C0 / DEL) を含む入力を reject。
fn validate_user_input(label: &str, s: &str) -> Result<()> {
    if let Some(c) = s.chars().find(|c| {
        let code = *c as u32;
        // 0x09 (\t), 0x0A (\n), 0x0D (\r) は escape 可なので許容、
        // それ以外の C0 (0x00..0x1F) と DEL (0x7F) は reject。
        (code < 0x20 && code != 0x09 && code != 0x0A && code != 0x0D) || code == 0x7F
    }) {
        bail!("{label} に制御文字 U+{:04X} を含む: {s:?}", c as u32);
    }
    Ok(())
}

/// TOML basic string 用のエスケープ (`"` `\` `\n` `\r` `\t`)
fn toml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_user_input_rejects_control_chars() {
        // 通常テキスト / tab・改行・CR は許容
        validate_user_input("x", "灰桜").unwrap();
        validate_user_input("x", "a\tb\nc\rd").unwrap();
        // C0 制御 (NUL/BEL) と DEL は reject (cli-added.toml 破壊 / self-DoS 防御)
        assert!(validate_user_input("x", "a\u{0}b").is_err());
        assert!(validate_user_input("x", "a\u{7}b").is_err());
        assert!(validate_user_input("x", "a\u{7f}b").is_err());
    }

    #[test]
    fn toml_escape_escapes_specials_only() {
        assert_eq!(toml_escape("ふつう"), "ふつう"); // 非対象は不変
        assert_eq!(toml_escape("a\"b"), "a\\\"b");
        assert_eq!(toml_escape("a\\b"), "a\\\\b");
        assert_eq!(toml_escape("a\nb"), "a\\nb");
        assert_eq!(toml_escape("a\rb"), "a\\rb");
        assert_eq!(toml_escape("a\tb"), "a\\tb");
    }
}

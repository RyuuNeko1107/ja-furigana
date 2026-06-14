//! データディレクトリ・設定パスの解決
//!
//! 優先順位: CLI `--data-dir` / `--config` > 環境変数 > **実行ファイルと同じディレクトリ** > XDG fallback
//!
//! 「`furigana.exe` をダブルクリックで起動 → 同じフォルダに `dict/` 等が展開される」
//! portable な配置を default にしてある。フォルダごとコピーすれば持ち運び可能。
//! `current_exe()` の解決に失敗した場合のみ XDG (`~/.local/share/furigana/` /
//! `%LOCALAPPDATA%\furigana\furigana\`) に fallback する。

use anyhow::{anyhow, Result};
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

/// 解決済みの各種パス
#[derive(Debug, Clone)]
pub struct Paths {
    /// 実行時データのルート (辞書 / rules 等)
    pub data_dir: PathBuf,
    /// 設定ファイル本体
    pub config_file: PathBuf,
}

impl Paths {
    /// デフォルト + override で解決
    pub fn resolve(override_data: Option<&Path>, override_config: Option<&Path>) -> Result<Self> {
        let data_dir = if let Some(d) = override_data {
            d.to_path_buf()
        } else {
            default_data_dir()?
        };
        let config_file = override_config
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("config.toml"));

        Ok(Self {
            data_dir,
            config_file,
        })
    }

    /// データルート: `<data_dir>/data/`
    ///
    /// すべてのデータをこの 1 階層に flat に集約する。core 辞書と rules を
    /// 別ディレクトリに分けない (どちらも同じ furigana-dict から PR/配布される
    /// もので、ユーザー視点で分ける意味が薄いため)。lib の loader は内部的に
    /// 必要なファイルだけ拾うため、両方に同じ path を渡しても衝突しない:
    /// `Dict::from_toml_dir(data/)` は `[entries]` セクション持つ TOML だけ
    /// merge し、`load_rules_dir(data/)` は `days.toml` / `counters/*.toml`
    /// 等の特定ファイル名だけ拾う (両者のスキャン対象は重ならない)。
    #[must_use]
    pub fn data_root(&self) -> PathBuf {
        self.data_dir.join("data")
    }

    /// core 辞書 (`data_root` と同じ。lib API 互換のため別 method を残してある)
    #[must_use]
    pub fn dict_core_dir(&self) -> PathBuf {
        self.data_root()
    }

    /// rules (`data_root` と同じ。lib API 互換のため別 method を残してある)
    #[must_use]
    pub fn rules_dir(&self) -> PathBuf {
        self.data_root()
    }

    /// user 辞書: `<data_dir>/data/user/`
    #[must_use]
    pub fn dict_user_dir(&self) -> PathBuf {
        self.data_root().join("user")
    }

    /// overrides ファイル: `<data_dir>/data/overrides.toml`
    #[must_use]
    pub fn overrides_file(&self) -> PathBuf {
        self.data_root().join("overrides.toml")
    }
}

/// `--data-dir` / env 未指定時の data_dir 解決:
/// 1. 実行ファイル (`furigana.exe`) と同じディレクトリ
/// 2. 失敗時のみ XDG / `%LOCALAPPDATA%` (滅多に到達しない)
fn default_data_dir() -> Result<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            return Ok(parent.to_path_buf());
        }
    }
    let project = ProjectDirs::from("com", "furigana", "furigana")
        .ok_or_else(|| anyhow!("プロジェクトディレクトリの解決に失敗"))?;
    Ok(project.data_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_uses_overrides_and_derives_subpaths() {
        let base = Path::new("/tmp/furi_d");
        let p = Paths::resolve(Some(base), Some(Path::new("/tmp/c.toml"))).unwrap();
        assert_eq!(p.data_dir, base);
        assert_eq!(p.config_file, Path::new("/tmp/c.toml"));
        // sub-path 導出 (OS セパレータ差を避けるため join で期待値を作る)
        assert_eq!(p.data_root(), base.join("data"));
        assert_eq!(p.dict_user_dir(), base.join("data").join("user"));
        assert_eq!(p.overrides_file(), base.join("data").join("overrides.toml"));
    }

    #[test]
    fn resolve_config_defaults_under_data_dir() {
        let base = Path::new("/tmp/furi_d");
        let p = Paths::resolve(Some(base), None).unwrap();
        assert_eq!(p.config_file, base.join("config.toml"));
    }
}

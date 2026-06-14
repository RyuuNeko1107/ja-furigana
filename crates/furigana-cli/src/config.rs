//! 設定ファイル (TOML) の読み込み
//!
//! 設定ファイルは optional。存在しない場合は default 値を使う。
//! 各フィールドは `serve` サブコマンドから読まれる。

use crate::paths::Paths;
use anyhow::{Context, Result};
use serde::Deserialize;

/// CLI 設定 (config.toml 全体)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct Config {
    /// HTTP サーバー設定
    #[serde(default)]
    pub server: ServerConfig,

    /// bearer 認証設定
    #[serde(default)]
    pub auth: AuthConfig,

    /// 自動辞書更新 (`furigana serve --auto-pull` の挙動も含む)
    #[serde(default)]
    pub auto_update: AutoUpdateConfig,

    /// rate limit 設定
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
}

/// `[server]` セクション
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// bind address (default: `127.0.0.1:8000`)
    #[serde(default = "default_bind")]
    pub bind: String,

    /// CORS 許可オリジン (空 = same-origin only)
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            cors_origins: Vec::new(),
        }
    }
}

fn default_bind() -> String {
    "127.0.0.1:8000".to_string()
}

/// `[auth]` セクション
#[derive(Debug, Default, Clone, Deserialize)]
pub struct AuthConfig {
    /// `/furigana` 認証用 (空 = 認証無効、ローカル想定のデフォルト)
    #[serde(default)]
    pub tokens: Vec<String>,

    /// `/admin/*` (reload 等) 専用トークン (空 = `/admin/*` 無効化)
    #[serde(default)]
    pub admin_tokens: Vec<String>,
}

/// `[auto_update]` セクション
///
/// `furigana serve` 起動中の **定期 polling** で GitHub Releases から
/// 最新 `ja-furigana-dict` を取得 → 自動 reload する仕組み。
/// 公開 API のみで完結するため admin_tokens 設定不要 (内部呼び出し)。
///
/// 起動時 1 回だけ pull したい場合は `--auto-pull` フラグの方を使う。
#[derive(Debug, Clone, Deserialize)]
pub struct AutoUpdateConfig {
    /// 定期 polling を有効化するか (default: false で opt-in)
    #[serde(default)]
    pub enabled: bool,

    /// polling 間隔。`"30m" / "1h" / "6h" / "1d"` 等の表記。
    /// 短すぎると GitHub API rate limit (60 req/h/IP) に当たるので 1h 以上推奨。
    #[serde(default = "default_interval")]
    pub interval: String,

    /// ピン留めする tag (例: `"v0.1.3"`)。空 or 未指定で **最新追従**。
    /// `--auto-pull` 起動時 pull もこのピンを尊重する。
    #[serde(default)]
    pub pin: String,
}

impl Default for AutoUpdateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval: default_interval(),
            pin: String::new(),
        }
    }
}

fn default_interval() -> String {
    "24h".to_string()
}

/// `[rate_limit]` セクション
#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    /// 1 秒あたりの許可リクエスト数 (default: 1)
    #[serde(default = "default_per_second")]
    pub per_second: u64,

    /// バーストサイズ (default: 5)
    #[serde(default = "default_burst_size")]
    pub burst_size: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            per_second: default_per_second(),
            burst_size: default_burst_size(),
        }
    }
}

fn default_per_second() -> u64 {
    1
}

fn default_burst_size() -> u32 {
    5
}

impl Config {
    /// 設定ファイルを読み込む (存在しなければ default)
    pub fn load(paths: &Paths) -> Result<Self> {
        if !paths.config_file.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&paths.config_file).with_context(|| {
            format!("設定ファイル読み込み失敗: {}", paths.config_file.display())
        })?;
        let cfg: Self = toml::from_str(&content)
            .with_context(|| format!("設定ファイルパース失敗: {}", paths.config_file.display()))?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rate_limit_is_1_per_sec_burst_5() {
        let cfg = Config::default();
        assert_eq!(cfg.rate_limit.per_second, 1);
        assert_eq!(cfg.rate_limit.burst_size, 5);
    }

    #[test]
    fn rate_limit_section_overrides_defaults() {
        let cfg: Config = toml::from_str(
            r#"
            [rate_limit]
            per_second = 20
            burst_size = 100
            "#,
        )
        .unwrap();
        assert_eq!(cfg.rate_limit.per_second, 20);
        assert_eq!(cfg.rate_limit.burst_size, 100);
    }

    #[test]
    fn missing_rate_limit_section_falls_back_to_defaults() {
        // 他セクションのみ指定 → rate_limit は default のまま
        let cfg: Config = toml::from_str(
            r#"
            [server]
            bind = "0.0.0.0:9000"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.rate_limit.per_second, 1);
        assert_eq!(cfg.rate_limit.burst_size, 5);
    }

    #[test]
    fn partial_rate_limit_keeps_other_default() {
        // per_second だけ指定 → burst_size は default(5)
        let cfg: Config = toml::from_str(
            r#"
            [rate_limit]
            per_second = 50
            "#,
        )
        .unwrap();
        assert_eq!(cfg.rate_limit.per_second, 50);
        assert_eq!(cfg.rate_limit.burst_size, 5);
    }

    fn temp_paths(tag: &str) -> (std::path::PathBuf, Paths) {
        let mut d = std::env::temp_dir();
        d.push(format!("furi_cfgtest_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let cfg = d.join("config.toml");
        (
            d.clone(),
            Paths {
                data_dir: d,
                config_file: cfg,
            },
        )
    }

    #[test]
    fn load_missing_file_returns_default() {
        let (dir, paths) = temp_paths("missing");
        // config.toml は作らない → default が返る
        let cfg = Config::load(&paths).unwrap();
        assert_eq!(cfg.rate_limit.per_second, 1); // default
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_parses_existing_file() {
        let (dir, paths) = temp_paths("valid");
        std::fs::write(&paths.config_file, "[rate_limit]\nper_second = 20\n").unwrap();
        let cfg = Config::load(&paths).unwrap();
        assert_eq!(cfg.rate_limit.per_second, 20);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_errors_on_bad_toml_with_path_context() {
        let (dir, paths) = temp_paths("bad");
        std::fs::write(&paths.config_file, "this = = = not toml").unwrap();
        let err = Config::load(&paths).unwrap_err();
        // エラー文脈にファイルパスが含まれる
        assert!(
            format!("{err:#}").contains("config.toml"),
            "error should mention the config path: {err:#}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

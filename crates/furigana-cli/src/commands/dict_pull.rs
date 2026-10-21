//! `furigana dict pull` の実装
//!
//! GitHub Releases から furigana-dict の tarball を取得して `<data_dir>/data/`
//! 1 階層に flat 展開する (`extract_to` 参照)。
//!
//! 流れ:
//! 1. version 解決 (`--version` 指定 or GitHub API で latest)
//! 2. tarball + sha256 sidecar を download
//! 3. SHA-256 検証 (sidecar が取れなければ中止。 `--allow-unverified` で明示的に省ける)
//! 4. 一時ディレクトリへ展開: `core/X` / `rules/X` を `X` に prefix 剥がして flat 配置
//! 5. **展開した新辞書を実際に build して検証する** (rules / 辞書が読めること、 件数が少なすぎないこと)。
//!    読めない辞書は入れ替えず、 稼働中の辞書をそのまま残す ([`DictRejected`])
//! 6. `user/` と `overrides.toml` を引き継ぎ、 `<data_dir>/data/` と rename で入れ替え
//!    (途中で失敗しても旧辞書はそのまま残る。 同時に 2 つ走らないよう lock file で排他)
//!
//! tarball の中身は furigana-dict repo そのままの 2 階層 (`core/...` `rules/...`)、
//! 配布側で flat 化する。これは「PR 投稿者が見る repo 内構造」と「実際に動く配置」を
//! 別軸で持たせるため (paths.rs の `data_root()` も参照)。

use crate::paths::Paths;
use anyhow::{anyhow, bail, Context, Result};
use std::time::Duration;

mod download;
mod extract;

use download::*;
use extract::*;

const REPO: &str = "RyuuNeko1107/ja-furigana-dict";
const USER_AGENT: &str = concat!("furigana-cli/", env!("CARGO_PKG_VERSION"));

// ─── セキュリティ上限値 ───
// 現行 ja-furigana-dict は ~1 MB 程度なので 50 MB は十分余裕。 archive bomb /
// 帯域 DoS / fs 圧迫を防ぐため compressed download / 展開合計 / 1 entry / entry 数
// に上限を設ける。 上限超過は abort、 利用者は値を再考できるよう error message に
// 明示する。
const MAX_DOWNLOAD_BYTES: usize = 50 * 1024 * 1024;
const MAX_UNCOMPRESSED_TOTAL: u64 = 200 * 1024 * 1024;
const MAX_PER_ENTRY_BYTES: u64 = 10 * 1024 * 1024;
const MAX_ENTRY_COUNT: usize = 50_000;
/// sha256 sidecar (`<hex>  <name>` の 1 行) の上限。 数百 byte しかない
const MAX_SIDECAR_BYTES: usize = 4 * 1024;
/// GitHub API の応答 (`releases/latest`) の上限
const MAX_API_BYTES: usize = 1024 * 1024;

/// 展開した新辞書の件数の下限。 空に近い tarball (中身が欠けた配布物) を入れ替えないための安全弁。
/// 実際の配布物は 10 万件を超える
const MIN_DICT_ENTRIES: usize = 10_000;

/// 取得した辞書の **中身が使えない** (rules / 辞書が読めない・件数が少なすぎる)。
///
/// 同じ tag を何度取得しても同じ結果になるので、 呼び出し側 (serve の自動更新) は同じ tag を
/// 再試行しない。 ネットワークの失敗など、 やり直せば直る失敗とは区別する。
#[derive(Debug)]
pub struct DictRejected(pub String);

impl std::fmt::Display for DictRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "取得した辞書は使えないため入れ替えませんでした (稼働中の辞書はそのまま): {}",
            self.0
        )
    }
}

impl std::error::Error for DictRejected {}

/// pull の挙動を変えるオプション
#[derive(Clone, Copy, Debug, Default)]
pub struct PullOptions {
    /// SHA-256 sidecar が取れない release でも続行する。 既定は中止 (整合性を確認できないため)
    pub allow_unverified: bool,
}

pub fn run(paths: &Paths, version: Option<&str>) -> Result<()> {
    run_with(paths, version, PullOptions::default())
}

pub fn run_with(paths: &Paths, version: Option<&str>, opts: PullOptions) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(60))
        .build()
        .context("HTTP クライアント初期化失敗")?;

    let tag = if let Some(v) = version {
        v.to_string()
    } else {
        println!("最新リリースを確認中...");
        resolve_latest_tag(&client)
            .context("最新リリースの解決に失敗 (network or GitHub API エラー)")?
    };
    // tag は archive name と URL に直接埋め込まれるため、 path traversal や
    // 別 host 注入を防ぐべく **strict format validation** を行う。
    // 想定: `v<digit>.<digit>.<digit>` (任意で `-pre.<digit>` or CalVer
    // `vYYYY.MM.DD`)、 `[A-Za-z0-9.-]` のみ、 `..` `/` `:` 等は禁止。
    validate_tag_format(&tag)
        .with_context(|| format!("不正な tag format (URL injection 防御のため拒否): {tag:?}"))?;
    println!("取得対象: {tag}");

    let archive_name = format!("furigana-dict-{tag}.tar.gz");
    let tarball_url = format!("https://github.com/{REPO}/releases/download/{tag}/{archive_name}");
    let sha_url = format!("{tarball_url}.sha256");

    println!("ダウンロード中: {archive_name}");
    let tarball = download_bytes(&client, &tarball_url)
        .with_context(|| format!("tarball 取得失敗: {tarball_url}"))?;
    println!("  {} bytes", tarball.len());

    println!("SHA-256 sidecar 取得中...");
    let expected_hex = match download_text(&client, &sha_url) {
        Ok(text) => Some(
            parse_sha256_sidecar(&text)
                .with_context(|| format!("sha256 sidecar の parse 失敗: {sha_url}"))?,
        ),
        Err(e) => {
            tracing::warn!("SHA-256 sidecar 取得失敗: {e}");
            None
        }
    };
    if verify_sha256(&tarball, expected_hex.as_deref(), opts.allow_unverified)? {
        println!("SHA-256 検証 OK");
    }

    println!("展開中...");
    extract_validated(&tarball, paths, |stage| {
        validate_staged(stage, MIN_DICT_ENTRIES)
    })
    .map_err(|e| {
        // 中身が使えない失敗はそのまま返す (呼び出し側が型で見分ける)
        if e.downcast_ref::<DictRejected>().is_some() {
            e
        } else {
            e.context("tar.gz 展開失敗")
        }
    })?;

    println!(
        "完了: {tag} を {} に配置しました",
        paths.data_root().display()
    );
    Ok(())
}

/// GitHub API `/releases/latest` から tag_name を取得。
fn resolve_latest_tag(client: &reqwest::blocking::Client) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
    }
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()?;
    let status = resp.status();
    if !status.is_success() {
        bail!(
            "GitHub API {status}: {url}\n  body: {}",
            error_body_head(resp)
        );
    }
    let body = read_limited(resp, MAX_API_BYTES, &url)?;
    let release: Release =
        serde_json::from_slice(&body).context("GitHub API の応答の parse 失敗")?;
    Ok(release.tag_name)
}

/// `resolve_latest_tag` の async 版。`serve` の auto_update task のように tokio
/// runtime 内から呼び出すためのフロント。実体は spawn_blocking で sync 版を呼ぶ。
pub async fn resolve_latest_tag_async() -> Result<String> {
    tokio::task::spawn_blocking(|| {
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .context("HTTP クライアント初期化失敗")?;
        resolve_latest_tag(&client)
    })
    .await
    .map_err(|e| anyhow!("spawn_blocking join error: {e}"))?
}

#[cfg(test)]
mod tests;

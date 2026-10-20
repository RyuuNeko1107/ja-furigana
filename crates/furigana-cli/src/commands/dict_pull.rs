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
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

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

fn download_bytes(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>> {
    let resp = client.get(url).send()?;
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {status}: {url}");
    }
    // 帯域 DoS / disk DoS 防御: Content-Length と body 実 size の両方で上限チェック。
    // server が Content-Length を 嘘ついても post-check で catch する (defense in depth)。
    if let Some(len) = resp.content_length() {
        if len > MAX_DOWNLOAD_BYTES as u64 {
            bail!("download size {len} bytes が上限 {MAX_DOWNLOAD_BYTES} bytes を超過: {url}");
        }
    }
    // Content-Length が無い (chunked) / 嘘の場合でも、 上限を超えた時点で読むのをやめる
    read_limited(resp, MAX_DOWNLOAD_BYTES, url)
}

/// エラー応答の本文の先頭 300 字 (診断表示用)。 `resp.text()` は全量を溜めるので、
/// 先頭 2 KiB だけ読んでやめる。 読めなくても診断が欠けるだけで、 失敗扱いは変えない。
fn error_body_head(reader: impl Read) -> String {
    let mut buf = Vec::new();
    let _ = reader.take(2048).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).chars().take(300).collect()
}

/// `reader` を最大 `limit` bytes まで読む。 超えたら (全部は読まずに) 中止。
fn read_limited(reader: impl Read, limit: usize, what: &str) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut buf)
        .with_context(|| format!("応答の読み込み失敗: {what}"))?;
    if buf.len() > limit {
        bail!("応答が上限 {limit} bytes を超過: {what}");
    }
    Ok(buf)
}

fn download_text(client: &reqwest::blocking::Client, url: &str) -> Result<String> {
    let resp = client.get(url).send()?;
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {status}: {url}");
    }
    let body = read_limited(resp, MAX_SIDECAR_BYTES, url)?;
    String::from_utf8(body).with_context(|| format!("UTF-8 でない応答: {url}"))
}

/// SHA-256 を検証する。 検証した (一致した) なら `Ok(true)`、 `allow_unverified` で省いたなら `Ok(false)`。
///
/// sidecar が無い / 取れない場合 (`expected = None`) は既定で中止する。 sidecar は tarball と
/// 同じ配布元から取るので破損しか検知できないが、 それすら無い状態で辞書を差し替えない。
fn verify_sha256(tarball: &[u8], expected: Option<&str>, allow_unverified: bool) -> Result<bool> {
    match expected {
        Some(expected_hex) => {
            let actual_hex = sha256_hex(tarball);
            if !actual_hex.eq_ignore_ascii_case(expected_hex) {
                bail!("SHA-256 mismatch:\n  expected: {expected_hex}\n  actual:   {actual_hex}");
            }
            Ok(true)
        }
        None if allow_unverified => {
            tracing::warn!("SHA-256 を検証せずに続行します (--allow-unverified)");
            Ok(false)
        }
        None => bail!(
            "SHA-256 sidecar が取得できないため中止します (整合性を確認できません)。\n  \
             検証なしで続けるなら `--allow-unverified` を指定してください"
        ),
    }
}

/// `sha256sum` 形式 (`<hex>  <filename>`) から hex を抜き出す。
fn parse_sha256_sidecar(text: &str) -> Result<String> {
    let first = text
        .lines()
        .next()
        .ok_or_else(|| anyhow!("空の sha256 sidecar"))?
        .trim();
    let hex = first
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("sha256 sidecar の形式が不正: {first:?}"))?;
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("sha256 hex の長さ/文字種が不正: {hex:?}");
    }
    Ok(hex.to_string())
}

/// tag format を strict に validate する。 URL / path 文脈に注入される値なので、
/// 既知の安全文字 `[A-Za-z0-9.\-]` のみ許可、 連続 `..` 禁止、 1〜64 文字。
///
/// 通常 release tag (`v0.1.0-alpha.8`、 `v2026.05.07` 等) はすべて通る。
/// `..` / `/` / `:` / null byte / control char などは reject。
fn validate_tag_format(tag: &str) -> Result<()> {
    if tag.is_empty() || tag.len() > 64 {
        bail!("tag 長が範囲外: {} 文字 (許容 1..=64)", tag.len());
    }
    if tag.contains("..") {
        bail!("tag に連続 `..` を含む (path traversal 経路): {tag:?}");
    }
    if !tag
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    {
        bail!("tag に許容外文字を含む (許容: A-Za-z0-9.-): {tag:?}");
    }
    Ok(())
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// テスト用の薄い入口: 検査なしで展開する (入れ替えの挙動の検証用)。
#[cfg(test)]
fn extract_to(tarball: &[u8], paths: &Paths) -> Result<()> {
    extract_validated(tarball, paths, |_| Ok(()))
}

/// tar.gz バイト列を `<data_dir>/data/` 配下に flat 展開する。
///
/// furigana-dict repo の archive は `core/...` `rules/...` で 2 階層に分かれて
/// いるが、配布物は最終的に `data/` 1 階層にまとめる:
/// - `core/unihan.toml`        → `data/unihan.toml`
/// - `core/jukugo/general.toml`→ `data/jukugo/general.toml`
/// - `rules/days.toml`         → `data/days.toml`
/// - `rules/counters/*.toml`   → `data/counters/*.toml`
///
/// lib loader は内部的に「Dict (recursive *.toml で `[entries]` 拾う) vs Rules
/// (特定ファイル名 + counters/ context/ サブのみ)」と排他的に scan するので、
/// 同じ `data/` ディレクトリを両方に渡しても干渉しない (paths.rs 参照)。
///
/// 「core/ と rules/ を分ける必要ない (同じ furigana-dict から PR/DL する
/// データなのに)」という指摘を受けてこの flat layout に統合した。
///
/// 入れ替えは **一時ディレクトリへ展開 → `user/` と `overrides.toml` を引き継ぎ → rename** の順。
/// 展開の途中で失敗 (壊れた archive / 上限超過 / ディスク満杯 / kill) しても、 稼働中の辞書は
/// そのまま残る。 archive 内の `user/` と `overrides.toml` は無視する (ユーザーのデータを
/// 配布物で上書きさせない)。 同時に 2 つの pull が走らないよう `<data_dir>/.furigana-dict.lock` で排他する。
///
/// 入れ替えの前に、 展開した新辞書を `validate` で検査する (build できるか・件数が少なすぎないか)。
/// 通らなければ入れ替えずに中止する。
fn extract_validated(
    tarball: &[u8],
    paths: &Paths,
    validate: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let data_root: PathBuf = paths.data_root();
    let user_dir: PathBuf = paths.dict_user_dir();
    let overrides: PathBuf = paths.overrides_file();
    let parent: PathBuf = data_root
        .parent()
        .ok_or_else(|| {
            anyhow!(
                "data_root の親ディレクトリが解決できない: {}",
                data_root.display()
            )
        })?
        .to_path_buf();
    fs::create_dir_all(&parent)?;

    let _lock = PullLock::acquire(&parent)?;
    let stage = fresh_sibling(&parent, ".data-new")?;

    let result = (|| -> Result<()> {
        // data_root 配下の相対パスで「ユーザーのもの」を判定する
        let user_rel = user_dir
            .strip_prefix(&data_root)
            .unwrap_or(Path::new("user"));
        let overrides_rel = overrides
            .strip_prefix(&data_root)
            .unwrap_or(Path::new("overrides.toml"));
        extract_into(tarball, &stage, user_rel, overrides_rel)?;
        // 配布物そのものを検査する (ユーザーの `user/` や overrides の不備は、 この pull のせいにしない)。
        // 入れ替えの前なので、 ここで失敗すれば稼働中の辞書は一切触られない
        validate(&stage)?;
        // 旧辞書のユーザーデータを (コピーで) 引き継ぐ。 元は入れ替えが済むまで触らない
        if user_dir.is_dir() {
            copy_dir_all(&user_dir, &stage.join(user_rel))?;
        }
        if overrides.is_file() {
            fs::copy(&overrides, stage.join(overrides_rel))
                .with_context(|| format!("引き継ぎ失敗: {}", overrides.display()))?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&stage);
        return Err(e);
    }

    if data_root.exists() {
        let backup = fresh_sibling(&parent, ".data-old")?;
        // `fresh_sibling` が空ディレクトリを作るので、 rename 先は一度消す
        fs::remove_dir(&backup)?;
        fs::rename(&data_root, &backup)
            .with_context(|| format!("旧辞書の退避失敗: {}", data_root.display()))?;
        if let Err(e) = fs::rename(&stage, &data_root) {
            // 入れ替えに失敗したら旧辞書を戻す
            let _ = fs::rename(&backup, &data_root);
            let _ = fs::remove_dir_all(&stage);
            return Err(anyhow!(e)).context("新辞書の配置失敗 (旧辞書を戻しました)");
        }
        let _ = fs::remove_dir_all(&backup);
    } else {
        fs::rename(&stage, &data_root)
            .with_context(|| format!("辞書の配置失敗: {}", data_root.display()))?;
    }
    Ok(())
}

/// 展開した新辞書を実際に build して、 使えるかを確かめる。
///
/// 配布物 (`stage`) だけを読む: ユーザーの `user/` や overrides の不備は、 この pull のせいにしない
/// (それらはまだ引き継いでいない)。 次のどれかなら [`DictRejected`] を返す。
/// - rules / 辞書が読めない (壊れた TOML・schema 違反など。 build が失敗する)
/// - 辞書の件数が `min_entries` に満たない (中身が欠けた配布物)
fn validate_staged(stage: &Path, min_entries: usize) -> Result<()> {
    let reject = |why: String| anyhow::Error::new(DictRejected(why));
    let f = furigana::Furigana::builder()
        .rules_dir(stage)
        .core_dict_dir(stage)
        .build()
        .map_err(|e| reject(format!("build できません: {e}")))?;
    let n = f.dict_size();
    if n < min_entries {
        return Err(reject(format!(
            "辞書の件数が少なすぎます ({n} 件、 下限 {min_entries} 件)"
        )));
    }
    Ok(())
}

/// 信頼できない tarball を `stage` (空の新規ディレクトリ) に展開する。
/// `user_rel` / `overrides_rel` (`data/` からの相対) に当たる entry は無視する。
fn extract_into(tarball: &[u8], stage: &Path, user_rel: &Path, overrides_rel: &Path) -> Result<()> {
    let gz = GzDecoder::new(tarball);
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(false);

    let mut total_uncompressed: u64 = 0;
    let mut entry_count: usize = 0;

    for entry in archive.entries()? {
        let mut entry = entry?;

        // entry 数上限 (zip bomb 的に大量小ファイルでも防御)
        entry_count += 1;
        if entry_count > MAX_ENTRY_COUNT {
            bail!("archive の entry 数が上限 {MAX_ENTRY_COUNT} を超過 (現在 {entry_count})");
        }

        // entry type 制限: Regular file と Directory のみ許可。 symlink / hardlink /
        // char device / block device / fifo は **絶対 reject** (path traversal 経路 +
        // sensitive file 露出の典型)。
        let entry_type = entry.header().entry_type();
        if !entry_type.is_file() && !entry_type.is_dir() {
            bail!(
                "拒否された archive entry type: {:?} ({})",
                entry_type,
                entry.path()?.display()
            );
        }

        // entry サイズ上限 (1 file)。 `entry.size()` は PAX 拡張の `size` を反映した実サイズ。
        // (header 生の size だと、 PAX で巨大サイズを宣言した entry が上限を素通りする)
        let entry_size = entry.size();
        if entry_size > MAX_PER_ENTRY_BYTES {
            bail!(
                "archive entry が大きすぎる: {entry_size} bytes (上限 {MAX_PER_ENTRY_BYTES} bytes): {}",
                entry.path()?.display()
            );
        }

        // 展開累積サイズ上限 (archive bomb 防御)
        total_uncompressed = total_uncompressed.saturating_add(entry_size);
        if total_uncompressed > MAX_UNCOMPRESSED_TOTAL {
            bail!(
                "archive 展開合計サイズが上限 {MAX_UNCOMPRESSED_TOTAL} bytes を超過 (現在 {total_uncompressed} bytes)"
            );
        }

        let entry_path = entry.path()?.into_owned();
        // archive 内 `core/...` `rules/...` の prefix を剥がして `stage` 直下に。
        // prefix だけのディレクトリエントリ (`core/` `rules/`) は rest が空に
        // なるので skip。
        let rest = if let Ok(rest) = entry_path.strip_prefix("core") {
            rest
        } else if let Ok(rest) = entry_path.strip_prefix("rules") {
            rest
        } else {
            // 想定外の top-level entry は無視 (README 等が混入してもスキップ)
            tracing::debug!("skip archive entry: {}", entry_path.display());
            continue;
        };
        if rest.as_os_str().is_empty() {
            continue;
        }
        // path traversal 防御: ディレクトリを作る **前** に、 構成要素が通常の名前だけか確認する
        // (`..` / 絶対パス / ドライブ名 / `:` (NTFS の代替データストリーム) を含むものは拒否)
        if !is_plain_relative(rest) {
            bail!(
                "不正な path を検出: {} (`..` / 絶対パス / `:` は不可)",
                entry_path.display()
            );
        }
        // ユーザーのデータ (user/ と overrides.toml) は配布物で上書きさせない
        if rest.starts_with(user_rel) || rest == overrides_rel {
            tracing::warn!(
                "archive 内のユーザー領域の entry を無視: {}",
                entry_path.display()
            );
            continue;
        }

        let dest = stage.join(rest);
        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&dest)?;
        } else {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            // stage は新規ディレクトリで symlink は作られない。 書く量にも上限を掛ける
            // (宣言サイズと実サイズが食い違っても 1 file の上限を超えて書かない)
            let mut out = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&dest)
                .with_context(|| format!("作成失敗: {}", dest.display()))?;
            let written = std::io::copy(&mut (&mut entry).take(MAX_PER_ENTRY_BYTES + 1), &mut out)?;
            if written > MAX_PER_ENTRY_BYTES {
                bail!(
                    "archive entry が大きすぎる (実際に書いた量): {} (上限 {MAX_PER_ENTRY_BYTES} bytes)",
                    entry_path.display()
                );
            }
        }
    }
    Ok(())
}

/// `rest` が通常の名前だけで構成された相対パスか (`..` / `/` 始まり / ドライブ名 / `:` を含む名前は不可)。
fn is_plain_relative(rest: &Path) -> bool {
    rest.components().all(|c| match c {
        Component::Normal(name) => !name.to_string_lossy().contains(':'),
        _ => false,
    })
}

/// `parent` の下に、 予測しにくい名前の新しい空ディレクトリを作る (`<prefix>-<pid>-<nanos>`)。
fn fresh_sibling(parent: &Path, prefix: &str) -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = parent.join(format!("{prefix}-{}-{nanos}", std::process::id()));
    // `create_dir` は既存なら失敗する (他のプロセスのものを再利用しない)
    fs::create_dir(&dir)
        .with_context(|| format!("一時ディレクトリの作成失敗: {}", dir.display()))?;
    Ok(dir)
}

/// 通常ファイルとディレクトリだけを再帰コピーする (symlink などは引き継がない)。
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let meta = fs::symlink_metadata(&from)?;
        if meta.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if meta.is_file() {
            fs::copy(&from, &to).with_context(|| format!("引き継ぎ失敗: {}", from.display()))?;
        } else {
            tracing::warn!("通常ファイル以外は引き継ぎません: {}", from.display());
        }
    }
    Ok(())
}

/// 同時に 2 つの pull (serve の auto_update / `dict pull` / REPL の `:pull`) が走らないようにする。
/// 取得できるまで待つ。 drop (ファイルを閉じる) で解放される。 前回の中断で残った
/// 一時ディレクトリ (`.data-new-*` / `.data-old-*`) は、 lock を取った後に掃除する。
struct PullLock {
    _file: fs::File,
}

impl PullLock {
    fn acquire(dir: &Path) -> Result<Self> {
        let path = dir.join(".furigana-dict.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("lock file を開けない: {}", path.display()))?;
        file.lock()
            .with_context(|| format!("lock の取得失敗: {}", path.display()))?;
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(".data-new-") || name.starts_with(".data-old-") {
                    let _ = fs::remove_dir_all(e.path());
                }
            }
        }
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_tag_format() {
        // 通常の release tag
        assert!(validate_tag_format("v0.1.0").is_ok());
        assert!(validate_tag_format("v0.1.0-alpha.8").is_ok());
        assert!(validate_tag_format("v2026.05.07").is_ok());
        // attack pattern
        assert!(validate_tag_format("").is_err());
        assert!(validate_tag_format("../../etc/passwd").is_err());
        assert!(validate_tag_format("v0.1.0/extra").is_err());
        assert!(validate_tag_format("v0..1.0").is_err());
        assert!(validate_tag_format("v0.1.0\0evil").is_err());
        assert!(validate_tag_format("v0.1.0:9000").is_err());
        assert!(validate_tag_format(&"v".repeat(65)).is_err());
    }

    #[test]
    fn parses_sha256_sidecar() {
        let text = "8e7d1c4...abcd  furigana-dict-v0.1.0.tar.gz\n";
        // 短すぎるので reject
        assert!(parse_sha256_sidecar(text).is_err());

        let valid = format!("{}  furigana-dict-v0.1.0.tar.gz\n", "a".repeat(64));
        assert_eq!(parse_sha256_sidecar(&valid).unwrap(), "a".repeat(64));
    }

    #[test]
    fn sha256_known_vector() {
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    // ─── extract_to security tests ───────────────────────────────────────────
    //
    // extract_to は信頼できない tarball を展開する security-critical 経路。
    // path traversal / symlink / archive bomb / prefix 剥がし / 既存保持 を検証する。
    //
    // 注: count / total ガードは prefix 判定 (core/rules 以外は continue で skip) より
    // **前**に発火するため、 entry 名を skip 対象にすれば unpack されず disk を汚さずに
    // 検証できる。 total bomb は ~200MB の zero stream を decompress するので CPU は重め
    // だが disk 書き込みは無い。

    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::{Read, Write};

    /// 生 tar (512B header 手構築) を gzip して返す。 `tar::Builder` は `..` を含む
    /// path の書き込みを拒否するため、 path traversal 攻撃 tarball の再現には
    /// header を手で組む必要がある (= 悪意ある配布者 / 改竄 archive の模擬)。
    fn raw_targz_single(name: &str, content: &[u8]) -> Vec<u8> {
        let mut header = [0u8; 512];
        let nb = name.as_bytes();
        header[..nb.len()].copy_from_slice(nb);
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(format!("{:011o}\0", content.len()).as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[156] = b'0'; // typeflag = regular
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        // checksum: 該当 8 byte を space 埋めして全 byte 和を octal で書く
        for b in &mut header[148..156] {
            *b = b' ';
        }
        let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());

        let mut tar = Vec::new();
        tar.extend_from_slice(&header);
        tar.extend_from_slice(content);
        let pad = (512 - content.len() % 512) % 512;
        tar.extend(std::iter::repeat_n(0u8, pad));
        tar.extend(std::iter::repeat_n(0u8, 1024)); // end-of-archive marker

        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(&tar).unwrap();
        enc.finish().unwrap()
    }

    enum Tar<'a> {
        File(&'a str, &'a [u8]),
        Dir(&'a str),
        Symlink(&'a str, &'a str),
        /// header.size を詐称した巨大 entry (per-entry bomb 用、 実 content は zero stream)
        BigFile(&'a str, u64),
    }

    fn build_targz(entries: &[Tar]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for e in entries {
            match e {
                Tar::File(path, data) => {
                    let mut h = tar::Header::new_gnu();
                    h.set_size(data.len() as u64);
                    h.set_entry_type(tar::EntryType::Regular);
                    h.set_mode(0o644);
                    builder.append_data(&mut h, path, &data[..]).unwrap();
                }
                Tar::Dir(path) => {
                    let mut h = tar::Header::new_gnu();
                    h.set_size(0);
                    h.set_entry_type(tar::EntryType::Directory);
                    h.set_mode(0o755);
                    builder.append_data(&mut h, path, std::io::empty()).unwrap();
                }
                Tar::Symlink(path, target) => {
                    let mut h = tar::Header::new_gnu();
                    h.set_entry_type(tar::EntryType::Symlink);
                    h.set_size(0);
                    h.set_mode(0o777);
                    builder.append_link(&mut h, path, target).unwrap();
                }
                Tar::BigFile(path, size) => {
                    let mut h = tar::Header::new_gnu();
                    h.set_size(*size);
                    h.set_entry_type(tar::EntryType::Regular);
                    h.set_mode(0o644);
                    let reader = std::io::repeat(0u8).take(*size);
                    builder.append_data(&mut h, path, reader).unwrap();
                }
            }
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn temp_paths(tag: &str) -> Paths {
        let mut d = std::env::temp_dir();
        d.push(format!("furigana_pulltest_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        Paths {
            data_dir: d.clone(),
            config_file: d.join("config.toml"),
        }
    }

    #[test]
    fn extract_to_flattens_core_and_rules_prefixes() {
        let p = temp_paths("flatten");
        let tb = build_targz(&[
            Tar::Dir("core/"),
            Tar::File("core/jukugo/x.toml", b"[entries]\n"),
            Tar::File("rules/days.toml", b"[meta]\n"),
            Tar::File("README.md", b"ignored top-level"),
        ]);
        extract_to(&tb, &p).unwrap();
        let root = p.data_root();
        assert!(
            root.join("jukugo/x.toml").exists(),
            "core/ prefix が剥がれて flat 配置"
        );
        assert!(
            root.join("days.toml").exists(),
            "rules/ prefix が剥がれて flat 配置"
        );
        assert!(
            !root.join("README.md").exists(),
            "未知 top-level entry は skip"
        );
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn extract_to_rejects_or_contains_path_traversal() {
        let p = temp_paths("traversal");
        // 手構築 raw tar で core/ prefix 付きの ../ traversal を仕込む。
        let tb = raw_targz_single("core/../../../../evil.toml", b"PWNED");
        let res = extract_to(&tb, &p);
        // bail するのが理想だが、 少なくとも data_root の外に evil.toml が書かれないこと。
        let escaped = p
            .data_dir
            .parent()
            .map(|g| g.join("evil.toml"))
            .filter(|x| x.exists());
        assert!(
            res.is_err() || escaped.is_none(),
            "path traversal: bail せず & data_root 外に書込み (res={res:?})"
        );
        // 念のため data_dir の祖先側に evil.toml が出来ていないこと
        assert!(
            escaped.is_none(),
            "data_root 外に evil.toml が漏れた: {escaped:?}"
        );
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn extract_to_rejects_symlink_entry() {
        let p = temp_paths("symlink");
        let tb = build_targz(&[Tar::Symlink("core/link.toml", "/etc/passwd")]);
        let res = extract_to(&tb, &p);
        assert!(res.is_err(), "symlink entry は reject されるべき");
        assert!(
            format!("{:#}", res.unwrap_err()).contains("entry type"),
            "symlink reject は entry type エラーであること"
        );
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn extract_to_rejects_oversized_entry_before_unpack() {
        let p = temp_paths("bigentry");
        // header.size を上限+1 に詐称。 unpack 前 (header check) で bail するので
        // 実 disk 書き込みは発生しない。
        let tb = build_targz(&[Tar::BigFile("core/huge.toml", MAX_PER_ENTRY_BYTES + 1)]);
        let res = extract_to(&tb, &p);
        assert!(res.is_err(), "per-entry 上限超過は bail");
        assert!(
            !p.data_root().join("huge.toml").exists(),
            "bail 前に unpack しない"
        );
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn extract_to_preserves_user_dir_and_overrides() {
        let p = temp_paths("preserve");
        // 既存の user データと overrides を作っておく
        fs::create_dir_all(p.dict_user_dir()).unwrap();
        fs::write(p.dict_user_dir().join("mine.toml"), b"[entries]\n").unwrap();
        fs::write(p.overrides_file(), b"[entries]\n").unwrap();
        // 旧配布ファイルも 1 つ置く (これは消えるべき)
        fs::write(p.data_root().join("old.toml"), b"stale").unwrap();

        let tb = build_targz(&[Tar::File("core/jukugo/new.toml", b"[entries]\n")]);
        extract_to(&tb, &p).unwrap();

        assert!(p.dict_user_dir().join("mine.toml").exists(), "user/ は保持");
        assert!(p.overrides_file().exists(), "overrides.toml は保持");
        assert!(
            p.data_root().join("jukugo/new.toml").exists(),
            "新ファイルは展開"
        );
        assert!(
            !p.data_root().join("old.toml").exists(),
            "旧配布ファイルは掃除"
        );
        fs::remove_dir_all(&p.data_dir).ok();
    }

    /// `prefix/i.bin` という skip 対象 entry を count 個並べた tar.gz。
    /// `size` を指定すると header.size を詐称 (zero stream content)、 None なら空 entry。
    fn targz_skippable(prefix: &str, count: usize, size: Option<u64>) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for i in 0..count {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Regular);
            h.set_mode(0o644);
            let name = format!("{prefix}/{i}.bin");
            if let Some(sz) = size {
                h.set_size(sz);
                builder
                    .append_data(&mut h, name, std::io::repeat(0u8).take(sz))
                    .unwrap();
            } else {
                h.set_size(0);
                builder.append_data(&mut h, name, std::io::empty()).unwrap();
            }
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn extract_to_rejects_too_many_entries() {
        let p = temp_paths("countbomb");
        // MAX_ENTRY_COUNT+1 個の skip 対象 (非 core/rules) entry。 count guard は
        // prefix 判定より前なので unpack されず disk を汚さずに発火する。
        let tb = targz_skippable("junk", MAX_ENTRY_COUNT + 1, None);
        let res = extract_to(&tb, &p);
        assert!(res.is_err(), "entry 数上限超過は bail");
        assert!(
            format!("{:#}", res.unwrap_err()).contains("entry 数"),
            "entry 数上限エラーであること"
        );
        // skip 対象なので何も展開されていない
        assert_nothing_extracted(&p);
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn extract_to_rejects_total_uncompressed_overflow() {
        let p = temp_paths("totalbomb");
        // 各 entry は per-entry 上限 (10MB) 未満だが、 合計が 200MB を超える。
        // 21 × 9.9MB ≈ 207.9MB > 200MB。 skip 対象名なので unpack されず disk ゼロ。
        let per = (MAX_PER_ENTRY_BYTES * 99) / 100; // 9.9MB
        let count = (MAX_UNCOMPRESSED_TOTAL / per) as usize + 1;
        let tb = targz_skippable("junk", count, Some(per));
        let res = extract_to(&tb, &p);
        assert!(res.is_err(), "展開合計上限超過は bail");
        assert!(
            format!("{:#}", res.unwrap_err()).contains("展開合計"),
            "展開合計上限エラーであること"
        );
        assert_nothing_extracted(&p);
        fs::remove_dir_all(&p.data_dir).ok();
    }

    // ─── セキュリティ監査 (REPORT-001〜004, 012, 013) の回帰テスト ───────────────

    #[test]
    fn verify_sha256_is_strict_by_default() {
        let data = b"abc";
        let good = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        // 一致 → 検証した
        assert!(verify_sha256(data, Some(good), false).unwrap());
        // 不一致 → 中止 (--allow-unverified でも、 sidecar があって違うなら通さない)
        let bad = "0".repeat(64);
        assert!(verify_sha256(data, Some(&bad), false).is_err());
        assert!(verify_sha256(data, Some(&bad), true).is_err());
        // sidecar が無い → 既定は中止。 明示した時だけ検証なしで続行
        let err = verify_sha256(data, None, false).unwrap_err();
        assert!(format!("{err:#}").contains("--allow-unverified"));
        assert!(!verify_sha256(data, None, true).unwrap());
    }

    #[test]
    fn read_limited_stops_at_the_limit() {
        let data = [7u8; 100];
        assert_eq!(read_limited(&data[..], 100, "t").unwrap().len(), 100);
        let err = read_limited(&data[..], 99, "t").unwrap_err();
        assert!(format!("{err:#}").contains("上限"));
        // 無限に続く読み手でも、 上限 + 1 で止まる (全部は読まない)
        assert!(read_limited(std::io::repeat(0u8), 1024, "t").is_err());
    }

    #[test]
    fn error_body_head_reads_only_a_prefix() {
        // 無限に続く本文でも返ってくる (全量を溜めない)。 表示は 300 字まで
        let head = error_body_head(std::io::repeat(b'x'));
        assert_eq!(head.chars().count(), 300);
        // 壊れた UTF-8 でも落ちない
        assert!(!error_body_head(&[0xffu8, 0xfe, b'a'][..]).is_empty());
    }

    /// PAX 拡張ヘッダ (`size=`) で実サイズを宣言し、 ustar ヘッダの size は 0 にした entry。
    /// header 生の size を見る実装は上限を素通りする。
    fn targz_pax_size(name: &str, declared: u64) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        let size = declared.to_string();
        builder
            .append_pax_extensions([("size", size.as_bytes())])
            .unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Regular);
        h.set_mode(0o644);
        h.set_size(0);
        builder.append_data(&mut h, name, std::io::empty()).unwrap();
        // 宣言したサイズ分の中身 (+ padding) を直接流す
        let w = builder.get_mut();
        let zeros = vec![0u8; 64 * 1024];
        let mut left = declared;
        while left > 0 {
            let n = left.min(zeros.len() as u64) as usize;
            w.write_all(&zeros[..n]).unwrap();
            left -= n as u64;
        }
        let pad = (512 - declared % 512) % 512;
        w.write_all(&vec![0u8; pad as usize]).unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn extract_to_rejects_pax_declared_oversize() {
        let p = temp_paths("paxsize");
        let tb = targz_pax_size("core/pax.toml", MAX_PER_ENTRY_BYTES + 1);
        let res = extract_to(&tb, &p);
        assert!(res.is_err(), "PAX で宣言した巨大 entry は上限で bail");
        assert!(
            !p.data_root().join("pax.toml").exists(),
            "巨大ファイルは書かれない"
        );
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn extract_to_creates_no_directory_outside_on_traversal() {
        let p = temp_paths("traversaldir");
        // data_root = <data_dir>/data。 `core/../../<名前>/y.toml` は <data_dir の親>/<名前> を指す
        let marker = format!("furigana_escape_{}", std::process::id());
        let tb = raw_targz_single(&format!("core/../../{marker}/y.toml"), b"x");
        let res = extract_to(&tb, &p);
        assert!(res.is_err(), "`..` を含む path は bail");
        let outside = p.data_dir.parent().unwrap().join(&marker);
        assert!(
            !outside.exists(),
            "検査より前にディレクトリを作らない: {}",
            outside.display()
        );
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn extract_to_rejects_colon_in_path_component() {
        let p = temp_paths("colon");
        // NTFS の代替データストリーム (`file.toml:stream`) になりうる名前
        let tb = raw_targz_single("core/jukugo/x.toml:evil", b"x");
        assert!(extract_to(&tb, &p).is_err());
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn extract_to_ignores_archive_entries_in_user_area() {
        let p = temp_paths("userarea");
        fs::create_dir_all(p.dict_user_dir()).unwrap();
        fs::write(p.dict_user_dir().join("mine.toml"), b"MINE").unwrap();
        fs::write(p.overrides_file(), b"MY-OVERRIDES").unwrap();

        let tb = build_targz(&[
            Tar::File("core/user/mine.toml", b"EVIL"),
            Tar::File("core/user/extra.toml", b"EVIL"),
            Tar::File("core/overrides.toml", b"EVIL"),
            Tar::File("core/jukugo/new.toml", b"[entries]\n"),
        ]);
        extract_to(&tb, &p).unwrap();

        assert_eq!(
            fs::read(p.dict_user_dir().join("mine.toml")).unwrap(),
            b"MINE",
            "user/ は配布物で上書きされない"
        );
        assert!(
            !p.dict_user_dir().join("extra.toml").exists(),
            "配布物から user/ に新しいファイルを置かせない"
        );
        assert_eq!(
            fs::read(p.overrides_file()).unwrap(),
            b"MY-OVERRIDES",
            "overrides.toml は配布物で上書きされない"
        );
        assert!(p.data_root().join("jukugo/new.toml").exists());
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn failed_extract_leaves_existing_dictionary_untouched() {
        let p = temp_paths("atomic");
        fs::create_dir_all(p.data_root()).unwrap();
        fs::write(p.data_root().join("old.toml"), b"[entries]\nOLD").unwrap();
        fs::create_dir_all(p.dict_user_dir()).unwrap();
        fs::write(p.dict_user_dir().join("mine.toml"), b"MINE").unwrap();

        // 途中で拒否される tarball (先に有効な entry、 後ろに symlink)
        let tb = build_targz(&[
            Tar::File("core/new.toml", b"[entries]\nNEW"),
            Tar::Symlink("core/link.toml", "/etc/passwd"),
        ]);
        assert!(extract_to(&tb, &p).is_err());

        // 旧辞書は消えていない (以前の実装は展開の前に全部消していた)
        assert_eq!(
            fs::read(p.data_root().join("old.toml")).unwrap(),
            b"[entries]\nOLD"
        );
        assert!(p.dict_user_dir().join("mine.toml").exists());
        assert!(
            !p.data_root().join("new.toml").exists(),
            "半端な新辞書は配置されない"
        );
        assert_no_staging_dirs(&p);
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn successful_extract_swaps_and_leaves_no_staging_dirs() {
        let p = temp_paths("swap");
        fs::create_dir_all(p.data_root()).unwrap();
        fs::write(p.data_root().join("old.toml"), b"stale").unwrap();
        let tb = build_targz(&[Tar::File("core/new.toml", b"[entries]\n")]);
        extract_to(&tb, &p).unwrap();
        assert!(p.data_root().join("new.toml").exists());
        assert!(!p.data_root().join("old.toml").exists());
        assert_no_staging_dirs(&p);
        // 続けてもう一度 (lock が解放されている)
        extract_to(&tb, &p).unwrap();
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn leftover_staging_dirs_are_cleaned_on_next_pull() {
        let p = temp_paths("leftover");
        fs::create_dir_all(p.data_dir.join(".data-new-1-1/x")).unwrap();
        fs::create_dir_all(p.data_dir.join(".data-old-1-1")).unwrap();
        let tb = build_targz(&[Tar::File("core/new.toml", b"[entries]\n")]);
        extract_to(&tb, &p).unwrap();
        assert_no_staging_dirs(&p);
        fs::remove_dir_all(&p.data_dir).ok();
    }

    fn assert_nothing_extracted(p: &Paths) {
        assert!(
            !p.data_root().exists() || fs::read_dir(p.data_root()).unwrap().count() == 0,
            "何も展開されていないこと"
        );
        assert_no_staging_dirs(p);
    }

    fn assert_no_staging_dirs(p: &Paths) {
        let left: Vec<_> = fs::read_dir(&p.data_dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".data-new-") || n.starts_with(".data-old-"))
            .collect();
        assert!(left.is_empty(), "一時ディレクトリが残っている: {left:?}");
    }

    // ─── 入れ替え前の検証 (REPORT-035) ──────────────────────────────────────

    fn is_rejected(e: &anyhow::Error) -> bool {
        e.downcast_ref::<DictRejected>().is_some()
    }

    #[test]
    fn validate_staged_accepts_a_readable_dictionary_and_checks_the_size() {
        let p = temp_paths("validate_ok");
        let stage = p.data_dir.join("stage");
        fs::create_dir_all(stage.join("jukugo")).unwrap();
        fs::write(
            stage.join("jukugo/a.toml"),
            "[meta]
schema_version = \"2\"
role = \"jukugo\"

[entries]
\"灰桜\" = \"ハイザクラ\"
",
        )
        .unwrap();
        validate_staged(&stage, 1).unwrap();
        // 件数が下限に満たなければ却下
        let err = validate_staged(&stage, 10).unwrap_err();
        assert!(is_rejected(&err), "{err:#}");
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn validate_staged_rejects_an_unreadable_dictionary() {
        let p = temp_paths("validate_bad");
        let stage = p.data_dir.join("stage");
        fs::create_dir_all(&stage).unwrap();
        // rules として読めない (schema_version が無い)
        fs::write(
            stage.join("days.toml"),
            "[entries]
1 = \"ついたち\"
",
        )
        .unwrap();
        let err = validate_staged(&stage, 0).unwrap_err();
        assert!(is_rejected(&err), "{err:#}");
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn rejected_dictionary_leaves_the_running_one_untouched() {
        let p = temp_paths("reject_keeps_old");
        // 稼働中の辞書 + ユーザーのデータ
        fs::create_dir_all(p.data_root()).unwrap();
        fs::write(p.data_root().join("old.toml"), b"running").unwrap();
        fs::create_dir_all(p.dict_user_dir()).unwrap();
        fs::write(
            p.dict_user_dir().join("mine.toml"),
            b"[entries]
",
        )
        .unwrap();

        let tb = build_targz(&[Tar::File(
            "core/jukugo/new.toml",
            b"[entries]
",
        )]);
        let mut seen = None;
        let res = extract_validated(&tb, &p, |stage| {
            // 検査には展開した配布物が渡る。 user/ はまだ引き継いでいない
            seen = Some((
                stage.join("jukugo/new.toml").exists(),
                stage.join("user").exists(),
            ));
            Err(anyhow::Error::new(DictRejected("テスト".into())))
        });
        let err = res.unwrap_err();
        assert!(is_rejected(&err), "{err:#}");
        assert_eq!(seen, Some((true, false)));
        // 旧辞書もユーザーのデータも無傷で、 一時ディレクトリも残らない
        assert_eq!(
            fs::read(p.data_root().join("old.toml")).unwrap(),
            b"running"
        );
        assert!(p.dict_user_dir().join("mine.toml").exists());
        assert!(!p.data_root().join("jukugo/new.toml").exists());
        let parent = p.data_root().parent().unwrap().to_path_buf();
        let leftovers: Vec<_> = fs::read_dir(&parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(".data-new") || n.starts_with(".data-old"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        fs::remove_dir_all(&p.data_dir).ok();
    }

    #[test]
    fn accepted_dictionary_is_swapped_in() {
        let p = temp_paths("validate_swap");
        fs::create_dir_all(p.data_root()).unwrap();
        fs::write(p.data_root().join("old.toml"), b"running").unwrap();
        let tb = build_targz(&[Tar::File(
            "core/jukugo/new.toml",
            "[meta]
schema_version = \"2\"
role = \"jukugo\"

[entries]
\"灰桜\" = \"ハイザクラ\"
"
            .as_bytes(),
        )]);
        extract_validated(&tb, &p, |stage| validate_staged(stage, 1)).unwrap();
        assert!(p.data_root().join("jukugo/new.toml").exists());
        assert!(!p.data_root().join("old.toml").exists());
        fs::remove_dir_all(&p.data_dir).ok();
    }
}

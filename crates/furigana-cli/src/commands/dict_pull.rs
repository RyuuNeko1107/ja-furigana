//! `furigana dict pull` の実装
//!
//! GitHub Releases から furigana-dict の tarball を取得して `<data_dir>/data/`
//! 1 階層に flat 展開する (`extract_to` 参照)。
//!
//! 流れ:
//! 1. version 解決 (`--version` 指定 or GitHub API で latest)
//! 2. tarball + sha256 sidecar を download
//! 3. SHA-256 検証
//! 4. `<data_dir>/data/` 配下の旧配布ファイルを削除 (`user/`, `overrides.toml` は保持)
//! 5. tarball を展開: `core/X` / `rules/X` を `data/X` に prefix 剥がして flat 配置
//!
//! tarball の中身は furigana-dict repo そのままの 2 階層 (`core/...` `rules/...`)、
//! 配布側で flat 化する。これは「PR 投稿者が見る repo 内構造」と「実際に動く配置」を
//! 別軸で持たせるため (paths.rs の `data_root()` も参照)。

use crate::paths::Paths;
use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
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

pub fn run(paths: &Paths, version: Option<&str>) -> Result<()> {
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
        Ok(text) => parse_sha256_sidecar(&text)
            .with_context(|| format!("sha256 sidecar の parse 失敗: {sha_url}"))?,
        Err(e) => {
            // sidecar が無い古い release もあり得るので warn にとどめる
            tracing::warn!("SHA-256 sidecar 取得失敗: {e}. 検証をスキップします");
            String::new()
        }
    };
    if !expected_hex.is_empty() {
        let actual_hex = sha256_hex(&tarball);
        if !actual_hex.eq_ignore_ascii_case(&expected_hex) {
            bail!("SHA-256 mismatch:\n  expected: {expected_hex}\n  actual:   {actual_hex}");
        }
        println!("SHA-256 検証 OK");
    }

    println!("展開中...");
    extract_to(&tarball, paths).context("tar.gz 展開失敗")?;

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
        let body = resp.text().unwrap_or_default();
        bail!(
            "GitHub API {status}: {url}\n  body: {}",
            body.chars().take(300).collect::<String>()
        );
    }
    let release: Release = resp.json()?;
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
    let bytes = resp.bytes()?.to_vec();
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        bail!(
            "download size {} bytes が上限 {MAX_DOWNLOAD_BYTES} bytes を超過 (post-check): {url}",
            bytes.len()
        );
    }
    Ok(bytes)
}

fn download_text(client: &reqwest::blocking::Client, url: &str) -> Result<String> {
    let resp = client.get(url).send()?;
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {status}: {url}");
    }
    Ok(resp.text()?)
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
/// 既存の `data_root/` 配下にあった分は **`user/` と `overrides.toml` を残して**
/// 削除してから展開する。これにより古い配布ファイルが残らない一方、ユーザー
/// 追加分は保持される。
fn extract_to(tarball: &[u8], paths: &Paths) -> Result<()> {
    let data_root: PathBuf = paths.data_root();
    let user_dir: PathBuf = paths.dict_user_dir();
    let overrides: PathBuf = paths.overrides_file();

    // 既存の配布ファイルを掃除 (user / overrides は保持)
    if data_root.exists() {
        for entry in fs::read_dir(&data_root)? {
            let path = entry?.path();
            if path == user_dir || path == overrides {
                continue;
            }
            if path.is_dir() {
                fs::remove_dir_all(&path)
                    .with_context(|| format!("既存削除失敗: {}", path.display()))?;
            } else {
                fs::remove_file(&path)
                    .with_context(|| format!("既存削除失敗: {}", path.display()))?;
            }
        }
    } else {
        fs::create_dir_all(&data_root)?;
    }

    let gz = GzDecoder::new(tarball);
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(false);
    archive.set_overwrite(true);

    let canonical_root = data_root
        .canonicalize()
        .unwrap_or_else(|_| data_root.clone());

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

        // entry サイズ上限 (1 file)
        let entry_size = entry.header().size().unwrap_or(0);
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
        // archive 内 `core/...` `rules/...` の prefix を剥がして `data/` 直下に。
        // prefix だけのディレクトリエントリ (`core/` `rules/`) は rest が空に
        // なるので skip (data_root はすでに作ってある)。
        let dest = if let Ok(rest) = entry_path.strip_prefix("core") {
            if rest.as_os_str().is_empty() {
                continue;
            }
            data_root.join(rest)
        } else if let Ok(rest) = entry_path.strip_prefix("rules") {
            if rest.as_os_str().is_empty() {
                continue;
            }
            data_root.join(rest)
        } else {
            // 想定外の top-level entry は無視 (README 等が混入してもスキップ)
            tracing::debug!("skip archive entry: {}", entry_path.display());
            continue;
        };
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        // path traversal 防御: dest の親が data_root 配下に収まることを確認
        let canonical_parent = dest
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .unwrap_or_else(|| dest.clone());
        if !canonical_parent.starts_with(&canonical_root) {
            bail!(
                "path traversal を検出: {} は {} の外",
                entry_path.display(),
                data_root.display()
            );
        }
        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&dest)?;
        } else {
            entry.unpack(&dest)?;
        }
    }
    Ok(())
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
        assert!(root.join("jukugo/x.toml").exists(), "core/ prefix が剥がれて flat 配置");
        assert!(root.join("days.toml").exists(), "rules/ prefix が剥がれて flat 配置");
        assert!(!root.join("README.md").exists(), "未知 top-level entry は skip");
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
        assert!(escaped.is_none(), "data_root 外に evil.toml が漏れた: {escaped:?}");
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
        assert!(!p.data_root().join("huge.toml").exists(), "bail 前に unpack しない");
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
        assert!(p.data_root().join("jukugo/new.toml").exists(), "新ファイルは展開");
        assert!(!p.data_root().join("old.toml").exists(), "旧配布ファイルは掃除");
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
        assert_eq!(fs::read_dir(p.data_root()).unwrap().count(), 0);
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
        assert_eq!(fs::read_dir(p.data_root()).unwrap().count(), 0);
        fs::remove_dir_all(&p.data_dir).ok();
    }
}

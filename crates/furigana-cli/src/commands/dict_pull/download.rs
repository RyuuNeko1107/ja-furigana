//! tarball と sha256 sidecar の取得・検証 (HTTP、 サイズ上限、 SHA-256、 tag 形式)。

use super::{MAX_DOWNLOAD_BYTES, MAX_SIDECAR_BYTES};
use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::Read;

pub(super) fn download_bytes(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>> {
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
pub(super) fn error_body_head(reader: impl Read) -> String {
    let mut buf = Vec::new();
    let _ = reader.take(2048).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).chars().take(300).collect()
}

/// `reader` を最大 `limit` bytes まで読む。 超えたら (全部は読まずに) 中止。
pub(super) fn read_limited(reader: impl Read, limit: usize, what: &str) -> Result<Vec<u8>> {
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

pub(super) fn download_text(client: &reqwest::blocking::Client, url: &str) -> Result<String> {
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
pub(super) fn verify_sha256(
    tarball: &[u8],
    expected: Option<&str>,
    allow_unverified: bool,
) -> Result<bool> {
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
pub(super) fn parse_sha256_sidecar(text: &str) -> Result<String> {
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
pub(super) fn validate_tag_format(tag: &str) -> Result<()> {
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

pub(super) fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

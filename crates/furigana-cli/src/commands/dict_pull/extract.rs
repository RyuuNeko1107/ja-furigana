//! 展開 (flat 化・上限・パス検査)、 新辞書の検証、 `<data_dir>/data/` との入れ替え、 排他 lock。

use super::{DictRejected, MAX_ENTRY_COUNT, MAX_PER_ENTRY_BYTES, MAX_UNCOMPRESSED_TOTAL};
use crate::paths::Paths;
use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// テスト用の薄い入口: 検査なしで展開する (入れ替えの挙動の検証用)。
#[cfg(test)]
pub(super) fn extract_to(tarball: &[u8], paths: &Paths) -> Result<()> {
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
pub(super) fn extract_validated(
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
pub(super) fn validate_staged(stage: &Path, min_entries: usize) -> Result<()> {
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
pub(super) fn extract_into(
    tarball: &[u8],
    stage: &Path,
    user_rel: &Path,
    overrides_rel: &Path,
) -> Result<()> {
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
pub(super) fn is_plain_relative(rest: &Path) -> bool {
    rest.components().all(|c| match c {
        Component::Normal(name) => !name.to_string_lossy().contains(':'),
        _ => false,
    })
}

/// `parent` の下に、 予測しにくい名前の新しい空ディレクトリを作る (`<prefix>-<pid>-<nanos>`)。
pub(super) fn fresh_sibling(parent: &Path, prefix: &str) -> Result<PathBuf> {
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
pub(super) fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
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
pub(super) struct PullLock {
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

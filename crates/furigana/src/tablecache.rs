//! アクセント表・連結表のバイナリキャッシュ。
//!
//! 表は TOML で約 20MB あり、 起動のたびに行を読んでハッシュの配列に畳むと 約 250 ms かかる。
//! 畳んだ結果 (約 5MB) をバイナリで保存しておき、 次回からはそれを読む (約 10 ms)。
//!
//! ## 古いキャッシュを使わないための検証
//!
//! キャッシュの先頭に、 元にした TOML ごとの (相対パス, 大きさ, 更新時刻, 内容のハッシュ) を記録する。
//! 読み込み時、 各ファイルの大きさと更新時刻が記録と同じならそのまま使う (速い判定)。
//! 違うときは内容のハッシュを計算して、 記録と同じなら使う (tar 展開などで更新時刻だけ変わった配布版)。
//! 内容が違えば作り直して上書きする。 含めるファイルの集合 (`--exclude-dict` で変わる) と キャッシュ形式の版は
//! ファイル名の key に入れるので、 設定ごとに別のキャッシュになる (行き来で作り直しを繰り返さない)。
//!
//! 書き込みは一時ファイルに書いてからリネームする (同時起動でも壊れない)。 書けない場所では黙って諦める。
//! 環境変数 `FURIGANA_NO_TABLE_CACHE=1` でキャッシュを使わない。

use crate::chain_phrases::ChainTable;
use crate::scoring::accent_lexicon::AccentLexicon;
use std::io::Read;
use std::path::{Path, PathBuf};

/// キャッシュ形式の版。 形式を変えたら上げる (古いキャッシュは key が変わって使われなくなる)
const FORMAT_VERSION: u32 = 1;
const MAGIC: &[u8; 4] = b"FGTB";
const CACHE_DIR: &str = ".furigana-cache";

/// キャッシュの元になる TOML 1 ファイル
#[derive(Debug, Clone)]
struct Source {
    /// 何番目の辞書ディレクトリか + そこからの相対パス (`/` 区切り)
    rel: String,
    path: PathBuf,
    size: u64,
    mtime_ns: u64,
}

// ─── バイナリの読み書き (小さな補助) ─────────────────────────────────────────────

pub(crate) struct Writer(pub(crate) Vec<u8>);

impl Writer {
    pub(crate) fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub(crate) fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn str(&mut self, s: &str) {
        self.u32(u32::try_from(s.len()).unwrap_or(u32::MAX));
        self.0.extend_from_slice(s.as_bytes());
    }
}

pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    pub(crate) fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    pub(crate) fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub(crate) fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }
    pub(crate) fn str(&mut self) -> Option<&'a str> {
        let n = self.u32()? as usize;
        std::str::from_utf8(self.take(n)?).ok()
    }
    /// 件数つきの配列の長さ (壊れたキャッシュで巨大な確保をしないよう、 残りバイト数で頭打ちにする)
    pub(crate) fn len(&mut self, min_item_bytes: usize) -> Option<usize> {
        let n = self.u64()? as usize;
        (n.checked_mul(min_item_bytes)? <= self.buf.len().saturating_sub(self.pos)).then_some(n)
    }
}

// ─── 元ファイルの列挙 ───────────────────────────────────────────────────────────

/// ファイル先頭 (`[meta]`) の `key = "..."` を読む (全体を読まない)
fn head_value(head: &str, key: &str) -> Option<String> {
    for line in head.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix(key) {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim_start().strip_prefix('"')?;
                return rest.split('"').next().map(str::to_string);
            }
        }
    }
    None
}

/// このファイルの役割が 表 (アクセント表 / 連結表) で、 かつ この lib が読める形式の版か。
///
/// `[meta] format = "<役割>/<版>"` があれば、 対応する版 ([`SUPPORTED_FORMATS`]) だけ読む。 知らない版は黙って無視する
/// (将来の辞書が形式を変えても、 古い lib は壊れずに その file を使わないだけ)。 `format` が無い file は 従来どおり role で読む。
pub(crate) fn is_supported_table(content: &str) -> bool {
    // [meta] は先頭にあるので、 先頭の約 1KB (char 境界に切る) だけ見る
    let head = &content[..content
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= 1024)
        .last()
        .unwrap_or(0)];
    let Some(role) = head_value(head, "role") else {
        return false;
    };
    if !matches!(role.as_str(), "accent" | "accent_chain") {
        return false;
    }
    match head_value(head, "format") {
        None => true,
        Some(f) => SUPPORTED_FORMATS.contains(&f.as_str()),
    }
}

/// この lib が読める表の形式 (`<役割>/<版>`)。 形式を変えたら 版を上げ、 古い版は残すか 切り捨てるかを選ぶ
pub(crate) const SUPPORTED_FORMATS: &[&str] = &["accent/1", "accent_chain/1"];

/// ファイル先頭の 1KB
fn head_of(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 1024];
    let n = f.read(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf[..n]).into_owned())
}

fn mtime_ns(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

fn collect_sources(dirs: &[PathBuf], excludes: &[String]) -> Vec<Source> {
    let mut out = Vec::new();
    for (i, dir) in dirs.iter().enumerate() {
        let Ok(files) = crate::loader::walk_toml_files(dir) else {
            continue;
        };
        for path in files {
            if crate::loader::is_excluded(dir, &path, excludes) {
                continue;
            }
            if !head_of(&path).is_some_and(|h| is_supported_table(&h)) {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let rel = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push(Source {
                rel: format!("{i}:{rel}"),
                size: meta.len(),
                mtime_ns: mtime_ns(&meta),
                path,
            });
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

/// ファイル内容のハッシュ (FNV-1a 64、 64KB ずつ)
fn content_hash(path: &Path) -> Option<u64> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            return Some(h);
        }
        for &b in &buf[..n] {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// キャッシュファイル名の key (形式の版 + 含めるファイルの集合)
fn cache_key(sources: &[Source]) -> u64 {
    let mut parts: Vec<String> = vec![FORMAT_VERSION.to_string()];
    parts.extend(sources.iter().map(|s| s.rel.clone()));
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    crate::keyhash::hash_parts(&refs)
}

fn cache_path(dir: &Path, key: u64) -> PathBuf {
    dir.join(CACHE_DIR).join(format!("tables-{key:016x}.fgtb"))
}

// ─── 読み込み / 書き出し ─────────────────────────────────────────────────────────

/// キャッシュのバイト列を 元ファイルと照らして読む。 古い / 壊れていれば `None`
fn try_load(bytes: &[u8], sources: &[Source]) -> Option<(AccentLexicon, ChainTable)> {
    let (body, tail) = bytes.split_at(bytes.len().checked_sub(8)?);
    if body.get(..4)? != MAGIC {
        return None;
    }
    let want = u64::from_le_bytes(tail.try_into().ok()?);
    if fast_checksum(body) != want {
        return None;
    }
    let mut r = Reader::new(&body[4..]);
    if r.u32()? != FORMAT_VERSION {
        return None;
    }
    // 元ファイルとの突き合わせ
    let n = r.len(8)?;
    if n != sources.len() {
        return None;
    }
    for s in sources {
        let rel = r.str()?;
        let (size, mtime, hash) = (r.u64()?, r.u64()?, r.u64()?);
        if rel != s.rel || size != s.size {
            return None;
        }
        if mtime != s.mtime_ns && content_hash(&s.path)? != hash {
            return None;
        }
    }
    let lex = AccentLexicon::read_from(&mut r)?;
    let chain = ChainTable::read_from(&mut r)?;
    Some((lex, chain))
}

/// 本体のバイト列に対する検査値 (全バイトを 8 byte ずつ混ぜる。 暗号用途ではない。 破損の検出のみ)
fn fast_checksum(body: &[u8]) -> u64 {
    let mut h: u64 = 0x9e37_79b9_7f4a_7c15 ^ (body.len() as u64);
    for chunk in body.chunks(8) {
        let mut b = [0u8; 8];
        b[..chunk.len()].copy_from_slice(chunk);
        h = (h ^ u64::from_le_bytes(b))
            .wrapping_mul(0x0000_0100_0000_01b3)
            .rotate_left(23);
    }
    h
}

fn build_bytes(sources: &[Source], lex: &AccentLexicon, chain: &ChainTable) -> Option<Vec<u8>> {
    let mut w = Writer(Vec::with_capacity(8 * 1024 * 1024));
    w.0.extend_from_slice(MAGIC);
    w.u32(FORMAT_VERSION);
    w.u64(sources.len() as u64);
    for s in sources {
        w.str(&s.rel);
        w.u64(s.size);
        w.u64(s.mtime_ns);
        w.u64(content_hash(&s.path)?);
    }
    lex.write_to(&mut w);
    chain.write_to(&mut w);
    let sum = fast_checksum(&w.0);
    w.u64(sum);
    Some(w.0)
}

/// 一時ファイルに書いてリネーム。 どの辞書ディレクトリにも書けなければ何もしない
fn write_cache(dirs: &[PathBuf], key: u64, bytes: &[u8]) {
    for dir in dirs {
        let target = cache_path(dir, key);
        let Some(parent) = target.parent() else {
            continue;
        };
        if std::fs::create_dir_all(parent).is_err() {
            continue;
        }
        let tmp = parent.join(format!(".tables-{key:016x}.{}.tmp", std::process::id()));
        if std::fs::write(&tmp, bytes).is_err() {
            let _ = std::fs::remove_file(&tmp);
            continue;
        }
        if std::fs::rename(&tmp, &target).is_ok() {
            return;
        }
        let _ = std::fs::remove_file(&tmp);
    }
}

/// アクセント表と連結表を読む。 有効なキャッシュがあればそれを使い、 無ければ TOML から作ってキャッシュを書く。
pub(crate) fn load_tables(
    dirs: &[PathBuf],
    excludes: &[String],
) -> crate::error::Result<(AccentLexicon, ChainTable)> {
    let use_cache = cache_enabled(std::env::var("FURIGANA_NO_TABLE_CACHE").ok().as_deref());
    load_tables_with(dirs, excludes, use_cache)
}

/// 環境変数 `FURIGANA_NO_TABLE_CACHE` の値からキャッシュを使うか決める
fn cache_enabled(var: Option<&str>) -> bool {
    var != Some("1")
}

fn load_tables_with(
    dirs: &[PathBuf],
    excludes: &[String],
    use_cache: bool,
) -> crate::error::Result<(AccentLexicon, ChainTable)> {
    let sources = collect_sources(dirs, excludes);
    if use_cache && !sources.is_empty() {
        let key = cache_key(&sources);
        for dir in dirs {
            if let Ok(bytes) = std::fs::read(cache_path(dir, key)) {
                if let Some(t) = try_load(&bytes, &sources) {
                    return Ok(t);
                }
            }
        }
    }
    let mut lex = AccentLexicon::default();
    let mut chain = ChainTable::default();
    for d in dirs {
        lex.load_dir(d, excludes)?;
        chain.load_dir(d, excludes)?;
    }
    if use_cache && !sources.is_empty() {
        if let Some(bytes) = build_bytes(&sources, &lex, &chain) {
            write_cache(dirs, cache_key(&sources), &bytes);
        }
    }
    Ok((lex, chain))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    const ACCENT: &str = "[meta]\nschema_version = \"2\"\nrole = \"accent\"\n\n[entries]\n\"天気\" = \"[テ]ンキ\"\n\"雨\" = \"[ア]メ\"\n";
    const CHAIN: &str = "[meta]\nschema_version = \"2\"\nrole = \"accent_chain\"\n\n[defaults]\n\"名詞,一般\" = \"C1\"\n\n[functional]\n\"が,ガ,助詞,格助詞\" = \"0/1|名詞%F1\"\n\n[words]\n\"猫,ネコ,名詞\" = \"1/2\"\n";

    fn tmpdir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("furigana_tablecache_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn cache_files(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir.join(CACHE_DIR))
            .map(|r| r.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn writes_cache_then_reuses_it_with_same_lookups() {
        let d = tmpdir("reuse");
        write(&d, "accent/a.toml", ACCENT);
        write(&d, "accent/c.toml", CHAIN);
        let dirs = vec![d.clone()];
        let (lex1, ch1) = load_tables(&dirs, &[]).unwrap();
        assert_eq!(cache_files(&d).len(), 1, "初回に キャッシュが書かれる");
        let (lex2, ch2) = load_tables(&dirs, &[]).unwrap();
        for (s, r) in [("天気", "テンキ"), ("雨", "アメ"), ("犬", "イヌ")] {
            assert_eq!(lex1.lookup(s, r), lex2.lookup(s, r), "{s}");
        }
        assert_eq!(lex2.lookup("天気", "テンキ"), Some((1, 3)));
        assert!(ch2.is_empty() == ch1.is_empty() && !ch2.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn edit_with_same_size_and_mtime_change_is_detected_by_content_hash() {
        let d = tmpdir("stale");
        write(&d, "accent/a.toml", ACCENT);
        let dirs = vec![d.clone()];
        let _ = load_tables(&dirs, &[]).unwrap();
        // 同じ大きさで 核だけ書き換える ([テ]ンキ → [テン]キ)。 更新時刻も変わる
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&d, "accent/a.toml", &ACCENT.replace("[テ]ンキ", "[テン]キ"));
        let (lex, _) = load_tables(&dirs, &[]).unwrap();
        assert_eq!(
            lex.lookup("天気", "テンキ"),
            Some((2, 3)),
            "古いキャッシュを使わない"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupted_cache_is_ignored_and_rebuilt() {
        let d = tmpdir("corrupt");
        write(&d, "accent/a.toml", ACCENT);
        let dirs = vec![d.clone()];
        let _ = load_tables(&dirs, &[]).unwrap();
        let f = cache_files(&d).remove(0);
        let mut bytes = std::fs::read(&f).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xff;
        std::fs::write(&f, bytes).unwrap();
        let (lex, _) = load_tables(&dirs, &[]).unwrap();
        assert_eq!(lex.lookup("雨", "アメ"), Some((1, 2)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn excluded_files_get_a_separate_cache() {
        let d = tmpdir("exclude");
        write(&d, "accent/a.toml", ACCENT);
        write(
            &d,
            "accent/b.toml",
            &ACCENT
                .replace("天気", "空気")
                .replace("[テ]ンキ", "[ク]ウキ"),
        );
        let dirs = vec![d.clone()];
        let (all, _) = load_tables(&dirs, &[]).unwrap();
        let (part, _) = load_tables(&dirs, &["accent/b.toml".to_string()]).unwrap();
        assert!(all.lookup("空気", "クウキ").is_some());
        assert!(
            part.lookup("空気", "クウキ").is_none(),
            "除外したファイルの内容がキャッシュ経由で漏れない"
        );
        assert_eq!(cache_files(&d).len(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unknown_format_version_is_ignored_but_missing_format_is_read() {
        let d = tmpdir("format");
        // format 無し (従来の file) は読む
        write(&d, "accent/a.toml", ACCENT);
        // 未来の版は無視 (同じ表記を別の核にしておく)
        write(
            &d,
            "accent/future.toml",
            "[meta]
schema_version = \"2\"
role = \"accent\"
format = \"accent/99\"

[entries]
\"空気\" = \"[クウ]キ\"
",
        );
        // 対応する版は読む
        write(
            &d,
            "accent/ok.toml",
            "[meta]
schema_version = \"2\"
role = \"accent\"
format = \"accent/1\"

[entries]
\"風\" = \"[カ]ゼ\"
",
        );
        let (lex, _) = load_tables(std::slice::from_ref(&d), &[]).unwrap();
        assert!(lex.lookup("天気", "テンキ").is_some(), "format 無しは読む");
        assert!(lex.lookup("風", "カゼ").is_some(), "対応する版は読む");
        assert!(lex.lookup("空気", "クウキ").is_none(), "未知の版は無視する");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unwritable_cache_dir_is_ignored() {
        let d = tmpdir("ro");
        write(&d, "accent/a.toml", ACCENT);
        // .furigana-cache をファイルとして置いて ディレクトリを作れないようにする
        std::fs::write(d.join(CACHE_DIR), b"x").unwrap();
        let (lex, _) = load_tables(std::slice::from_ref(&d), &[]).unwrap();
        assert_eq!(lex.lookup("天気", "テンキ"), Some((1, 3)));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 検証用: 辞書を置き、 元ファイルの一覧と キャッシュのバイト列を作る
    fn built(name: &str) -> (PathBuf, Vec<Source>, Vec<u8>) {
        let d = tmpdir(name);
        write(&d, "accent/a.toml", ACCENT);
        write(&d, "accent/c.toml", CHAIN);
        let sources = collect_sources(std::slice::from_ref(&d), &[]);
        assert_eq!(sources.len(), 2);
        let mut lex = AccentLexicon::default();
        let mut chain = ChainTable::default();
        lex.load_dir(&d, &[]).unwrap();
        chain.load_dir(&d, &[]).unwrap();
        let bytes = build_bytes(&sources, &lex, &chain).unwrap();
        (d, sources, bytes)
    }

    /// 本体を書き換えたあと 検査値を付け直す (検査値以外の検証を通すため)
    fn reseal(mut bytes: Vec<u8>) -> Vec<u8> {
        let n = bytes.len() - 8;
        bytes.truncate(n);
        let sum = fast_checksum(&bytes);
        bytes.extend_from_slice(&sum.to_le_bytes());
        bytes
    }

    #[test]
    fn hashes_match_known_values() {
        assert_eq!(fast_checksum(b""), 0x9e37_79b9_7f4a_7c15);
        assert_eq!(fast_checksum(b"abcdefgh"), 0x8113_6f66_5a59_a24f);
        assert_eq!(fast_checksum(b"abcdefghi"), 0xae8d_6753_6dd6_d442);
        let d = tmpdir("hash");
        write(&d, "a", "a");
        write(&d, "empty", "");
        assert_eq!(content_hash(&d.join("a")), Some(0xaf63_dc4c_8601_ec8c));
        assert_eq!(content_hash(&d.join("empty")), Some(0xcbf2_9ce4_8422_2325));
        assert_eq!(content_hash(&d.join("none")), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn try_load_accepts_its_own_bytes_with_full_tables() {
        let (d, sources, bytes) = built("roundtrip");
        let (lex, chain) = try_load(&bytes, &sources).unwrap();
        assert_eq!(lex.lookup("天気", "テンキ"), Some((1, 3)));
        assert_eq!(lex.lookup("雨", "アメ"), Some((1, 2)));
        assert!(!chain.is_empty());
        assert!(bytes.len() > 100, "表の中身が入っている");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn try_load_rejects_bad_headers() {
        let (d, sources, bytes) = built("headers");
        // 短すぎる / 空
        assert!(try_load(&[], &sources).is_none());
        assert!(try_load(&bytes[..7], &sources).is_none());
        assert!(try_load(&bytes[..11], &sources).is_none());
        // 検査値だけ違う
        let mut b = bytes.clone();
        *b.last_mut().unwrap() ^= 1;
        assert!(try_load(&b, &sources).is_none());
        // 検査値は合っているが magic が違う
        let mut b = bytes.clone();
        b[0] = b'X';
        assert!(try_load(&reseal(b), &sources).is_none());
        // 形式の版が違う
        let mut b = bytes.clone();
        b[4..8].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert!(try_load(&reseal(b), &sources).is_none());
        // 元ファイルの数が違う
        assert!(try_load(&bytes, &sources[..1]).is_none());
        let mut more = sources.clone();
        more.push(sources[0].clone());
        assert!(try_load(&bytes, &more).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn try_load_checks_each_source() {
        let (d, sources, bytes) = built("sources");
        // 名前だけ違う / 大きさだけ違う
        let mut s = sources.clone();
        s[1].rel.push('x');
        assert!(try_load(&bytes, &s).is_none());
        let mut s = sources.clone();
        s[1].size += 1;
        assert!(try_load(&bytes, &s).is_none());
        // 更新時刻だけ違う (内容は同じ) → 使える
        let mut s = sources.clone();
        for x in &mut s {
            x.mtime_ns += 1;
        }
        assert!(try_load(&bytes, &s).is_some());
        // 更新時刻も内容も違う (大きさは同じ) → 使わない
        write(&d, "accent/a.toml", &ACCENT.replace("[テ]ンキ", "[テン]キ"));
        let mut s = collect_sources(std::slice::from_ref(&d), &[]);
        assert_eq!(s[0].size, sources[0].size);
        s[0].mtime_ns += 1;
        assert!(try_load(&bytes, &s).is_none());
        // 内容が違っても 大きさと更新時刻が同じなら 速い判定で使う
        s[0].mtime_ns = sources[0].mtime_ns;
        assert!(try_load(&bytes, &s).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cache_is_used_only_when_enabled_and_not_for_empty_dirs() {
        let d = tmpdir("enabled");
        write(&d, "accent/a.toml", ACCENT);
        let dirs = vec![d.clone()];
        let (_, _) = load_tables_with(&dirs, &[], true).unwrap();
        assert_eq!(cache_files(&d).len(), 1);
        // 同じ大きさで 内容を変え 更新時刻を戻す → キャッシュが使われていれば 古い値のまま
        let p = d.join("accent/a.toml");
        let old = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::fs::write(&p, ACCENT.replace("[テ]ンキ", "[テン]キ")).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let (lex, _) = load_tables_with(&dirs, &[], true).unwrap();
        assert_eq!(
            lex.lookup("天気", "テンキ"),
            Some((1, 3)),
            "キャッシュを使う"
        );
        let (lex, _) = load_tables_with(&dirs, &[], false).unwrap();
        assert_eq!(
            lex.lookup("天気", "テンキ"),
            Some((2, 3)),
            "無効なら TOML を読む"
        );
        let _ = std::fs::remove_dir_all(&d);

        // 無効のときは 書かない
        let d = tmpdir("disabled");
        write(&d, "accent/a.toml", ACCENT);
        let _ = load_tables_with(std::slice::from_ref(&d), &[], false).unwrap();
        assert!(cache_files(&d).is_empty());
        let _ = std::fs::remove_dir_all(&d);

        // 表が 1 つも無い辞書には 書かない
        let d = tmpdir("none");
        let _ = load_tables_with(std::slice::from_ref(&d), &[], true).unwrap();
        assert!(cache_files(&d).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cache_enabled_only_when_not_disabled() {
        assert!(cache_enabled(None));
        assert!(cache_enabled(Some("0")));
        assert!(!cache_enabled(Some("1")));
    }
}

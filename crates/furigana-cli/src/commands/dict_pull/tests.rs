use super::*;
use std::fs;

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

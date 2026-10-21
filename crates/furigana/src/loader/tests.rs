use super::walk::head_meta;
use super::walk::read_head;
use super::walk::HEAD_BYTES;
use super::*;
use crate::rules::CountersData;
use std::path::PathBuf;

// ─── for_each_toml_in_dir_roles (役割で絞る walker) ───────────────────

fn roles_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("loader_roles_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn put(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn collect_roles(dir: &Path, wanted: &str) -> Result<Vec<(String, usize)>> {
    let mut seen = Vec::new();
    for_each_toml_in_dir_roles(
        dir,
        &[],
        |role| role == Some(wanted),
        |content, from, _role| {
            seen.push((
                Path::new(from)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                content.len(),
            ));
            Ok(())
        },
    )?;
    Ok(seen)
}

#[test]
fn roles_walker_visits_only_wanted_roles() {
    let d = roles_dir("wanted");
    put(
        &d,
        "a.toml",
        "[meta]\nschema_version = \"2\"\nrole = \"compat\"\n[map]\n\"髙\" = \"高\"\n",
    );
    put(
        &d,
        "b.toml",
        "[meta]\nschema_version = \"2\"\nrole = \"accent\"\n[entries]\n\"雨\" = \"[ア]メ\"\n",
    );
    let got = collect_roles(&d, "compat").unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, "a.toml");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn roles_walker_still_rejects_bad_schema_in_skipped_files() {
    let d = roles_dir("badschema");
    put(
        &d,
        "a.toml",
        "[meta]\nschema_version = \"2\"\nrole = \"compat\"\n",
    );
    put(
        &d,
        "b.toml",
        "[meta]\nschema_version = \"1\"\nrole = \"accent\"\n",
    );
    let err = collect_roles(&d, "compat").unwrap_err();
    assert!(matches!(err, FuriganaError::Validation(_)), "{err:?}");
    // schema_version が無い file も同じ
    put(&d, "b.toml", "[meta]\nrole = \"accent\"\n");
    assert!(collect_roles(&d, "compat").is_err());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn roles_walker_reads_large_wanted_files_in_full() {
    let d = roles_dir("large");
    let mut body = String::from("[meta]\nschema_version = \"2\"\nrole = \"accent\"\n[entries]\n");
    for i in 0..5000 {
        body.push_str(&format!("\"語{i}\" = \"[カ]ゴ\"\n"));
    }
    assert!(body.len() > HEAD_BYTES * 2);
    put(&d, "big.toml", &body);
    let got = collect_roles(&d, "accent").unwrap();
    assert_eq!(got, vec![("big.toml".to_string(), body.len())]);
    assert!(collect_roles(&d, "compat").unwrap().is_empty());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn roles_walker_falls_back_when_meta_is_not_at_the_top() {
    let d = roles_dir("late_meta");
    // [meta] が先頭 HEAD_BYTES に収まらない file は、 全体を読んで従来どおり判定する
    let mut body = "# ".to_string();
    body.push_str(&"あ".repeat(HEAD_BYTES));
    body.push_str("\n[meta]\nschema_version = \"2\"\nrole = \"compat\"\n[map]\n\"髙\" = \"高\"\n");
    put(&d, "late.toml", &body);
    let got = collect_roles(&d, "compat").unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1, body.len());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn roles_walker_infers_role_from_path_when_meta_has_none() {
    let d = roles_dir("infer");
    put(
        &d,
        "jukugo/x.toml",
        "[meta]\nschema_version = \"2\"\n[entries]\n\"猫\" = \"ねこ\"\n",
    );
    assert_eq!(collect_roles(&d, "jukugo").unwrap().len(), 1);
    std::fs::remove_dir_all(&d).ok();
}

// ─── read_head / head_meta (先頭 HEAD_BYTES だけ読む判定) ──────────────

#[test]
fn read_head_reports_completeness_at_the_boundary() {
    assert_eq!(HEAD_BYTES, 16 * 1024);
    let d = roles_dir("readhead");
    // 空 file: 全体が収まった
    put(&d, "empty.toml", "");
    assert_eq!(
        read_head(&d.join("empty.toml")).unwrap(),
        (String::new(), true)
    );
    // 小さい file: 全体を返し complete
    put(&d, "small.toml", "abc\n");
    assert_eq!(
        read_head(&d.join("small.toml")).unwrap(),
        ("abc\n".to_string(), true)
    );
    // HEAD_BYTES - 1: まだ収まっている
    let just_under = "a".repeat(HEAD_BYTES - 1);
    put(&d, "under.toml", &just_under);
    assert_eq!(
        read_head(&d.join("under.toml")).unwrap(),
        (just_under, true)
    );
    // ちょうど HEAD_BYTES: 続きがあるかもしれないので complete ではない
    let exact = "a".repeat(HEAD_BYTES);
    put(&d, "exact.toml", &exact);
    assert_eq!(read_head(&d.join("exact.toml")).unwrap(), (exact, false));
    // 超える: 先頭 HEAD_BYTES だけ
    put(&d, "over.toml", &"a".repeat(HEAD_BYTES + 100));
    let (h, c) = read_head(&d.join("over.toml")).unwrap();
    assert_eq!(h.len(), HEAD_BYTES);
    assert!(!c);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn read_head_trims_a_split_char_but_rejects_bad_utf8_when_complete() {
    let d = roles_dir("readhead_utf8");
    // 「あ」は 3 byte。 16384 = 3 * 5461 + 1 なので境界で char が割れる → 手前まで
    put(&d, "split.toml", &"あ".repeat(6000));
    let (h, c) = read_head(&d.join("split.toml")).unwrap();
    assert!(!c);
    assert_eq!(h.len(), 3 * 5461);
    assert!(h.chars().all(|ch| ch == 'あ'));
    // 全体が収まっているのに UTF-8 として不正なら error
    std::fs::write(d.join("bad.toml"), [b'a', 0xff, b'b']).unwrap();
    assert!(read_head(&d.join("bad.toml")).is_err());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn head_meta_decides_only_when_the_section_is_closed() {
    // 先頭に別の行があっても [meta] を見つける
    let closed = "# c\n[meta]\nschema_version = \"2\"\nrole = \"compat\"\n[map]\n\"a\" = \"b\"\n";
    let t = head_meta(closed, false).expect("次の section があれば先頭で確定する");
    assert_eq!(t.role.as_deref(), Some("compat"));
    let t = head_meta(closed, true).unwrap();
    assert_eq!(t.role.as_deref(), Some("compat"));
    // [meta] が末尾まで続いている: file が途中で切れているなら続きがあるかも → None
    let open = "# c\n[meta]\nschema_version = \"2\"\nrole = \"compat\"\n";
    assert!(head_meta(open, false).is_none());
    // 全体が収まっているなら末尾までが [meta] で確定
    let t = head_meta(open, true).unwrap();
    assert_eq!(t.role.as_deref(), Some("compat"));
    // [meta] が無い / 解析できない
    assert!(head_meta("[entries]\n", true).is_none());
    assert!(head_meta("[meta]\nrole = [\n[x]\n", true).is_none());
}

#[test]
fn parses_counters_toml() {
    let toml_str = r#"
        [counter."本"]
        default = "ホン"

        [[counter."本".rules]]
        last_digit = [1, 6, 8, 0]
        suffix = "ポン"
        sokuonize = true
    "#;
    let data: CountersData = parse_toml(toml_str, "counters.toml").unwrap();
    let hon = data.counter.get("本").unwrap();
    assert_eq!(hon.default.as_deref(), Some("ホン"));
    assert_eq!(hon.rules[0].suffix, "ポン");
}

#[test]
fn invalid_toml_error_includes_file_name() {
    let err = parse_toml::<CountersData>("[invalid", "counters.toml").unwrap_err();
    match err {
        FuriganaError::Toml { file, .. } => assert_eq!(file, "counters.toml"),
        other => panic!("expected Toml error, got {other:?}"),
    }
}

#[test]
fn parses_days_toml_lookup_by_int() {
    let toml_str = r#"
        [entries]
        "1" = "ツイタチ"
        "20" = "ハツカ"
    "#;
    let data: crate::rules::DaysData = parse_toml(toml_str, "days.toml").unwrap();
    assert_eq!(data.get(1), Some("ツイタチ"));
    assert_eq!(data.get(20), Some("ハツカ"));
    assert_eq!(data.get(15), None);
}

#[test]
fn parses_scales_toml_with_array_of_tables() {
    let toml_str = r#"
        [[entry]]
        kanji = "万"
        kana = "マン"
        [[entry]]
        kanji = "億"
        kana = "オク"
    "#;
    let data: crate::rules::ScalesData = parse_toml(toml_str, "scales.toml").unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(data.lookup("億"), Some("オク"));
}

#[test]
fn parses_units_toml_with_inline_tables() {
    let toml_str = r#"
        [entries]
        "km" = { kana = "キロメートル" }
        "L"  = { kana = "リットル", ci = true }
    "#;
    let data: crate::rules::UnitsData = parse_toml(toml_str, "units.toml").unwrap();
    assert_eq!(data.lookup("km"), Some("キロメートル"));
    assert_eq!(data.lookup("l"), Some("リットル"));
}

fn fresh_temp_dir(suffix: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "furigana_loader_{}_{}_{}",
        std::process::id(),
        suffix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn load_rules_dir_errors_when_dir_missing() {
    let nonexistent = std::env::temp_dir().join("furigana_does_not_exist_xyz123");
    std::fs::remove_dir_all(&nonexistent).ok();
    let err = load_rules_dir(&nonexistent).unwrap_err();
    assert!(matches!(err, FuriganaError::Validation(_)));
}

#[test]
fn load_rules_dir_with_no_files_yields_default() {
    let dir = fresh_temp_dir("empty");
    let data = load_rules_dir(&dir).unwrap();
    assert!(data.counters.simple.is_empty());
    assert!(data.scales.is_empty());
    assert!(data.compat.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_rules_dir_loads_and_merges_context_cues() {
    let a = "[meta]
schema_version = \"2\"
role = \"context_cues\"

[slash_pair]
date_topic = [\"配信\"]
";
    let b = "[meta]
schema_version = \"2\"
role = \"context_cues\"

[slash_pair]
fraction_after = [\"くらい\"]
";
    let dir = fresh_temp_dir("cues_ok");
    std::fs::write(dir.join("context_cues.toml"), a).unwrap();
    std::fs::write(dir.join("more.toml"), b).unwrap();
    let data = load_rules_dir(&dir).unwrap();
    assert_eq!(data.context_cues.slash_pair.date_topic, ["配信"]);
    assert_eq!(data.context_cues.slash_pair.fraction_after, ["くらい"]);
    std::fs::remove_dir_all(&dir).ok();

    // 空の語は読み込みで弾く
    let bad = "[meta]
schema_version = \"2\"
role = \"context_cues\"

[slash_pair]
date_topic = [\"\"]
";
    let dir = fresh_temp_dir("cues_bad");
    std::fs::write(dir.join("context_cues.toml"), bad).unwrap();
    assert!(load_rules_dir(&dir).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_rules_dir_infers_models_role_from_file_name() {
    // role を書かない古い形式でも、 ファイル名 models.toml なら型番パターンとして読む
    let src = r#"[meta]
schema_version = "2"

[[model]]
pattern = 'PS([2-5])'
reading = "ピーエス{1:english}"
"#;
    let dir = fresh_temp_dir("models_by_name");
    std::fs::write(dir.join("models.toml"), src).unwrap();
    let data = load_rules_dir(&dir).unwrap();
    assert_eq!(data.models.model.len(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_rules_dir_loads_and_validates_models() {
    let good = r#"[meta]
schema_version = "2"
role = "models"

[parts]
"Ti" = "ティーアイ"

[[model]]
pattern = 'RTX\s?([0-9]{4})'
reading = "アールティーエックス{1:each}"
"#;
    let dir = fresh_temp_dir("models_ok");
    std::fs::write(dir.join("models.toml"), good).unwrap();
    let data = load_rules_dir(&dir).unwrap();
    assert_eq!(data.models.model.len(), 1);
    assert_eq!(data.models.parts["Ti"], "ティーアイ");
    std::fs::remove_dir_all(&dir).ok();

    // 不正な pattern は load で Validation エラー (file 名つき)
    let bad = r#"[meta]
schema_version = "2"
role = "models"

[[model]]
pattern = 'RTX(['
reading = "ア"
"#;
    let dir = fresh_temp_dir("models_bad");
    std::fs::write(dir.join("models.toml"), bad).unwrap();
    let err = load_rules_dir(&dir).unwrap_err();
    assert!(
        matches!(err, FuriganaError::Validation(ref m) if m.contains("models.toml")),
        "{err}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_rules_dir_loads_files_when_present() {
    let dir = fresh_temp_dir("present");
    std::fs::write(
        dir.join(SCALES_FILE),
        "[meta]\nschema_version = \"2\"\n\n[[entry]]\nkanji = \"万\"\nkana = \"マン\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join(COUNTERS_FILE),
        "[meta]\nschema_version = \"2\"\n\n[counter.\"本\"]\ndefault = \"ホン\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join(COMPAT_FILE),
        "[meta]\nschema_version = \"2\"\n\n[map]\n\"髙\" = \"高\"\n",
    )
    .unwrap();

    let data = load_rules_dir(&dir).unwrap();
    assert_eq!(data.scales.lookup("万"), Some("マン"));
    assert_eq!(
        data.counters
            .counter
            .get("本")
            .and_then(|c| c.default.as_deref()),
        Some("ホン")
    );
    assert_eq!(data.compat.lookup("髙"), Some("高"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_rules_dir_propagates_parse_errors() {
    let dir = fresh_temp_dir("parse_err");
    // 壊れた TOML は validate_schema_version が silent pass し、 後段の
    // parse_toml が proper な Toml error を返す (= ★A1b の UX 設計)。
    std::fs::write(dir.join(COUNTERS_FILE), "壊れた").unwrap();
    let err = load_rules_dir(&dir).unwrap_err();
    assert!(matches!(err, FuriganaError::Toml { .. }));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_rules_dir_merges_counters_subdir() {
    let dir = fresh_temp_dir("counters_subdir");
    let sub = dir.join("counters");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(
        sub.join("01_simple.toml"),
        "[meta]\nschema_version = \"2\"\n\n[simple]\n\"円\" = \"エン\"\n",
    )
    .unwrap();
    std::fs::write(
        sub.join("02_objects.toml"),
        "[meta]\nschema_version = \"2\"\n\n[counter.\"本\"]\ndefault = \"ホン\"\n",
    )
    .unwrap();
    std::fs::write(
        sub.join("03_time.toml"),
        "[meta]\nschema_version = \"2\"\n\n[counter.\"時\"]\ndefault = \"ジ\"\n",
    )
    .unwrap();

    let data = load_rules_dir(&dir).unwrap();
    assert_eq!(
        data.counters.simple.get("円").map(String::as_str),
        Some("エン")
    );
    assert_eq!(
        data.counters
            .counter
            .get("本")
            .and_then(|c| c.default.as_deref()),
        Some("ホン")
    );
    assert_eq!(
        data.counters
            .counter
            .get("時")
            .and_then(|c| c.default.as_deref()),
        Some("ジ")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_rules_dir_merges_single_file_and_subdir() {
    // 新 loader (role 駆動 + 再帰 walk) は単一 file と subdir 配下を **両方 merge**
    // する。 旧 loader では「単一 file 優先で subdir は ignore」 だったが、
    // role tag 駆動なら同 role の file が複数 dir に存在しても自然に merge できる。
    let dir = fresh_temp_dir("counters_merge");
    let sub = dir.join("counters");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(
        dir.join(COUNTERS_FILE),
        "[meta]\nschema_version = \"2\"\n\n[counter.\"本\"]\ndefault = \"ホン\"\n",
    )
    .unwrap();
    std::fs::write(
        sub.join("more.toml"),
        "[meta]\nschema_version = \"2\"\n\n[counter.\"匹\"]\ndefault = \"ヒキ\"\n",
    )
    .unwrap();

    let data = load_rules_dir(&dir).unwrap();
    assert!(data.counters.counter.contains_key("本"));
    assert!(data.counters.counter.contains_key("匹"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_rules_dir_role_tag_overrides_filename() {
    // file 名から推定できない場所に置いても [meta] role があれば認識される。
    let dir = fresh_temp_dir("role_tag_override");
    std::fs::create_dir_all(dir.join("custom")).unwrap();
    std::fs::write(
        dir.join("custom").join("anywhere.toml"),
        "[meta]\nschema_version = \"2\"\nrole = \"counters\"\n\n[counter.\"枚\"]\ndefault = \"マイ\"\n",
    )
    .unwrap();

    let data = load_rules_dir(&dir).unwrap();
    assert!(data.counters.counter.contains_key("枚"));
    std::fs::remove_dir_all(&dir).ok();
}

// ─── schema_version validation tests ─────────────────────────────────────

#[test]
fn is_excluded_matches_dir_prefix_and_file_stem() {
    let d = Path::new("core");
    let ex = |pats: &[&str], p: &str| {
        let pats: Vec<String> = pats.iter().map(|s| (*s).to_string()).collect();
        is_excluded(d, &d.join(p), &pats)
    };
    assert!(ex(&["works"], "works/vtuber/hololive.toml"));
    assert!(ex(&["works/vtuber"], "works/vtuber/hololive.toml"));
    assert!(!ex(&["works/vtuber"], "works/anime/gintama.toml"));
    // 前方一致は path 区切り単位 (works2 は works に当たらない)
    assert!(!ex(&["works"], "works2/a.toml"));
    // file 指定 (.toml 省略可 / 前後の / と \ 区切りも可)
    assert!(ex(
        &["jukugo/society/mahjong"],
        "jukugo/society/mahjong.toml"
    ));
    assert!(ex(
        &[r"/jukugo\society\mahjong.toml/"],
        "jukugo/society/mahjong.toml"
    ));
    assert!(!ex(
        &["jukugo/society/mahjong"],
        "jukugo/society/mahjong_extra.toml"
    ));
    assert!(!ex(&[], "works/a.toml"));
    assert!(!ex(&[""], "works/a.toml"));
}

#[test]
fn parse_meta_reads_only_meta_section() {
    // [meta] の後ろが TOML として壊れていても role / schema は読める (= 全体を解析しない)
    let content = "# c
[meta]
schema_version = \"2\"
role = \"accent\"

[entries]
\"a\" = = broken
";
    assert_eq!(parse_meta_role(content).as_deref(), Some("accent"));
    assert_eq!(parse_meta_schema_version(content).as_deref(), Some("2"));
    // [meta] が末尾にあっても読める
    let tail = "[entries]
\"a\" = \"b\"
[meta]
role = \"reading\"
";
    assert_eq!(parse_meta_role(tail).as_deref(), Some("reading"));
}

#[test]
fn parse_meta_schema_version_returns_value_when_present() {
    let content = "[meta]\nschema_version = \"2\"\nrole = \"entries\"\n";
    assert_eq!(parse_meta_schema_version(content).as_deref(), Some("2"));
}

#[test]
fn parse_meta_schema_version_returns_none_when_absent() {
    let content = "[meta]\nrole = \"entries\"\n";
    assert!(parse_meta_schema_version(content).is_none());
}

#[test]
fn parse_meta_schema_version_returns_none_when_no_meta_block() {
    let content = "[entries]\n\"猫\" = \"ネコ\"\n";
    assert!(parse_meta_schema_version(content).is_none());
}

#[test]
fn validate_schema_version_accepts_v2() {
    let content = "[meta]\nschema_version = \"2\"\n";
    assert!(validate_schema_version(content, "test.toml").is_ok());
}

#[test]
fn validate_schema_version_rejects_v1_explicitly() {
    let content = "[meta]\nschema_version = \"1\"\n";
    let err = validate_schema_version(content, "test.toml").unwrap_err();
    match err {
        FuriganaError::Validation(msg) => {
            assert!(msg.contains("not supported"), "msg: {msg}");
            assert!(msg.contains("\"1\""), "msg: {msg}");
            assert!(msg.contains("test.toml"), "msg: {msg}");
            assert!(msg.contains("migrate_v2.py"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validate_schema_version_rejects_missing_field_as_legacy() {
    let content = "[meta]\nrole = \"entries\"\n";
    let err = validate_schema_version(content, "old_dict.toml").unwrap_err();
    match err {
        FuriganaError::Validation(msg) => {
            assert!(msg.contains("legacy format pre-0.1.0"), "msg: {msg}");
            assert!(msg.contains("missing"), "msg: {msg}");
            assert!(msg.contains("old_dict.toml"), "msg: {msg}");
            assert!(msg.contains("migrate_v2.py"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validate_schema_version_rejects_unknown_version() {
    let content = "[meta]\nschema_version = \"99\"\n";
    let err = validate_schema_version(content, "test.toml").unwrap_err();
    match err {
        FuriganaError::Validation(msg) => {
            assert!(msg.contains("not supported"), "msg: {msg}");
            assert!(msg.contains("\"99\""), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validate_schema_version_rejects_no_meta_block() {
    let content = "[entries]\n\"猫\" = \"ネコ\"\n";
    let err = validate_schema_version(content, "no_meta.toml").unwrap_err();
    match err {
        FuriganaError::Validation(msg) => {
            assert!(msg.contains("legacy format"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

// ─── A1b: caller-side schema_version 強制 tests ─────────────────────────

#[test]
fn load_rules_dir_rejects_legacy_file_without_meta() {
    // alpha.10〜 lib は schema_version = "2" を必須化 (★A1b)。
    // [meta] 不在 = 旧 alpha era format → Validation error で reject。
    let dir = fresh_temp_dir("a1b_legacy");
    std::fs::write(
        dir.join(SCALES_FILE),
        "[[entry]]\nkanji = \"万\"\nkana = \"マン\"\n",
    )
    .unwrap();
    let err = load_rules_dir(&dir).unwrap_err();
    match err {
        FuriganaError::Validation(msg) => {
            assert!(
                msg.contains("schema_version"),
                "expected schema_version error: {msg}"
            );
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_rules_dir_rejects_v1_format() {
    let dir = fresh_temp_dir("a1b_v1");
    std::fs::write(
        dir.join(SCALES_FILE),
        "[meta]\nschema_version = \"1\"\n\n[[entry]]\nkanji = \"万\"\nkana = \"マン\"\n",
    )
    .unwrap();
    let err = load_rules_dir(&dir).unwrap_err();
    assert!(matches!(err, FuriganaError::Validation(_)));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn supported_schema_versions_contains_v2() {
    assert!(SUPPORTED_SCHEMA_VERSIONS.contains(&"2"));
}

#[test]
fn load_rules_dir_loads_and_merges_person_name_cues() {
    let a = "[meta]
schema_version = \"2\"
role = \"person_name_cues\"

[cues]
honorifics = [\"さん\"]
titles = [\"選手\"]
";
    let b = "[meta]
schema_version = \"2\"
role = \"person_name_cues\"

[cues]
honorifics = [\"さん\", \"くん\"]
titles = [\"監督\"]
compound_heads = [\"会\"]
";
    let dir = fresh_temp_dir("person_cues_ok");
    // file 名からも role を推定できる
    std::fs::write(dir.join("person_name_cues.toml"), a).unwrap();
    std::fs::write(dir.join("more.toml"), b).unwrap();
    let data = load_rules_dir(&dir).unwrap();
    // 合算された語 (並びは file の読み込み順で、 結果には影響しない)
    let sorted = |v: &Vec<String>| {
        let mut v = v.clone();
        v.sort();
        v
    };
    assert_eq!(
        sorted(&data.person_name_cues.cues.honorifics),
        ["くん", "さん"]
    );
    assert_eq!(sorted(&data.person_name_cues.cues.titles), ["監督", "選手"]);
    assert_eq!(data.person_name_cues.cues.compound_heads, ["会"]);
    std::fs::remove_dir_all(&dir).ok();

    // ファイルが無ければ空 (既定値は持たない)
    let dir = fresh_temp_dir("person_cues_none");
    let data = load_rules_dir(&dir).unwrap();
    assert!(data.person_name_cues.cues.honorifics.is_empty());
    assert!(data.person_name_cues.cues.titles.is_empty());
    std::fs::remove_dir_all(&dir).ok();

    // 書式が壊れていれば読み込みで弾く (空の語)
    let bad = "[meta]
schema_version = \"2\"
role = \"person_name_cues\"

[cues]
titles = [\" \"]
";
    let dir = fresh_temp_dir("person_cues_bad");
    std::fs::write(dir.join("person_name_cues.toml"), bad).unwrap();
    assert!(load_rules_dir(&dir).is_err());
    std::fs::remove_dir_all(&dir).ok();

    // 複数 file に分けて 1 つの表の上限 (1,000 語) を超える書き方も、 合算後に弾く
    let dir = fresh_temp_dir("person_cues_many");
    for n in 0..2 {
        let words: Vec<String> = (0..600).map(|i| format!("\"語{n}_{i}\"")).collect();
        std::fs::write(
            dir.join(format!("part{n}.toml")),
            format!(
                "[meta]
schema_version = \"2\"
role = \"person_name_cues\"

[cues]
titles = [{}]
",
                words.join(", ")
            ),
        )
        .unwrap();
    }
    assert!(load_rules_dir(&dir).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

/// ファイル名から role を推定する対応表 (ディレクトリ構造に依存しない)。 辞書側のファイル名が role に結びつく
#[test]
fn role_is_inferred_from_rule_file_names() {
    use super::meta::infer_role_from_path;
    for (name, role) in [
        ("context_cues.toml", "context_cues"),
        ("person_name_cues.toml", "person_name_cues"),
        ("postprocess.toml", "postprocess"),
        ("models.toml", "models"),
    ] {
        assert_eq!(
            infer_role_from_path(&PathBuf::from("rules").join("text").join(name)),
            Some(role),
            "{name}"
        );
    }
    assert_eq!(
        infer_role_from_path(&PathBuf::from("rules/unknown_table.toml")),
        None
    );
}

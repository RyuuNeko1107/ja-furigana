use super::*;

// ─── 変異テスト由来の追加 ────────────────────────────────────────────────

#[test]
fn name_mei_role_file_loads_into_mei_table_only() {
    let dir = std::env::temp_dir().join(format!(
        "furigana_dict_test_{}_mei_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("mei.toml"),
        "[meta]\nschema_version = \"2\"\nrole = \"name_mei\"\n\n[entries]\n\"太郎\" = \"タロウ\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("sei.toml"),
        "[meta]\nschema_version = \"2\"\nrole = \"name_sei\"\n\n[entries]\n\"山田\" = \"ヤマダ\"\n",
    )
    .unwrap();
    let d = Dict::from_toml_dir(&dir).unwrap();
    assert_eq!(d.lookup_name_mei("太郎"), Some("タロウ"));
    assert_eq!(d.lookup_name_sei("太郎"), None);
    assert_eq!(d.lookup_name_sei("山田"), Some("ヤマダ"));
    assert_eq!(d.lookup_name_mei("山田"), None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn from_toml_str_basic() {
    let toml_str = r#"
        [entries]
        "灰桜" = "ハイザクラ"
        "黎明" = "レイメイ"
    "#;
    let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
    assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
    assert_eq!(d.lookup("黎明"), Some("レイメイ"));
    assert_eq!(d.len(), 2);
    // ★A2: rich field にも Simple variant として entry が保持される
    assert!(matches!(d.lookup_rich("灰桜"), Some(Entry::Simple(_))));
    assert_eq!(
        d.lookup_rich("灰桜").unwrap().default_reading(),
        "ハイザクラ"
    );
}

// ─── ★A2: 新 format inline match の load test ────────────────────────────

#[test]
fn from_toml_str_inline_detailed_form() {
    // inline 形式: "上手" = { reading = "ジョウズ", match = [...] }
    let toml_str = r#"
        [entries]
        "上手" = { reading = "ジョウズ", match = [
          { next_eq = "から", reading = "カミテ" },
        ]}
    "#;
    let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
    // 旧 simple lookup は default_reading が返る
    assert_eq!(d.lookup("上手"), Some("ジョウズ"));
    // rich lookup は Detailed variant
    let entry = d.lookup_rich("上手").expect("rich entry");
    assert!(matches!(entry, Entry::Detailed(_)));
    assert_eq!(entry.default_reading(), "ジョウズ");
    assert_eq!(entry.matches().len(), 1);
    assert_eq!(entry.matches()[0].reading, "カミテ");
    assert_eq!(
        entry.matches()[0].condition.next_eq.as_deref(),
        Some("から")
    );
}

#[test]
fn from_toml_str_expanded_detailed_form() {
    // expanded sub-table 形式 ([entries."x"] と [[entries."x".match]])
    let toml_str = r#"
        [entries."上手"]
        reading = "ジョウズ"

        [[entries."上手".match]]
        next_eq = "から"
        reading = "カミテ"

        [[entries."上手".match]]
        prev_eq = "下"
        reading = "シタテ"
    "#;
    let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
    assert_eq!(d.lookup("上手"), Some("ジョウズ"));
    let entry = d.lookup_rich("上手").expect("rich entry");
    assert_eq!(entry.matches().len(), 2);
    assert_eq!(entry.matches()[0].reading, "カミテ");
    assert_eq!(entry.matches()[1].reading, "シタテ");
    assert_eq!(entry.matches()[1].condition.prev_eq.as_deref(), Some("下"));
}

#[test]
fn from_toml_str_hoists_absorbed_entries() {
    // TOML footgun: detailed entry の match block の後に書かれた bare simple entry は
    // TOML 仕様で match block の table に吸収される。 loader が hoist 救済して
    // top-level simple entry として拾うこと、 かつ detailed entry 本体が壊れないこと。
    let toml_str = r#"
        [entries]
        "霊夢" = "レイム"

        [entries."上手"]
        reading = "ジョウズ"

        [[entries."上手".match]]
        next_eq = "から"
        reading = "カミテ"

        [entries."流行"]
        reading = "リュウコウ"

        [[entries."流行".match]]
        next_starts_any = ["る", "っ"]
        reading = "ハヤ"

        # ↓ これらは TOML 上 entries.流行.match[0] に吸収される
        "完治" = "カンチ"
        "天気" = "テンキ"
        "上手" = "ジョウズ"
    "#;
    let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
    // 吸収されていた simple entry が救済されている
    assert_eq!(d.lookup("完治"), Some("カンチ"));
    assert_eq!(d.lookup("天気"), Some("テンキ"));
    assert!(matches!(d.lookup_rich("完治"), Some(Entry::Simple(_))));
    // detailed entry 本体は壊れていない (match block も保持)
    assert_eq!(d.lookup("流行"), Some("リュウコウ"));
    let ry = d.lookup_rich("流行").expect("流行 rich");
    assert_eq!(ry.matches().len(), 1);
    assert_eq!(ry.matches()[0].reading, "ハヤ");
    // match block の正規 field (reading) は surface として hoist されない
    assert_eq!(d.lookup("ハヤ"), None);
    // 吸収された "上手" は explicit detailed entry を **上書きしない** (= 決定論的、
    // 文脈 match カミテ を保持)
    let kt = d.lookup_rich("上手").expect("上手 rich");
    assert!(
        matches!(kt, Entry::Detailed(_)),
        "detailed 上手 が hoist で潰れた"
    );
    assert_eq!(kt.matches().len(), 1);
    assert_eq!(kt.matches()[0].reading, "カミテ");
}

#[test]
fn from_toml_str_mixed_simple_and_detailed() {
    // 同 file に Simple と Detailed が混在しても OK
    let toml_str = r#"
        [entries]
        "魔理沙" = "マリサ"
        "上手" = { reading = "ジョウズ", match = [
          { next_eq = "から", reading = "カミテ" },
        ]}
        "霊夢" = "レイム"
    "#;
    let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
    assert_eq!(d.len(), 3);
    assert!(matches!(d.lookup_rich("魔理沙"), Some(Entry::Simple(_))));
    assert!(matches!(d.lookup_rich("上手"), Some(Entry::Detailed(_))));
    assert!(matches!(d.lookup_rich("霊夢"), Some(Entry::Simple(_))));
}

#[test]
fn from_toml_str_silently_skips_unrelated_inline_table() {
    // rules 系 file (例: units.toml の `{ kana = "..." }`) と同 dir 混在しても
    // EntryDetail に変換失敗 → silent skip して残り entry は load される
    let toml_str = r#"
        [entries]
        "灰桜" = "ハイザクラ"
        "km" = { kana = "キロメートル" }
        "黎明" = "レイメイ"
    "#;
    let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
    assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
    assert_eq!(d.lookup("黎明"), Some("レイメイ"));
    assert!(
        d.lookup("km").is_none(),
        "rules 系 inline table は silent skip"
    );
    assert_eq!(d.len(), 2);
}

#[test]
fn from_toml_str_empty() {
    let d = Dict::from_toml_str("", "test.toml").unwrap();
    assert!(d.is_empty());
}

#[test]
fn from_toml_str_with_comments() {
    let toml_str = r#"
        # コメント
        [entries]
        "灰桜" = "ハイザクラ"  # inline comment
    "#;
    let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
    assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
}

#[test]
fn from_toml_str_invalid_errors() {
    let err = Dict::from_toml_str("[invalid", "test.toml").unwrap_err();
    assert!(matches!(err, FuriganaError::Toml { .. }));
}

#[test]
fn merge_overwrites() {
    let mut a = Dict::from_toml_str("[entries]\n\"灰桜\" = \"ハイザクラ\"\n", "a.toml").unwrap();
    let b = Dict::from_toml_str(
        "[entries]\n\"灰桜\" = \"カイオウ\"\n\"黎明\" = \"レイメイ\"\n",
        "b.toml",
    )
    .unwrap();
    a.merge(b);
    assert_eq!(a.lookup("灰桜"), Some("カイオウ")); // b が後勝ち
    assert_eq!(a.lookup("黎明"), Some("レイメイ"));
}

#[test]
fn insert_works() {
    let mut d = Dict::new();
    d.insert("灰桜", "ハイザクラ");
    assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
}

fn fresh_temp_dir(suffix: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "furigana_dict_test_{}_{}_{}",
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

/// `[meta] schema_version = "2"` block を body の先頭に付ける test helper macro。
/// `concat!` 経由で literal `&'static str` を返す (= `std::fs::write` に直接渡せる)。
macro_rules! v2 {
    ($body:expr) => {
        concat!("[meta]\nschema_version = \"2\"\n\n", $body)
    };
}

#[test]
fn from_toml_dir_loads_multiple_files() {
    let dir = fresh_temp_dir("dir_load");
    std::fs::write(
        dir.join("01_a.toml"),
        v2!("[entries]\n\"灰桜\" = \"ハイザクラ\"\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("02_b.toml"),
        v2!("[entries]\n\"黎明\" = \"レイメイ\"\n"),
    )
    .unwrap();

    let d = Dict::from_toml_dir(&dir).unwrap();
    assert_eq!(d.len(), 2);
    assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
    assert_eq!(d.lookup("黎明"), Some("レイメイ"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn from_toml_dir_filename_order_decides_priority() {
    let dir = fresh_temp_dir("dir_priority");
    std::fs::write(
        dir.join("01_lower.toml"),
        v2!("[entries]\n\"灰桜\" = \"ハイザクラ\"\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("02_higher.toml"),
        v2!("[entries]\n\"灰桜\" = \"カイオウ\"\n"),
    )
    .unwrap();

    let d = Dict::from_toml_dir(&dir).unwrap();
    assert_eq!(d.lookup("灰桜"), Some("カイオウ"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn from_toml_dir_missing_returns_empty() {
    let d = Dict::from_toml_dir("/nonexistent/dir/path/xyz_furigana_test").unwrap();
    assert!(d.is_empty());
}

#[test]
fn from_toml_dir_recurses_one_level_into_subdirs() {
    // jukugo/general.toml + jukugo/places.toml のような構造を扱えること
    let dir = fresh_temp_dir("dir_subdir");
    let sub = dir.join("jukugo");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(
        sub.join("general.toml"),
        v2!("[entries]\n\"灰桜\" = \"ハイザクラ\"\n"),
    )
    .unwrap();
    std::fs::write(
        sub.join("places.toml"),
        v2!("[entries]\n\"湯島\" = \"ユシマ\"\n"),
    )
    .unwrap();
    // 直下のファイルもまだ拾えること
    std::fs::write(
        dir.join("top.toml"),
        v2!("[entries]\n\"黎明\" = \"レイメイ\"\n"),
    )
    .unwrap();

    let d = Dict::from_toml_dir(&dir).unwrap();
    assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
    assert_eq!(d.lookup("湯島"), Some("ユシマ"));
    assert_eq!(d.lookup("黎明"), Some("レイメイ"));
    assert_eq!(d.len(), 3);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn from_toml_dir_recurses_arbitrary_depth() {
    // works/game/touhou.toml のような任意深度の構造を扱えること
    let dir = fresh_temp_dir("dir_deep");
    let deep = dir.join("works").join("game").join("series");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(
        deep.join("touhou.toml"),
        v2!("[entries]\n\"霊夢\" = \"レイム\"\n\"魔理沙\" = \"マリサ\"\n"),
    )
    .unwrap();
    // 別の深い階層
    let deep2 = dir.join("works").join("anime");
    std::fs::create_dir_all(&deep2).unwrap();
    std::fs::write(
        deep2.join("placeholder.toml"),
        v2!("[entries]\n\"宵闇\" = \"ヨイヤミ\"\n"),
    )
    .unwrap();

    let d = Dict::from_toml_dir(&dir).unwrap();
    assert_eq!(d.lookup("霊夢"), Some("レイム"));
    assert_eq!(d.lookup("魔理沙"), Some("マリサ"));
    assert_eq!(d.lookup("宵闇"), Some("ヨイヤミ"));
    assert_eq!(d.len(), 3);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn from_toml_dir_skips_loanwords_role_via_meta_tag() {
    // role 駆動 dispatch: 同じ dir に jukugo file と loanwords file が混在しても、
    // [meta] role tag で識別して loanwords は Dict に load されないこと
    let dir = fresh_temp_dir("dir_role_loanwords");
    std::fs::write(
        dir.join("jukugo.toml"),
        "[meta]\nschema_version = \"2\"\nrole = \"jukugo\"\n\n[entries]\n\"灰桜\" = \"ハイザクラ\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("loan.toml"),
        "[meta]\nschema_version = \"2\"\nrole = \"loanwords\"\n\n[entries]\n\"Kubernetes\" = \"クバネティス\"\n",
    )
    .unwrap();

    let d = Dict::from_toml_dir(&dir).unwrap();
    assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
    assert_eq!(
        d.lookup("Kubernetes"),
        None,
        "loanwords は role tag で skip"
    );
    assert_eq!(d.len(), 1);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn from_toml_dir_skips_single_overrides_role_via_meta_tag() {
    // single_overrides も role 駆動で識別、 path-based skip に依存しない
    let dir = fresh_temp_dir("dir_role_single");
    std::fs::write(
        dir.join("jukugo.toml"),
        "[meta]\nschema_version = \"2\"\nrole = \"jukugo\"\n\n[entries]\n\"灰桜\" = \"ハイザクラ\"\n",
    )
    .unwrap();
    // 「single_overrides.toml」 という file 名以外で role tag だけで skip されるか
    std::fs::write(
        dir.join("custom_overrides_filename.toml"),
        "[meta]\nschema_version = \"2\"\nrole = \"single_overrides\"\n\n[entries]\n\"土\" = \"ツチ\"\n",
    )
    .unwrap();

    let d = Dict::from_toml_dir(&dir).unwrap();
    assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
    assert_eq!(d.lookup("土"), None, "single_overrides は role tag で skip");
    assert_eq!(d.len(), 1);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn from_toml_dir_path_inference_back_compat() {
    // role tag 無い古い release との互換: path 中の `loanwords/` で skip
    let dir = fresh_temp_dir("dir_path_compat");
    let loan = dir.join("loanwords");
    std::fs::create_dir_all(&loan).unwrap();
    std::fs::write(
        dir.join("jukugo.toml"),
        v2!("[entries]\n\"灰桜\" = \"ハイザクラ\"\n"),
    )
    .unwrap();
    std::fs::write(
        loan.join("it.toml"),
        v2!("[entries]\n\"Kubernetes\" = \"クバネティス\"\n"),
    )
    .unwrap();
    // single_overrides.toml も file 名で skip
    std::fs::write(
        dir.join("single_overrides.toml"),
        v2!("[entries]\n\"土\" = \"ツチ\"\n"),
    )
    .unwrap();

    let d = Dict::from_toml_dir(&dir).unwrap();
    assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
    assert_eq!(
        d.lookup("Kubernetes"),
        None,
        "loanwords/ dir は path 推定で skip"
    );
    assert_eq!(
        d.lookup("土"),
        None,
        "single_overrides.toml は file 名推定で skip"
    );
    assert_eq!(d.len(), 1);

    std::fs::remove_dir_all(&dir).ok();
}

// ─── A1b: schema_version 強制 tests ──────────────────────────────────────

#[test]
fn from_toml_file_rejects_legacy_format_without_meta() {
    // alpha.10〜 dict file も schema_version = "2" を必須化 (★A1b)。
    let dir = fresh_temp_dir("a1b_dict_legacy");
    let path = dir.join("legacy.toml");
    std::fs::write(&path, "[entries]\n\"灰桜\" = \"ハイザクラ\"\n").unwrap();
    let err = Dict::from_toml_file(&path).unwrap_err();
    match err {
        crate::error::FuriganaError::Validation(msg) => {
            assert!(msg.contains("schema_version"), "msg: {msg}");
        }
        other => panic!("expected Validation, got {other:?}"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn from_toml_dir_rejects_legacy_file() {
    // dir 配下に legacy file が混在する場合、 最初に見つけた legacy で fail
    let dir = fresh_temp_dir("a1b_dict_dir_legacy");
    std::fs::write(
        dir.join("legacy.toml"),
        "[entries]\n\"灰桜\" = \"ハイザクラ\"\n",
    )
    .unwrap();
    let err = Dict::from_toml_dir(&dir).unwrap_err();
    assert!(matches!(err, crate::error::FuriganaError::Validation(_)));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn from_toml_file_accepts_v2_format() {
    let dir = fresh_temp_dir("a1b_dict_v2");
    let path = dir.join("v2.toml");
    std::fs::write(&path, v2!("[entries]\n\"灰桜\" = \"ハイザクラ\"\n")).unwrap();
    let d = Dict::from_toml_file(&path).expect("v2 format should accept");
    assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rich_matching_prefix_enumeration_is_deterministic_sorted() {
    // 同一先頭 char の rich entry 群が surface 昇順で列挙されること
    // (= HashMap 反復順に依存しない決定的順序)。これにより同 score 候補の
    // tie-break / analyze() alternatives 順序が run 間で揺れない。
    let mut d = Dict::default();
    // 挿入順をわざと非ソートにする
    for s in ["生命体", "生", "生命", "生死", "生物"] {
        d.insert(s.to_string(), "ダミー".to_string());
    }
    let got: Vec<&str> = d.rich_matching_prefix("生命体").map(|(s, _)| s).collect();
    let mut want = got.clone();
    want.sort_unstable();
    assert_eq!(
        got, want,
        "列挙順が surface 昇順 (決定的) であること: {got:?}"
    );
    // tail の接頭辞になりうる entry だけ (生 / 生命 / 生命体)。
    // 生死 / 生物 は 2 文字目が違うので走査対象外 = bucket 全件を舐めない。
    assert_eq!(got, vec!["生", "生命", "生命体"]);
}

#[test]
fn insert_skips_oversized_value() {
    // runtime の insert (= add_reading) も TOML load と同じ per-value sanitize を
    // 通す。上限超過 reading は OOM 経路なので静かに skip され、追加されない。
    let mut d = Dict::default();
    let huge = "あ".repeat(crate::sanitize::MAX_DICT_VALUE_CHARS + 1);
    d.insert("猫".to_string(), huge);
    assert_eq!(d.lookup("猫"), None, "上限超過 reading は追加しない");
    // 正常な追加は従来どおり通る
    d.insert("猫".to_string(), "ネコ".to_string());
    assert_eq!(d.lookup("猫"), Some("ネコ"));
}

#[test]
fn insert_skips_forbidden_char_value() {
    // NULL / bidi / zero-width 等の禁止文字を含む値も skip (Trojan Source 防御)。
    let mut d = Dict::default();
    d.insert("犬".to_string(), "イ\u{0}ヌ".to_string());
    assert_eq!(d.lookup("犬"), None, "禁止文字を含む reading は追加しない");
    // 上限超過の surface 側も skip
    let huge_surface = "漢".repeat(crate::sanitize::MAX_DICT_VALUE_CHARS + 1);
    d.insert(huge_surface.clone(), "ヨミ".to_string());
    assert_eq!(d.lookup(&huge_surface), None);
}

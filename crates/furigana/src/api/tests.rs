use super::builder::load_loanwords_into;
use super::*;
use std::path::PathBuf;

#[test]
fn analyze_light_matches_analyze_on_tokens() {
    let f = Furigana::minimal().unwrap();
    for text in [
        "私は学生です",
        "今日は2時から",
        "ありがとう",
        "水田さんが来た",
        "髙田です",
        "草",
    ] {
        let full = f.analyze(text);
        let light = f.analyze_light(text);
        assert_eq!(full.tokens.len(), light.tokens.len(), "{text}");
        for (a, b) in full.tokens.iter().zip(&light.tokens) {
            assert_eq!(a.surface, b.surface, "{text}");
            assert_eq!(a.reading, b.reading, "{text}");
            assert_eq!(a.range, b.range, "{text}");
            assert_eq!(a.accent_phrases, b.accent_phrases, "{text}");
            assert_eq!(a.band, b.band, "{text}");
        }
        assert!(light.candidates.is_empty(), "{text}");
        assert!(light
            .tokens
            .iter()
            .all(|t| t.alternatives.is_empty() && !t.ambiguous && t.why.is_none()));
        assert_eq!(full.path_indices, light.path_indices, "{text}");
    }
}

#[test]
fn token_band_tells_dictionary_entries_from_fallbacks() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("灰桜", "ハイザクラ");
    let r = f.analyze_light("灰桜");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].band, 1000, "辞書の見出しは band 1000");
}

#[test]
fn token_band_keeps_the_signal_meaning_for_normalized_surfaces() {
    let dir = std::env::temp_dir().join(format!("band_compat_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("compat.toml"),
        "[meta]
schema_version = \"2\"
role = \"compat\"

[map]
\"髙\" = \"高\"
",
    )
    .unwrap();
    let mut f = Furigana::builder().rules_dir(&dir).build().unwrap();
    f.add_reading("高田", "タカダ");
    // 辞書の見出しがそのまま当たる表記は 1000、 異体字を正規化して当たった表記は原文に戻すので 50
    // (改善 signal の集計は、 解析した表記と原文の表記が一致する token だけを辞書の候補に結び付けてきた)
    assert_eq!(f.analyze_light("高田").tokens[0].band, 1000);
    let variant = f.analyze_light("髙田");
    assert_eq!(variant.tokens[0].surface, "髙田");
    assert_eq!(variant.tokens[0].band, 50);
    assert_eq!(f.analyze("髙田").tokens[0].band, 50, "analyze も同じ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn analyze_reading_matches_analyze_light_except_table_accents() {
    let f = Furigana::minimal().unwrap();
    for text in [
        "私は学生です",
        "今日は2時から",
        "ありがとう",
        "水田さんが来た",
    ] {
        let light = f.analyze_light(text);
        let reading = f.analyze_reading(text);
        assert_eq!(light.tokens.len(), reading.tokens.len(), "{text}");
        for (a, b) in light.tokens.iter().zip(&reading.tokens) {
            assert_eq!(a.surface, b.surface, "{text}");
            assert_eq!(a.reading, b.reading, "{text}");
            assert_eq!(a.range, b.range, "{text}");
            assert_eq!(a.band, b.band, "{text}");
        }
    }
}

#[test]
fn warm_up_does_not_change_output() {
    let f = Furigana::minimal().unwrap();
    let before = f.to_hiragana("今日は2時から水田さんが来た");
    f.warm_up();
    assert_eq!(f.to_hiragana("今日は2時から水田さんが来た"), before);
}

#[test]
fn accent_from_analysis_equals_to_accent() {
    let f = Furigana::minimal().unwrap();
    for text in ["私は学生です", "今日は2時から", "ありがとう"] {
        let a = serde_json::to_value(f.to_accent(text)).unwrap();
        let b = serde_json::to_value(AccentResult::from_analysis(f.analyze(text))).unwrap();
        assert_eq!(a, b, "{text}");
    }
}

/// 不変条件テスト群で使う代表入力 (漢字/かな/英数/記号/改行/tab/絵文字/
/// astral 漢字/踊り字/混在)。
const INVARIANT_INPUTS: &[&str] = &[
    "",
    "こんにちは",
    "灰桜",
    "猫が好き",
    "猫API犬",
    "猫が\n好きABC123",
    "猫\tいぬ",
    "人々",
    "𠮷野家",   // CJK 拡張B漢字
    "魚は𩸽だ", // astral 漢字 + かな
    "絵文字😀テスト🎉",
    "https://example.com を見て",
    "9時30分に1000円",
    "葛\u{E0100}飾", // IVS 付き漢字 (lookup 正規化 / 表示は IVS 保持)
    "３本",          // 全角数字 (NFKC で半角化して lookup、 表示は全角保持)
    "①番",           // 丸数字 (NFKC で 1 番化して lookup、 表示は丸数字保持)
    "猫{犬}",        // ruby delimiter `{` `}` を含む入力 (escape 必須)
    "a|b猫",         // ruby delimiter `|` を含む入力
    "C:\\猫\\犬",    // backslash を含む入力 (escape char 自身)
];

#[test]
fn postprocess_applies_the_rules_for_the_mode_only() {
    // 自前で出力を組み立てる呼び出し側が、 to_hiragana と同じ後処理を掛けられる
    let dir = std::env::temp_dir().join(format!("furigana_pp_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("postprocess.toml"),
        "[meta]
schema_version = \"2\"
role = \"postprocess\"

         [[rule]]
pattern = \"ジュウパー\"
replacement = \"ジュッパー\"
modes = [\"hiragana\"]
",
    )
    .unwrap();
    let f = FuriganaBuilder::new().rules_dir(&dir).build().unwrap();
    assert_eq!(f.postprocess("ジュウパー", "hiragana"), "ジュッパー");
    assert_eq!(f.postprocess("ジュウパー", "tts"), "ジュウパー");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn to_hiragana_keeps_kana_surface_verbatim() {
    // 全 kana surface は表記そのまま維持する (surface_is_all_kana 分岐)。
    // カタカナ語はカタカナのまま (こーひー化しない)、 助詞も surface 維持。
    let f = Furigana::minimal().unwrap();
    assert_eq!(f.to_hiragana("コーヒー"), "コーヒー");
    assert_eq!(f.to_hiragana("猫は好き"), "ねこはすき");
}

#[test]
fn invariant_surface_concat_equals_input() {
    // 不変条件: token の surface を連結すると入力と完全一致する
    // (= 文字の欠落/重複/順序入替が無い = 全 byte を gap なく覆う)。
    let f = Furigana::minimal().unwrap();
    for &input in INVARIANT_INPUTS {
        let concat: String = f
            .tokenize(input)
            .iter()
            .map(|t| t.surface.clone())
            .collect();
        assert_eq!(concat, input, "surface 連結が入力と不一致: {input:?}");
    }
}

#[test]
fn invariant_ruby_stripped_equals_hiragana() {
    // 不変条件: to_ruby から ruby markup `{surface|reading}` を読みに畳むと
    // to_hiragana と一致する (2 つの公開経路の読みが整合)。
    let f = Furigana::minimal().unwrap();
    for &input in INVARIANT_INPUTS {
        let ruby = f.to_ruby(input);
        let stripped = strip_ruby_markup(&ruby);
        assert_eq!(
            stripped,
            f.to_hiragana(input),
            "ruby-strip と to_hiragana が不一致: {input:?} (ruby={ruby:?})"
        );
    }
}

/// `{surface|reading}` → reading に畳み、 markup 外の素テキストはそのまま残す
/// (テスト用 ruby パーサ)。 `\{` `\}` `\|` `\\` の escape を un-escape する。
fn strip_ruby_markup(ruby: &str) -> String {
    let mut out = String::new();
    let mut chars = ruby.chars();
    while let Some(c) = chars.next() {
        match c {
            // 素テキスト中の escape → un-escape
            '\\' => {
                if let Some(lit) = chars.next() {
                    out.push(lit);
                }
            }
            '{' => {
                // surface を読み飛ばし、 unescaped `|` 以降の reading を un-escape
                // して取り、 unescaped `}` で閉じる。
                let mut seen_pipe = false;
                let mut reading = String::new();
                while let Some(inner) = chars.next() {
                    match inner {
                        '\\' => {
                            let lit = chars.next();
                            if seen_pipe {
                                if let Some(l) = lit {
                                    reading.push(l);
                                }
                            }
                        }
                        '|' if !seen_pipe => seen_pipe = true,
                        '}' => break,
                        other if seen_pipe => reading.push(other),
                        _ => {} // surface char (discard)
                    }
                }
                out.push_str(&reading);
            }
            other => out.push(other),
        }
    }
    out
}

#[test]
fn minimal_init_works() {
    let f = Furigana::minimal().expect("minimal init failed");
    // 漢字無しの入力は素通し
    assert_eq!(f.to_ruby("こんにちは"), "こんにちは");
}

#[test]
fn add_reading_then_to_ruby() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("灰桜", "ハイザクラ");
    // contains だと ruby markup (`{…|…}`) が落ちても緑になる。完全一致で固定。
    assert_eq!(f.to_ruby("灰桜"), "{灰桜|はいざくら}");
}

#[test]
fn builder_with_extra_entries() {
    let f = Furigana::builder()
        .add_entry("灰桜", "ハイザクラ")
        .add_entry("黎明", "レイメイ")
        .build()
        .unwrap();
    assert_eq!(f.dict_size(), 2);

    // 個別 contains だと間の助詞「と」脱落や順序入替を見逃す。全文一致で固定。
    assert_eq!(
        f.to_ruby("灰桜と黎明"),
        "{灰桜|はいざくら}と{黎明|れいめい}"
    );
}

#[test]
fn to_hiragana_basic() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("灰桜", "ハイザクラ");
    // starts_with だと末尾「の道」の変換が未検証。全文一致で固定。
    assert_eq!(f.to_hiragana("灰桜の道"), "はいざくらのみち");
}

#[test]
fn minimal_has_no_rules_loaded() {
    // 本体には rules を embed しない方針なので、minimal は空 default。
    // 「一人」は context.toml の default が無いため lindera 由来の読みになる。
    let f = Furigana::minimal().unwrap();
    let ruby = f.to_ruby("一人");
    // 何らかのひらがな化はされるはずだが、context default の "ヒトリ" は出ない
    // (lindera が 一+人 で個別に読むため、典型的には「いちにん」)
    assert!(!ruby.is_empty(), "ruby: {ruby}");
}

#[test]
fn empty_input_yields_empty() {
    let f = Furigana::minimal().unwrap();
    assert_eq!(f.to_ruby(""), "");
    assert_eq!(f.to_hiragana(""), "");
    assert!(f.tokenize("").is_empty());
}

#[test]
fn newline_input_does_not_blank_out() {
    // regression: 改行を含む input は Lindera が改行を token から落とし、
    // fallback provider が disable → path 全空 → 出力が空になる bug があった。
    // Lindera fallback の gap-passthrough で改行を覆い、 改行を保持したまま
    // 各行を正しく変換できることを確認する。
    let f = Furigana::minimal().unwrap();
    let hira = f.to_hiragana("猫が\n好き");
    assert_eq!(hira, "ねこが\nすき", "改行を挟んでも path 構築できる");

    // 先頭 / 末尾 / 連続改行も path 全空にならない
    assert_eq!(f.to_hiragana("\n猫"), "\nねこ");
    assert_eq!(f.to_hiragana("猫\n"), "ねこ\n");
    assert_eq!(f.to_hiragana("猫\n\n犬"), "ねこ\n\nいぬ");

    // ruby も同様 (漢字部だけ ruby 化、 改行はそのまま)
    let ruby = f.to_ruby("灰桜\n道");
    assert!(ruby.contains('\n'), "改行が保持される: {ruby}");
    assert!(!ruby.is_empty());
}

#[test]
fn nonempty_input_never_blanks_out_with_format_gap_chars() {
    // 不変条件ガード: 漢字の間に Lindera が落としうる format/幅ゼロ文字が挟まっても
    // safety net (gap passthrough) が働き、非空入力は決して空出力にならない。
    // (監査で「ZWSP 等で safety net 全廃 → 空出力」が疑われたが、実際には Lindera が
    //  これらを token 保持するため再現せず。回帰防止のためガードとして固定する。)
    let f = Furigana::minimal().unwrap();
    for &gap in &[
        "\u{200B}", // ZWSP
        "\u{FEFF}", // BOM / ZWNBSP
        "\u{200D}", // ZWJ
        "\u{00AD}", // SOFT HYPHEN
        "\u{200E}", // LRM
        "\u{2060}", // WORD JOINER
        "\u{FE0F}", // VS16
    ] {
        let input = format!("猫{gap}犬");
        let out = f.to_hiragana(&input);
        assert!(
            !out.is_empty(),
            "blank out on gap char U+{:04X}",
            gap.chars().next().unwrap() as u32
        );
    }
}

#[test]
fn half_width_space_is_preserved_not_widened() {
    // 旧実装は preprocess_input で半角 space → 全角 space (U+3000) に置換していた
    // (= scoring engine が ASCII whitespace を覆えない問題の workaround)。
    // Lindera fallback の gap-passthrough 導入でこの hack は不要になり撤去。
    // 半角 space は出力にそのまま残る (= 全角化けしない) ことを確認する。
    let f = Furigana::minimal().unwrap();

    // 英字混在: space が全角化すると TTS / 表示で不自然
    assert_eq!(f.to_hiragana("hello world"), "hello world");
    assert!(!f.to_ruby("hello world").contains('\u{3000}'));

    // 旧 preprocess_input が導入された元凶ケース。 path 全空にならず、 かつ
    // 半角 space 区切りが維持される (全角に化けない)。
    let hira = f.to_hiragana("変なの 水田");
    assert!(!hira.is_empty(), "path 構築できる");
    assert!(hira.contains(' '), "半角 space が保持される: {hira:?}");
    assert!(!hira.contains('\u{3000}'), "全角化しない: {hira:?}");
}

#[test]
fn debug_format_shows_summary() {
    let f = Furigana::minimal().unwrap();
    let s = format!("{f:?}");
    assert!(s.contains("Furigana"));
    assert!(s.contains("dict_size"));
}

// 注: 以下 3 テストは過去 cargo test harness で 51 GB alloc 暴走を起こしていたが、
// 原因が旧 `NumberChunker` (chunks/ module、 alpha.15 で削除済) の dynamic regex の
// **never-match pattern** (`r"(?P<n>\A\B)(?P<x>\A\B)"`) であったことを切り分け、
// `Option<Regex>` 化で完全回避した。 同 pattern は現 scoring/numbers.rs の
// regex builder 群にも踏襲されている。CHANGELOG 参照。

#[test]
fn to_tts_inserts_pauses() {
    let f = Furigana::minimal().unwrap();
    let opts = TtsOptions::default();
    let result = f.to_tts("こんにちは。さようなら。", &opts);
    assert!(result.contains("こんにちは。 "), "result: {result}");
}

#[test]
fn to_tts_with_non_space_marker_preserves_long_pause() {
    let f = Furigana::minimal().unwrap();
    let opts = TtsOptions {
        short_pause: "<s>".to_string(),
        long_pause: "<l>".to_string(),
        keep_period: true,
        silence_symbols: false,
    };
    let result = f.to_tts("こんにちは。さよなら。", &opts);
    assert!(result.contains("こんにちは。<l>"), "result: {result}");
}

#[test]
fn to_tts_silences_emoji_and_kaomoji_parts() {
    let f = Furigana::minimal().unwrap();
    let opts = TtsOptions {
        silence_symbols: true,
        ..TtsOptions::default()
    };
    // 絵文字は読み上げない
    assert!(!f.to_tts("こんにちは🎉", &opts).contains('🎉'));
    // 顔文字パーツ (中黒 / ω / 括弧) が 「なかぐろ おめが」 化しない
    let kaomoji = f.to_tts("すごい(´・ω・`)", &opts);
    assert!(!kaomoji.contains("なかぐろ"), "{kaomoji:?}");
    assert!(!kaomoji.contains("おめが"), "{kaomoji:?}");
    assert!(kaomoji.contains("すごい"), "{kaomoji:?}");
    // 句読点は pause 情報なので残る
    assert!(f.to_tts("ねこ。いぬ。", &opts).contains('。'));
}

#[test]
fn to_tts_keeps_symbols_that_have_readings() {
    // 単位 / 演算子 / 通貨は 「読みを持つ語」 なので silence_symbols でも落とさない
    let f = Furigana::minimal().unwrap();
    let opts = TtsOptions {
        silence_symbols: true,
        ..TtsOptions::default()
    };
    // silence_symbols の有無で出力が変わらない = 記号が落ちていない
    for input in ["50%オフ", "25℃です", "3×4", "1000円+税"] {
        assert_eq!(
            f.to_tts(input, &opts),
            f.to_tts(input, &TtsOptions::default()),
            "{input:?} は silence_symbols で変わってはいけない"
        );
    }
    // 逆に 顔文字パーツとしても使う記号は装飾側に残す (whitelist を広げすぎない)
    for c in ['/', '／', '~', '〜', '<', '>', '&', '@', '#', '・'] {
        assert!(tts::is_decorative_symbol(c), "{c:?} は装飾記号のまま");
    }
}

#[test]
fn to_tts_keeps_symbols_by_default() {
    // default は従来どおり (= 記号も読む)
    let f = Furigana::minimal().unwrap();
    let opts = TtsOptions::default();
    assert!(f.to_tts("こんにちは🎉", &opts).contains('🎉'));
}

#[test]
fn segment_tts_returns_vec() {
    let f = Furigana::minimal().unwrap();
    let opts = TtsOptions::default();
    let segs = f.segment_tts("ぶん1。ぶん2。ぶん3。", &opts, 60);
    assert_eq!(segs.len(), 3);
    // len だけだと ["", "", ""] のような空 split でも緑になる。
    // 各 segment が非空で文末「。」を保持していることまで固定する。
    for (i, s) in segs.iter().enumerate() {
        assert!(!s.is_empty(), "segment {i} is empty: {segs:?}");
        assert!(s.ends_with('。'), "segment {i} keeps period: {s:?}");
    }
}

#[test]
fn rules_dir_overrides_default() {
    // テスト用 fixture (本来は furigana-dict から pull したものを使う)
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("rules");
    let f = Furigana::builder()
        .rules_dir(&dir)
        .build()
        .expect("build with rules_dir failed");
    // 3本 → サンボン (counters.toml 由来、 NumberCandidateProvider が hit)
    let hira = f.to_hiragana("3本");
    assert!(hira.contains("さんぼん"), "hiragana: {hira}");
}

// ─── Smart engine wire-up sanity tests ───────────────────────────────────

#[test]
fn to_ruby_uses_dict_then_lindera_fallback() {
    // 「灰桜の道」 → 灰桜 (dict band 1000、 ハイザクラ) + の (Lindera band 50、 ノ)
    // + 道 (Lindera band 50、 ミチ) → "{灰桜|はいざくら}の{道|みち}"
    let f = Furigana::builder()
        .add_entry("灰桜", "ハイザクラ")
        .build()
        .unwrap();
    let ruby = f.to_ruby("灰桜の道");
    assert!(ruby.contains("{灰桜|はいざくら}"), "expected ruby: {ruby}");
}

// ─── 入力正規化: lookup のみ正規化 / 表示 surface は原文保持 ───────────────

#[test]
fn ivs_normalized_for_lookup_but_surface_keeps_selector() {
    // 葛飾 (IVS U+E0100 付き) は dict lookup では IVS を剥がして 「葛飾」 で hit、
    // 表示 surface は IVS を保ったまま (= 原文尊重)。
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("葛飾", "カツシカ");
    let input = "葛\u{E0100}飾";

    // surface 連結は原文 (IVS 込み) と完全一致
    let concat: String = f
        .tokenize(input)
        .iter()
        .map(|t| t.surface.clone())
        .collect();
    assert_eq!(concat, input, "surface が IVS を保持して原文一致");

    // ruby は IVS を保った surface + 正規化形で引いた読み
    assert_eq!(f.to_ruby(input), "{葛\u{E0100}飾|かつしか}");
    assert_eq!(f.to_hiragana(input), "かつしか");
}

#[test]
fn fullwidth_digit_normalized_for_lookup_but_surface_kept() {
    // 全角 「３本」 は NFKC で 「3本」 に正規化して counter lookup (サンボン)、
    // 表示 surface は全角 「３」 のまま。
    let f = Furigana::builder()
        .rules_dir(fixture_rules_dir())
        .build()
        .expect("build with rules_dir");
    let input = "３本";

    let concat: String = f
        .tokenize(input)
        .iter()
        .map(|t| t.surface.clone())
        .collect();
    assert_eq!(concat, input, "全角 surface が保持される");
    assert_eq!(f.to_ruby(input), "{３本|さんぼん}");
    assert_eq!(f.to_hiragana(input), "さんぼん");
}

#[test]
fn plain_japanese_takes_identity_fast_path() {
    // 正規化で何も変わらない通常入力は remap を経ず旧来どおりの surface を返す
    // (= 異体字 / IVS / 全角を含まない大多数の入力は挙動不変)。
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("灰桜", "ハイザクラ");
    assert_eq!(f.to_ruby("灰桜の道"), "{灰桜|はいざくら}の{道|みち}");
}

#[test]
fn analyze_remaps_surface_and_range_to_original() {
    // analyze() も token.surface / range を原文へ戻す (reading は正規化形で解決)。
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("葛飾", "カツシカ");
    let input = "葛\u{E0100}飾";
    let r = f.analyze(input);
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].surface, input, "原文 surface (IVS 込み)");
    assert_eq!(r.tokens[0].reading, "カツシカ");
    // range は原文 byte 全域 (葛 3 + IVS 4 + 飾 3 = 10 byte)
    assert_eq!(r.tokens[0].range, 0..input.len());
}

#[test]
fn compat_loaded_from_core_dict_dir_not_just_rules_dir() {
    // production の wrapper は rules_dir と core_dict_dir を別 path で mount する。
    // compat.toml は dict 配布で core/ に同梱されるので、 core_dict_dir 経由でも
    // 読まれて 髙→高 の異体字正規化が効くこと (回帰: 旧実装は rules_dir のみ走査で
    // compat が production で完全に dead だった)。
    let dir = std::env::temp_dir().join(format!(
        "furigana_compat_core_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("compat.toml"),
        "[meta]\nschema_version = \"2\"\nrole = \"compat\"\n\n[map]\n\"髙\" = \"高\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("entries.toml"),
        "[meta]\nschema_version = \"2\"\nrole = \"jukugo\"\n\n[entries]\n\"高田\" = \"タカダ\"\n",
    )
    .unwrap();
    let f = Furigana::builder().core_dict_dir(&dir).build().unwrap();
    // 髙田 (異体字) → core 同梱 compat で 高田 を lookup (たかだ)、 表示は 髙 を保持
    assert_eq!(f.to_ruby("髙田"), "{髙田|たかだ}");
}

#[test]
fn multichar_compat_normalized_for_lookup_keeps_original_surface() {
    // canonical 複数文字の compat (廿 = 二十、 旧字漢数字) で lookup し、 表示 surface は
    // 廿 を保持する。 1 文字 → 2 文字展開でも alignment が崩れないことの回帰ガード。
    let dir = std::env::temp_dir().join(format!(
        "furigana_multichar_compat_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("compat.toml"),
        "[meta]\nschema_version = \"2\"\nrole = \"compat\"\n\n[map]\n\"廿\" = \"二十\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("entries.toml"),
        "[meta]\nschema_version = \"2\"\nrole = \"jukugo\"\n\n[entries]\n\"二十\" = \"ニジュウ\"\n",
    )
    .unwrap();
    let f = Furigana::builder().core_dict_dir(&dir).build().unwrap();
    // 廿 → 二十 で lookup (ニジュウ)、 surface は 廿 のまま
    assert_eq!(f.to_ruby("廿"), "{廿|にじゅう}");
    // surface 連結は原文一致 (1→2 文字展開でも不変条件維持)
    let concat: String = f.tokenize("廿").iter().map(|t| t.surface.clone()).collect();
    assert_eq!(concat, "廿");
}

#[test]
fn multichar_compat_merges_split_expansion_into_one_token() {
    // 展開後の norm が複数 token に分割されるケース (卅 → 三十 → 三 + 十) で、
    // 空 surface token を作らず読みを結合して 1 token にする
    // (回帰: `{卅|さん}{|じゅう}` のような空 surface ruby を防ぐ)。
    let dir = std::env::temp_dir().join(format!(
        "furigana_multichar_split_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("compat.toml"),
        "[meta]\nschema_version = \"2\"\nrole = \"compat\"\n\n[map]\n\"卅\" = \"三十\"\n",
    )
    .unwrap();
    // 「三十」 を entry にせず 「三」「十」 個別だけ登録 → path は 2 token に分割される
    std::fs::write(
        dir.join("entries.toml"),
        "[meta]\nschema_version = \"2\"\nrole = \"jukugo\"\n\n[entries]\n\"三\" = \"サン\"\n\"十\" = \"ジュウ\"\n",
    )
    .unwrap();
    let f = Furigana::builder().core_dict_dir(&dir).build().unwrap();
    // 分割された 三+十 の読みを結合し、 surface は 卅 のまま 1 token
    let toks = f.tokenize("卅");
    assert_eq!(toks.len(), 1, "1 token に結合される: {toks:?}");
    assert_eq!(toks[0].surface, "卅");
    assert_eq!(f.to_ruby("卅"), "{卅|さんじゅう}");
    assert!(
        !f.to_ruby("卅").contains("{|"),
        "空 surface ruby を出さない"
    );
}

// ─── analyze() (F1) tests ────────────────────────────────────────────────

#[test]
fn analyze_empty_input_yields_empty_result() {
    let f = Furigana::minimal().unwrap();
    let r = f.analyze("");
    assert!(r.tokens.is_empty());
    assert!(r.candidates.is_empty());
    assert!(r.path_indices.is_empty());
    assert!(r.boundary_regions.is_empty());
}

#[test]
fn analyze_single_jukugo_entry_yields_one_token() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("灰桜", "ハイザクラ");
    let r = f.analyze("灰桜");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].surface, "灰桜");
    assert_eq!(r.tokens[0].reading, "ハイザクラ");
    assert_eq!(r.tokens[0].range, 0..6); // UTF-8 3 bytes × 2
    assert_eq!(r.path_indices, vec![0]);
}

#[test]
fn analyze_jukugo_prefers_longer_match_over_unihan() {
    // 「灰桜」 jukugo (band 1000、 length 2) が
    // 「灰」 unihan + 「桜」 unihan (各 band 100) を path レベルで上回る
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("灰桜", "ハイザクラ");
    f.add_reading("灰", "ハイ");
    f.add_reading("桜", "サクラ");
    let r = f.analyze("灰桜");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].reading, "ハイザクラ");
}

#[test]
fn analyze_unihan_fallback_when_no_jukugo() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("猫", "ネコ");
    let r = f.analyze("猫");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].surface, "猫");
    assert_eq!(r.tokens[0].reading, "ネコ");
}

#[test]
fn analyze_url_protected_token_passthrough() {
    let f = Furigana::minimal().unwrap();
    let input = "https://example.com";
    let r = f.analyze(input);
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].surface, input);
    assert_eq!(r.tokens[0].reading, input); // passthrough
}

#[test]
fn analyze_alphabet_passthrough_returns_surface() {
    let f = Furigana::minimal().unwrap();
    let r = f.analyze("API");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].surface, "API");
    assert_eq!(r.tokens[0].reading, "API"); // passthrough_only (lookup 無し)
}

#[test]
fn analyze_falls_back_to_lindera_when_no_other_provider_covers() {
    // alpha.13 以前: 「猫が好き」 のような ひらがな混在 input は dict / 保護 /
    // 英字 のどれも cover せず path 構築不能だった。
    // alpha.13+ : Lindera fallback (band 50) が input 全体を tokenize、
    // 他 provider が空の位置を埋めるので path が必ず構築される (safety net)。
    let f = Furigana::minimal().unwrap();
    let r = f.analyze("猫が好き");
    assert!(
        !r.tokens.is_empty(),
        "Lindera fallback should cover input: {r:?}"
    );
    // path 全体を Lindera で覆ったので token 列が input を完全に span するはず
    let total_len: usize = r.tokens.iter().map(|t| t.range.end - t.range.start).sum();
    assert_eq!(total_len, "猫が好き".len());
}

#[test]
fn analyze_emits_boundary_region_for_kanji_run() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("灰桜", "ハイザクラ");
    let r = f.analyze("灰桜");
    // 漢字 2 字連続 region として検出される
    assert_eq!(r.boundary_regions.len(), 1);
    assert_eq!(r.boundary_regions[0], 0..6);
}

#[test]
fn analyze_strips_intonation_brackets_from_reading() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("灰桜", "ハ[イザクラ");
    let r = f.analyze("灰桜");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].reading, "ハイザクラ");
    // 0.2.0: accent_phrases にパース結果が入る
    assert_eq!(r.tokens[0].accent_phrases.len(), 1);
    assert_eq!(r.tokens[0].accent_phrases[0].reading, "ハイザクラ");
    assert_eq!(r.tokens[0].accent_phrases[0].mora, 5);
    assert_eq!(r.tokens[0].accent_phrases[0].accent, Some(0)); // flat
}

#[test]
fn to_accent_returns_accent_result_with_bracket_entry() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("雨", "ア]メ");
    let r = f.to_accent("雨");
    assert_eq!(r.schema_version, "1");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].surface, "雨");
    assert_eq!(r.tokens[0].reading, "アメ");
    assert_eq!(r.tokens[0].accent_phrases.len(), 1);
    assert_eq!(r.tokens[0].accent_phrases[0].accent, Some(1)); // 頭高
}

#[test]
fn to_accent_no_brackets_yields_empty_phrases() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("猫", "ネコ");
    let r = f.to_accent("猫");
    assert_eq!(r.tokens[0].reading, "ネコ");
    assert!(r.tokens[0].accent_phrases.is_empty());
}

// ─── accent 推定 (opt-in、 ADR-0007) ─────────────────────────────────────

#[test]
fn estimate_accent_fills_loanword_by_minus3_rule() {
    let f = Furigana::builder().estimate_accent(true).build().unwrap();
    let r = f.to_accent("カーテン");
    assert_eq!(r.tokens[0].reading, "カーテン"); // 長音保持 (カアテン 化けの回帰 pin)
    let p = &r.tokens[0].accent_phrases[0];
    assert_eq!(p.reading, "カーテン");
    assert_eq!(p.mora, 4);
    assert_eq!(p.accent, Some(1)); // -3 = ー → 左シフト → カ]ーテン
    assert!(p.estimated);
}

#[test]
fn estimate_accent_flags_name_via_honorific_context() {
    // 田中 は IPADIC 人名 (敬称文脈の standalone 照会で is_name 補完)
    let f = Furigana::builder().estimate_accent(true).build().unwrap();
    let r = f.to_accent("田中さん");
    let tanaka = &r.tokens[0];
    assert_eq!(tanaka.surface, "田中");
    let p = &tanaka.accent_phrases[0];
    assert_eq!(p.accent, Some(1)); // 3 mora 姓 → -3 = タ]ナカ
    assert!(p.estimated);
    // 敬称そのものは推定対象外
    assert!(r.tokens[1].accent_phrases.is_empty());
}

#[test]
fn exclude_dict_path_skips_entries_under_the_path() {
    let dir = std::env::temp_dir().join(format!(
        "furigana_exclude_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("works/vtuber")).unwrap();
    std::fs::write(
        dir.join("general.toml"),
        "[meta]
schema_version = \"2\"
role = \"jukugo\"

[entries]
\"灰桜\" = \"ハイザクラ\"
",
    )
    .unwrap();
    std::fs::write(
        dir.join("works/vtuber/names.toml"),
        "[meta]
schema_version = \"2\"
role = \"works\"

[entries]
\"天音\" = \"アマネ\"
",
    )
    .unwrap();
    let all = Furigana::builder().core_dict_dir(&dir).build().unwrap();
    assert_eq!(all.to_ruby("天音"), "{天音|あまね}");
    let ex = Furigana::builder()
        .core_dict_dir(&dir)
        .exclude_dict_path("works")
        .build()
        .unwrap();
    assert_ne!(ex.to_ruby("天音"), "{天音|あまね}");
    assert_eq!(ex.to_ruby("灰桜"), "{灰桜|はいざくら}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn person_name_tables_and_name_hint_through_public_api() {
    let dir = std::env::temp_dir().join(format!(
        "furigana_names_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("names")).unwrap();
    std::fs::write(
        dir.join("entries.toml"),
        "[meta]
schema_version = \"2\"
role = \"jukugo\"

[entries]
\"水田\" = \"スイデン\"
\"選手\" = \"センシュ\"
",
    )
    .unwrap();
    std::fs::write(
        dir.join("names/sei.toml"),
        "[meta]
schema_version = \"2\"
role = \"name_sei\"

[entries]
\"水田\" = \"みずた\"
",
    )
    .unwrap();
    // 手がかり (敬称・肩書き) は辞書の rules が正。 lib には既定値が無いので fixture を置く
    let rules = dir.with_extension("rules");
    std::fs::create_dir_all(&rules).unwrap();
    std::fs::write(
        rules.join("person_name_cues.toml"),
        include_str!("../../tests/fixtures/rules/person_name_cues.toml"),
    )
    .unwrap();
    let f = Furigana::builder()
        .rules_dir(&rules)
        .core_dict_dir(&dir)
        .build()
        .unwrap();
    // 手がかり (選手) があれば人名の読み、 無ければ辞書の既定 (田んぼの水田)
    assert_eq!(f.to_hiragana("水田選手"), "みずたせんしゅ");
    assert_eq!(f.to_hiragana("広い水田"), "ひろいすいでん");
    // ヒント: Name は手がかり無しで人名、 NotName は専用経路を止める
    let name = f.tokenize_with_hint("水田", NameHint::Name);
    assert_eq!(name[0].reading.as_deref(), Some("ミズタ"));
    let not = f.tokenize_with_hint("水田選手", NameHint::NotName);
    assert_eq!(not[0].reading.as_deref(), Some("スイデン"));
    // analyze には why と、 採用しなかった読みが付く
    let a = f.analyze_with_hint("水田選手", NameHint::Auto);
    assert!(a.tokens[0]
        .why
        .as_deref()
        .unwrap_or("")
        .starts_with("name:"));
    assert!(a.tokens[0]
        .alternatives
        .iter()
        .any(|x| x.reading == "スイデン"));
    std::fs::remove_dir_all(&rules).ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn accent_lexicon_on_by_default_and_can_be_disabled() {
    let dir = std::env::temp_dir().join(format!(
        "furigana_accent_lexicon_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("accent")).unwrap();
    std::fs::write(
        dir.join("entries.toml"),
        "[meta]
schema_version = \"2\"
role = \"jukugo\"

[entries]
\"天気\" = \"テンキ\"
",
    )
    .unwrap();
    std::fs::write(
        dir.join("accent/table.toml"),
        "[meta]
schema_version = \"2\"
role = \"accent\"

[entries]
\"天気\" = \"[テ]ンキ\"
",
    )
    .unwrap();
    let on = Furigana::builder().core_dict_dir(&dir).build().unwrap();
    let t = &on.to_accent("天気").tokens[0];
    assert_eq!(t.surface, "天気");
    assert_eq!(t.accent_phrases[0].accent, Some(1));
    assert!(!t.accent_phrases[0].estimated);

    let off = Furigana::builder()
        .core_dict_dir(&dir)
        .accent_lexicon(false)
        .build()
        .unwrap();
    assert!(off.to_accent("天気").tokens[0].accent_phrases.is_empty());
    // 読みは表の有無で変わらない
    assert_eq!(on.to_ruby("天気"), off.to_ruby("天気"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn estimate_accent_off_by_default() {
    // flag なし = ADR-0002 の従来挙動 (不明は accent_phrases 空)
    let f = Furigana::minimal().unwrap();
    let r = f.to_accent("カーテン");
    assert!(r.tokens[0].accent_phrases.is_empty());
}

#[test]
fn estimate_accent_does_not_change_readings() {
    // 推定 on/off で読み出力は完全一致 (accent_phrases のみの差)
    let on = Furigana::builder().estimate_accent(true).build().unwrap();
    let off = Furigana::minimal().unwrap();
    for input in ["田中さんとカーテン", "烈核の一撃", "猫が好き"] {
        assert_eq!(on.to_hiragana(input), off.to_hiragana(input));
        assert_eq!(on.to_ruby(input), off.to_ruby(input));
    }
}

#[test]
fn estimated_field_absent_in_dict_bracket_json() {
    // dict bracket 由来 (真値) の JSON には estimated が出ない (additive 互換)
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("雨", "ア]メ");
    let json = serde_json::to_string(&f.to_accent("雨")).unwrap();
    assert!(!json.contains("estimated"), "json = {json}");
}

// ─── OOV 促音便 join (default on、 ADR-0008) ─────────────────────────────

#[test]
fn oov_kanji_chain_gets_sokuonbin() {
    // 烈核 (辞書/IPADIC に無い合成語) が単漢字 chain になったとき ガクコウ 型の
    // べた読みでなく促音便が入る
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("烈", "レツ");
    f.add_reading("核", "カク");
    assert_eq!(f.to_hiragana("烈核"), "れっかく");
}

#[test]
fn known_word_boundary_not_geminated() {
    // multi-char token (実単語) が絡む join は促音便しない
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("圧縮", "アッシュク");
    f.add_reading("率", "リツ");
    assert_eq!(f.to_hiragana("圧縮率"), "あっしゅくりつ");
}

#[test]
fn analyze_expands_odoriji_with_rendaku() {
    // 神々 → カミ + ガミ (連濁あり)
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("神", "カミ");
    let r = f.analyze("神々");
    assert_eq!(r.tokens.len(), 2);
    assert_eq!(r.tokens[0].surface, "神");
    assert_eq!(r.tokens[0].reading, "カミ");
    assert_eq!(r.tokens[1].surface, "々");
    assert_eq!(r.tokens[1].reading, "ガミ");
}

#[test]
fn analyze_odoriji_falls_back_to_clone_for_non_voiceable() {
    // 我々 → ワレ + ワレ (ワ 行は連濁対象外、 そのまま複製)
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("我", "ワレ");
    let r = f.analyze("我々");
    assert_eq!(r.tokens.len(), 2);
    assert_eq!(r.tokens[1].surface, "々");
    assert_eq!(r.tokens[1].reading, "ワレ");
}

#[test]
fn analyze_odoriji_loses_to_jukugo_when_dict_has_explicit_entry() {
    // dict に 「神々」 = カミガミ を登録すると、 jukugo (band 1000) が
    // 「神」+「々」 (band 100 × 2) を上回り、 単一 token に
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("神々", "カミガミ");
    f.add_reading("神", "カミ");
    let r = f.analyze("神々");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].surface, "神々");
    assert_eq!(r.tokens[0].reading, "カミガミ");
}

#[test]
fn analyze_candidates_include_all_overlapping_entries() {
    // 同位置で jukugo + unihan が両方候補に上がる (path 採択は jukugo 勝ち)
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("灰桜", "ハイザクラ");
    f.add_reading("灰", "ハイ");
    let r = f.analyze("灰桜");
    // 採択 path は 「灰桜」 1 token
    assert_eq!(r.tokens.len(), 1);
    // candidates[0] には dict 由来の 「灰桜」 + 「灰」 の両方が上がる
    let pos0_surfaces: Vec<&str> = r.candidates[0].iter().map(|c| c.surface.as_str()).collect();
    assert!(pos0_surfaces.contains(&"灰桜"));
    assert!(pos0_surfaces.contains(&"灰"));
}

// ─── analyze() (C3) tests: NumberCandidateProvider 統合 ────────────────────

fn fixture_rules_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("rules")
}

#[test]
fn analyze_number_counter_path_uses_band_950() {
    // fixture rules 経由で counter regex が effective、 「3本」 が NumberProvider 経由で path に乗る
    let f = Furigana::builder()
        .rules_dir(fixture_rules_dir())
        .build()
        .expect("build with rules_dir");
    let r = f.analyze("3本");
    assert_eq!(r.tokens.len(), 1, "expected single counter token: {r:?}");
    assert_eq!(r.tokens[0].surface, "3本");
    assert_eq!(r.tokens[0].reading, "サンボン");
}

#[test]
fn analyze_number_si_unit_beats_alphabet_passthrough_for_mixed_surface() {
    // 「100km」: AlphabetPassthrough が pure passthrough だと band 100 (miss)、
    // NumberProvider の SI candidate (band 950) が勝つ
    let f = Furigana::builder()
        .rules_dir(fixture_rules_dir())
        .build()
        .expect("build with rules_dir");
    let r = f.analyze("100km");
    assert_eq!(r.tokens.len(), 1, "expected single SI token: {r:?}");
    assert_eq!(r.tokens[0].surface, "100km");
    // 100 + カ行の単位 は促音化する (★2026-09-15: ヒャク → ヒャッ)。
    assert_eq!(r.tokens[0].reading, "ヒャッキロメートル");
}

#[test]
fn analyze_pure_digit_uses_number_provider_not_alphabet() {
    // 「100」 のみ: AlphabetPassthrough miss は band 100、 NumberProvider digit は band 950 → 後者が勝つ
    let f = Furigana::builder()
        .rules_dir(fixture_rules_dir())
        .build()
        .expect("build with rules_dir");
    let r = f.analyze("100");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].surface, "100");
    assert_eq!(r.tokens[0].reading, "ヒャク");
}

#[test]
fn analyze_dict_entry_overrides_number_provider_for_counter_surface() {
    // dict に 「3本」 = カスタム読み を入れると band 1000 で NumberProvider 950 を override
    let mut f = Furigana::builder()
        .rules_dir(fixture_rules_dir())
        .build()
        .expect("build with rules_dir");
    f.add_reading("3本", "ミホン");
    let r = f.analyze("3本");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(
        r.tokens[0].reading, "ミホン",
        "dict 1000 が special 950 に勝つ"
    );
}

#[test]
fn analyze_date_full_pattern_emits_single_token() {
    let f = Furigana::builder()
        .rules_dir(fixture_rules_dir())
        .build()
        .expect("build with rules_dir");
    let r = f.analyze("2025年10月30日");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].surface, "2025年10月30日");
    assert!(
        r.tokens[0].reading.contains("ジュウガツ"),
        "reading: {}",
        r.tokens[0].reading,
    );
}

#[test]
fn analyze_numeric_phrase_emits_single_token() {
    // ★0.2.0 残件再統合: numeric_phrases (二十歳=ハタチ) が NumberCandidateProvider
    // (band 950) 経由で 1 token になり、 Lindera 分解 (二十+歳) に勝つ。
    let f = Furigana::builder()
        .rules_dir(fixture_rules_dir())
        .build()
        .expect("build with rules_dir");
    let r = f.analyze("二十歳");
    assert_eq!(r.tokens.len(), 1, "expected single phrase token: {r:?}");
    assert_eq!(r.tokens[0].surface, "二十歳");
    assert_eq!(r.tokens[0].reading, "ハタチ");

    // 非数字先頭の慣用語句 (明後日) も同様に 1 token
    let r2 = f.analyze("明後日");
    assert_eq!(r2.tokens.len(), 1, "expected single phrase token: {r2:?}");
    assert_eq!(r2.tokens[0].reading, "アサッテ");
}

#[test]
fn analyze_dict_entry_overrides_numeric_phrase() {
    // dict 完全一致 (band 1000) は phrase (band 950) を上書きできる
    let mut f = Furigana::builder()
        .rules_dir(fixture_rules_dir())
        .build()
        .expect("build with rules_dir");
    f.add_reading("二十歳", "ニジュッサイ");
    let r = f.analyze("二十歳");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(
        r.tokens[0].reading, "ニジュッサイ",
        "dict 1000 が phrase 950 に勝つ"
    );
}

#[test]
fn analyze_minimal_falls_back_to_lindera_for_counter_when_rules_empty() {
    // alpha.13 以前: minimal() = 空 RulesData → counter regex None で 「3本」 の
    // 「本」 を覆える provider 無し、 path 構築不能だった。
    // alpha.13+ : Lindera fallback が「3」「本」 を band 50 で edge 化、 path 構築成功。
    let f = Furigana::minimal().unwrap();
    let r = f.analyze("3本");
    assert!(
        !r.tokens.is_empty(),
        "Lindera fallback should provide edges: {r:?}"
    );
    let total_len: usize = r.tokens.iter().map(|t| t.range.end - t.range.start).sum();
    assert_eq!(total_len, "3本".len());
}

// ─── analyze() (★A2 alpha.12) DictBridge MatchCondition 評価 tests ─────

/// Detailed entry を含む dict を temp file 経由で build する helper。
/// Furigana の内部 dict 構築経路は file load なので、 unit test で
/// rich field を inject するために temp TOML を書き出して dir 経由 load する。
fn build_with_dict_toml(toml_body: &str) -> Furigana {
    let dir = std::env::temp_dir().join(format!(
        "furigana_dict_bridge_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let dict_file = dir.join("test.toml");
    std::fs::write(
        &dict_file,
        format!(
            "[meta]\nschema_version = \"2\"\nrole = \"jukugo\"\n\n{}",
            toml_body
        ),
    )
    .unwrap();
    Furigana::builder()
        .core_dict_dir(&dir)
        .build()
        .expect("build with detailed entry")
}

#[test]
fn dict_bridge_evaluates_inline_match_default_reading() {
    // 「上手」 単独 (= 文脈 「から」 無し) → default 「ジョウズ」
    let f = build_with_dict_toml(
        r#"[entries."上手"]
reading = "ジョウズ"

[[entries."上手".match]]
next_eq = "から"
reading = "カミテ"
"#,
    );
    let r = f.analyze("上手");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].reading, "ジョウズ");
}

#[test]
fn dict_bridge_evaluates_inline_match_with_next_eq() {
    // 「上手から」 → 文脈 「から」 match → reading 「カミテ」
    // path 全 cover のため 「から」 も dict に Simple で追加
    let f = build_with_dict_toml(
        r#"[entries]
"から" = "カラ"

[entries."上手"]
reading = "ジョウズ"

[[entries."上手".match]]
next_eq = "から"
reading = "カミテ"
"#,
    );
    let r = f.analyze("上手から");
    let kamite_token = r.tokens.iter().find(|t| t.surface == "上手");
    assert!(
        kamite_token.is_some(),
        "expected 「上手」 token in path: {r:?}"
    );
    assert_eq!(kamite_token.unwrap().reading, "カミテ");
}

#[test]
fn dict_bridge_evaluates_match_with_prev_eq() {
    // 「下上手」 → 「下」 (= 単漢字 dict entry あり) + 「上手」 (= prev_eq "下" → シタテ)
    let f = build_with_dict_toml(
        r#"[entries]
"下" = "シタ"

[entries."上手"]
reading = "ジョウズ"

[[entries."上手".match]]
prev_eq = "下"
reading = "シタテ"
"#,
    );
    let r = f.analyze("下上手");
    let jouzu_token = r.tokens.iter().find(|t| t.surface == "上手");
    assert!(jouzu_token.is_some(), "expected 「上手」 token: {r:?}");
    assert_eq!(jouzu_token.unwrap().reading, "シタテ");
}

// ─── analyze() (★A2 alpha.12) [[kanji]] block 評価 tests ───────────────

/// [[kanji]] block (role = "kanji") を含む dict を temp file 経由で build する helper。
fn build_with_kanji_toml(toml_body: &str) -> Furigana {
    let dir = std::env::temp_dir().join(format!(
        "furigana_kanji_block_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let dict_file = dir.join("test.toml");
    std::fs::write(
        &dict_file,
        format!(
            "[meta]\nschema_version = \"2\"\nrole = \"kanji\"\n\n{}",
            toml_body
        ),
    )
    .unwrap();
    Furigana::builder()
        .core_dict_dir(&dir)
        .build()
        .expect("build with [[kanji]] block")
}

#[test]
fn dict_bridge_evaluates_kanji_block_default_reading() {
    // 「生」 単独 → default 「セイ」 (= 文脈 「じる」 無し / prev 漢字ではない)
    let f = build_with_kanji_toml(
        r#"[[kanji]]
char = "生"
default = "セイ"

[[kanji.match]]
next_eq = "じる"
reading = "ショウ"
"#,
    );
    let r = f.analyze("生");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].reading, "セイ");
}

#[test]
fn dict_bridge_evaluates_kanji_block_with_next_eq() {
    // 「生じる」 → 「生」 が next_eq "じる" match → 「ショウ」
    // [[kanji]] block の match condition が DictBridge で正しく評価される
    // ことを確認。 ただし Lindera が「生じる」 を 1 token として返す場合
    // (alpha.18 で 活用形 band up)、 1 edge path が勝つ可能性あり。
    // 「生」 単体 reading を確実に評価するため、 後続の context-only
    // 検証で final output 「ショウジル」 が含まれることを check。
    let f = build_with_kanji_toml(
        r#"[entries]
"じる" = "ジル"

[[kanji]]
char = "生"
default = "セイ"

[[kanji.match]]
next_eq = "じる"
reading = "ショウ"
"#,
    );
    let r = f.analyze("生じる");
    // path の reading 全体 (= 各 token の reading 結合) が 「ショウジル」 で始まる
    // ことを check (= 「生」 が 「ショウ」、 「じる」 が 「ジル」 / 「生じる」 1 token も同等)
    let combined: String = r.tokens.iter().map(|t| t.reading.as_str()).collect();
    assert_eq!(combined, "ショウジル", "result: {r:?}");
}

// ─── ADR-0004: ambiguous / alternatives ─────────────────────────────────

#[test]
fn analyze_entry_with_alt_shows_ambiguous() {
    let mut f = Furigana::minimal().unwrap();
    f.merge_dict_toml(
        r#"
[meta]
schema_version = "2"
role = "jukugo"

[entries."上手"]
reading = "ジョウズ"

[[entries."上手".alt]]
reading = "カミテ"
weight = 30
"#,
    )
    .unwrap();
    let r = f.analyze("上手");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens[0].reading, "ジョウズ");
    assert!(r.tokens[0].ambiguous);
    assert_eq!(r.tokens[0].alternatives.len(), 1);
    assert_eq!(r.tokens[0].alternatives[0].reading, "カミテ");
    assert_eq!(r.tokens[0].alternatives[0].weight, 30);
}

#[test]
fn analyze_entry_without_alt_not_ambiguous() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("猫", "ネコ");
    let r = f.analyze("猫");
    assert!(!r.tokens[0].ambiguous);
    assert!(r.tokens[0].alternatives.is_empty());
}

#[test]
fn to_accent_with_alt_includes_alternatives() {
    let mut f = Furigana::minimal().unwrap();
    f.merge_dict_toml(
        r#"
[meta]
schema_version = "2"
role = "jukugo"

[entries."上手"]
reading = "ジョ]ウズ"

[[entries."上手".alt]]
reading = "カ]ミテ"
weight = 30
"#,
    )
    .unwrap();
    let r = f.to_accent("上手");
    assert!(r.tokens[0].ambiguous);
    assert_eq!(r.tokens[0].alternatives[0].reading, "カミテ");
}

#[test]
fn loanwords_digits_table_is_loaded_from_core_dict_dir() {
    let dir = std::env::temp_dir().join(format!(
        "furigana_digits_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("loanwords")).unwrap();
    std::fs::write(
        dir.join("loanwords/digits.toml"),
        "[meta]
schema_version = \"2\"
role = \"loanwords\"

[entries]
\"Windows\" = \"ウィンドウズ\"
\"no\" = \"ノー\"

[digits]
\"Windows\" = \"english\"
\"No\" = { style = \"english\", reading = \"ナンバー\" }
",
    )
    .unwrap();
    let f = Furigana::builder().core_dict_dir(&dir).build().unwrap();
    // [entries] の読みと [digits] の読み方が使われる
    assert_eq!(f.to_hiragana("Windows11"), "ウィンドウズイレブン");
    assert_eq!(f.to_hiragana("Windows 10"), "ウィンドウズテン");
    // 読みの上書きは 「語 + 数字」 の時だけ。 単独の No は [entries] の読み
    assert_eq!(f.to_hiragana("No.1"), "ナンバーワン");
    assert_eq!(f.to_hiragana("no"), "ノー");
    // [digits] に無い語は素通し
    assert_eq!(f.to_hiragana("Linux11"), "Linux11");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn loanwords_sentence_words_are_loaded_under_a_prefixed_key() {
    let dir = std::env::temp_dir().join(format!(
        "furigana_sw_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("loanwords")).unwrap();
    std::fs::write(
        dir.join("loanwords/sw.toml"),
        r#"[meta]
schema_version = "2"
role = "loanwords"

[entries]
"thank" = "サンク"

[sentence_words]
"you" = "ユー"
"don't" = "ドント"
"#,
    )
    .unwrap();
    let mut out = HashMap::new();
    load_loanwords_into(&mut out, &dir.join("loanwords"), &[]).unwrap();
    assert_eq!(out.get("thank").map(String::as_str), Some("サンク"));
    assert_eq!(out.get("#sw:you").map(String::as_str), Some("ユー"));
    assert_eq!(out.get("#sw:don't").map(String::as_str), Some("ドント"));
    // 単独の語としては引かれない (接頭辞なしの key には入らない)
    assert!(!out.contains_key("you"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn loanwords_digits_table_rejects_unknown_style() {
    let dir = std::env::temp_dir().join(format!(
        "furigana_digits_bad_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("loanwords")).unwrap();
    std::fs::write(
        dir.join("loanwords/digits.toml"),
        "[meta]
schema_version = \"2\"
role = \"loanwords\"

[digits]
\"Windows\" = \"nope\"
",
    )
    .unwrap();
    let err = Furigana::builder().core_dict_dir(&dir).build().err();
    assert!(err.is_some_and(|e| e.to_string().contains("未知")));
    let _ = std::fs::remove_dir_all(&dir);
}

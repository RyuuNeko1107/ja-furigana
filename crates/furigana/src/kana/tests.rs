use super::normalize::is_plain_char;
use super::normalize::normalize_text_aligned_slow;
use super::*;
use crate::rules::CompatData;
use std::collections::HashMap;

/// `is_plain_char` を直接固定する (素通り判定そのもの + BMP 境界 U+10000)。
#[test]
fn is_plain_char_direct() {
    let compat = make_compat(&[("髙", "高")]);
    let set: crate::fxmap::FxSet<char> = ['髙'].into_iter().collect();
    for cs in [Some(&set), None] {
        // 普通の文字 (2 回目はキャッシュ経由でも同じ)
        for _ in 0..2 {
            assert!(is_plain_char('あ', &compat, cs));
            assert!(is_plain_char('A', &compat, cs));
            assert!(is_plain_char('\u{1_0000}', &compat, cs), "BMP 直後の補助面");
            assert!(is_plain_char('\u{2_0000}', &compat, cs));
            // NFKC で変わる / compat 登録 / VS / 不可視
            assert!(!is_plain_char('Ａ', &compat, cs));
            assert!(!is_plain_char('ﾋ', &compat, cs));
            assert!(!is_plain_char('髙', &compat, cs));
            assert!(!is_plain_char('\u{FE0F}', &compat, cs));
            assert!(!is_plain_char('\u{200B}', &compat, cs));
        }
    }
}

/// 素通りの経路が、 1 文字ずつの本来の経路と全く同じ結果を出すこと (BMP 全部 + 補助面の一部、 前後に文字を挟んで)。
#[test]
fn plain_fast_path_matches_slow_path_for_every_char() {
    let compat = make_compat(&[("髙", "高"), ("﨑", "崎")]);
    let set: crate::fxmap::FxSet<char> = ['髙', '﨑'].into_iter().collect();
    let ranges = [
        0u32..0x1_0000,
        0x1_F000..0x1_FB00,
        0x2_0000..0x2_0100,
        0xE_0000..0xE_0200,
    ];
    for r in ranges {
        for cp in r {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            for s in [format!("{c}"), format!("あ{c}い"), format!("ﾋ{c}")] {
                let fast = normalize_text_aligned_with(&s, &compat, Some(&set));
                let fast_none = normalize_text_aligned_with(&s, &compat, None);
                let slow = normalize_text_aligned_slow(&s, &compat);
                assert_eq!(fast.text, slow.text, "text U+{cp:04X}");
                assert_eq!(fast.changed, slow.changed, "changed U+{cp:04X}");
                assert_eq!(fast_none.text, slow.text, "text(None) U+{cp:04X}");
            }
        }
    }
}

// ─── kata_to_hira / hira_to_kata ──────────────────────────────

#[test]
fn kata_to_hira_small_ka_ke() {
    // ★2026-09-13: カタカナ範囲定数は ン までなので ヵ / ヶ が素通りしていた。
    // 小書きにせず大きい か / け (TTS が読める字)。 ケ の代用 (スヶベ) が大半
    assert_eq!(kata_to_hira("ヵ"), "か");
    assert_eq!(kata_to_hira("ヶ"), "け");
    assert_eq!(kata_to_hira("スヶベ"), "すけべ");
    assert_eq!(kata_to_hira("マイヶル"), "まいける");
    assert!(!kata_to_hira("ヵヶ").contains(['ゕ', 'ゖ']));
    assert_eq!(kata_to_hira("ヴ"), "ゔ");
}

#[test]
fn kata_to_hira_basic() {
    assert_eq!(kata_to_hira("ヨム"), "よむ");
    assert_eq!(kata_to_hira("トウキョウ"), "とうきょう");
    assert_eq!(kata_to_hira("ヴァイオリン"), "ゔぁいおりん");
}

#[test]
fn kata_to_hira_passthrough() {
    assert_eq!(kata_to_hira("漢字"), "漢字");
    assert_eq!(kata_to_hira("hello123"), "hello123");
    assert_eq!(kata_to_hira(""), "");
}

#[test]
fn kata_to_hira_keeps_long_mark_and_punct() {
    assert_eq!(kata_to_hira("コーヒー・ラテ"), "こーひー・らて");
}

#[test]
fn hira_to_kata_basic() {
    assert_eq!(hira_to_kata("よむ"), "ヨム");
    assert_eq!(hira_to_kata("とうきょう"), "トウキョウ");
    assert_eq!(hira_to_kata("ゔぁ"), "ヴァ");
}

#[test]
fn round_trip_kata_hira() {
    let original = "アイウエオカキクケコ";
    assert_eq!(hira_to_kata(&kata_to_hira(original)), original);
}

// ─── 単文字判定 ───────────────────────────────────────────────

#[test]
fn is_hiragana_char_works() {
    assert!(is_hiragana_char('あ'));
    assert!(is_hiragana_char('ん'));
    assert!(is_hiragana_char('ゔ'));
    assert!(!is_hiragana_char('ア'));
    assert!(!is_hiragana_char('a'));
}

#[test]
fn is_katakana_char_works() {
    assert!(is_katakana_char('ア'));
    assert!(is_katakana_char('ン'));
    assert!(is_katakana_char('ヴ'));
    assert!(!is_katakana_char('あ'));
    assert!(!is_katakana_char('a'));
}

#[test]
fn is_kanji_char_works() {
    assert!(is_kanji_char('漢'));
    assert!(is_kanji_char('東'));
    assert!(is_kanji_char('々'));
    assert!(is_kanji_char('〆'));
    assert!(is_kanji_char('ヶ'));
    assert!(!is_kanji_char('あ'));
    assert!(!is_kanji_char('a'));
}

// ─── has_kanji / has_katakana ───────────────────────────────

#[test]
fn has_kanji_works() {
    assert!(has_kanji("読む"));
    assert!(has_kanji("東京タワー"));
    assert!(has_kanji("々"));
    assert!(!has_kanji("よむ"));
    assert!(!has_kanji("カタカナ"));
    assert!(!has_kanji(""));
}

#[test]
fn has_katakana_works() {
    assert!(has_katakana("カタカナ"));
    assert!(has_katakana("漢字とカナ"));
    assert!(has_katakana("コーヒー"));
    assert!(!has_katakana("ひらがな"));
    assert!(!has_katakana("漢字"));
}

// ─── pure 判定 ────────────────────────────────────────────────

#[test]
fn is_pure_katakana_works() {
    assert!(is_pure_katakana("カタカナ"));
    assert!(is_pure_katakana("タワー"));
    assert!(is_pure_katakana("コーヒー・ラテ"));
    assert!(!is_pure_katakana("漢字"));
    assert!(!is_pure_katakana("ひらがな"));
    assert!(!is_pure_katakana(""));
    assert!(!is_pure_katakana("カナと漢字"));
}

#[test]
fn is_pure_hiragana_works() {
    assert!(is_pure_hiragana("ひらがな"));
    assert!(is_pure_hiragana("ゔぁい"));
    assert!(!is_pure_hiragana("カタカナ"));
    assert!(!is_pure_hiragana(""));
    assert!(!is_pure_hiragana("ひらと漢字"));
}

// ─── zen_to_han ──────────────────────────────────────────────

#[test]
fn zen_to_han_digits_and_symbols() {
    assert_eq!(zen_to_han("１２３"), "123");
    assert_eq!(zen_to_han("５０％"), "50%");
    assert_eq!(zen_to_han("Ａ＋Ｂ"), "A+B");
    assert_eq!(zen_to_han("ｈｅｌｌｏ"), "hello");
    assert_eq!(zen_to_han("１．５"), "1.5");
}

#[test]
fn zen_to_han_passthrough() {
    // 漢字・カナはそのまま
    assert_eq!(zen_to_han("漢字"), "漢字");
    assert_eq!(zen_to_han("カナ"), "カナ");
}

// ─── normalize_text ──────────────────────────────────────────

fn make_compat(pairs: &[(&str, &str)]) -> CompatData {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(v, c)| ((*v).to_string(), (*c).to_string()))
        .collect();
    CompatData { map }
}

#[test]
fn normalize_text_replaces_variants() {
    let compat = make_compat(&[("髙", "高"), ("﨑", "崎")]);
    assert_eq!(normalize_text("髙﨑", &compat), "高崎");
}

#[test]
fn normalize_text_keeps_unmapped() {
    let compat = make_compat(&[]);
    assert_eq!(normalize_text("こんにちは", &compat), "こんにちは");
}

#[test]
fn normalize_text_strips_variation_selectors() {
    let compat = make_compat(&[]);
    // IVS / 異体字セレクタ付き漢字は base char に正規化され dict lookup できる。
    // NFKC/NFC は VS を剥がさないので明示除去が要る。人名 (葛飾/辻 等) で頻出。
    assert_eq!(normalize_text("葛\u{E0100}飾", &compat), "葛飾"); // IVS (VS Supplement)
    assert_eq!(normalize_text("辻\u{FE00}", &compat), "辻"); // VS1 (BMP)
}

#[test]
fn normalize_text_turns_roman_numerals_into_digits() {
    let c = CompatData::default();
    assert_eq!(normalize_text("Ⅱ世", &c), "2世");
    assert_eq!(normalize_text("第Ⅲ期", &c), "第3期");
    assert_eq!(normalize_text("ドラクエⅤ", &c), "ドラクエ5");
    assert_eq!(normalize_text("ⅰⅻ", &c), "112");
    // 範囲の両端: Ⅰ(U+2160) と Ⅻ(U+216B) は変換、 Ⅼ(U+216C) 以降は従来どおり NFKC
    assert_eq!(normalize_text("\u{2160}\u{216B}", &c), "112");
    assert_eq!(normalize_text("\u{216C}", &c), "L");
}

#[test]
fn normalize_text_separates_circled_numbers_and_turns_superscripts_into_powers() {
    let c = CompatData::default();
    // 丸数字は連続した時だけ空白で区切る (①②③ が 123 にならない)。 1 つだけなら従来どおり
    assert_eq!(normalize_text("①②③", &c), "1・2・3");
    assert_eq!(normalize_text("①番", &c), "1番");
    assert_eq!(normalize_text("⑩⑪", &c), "10・11");
    // 上付き数字は直前が数字・英字・閉じ括弧の時だけ累乗にする
    assert_eq!(normalize_text("10²", &c), "10^2");
    assert_eq!(normalize_text("x²+y³", &c), "x^2+y^3");
    assert_eq!(normalize_text("(a+b)²", &c), "(a+b)^2");
    assert_eq!(normalize_text("注釈¹", &c), "注釈1");
    // 漢字の直後の ² ³ は 「の2乗」 (身長²)。 脚注の 注² は数字
    assert_eq!(normalize_text("身長²", &c), "身長の2乗");
    assert_eq!(normalize_text("注²", &c), "注2");
    // 底が切れた ² (「²-5x+b=0」) は、 後ろが演算子なら指数のまま残す。 注記の ² は従来どおり数字
    assert_eq!(normalize_text("²-5x+b=0", &c), "^2-5x+b=0");
    assert_eq!(normalize_text("注²。", &c), "注2。");
    // 上付きのマイナス・複数桁の指数
    assert_eq!(normalize_text("10⁻³=0.001", &c), "10^-3=0.001");
    assert_eq!(normalize_text("x⁻¹", &c), "x^-1");
    assert_eq!(normalize_text("x¹⁰", &c), "x^10");
    // 数式の下付き数字は、 式の記号がある行の 小文字 1 字 / log の直後だけ残す (H₂O / CO₂ は数になる)
    assert_eq!(normalize_text("log₂8=3", &c), "log₂8=3");
    assert_eq!(normalize_text("a₁+a₂=b₁₀", &c), "a₁+a₂=b₁₀");
    assert_eq!(normalize_text("H₂O", &c), "H2O");
    assert_eq!(normalize_text("CO₂ = x", &c), "CO2 = x");
    assert_eq!(normalize_text("x₁を使う", &c), "x1を使う");
}

#[test]
fn normalize_text_composes_halfwidth_voiced_marks_and_drops_invisible_chars() {
    let c = CompatData::default();
    assert_eq!(normalize_text("ﾋﾞｰﾙ", &c), "ビール");
    assert_eq!(normalize_text("ﾊﾟｿｺﾝ", &c), "パソコン");
    // 合成できない組 (ｱﾞ) は結合記号のまま残す
    assert!(normalize_text("ｱﾞ", &c).starts_with('ア'));
    // 不可視文字は除く (表示用の surface は alignment で元のまま)
    assert_eq!(normalize_text("漢\u{200B}字", &c), "漢字");
    assert_eq!(normalize_text("\u{FEFF}先頭", &c), "先頭");
    assert_eq!(normalize_text("ソフト\u{AD}ハイフン", &c), "ソフトハイフン");
}

#[test]
fn normalize_text_applies_nfkc() {
    let compat = make_compat(&[]);
    // 全角数字 NFKC → 半角
    assert_eq!(normalize_text("１２３", &compat), "123");
}

#[test]
fn normalize_text_keeps_kaomoji_cheek_marks() {
    // ´ ¨ ¯ ˘ は NFKC で 「空白 + 結合記号」 に分解されて顔文字が崩れるので残す
    let compat = make_compat(&[]);
    for s in ["(´・ω・`)", "¨", "¯", "˘"] {
        assert_eq!(normalize_text(s, &compat), s);
    }
    // 他の互換文字は従来どおり分解される
    assert_eq!(normalize_text("ｱ", &compat), "ア");
}

#[test]
fn normalize_text_expands_multichar_canonical() {
    // canonical が複数文字の compat (旧字漢数字 廿=二十 / 卅=三十) を完全展開する
    // (旧実装は先頭 1 文字のみで 廿→二 と十を落としていた)。
    let compat = make_compat(&[("廿", "二十"), ("卅", "三十")]);
    assert_eq!(normalize_text("廿日", &compat), "二十日");
    assert_eq!(normalize_text("卅", &compat), "三十");
}

#[test]
fn normalize_text_empty() {
    let compat = make_compat(&[]);
    assert_eq!(normalize_text("", &compat), "");
}

#[test]
fn voice_first_kana_covers_rendaku_table() {
    // 先頭かなの連濁 (清音→濁音)。カタカナ・ひらがな両方の各 arm を網羅。
    let pairs = [
        ("カ", "ガ"),
        ("キ", "ギ"),
        ("ク", "グ"),
        ("ケ", "ゲ"),
        ("コ", "ゴ"),
        ("サ", "ザ"),
        ("シ", "ジ"),
        ("ス", "ズ"),
        ("セ", "ゼ"),
        ("ソ", "ゾ"),
        ("タ", "ダ"),
        ("チ", "ヂ"),
        ("ツ", "ヅ"),
        ("テ", "デ"),
        ("ト", "ド"),
        ("ハ", "バ"),
        ("ヒ", "ビ"),
        ("フ", "ブ"),
        ("ヘ", "ベ"),
        ("ホ", "ボ"),
        ("か", "が"),
        ("き", "ぎ"),
        ("く", "ぐ"),
        ("け", "げ"),
        ("こ", "ご"),
        ("さ", "ざ"),
        ("し", "じ"),
        ("す", "ず"),
        ("せ", "ぜ"),
        ("そ", "ぞ"),
        ("た", "だ"),
        ("ち", "ぢ"),
        ("つ", "づ"),
        ("て", "で"),
        ("と", "ど"),
        ("は", "ば"),
        ("ひ", "び"),
        ("ふ", "ぶ"),
        ("へ", "べ"),
        ("ほ", "ぼ"),
    ];
    for (clean, voiced) in pairs {
        assert_eq!(
            voice_first_kana(clean).as_deref(),
            Some(voiced),
            "{clean}→{voiced}"
        );
    }
    // 2 文字目以降は保持
    assert_eq!(voice_first_kana("カミ").as_deref(), Some("ガミ"));
    assert_eq!(voice_first_kana("ひと").as_deref(), Some("びと")); // 人々→ひとびと
                                                                   // 連濁不可 (母音 / ナマヤラワ / 既濁音 / 半濁音 / 空) は None
    for none in [
        "ア", "ナ", "マ", "ヤ", "ラ", "ワ", "ン", "ガ", "パ", "あ", "な", "ん", "",
    ] {
        assert_eq!(voice_first_kana(none), None, "{none} は連濁不可");
    }
}

#[test]
fn normalize_long_vowel_expands_by_vowel_grade() {
    // ー を直前かなの母音段で展開: ア段→ア / イ段→イ / ウ段→ウ / エ段→イ / オ段→ウ。
    assert_eq!(normalize_long_vowel("カー"), "カア"); // a
    assert_eq!(normalize_long_vowel("キー"), "キイ"); // i
    assert_eq!(normalize_long_vowel("クー"), "クウ"); // u
    assert_eq!(normalize_long_vowel("ケー"), "ケイ"); // e → イ
    assert_eq!(normalize_long_vowel("コー"), "コウ"); // o → ウ
    assert_eq!(normalize_long_vowel("サー"), "サア");
    assert_eq!(normalize_long_vowel("スー"), "スウ");
    assert_eq!(normalize_long_vowel("ソー"), "ソウ");
    assert_eq!(normalize_long_vowel("ネー"), "ネイ");
    assert_eq!(normalize_long_vowel("ガッコー"), "ガッコウ"); // 実例
                                                              // 先頭の ー は展開しない (i>0 ガード)、 ー 以外は不変
    assert_eq!(normalize_long_vowel("ーカ"), "ーカ");
    assert_eq!(normalize_long_vowel("ネコ"), "ネコ");
}

#[test]
fn zen_to_han_converts_fullwidth() {
    // 全角英数記号 → 半角 (各 arm を個別に固定)。
    assert_eq!(zen_to_han("ＡＢＣ"), "ABC");
    assert_eq!(zen_to_han("ａｂｃ"), "abc");
    assert_eq!(zen_to_han("１２３"), "123");
    // 記号 arm を網羅: － \u{2212} ＋ ～ 〜 ％ ． ， ／
    assert_eq!(zen_to_han("－"), "-");
    assert_eq!(zen_to_han("\u{2212}"), "-");
    assert_eq!(zen_to_han("＋"), "+");
    assert_eq!(zen_to_han("～"), "~");
    assert_eq!(zen_to_han("〜"), "~");
    assert_eq!(zen_to_han("％"), "%");
    assert_eq!(zen_to_han("．"), ".");
    assert_eq!(zen_to_han("，"), ",");
    assert_eq!(zen_to_han("／"), "/");
    assert_eq!(zen_to_han("漢字"), "漢字"); // 非対象は不変
}

use super::*;
use crate::api::Furigana;

fn phrases(f: &Furigana, input: &str) -> MoraPhrases {
    to_mora_phrases(&f.to_accent(input))
}

fn shape(p: &MoraPhrases) -> Vec<(String, Option<usize>, PhraseBreak)> {
    p.phrases
        .iter()
        .map(|ph| (ph.morae.concat(), ph.nucleus, ph.break_before))
        .collect()
}

#[test]
fn spell_word_names_each_letter() {
    assert_eq!(spell_word("AviUtl"), "エーブイアイユーティーエル");
    assert_eq!(spell_word("abc"), "エービーシー");
    assert_eq!(spell_word("XYZ"), "エックスワイゼット");
    // 英字以外は読まない
    assert_eq!(spell_word("A-1"), "エー");
    assert_eq!(spell_word(""), "");
}

#[test]
fn romaji_word_reads_simple_syllables() {
    let cases = [
        ("AviUtl", "アビウトル"),
        ("kawaii", "カワイイ"),
        ("sakura", "サクラ"),
        ("shimbun", "シンブン"),
        ("kitte", "キッテ"),
        ("kyou", "キョウ"),
        ("chotto", "チョット"),
        ("jyu", "ジュ"),
        ("tsunami", "ツナミ"),
        ("fuji", "フジ"),
        ("yama", "ヤマ"),
        ("hon", "ホン"),
        ("tea", "テア"),
        ("ra", "ラ"),
        ("lamp", "ランプ"),
        ("cat", "カト"),
        ("nyan", "ニャン"),
        ("yuki", "ユキ"),
        ("yoru", "ヨル"),
        ("ash", "アシ"),
        ("bunch", "ブンチ"),
        ("kyu", "キュ"),
        ("rakuda", "ラクダ"),
    ];
    for (w, want) in cases {
        assert_eq!(romaji_word(w), want, "{w}");
    }
    assert_eq!(romaji_word(""), "");
}

#[test]
fn alpha_reading_follows_the_mode() {
    assert_eq!(alpha_reading("Ab", UnknownAlpha::Drop), None);
    assert_eq!(
        alpha_reading("Ab", UnknownAlpha::Spell).as_deref(),
        Some("エービー")
    );
    assert_eq!(
        alpha_reading("sa", UnknownAlpha::Romaji).as_deref(),
        Some("サ")
    );
}

#[test]
fn alpha_split_separates_latin_runs() {
    assert_eq!(
        alpha_split("、AviUtl。ab"),
        vec![
            (false, "、".to_string()),
            (true, "AviUtl".to_string()),
            (false, "。".to_string()),
            (true, "ab".to_string()),
        ]
    );
    assert!(alpha_split("").is_empty());
    // 数字は英字語に含めない
    assert_eq!(
        alpha_split("a1b"),
        vec![
            (true, "a".to_string()),
            (false, "1".to_string()),
            (true, "b".to_string())
        ]
    );
}

#[test]
fn chained_unknown_alpha_modes() {
    use crate::chain_phrases::ChainedPhrase;
    let ph = |sound: &str, accent: usize, gap: &str| ChainedPhrase {
        surface: sound.to_string(),
        reading: sound.to_string(),
        mora: sound.chars().count(),
        accent,
        estimated: false,
        sound: sound.to_string(),
        gap_before: gap.to_string(),
    };
    let spell = SymbolOptions {
        unknown_alpha: UnknownAlpha::Spell,
        ..SymbolOptions::default()
    };
    // 先頭の英字語: 区切りなし。 後ろの句は 記号の分だけ区切りが付く
    let out = chained_to_mora_phrases(&[ph("デス", 1, "AB、")], "", spell);
    assert_eq!(out.unknown, vec!["AB".to_string()]);
    assert_eq!(out.phrases.phrases.len(), 2);
    assert_eq!(out.phrases.phrases[0].morae.concat(), "エービー");
    assert_eq!(out.phrases.phrases[0].nucleus, None);
    assert_eq!(out.phrases.phrases[0].break_before, PhraseBreak::None);
    assert_eq!(out.phrases.phrases[1].break_before, PhraseBreak::Short);
    // 2 句目以降の英字語: 直前の記号が区切りになる
    let out = chained_to_mora_phrases(&[ph("ア", 0, ""), ph("イ", 0, "。C")], "", spell);
    assert_eq!(out.phrases.phrases.len(), 3);
    assert_eq!(out.phrases.phrases[1].morae.concat(), "シー");
    assert_eq!(out.phrases.phrases[1].break_before, PhraseBreak::Long);
    assert_eq!(out.phrases.phrases[2].break_before, PhraseBreak::None);
    // 末尾の英字語と記号
    let out = chained_to_mora_phrases(&[ph("ア", 0, "")], "、D？", spell);
    assert_eq!(out.phrases.phrases.len(), 2);
    assert_eq!(out.phrases.phrases[1].morae.concat(), "ディー");
    assert_eq!(out.phrases.phrases[1].break_before, PhraseBreak::Short);
    assert_eq!(out.phrases.trailing, PhraseBreak::Question);
    // Drop: 語は落ちるが 一覧には残る
    let out = chained_to_mora_phrases(&[ph("デス", 1, "AB、")], "", SymbolOptions::default());
    assert_eq!(out.unknown, vec!["AB".to_string()]);
    assert_eq!(out.phrases.phrases.len(), 1);
    // Romaji
    let romaji = SymbolOptions {
        unknown_alpha: UnknownAlpha::Romaji,
        ..SymbolOptions::default()
    };
    let out = chained_to_mora_phrases(&[ph("デス", 1, "kawaii")], "", romaji);
    assert_eq!(out.phrases.phrases[0].morae.concat(), "カワイイ");
}

#[test]
fn chained_phrases_flat_has_no_nucleus_and_gap_sets_break() {
    use crate::chain_phrases::ChainedPhrase;
    let ph = |sound: &str, accent: usize, gap: &str| ChainedPhrase {
        surface: sound.to_string(),
        reading: sound.to_string(),
        mora: sound.chars().count(),
        accent,
        estimated: false,
        sound: sound.to_string(),
        gap_before: gap.to_string(),
    };
    let m = chained_to_mora_phrases(
        &[ph("アメ", 0, ""), ph("カゼ", 1, "、"), ph("ユキ", 2, "。?")],
        "！",
        SymbolOptions::default(),
    )
    .phrases;
    assert_eq!(m.phrases.len(), 3);
    // 平板 (0) は 核なし、 起伏はその位置
    assert_eq!(m.phrases[0].nucleus, None);
    assert_eq!(m.phrases[1].nucleus, Some(1));
    assert_eq!(m.phrases[2].nucleus, Some(2));
    // 先頭句は常に区切りなし / 記号の並びは強い方
    assert_eq!(m.phrases[0].break_before, PhraseBreak::None);
    assert_eq!(m.phrases[1].break_before, PhraseBreak::Short);
    assert_eq!(m.phrases[2].break_before, PhraseBreak::Question);
    assert_eq!(m.trailing, PhraseBreak::Long);
    // 先頭の句の前に記号があっても 区切りは付けない
    let m = chained_to_mora_phrases(&[ph("アメ", 1, "、")], "", SymbolOptions::default()).phrases;
    assert_eq!(m.phrases[0].break_before, PhraseBreak::None);
    assert_eq!(m.trailing, PhraseBreak::None);
    // 記号列に出せない文字だけの句は作らない
    let m = chained_to_mora_phrases(&[ph("😀", 0, "")], "", SymbolOptions::default()).phrases;
    assert!(m.phrases.is_empty());
}

#[test]
fn mora_split_merges_small_kana() {
    assert_eq!(mora_split("キョウ"), ["キョ", "ウ"]);
    assert_eq!(mora_split("カーテン"), ["カ", "ー", "テ", "ン"]);
    // 先頭に小書きが来ても panic しない (直前が無いので単独モーラ)
    assert_eq!(mora_split("ャ"), ["ャ"]);
    assert!(mora_split("").is_empty());
}

#[test]
fn particle_attaches_to_previous_phrase_keeping_nucleus() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("雨", "ア]メ");
    let p = phrases(&f, "雨が");
    assert_eq!(
        shape(&p),
        [("アメガ".to_string(), Some(1), PhraseBreak::None)]
    );
}

#[test]
fn masu_puts_nucleus_on_masu() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("食べ", "タ]ベ");
    f.add_reading("行き", "イ[キ");
    let nuc = |s: &str| {
        let p = phrases(&f, s);
        (p.phrases[0].morae.concat(), p.phrases[0].nucleus_pos())
    };
    assert_eq!(nuc("食べます"), ("タベマス".to_string(), 3), "タベマ'ス");
    assert_eq!(nuc("行きました").1, 3, "イキマ'シタ");
    assert_eq!(nuc("食べません").1, 4, "タベマセ'ン");
}

#[test]
fn desu_puts_nucleus_on_de_only_after_heiban() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("学生", "ガ[クセイ");
    f.add_reading("雨", "ア]メ");
    let nuc = |s: &str| phrases(&f, s).phrases[0].nucleus_pos();
    assert_eq!(nuc("学生です"), 5, "ガクセイデ'ス");
    assert_eq!(nuc("雨です"), 1, "起伏は元の核 (ア'メデス)");
}

#[test]
fn heiban_phrase_has_no_nucleus() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("魚", "サ[カナ");
    let p = phrases(&f, "魚が");
    assert_eq!(p.phrases[0].nucleus, None);
    // 平板は句末を核位置として扱う
    assert_eq!(p.phrases[0].nucleus_pos(), 4);
}

#[test]
fn ha_he_wo_become_phonetic_even_with_accent_phrases() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("雨", "ア]メ");
    f.add_reading("は", "[ハ]");
    f.add_reading("見る", "ミル");
    assert_eq!(shape(&phrases(&f, "雨は"))[0].0, "アメワ");
    assert_eq!(shape(&phrases(&f, "雨を見る"))[0].0, "アメオ");
}

#[test]
fn punctuation_becomes_break_before_next_phrase() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("雨", "ア]メ");
    f.add_reading("雪", "ユ]キ");
    f.add_reading("風", "カ[ゼ]");
    let p = phrases(&f, "雨、雪。風");
    assert_eq!(
        shape(&p).iter().map(|(_, _, b)| *b).collect::<Vec<_>>(),
        [PhraseBreak::None, PhraseBreak::Short, PhraseBreak::Long]
    );
    assert_eq!(p.trailing, PhraseBreak::None);
}

#[test]
fn question_is_per_sentence_and_trailing_is_kept() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("雨", "ア]メ");
    f.add_reading("雪", "ユ]キ");
    // 文中の `？` は 次の句の break、 文末の `。` は trailing
    let p = phrases(&f, "雨？雪。");
    assert_eq!(p.phrases[1].break_before, PhraseBreak::Question);
    assert_eq!(p.trailing, PhraseBreak::Long);
}

#[test]
fn nakaguro_does_not_pause() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("甲", "コ]ウ");
    f.add_reading("乙", "オ]ツ");
    assert_eq!(
        phrases(&f, "甲・乙").phrases[1].break_before,
        PhraseBreak::None
    );
}

#[test]
fn unreadable_token_is_dropped_but_breaks_phrase() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("雨", "ア]メ");
    let p = phrases(&f, "雨🌧");
    assert_eq!(p.phrases.len(), 1);
    assert_eq!(p.phrases[0].morae.concat(), "アメ");
}

#[test]
fn empty_input_yields_no_phrases() {
    let f = Furigana::minimal().unwrap();
    let p = phrases(&f, "");
    assert!(p.phrases.is_empty());
    assert_eq!(p.trailing, PhraseBreak::None);
}

#[test]
fn multi_phrase_entry_splits_into_phrases() {
    let mut f = Furigana::minimal().unwrap();
    f.add_reading("都立", "[トウキョウ][ト]リツ");
    let p = phrases(&f, "都立");
    assert_eq!(
        shape(&p),
        [
            // `]` 無しの句は句末核として parse される (4 モーラ目 = ウ)
            ("トウキョウ".to_string(), Some(4), PhraseBreak::None),
            ("トリツ".to_string(), Some(1), PhraseBreak::None),
        ]
    );
}

#[test]
fn no_phrase_is_empty() {
    // adapter が 「各句に核 1 つ」 を無条件に満たせることの前提
    let f = Furigana::builder().estimate_accent(true).build().unwrap();
    for input in ["今日は雨が降るかもしれない。", "田中さんが来た！", "…?"]
    {
        for p in to_mora_phrases(&f.to_accent(input)).phrases {
            assert!(!p.morae.is_empty(), "empty phrase for {input:?}");
            assert!(p.nucleus_pos() >= 1, "no nucleus for {input:?}");
            assert!(
                p.morae.iter().all(|m| m.chars().all(is_symbol_kana)),
                "non-kana mora in {input:?}: {:?}",
                p.morae
            );
        }
    }
}

#[test]
fn unknown_words_come_from_alpha_runs_only() {
    use crate::api::{AccentResult, AccentToken};
    let result = AccentResult {
        schema_version: "1".to_string(),
        tokens: vec![AccentToken {
            surface: "AB.C".to_string(),
            reading: "AB.C".to_string(),
            accent_phrases: Vec::new(),
            ambiguous: false,
            alternatives: Vec::new(),
        }],
    };
    let spell = SymbolOptions {
        unknown_alpha: UnknownAlpha::Spell,
        ..SymbolOptions::default()
    };
    let out = to_mora_phrases_with(&result, spell);
    assert_eq!(out.unknown, vec!["AB".to_string(), "C".to_string()]);
    assert_eq!(out.phrases.phrases.len(), 2);
    assert_eq!(out.phrases.phrases[0].morae.concat(), "エービー");
    assert_eq!(out.phrases.phrases[1].morae.concat(), "シー");
    let out = to_mora_phrases_with(&result, SymbolOptions::default());
    assert_eq!(out.unknown.len(), 2);
    assert!(out.phrases.phrases.is_empty());
}

#[test]
fn particles_are_spoken_as_sounds_in_token_results() {
    use crate::api::{AccentResult, AccentToken};
    for (surface, want) in [("は", "ワ"), ("へ", "エ"), ("を", "オ")] {
        let result = AccentResult {
            schema_version: "1".to_string(),
            tokens: vec![AccentToken {
                surface: surface.to_string(),
                reading: hira_to_kata(surface),
                accent_phrases: Vec::new(),
                ambiguous: false,
                alternatives: Vec::new(),
            }],
        };
        let out = to_mora_phrases(&result);
        assert_eq!(out.phrases[0].morae.concat(), want, "{surface}");
    }
}

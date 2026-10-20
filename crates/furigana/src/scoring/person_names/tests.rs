use super::cues::may_have_cue;
use super::cues::token_may_be_cue;
use super::*;
use std::ops::Range;

fn token(surface: &str, reading: &str, range: Range<usize>) -> Token {
    Token {
        surface: surface.to_string(),
        reading: reading.to_string(),
        range,
        accent_phrases: Vec::new(),
        ambiguous: false,
        alternatives: Vec::new(),
        is_name: false,
        band: 0,
        why: None,
    }
}

fn dict() -> Dict {
    let mut d = Dict::new();
    d.insert_name(true, "水田", "みずた");
    d.insert_name(true, "橋本", "はしもと");
    d.insert_name(true, "鳥", "とり");
    d.insert_name(false, "雄一郎", "ゆういちろう");
    d.insert_name(false, "環奈", "かんな");
    d
}

#[test]
fn surname_with_title_is_read_as_name() {
    let d = dict();
    let mut t = vec![
        token("水田", "スイデン", 0..6),
        token("選手", "センシュ", 6..12),
        token("が", "ガ", 12..15),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t[0].reading, "ミズタ");
    assert!(t[0].is_name);
    assert_eq!(t[0].alternatives[0].reading, "スイデン");
    assert_eq!(t.len(), 3);
}

/// 位置 (byte) を連続させて token 列を作る
fn chain(parts: &[(&str, &str)]) -> Vec<Token> {
    let mut pos = 0;
    let mut out = Vec::new();
    for (s, r) in parts {
        out.push(token(s, r, pos..pos + s.len()));
        pos += s.len();
    }
    out
}

#[test]
fn several_names_in_one_sentence_are_all_converted() {
    let d = dict();
    // 前置きの語 2 つ + 姓 名 敬称 (4 token → 3 token に減る) + 助詞 + 姓 敬称 + 姓 敬称
    let mut t = chain(&[
        ("ああ", "アア"),
        ("ええ", "エエ"),
        ("水田", "スイデン"),
        ("雄", "ユウ"),
        ("一郎", "イチロウ"),
        ("さん", "サン"),
        ("と", "ト"),
        ("水田", "スイデン"),
        ("さん", "サン"),
        ("と", "ト"),
        ("橋本", "ハシモト"),
        ("環奈", "タマキナ"),
        ("さん", "サン"),
    ]);
    PersonNamePass::new(&d).apply(&mut t);
    let surfaces: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
    assert_eq!(
        surfaces,
        [
            "ああ",
            "ええ",
            "水田",
            "雄一郎",
            "さん",
            "と",
            "水田",
            "さん",
            "と",
            "橋本",
            "環奈",
            "さん"
        ]
    );
    assert_eq!(t[2].reading, "ミズタ");
    assert_eq!(t[3].reading, "ユウイチロウ");
    assert_eq!(t[6].reading, "ミズタ");
    assert_eq!(t[9].reading, "ハシモト");
    assert_eq!(t[10].reading, "カンナ");
}

#[test]
fn surname_and_given_with_honorific_is_split() {
    let d = dict();
    // 水田 | 雄一郎 | さん (Lindera が 雄一郎 を 1 字ずつ割った想定も含め、 結合して引く)
    let mut t = vec![
        token("水田", "スイデン", 0..6),
        token("雄", "ユウ", 6..9),
        token("一郎", "イチロウ", 9..15),
        token("さん", "サン", 15..21),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    let surfaces: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
    assert_eq!(surfaces, ["水田", "雄一郎", "さん"]);
    assert_eq!(t[0].reading, "ミズタ");
    assert_eq!(t[1].reading, "ユウイチロウ");
    assert_eq!(t[1].range, 6..15);
}

#[test]
fn no_cue_leaves_common_word_alone() {
    let d = dict();
    let mut t = vec![
        token("水田", "スイデン", 0..6),
        token("に", "ニ", 6..9),
        token("水", "ミズ", 9..12),
    ];
    let before: Vec<String> = t.iter().map(|x| x.reading.clone()).collect();
    PersonNamePass::new(&d).apply(&mut t);
    let after: Vec<String> = t.iter().map(|x| x.reading.clone()).collect();
    assert_eq!(before, after);
}

#[test]
fn common_word_given_name_is_not_a_name() {
    let mut d = dict();
    d.insert_name(true, "今日", "きょう");
    d.insert_name(false, "新人", "あらと");
    // 今日 + 新人 + さん: 新人 は一般語なので あらと にしない
    let mut t = vec![
        token("今日", "キョウ", 0..6),
        token("新人", "シンジン", 6..12),
        token("さん", "サン", 12..18),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t[1].reading, "シンジン");
}

#[test]
fn title_followed_by_a_group_noun_is_not_a_cue() {
    let d = dict();
    // 日野市医師会: 医師 + 会 は医師会で、 市 を名 (いち) にしない
    let mut t = vec![
        token("日野", "ヒノ", 0..6),
        token("市", "シ", 6..9),
        token("医師", "イシ", 9..15),
        token("会", "カイ", 15..18),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t[1].reading, "シ");
}

#[test]
fn compound_title_is_not_a_cue() {
    let d = dict();
    // 選手権 は肩書きではない (水田選手権 は人名にしない)
    let mut t = vec![
        token("水田", "スイデン", 0..6),
        token("選手権", "センシュケン", 6..15),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t[0].reading, "スイデン");
}

#[test]
fn single_char_surname_alone_is_not_converted() {
    let d = dict();
    // 鳥さん は一般語と区別できないので触らない
    let mut t = vec![token("鳥", "チョウ", 0..3), token("さん", "サン", 3..9)];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t[0].reading, "チョウ");
}

#[test]
fn full_name_in_dict_wins_over_tables() {
    let mut d = dict();
    d.insert_name(true, "江川", "えがわ");
    d.insert_name(false, "卓", "たかし");
    d.insert("江川卓", "エガワスグル");
    let mut t = vec![
        token("江川卓", "エガワスグル", 0..9),
        token("投手", "トウシュ", 9..15),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t[0].reading, "エガワスグル");
    assert_eq!(t.len(), 2);
}

#[test]
fn dzu_zu_difference_alone_is_not_replaced() {
    let mut d = dict();
    d.insert_name(true, "望月", "もちずき");
    let mut t = vec![
        token("望月", "モチヅキ", 0..6),
        token("先生", "センセイ", 6..12),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t[0].reading, "モチヅキ", "づ/ず だけの違いは現行を残す");
    assert!(!t[0].is_name);
}

#[test]
fn two_char_run_is_surname_only_not_split() {
    let mut d = dict();
    d.insert_name(true, "久", "ひさ");
    d.insert_name(false, "保", "たもつ");
    d.insert_name(true, "久保", "くぼ");
    let mut t = vec![token("久保", "クボ", 0..6), token("くん", "クン", 6..12)];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t[0].reading, "クボ");
    assert_eq!(t.len(), 2);
}

#[test]
fn name_hint_reads_without_cue_and_allows_single_char_surname() {
    let d = dict();
    let p = PersonNamePass::new(&d).with_hint(NameHint::Name);
    let mut t = vec![
        token("水田", "スイデン", 0..6),
        token("雄一郎", "ユウイチロウ", 6..15),
    ];
    p.apply(&mut t);
    assert_eq!(t[0].reading, "ミズタ");
    let mut one = vec![token("鳥", "チョウ", 0..3)];
    p.apply(&mut one);
    assert_eq!(one[0].reading, "トリ");
    // 文の中の人名でない部分には触れない (漢字列の外)
    let mut s = vec![
        token("広い", "ヒロイ", 0..6),
        token("水田", "スイデン", 6..12),
        token("に", "ニ", 12..15),
    ];
    p.apply(&mut s);
    assert_eq!(
        s[1].reading, "ミズタ",
        "Name ヒントでは文中の漢字列も人名として読む (欄が人名と言っている)"
    );
}

#[test]
fn not_name_hint_disables_the_pass() {
    let d = dict();
    let p = PersonNamePass::new(&d).with_hint(NameHint::NotName);
    let mut t = vec![
        token("水田", "スイデン", 0..6),
        token("選手", "センシュ", 6..12),
    ];
    p.apply(&mut t);
    assert_eq!(t[0].reading, "スイデン");
}

#[test]
fn whole_three_char_surname_beats_surname_plus_given_split() {
    let mut d = dict();
    d.insert_name(true, "太田", "おおた");
    d.insert_name(false, "原", "はら");
    d.insert_name(true, "太田原", "おおたわら");
    let mut t = vec![
        token("太田", "オオタ", 0..6),
        token("原", "ハラ", 6..9),
        token("さん", "サン", 9..15),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t[0].surface, "太田原");
    assert_eq!(t[0].reading, "オオタワラ");
}

#[test]
fn merged_honorific_token_is_split_when_table_hits() {
    let mut d = dict();
    d.insert_name(true, "太田原", "おおたわら");
    // Lindera が 太田 | 原さん と切った所
    let mut t = vec![
        token("太田", "オオタ", 0..6),
        token("原さん", "ハラサン", 6..15),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    let s: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
    assert_eq!(s, ["太田原", "さん"]);
    assert_eq!(t[0].reading, "オオタワラ");
    assert_eq!(t[1].range, 9..15);
}

#[test]
fn honorific_straddling_a_kana_tail_is_repaired() {
    let mut d = dict();
    d.insert_name(true, "田中", "たなか");
    d.insert_name(false, "弘利", "ひろとし");
    // 形態素解析が 田中 | 弘 | 利さ | ん と切った所
    let mut t = vec![
        token("田中", "タナカ", 0..6),
        token("弘", "ヒロ", 6..9),
        token("利さ", "リサ", 9..16),
        token("ん", "ン", 16..19),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    let s: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
    assert_eq!(s, ["田中", "弘利", "さん"]);
    assert_eq!(t[1].reading, "ヒロトシ");
}

#[test]
fn merged_honorific_token_untouched_without_table_hit() {
    let d = dict();
    let mut t = vec![
        token("太田", "オオタ", 0..6),
        token("原さん", "ハラサン", 6..15),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t.len(), 2);
    assert_eq!(t[1].surface, "原さん");
}

#[test]
fn shop_ya_is_not_a_name() {
    let mut d = dict();
    d.insert_name(true, "本屋", "もとや");
    let mut t = vec![token("本屋", "ホンヤ", 0..6), token("さん", "サン", 6..12)];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t[0].reading, "ホンヤ");
}

#[test]
fn sama_is_not_a_cue() {
    let mut d = dict();
    d.insert_name(true, "王子", "おうし");
    let mut t = vec![token("王子", "オウジ", 0..6), token("様", "サマ", 6..9)];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t[0].reading, "オウジ");
}

#[test]
fn already_correct_is_untouched() {
    let d = dict();
    let mut t = vec![
        token("橋本", "ハシモト", 0..6),
        token("環奈", "カンナ", 6..12),
        token("さん", "サン", 12..18),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    assert!(!t[0].is_name, "読みも境界も同じなら触らない");
    assert!(t[0].alternatives.is_empty());
}

// ---- apply の境界・演算の検査 (mutation testing で見逃された所) ----

#[test]
fn name_hint_reads_consecutive_kanji_tokens_as_one_full_name() {
    // Name ヒントの手がかりは 「漢字 token 列の終わり」。 水田 | 雄一郎 の間 (次も漢字) は手がかりではなく、
    // 姓 + 名 を 1 回で組み直す (alternatives に 2 token 分の元の読みが残る)。
    let d = dict();
    let p = PersonNamePass::new(&d).with_hint(NameHint::Name);
    let mut t = vec![
        token("水田", "スイデン", 0..6),
        token("雄一郎", "ユウイチロウ", 6..15),
    ];
    p.apply(&mut t);
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].reading, "ミズタ");
    assert_eq!(t[0].alternatives[0].reading, "スイデンユウイチロウ");
    assert_eq!(t[0].range, 0..6);
    assert_eq!(t[1].reading, "ユウイチロウ");
    assert_eq!(t[1].range, 6..15);
}

#[test]
fn replaced_ranges_follow_the_token_start_offset() {
    let d = dict();
    // 前に別の token がある (start != 0) 時も、 姓・名の range は元の位置から続く
    let mut t = vec![
        token("の", "ノ", 0..3),
        token("水田", "スイデン", 3..9),
        token("雄一郎", "ユウイチロウ", 9..18),
        token("さん", "サン", 18..24),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    let s: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
    assert_eq!(s, ["の", "水田", "雄一郎", "さん"]);
    assert_eq!(t[1].range, 3..9);
    assert_eq!(t[2].range, 9..18);
    assert_eq!(t[3].range, 18..24);
    // 姓だけの場合
    let mut u = vec![
        token("の", "ノ", 0..3),
        token("水田", "スイデン", 3..9),
        token("選手", "センシュ", 9..15),
    ];
    PersonNamePass::new(&d).apply(&mut u);
    assert_eq!(u[1].range, 3..9);
    assert_eq!(u[1].reading, "ミズタ");
}

#[test]
fn longest_run_with_given_name_wins_up_to_seven_chars() {
    let mut d = Dict::new();
    d.insert_name(true, "小野", "おの");
    d.insert_name(true, "東小野", "ひがしおの");
    d.insert_name(false, "大二郎太", "だいじろうた");
    // 東 | 小野 | 大二郎太 | さん: 7 字 (姓 3 + 名 4) まで許し、 6 字 (小野 + 大二郎太) より長いほうを採る
    let mut t = vec![
        token("東", "ヒガシ", 0..3),
        token("小野", "オノ", 3..9),
        token("大二郎太", "ダイジロウタ", 9..21),
        token("さん", "サン", 21..27),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    let s: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
    assert_eq!(s, ["東小野", "大二郎太", "さん"]);
    assert_eq!(t[0].reading, "ヒガシオノ");
    assert_eq!(t[0].range, 0..9);
    assert_eq!(t[1].range, 9..21);
    assert!(t[0].is_name && t[1].is_name);
}

#[test]
fn run_longer_than_seven_chars_is_not_looked_up() {
    let mut d = Dict::new();
    d.insert_name(true, "壱弐参肆伍陸漆捌", "ななもじ");
    let mut t = vec![
        token("壱弐参肆", "イチ", 0..12),
        token("伍陸漆捌", "ゴ", 12..24),
        token("さん", "サン", 24..30),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t.len(), 3, "8 字の列は姓の表にあっても引かない");
    assert!(!t[0].is_name);
    assert_eq!(t[0].reading, "イチ");
}

#[test]
fn surname_given_beats_longer_surname_only_run() {
    let mut d = Dict::new();
    d.insert_name(true, "田中", "たなか");
    d.insert_name(false, "太", "ふとし");
    d.insert_name(true, "西田中太", "にしたなかた");
    // 西田中太 全体が姓の表にあっても、 姓 + 名 が引ける 田中 | 太 を優先する (名が引けたほうが先)
    let mut t = vec![
        token("西", "ニシ", 0..3),
        token("田中", "デンチュウ", 3..9),
        token("太", "タ", 9..12),
        token("さん", "サン", 12..18),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    let s: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
    assert_eq!(s, ["西", "田中", "太", "さん"]);
    assert!(!t[0].is_name);
    assert_eq!(t[1].reading, "タナカ");
    assert_eq!(t[2].reading, "フトシ");
}

#[test]
fn single_three_char_token_is_not_resplit_into_surname_and_given() {
    let mut d = Dict::new();
    d.insert_name(true, "北海", "ほっかい");
    d.insert_name(false, "道", "どう");
    // 北海道 を 1 語と読んだ形態素解析の結果は、 北海 | 道 に割り直さない (前に token がある位置でも)
    let mut t = vec![
        token("の", "ノ", 0..3),
        token("北海道", "ホッカイドウ", 3..12),
        token("さん", "サン", 12..18),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t.len(), 3);
    assert_eq!(t[1].surface, "北海道");
    assert_eq!(t[1].reading, "ホッカイドウ");
    assert!(!t[1].is_name);
    // 先頭にある時も同じ
    let mut u = vec![
        token("北海道", "ホッカイドウ", 0..9),
        token("さん", "サン", 9..15),
    ];
    PersonNamePass::new(&d).apply(&mut u);
    assert_eq!(u.len(), 2);
    assert_eq!(u[0].surface, "北海道");
    // 人名と言われている時 (Name) は割る
    let p = PersonNamePass::new(&d).with_hint(NameHint::Name);
    let mut n = vec![token("北海道", "ホッカイドウ", 0..9)];
    p.apply(&mut n);
    let s: Vec<&str> = n.iter().map(|x| x.surface.as_str()).collect();
    assert_eq!(s, ["北海", "道"]);
    assert_eq!(n[0].range, 0..6);
    assert_eq!(n[1].range, 6..9);
}

#[test]
fn three_char_first_token_of_multi_token_name_is_still_corrected() {
    let mut d = Dict::new();
    d.insert_name(true, "左右田", "さうだ");
    d.insert_name(false, "卓", "たく");
    // 左右田 | 卓 のように 3 字の token で始まっても、 複数 token の姓 + 名 は組み直す
    let mut t = vec![
        token("左右田", "サユウデン", 0..9),
        token("卓", "スグル", 9..12),
        token("さん", "サン", 12..18),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t[0].reading, "サウダ");
    assert_eq!(t[1].reading, "タク");
    assert!(t[0].is_name && t[1].is_name);
}

#[test]
fn parse_table_reads_entries_only() {
    let c = "[meta]\nschema_version = \"2\"\nrole = \"name_sei\"\n\n[entries]\n\"水田\" = \"みずた\"\n\"橋本\" = \"はしもと\"\n";
    let v = parse_name_table(c);
    assert_eq!(
        v,
        vec![
            ("水田".to_string(), "みずた".to_string()),
            ("橋本".to_string(), "はしもと".to_string())
        ]
    );
}

// ---- 変異テスト (cargo-mutants) の見逃しを殺すための追加 ----

fn surfaces(t: &[Token]) -> Vec<&str> {
    t.iter().map(|x| x.surface.as_str()).collect()
}

fn with_band(mut t: Token, band: u32) -> Token {
    t.band = band as _;
    t
}

fn straddle_dict() -> Dict {
    let mut d = Dict::new();
    d.insert_name(true, "田中", "たなか");
    d.insert_name(false, "弘利", "ひろとし");
    d.insert_name(true, "三津谷", "みつや");
    d.insert_name(false, "雄一郎太", "ゆういちろうた");
    d
}

#[test]
fn parse_table_header_detection_needs_all_three_conditions() {
    // 行末が ] でも [ で始まらない行は見出しではない (値に ] を含む entry)
    let c = "[entries]\n\"a\" = \"[b]\"\n\"c\" = \"d\"\n";
    assert_eq!(
        parse_name_table(c),
        vec![
            ("a".to_string(), "[b]".to_string()),
            ("c".to_string(), "d".to_string())
        ]
    );
    // 行末が ] でも ["..."] で始まる行は見出しではない (entries を抜けない)
    let c = "[entries]\n[\"x\"]\n\"c\" = \"d\"\n";
    assert_eq!(
        parse_name_table(c),
        vec![("c".to_string(), "d".to_string())]
    );
}

#[test]
fn parse_table_skips_non_entry_lines_and_empty_parts() {
    // [entries] の中でも " で始まらない行は読まない
    let c = "[entries]\nkey = \"v\"\n\"a\" = \"b\"\n";
    assert_eq!(
        parse_name_table(c),
        vec![("a".to_string(), "b".to_string())]
    );
    // [entries] の外の " で始まる行は読まない
    let c = "[meta]\n\"x\" = \"y\"\n[entries]\n\"a\" = \"b\"\n";
    assert_eq!(
        parse_name_table(c),
        vec![("a".to_string(), "b".to_string())]
    );
    // 表記か読みのどちらかが空の行は捨てる
    let c = "[entries]\n\"\" = \"y\"\n\"a\" = \"\"\n\"c\" = \"d\"\n";
    assert_eq!(
        parse_name_table(c),
        vec![("c".to_string(), "d".to_string())]
    );
}

#[test]
fn token_may_be_cue_is_a_precise_prefilter() {
    // 手がかりでない
    assert!(!token_may_be_cue(""));
    assert!(!token_may_be_cue("に"));
    assert!(!token_may_be_cue("水田"));
    // 敬称の後半だけの token
    assert!(token_may_be_cue("ん"));
    assert!(token_may_be_cue("ゃん"));
    // 手がかり語で始まる
    assert!(token_may_be_cue("さん"));
    assert!(token_may_be_cue("選手"));
    // 手がかり語の先頭の字で始まるだけ (敬称でも肩書きの頭でもない) では通さない
    assert!(!token_may_be_cue("さい"));
    // 敬称で終わる
    assert!(token_may_be_cue("原さん"));
    assert!(token_may_be_cue("原氏"));
    // ん で終わるだけでは通さない
    assert!(!token_may_be_cue("あん"));
}

#[test]
fn may_have_cue_depends_on_tokens() {
    let none = vec![token("水田", "スイデン", 0..6), token("に", "ニ", 6..9)];
    assert!(!may_have_cue(&none));
    let some = vec![
        token("水田", "スイデン", 0..6),
        token("さん", "サン", 6..12),
    ];
    assert!(may_have_cue(&some));
}

#[test]
fn parse_boundaries_of_given_name_length() {
    let mut d = Dict::new();
    d.insert_name(true, "水田", "みずた");
    d.insert_name(false, "雄", "ゆう");
    d.insert_name(false, "雄一郎太", "ゆういちろうた");
    d.insert_name(false, "雄一郎太朗", "ゆういちろうたろう");
    let p = PersonNamePass::new(&d);
    // 3 字 (姓 2 + 名 1) から姓 + 名に割る
    let r = p.parse("水田雄").unwrap();
    assert_eq!(r.sei, "水田");
    assert_eq!(r.mei.unwrap().0, "雄");
    // 名 4 字までは許す
    let r = p.parse("水田雄一郎太").unwrap();
    assert_eq!(r.sei, "水田");
    assert_eq!(r.mei.unwrap().0, "雄一郎太");
    // 名 5 字は許さない (姓だけとしても引けない)
    assert!(p.parse("水田雄一郎太朗").is_none());
}

#[test]
fn dict_full_name_is_not_resplit_across_tokens() {
    let mut d = dict();
    d.insert_name(true, "江川", "えがわ");
    d.insert_name(false, "卓", "たかし");
    d.insert("江川卓", "エガワスグル");
    // 江川 | 卓 に割れていても、 辞書にフルネームがあれば名の表で組み直さない
    let mut t = vec![
        token("江川", "エガワ", 0..6),
        token("卓", "スグル", 6..9),
        token("投手", "トウシュ", 9..15),
    ];
    PersonNamePass::new(&d).apply(&mut t);
    assert_eq!(t[1].reading, "スグル");
    assert_eq!(t.len(), 3);
}

#[test]
fn straddle_repair_fixes_ranges_and_copies_from_honorific_token() {
    let d = straddle_dict();
    let mut t = vec![
        with_band(token("田中", "タナカ", 0..6), 1),
        with_band(token("弘", "ヒロ", 6..9), 2),
        with_band(token("利さ", "リサ", 9..15), 3),
        with_band(token("ん", "ン", 15..18), 7),
    ];
    PersonNamePass::new(&d).repair_kana_straddle(&mut t);
    assert_eq!(surfaces(&t), ["田中", "弘", "利", "さん"]);
    assert_eq!(t[2].reading, "リ");
    assert_eq!(t[2].range, 9..12);
    assert_eq!(t[2].band, 3);
    assert_eq!(t[3].reading, "サン");
    assert_eq!(t[3].range, 12..18);
    assert_eq!(t[3].band, 7, "右側は敬称側の token を元にする");
}

#[test]
fn straddle_repair_at_index_one_and_skip_ahead() {
    let d = straddle_dict();
    // 直前の token が 1 つだけ (index 1 で当たる)
    let mut t = vec![
        token("田中", "タナカ", 0..6),
        token("弘利さ", "ヒロトシサ", 6..15),
        token("ん", "ン", 15..18),
    ];
    PersonNamePass::new(&d).repair_kana_straddle(&mut t);
    assert_eq!(surfaces(&t), ["田中", "弘利", "さん"]);
    assert_eq!(t[1].reading, "ヒロトシ");
    // 1 つ直したあと、 すぐ後ろの (2 token 先以降の) 別の箇所も直す
    let mut t = vec![
        token("を", "ヲ", 0..3),
        token("を", "ヲ", 3..6),
        token("田中", "タナカ", 6..12),
        token("弘", "ヒロ", 12..15),
        token("利さ", "リサ", 15..21),
        token("ん", "ン", 21..24),
        token("田中", "タナカ", 24..30),
        token("弘利さ", "ヒロトシサ", 30..39),
        token("ん", "ン", 39..42),
    ];
    PersonNamePass::new(&d).repair_kana_straddle(&mut t);
    assert_eq!(
        surfaces(&t),
        ["を", "を", "田中", "弘", "利", "さん", "田中", "弘利", "さん"]
    );
}

#[test]
fn straddle_repair_leaves_unmatched_cases_alone() {
    let d = straddle_dict();
    // 読みが かな の部分で終わっていない
    let mut t = vec![
        token("田中", "タナカ", 0..6),
        token("弘", "ヒロ", 6..9),
        token("利さ", "トシ", 9..15),
        token("ん", "ン", 15..18),
    ];
    PersonNamePass::new(&d).repair_kana_straddle(&mut t);
    assert_eq!(surfaces(&t), ["田中", "弘", "利さ", "ん"]);
    // 表に当たらない
    let mut t = vec![
        token("森", "モリ", 0..3),
        token("利さ", "リサ", 3..9),
        token("ん", "ン", 9..12),
    ];
    PersonNamePass::new(&d).repair_kana_straddle(&mut t);
    assert_eq!(surfaces(&t), ["森", "利さ", "ん"]);
    // 次の token が敬称の続きでなければ、 かなの頭が敬称の頭と違っていても直さない
    let mut t = vec![
        token("田中", "タナカ", 0..6),
        token("弘", "ヒロ", 6..9),
        token("利あ", "リア", 9..15),
        token("ん", "ン", 15..18),
    ];
    PersonNamePass::new(&d).repair_kana_straddle(&mut t);
    assert_eq!(surfaces(&t), ["田中", "弘", "利あ", "ん"]);
    // 敬称が丸ごと token に入っている時 (続きの token は空) は 「またぎ」 ではない
    let mut t = vec![
        token("田中", "タナカ", 0..6),
        token("弘", "ヒロ", 6..9),
        token("利さん", "リサン", 9..18),
        token("", "", 18..18),
    ];
    PersonNamePass::new(&d).repair_kana_straddle(&mut t);
    assert_eq!(surfaces(&t), ["田中", "弘", "利さん", ""]);
}

#[test]
fn straddle_repair_allows_exactly_max_run_chars() {
    let d = straddle_dict();
    // 三津谷 (3) + 雄一郎 + 太 = 7 字 (上限ちょうど) は引く
    let mut t = vec![
        token("三津谷", "ミツヤ", 0..9),
        token("雄一郎", "ユウイチロウ", 9..18),
        token("太さ", "タサ", 18..24),
        token("ん", "ン", 24..27),
    ];
    PersonNamePass::new(&d).repair_kana_straddle(&mut t);
    assert_eq!(surfaces(&t), ["三津谷", "雄一郎", "太", "さん"]);
}

#[test]
fn merged_honorific_split_fixes_ranges() {
    let d = straddle_dict();
    let mut t = vec![
        with_band(token("田中", "タナカ", 0..6), 1),
        with_band(token("弘", "ヒロ", 6..9), 2),
        with_band(token("利さん", "リサン", 9..18), 3),
    ];
    PersonNamePass::new(&d).split_merged_honorific(&mut t);
    assert_eq!(surfaces(&t), ["田中", "弘", "利", "さん"]);
    assert_eq!(t[2].reading, "リ");
    assert_eq!(t[2].range, 9..12);
    assert_eq!(t[3].reading, "サン");
    assert_eq!(t[3].range, 12..18);
}

#[test]
fn merged_honorific_split_continues_past_a_split() {
    let d = straddle_dict();
    let mut t = vec![
        token("を", "ヲ", 0..3),
        token("を", "ヲ", 3..6),
        token("田中", "タナカ", 6..12),
        token("弘", "ヒロ", 12..15),
        token("利さん", "リサン", 15..24),
        token("田中", "タナカ", 24..30),
        token("弘利さん", "ヒロトシサン", 30..42),
    ];
    PersonNamePass::new(&d).split_merged_honorific(&mut t);
    assert_eq!(
        surfaces(&t),
        ["を", "を", "田中", "弘", "利", "さん", "田中", "弘利", "さん"]
    );
}

#[test]
fn merged_honorific_split_skips_when_reading_or_table_mismatch() {
    let d = straddle_dict();
    // 読みが 敬称で終わっていない
    let mut t = vec![
        token("田中", "タナカ", 0..6),
        token("弘", "ヒロ", 6..9),
        token("利さん", "トシ", 9..18),
    ];
    PersonNamePass::new(&d).split_merged_honorific(&mut t);
    assert_eq!(surfaces(&t), ["田中", "弘", "利さん"]);
    // 表に当たらない
    let mut t = vec![token("森", "モリ", 0..3), token("利さん", "リサン", 3..12)];
    PersonNamePass::new(&d).split_merged_honorific(&mut t);
    assert_eq!(surfaces(&t), ["森", "利さん"]);
}

#[test]
fn merged_honorific_split_allows_exactly_max_run_chars() {
    let d = straddle_dict();
    // 三津谷 (3) + 雄一郎 + 太 = 7 字 (上限ちょうど) は引く
    let mut t = vec![
        token("三津谷", "ミツヤ", 0..9),
        token("雄一郎", "ユウイチロウ", 9..18),
        token("太さん", "タサン", 18..27),
    ];
    PersonNamePass::new(&d).split_merged_honorific(&mut t);
    assert_eq!(surfaces(&t), ["三津谷", "雄一郎", "太", "さん"]);
}

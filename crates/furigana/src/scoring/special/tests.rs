use super::alphabet::*;
use super::protect::*;
use crate::scoring::candidate::{
    CandidateProvider, ScoringContext, BAND_DICT_EXACT, BAND_KANJI, BAND_PROTECTED,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::*;
use crate::rules::emoji::EmojiEntry;
use crate::scoring::boundary::BoundaryAnalysis;

fn ctx(input: &str) -> ScoringContext<'_> {
    let boundary = Box::leak(Box::new(BoundaryAnalysis::empty()));
    ScoringContext::new(input, boundary)
}

// (is_emoji_char の unit test は crate::char_class 側に移動)

// ─── 変異テスト由来の追加 ────────────────────────────────────────────────

#[test]
fn joiner_outside_an_emoji_run_is_not_an_emoji() {
    // ZWJ / VS16 / keycap 記号 / 肌色は、 絵文字の途中でなければ単独で絵文字にならない
    for joiner in ["\u{200D}", "\u{FE0F}"] {
        let input = format!("あ{joiner}い");
        assert!(
            extract_protected_tokens(&input).is_empty(),
            "{joiner:?} alone"
        );
    }
}

#[test]
fn keycap_without_variation_selector_is_one_emoji_token() {
    // 1⃣ (VS16 なしの keycap) も 1 つの絵文字
    let input = "あ1\u{20E3}い";
    let tokens = extract_protected_tokens(input);
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].kind, ProtectedKind::Emoji);
    assert_eq!(&input[tokens[0].range.clone()], "1\u{20E3}");
}

#[test]
fn protect_provider_emits_candidate_for_first_of_several_tokens() {
    // 二分探索の起点: 先頭の保護 token の位置でも、 後ろの token の位置でも候補が出る
    let input = "あ😀いう😀";
    let p = ProtectTokenProvider::new(input);
    let starts: Vec<usize> = p.tokens().iter().map(|t| t.range.start).collect();
    assert_eq!(starts.len(), 2);
    let c = ctx(input);
    for &pos in &starts {
        let mut out = Vec::new();
        p.candidates_at(&c, pos, &mut out);
        assert_eq!(out.len(), 1, "pos {pos}");
    }
    let mut out = Vec::new();
    p.candidates_at(&c, 0, &mut out);
    assert!(out.is_empty());
}

// ─── URL extraction ──────────────────────────────────────────────────────

#[test]
fn extract_simple_https_url() {
    let tokens = extract_protected_tokens("見て https://example.com/path です");
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].kind, ProtectedKind::Url);
    let s = "見て https://example.com/path です";
    let extracted = &s[tokens[0].range.clone()];
    assert_eq!(extracted, "https://example.com/path");
}

#[test]
fn extract_www_url_without_scheme() {
    let s = "www.example.com を見て";
    let tokens = extract_protected_tokens(s);
    // 「種別が含まれる」だけだと over-capture (末尾の space や「を」 を飲む) を
    // 見逃す。抽出 range の中身を完全一致で固定する。
    let url = tokens
        .iter()
        .find(|t| t.kind == ProtectedKind::Url)
        .expect("url token");
    assert_eq!(&s[url.range.clone()], "www.example.com");
}

#[test]
fn extract_email() {
    let s = "foo@bar.com にメール";
    let tokens = extract_protected_tokens(s);
    let email = tokens
        .iter()
        .find(|t| t.kind == ProtectedKind::Email)
        .expect("email token");
    assert_eq!(&s[email.range.clone()], "foo@bar.com");
}

// ─── Emoji extraction ────────────────────────────────────────────────────

#[test]
fn extract_single_emoji() {
    let tokens = extract_protected_tokens("こんにちは😀");
    let emoji = tokens
        .iter()
        .find(|t| t.kind == ProtectedKind::Emoji)
        .expect("emoji token");
    let s = "こんにちは😀";
    let extracted = &s[emoji.range.clone()];
    assert_eq!(extracted, "😀");
}

#[test]
fn extract_consecutive_emoji_as_single_token() {
    let tokens = extract_protected_tokens("🎉🎊✨ パーティー");
    let emoji_tokens: Vec<&ProtectedToken> = tokens
        .iter()
        .filter(|t| t.kind == ProtectedKind::Emoji)
        .collect();
    assert_eq!(
        emoji_tokens.len(),
        1,
        "consecutive emoji が 1 token に集約される"
    );
    let s = "🎉🎊✨ パーティー";
    let extracted = &s[emoji_tokens[0].range.clone()];
    assert_eq!(extracted, "🎉🎊✨");
}

#[test]
fn extract_emoji_separated_by_text() {
    let tokens = extract_protected_tokens("こんにちは🎉お元気ですか🚀");
    let emoji_tokens: Vec<&ProtectedToken> = tokens
        .iter()
        .filter(|t| t.kind == ProtectedKind::Emoji)
        .collect();
    assert_eq!(emoji_tokens.len(), 2);
}

// ─── 重複削除 ────────────────────────────────────────────────────────────

#[test]
fn extract_no_duplicate_tokens_for_overlapping_url_email() {
    // URL の中に email っぽい substring がある場合、 広い range の URL が勝ち、
    // 包含される email token は dedup で消える。「URL が 1 つ以上」 だけだと
    // dedup ロジック自体を検証できないので、 token 総数と内訳・range を固定する。
    let s = "https://user@example.com/page";
    let tokens = extract_protected_tokens(s);
    assert_eq!(
        tokens.len(),
        1,
        "email は URL に包含され dedup される: {tokens:?}"
    );
    assert_eq!(tokens[0].kind, ProtectedKind::Url);
    assert_eq!(&s[tokens[0].range.clone()], s, "URL は全体を覆う");
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == ProtectedKind::Email)
            .count(),
        0,
        "包含 email token は残らない"
    );
}

// ─── ProtectTokenProvider ────────────────────────────────────────────────

#[test]
fn provider_returns_candidate_at_token_start() {
    let input = "foo https://example.com bar";
    let provider = ProtectTokenProvider::new(input);
    let url_start = input.find("https").unwrap();
    let candidates = provider.candidates_vec(&ctx(input), url_start);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].surface, "https://example.com");
    assert_eq!(candidates[0].reading, "https://example.com"); // passthrough
    assert_eq!(candidates[0].score.band, BAND_PROTECTED);
}

#[test]
fn provider_returns_empty_at_non_token_position() {
    let input = "foo https://example.com bar";
    let provider = ProtectTokenProvider::new(input);
    let candidates = provider.candidates_vec(&ctx(input), 0);
    assert!(candidates.is_empty(), "pos 0 は URL の start ではない");
}

#[test]
fn provider_emoji_passthrough_reading() {
    let input = "Hi😀";
    let provider = ProtectTokenProvider::new(input);
    let emoji_start = input.find('😀').unwrap();
    let candidates = provider.candidates_vec(&ctx(input), emoji_start);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].surface, "😀");
    assert_eq!(candidates[0].reading, "😀"); // passthrough、 ひらがな化しない
}

#[test]
fn provider_with_no_protected_tokens_yields_empty() {
    let input = "ただのテキスト";
    let provider = ProtectTokenProvider::new(input);
    assert!(provider.tokens().is_empty());
    assert!(provider.candidates_vec(&ctx(input), 0).is_empty());
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn protected_band_higher_than_dict_exact() {
    // 保護 token は dict 完全一致より優先される必要がある
    use crate::scoring::candidate::BAND_DICT_EXACT;
    assert!(BAND_PROTECTED > BAND_DICT_EXACT);
}

// ─── アルファベット passthrough (C2) tests ───────────────────────────────

#[test]
fn alphabet_char_detects_ascii_alphanumeric() {
    assert!(is_alphabet_char('A'));
    assert!(is_alphabet_char('z'));
    assert!(is_alphabet_char('5'));
}

#[test]
fn alphabet_char_detects_fullwidth() {
    assert!(is_alphabet_char('Ａ'));
    assert!(is_alphabet_char('ｚ'));
    assert!(is_alphabet_char('５'));
}

#[test]
fn alphabet_char_rejects_non_alphabet() {
    assert!(!is_alphabet_char('猫'));
    assert!(!is_alphabet_char('あ'));
    assert!(!is_alphabet_char('!'));
}

#[test]
fn alphabet_char_accepts_whitespace_passthrough() {
    // space / tab は alphabet 扱い (= 「猫 犬」 のような ASCII whitespace 含み input
    // で path 構築失敗を防ぐ passthrough 設計、 impl 側 doc comment 参照)
    assert!(is_alphabet_char(' '));
    assert!(is_alphabet_char('\t'));
}

#[test]
fn find_alphabet_ranges_basic() {
    let ranges = find_alphabet_ranges("APIサーバー");
    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0], 0..3); // "API" = 3 ASCII bytes
}

#[test]
fn find_alphabet_ranges_multiple() {
    let ranges = find_alphabet_ranges("ABCのDEF");
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0], 0..3); // "ABC"
                                 // "の" は 3 bytes (UTF-8)、 「DEF」 は 6..9
    assert_eq!(ranges[1], 6..9);
}

#[test]
fn find_alphabet_ranges_splits_space_next_to_digit() {
    // 数字に接する空白は range を繋がない。 「2 M3.7」 を 1 つの passthrough で
    // 覆うと数値の読みが負ける (★2026-09-15)。
    let ranges = find_alphabet_ranges("2 M3.7");
    // "2" / "M3" / "7" (「.」 は英数でないので range を切る)
    assert_eq!(ranges, vec![0..1, 2..4, 5..6]);
}

#[test]
fn find_alphabet_ranges_keeps_space_between_letters() {
    // 英字どうしの間の空白は従来どおり 1 range にまとめる。
    let ranges = find_alphabet_ranges("Hello World");
    assert_eq!(ranges, vec![0..11]);
}

#[test]
fn find_alphabet_ranges_empty_for_no_alphabet() {
    let ranges = find_alphabet_ranges("漢字とひらがな");
    assert!(ranges.is_empty());
}

#[test]
fn find_alphabet_ranges_joins_hyphen_between_letters() {
    // Wi-Fi / Blu-ray / e-mail は 1 語 (連結記号を含めて 1 range)
    let ranges = find_alphabet_ranges("ポケットWi-Fiつけよう");
    assert_eq!(ranges.len(), 1);
    assert_eq!(&"ポケットWi-Fiつけよう"[ranges[0].clone()], "Wi-Fi");
    let ranges = find_alphabet_ranges("Blu-ray");
    assert_eq!(ranges, vec![0..7]);
    let ranges = find_alphabet_ranges("don't");
    assert_eq!(ranges, vec![0..5]);
}

#[test]
fn find_alphabet_ranges_does_not_join_hyphen_next_to_digits_or_spaces() {
    // 「3-1」 は数値式 (数字提供者に任せる)、 「A - B」 は空白挟みで語ではない
    let ranges = find_alphabet_ranges("3-1");
    assert_eq!(ranges, vec![0..1, 2..3]);
    // ★2026-09-11: range の前後の空白は落とす (= 「A」 と 「B」)。
    // 空白を含めたままだと 「優勝 5回」 の 「 5」 が range になり、
    // 数字を passthrough が覆って助数詞が効かなくなる。
    let ranges = find_alphabet_ranges("A - B");
    assert_eq!(ranges, vec![0..1, 4..5]);
    // 末尾 / 先頭のハイフンは語に含めない
    let ranges = find_alphabet_ranges("abc-");
    assert_eq!(ranges, vec![0..3]);
    let ranges = find_alphabet_ranges("-abc");
    assert_eq!(ranges, vec![1..4]);
    // 数字と英字の間 (USB-3) も連結しない = 従来挙動
    let ranges = find_alphabet_ranges("USB-3");
    assert_eq!(ranges, vec![0..3, 4..5]);
}

#[test]
fn alphabet_passthrough_looks_up_hyphenated_word_as_one_token() {
    let input = "ポケットWi-Fiつけよう";
    let mut lookup = HashMap::new();
    lookup.insert("wi-fi".to_string(), "ワイファイ".to_string());
    let provider = AlphabetPassthroughProvider::new(input, Arc::new(lookup));
    let pos = "ポケット".len();
    let candidates = provider.candidates_vec(&ctx(input), pos);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].surface, "Wi-Fi");
    assert_eq!(candidates[0].reading, "ワイファイ");
    assert_eq!(candidates[0].score.band, BAND_DICT_EXACT);
}

#[test]
fn alphabet_passthrough_falls_back_to_connector_stripped_key() {
    // dict に "wifi" しか無くても "Wi-Fi" が hit する
    let input = "Wi-Fi";
    let mut lookup = HashMap::new();
    lookup.insert("wifi".to_string(), "ワイファイ".to_string());
    let provider = AlphabetPassthroughProvider::new(input, Arc::new(lookup));
    let candidates = provider.candidates_vec(&ctx(input), 0);
    assert_eq!(candidates[0].reading, "ワイファイ");
    assert_eq!(candidates[0].score.band, BAND_DICT_EXACT);
}

#[test]
fn alphabet_passthrough_unknown_hyphenated_word_passes_through_whole() {
    let input = "Blu-ray";
    let provider = AlphabetPassthroughProvider::passthrough_only(input);
    let candidates = provider.candidates_vec(&ctx(input), 0);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].surface, "Blu-ray");
    assert_eq!(candidates[0].reading, "Blu-ray");
}

#[test]
fn normalize_alphabet_lowercases_ascii() {
    assert_eq!(normalize_alphabet("API"), "api");
    assert_eq!(normalize_alphabet("Hello"), "hello");
}

#[test]
fn normalize_alphabet_converts_fullwidth_to_halfwidth() {
    assert_eq!(normalize_alphabet("ＡＰＩ"), "api");
    assert_eq!(normalize_alphabet("Ｈｅｌｌｏ"), "hello");
    assert_eq!(normalize_alphabet("１２３"), "123");
}

#[test]
fn normalize_alphabet_preserves_non_alphabetic() {
    // 非 alphabet 文字 (ここでは passed through but unrelated)
    assert_eq!(normalize_alphabet("a-b_c.d"), "a-b_c.d");
}

#[test]
fn alphabet_passthrough_provider_returns_surface_when_no_lookup() {
    let input = "APIサーバー";
    let provider = AlphabetPassthroughProvider::passthrough_only(input);
    let candidates = provider.candidates_vec(&ctx(input), 0);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].surface, "API");
    assert_eq!(candidates[0].reading, "API"); // passthrough: reading = surface
                                              // miss は fallback band (= BAND_KANJI)、 数字系 provider (band 950) との競合解決のため
    assert_eq!(candidates[0].score.band, BAND_KANJI);
}

#[test]
fn alphabet_passthrough_provider_uses_lookup_when_hit() {
    let input = "APIサーバー";
    let mut lookup = HashMap::new();
    lookup.insert("api".to_string(), "エーピーアイ".to_string());
    let provider = AlphabetPassthroughProvider::new(input, Arc::new(lookup));
    let candidates = provider.candidates_vec(&ctx(input), 0);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].surface, "API");
    assert_eq!(candidates[0].reading, "エーピーアイ");
}

#[test]
fn alphabet_with_trailing_symbol_uses_lookup_only_when_listed() {
    // C# は外来語表にあれば記号込みで引く。 表に無い X# は従来どおり英字だけ
    let input = "C#とX#";
    let mut lookup = HashMap::new();
    lookup.insert("c#".to_string(), "シーシャープ".to_string());
    let provider = AlphabetPassthroughProvider::new(input, Arc::new(lookup));
    let c = provider.candidates_vec(&ctx(input), 0);
    assert!(c
        .iter()
        .any(|x| x.surface == "C#" && x.reading == "シーシャープ"));
    let x = provider.candidates_vec(&ctx(input), "C#と".len());
    assert!(x.iter().all(|c| c.surface == "X"), "{x:?}");
}

#[test]
fn alphabet_passthrough_normalizes_fullwidth_for_lookup() {
    let input = "ＡＰＩを使う";
    let mut lookup = HashMap::new();
    lookup.insert("api".to_string(), "エーピーアイ".to_string());
    let provider = AlphabetPassthroughProvider::new(input, Arc::new(lookup));
    let candidates = provider.candidates_vec(&ctx(input), 0);
    assert_eq!(candidates.len(), 1);
    // surface は full-width のまま、 normalize されるのは lookup key のみ
    assert_eq!(candidates[0].surface, "ＡＰＩ");
    assert_eq!(candidates[0].reading, "エーピーアイ");
}

#[test]
fn alphabet_passthrough_returns_empty_at_non_alphabet_position() {
    let input = "APIサーバー";
    let provider = AlphabetPassthroughProvider::passthrough_only(input);
    // pos 3 は 「サ」 の start (= API の後)、 alphabet ではない
    assert!(provider.candidates_vec(&ctx(input), 3).is_empty());
}

#[test]
fn alphabet_passthrough_no_alphabet_in_input() {
    let input = "ただの日本語";
    let provider = AlphabetPassthroughProvider::passthrough_only(input);
    assert!(provider.ranges().is_empty());
    assert!(provider.candidates_vec(&ctx(input), 0).is_empty());
}

// ─── 英字 + 数字 ([digits]) ────────────────────────────────────────────

fn digits_lookup() -> Arc<HashMap<String, String>> {
    let mut m = HashMap::new();
    m.insert("windows".to_string(), "ウィンドウズ".to_string());
    m.insert(format!("{DIGITS_KEY_PREFIX}windows"), "english".to_string());
    m.insert("no".to_string(), "ノー".to_string());
    m.insert(
        format!("{DIGITS_KEY_PREFIX}no"),
        format!("english{DIGITS_SEP}ナンバー"),
    );
    m.insert("rtx".to_string(), "アールティーエックス".to_string());
    m.insert(format!("{DIGITS_KEY_PREFIX}rtx"), "each".to_string());
    m.insert("max".to_string(), "マックス".to_string());
    m.insert("5g".to_string(), "ファイブジー".to_string());
    Arc::new(m)
}

fn readings(input: &str) -> Vec<(String, String)> {
    let provider = AlphabetPassthroughProvider::new(input, digits_lookup());
    provider
        .candidates_vec(&ctx(input), 0)
        .into_iter()
        .map(|c| (c.surface.to_string(), c.reading.to_string()))
        .collect()
}

#[test]
fn emoji_clusters_split_skin_tone_zwj_flag_and_keycap() {
    assert_eq!(emoji_clusters("😂😂"), ["😂", "😂"]);
    assert_eq!(emoji_clusters("👍\u{1F3FD}🙏"), ["👍\u{1F3FD}", "🙏"]);
    assert_eq!(emoji_clusters("⏰\u{FE0F}🔥"), ["⏰\u{FE0F}", "🔥"]);
    // ZWJ で繋いだ家族は 1 つ
    let family = "👨\u{200D}👩\u{200D}👧";
    assert_eq!(emoji_clusters(family), [family]);
    // 国旗は地域指示記号 2 つで 1 つ
    assert_eq!(emoji_clusters("🇯🇵🇺🇸"), ["🇯🇵", "🇺🇸"]);
    // keycap
    assert_eq!(
        emoji_clusters("1\u{FE0F}\u{20E3}2\u{FE0F}\u{20E3}"),
        ["1\u{FE0F}\u{20E3}", "2\u{FE0F}\u{20E3}"]
    );
}

#[test]
fn emoji_reading_uses_table_with_fallbacks() {
    let mut t = HashMap::<String, EmojiEntry>::new();
    t.insert(
        "⏰".to_string(),
        EmojiEntry::Simple("めざましどけい|目覚まし|時計".to_string()),
    );
    t.insert("👍".to_string(), EmojiEntry::Simple("いいね".to_string()));
    t.insert("😂".to_string(), EmojiEntry::Simple(String::new()));
    t.insert("🍣".to_string(), EmojiEntry::Simple("!すし".to_string()));
    t.insert(
        "🍜".to_string(),
        EmojiEntry::Simple("ラーメン|麺".to_string()),
    );
    t.insert("🦀".to_string(), EmojiEntry::Simple("かに|蟹".to_string()));
    let wo = "を食べる";
    // VS16 / 肌色つきでも基本形で引く
    assert_eq!(
        emoji_reading("⏰\u{FE0F}", &t, "今日は", wo).as_deref(),
        Some("めざましどけい")
    );
    assert_eq!(
        emoji_reading("👍\u{1F3FD}", &t, "", wo).as_deref(),
        Some("いいね")
    );
    // 直後が助詞でも語でもなければ (文末・句読点・空白・数字・記号) 装飾 (None = 1 つも読みに変わらない)
    assert_eq!(emoji_reading("⏰", &t, "今日は", ""), None);
    assert_eq!(emoji_reading("⏰", &t, "今日は", "。"), None);
    assert_eq!(emoji_reading("⏰", &t, "今日は", " 続き"), None);
    assert_eq!(emoji_reading("⏰", &t, "今日は", "12時"), None);
    assert_eq!(emoji_reading("⏰", &t, "今日は", "✨"), None);
    // 語が続く時も読む (⚾見に行く / 🎧ながら)。 ただし行頭・空白・開き括弧の直後 (見出し) や、 直前が文末の形なら装飾
    assert_eq!(
        emoji_reading("⏰", &t, "今日は", "起きた").as_deref(),
        Some("めざましどけい")
    );
    assert_eq!(emoji_reading("⏰", &t, "", "起きた"), None);
    assert_eq!(emoji_reading("⏰", &t, "【", "起きた"), None);
    assert_eq!(
        emoji_reading("⏰", &t, "ありがとうございます", "今日は"),
        None
    );
    assert_eq!(emoji_reading("⏰", &t, "楽しかったね", "今日は"), None);
    // 助詞が続く時は行頭でも読む
    assert_eq!(
        emoji_reading("⏰", &t, "", "を止める").as_deref(),
        Some("めざましどけい")
    );
    // 先頭 ! は常に読む
    assert_eq!(emoji_reading("🍣", &t, "", "").as_deref(), Some("すし"));
    // 直前が名詞 (漢字・カタカナ・英数字・絵文字) なら、 その語に添えた装飾 (綴りに関係なく)
    assert_eq!(emoji_reading("🍜", &t, "ラーメン", wo), None);
    assert_eq!(emoji_reading("🍜", &t, "拉麺", wo), None);
    assert_eq!(emoji_reading("🍜", &t, "ramen", wo), None);
    assert_eq!(emoji_reading("🍜", &t, "3", wo), None);
    assert_eq!(emoji_reading("🍜", &t, "🍙", wo), None);
    // 直前が助詞・句読点・空白なら名詞として読む
    assert_eq!(
        emoji_reading("🍜", &t, "昼は", wo).as_deref(),
        Some("ラーメン")
    );
    assert_eq!(
        emoji_reading("🍜", &t, "昼、", wo).as_deref(),
        Some("ラーメン")
    );
    assert_eq!(
        emoji_reading("🍜", &t, "昼 ", wo).as_deref(),
        Some("ラーメン")
    );
    // 直前・直後の語が同じ読み・別表記なら装飾 (ひらがな・別表記)
    assert_eq!(emoji_reading("🍜", &t, "らーめん", wo), None);
    assert_eq!(emoji_reading("⏰", &t, "あさのめざましどけい", wo), None);
    assert_eq!(emoji_reading("🦀", &t, "今日は", "蟹座の人"), None);
    assert_eq!(emoji_reading("🦀", &t, "今日は", "カニ料理を"), None);
    // かな 1 文字の読み (き / は) は重複判定に使わない: 🌳 = き|木 が 「きょう」 の前で消えない
    t.insert("🌳".to_string(), EmojiEntry::Simple("き|木".to_string()));
    assert_eq!(
        emoji_reading("🌳", &t, "今日は", "の下").as_deref(),
        Some("き")
    );
    assert_eq!(
        emoji_reading("🌳", &t, "今日は", "きょうも").as_deref(),
        Some("き")
    );
    assert_eq!(emoji_reading("🌳", &t, "今日は", "木の下"), None);
    // 行末に同じ絵文字がもう一度ある挟み装飾
    assert_eq!(emoji_reading("🍜", &t, "昼は", "を食べた🍜"), None);
    // 空の読み = 常に読まない。 載っていない絵文字は表記のまま混ぜる
    assert_eq!(emoji_reading("😂😂", &t, "", "").as_deref(), Some(""));
    assert_eq!(emoji_reading("⏰🔥", &t, "", ""), None);
    assert_eq!(
        emoji_reading("⏰🔥😂", &t, "今日は", "を").as_deref(),
        Some("⏰🔥")
    );
    // 1 つも当たらない / 表が空なら None (表記のまま)
    assert_eq!(emoji_reading("🔥", &t, "今日は", wo), None);
    assert_eq!(
        emoji_reading("⏰", &HashMap::<String, EmojiEntry>::new(), "今日は", wo),
        None
    );
}

#[test]
fn emoji_reading_follows_context_matches() {
    let data: crate::rules::EmojiData = toml::from_str(
        r#"
        [entries."🙏"]
        reading = ""
        [[entries."🙏".match]]
        alone = true
        reading = "ありがとう"
        [[entries."🙏".match]]
        prev_ends_any = ["お願い", "お願いします"]
        reading = ""

        # 既定の読みを省略 = 当たらない時は辞書に載っていないのと同じ (表記のまま)
        [entries."😭"]
        [[entries."😭".match]]
        alone = true
        reading = "うえーん"
        "#,
    )
    .unwrap();
    let t: HashMap<String, EmojiEntry> = data.entries.into_iter().collect();
    // 発言が絵文字だけなら読む
    assert_eq!(
        emoji_reading("🙏", &t, "", "").as_deref(),
        Some("ありがとう")
    );
    // 文に添えた時は既定 (読まない)。 match の reading が空でも 「読まない」 を明示する
    assert_eq!(
        emoji_reading("🙏", &t, "ありがとう", "").as_deref(),
        Some("")
    );
    assert_eq!(
        emoji_reading("🙏", &t, "よろしくお願い", "").as_deref(),
        Some("")
    );
    // 連なり (🙏🙏) は alone に当たらない → 既定の読まない
    assert_eq!(emoji_reading("🙏🙏", &t, "", "").as_deref(), Some(""));
    // 既定の読みが無い絵文字: 当たれば読む、 当たらなければ表記のまま (None)
    assert_eq!(emoji_reading("😭", &t, "", "").as_deref(), Some("うえーん"));
    assert_eq!(emoji_reading("😭", &t, "悲しい", ""), None);
}

#[test]
fn emoji_run_is_extracted_as_one_token_including_joiners_and_misc_symbols() {
    // ⏰ (U+23F0) と VS16、 肌色、 国旗も絵文字の一部
    for emoji in ["⏰\u{FE0F}", "👍\u{1F3FD}", "🇯🇵", "1\u{FE0F}\u{20E3}", "⭐"] {
        let input = format!("あ{emoji}い");
        let tokens = extract_protected_tokens(&input);
        assert_eq!(tokens.len(), 1, "{emoji:?}");
        assert_eq!(tokens[0].kind, ProtectedKind::Emoji, "{emoji:?}");
        assert_eq!(&input[tokens[0].range.clone()], emoji, "{emoji:?}");
    }
    // 普通の数字は絵文字にならない
    assert!(extract_protected_tokens("123").is_empty());
}

fn markup_surfaces(input: &str) -> Vec<&str> {
    extract_protected_tokens(input)
        .iter()
        .filter(|t| t.kind == ProtectedKind::Markup)
        .map(|t| &input[t.range.clone()])
        .collect()
}

#[test]
fn html_like_tags_are_protected_markup() {
    // 既知のタグ名の開き・閉じ・属性つき・自己閉じ (大文字小文字は問わない)
    assert_eq!(
        markup_surfaces("<center>琳琅満目</center>"),
        vec!["<center>", "</center>"]
    );
    assert_eq!(
        markup_surfaces("あ<small>ちいさい</small>い<i>斜</i><b>太</b>"),
        vec!["<small>", "</small>", "<i>", "</i>", "<b>", "</b>"]
    );
    assert_eq!(markup_surfaces("<BR>え<br/>お"), vec!["<BR>", "<br/>"]);
    assert_eq!(
        markup_surfaces("<a href=\"x\">リンク</a>"),
        vec!["<a href=\"x\">", "</a>"]
    );
    // 山括弧の普通の使い方 / 未知のタグ名 / 不等号は対象外
    for input in [
        "<Team Name>",
        "<studio>",
        "1<2>3",
        "a < b > c",
        "<the>",
        "<>",
    ] {
        assert!(markup_surfaces(input).is_empty(), "{input}");
    }
}

#[test]
fn mfm_functions_are_protected_markup_but_content_is_kept() {
    let input = "$[font.serif $[x2 **琳琅満目**]]";
    let surfaces = markup_surfaces(input);
    assert_eq!(surfaces, vec!["$[font.serif ", "$[x2 ", "]", "]"]);
    // 中身の文章は記法に含まれない
    let tokens = extract_protected_tokens(input);
    let content = input.find("琳").unwrap();
    assert!(tokens
        .iter()
        .all(|t| !(t.range.start <= content && content < t.range.end)));
    // 引数つき・複数
    assert_eq!(
        markup_surfaces("$[position.x=-3,y=-2 あ]い$[flip う]"),
        vec!["$[position.x=-3,y=-2 ", "]", "$[flip ", "]"]
    );
    // 閉じが無ければ開きだけ
    assert_eq!(markup_surfaces("$[x2 おわらない"), vec!["$[x2 "]);
    // 名前が無い / 空白が無い / 英数字以外 は MFM ではない
    for input in ["$[ あ]", "$[x2]", "$[あ い]", "お金は$100[円]"] {
        assert!(markup_surfaces(input).is_empty(), "{input}");
    }
}

#[test]
fn markup_takes_adjacent_symbols_so_the_path_can_reach_it() {
    // 形態素解析が `**</` / `]</` / `...</` を 1 語にまとめても記法に届くよう、 直前に隣り合う記号も記法の範囲に含める
    assert_eq!(markup_surfaces("**あ**</center>"), vec!["**</center>"]);
    assert_eq!(markup_surfaces("[URL]</center>"), vec!["]</center>"]);
    assert_eq!(markup_surfaces("おわり...</small>"), vec!["...</small>"]);
    assert_eq!(
        markup_surfaces("「<plain>あ</plain>」"),
        vec!["「<plain>", "</plain>"]
    );
    assert_eq!(markup_surfaces("**$[x2 あ]**"), vec!["**$[x2 ", "]"]);
    // 隣の記法には食い込まない (`</i><b>` の `>` は前のタグのもの)
    assert_eq!(
        markup_surfaces("<i>斜</i><b>太</b>"),
        vec!["<i>", "</i>", "<b>", "</b>"]
    );
    // 離れた所に別の記法があっても、 隣り合う記号は取り込む
    assert_eq!(
        markup_surfaces("<br>あ**</center>"),
        vec!["<br>", "**</center>"]
    );
    // 記法と記法のあいだに記号が 1 つだけ挟まる時は、 その記号は後ろの記法に含める
    assert_eq!(
        markup_surfaces("<i>x</i>*<b>y</b>"),
        vec!["<i>", "</i>", "*<b>", "</b>"]
    );
    // `。` `、` は間なので含めない
    assert_eq!(markup_surfaces("おわり。</small>"), vec!["</small>"]);
    assert_eq!(markup_surfaces("おわり、<small>"), vec!["<small>"]);
    // 文字や数字は含めない
    assert_eq!(markup_surfaces("A1</small>"), vec!["</small>"]);
}

#[test]
fn markup_is_kept_next_to_other_tokens_but_dropped_when_overlapping() {
    // 隣り合うだけ (重ならない) なら、 両方取る: 直前にメール
    assert_eq!(
        kinds_and_surfaces("a@b.co<br>"),
        vec![
            (ProtectedKind::Email, "a@b.co"),
            (ProtectedKind::Markup, "<br>")
        ]
    );
    // 文字参照を属性に含むタグは、 文字参照と重なるのでタグの開きは取らない (閉じは取る)
    assert_eq!(markup_surfaces(r#"<a title="&amp;">x</a>"#), vec!["</a>"]);
}

#[test]
fn markup_does_not_override_url_or_entity() {
    // URL の直後のタグは別々に取る (URL は URL のまま、 タグはタグ)
    let input = "https://example.com/a<b>x</b>";
    let tokens = extract_protected_tokens(input);
    assert!(tokens.iter().any(
        |t| t.kind == ProtectedKind::Url && &input[t.range.clone()] == "https://example.com/a"
    ));
    assert_eq!(markup_surfaces(input), vec!["<b>", "</b>"]);
    // 文字参照 (&lt;b&gt;) はタグではない
    assert!(markup_surfaces("&lt;b&gt;").is_empty());
}

#[test]
fn ascii_art_runs_of_mixed_symbols_are_protected_with_empty_reading() {
    // 種類の違う記号が 3 種類以上 (6 文字以上)、 または 2 種類で 12 文字以上の連なり
    for art in [
        "/:::::::::::{:::/ }:}",
        "!:.:.:.:.:l|:! '",
        r"|\_::::\__ ' :: / ",
        "|---|---|---|",
        "=======-------=======",
        "┌──┬──┐",
        "(((((((((())))))))))",
    ] {
        let input = format!("あ{}い", art.trim());
        let tokens = extract_protected_tokens(&input);
        assert_eq!(tokens.len(), 1, "{art}");
        assert_eq!(tokens[0].kind, ProtectedKind::AsciiArt, "{art}");
        assert_eq!(&input[tokens[0].range.clone()], art.trim(), "{art}");
    }
    // 読みは空 (丸ごと除外)
    let input = "ヽ/:::{:::/ }:}丶";
    let provider = ProtectTokenProvider::new(input);
    let start = input.find('/').unwrap();
    let mut cands = Vec::new();
    provider.candidates_at(&ctx(input), start, &mut cands);
    assert!(!cands.is_empty());
    assert!(cands.iter().all(|c| c.reading.is_empty()));
}

#[test]
fn ascii_art_does_not_swallow_ordinary_punctuation() {
    // 同じ記号・強調の連続 / 短い記号 / 文字や数字をはさむ記号 / 数式・日付・URL は対象外
    for input in [
        "えええ!!!!!!!!!!",
        "うーん......",
        "?!?!?!?!?!",
        "-----",
        "**太字**",
        "(1,0,0)→(0,1,0)",
        "10/8(木)-14(水)",
        "A / B / C",
        "a-b-c-d",
        "(笑)",
        "ほげ!:3",
        "11:00-19:00",
    ] {
        assert!(
            extract_protected_tokens(input)
                .iter()
                .all(|t| t.kind != ProtectedKind::AsciiArt),
            "{input}"
        );
    }
    // 境界: 5 文字は対象外、 6 文字 (3 種類) は対象
    assert!(extract_protected_tokens(r"あ|\/_:い")
        .iter()
        .all(|t| t.kind != ProtectedKind::AsciiArt));
    assert!(extract_protected_tokens(r#"あ|\/_:"い"#)
        .iter()
        .any(|t| t.kind == ProtectedKind::AsciiArt));
    // 空白を除いて数える (前後の空白は run に含めない)
    let input = r#"あ  |\/ _:"  い"#;
    let tokens = extract_protected_tokens(input);
    let art: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == ProtectedKind::AsciiArt)
        .collect();
    assert_eq!(art.len(), 1);
    assert_eq!(&input[art[0].range.clone()], r#"|\/ _:""#);
}

fn art_runs(input: &str) -> Vec<&str> {
    ascii_art_runs(input)
        .into_iter()
        .map(|r| &input[r])
        .collect()
}

#[test]
fn ascii_art_judge_thresholds_are_exact() {
    // 6 文字 (種類 3 以上) からが対象: 5 文字は対象外
    assert_eq!(art_runs(r#"|\/_:""#), vec![r#"|\/_:""#]);
    assert!(art_runs(r"|\/_:").is_empty());
    // ちょうど 3 種類・6 文字
    assert_eq!(art_runs(r"||//\\"), vec![r"||//\\"]);
    // 2 種類は 12 文字から (11 文字は対象外、 12 文字は対象)
    assert!(art_runs("|-|-|-|-|-|").is_empty());
    assert_eq!(art_runs("|-|-|-|-|-|-"), vec!["|-|-|-|-|-|-"]);
    // 1 種類は何文字でも対象外
    assert!(art_runs("------------------------").is_empty());
    // 2 種類で短い (8 文字) は対象外
    assert!(art_runs("|-|-|-|-").is_empty());
    // 強調記号 (. ! ? ~ ,) は種類に数えない: 2 種類 + 強調だけ → 対象外
    assert!(art_runs("|-|-.!?~,").is_empty());
    // 強調記号は文字数には数える: 3 種類 + 強調 3 つで 6 文字
    assert_eq!(art_runs(r"|\/!!!"), vec![r"|\/!!!"]);
    // 強調記号だけの連なりは何文字でも対象外
    assert!(art_runs("!!!!!!!!!!????????......~~~~~,,,,,").is_empty());
}

#[test]
fn ascii_art_run_bridges_look_alike_letters_only_between_symbols() {
    // 記号にはさまれた 1 文字の「線に見える英字」 は連なりを続ける (範囲に含める)
    assert_eq!(art_runs("|:/l/:|"), vec!["|:/l/:|"]);
    assert_eq!(art_runs("|:/i/:|"), vec!["|:/i/:|"]);
    // 前後が空白でもよい (前だけ / 後ろだけ / 両方)
    assert_eq!(art_runs("|:/ l /:|"), vec!["|:/ l /:|"]);
    assert_eq!(art_runs("|:/ l/:|"), vec!["|:/ l/:|"]);
    assert_eq!(art_runs("|:/l /:|"), vec!["|:/l /:|"]);
    // 橋渡しの対象でない英字は連なりを切る (それぞれ 3 文字で短く、 対象外)
    assert!(art_runs("|:/a/:|").is_empty());
    assert!(art_runs("|:/あ/:|").is_empty());
    // 連なりの先頭・末尾・記号の外側の英字は範囲に含めない
    assert_eq!(art_runs(r"l|:/_\|"), vec![r"|:/_\|"]);
    assert_eq!(art_runs(r"|:/_\|l"), vec![r"|:/_\|"]);
    assert_eq!(art_runs(r"あl|:/_\|"), vec![r"|:/_\|"]);
    assert_eq!(art_runs(r"|:/_\|lあ"), vec![r"|:/_\|"]);
    // 橋渡しの英字の直後が空白で連なりが終わる時は、 範囲の末尾はその英字まで (空白は含めない)
    assert_eq!(art_runs(r"|:/_\|l あ"), vec![r"|:/_\|l"]);
    assert_eq!(art_runs(r"|:/_\|l  "), vec![r"|:/_\|l"]);
    // 橋渡しの英字が 2 つ続くと切れる (片方は前も後ろも記号ではない)
    assert!(art_runs("|:/ll/:|").is_empty());
}

#[test]
fn ascii_art_runs_split_and_end_at_the_right_places() {
    // 文字をはさんだ 2 つの連なりは別々に返す
    let input = r#"|\/_:"|あ|\/_:"|"#;
    assert_eq!(art_runs(input), vec![r#"|\/_:"|"#, r#"|\/_:"|"#]);
    let runs = ascii_art_runs(input);
    assert_eq!(runs[0].start, 0);
    assert_eq!(runs[1].end, input.len());
    // 末尾で終わる連なり (最後まで走査して返す)
    assert_eq!(art_runs(r#"あ|\/_:""#), vec![r#"|\/_:""#]);
    // 先頭・末尾の空白は範囲に含めず、 中の空白は含める
    assert_eq!(art_runs(r#"あ  |\/ _:"  い"#), vec![r#"|\/ _:""#]);
    // 空白を除いて数える (6 文字ちょうど)
    assert_eq!(art_runs(r#"|\ / _ : ""#), vec![r#"|\ / _ : ""#]);
    // 記号が無い / 空
    assert!(art_runs("").is_empty());
    assert!(art_runs("あいう abc").is_empty());
}

fn kinds_and_surfaces(input: &str) -> Vec<(ProtectedKind, &str)> {
    extract_protected_tokens(input)
        .into_iter()
        .map(|t| (t.kind, &input[t.range]))
        .collect()
}

#[test]
fn ascii_art_is_kept_next_to_other_tokens_but_dropped_when_overlapping() {
    let art = r#"|\/_:"|"#;
    // 隣り合うだけ (重ならない) なら、 両方取る: 直後に文字参照 / 直前にメール
    assert_eq!(
        kinds_and_surfaces(&format!("{art}&amp;")),
        vec![
            (ProtectedKind::AsciiArt, art),
            (ProtectedKind::Entity, "&amp;")
        ]
    );
    assert_eq!(
        kinds_and_surfaces(&format!("a@b.co{art}")),
        vec![
            (ProtectedKind::Email, "a@b.co"),
            (ProtectedKind::AsciiArt, art)
        ]
    );
    // 離れていれば両方取る
    assert_eq!(
        kinds_and_surfaces(&format!("あ&amp;い{art}")),
        vec![
            (ProtectedKind::Entity, "&amp;"),
            (ProtectedKind::AsciiArt, art)
        ]
    );
    // 先頭が他のトークンの末尾と重なる (文字参照の `;` が記号の連なりに入る) / 他のトークンを含む / 他のトークンに含まれる は取らない
    assert_eq!(
        kinds_and_surfaces(&format!("&amp;{art}")),
        vec![(ProtectedKind::Entity, "&amp;")]
    );
    assert_eq!(
        kinds_and_surfaces(&format!("(^_^){art}")),
        vec![(ProtectedKind::Kaomoji, "(^_^)")]
    );
    assert_eq!(
        kinds_and_surfaces(&format!("{art}(^_^)")),
        vec![(ProtectedKind::Kaomoji, "(^_^)")]
    );
    let url_input = format!("https://example.com/a{art}");
    let tokens = extract_protected_tokens(&url_input);
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].kind, ProtectedKind::Url);
}

#[test]
fn ascii_art_does_not_override_url_or_kaomoji() {
    // URL の中の記号は URL が取る
    let url = "https://example.com/a/b/c?x=1&y=2";
    let tokens = extract_protected_tokens(url);
    assert!(tokens.iter().all(|t| t.kind != ProtectedKind::AsciiArt));
    // 括弧の顔文字は顔文字のまま
    let kao = "(´・ω・`)";
    let tokens = extract_protected_tokens(kao);
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].kind, ProtectedKind::Kaomoji);
}

#[test]
fn kaomoji_in_parentheses_is_protected_with_empty_reading() {
    for kao in [
        "(´・ω・`)",
        "(;´Д`)",
        "(^o^)",
        "(≧▽≦)",
        "(T_T)",
        "ヽ(´ー｀)ノ",
        "＼(^o^)／",
        "(^^)/",
        "(^-^)v",
        "( ^ω^)",
        "( ̆ω ̆ )",
        "(* ́ω`*)",
        "ε=ε=(ノ≧∇≦)ノ",
        "ε=ε=ε=┌(;´Д`)┘",
    ] {
        let input = format!("あ{kao}い");
        let tokens = extract_protected_tokens(&input);
        assert_eq!(tokens.len(), 1, "{kao}");
        assert_eq!(tokens[0].kind, ProtectedKind::Kaomoji, "{kao}");
        assert_eq!(&input[tokens[0].range.clone()], kao, "{kao}");
    }
    let input = "(´・ω・`)";
    let provider = ProtectTokenProvider::new(input);
    let cands = provider.candidates_vec(&ctx(input), 0);
    assert_eq!(cands.len(), 1);
    assert_eq!(cands[0].reading, "");
}

#[test]
fn parentheses_that_are_not_faces_are_left_alone() {
    for text in [
        "(笑)",
        "(株)",
        "(A・B)",
        "(1)",
        "(abc)",
        "(あ・い)",
        "(a_b_c)",
        "()",
    ] {
        let tokens = extract_protected_tokens(text);
        assert!(
            tokens.iter().all(|t| t.kind != ProtectedKind::Kaomoji),
            "{text}"
        );
    }
    // 中身が長すぎるものは顔ではない (15 文字)
    let long = format!("({})", "´".repeat(15));
    assert!(extract_protected_tokens(&long)
        .iter()
        .all(|t| t.kind != ProtectedKind::Kaomoji));
    // 14 文字までは顔
    let ok = format!("({})", "^".repeat(14));
    assert!(extract_protected_tokens(&ok)
        .iter()
        .any(|t| t.kind == ProtectedKind::Kaomoji));
}

#[test]
fn html_entities_are_protected_as_silent_tokens() {
    for ent in [
        "&#8230;", "&#x2026;", "&amp;", "&lt;", "&gt;", "&quot;", "&nbsp;",
    ] {
        let input = format!("あ{ent}い");
        let tokens = extract_protected_tokens(&input);
        assert_eq!(tokens.len(), 1, "{ent}");
        assert_eq!(tokens[0].kind, ProtectedKind::Entity, "{ent}");
        assert_eq!(&input[tokens[0].range.clone()], ent);
    }
    // 実体参照ではない & は取らない
    for plain in ["A&B", "R&D", "&amp", "&#;", "&foo;", "a & b"] {
        assert!(
            extract_protected_tokens(plain)
                .iter()
                .all(|t| t.kind != ProtectedKind::Entity),
            "{plain}"
        );
    }
    // URL の中のものは URL が取る
    let url = "https://example.com/?a=1&amp;b=2";
    let tokens = extract_protected_tokens(url);
    assert!(tokens.iter().all(|t| t.kind != ProtectedKind::Entity));
}

#[test]
fn bare_kaomoji_without_parentheses_is_protected() {
    for kao in [
        "^^",
        "^_^",
        "^o^",
        "^-^",
        "TT",
        "T_T",
        "T-T",
        ";_;",
        ";ω;",
        ">_<",
        ">ω<",
        "orz",
        "Orz",
        "XD",
        ":)",
        ":D",
        "(^^;",
        "(;^_^A",
        "(^-^;",
        "(^^ゞ",
        "∩^ω^∩",
        "∩´ω`∩",
        "∩・ω・∩",
        "=ω=",
        "=^ω^=",
        "＝ω＝",
        "∩^ω^",
    ] {
        let input = format!("あ{kao} い");
        let tokens = extract_protected_tokens(&input);
        assert_eq!(tokens.len(), 1, "{kao}");
        assert_eq!(tokens[0].kind, ProtectedKind::Kaomoji, "{kao}");
        assert_eq!(&input[tokens[0].range.clone()], kao, "{kao}");
    }
    // 引用符の目
    assert_eq!(extract_protected_tokens("あ( '-' )い").len(), 1);
    // 顔ではないもの: 累乗 / Tシャツ / 英単語の一部 / 英数字に挟まれた記号
    for text in [
        "2^3^4",
        "x^2^3",
        "T-Tシャツ",
        "Corzo",
        "box:D",
        "ab:)cd",
        "http://a.com/^^",
        "A∩B",
        "a=ωb",
    ] {
        assert!(
            extract_protected_tokens(text)
                .iter()
                .all(|t| t.kind != ProtectedKind::Kaomoji),
            "{text}"
        );
    }
}

#[test]
fn url_is_not_replaced_by_kaomoji_reading() {
    let input = "https://a.com/(^o^)";
    let provider = ProtectTokenProvider::new(input);
    let cands = provider.candidates_vec(&ctx(input), 0);
    assert!(cands.iter().all(|c| !c.reading.is_empty()));
}

fn brand_lookup() -> Arc<HashMap<String, String>> {
    let mut m = HashMap::new();
    m.insert("hello".to_string(), "ハロー".to_string());
    m.insert("world".to_string(), "ワールド".to_string());
    m.insert("intel".to_string(), "インテル".to_string());
    m.insert("core".to_string(), "コア".to_string());
    Arc::new(m)
}

fn brand_readings(input: &str, starts: &[usize]) -> Vec<(String, String)> {
    let provider =
        AlphabetPassthroughProvider::new(input, brand_lookup()).split_at_model_starts(starts);
    provider
        .candidates_vec(&ctx(input), 0)
        .into_iter()
        .map(|c| (c.surface.to_string(), c.reading.to_string()))
        .collect()
}

#[test]
fn multi_word_range_is_read_when_every_word_is_known() {
    assert_eq!(
        brand_readings("Hello World", &[]),
        vec![("Hello World".to_string(), "ハロー ワールド".to_string())]
    );
    // 1 語でも未登録なら英文とみなして素通し
    assert_eq!(
        brand_readings("Hello there", &[]),
        vec![("Hello there".to_string(), "Hello there".to_string())]
    );
    assert_eq!(
        brand_readings("foo bar", &[]),
        vec![("foo bar".to_string(), "foo bar".to_string())]
    );
}

#[test]
fn range_before_a_model_reads_known_words_even_if_some_are_unknown() {
    // 型番の頭 (6) で range が切れ、 末尾の空白は読みにも残る
    assert_eq!(
        brand_readings("Foo Core i9", &[9]),
        vec![("Foo Core ".to_string(), "Foo コア ".to_string())]
    );
    assert_eq!(
        brand_readings("Intel i9", &[6]),
        vec![("Intel ".to_string(), "インテル ".to_string())]
    );
    // 1 語も知らなければそのまま
    assert_eq!(
        brand_readings("Foo Bar i9", &[8]),
        vec![("Foo Bar ".to_string(), "Foo Bar ".to_string())]
    );
}

#[test]
fn split_at_model_starts_cuts_only_the_range_that_contains_the_start() {
    let input = "Hello i9";
    let ranges = |starts: &[usize]| {
        AlphabetPassthroughProvider::passthrough_only(input)
            .split_at_model_starts(starts)
            .ranges()
            .to_vec()
    };
    assert_eq!(ranges(&[]), vec![0..8]);
    assert_eq!(ranges(&[6]), vec![0..6]);
    // 先頭と同じ位置 / 終端と同じ位置 / 範囲外では切らない
    assert_eq!(ranges(&[0]), vec![0..8]);
    assert_eq!(ranges(&[8]), vec![0..8]);
    assert_eq!(ranges(&[20]), vec![0..8]);
    // 複数あれば最初のもの
    assert_eq!(ranges(&[7, 6]), vec![0..6]);
}

fn sentence_lookup() -> Arc<HashMap<String, String>> {
    let mut m = HashMap::new();
    for (k, v) in [
        ("thank", "サンク"),
        ("love", "ラブ"),
        ("game", "ゲーム"),
        ("over", "オーバー"),
        ("much", "マッチ"),
        ("no", "ノー"),
    ] {
        m.insert(k.to_string(), v.to_string());
    }
    for (k, v) in [
        ("you", "ユー"),
        ("i", "アイ"),
        ("so", "ソー"),
        ("to", "トゥ"),
        ("a", "ア"),
    ] {
        m.insert(format!("{SENTENCE_KEY_PREFIX}{k}"), v.to_string());
    }
    Arc::new(m)
}

fn sentence_readings(input: &str) -> Vec<(String, String)> {
    let provider = AlphabetPassthroughProvider::new(input, sentence_lookup());
    provider
        .candidates_vec(&ctx(input), 0)
        .into_iter()
        .map(|c| (c.surface.to_string(), c.reading.to_string()))
        .collect()
}

fn one(input: &str) -> String {
    let r = sentence_readings(input);
    assert_eq!(r.len(), 1, "{input}: {r:?}");
    r[0].1.clone()
}

#[test]
fn sentence_words_are_used_only_inside_multi_word_english() {
    // 全部の語が辞書か英文の語表にあれば語ごとに読む
    assert_eq!(one("thank you so much"), "サンク ユー ソー マッチ");
    assert_eq!(one("I love you"), "アイ ラブ ユー");
    // 単独の so / to / a は引かない (従来どおり素通し)
    assert_eq!(one("so"), "so");
    assert_eq!(one("to"), "to");
    // 英文の語表に無い語が混じれば素通し
    assert_eq!(one("thank you very much"), "thank you very much");
}

#[test]
fn concatenated_words_are_split_into_known_words() {
    assert_eq!(one("thankyou"), "サンクユー");
    assert_eq!(one("iloveyou"), "アイラブユー");
    assert_eq!(one("GameOver"), "ゲームオーバー");
    assert_eq!(one("gameover"), "ゲームオーバー");
    assert_eq!(one("iLoveYou"), "アイラブユー");
    assert_eq!(one("thankyousomuch"), "サンクユーソーマッチ");
}

#[test]
fn concatenation_needs_every_part_known_and_a_real_word() {
    // 1 部分でも未知なら素通し
    assert_eq!(one("thankyoux"), "thankyoux");
    assert_eq!(one("gamexover"), "gamexover");
    // 英文の語だけ (辞書の語を含まない) は読まない
    assert_eq!(one("itisto"), "itisto");
    assert_eq!(one("youtoso"), "youtoso");
    // 1 語で辞書にある物は区切らず辞書どおり
    assert_eq!(one("thank"), "サンク");
}

#[test]
fn short_parts_cannot_sit_next_to_each_other_or_stand_alone_mid_word() {
    // 固有名詞 Aniimo (an + i + imo …) は割らない: 短い部分どうしの隣り合わせ / 途中の 1 文字は不可
    assert_eq!(one("aniimo"), "aniimo");
    assert_eq!(one("soupto"), "soupto");
    // 先頭の i / a は 1 文字でも可 (iloveyou)。 途中の 1 文字は不可
    assert_eq!(one("iloveyou"), "アイラブユー");
    assert_eq!(one("aloveyou"), "アラブユー");
    assert_eq!(one("youiloveyou"), "youiloveyou");
    // 長い語をはさめば短い語は使える (thankyousomuch の so)
    assert_eq!(one("thankyousomuch"), "サンクユーソーマッチ");
    // 短い語が隣り合う区切り (so + to) は採用しない
    assert_eq!(one("lovesoto"), "lovesoto");
}

#[test]
fn multi_word_needs_at_least_one_content_word() {
    // 機能語だけ (a a a a a / go up) は読まない
    assert_eq!(one("a a a a a"), "a a a a a");
    assert_eq!(one("up to you"), "up to you");
    // 辞書の語 (3 文字以上) が 1 つでもあれば読む
    assert_eq!(one("love you"), "ラブ ユー");
    assert_eq!(one("thank you"), "サンク ユー");
}

#[test]
fn concatenation_skips_short_acronyms_digits_and_long_inputs() {
    // 5 文字以下は区切らない / 6 文字からは区切る
    assert_eq!(one("noyou"), "noyou");
    assert_eq!(one("noyous"), "noyous");
    assert_eq!(one("loveyou"), "ラブユー");
    // 全部大文字 (略語) は対象外
    assert_eq!(one("THANKYOU"), "THANKYOU");
    // 数字・記号まじりは対象外 (英字 range に数字が入る)
    assert_eq!(one("thankyou2"), "thankyou2");
    // 長すぎる入力は試さない
    let long = "thank".repeat(9);
    assert_eq!(one(&long), long);
    let max = "thank".repeat(8);
    assert_eq!(one(&max), "サンク".repeat(8));
}

#[test]
fn word_break_prefers_fewest_parts() {
    let provider = AlphabetPassthroughProvider::new("x", sentence_lookup());
    let (r, parts, content) = provider.word_break("thankyou").unwrap();
    assert_eq!((r.as_str(), parts, content), ("サンクユー", 2, 1));
    // 3 字以上の辞書の語は内容語、 1〜2 字や英文の語は数えない
    let (_, _, content) = provider.word_break("iloveyou").unwrap();
    assert_eq!(content, 1);
    assert!(provider.word_break("zzzz").is_none());
    assert!(provider
        .word_break("")
        .is_none_or(|(r, p, _)| r.is_empty() && p == 0));
    // 辞書の 2 字の語 (no) は内容語として使わない: noyou は 英文の語表に no が無いので区切れない
    assert!(provider.word_break("noyou").is_none());
}

#[test]
fn camel_chunks_split_after_lowercase_before_uppercase() {
    assert_eq!(camel_chunks("GameOver"), vec!["Game", "Over"]);
    assert_eq!(camel_chunks("iLoveYou"), vec!["i", "Love", "You"]);
    assert_eq!(camel_chunks("gameover"), vec!["gameover"]);
    assert_eq!(camel_chunks("ABC"), vec!["ABC"]);
    assert_eq!(camel_chunks("aB"), vec!["a", "B"]);
    assert_eq!(camel_chunks("Ab"), vec!["Ab"]);
    assert_eq!(camel_chunks(""), vec![""]);
}

#[test]
fn word_reading_falls_back_to_sentence_words() {
    let provider = AlphabetPassthroughProvider::new("x", sentence_lookup());
    assert_eq!(provider.word_reading("thank"), Some("サンク"));
    assert_eq!(provider.word_reading("you"), Some("ユー"));
    assert_eq!(provider.word_reading("zzz"), None);
}

fn w_lookup() -> Arc<HashMap<String, String>> {
    let mut m = HashMap::new();
    m.insert("w".to_string(), "ワラ".to_string());
    m.insert("ww".to_string(), "ワラワラ".to_string());
    Arc::new(m)
}

fn w_reading(input: &str, pos: usize) -> String {
    let provider = AlphabetPassthroughProvider::new(input, w_lookup());
    provider
        .candidates_vec(&ctx(input), pos)
        .into_iter()
        .next()
        .map(|c| c.reading.to_string())
        .unwrap_or_default()
}

#[test]
fn w_next_to_an_operator_is_the_letter() {
    assert_eq!(w_reading("w=2", 0), "ダブリュー");
    assert_eq!(w_reading("x+w", 2), "ダブリュー");
    assert_eq!(w_reading("a*w", 2), "ダブリュー");
    // 演算子が無ければ笑い
    assert_eq!(w_reading("それなw", "それな".len()), "ワラ");
}

#[test]
fn single_w_before_a_particle_is_the_letter_not_laughter() {
    for p in ["は", "を", "が", "の", "も", "に", "で", "と"] {
        let input = format!("キーw{p}押す");
        assert_eq!(w_reading(&input, "キー".len()), "ダブリュー", "{input}");
    }
    assert_eq!(w_reading("W は小文字", 0), "ワラ");
    assert_eq!(w_reading("Wは小文字", 0), "ダブリュー");
    // 文頭 / 空白 / 括弧・引用符の後ろの小文字 w
    assert_eq!(w_reading("wは小文字", 0), "ダブリュー");
    assert_eq!(w_reading("これは wを押す", "これは ".len()), "ダブリュー");
    for open in ["「", "『", "(", "（", "【", "[", "\"", "“"] {
        let input = format!("{open}wを押す");
        assert_eq!(w_reading(&input, open.len()), "ダブリュー", "{input}");
    }
    // 大文字 W は何の後ろでも文字 (ガンダムWが)
    assert_eq!(w_reading("ガンダムWが", "ガンダム".len()), "ダブリュー");
}

#[test]
fn w_after_japanese_text_is_laughter_even_before_a_particle() {
    // 仮名・漢字・片仮名の後ろの w は、 助詞が続いても笑い (実コメントで誤判定が出た形)
    for prefix in ["なぁ", "対応", "ウェイ", "ね", "草", "すごい"] {
        for p in ["と", "は", "を", "が", "の", "も", "に", "で"] {
            let input = format!("{prefix}w{p}続く");
            assert_eq!(w_reading(&input, prefix.len()), "ワラ", "{input}");
        }
    }
    // 小文字 w + 長音の後ろ (キーw) だけが文字名
    assert_eq!(w_reading("キーwを押す", "キー".len()), "ダブリュー");
}

#[test]
fn w_at_the_end_or_repeated_stays_laughter() {
    assert_eq!(w_reading("ゆっくりw", "ゆっくり".len()), "ワラ");
    assert_eq!(w_reading("ゆっくりw。", "ゆっくり".len()), "ワラ");
    assert_eq!(w_reading("ゆっくりwwを", "ゆっくり".len()), "ワラワラ");
}

#[test]
fn digits_word_glued_reads_word_and_number_as_one_candidate() {
    assert_eq!(
        readings("Windows11"),
        vec![("Windows11".to_string(), "ウィンドウズイレブン".to_string())]
    );
    assert_eq!(
        readings("RTX4090"),
        vec![(
            "RTX4090".to_string(),
            "アールティーエックスヨンゼロキュウゼロ".to_string()
        )]
    );
}

#[test]
fn digits_word_not_listed_stays_passthrough() {
    // Max は [entries] にあるが [digits] には無い: Max5 を勝手に マックスファイブ にしない
    assert_eq!(
        readings("Max5"),
        vec![("Max5".to_string(), "Max5".to_string())]
    );
}

#[test]
fn digits_word_with_separator_extends_over_the_number() {
    for (input, surface, reading) in [
        ("Windows 10", "Windows 10", "ウィンドウズテン"),
        ("Windows-11", "Windows-11", "ウィンドウズイレブン"),
        ("No.1", "No.1", "ナンバーワン"),
    ] {
        let got = readings(input);
        assert!(
            got.contains(&(surface.to_string(), reading.to_string())),
            "{input}: {got:?}"
        );
    }
}

#[test]
fn digits_word_defers_to_decimal_and_version() {
    // 数字の直後が `.数字` なら [digits] の連結を作らない (iOS 17.2 / Ver2.0 は別に読む)
    assert!(!readings("Windows11.5")
        .iter()
        .any(|(_, r)| r == "ウィンドウズイレブン"));
    assert!(!readings("Windows 10.5")
        .iter()
        .any(|(s, _)| s == "Windows 10"));
}

#[test]
fn digits_override_reading_applies_only_with_a_number() {
    // 単独の No は [entries] の ノー のまま
    assert_eq!(readings("No"), vec![("No".to_string(), "ノー".to_string())]);
}

#[test]
fn letters_before_digits_gives_a_letters_only_candidate_before_japanese() {
    // SSR3体: 英字だけの passthrough も出し、 数字 + 助数詞を数値側に譲れるようにする
    let got = readings("SSR3体");
    assert!(got.contains(&("SSR3".to_string(), "SSR3".to_string())));
    assert!(got.contains(&("SSR".to_string(), "SSR".to_string())));
    // 日本語が続かない素の英数字には出さない
    let got = readings("SSR3");
    assert_eq!(got, vec![("SSR3".to_string(), "SSR3".to_string())]);
}

#[test]
fn letters_before_version_uses_the_dictionary_reading() {
    // Max2.0: 数字の続きが `.数字` の時だけ 英字部分の辞書 reading を使う
    let got = readings("Max2.0");
    assert!(got.contains(&("Max".to_string(), "マックス".to_string())));
    // 助数詞向けの切り出し (日本語が続く) では辞書 reading を使わない
    let got = readings("Max5体");
    assert!(got.contains(&("Max".to_string(), "Max".to_string())));
}

#[test]
fn dictionary_hit_wins_over_the_unit_skip_only_for_uppercase() {
    // 5G は SI 単位 g と紛れる: 大文字表記は辞書 (ファイブジー)、 小文字 5g は単位側に譲る
    let units: Arc<HashSet<String>> = Arc::new(["g".to_string()].into_iter().collect());
    let upper = AlphabetPassthroughProvider::with_units("5G", digits_lookup(), units.clone());
    assert_eq!(upper.candidates_vec(&ctx("5G"), 0).len(), 1);
    let lower = AlphabetPassthroughProvider::with_units("5g", digits_lookup(), units);
    assert!(lower.candidates_vec(&ctx("5g"), 0).is_empty());
}

#[test]
fn letters_before_digits_only_for_letters_then_digits() {
    assert_eq!(letters_before_digits("SSR3"), Some(3));
    assert_eq!(letters_before_digits("Windows11"), Some(7));
    // 英字で始まらない / 数字だけ / 英字だけ / 数字の後に英字が続く形は対象外
    assert_eq!(letters_before_digits("123"), None);
    assert_eq!(letters_before_digits("5G"), None);
    assert_eq!(letters_before_digits("abc"), None);
    assert_eq!(letters_before_digits("H2A"), None);
}

//! [`NumberCandidateProvider`] の integration-level tests
//! (fixture rules を load して `candidates_at` を直接叩く)。

use super::*;
use crate::loader::load_rules_dir;
use crate::scoring::boundary::BoundaryAnalysis;
use std::path::PathBuf;

fn ctx(input: &str) -> ScoringContext<'_> {
    let boundary = Box::leak(Box::new(BoundaryAnalysis::empty()));
    ScoringContext { input, boundary }
}

fn rules() -> RulesData {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("rules");
    load_rules_dir(&dir).expect("load rules failed")
}

fn provider() -> NumberCandidateProvider {
    NumberCandidateProvider::new(&rules())
}

fn find<'a>(cands: &'a [Candidate], surface: &str) -> Option<&'a Candidate> {
    cands.iter().find(|c| c.surface == surface)
}

// ─── 構築 / 空入力 ───────────────────────────────────────────────────────

#[test]
fn empty_rules_yields_empty_candidates_for_pure_number() {
    let p = NumberCandidateProvider::new(&RulesData::default());
    let cands = p.candidates_at(&ctx("3"), 0);
    // counter / scale / si_unit / symbol いずれも空、 しかし DIGIT は static なので 1 候補
    assert_eq!(cands.len(), 1);
    assert_eq!(cands[0].surface, "3");
    assert_eq!(cands[0].score.band, BAND_SPECIAL);
}

#[test]
fn empty_input_yields_empty() {
    let p = provider();
    assert!(p.candidates_at(&ctx(""), 0).is_empty());
}

#[test]
fn pos_at_end_yields_empty() {
    let p = provider();
    let input = "3本";
    assert!(p.candidates_at(&ctx(input), input.len()).is_empty());
}

// ─── 単一助数詞 ──────────────────────────────────────────────────────────

#[test]
fn single_counter_basic() {
    let p = provider();
    let cands = p.candidates_at(&ctx("3本のバナナ"), 0);
    let c = find(&cands, "3本").expect("3本 candidate");
    assert_eq!(c.reading, "サンボン");
    assert_eq!(c.score.band, BAND_SPECIAL);
    assert_eq!(c.score.length, 2); // "3" + "本" = 2 文字
}

#[test]
fn recursive_counter_me_through_provider() {
    // 「2 個目」 = 個 (base) + 目 (recursive) を 1 token 化 → ニコメ。
    // regression: 以前は 「2個」 で止まり 「目」 が単漢字 fallback で 「モク」 と
    // 誤読されて 「ニコモク」 になっていた。
    let p = provider();
    let c2 = find(&p.candidates_at(&ctx("2個目"), 0), "2個目")
        .expect("2個目 candidate")
        .clone();
    assert_eq!(c2.reading, "ニコメ");
    // 「5 人目」 = 人 special (5→ゴニン) + メ → ゴニンメ
    let c5 = find(&p.candidates_at(&ctx("5人目"), 0), "5人目")
        .expect("5人目 candidate")
        .clone();
    assert_eq!(c5.reading, "ゴニンメ");
    // 「3 回目」 = 回 default + メ → サンカイメ
    let c3 = find(&p.candidates_at(&ctx("3回目"), 0), "3回目")
        .expect("3回目 candidate")
        .clone();
    assert_eq!(c3.reading, "サンカイメ");
}

#[test]
fn recursive_counter_me_kanji_numeral() {
    // 漢数字版: 「一個目」 → イッコメ (個 + 目)。 旧: 個 までで止まり 目 → モク。
    let p = provider();
    let c1 = find(&p.candidates_at(&ctx("一個目"), 0), "一個目")
        .expect("一個目 candidate")
        .clone();
    assert_eq!(c1.reading, "イッコメ");
    // 「二回目」 → ニカイメ
    let c2 = find(&p.candidates_at(&ctx("二回目"), 0), "二回目")
        .expect("二回目 candidate")
        .clone();
    assert_eq!(c2.reading, "ニカイメ");
    // bare 漢数字 + 助数詞 (目なし) は candidate にしない (chunker 互換維持)
    assert!(
        find(&p.candidates_at(&ctx("一個"), 0), "一個").is_none(),
        "漢数字 + 助数詞 (目なし) は kanji recursive regex に match しない"
    );
}

#[test]
fn single_counter_includes_bare_digit_too() {
    // 「3本」 の位置 0 では digit "3" 候補も同時に提案される (DP が長い方を選ぶ)
    let p = provider();
    let cands = p.candidates_at(&ctx("3本のバナナ"), 0);
    assert!(
        find(&cands, "3").is_some(),
        "bare digit candidate should exist"
    );
    assert!(
        find(&cands, "3本").is_some(),
        "counter candidate should exist"
    );
}

#[test]
fn single_counter_zero_no_sokuon() {
    let p = provider();
    let cands = p.candidates_at(&ctx("0本"), 0);
    let c = find(&cands, "0本").expect("0本 candidate");
    assert_eq!(c.reading, "ゼロホン");
}

#[test]
fn single_counter_day_uses_period_default() {
    // 「N日」 単独は **期間扱い**: days.toml 特殊読み (1=ツイタチ) を bypass、 default 「ニチ」
    let p = provider();
    let cands = p.candidates_at(&ctx("1日に2回"), 0);
    let c = find(&cands, "1日").expect("1日 candidate");
    assert_eq!(c.reading, "イチニチ");
}

#[test]
fn single_counter_handles_full_width_digit() {
    let p = provider();
    let cands = p.candidates_at(&ctx("３本"), 0);
    let c = find(&cands, "３本").expect("full-width counter candidate");
    assert_eq!(c.reading, "サンボン");
}

#[test]
fn single_counter_kansuji_only_in_date_pattern() {
    // 漢数字 「一日」 単独は counter_re (NUM_PAT = Arabic 数字限定) では match しない、
    // 既存 chunker と同じ挙動 (= 漢数字 normalization は DATE_NUM_PAT 経由でのみ動く)。
    let p = provider();
    let cands = p.candidates_at(&ctx("一日中"), 0);
    assert!(
        find(&cands, "一日").is_none(),
        "漢数字 単独 + counter は candidate にならない (chunker 互換): {cands:?}",
    );
}

#[test]
fn date_md_normalizes_kansuji() {
    // 日付 pattern 内の漢数字は kansuji_to_arabic で normalize される。
    let p = provider();
    let cands = p.candidates_at(&ctx("六月一日"), 0);
    let c = find(&cands, "六月一日").expect("date MD with kansuji");
    // 一日 → days.toml の特殊読み (ツイタチ) を採用
    assert!(c.reading.contains("ツイタチ"), "reading: {}", c.reading);
    assert!(c.reading.contains("ロクガツ"), "reading: {}", c.reading);
}

// ─── 日付 ────────────────────────────────────────────────────────────────

#[test]
fn date_full_emits_single_candidate() {
    let p = provider();
    let cands = p.candidates_at(&ctx("2025年10月30日に集合"), 0);
    let c = find(&cands, "2025年10月30日").expect("date full candidate");
    assert!(c.reading.contains("ジュウガツ"), "reading: {}", c.reading);
    assert_eq!(c.score.band, BAND_SPECIAL);
}

#[test]
fn date_md_uses_special_day_reading() {
    // 日付内 「1日」 は days.toml の 「ツイタチ」
    let p = provider();
    let cands = p.candidates_at(&ctx("1月1日に集合"), 0);
    let c = find(&cands, "1月1日").expect("date MD candidate");
    assert!(c.reading.contains("イチガツ"), "reading: {}", c.reading);
    assert!(c.reading.contains("ツイタチ"), "reading: {}", c.reading);
}

// ─── 時刻 ────────────────────────────────────────────────────────────────

#[test]
fn time_colon_basic() {
    let p = provider();
    let cands = p.candidates_at(&ctx("9:30に集合"), 0);
    let c = find(&cands, "9:30").expect("time colon candidate");
    assert!(c.reading.contains("クジ"), "reading: {}", c.reading);
    assert!(
        c.reading.contains("サンジュッフン") || c.reading.contains("サンジュップン"),
        "reading: {}",
        c.reading,
    );
}

#[test]
fn time_jp_full() {
    let p = provider();
    let cands = p.candidates_at(&ctx("9時30分に集合"), 0);
    let c = find(&cands, "9時30分").expect("time JP candidate");
    assert!(c.reading.contains("クジ"), "reading: {}", c.reading);
}

#[test]
fn time_jp_hour_only() {
    let p = provider();
    let cands = p.candidates_at(&ctx("9時に集合"), 0);
    let c = find(&cands, "9時").expect("time JP hour-only candidate");
    assert_eq!(c.reading, "クジ");
}

// ─── 大数スケール ────────────────────────────────────────────────────────

#[test]
fn scale_with_trailing_unit_when_units_table_has_kanji_unit() {
    // fixture rules の units は SI 単位 (km / L 等) のみで 「円」 を含まないので、
    // build_scale_regex の trailing_unit は None になる。 scale candidate は 「3万」 で出る。
    let p = provider();
    let cands = p.candidates_at(&ctx("3万円のもの"), 0);
    // chunker の split_scale テストと同じく、 「3万」 OR 「3万円」 のどちらかが候補化される
    let has_scale = cands
        .iter()
        .any(|c| (c.surface == "3万" || c.surface == "3万円") && !c.reading.is_empty());
    assert!(has_scale, "no scale candidate found: {cands:?}");
}

#[test]
fn scale_without_trailing_unit() {
    let p = provider();
    let cands = p.candidates_at(&ctx("3万"), 0);
    let c = find(&cands, "3万").expect("scale candidate");
    assert!(c.reading.contains("マン"), "reading: {}", c.reading);
}

// ─── SI 単位 ─────────────────────────────────────────────────────────────

#[test]
fn si_unit_basic() {
    let p = provider();
    let cands = p.candidates_at(&ctx("100km先"), 0);
    let c = find(&cands, "100km").expect("SI unit candidate");
    assert!(c.reading.contains("ヒャク"), "reading: {}", c.reading);
    assert!(c.reading.contains("キロメートル"), "reading: {}", c.reading);
}

// ─── 記号 ────────────────────────────────────────────────────────────────

#[test]
fn symbol_single_char() {
    let p = provider();
    let cands = p.candidates_at(&ctx("+5"), 0);
    let c = find(&cands, "+").expect("symbol candidate");
    assert_eq!(c.reading, "プラス");
    assert_eq!(c.score.length, 1);
}

#[test]
fn symbol_skipped_when_not_in_table() {
    // counters.toml の simple に 「‰」 もあるが symbols.toml fixture には未登録だと no-op
    // (= '※' のような未登録記号は 7 番からは候補出ず、 8 番素の数字でも該当しない)
    let p = provider();
    let cands = p.candidates_at(&ctx("※"), 0);
    // 候補ゼロ (記号 table miss + digit miss)
    assert!(cands.is_empty(), "expected no candidates: {cands:?}");
}

#[test]
fn tilde_emits_kara_in_numeric_context() {
    // 「2〜3回」 のような range context では 〜 → から (= 既存挙動維持)。
    let p = provider();
    let input = "2〜3回";
    let pos = "2".len(); // 〜 の byte position
    let cands = p.candidates_at(&ctx(input), pos);
    let c = find(&cands, "〜").expect("tilde candidate in numeric context");
    assert_eq!(c.reading, "から");
}

#[test]
fn tilde_silent_in_kana_context() {
    // 「へ〜うま」 のような kana context では range ではなく長音・強調用途。
    // 「から」 は誤読なので空 reading で surface のみ消費する。
    let p = provider();
    let input = "へ〜うま";
    let pos = "へ".len(); // 〜 の byte position
    let cands = p.candidates_at(&ctx(input), pos);
    let c = find(&cands, "〜").expect("tilde candidate in kana context");
    assert_eq!(c.reading, "", "kana 文脈の 〜 は読み上げない (空 reading)");
}

#[test]
fn tilde_silent_at_end_of_string() {
    // 「がんばれ〜」 のような文末 (= prev kana / next 無し) でも 「から」 は誤読。
    // 空 reading で消費し 「がんばれ」 と読む。
    let p = provider();
    let input = "がんばれ〜";
    let pos = "がんばれ".len(); // 末尾 〜 の byte position
    let cands = p.candidates_at(&ctx(input), pos);
    let c = find(&cands, "〜").expect("tilde candidate at end");
    assert_eq!(c.reading, "", "文末の 〜 は読み上げない (空 reading)");
}

#[test]
fn tilde_emits_kara_when_only_prev_is_digit() {
    // 「2〜あ」 のように prev だけ数字でも range 文脈 (= 「2 から あ」 的、 不自然だが
    // range 解釈は許容)。
    let p = provider();
    let input = "2〜あ";
    let pos = "2".len();
    let cands = p.candidates_at(&ctx(input), pos);
    assert!(cands
        .iter()
        .any(|c| c.surface == "〜" && c.reading == "から"));
}

// ─── 素の数字 ────────────────────────────────────────────────────────────

#[test]
fn bare_digit_basic() {
    let p = provider();
    let cands = p.candidates_at(&ctx("12345です"), 0);
    let c = find(&cands, "12345").expect("bare digit candidate");
    assert!(!c.reading.is_empty());
    assert_eq!(c.score.band, BAND_SPECIAL);
}

#[test]
fn bare_digit_handles_full_width() {
    let p = provider();
    let cands = p.candidates_at(&ctx("１２３"), 0);
    let c = find(&cands, "１２３").expect("full-width digit candidate");
    assert_eq!(c.reading, "ヒャクニジュウサン");
}

// ─── 複数候補の同位置出力 ────────────────────────────────────────────────

#[test]
fn date_md_and_counter_both_emitted_at_pos_0() {
    // 「1月1日」 の pos 0 で 「1月1日」 (date MD) と 「1月」 (counter) が並列に出る
    // (DP が edge_count で longer match を選ぶ責務)
    let p = provider();
    let cands = p.candidates_at(&ctx("1月1日"), 0);
    assert!(find(&cands, "1月1日").is_some(), "date candidate");
    assert!(find(&cands, "1月").is_some(), "counter candidate");
}

#[test]
fn si_and_scale_dont_collide_for_pure_number() {
    // 「100」 単独 (unit / scale なし) は digit のみ
    let p = provider();
    let cands = p.candidates_at(&ctx("100"), 0);
    // "100" digit candidate
    assert!(find(&cands, "100").is_some(), "digit candidate");
    // SI 候補は出ない (single の k や m もないため)
    assert!(find(&cands, "100m").is_none());
}

// ─── range の正しさ ─────────────────────────────────────────────────────

#[test]
fn candidate_range_aligns_with_input_bytes() {
    let p = provider();
    let input = "abc3本";
    let pos = 3; // "abc" 後の "3" 位置 (3 ASCII bytes)
    let cands = p.candidates_at(&ctx(input), pos);
    let c = find(&cands, "3本").expect("3本 candidate at offset 3");
    // "3本" = "3" (1 byte) + "本" (3 bytes UTF-8) = 4 bytes
    assert_eq!(c.range, 3..7);
}

// ─── debug: empty rules でも static regex の DIGIT は走る ───────────────

#[test]
fn digit_regex_is_static_and_works_with_empty_rules() {
    let p = NumberCandidateProvider::new(&RulesData::default());
    let cands = p.candidates_at(&ctx("42x"), 0);
    let c = find(&cands, "42").expect("bare digit candidate even with empty rules");
    assert!(!c.reading.is_empty());
}

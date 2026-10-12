//! [`NumberCandidateProvider`] の integration-level tests
//! (fixture rules を load して `candidates_at` を直接叩く)。

use super::*;
use crate::loader::load_rules_dir;
use crate::scoring::boundary::BoundaryAnalysis;
use crate::scoring::candidate::Candidate;
use std::path::PathBuf;

fn ctx(input: &str) -> ScoringContext<'_> {
    let boundary = Box::leak(Box::new(BoundaryAnalysis::empty()));
    ScoringContext::new(input, boundary)
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
    let cands = p.candidates_vec(&ctx("3"), 0);
    // counter / scale / si_unit / symbol いずれも空、 しかし DIGIT は static なので 1 候補
    assert_eq!(cands.len(), 1);
    assert_eq!(cands[0].surface, "3");
    assert_eq!(cands[0].score.band, BAND_SPECIAL);
}

#[test]
fn empty_input_yields_empty() {
    let p = provider();
    assert!(p.candidates_vec(&ctx(""), 0).is_empty());
}

#[test]
fn pos_at_end_yields_empty() {
    let p = provider();
    let input = "3本";
    assert!(p.candidates_vec(&ctx(input), input.len()).is_empty());
}

// ─── 数詞慣用語句 (numeric_phrases、 ★0.2.0 再統合) ──────────────────────

#[test]
fn phrase_basic_hatachi() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("二十歳になった"), 0);
    let c = find(&cands, "二十歳").expect("二十歳 candidate");
    assert_eq!(c.reading, "ハタチ");
    assert_eq!(c.score.band, BAND_SPECIAL);
    assert_eq!(c.score.length, 3);
}

#[test]
fn phrase_non_digit_lead_asatte() {
    // 「明後日」 は先頭が数字系 char ではない → numeric lead guard より前の
    // try_phrase で拾われることを確認する (guard 後だと絶対に出ない)。
    let p = provider();
    let cands = p.candidates_vec(&ctx("明後日は晴れ"), 0);
    let c = find(&cands, "明後日").expect("明後日 candidate");
    assert_eq!(c.reading, "アサッテ");
}

#[test]
fn phrase_overlapping_surfaces_both_emitted() {
    // 「一人前」 の pos 0 では 「一人前」 と 「一人」 の両方が candidate に上がる。
    // 採択は DP の length / edge_count 軸 (長い方が勝つ) に委ねる。
    let p = provider();
    let cands = p.candidates_vec(&ctx("一人前の寿司"), 0);
    assert_eq!(
        find(&cands, "一人前").map(|c| c.reading.as_str()),
        Some("イチニンマエ")
    );
    assert_eq!(
        find(&cands, "一人").map(|c| c.reading.as_str()),
        Some("ヒトリ")
    );
}

#[test]
fn phrase_empty_rules_yields_no_phrase_candidates() {
    let p = NumberCandidateProvider::new(&RulesData::default());
    let cands = p.candidates_vec(&ctx("二十歳"), 0);
    assert!(
        find(&cands, "二十歳").is_none(),
        "空 rules では phrase candidate なし: {cands:?}"
    );
}

// ─── 単一助数詞 ──────────────────────────────────────────────────────────

#[test]
fn single_counter_basic() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("3本のバナナ"), 0);
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
    let c2 = find(&p.candidates_vec(&ctx("2個目"), 0), "2個目")
        .expect("2個目 candidate")
        .clone();
    assert_eq!(c2.reading, "ニコメ");
    // 「5 人目」 = 人 special (5→ゴニン) + メ → ゴニンメ
    let c5 = find(&p.candidates_vec(&ctx("5人目"), 0), "5人目")
        .expect("5人目 candidate")
        .clone();
    assert_eq!(c5.reading, "ゴニンメ");
    // 「3 回目」 = 回 default + メ → サンカイメ
    let c3 = find(&p.candidates_vec(&ctx("3回目"), 0), "3回目")
        .expect("3回目 candidate")
        .clone();
    assert_eq!(c3.reading, "サンカイメ");
}

#[test]
fn recursive_counter_me_kanji_numeral() {
    // 漢数字版: 「一個目」 → イッコメ (個 + 目)。 旧: 個 までで止まり 目 → モク。
    let p = provider();
    let c1 = find(&p.candidates_vec(&ctx("一個目"), 0), "一個目")
        .expect("一個目 candidate")
        .clone();
    assert_eq!(c1.reading, "イッコメ");
    // 「二回目」 → ニカイメ
    let c2 = find(&p.candidates_vec(&ctx("二回目"), 0), "二回目")
        .expect("二回目 candidate")
        .clone();
    assert_eq!(c2.reading, "ニカイメ");
    // bare 漢数字 + 助数詞 (目なし) は candidate にしない (chunker 互換維持)
    assert!(
        find(&p.candidates_vec(&ctx("一個"), 0), "一個").is_none(),
        "漢数字 + 助数詞 (目なし) は kanji recursive regex に match しない"
    );
}

#[test]
fn single_counter_includes_bare_digit_too() {
    // 「3本」 の位置 0 では digit "3" 候補も同時に提案される (DP が長い方を選ぶ)
    let p = provider();
    let cands = p.candidates_vec(&ctx("3本のバナナ"), 0);
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
    let cands = p.candidates_vec(&ctx("0本"), 0);
    let c = find(&cands, "0本").expect("0本 candidate");
    assert_eq!(c.reading, "ゼロホン");
}

#[test]
fn single_counter_day_uses_period_default() {
    // 「N日」 単独は **期間扱い**: days.toml 特殊読み (1=ツイタチ) を bypass、 default 「ニチ」
    let p = provider();
    let cands = p.candidates_vec(&ctx("1日に2回"), 0);
    let c = find(&cands, "1日").expect("1日 candidate");
    assert_eq!(c.reading, "イチニチ");
}

#[test]
fn single_counter_handles_full_width_digit() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("３本"), 0);
    let c = find(&cands, "３本").expect("full-width counter candidate");
    assert_eq!(c.reading, "サンボン");
}

#[test]
fn single_counter_kansuji_only_in_date_pattern() {
    // 漢数字 「一日」 単独は counter_re (NUM_PAT = Arabic 数字限定) では match しない、
    // 既存 chunker と同じ挙動 (= 漢数字 normalization は DATE_NUM_PAT 経由でのみ動く)。
    let p = provider();
    let cands = p.candidates_vec(&ctx("一日中"), 0);
    assert!(
        find(&cands, "一日").is_none(),
        "漢数字 単独 + counter は candidate にならない (chunker 互換): {cands:?}",
    );
}

#[test]
fn date_md_normalizes_kansuji() {
    // 日付 pattern 内の漢数字は kansuji_to_arabic で normalize される。
    let p = provider();
    let cands = p.candidates_vec(&ctx("六月一日"), 0);
    let c = find(&cands, "六月一日").expect("date MD with kansuji");
    // 一日 → days.toml の特殊読み (ツイタチ)。contains だと月部の脱落・順序入替を
    // 見逃すので全文一致で固定。
    assert_eq!(c.reading, "ロクガツツイタチ");
}

#[test]
fn date_kansuji_consecutive_digits_are_positional() {
    // 法令・判例の 「一二月一日」 (12 月) / 「三月二三日」 (23 日)。 日付の欄に概数 (二三 = 2〜3) は来ない
    let p = provider();
    let cands = p.candidates_vec(&ctx("一二月一日"), 0);
    let c = find(&cands, "一二月一日").expect("date MD positional month");
    assert_eq!(c.reading, "ジュウニガツツイタチ");
    let cands = p.candidates_vec(&ctx("三月二三日"), 0);
    let c = find(&cands, "三月二三日").expect("date MD positional day");
    assert_eq!(c.reading, "サンガツニジュウサンニチ");
}

#[test]
fn fraction_with_optional_spaces() {
    // 分数は 分 = ブン (助数詞 分 = フン と区別)。 Qiita 等の 「33 分の 10」 のような空白入りも 1 候補にする
    let p = provider();
    let cands = p.candidates_vec(&ctx("33 分の 10 という"), 0);
    let c = find(&cands, "33 分の 10").expect("spaced fraction");
    assert_eq!(c.reading, "サンジュウサンブンノジュウ");
    let cands = p.candidates_vec(&ctx("3分の1"), 0);
    let c = find(&cands, "3分の1").expect("fraction");
    assert_eq!(c.reading, "サンブンノイチ");
}

#[test]
fn counter_allows_single_space_but_not_before_kanji_word() {
    // 「5 分」 = ゴフン (空白 1 つを許す)。 ただし空白入りで助数詞の直後が漢字なら別の語 (15 本体 / 3 人間)
    let p = provider();
    let cands = p.candidates_vec(&ctx("5 分の遅れ"), 0);
    assert_eq!(
        find(&cands, "5 分").expect("spaced counter").reading,
        "ゴフン"
    );
    let cands = p.candidates_vec(&ctx("15 本体"), 0);
    assert!(
        find(&cands, "15 本").is_none(),
        "15 本体 の 本 は助数詞にしない: {cands:?}"
    );
}

// ─── 日付 ────────────────────────────────────────────────────────────────

#[test]
fn date_full_emits_single_candidate() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("2025年10月30日に集合"), 0);
    let c = find(&cands, "2025年10月30日").expect("date full candidate");
    // 年・月・日すべてを完全一致で固定 (contains だと年/日の脱落を見逃す)。
    assert_eq!(c.reading, "ニセンニジュウゴネンジュウガツサンジュウニチ");
    assert_eq!(c.score.band, BAND_SPECIAL);
}

#[test]
fn date_md_uses_special_day_reading() {
    // 日付内 「1日」 は days.toml の 「ツイタチ」
    let p = provider();
    let cands = p.candidates_vec(&ctx("1月1日に集合"), 0);
    let c = find(&cands, "1月1日").expect("date MD candidate");
    assert_eq!(c.reading, "イチガツツイタチ");
}

// ─── 時刻 ────────────────────────────────────────────────────────────────

#[test]
fn time_colon_basic() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("9:30に集合"), 0);
    let c = find(&cands, "9:30").expect("time colon candidate");
    // 時・分を通して全文固定 (fixture rules が決定する促音形を pin)。
    assert_eq!(c.reading, "クジサンジュップン");
}

#[test]
fn time_jp_full() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("9時30分に集合"), 0);
    let c = find(&cands, "9時30分").expect("time JP candidate");
    // 旧 test は分部 (30分) を一切見ていなかった。全文固定。
    assert_eq!(c.reading, "クジサンジュップン");
}

#[test]
fn time_jp_hour_only() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("9時に集合"), 0);
    let c = find(&cands, "9時").expect("time JP hour-only candidate");
    assert_eq!(c.reading, "クジ");
}

// ─── 大数スケール ────────────────────────────────────────────────────────

#[test]
fn scale_with_trailing_unit_when_units_table_has_kanji_unit() {
    // fixture rules の units は SI 単位 (km / L 等) のみで 「円」 を含まないので、
    // build_scale_regex の trailing_unit は None になる。 scale candidate は 「3万」 で出る。
    let p = provider();
    let cands = p.candidates_vec(&ctx("3万円のもの"), 0);
    // fixture units に 「円」 は無いので scale candidate は 「3万」。!is_empty() だと
    // 誤読でも緑になるので読みを完全一致で固定する。
    let c = find(&cands, "3万").expect("scale candidate 3万");
    assert_eq!(c.reading, "サンマン");
}

#[test]
fn scale_without_trailing_unit() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("3万"), 0);
    let c = find(&cands, "3万").expect("scale candidate");
    assert_eq!(c.reading, "サンマン");
}

// ─── SI 単位 ─────────────────────────────────────────────────────────────

#[test]
fn si_unit_basic() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("100km先"), 0);
    let c = find(&cands, "100km").expect("SI unit candidate");
    // 数値部+単位部を通して固定 (順序 「キロメートルヒャク」 等の誤結合を排除)。
    // 100 + カ行の単位 は促音化する (★2026-09-15: ヒャク → ヒャッ)。
    assert_eq!(c.reading, "ヒャッキロメートル");
}

// ─── 記号 ────────────────────────────────────────────────────────────────

#[test]
fn symbol_single_char() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("+5"), 0);
    let c = find(&cands, "+").expect("symbol candidate");
    assert_eq!(c.reading, "プラス");
    assert_eq!(c.score.length, 1);
}

#[test]
fn symbol_skipped_when_not_in_table() {
    // counters.toml の simple に 「‰」 もあるが symbols.toml fixture には未登録だと no-op
    // (= '※' のような未登録記号は 7 番からは候補出ず、 8 番素の数字でも該当しない)
    let p = provider();
    let cands = p.candidates_vec(&ctx("※"), 0);
    // 候補ゼロ (記号 table miss + digit miss)
    assert!(cands.is_empty(), "expected no candidates: {cands:?}");
}

#[test]
fn tilde_emits_kara_in_numeric_context() {
    // 「2〜3回」 のような range context では 〜 → から (= 既存挙動維持)。
    let p = provider();
    let input = "2〜3回";
    let pos = "2".len(); // 〜 の byte position
    let cands = p.candidates_vec(&ctx(input), pos);
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
    let cands = p.candidates_vec(&ctx(input), pos);
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
    let cands = p.candidates_vec(&ctx(input), pos);
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
    let cands = p.candidates_vec(&ctx(input), pos);
    assert!(cands
        .iter()
        .any(|c| c.surface == "〜" && c.reading == "から"));
}

// ─── `-` = マイナス vs ハイフン (数字 context 限定) ─────────────────────

#[test]
fn minus_between_digits_is_silent() {
    // 2026-09-11 変更: 数字に挟まれた `-` は減算ではなく区切り。
    // 実データ 704 万行のハイフンは 3-1 / 22-0 (スコア)、 マリオ 2-1 (面数)、
    // 2024-2025 (年範囲) ばかりで、 マイナス と読むべき例は無かった。
    let p = provider();
    let input = "3-1";
    let pos = "3".len();
    let cands = p.candidates_vec(&ctx(input), pos);
    let c = find(&cands, "-").expect("hyphen candidate between digits");
    assert_eq!(c.reading, "", "数字間の - は読み上げない");
    assert!(
        !cands.iter().any(|c| c.reading.starts_with("マイナス")),
        "符号付き数値候補も出さない: {cands:?}"
    );
}

#[test]
fn minus_emits_reading_when_only_next_is_digit() {
    // 「-3」 (負数 / 文頭) も数字 context。
    let p = provider();
    let cands = p.candidates_vec(&ctx("-3"), 0);
    assert!(cands
        .iter()
        .any(|c| c.surface == "-" && c.reading == "マイナス"));
}

#[test]
fn hyphen_between_letters_is_silent() {
    // 「Wi-Fi」 の `-` は語内ハイフン。 マイナスと読むと誤読 (実運用の誤読報告より)。
    let p = provider();
    let input = "Wi-Fi";
    let pos = "Wi".len();
    let cands = p.candidates_vec(&ctx(input), pos);
    let c = find(&cands, "-").expect("hyphen candidate between letters");
    assert_eq!(c.reading, "", "英字間の - は読み上げない (空 reading)");
}

#[test]
fn hyphen_in_kana_context_is_silent() {
    // 「あ-い」 「終わり-」 のような区切り用途も読まない。
    let p = provider();
    let cands = p.candidates_vec(&ctx("あ-い"), "あ".len());
    let c = find(&cands, "-").expect("hyphen candidate in kana context");
    assert_eq!(c.reading, "");

    let cands = p.candidates_vec(&ctx("終わり-"), "終わり".len());
    let c = find(&cands, "-").expect("hyphen candidate at end");
    assert_eq!(c.reading, "");
}

#[test]
fn fullwidth_minus_follows_same_context_rule() {
    // 全角 `－` / 数学記号 `−` も半角と同じ扱い。 負数 (前が数字でない) は マイナス、
    // 数字に挟まれた区切りは無音。
    let p = provider();
    for ch in ['\u{FF0D}', '\u{2212}'] {
        let neg = format!("{ch}3");
        let cands = p.candidates_vec(&ctx(&neg), 0);
        assert!(
            cands
                .iter()
                .any(|c| c.surface == ch.to_string() && c.reading == "マイナス"),
            "負数の {ch} は マイナス"
        );

        let score = format!("3{ch}1");
        let pos = "3".len();
        let cands = p.candidates_vec(&ctx(&score), pos);
        let c = find(&cands, &ch.to_string()).expect("hyphen candidate");
        assert_eq!(c.reading, "", "数字間の {ch} は無音");
    }
}

// ─── 素の数字 ────────────────────────────────────────────────────────────

#[test]
fn bare_digit_basic() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("12345です"), 0);
    let c = find(&cands, "12345").expect("bare digit candidate");
    // 数値合成の本丸経路。!is_empty() だと桁上がりの誤りを見逃すので完全一致で固定。
    assert_eq!(c.reading, "イチマンニセンサンビャクヨンジュウゴ");
    assert_eq!(c.score.band, BAND_SPECIAL);
}

#[test]
fn bare_digit_handles_full_width() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("１２３"), 0);
    let c = find(&cands, "１２３").expect("full-width digit candidate");
    assert_eq!(c.reading, "ヒャクニジュウサン");
}

// ─── 複数候補の同位置出力 ────────────────────────────────────────────────

#[test]
fn date_md_and_counter_both_emitted_at_pos_0() {
    // 「1月1日」 の pos 0 で 「1月1日」 (date MD) と 「1月」 (counter) が並列に出る
    // (DP が edge_count で longer match を選ぶ責務)
    let p = provider();
    let cands = p.candidates_vec(&ctx("1月1日"), 0);
    assert!(find(&cands, "1月1日").is_some(), "date candidate");
    assert!(find(&cands, "1月").is_some(), "counter candidate");
}

#[test]
fn si_and_scale_dont_collide_for_pure_number() {
    // 「100」 単独 (unit / scale なし) は digit のみ
    let p = provider();
    let cands = p.candidates_vec(&ctx("100"), 0);
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
    let cands = p.candidates_vec(&ctx(input), pos);
    let c = find(&cands, "3本").expect("3本 candidate at offset 3");
    // "3本" = "3" (1 byte) + "本" (3 bytes UTF-8) = 4 bytes
    assert_eq!(c.range, 3..7);
}

// ─── debug: empty rules でも static regex の DIGIT は走る ───────────────

#[test]
fn digit_regex_is_static_and_works_with_empty_rules() {
    let p = NumberCandidateProvider::new(&RulesData::default());
    let cands = p.candidates_vec(&ctx("42x"), 0);
    let c = find(&cands, "42").expect("bare digit candidate even with empty rules");
    assert_eq!(c.reading, "ヨンジュウニ");
}

// ─── 単位 lookup miss / 日 specials (★2026-09-11) ───────────────────────────

#[test]
fn si_unit_lookup_miss_emits_no_candidate() {
    // fixture の 「g」 は ci = false なので 「4G」 は lookup miss。
    // 以前は空文字を連結して 「ヨン」 だけ返し、 **単位文字を黙って落として**
    // いた。 候補を出さず alphabet passthrough に委ねるのが正しい。
    let p = provider();
    let cands = p.candidates_vec(&ctx("4G"), 0);
    assert!(
        find(&cands, "4G").is_none(),
        "解決できない単位 span は候補化しない: {cands:?}"
    );
}

#[test]
fn si_unit_lowercase_still_reads() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("4g"), 0);
    let c = find(&cands, "4g").expect("小文字 g は読める");
    assert_eq!(c.reading, "ヨングラム");
}

#[test]
fn bare_day_counter_honors_specials() {
    // 単独 「2 日」 は暦の日付ではないので days.toml を使わないが、
    // counter 側 specials は尊重する (期間表現でも フツカ)。
    let p = provider();
    let cands = p.candidates_vec(&ctx("2日"), 0);
    let c = find(&cands, "2日").expect("2日 候補");
    assert_eq!(c.reading, "フツカ");
}

#[test]
fn bare_day_counter_without_specials_falls_back_to_default() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("5日"), 0);
    let c = find(&cands, "5日").expect("5日 候補");
    assert_eq!(c.reading, "ゴニチ");
}

#[test]
fn hyphen_after_latin_before_digit_is_silent() {
    // 「RX-78-2」 「ak-74」 のような型番。 前が英字なら符号ではない。
    let p = provider();
    let input = "RX-78";
    let pos = "RX".len();
    let cands = p.candidates_vec(&ctx(input), pos);
    let c = find(&cands, "-").expect("hyphen candidate after latin");
    assert_eq!(c.reading, "", "英字直後の - は読み上げない");
}

#[test]
fn minus_after_kanji_is_still_sign() {
    // 「弾道-3」 のような能力値表記は負数なので マイナス のまま。
    let p = provider();
    let input = "弾道-3";
    let pos = "弾道".len();
    let cands = p.candidates_vec(&ctx(input), pos);
    let c = find(&cands, "-").expect("minus candidate after kanji");
    assert_eq!(c.reading, "マイナス");
}

// ─── not_before (助数詞の字が動詞の語幹を兼ねる、 ★2026-09-24) ─────────────

#[test]
fn not_before_blocks_counter_when_verb_okurigana_follows() {
    // 「4000行って」 は 行く の活用。 行 の not_before (っ) に当たるので counter 候補を出さない
    let p = provider();
    let cands = p.candidates_vec(&ctx("4000行って"), 0);
    assert!(find(&cands, "4000行").is_none(), "{cands:?}");
    assert!(find(&cands, "4000").is_some());
}

#[test]
fn not_before_keeps_counter_for_other_following_chars() {
    // 「3行で」 「2行目」 は助数詞のまま
    let p = provider();
    let cands = p.candidates_vec(&ctx("3行で"), 0);
    assert_eq!(
        find(&cands, "3行").expect("3行 candidate").reading,
        "サンギョウ"
    );
    let cands = p.candidates_vec(&ctx("2行目"), 0);
    assert!(find(&cands, "2行目").is_some(), "{cands:?}");
}

#[test]
fn not_before_at_end_of_input_keeps_counter() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("4行"), 0);
    assert!(find(&cands, "4行").is_some());
}

#[test]
fn not_before_on_scale_trailing_counter_drops_only_the_counter() {
    // 「1000万行くな」 は 万 までを数として読み、 行 は外す
    let p = provider();
    let cands = p.candidates_vec(&ctx("1000万行くな"), 0);
    assert!(find(&cands, "1000万行").is_none(), "{cands:?}");
    assert_eq!(
        find(&cands, "1000万").expect("1000万 candidate").reading,
        "センマン"
    );
}

#[test]
fn scale_trailing_false_keeps_counter_off_scale() {
    // scale_trailing = false の助数詞は 「100万段」 の末尾に付かない。 数字だけの 「3段」 は助数詞のまま
    let p = provider();
    let cands = p.candidates_vec(&ctx("100万段"), 0);
    assert!(find(&cands, "100万段").is_none(), "{cands:?}");
    assert!(find(&cands, "100万").is_some());
    let cands = p.candidates_vec(&ctx("3段"), 0);
    assert_eq!(
        find(&cands, "3段").expect("3段 candidate").reading,
        "サンダン"
    );
}

#[test]
fn counter_rule_default_keeps_scale_trailing_on() {
    assert!(crate::rules::CounterRule::default().scale_trailing);
    assert!(crate::rules::CounterRule::default().not_before.is_empty());
}

#[test]
fn repeated_plus_covers_the_run() {
    // Lindera が 「++」 を 1 token にまとめても素通しにならないよう、 列全体の候補を出す (C++ / g++)
    let p = provider();
    let cands = p.candidates_vec(&ctx("C++と"), 1);
    let c = find(&cands, "++").expect("run candidate");
    assert_eq!(c.reading, "プラスプラス");
}

#[test]
fn repeated_slash_is_not_merged() {
    // 「///」 (照れ表現) / 「//」 (コメント記号) はまとめない
    let p = provider();
    let cands = p.candidates_vec(&ctx("いいね///"), "いいね".len());
    assert!(find(&cands, "///").is_none(), "{cands:?}");
}

// ─── 位取りの漢数字 (〇〜九 だけの 3 桁以上) は opt-in でない助数詞でも counter (★2026-09-24) ───

#[test]
fn positional_kanji_numeral_counts_even_without_opt_in() {
    // fixture の 年 は kanji_numeral 無し。 一九九九年 は位取りなので数として読む
    let p = provider();
    let cands = p.candidates_vec(&ctx("一九九九年"), 0);
    let c = find(&cands, "一九九九年").expect("一九九九年 candidate");
    assert_eq!(c.reading, "センキュウヒャクキュウジュウキュウネン");
}

#[test]
fn short_or_unit_kanji_numeral_still_needs_opt_in() {
    // 2 桁 (一九) や 単位入り (十年) は従来どおり opt-in が要る
    let p = provider();
    assert!(find(&p.candidates_vec(&ctx("二三年"), 0), "二三年").is_none());
    assert!(find(&p.candidates_vec(&ctx("二十年"), 0), "二十年").is_none());
}

// ─── 読点区切りの漢数字 + 助数詞 (法令・判例の金額表記、 ★2026-09-24) ───

#[test]
fn grouped_kanji_numeral_reads_as_one_number() {
    let p = provider();
    let c =
        find(&p.candidates_vec(&ctx("三、五〇〇円"), 0), "三、五〇〇円").map(|c| c.reading.clone());
    // fixture に 円 が無ければ 年 で確認する
    let c = c.or_else(|| {
        find(&p.candidates_vec(&ctx("一、九九九年"), 0), "一、九九九年").map(|c| c.reading.clone())
    });
    let r = c.expect("grouped candidate");
    assert!(
        r == "サンゼンゴヒャクエン" || r == "センキュウヒャクキュウジュウキュウネン",
        "{r}"
    );
}

#[test]
fn grouped_kanji_numeral_needs_exactly_three_digits_after_comma() {
    // 第一、二条 / 一、二、三 のような列挙は数にしない
    let p = provider();
    assert!(p
        .candidates_vec(&ctx("一、二年"), 0)
        .iter()
        .all(|c| c.surface != "一、二年"));
}

// ─── 位取りの漢数字の小数 (★2026-09-27) ───

#[test]
fn kanji_decimal_reads_as_decimal() {
    let p = provider();
    let c = p.candidates_vec(&ctx("一・〇〇七を"), 0);
    assert_eq!(
        find(&c, "一・〇〇七").expect("decimal").reading,
        "イチテンゼロゼロナナ"
    );
}

#[test]
fn kanji_enumeration_is_not_decimal() {
    // 小数部 1 桁 (第一・二条 / 一・二・三) は列挙として扱い、 小数にしない
    let p = provider();
    assert!(find(&p.candidates_vec(&ctx("一・二・三"), 0), "一・二").is_none());
    // 漢数字の列の途中からは出さない
    let input = "二一・五〇";
    assert!(find(&p.candidates_vec(&ctx(input), "二".len()), "一・五〇").is_none());
}

#[test]
fn si_unit_allows_space_before_symbol_unit() {
    // 「50 °C」 (数字と記号始まりの単位の間の空白 1 つ) も単位として読む。 英字の単位は空白越しに取らない (Top 5 in)
    let mut r = rules();
    for (k, v) in [("°C", "ド"), ("in", "インチ")] {
        r.units.entries.insert(
            k.to_string(),
            crate::rules::UnitEntry {
                kana: v.to_string(),
                ci: true,
            },
        );
    }
    let p = NumberCandidateProvider::new(&r);
    let c = p.candidates_vec(&ctx("50 °Cを"), 0);
    assert_eq!(
        find(&c, "50 °C").expect("spaced unit").reading,
        "ゴジュウド"
    );
    assert!(find(&p.candidates_vec(&ctx("5 in"), 0), "5 in").is_none());
}

// ─── 助数詞の付かない漢数字の列 (★2026-09-27) ───

#[test]
fn bare_kanji_number_reads_as_number() {
    let p = provider();
    let c = p.candidates_vec(&ctx("七百七十三です"), 0);
    assert_eq!(
        find(&c, "七百七十三").expect("bare number").reading,
        "ナナヒャクナナジュウサン"
    );
    let c = p.candidates_vec(&ctx("二〇二五に"), 0);
    assert_eq!(
        find(&c, "二〇二五").expect("positional").reading,
        "ニセンニジュウゴ"
    );
}

#[test]
fn bare_kanji_number_skips_counter_approx_and_words() {
    let p = provider();
    // 直後が漢字 (助数詞) は既存の読みに任せる
    assert!(find(&p.candidates_vec(&ctx("十一項"), 0), "十一").is_none());
    // 概数 (三十七八) / 位の字も 〇 も無い 2 桁 (八九) / 万 始まり (万一) は出さない
    assert!(find(&p.candidates_vec(&ctx("三十七八、"), 0), "三十七八").is_none());
    assert!(find(&p.candidates_vec(&ctx("八九と"), 0), "八九").is_none());
    assert!(find(&p.candidates_vec(&ctx("万一の"), 0), "万一").is_none());
}

#[test]
fn counter_kanji_does_not_start_with_large_unit() {
    // 数万件 / 何万件 の 万件 を 「一万件」 と読まない (万 / 億 / 兆 始まりは前の語の一部)
    let p = provider();
    assert!(find(&p.candidates_vec(&ctx("万件"), 0), "万件").is_none());
}

#[test]
fn grouped_kanji_numeral_allows_zero_in_head_and_thousand_unit() {
    // 先頭の 2 桁目以降の 〇 (二九〇、〇〇〇年) と、 千単位 (一〇、〇〇〇千年 = 10,000 千年) を読む
    let p = provider();
    let c = p.candidates_vec(&ctx("二九〇、〇〇〇年"), 0);
    assert_eq!(
        find(&c, "二九〇、〇〇〇年")
            .expect("head with zero")
            .reading,
        "ニジュウキュウマンネン"
    );
    let c = p.candidates_vec(&ctx("一〇、〇〇〇千年"), 0);
    let r = &find(&c, "一〇、〇〇〇千年").expect("thousand unit").reading;
    assert!(r.starts_with("イチマンセン"), "{r}");
}

#[test]
fn counter_kanji_starts_at_nan_with_rendaku() {
    // 何百回目 は 何 の位置から 1 候補 (三 と同じ連濁・促音で ナンビャッカイメ)。
    // 百 の位置からは出さない (出すと 何 が単独で なに になる)
    let p = provider();
    let input = "何百回目";
    let c = p.candidates_vec(&ctx(input), 0);
    assert_eq!(
        find(&c, "何百回目").expect("nan prefix").reading,
        "ナンビャッカイメ"
    );
    let c = p.candidates_vec(&ctx(input), '何'.len_utf8());
    assert!(find(&c, "百回目").is_none());
    // 何 の後が 百 / 千 でなければ 何 の位置では出さない (何回目 は別経路)
    assert!(p.candidates_vec(&ctx("何回目"), 0).is_empty());
}

#[test]
fn kanji_time_with_minutes() {
    // 議事録の 午後零時三分散会 / 午前十時一分開議。 分 必須、 時 24 / 分 59 まで
    let p = provider();
    let c = p.candidates_vec(&ctx("零時三分散会"), 0);
    assert_eq!(
        find(&c, "零時三分").expect("kanji time").reading,
        "レイジサンプン"
    );
    let c = p.candidates_vec(&ctx("十時一分"), 0);
    assert_eq!(
        find(&c, "十時一分").expect("kanji time").reading,
        "ジュウジイップン"
    );
    // 分 が無い / 範囲外は出さない
    assert!(find(&p.candidates_vec(&ctx("三時"), 0), "三時")
        .is_none_or(|c| !c.reading.contains("フン")));
    assert!(find(&p.candidates_vec(&ctx("三時七十分"), 0), "三時七十分").is_none());
}

#[test]
fn two_digit_positional_kansuji() {
    // 位取りで書いた 2 桁 (五三人 / 三〇キロ / 三八・四) は数として読む
    let p = provider();
    let c = p.candidates_vec(&ctx("五三人"), 0);
    assert_eq!(
        find(&c, "五三人").expect("counter").reading,
        "ゴジュウサンニン"
    );
    let c = p.candidates_vec(&ctx("三〇キロ"), 0);
    assert_eq!(find(&c, "三〇").expect("bare").reading, "サンジュウ");
    let c = p.candidates_vec(&ctx("三八・四％"), 0);
    assert_eq!(
        find(&c, "三八・四").expect("decimal").reading,
        "サンジュウハチテンヨン"
    );
    // 1 つ違いで上がる並び (二三 / 七八 = 概数) と 〇 始まり (〇〇) は数にしない
    assert!(find(&p.candidates_vec(&ctx("七八人"), 0), "七八人").is_none());
    assert!(find(&p.candidates_vec(&ctx("〇〇の"), 0), "〇〇").is_none());
    assert!(find(&p.candidates_vec(&ctx("三・四％"), 0), "三・四").is_none());
    // 第 の後ろは概数にならないので 1 つ違いの並びも桁で読む (第一二回 = 12 回)
    let c = p.candidates_vec(&ctx("第一二回"), "第".len());
    assert_eq!(find(&c, "一二回").expect("ordinal").reading, "ジュウニカイ");
    // 同じ字の繰り返しは 顔文字・効果線 (⊂二二) では数にせず、 第 の後なら数
    assert!(find(&p.candidates_vec(&ctx("⊂二二("), "⊂".len()), "二二").is_none());
    let c = p.candidates_vec(&ctx("第一一　"), "第".len());
    assert_eq!(
        find(&c, "一一").expect("ordinal repeat").reading,
        "ジュウイチ"
    );
}

#[test]
fn positional_in_numeric_context() {
    // ％ の前は 1 つ違いでも数 (四五％ = 45%)、 〇 を含む数は後ろが漢字でも数 (一〇以上)
    let p = provider();
    let c = p.candidates_vec(&ctx("四五％"), 0);
    assert_eq!(
        find(&c, "四五％").expect("percent").reading,
        "ヨンジュウゴパーセント"
    );
    let c = p.candidates_vec(&ctx("一〇以上"), 0);
    assert_eq!(find(&c, "一〇").expect("zero").reading, "ジュウ");
    // 3 桁で英字の単位が続く (一四二MHz)
    let c = p.candidates_vec(&ctx("一四二MHz"), 0);
    assert_eq!(
        find(&c, "一四二").expect("latin unit").reading,
        "ヒャクヨンジュウニ"
    );
    // 〇 の無い 2 桁の後ろが漢字 (八五郎) は出さない
    assert!(find(&p.candidates_vec(&ctx("八五郎"), 0), "八五").is_none());
}

#[test]
fn approx_pair_counter_not_read_as_last_digit() {
    // 概数 (二三件 / 四五回) を 1 つの数として助数詞に渡すと後ろの桁だけ残る (さんけん / ごかい) ので出さない
    let p = provider();
    assert!(find(&p.candidates_vec(&ctx("二三件"), 0), "二三件").is_none());
    assert!(find(&p.candidates_vec(&ctx("四五回"), 0), "四五回").is_none());
}

// ─── バージョン表記 / 通貨記号の前置 ─────────────────────────────────────

#[test]
fn version_dotted_three_parts_reads_each_part_joined_by_ten() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("1.2.3"), 0);
    let v = find(&cands, "1.2.3").expect("1.2.3 の候補");
    assert_eq!(v.reading, "イチテンニテンサン");
    assert_eq!(v.score.band, BAND_SPECIAL);
}

#[test]
fn version_two_parts_is_a_decimal_not_a_version() {
    let p = provider();
    let cands = p.candidates_vec(&ctx("3.14"), 0);
    assert!(cands
        .iter()
        .all(|c| c.reading != "サンテンイチヨン" || c.surface == "3.14"));
    assert_eq!(find(&cands, "3.14").unwrap().reading, "サンテンイチヨン");
}

#[test]
fn currency_prefix_reads_number_then_currency_name() {
    let p = provider();
    for (input, surface, reading) in [
        ("¥1000", "¥1000", "センエン"),
        ("￥1,500", "￥1,500", "センゴヒャクエン"),
        ("$100", "$100", "ヒャクドル"),
        ("€20", "€20", "ニジュウユーロ"),
        ("£3", "£3", "サンポンド"),
    ] {
        let cands = p.candidates_vec(&ctx(input), 0);
        let c = find(&cands, surface).unwrap_or_else(|| panic!("{input} の候補"));
        assert_eq!(c.reading, reading, "{input}");
    }
}

#[test]
fn currency_prefix_needs_a_digit_right_after_the_symbol() {
    let p = provider();
    assert!(find(&p.candidates_vec(&ctx("$abc"), 0), "$abc").is_none());
    assert!(p
        .candidates_vec(&ctx("$abc"), 0)
        .iter()
        .all(|c| c.surface == "$"));
    // 符号付きは通貨として読まない
    assert!(p
        .candidates_vec(&ctx("¥-5"), 0)
        .iter()
        .all(|c| c.surface == "¥"));
}

#[test]
fn version_rule_skips_dotted_dates() {
    let p = provider();
    // 西暦 + 月 + 日の形 (2026.1.16) はバージョンではなく日付として読む
    assert_eq!(
        find(&p.candidates_vec(&ctx("2026.1.16"), 0), "2026.1.16")
            .map(|c| c.reading.to_string())
            .as_deref(),
        Some("ニセンニジュウロクネンイチガツジュウロクニチ")
    );
    // 月が 13 以上 / 西暦が 4 桁でなければバージョン
    assert!(find(&p.candidates_vec(&ctx("2026.13.16"), 0), "2026.13.16").is_some());
    assert!(find(&p.candidates_vec(&ctx("12.1.16"), 0), "12.1.16").is_some());
    // 日が 32 以上はバージョン
    assert!(find(&p.candidates_vec(&ctx("2026.1.32"), 0), "2026.1.32").is_some());
}

fn slash_provider() -> NumberCandidateProvider {
    let mut r = rules();
    let units: UnitsData = toml::from_str(
        r#"
        [entries]
        "km" = { kana = "キロメートル" }
        "km/h" = { kana = "キロメートルマイジ" }
        "m/s" = { kana = "メートルマイビョウ", ci = false }
        "#,
    )
    .unwrap();
    r.units = units;
    NumberCandidateProvider::new(&r)
}

#[test]
fn slash_unit_alone_is_read_at_a_word_start() {
    let p = slash_provider();
    for (input, surface, reading) in [
        ("km/h", "km/h", "キロメートルマイジ"),
        ("KM/H", "KM/H", "キロメートルマイジ"),
        ("時速km/hです", "km/h", "キロメートルマイジ"),
        ("m/s", "m/s", "メートルマイビョウ"),
    ] {
        let pos = input.find(surface).unwrap();
        let cands = p.candidates_vec(&ctx(input), pos);
        let c = find(&cands, surface).unwrap_or_else(|| panic!("{input} の候補"));
        assert_eq!(c.reading, reading, "{input}");
    }
    // ci = false の m/s は大文字では読まない
    assert!(find(&p.candidates_vec(&ctx("M/S"), 0), "M/S").is_none());
}

#[test]
fn slash_unit_is_not_read_inside_urls_paths_or_words() {
    let p = slash_provider();
    for (input, pos) in [
        ("a.com/km/h", 6),
        ("/km/h", 1),
        ("x?v=km/h", 4),
        ("foo@km/h", 4),
        ("a_km/h", 2),
        ("1km/h", 1),
        ("km/h/x", 0),
        ("km/h_x", 0),
        ("km/h-x", 0),
        ("km/hx", 0),
        ("km/h2", 0),
    ] {
        let cands = p.candidates_vec(&ctx(input), pos);
        assert!(
            cands
                .iter()
                .all(|c| c.surface != "km/h" && c.surface != "m/s"),
            "{input} は読まない"
        );
    }
}

#[test]
fn slash_unit_with_number_is_read_by_the_si_unit_rule() {
    let p = slash_provider();
    let cands = p.candidates_vec(&ctx("60km/h"), 0);
    let c = find(&cands, "60km/h").expect("数字つき");
    assert_eq!(c.reading, "ロクジュッキロメートルマイジ");
}

fn reading_at(input: &str, pos: usize, surface: &str) -> Option<String> {
    let p = provider();
    let cands = p.candidates_vec(&ctx(input), pos);
    find(&cands, surface).map(|c| c.reading.to_string())
}

#[test]
fn time_with_zero_minutes_reads_only_the_hour() {
    assert_eq!(reading_at("10:00", 0, "10:00").as_deref(), Some("ジュウジ"));
    assert_eq!(
        reading_at("１０：００", 0, "１０：００").as_deref(),
        Some("ジュウジ")
    );
    // 分があれば従来どおり。 秒つきは 00 分でも読む
    assert!(reading_at("10:30", 0, "10:30").unwrap().ends_with("プン"));
    assert!(reading_at("10:00:15", 0, "10:00:15")
        .unwrap()
        .contains("ビョウ"));
    assert!(
        reading_at("10:00:15", 0, "10:00:15")
            .unwrap()
            .contains("プン")
            || reading_at("10:00:15", 0, "10:00:15")
                .unwrap()
                .contains("フン")
    );
}

#[test]
fn time_is_only_valid_hour_minute_second() {
    // 24 時 59 分 59 秒まで時刻
    assert!(!reading_at("24:59", 0, "24:59").unwrap().contains("タイ"));
    assert!(reading_at("23:59:59", 0, "23:59:59")
        .unwrap()
        .contains("ビョウ"));
    // 時 30 以上 / 分 60 以上 / 秒 60 以上は時刻ではなく比 (25〜29 時は深夜の時刻表記)
    assert_eq!(
        reading_at("30:00", 0, "30:00").as_deref(),
        Some("サンジュウタイゼロ")
    );
    assert!(reading_at("12:60", 0, "12:60").unwrap().contains("タイ"));
    // 12:30:60 は時刻でも比でもない (途中の 12:30 も出さない)
    assert!(reading_at("12:30:60", 0, "12:30").is_none());
    assert!(reading_at("12:30:60", 0, "12:30:60").is_none());
}

#[test]
fn ratio_reads_tai_and_ignores_longer_chains() {
    assert_eq!(reading_at("1:1", 0, "1:1").as_deref(), Some("イチタイイチ"));
    assert_eq!(reading_at("3:7", 0, "3:7").as_deref(), Some("サンタイナナ"));
    assert_eq!(
        reading_at("50:50", 0, "50:50").as_deref(),
        Some("ゴジュウタイゴジュウ")
    );
    // 1:2:3 は比にしない
    assert!(reading_at("1:2:3", 0, "1:2").is_none());
    // 途中から (前が数字 / コロン) は出さない
    assert!(reading_at("1:1:1", 2, "1:1").is_none());
    assert!(reading_at("11:1", 1, "1:1").is_none());
}

#[test]
fn hyphen_after_time_unit_is_kara() {
    let p = provider();
    let input = "9時-17時";
    let cands = p.candidates_vec(&ctx(input), "9時".len());
    let c = find(&cands, "-").expect("ハイフン");
    assert_eq!(c.reading, "から");
    // 時分秒以外の直後は従来どおり読まない / 符号
    let cands = p.candidates_vec(&ctx("9日-17"), "9日".len());
    assert!(cands
        .iter()
        .all(|c| c.surface != "-" || c.reading != "から"));
    // 数字が続かなければ範囲ではない
    let cands = p.candidates_vec(&ctx("9時-あ"), "9時".len());
    assert!(cands.iter().all(|c| c.reading != "から"));
}

#[test]
fn tilde_between_single_capitals_is_kara() {
    let p = provider();
    for (input, pos) in [("A〜Z", 1), ("A~C", 1)] {
        let cands = p.candidates_vec(&ctx(input), pos);
        assert!(cands.iter().any(|c| c.reading == "から"), "{input}");
    }
    // 単語どうし / 小文字 / 片側が複数文字 は長音・強調なので読まない
    for (input, pos) in [("Hello〜World", 5), ("a〜z", 1), ("AB〜C", 2), ("A〜BC", 1)] {
        let cands = p.candidates_vec(&ctx(input), pos);
        assert!(cands.iter().all(|c| c.reading != "から"), "{input}");
    }
}

#[test]
fn normalize_digits_maps_fullwidth_only() {
    assert_eq!(normalize_digits("０１２３４５６７８９"), "0123456789");
    assert_eq!(normalize_digits("a1９"), "a19");
}

#[test]
fn slash_unit_key_starting_with_a_symbol_is_not_read_bare() {
    let mut r = rules();
    r.units = toml::from_str(
        r#"
        [entries]
        "°/s" = { kana = "ドマイビョウ" }
        "#,
    )
    .unwrap();
    let p = NumberCandidateProvider::new(&r);
    assert!(find(&p.candidates_vec(&ctx("°/s"), 0), "°/s").is_none());
}

fn model_provider() -> NumberCandidateProvider {
    let mut r = rules();
    r.models = toml::from_str(
        r#"
        [parts]
        "Ti" = "ティーアイ"

        [[model]]
        pattern = 'RTX\s?([0-9]{4})(?:\s?(Ti))?'
        reading = "アールティーエックス{1:each}{2:part}"

        [[model]]
        pattern = 'i([3579])-?([0-9]{4,5})([A-Z]?)'
        reading = "アイ{1:english}{2:each}{3:part}"
        "#,
    )
    .unwrap();
    NumberCandidateProvider::new(&r)
}

#[test]
fn model_candidate_covers_the_whole_model_number() {
    let p = model_provider();
    let cands = p.candidates_vec(&ctx("RTX 4070 Ti"), 0);
    let c = find(&cands, "RTX 4070 Ti").expect("型番候補");
    assert_eq!(c.reading, "アールティーエックスヨンゼロナナゼロティーアイ");
    assert_eq!(c.score.band, BAND_SPECIAL);
}

#[test]
fn model_candidate_is_not_emitted_without_a_match() {
    let p = model_provider();
    // 語の途中 / 英字以外の位置 / パターン外
    assert!(find(&p.candidates_vec(&ctx("xRTX4090"), 1), "RTX4090").is_none());
    assert!(find(&p.candidates_vec(&ctx("4090"), 0), "4090").is_some()); // 素の数字は従来どおり
    assert!(p
        .candidates_vec(&ctx("GTX1660"), 0)
        .iter()
        .all(|c| c.surface != "GTX1660"));
    // 型番パターンが無い rules では出ない
    let plain = NumberCandidateProvider::new(&rules());
    assert!(plain
        .candidates_vec(&ctx("RTX4090"), 0)
        .iter()
        .all(|c| c.surface != "RTX4090"));
}

#[test]
fn model_starts_lists_every_word_start_that_matches() {
    let p = model_provider();
    let input = "Hello RTX 4090 とi9-13900K";
    let starts = p.model_starts(input);
    assert_eq!(starts, vec![6, input.find("i9").unwrap()]);
    assert!(p.model_starts("ただの日本語").is_empty());
    assert!(NumberCandidateProvider::new(&rules())
        .model_starts(input)
        .is_empty());
}

fn phone(input: &str) -> Option<String> {
    reading_at(input, 0, input)
}

#[test]
fn phone_numbers_are_read_digit_by_digit() {
    assert_eq!(
        phone("090-1234-5678").as_deref(),
        Some("ゼロキュウゼロ イチニサンヨン ゴロクナナハチ")
    );
    assert_eq!(
        phone("03-1234-5678").as_deref(),
        Some("ゼロサン イチニサンヨン ゴロクナナハチ")
    );
    assert_eq!(
        phone("0120-123-456").as_deref(),
        Some("ゼロイチニゼロ イチニサン ヨンゴロク")
    );
    // 空白区切り / ハイフンなしの携帯 (3-4-4 に区切る)
    assert_eq!(
        phone("090 1234 5678").as_deref(),
        Some("ゼロキュウゼロ イチニサンヨン ゴロクナナハチ")
    );
    assert_eq!(
        phone("09012345678").as_deref(),
        Some("ゼロキュウゼロ イチニサンヨン ゴロクナナハチ")
    );
}

#[test]
fn international_numbers_start_with_plus_and_allow_up_to_15_digits() {
    assert_eq!(
        phone("+81-90-1234-5678").as_deref(),
        Some("プラスハチイチ キュウゼロ イチニサンヨン ゴロクナナハチ")
    );
    assert_eq!(
        phone("+1 555-123-4567").as_deref(),
        Some("プラスイチ ゴゴゴ イチニサン ヨンゴロクナナ")
    );
    // 8 桁 (下限) は読む。 7 桁以下は電話番号ではない
    assert!(phone("+81 3 1234 5678").is_some());
    assert!(phone("+12 345 678").is_some());
    assert!(phone("+1 234 567").is_none());
    // 15 桁 (上限) は読み、 16 桁は読まない
    assert!(phone("+123 4567 8901 2345").is_some());
    assert!(phone("+123 4567 8901 23456").is_none());
}

#[test]
fn domestic_numbers_need_10_or_11_digits() {
    // 9 桁 / 12 桁は国内の電話番号ではない
    assert!(phone("03-123-4567").is_none());
    assert!(phone("0120-1234-56789").is_none());
    assert!(phone("090-12345-67890").is_none());
    // 先頭が 0 でなければ対象外 (日付など)
    assert!(phone("12-1234-5678").is_none());
    // 年付きの日付は電話番号ではない (日付として別に読む)
    assert_ne!(phone("2024-10-12").as_deref(), Some("ニセンニジュウヨン"));
}

#[test]
fn phone_is_not_matched_in_the_middle_of_a_longer_chain() {
    let p = provider();
    // 前が数字 / ハイフン、 後ろが数字 / ハイフンなら途中
    assert!(find(
        &p.candidates_vec(&ctx("1090-1234-5678"), 1),
        "090-1234-5678"
    )
    .is_none());
    assert!(find(&p.candidates_vec(&ctx("2024-03-1234"), 5), "03-1234").is_none());
    assert!(find(
        &p.candidates_vec(&ctx("090-1234-56789"), 0),
        "090-1234-5678"
    )
    .is_none());
    assert!(find(
        &p.candidates_vec(&ctx("090-1234-5678-9"), 0),
        "090-1234-5678"
    )
    .is_none());
    // 国際の + は英数字の直後 (A+1234 5678) なら足し算
    assert!(find(&p.candidates_vec(&ctx("A+1234 5678"), 1), "+1234 5678").is_none());
    // 前が日本語や空白なら電話番号
    assert!(find(
        &p.candidates_vec(&ctx("電話090-1234-5678"), "電話".len()),
        "090-1234-5678"
    )
    .is_some());
    assert!(find(
        &p.candidates_vec(&ctx("x +81-90-1234-5678"), 2),
        "+81-90-1234-5678"
    )
    .is_some());
}

#[test]
fn slash_pair_is_a_date_when_month_and_day_are_valid() {
    // 日 13 以上は確実に日付
    assert_eq!(
        reading_at("12/25", 0, "12/25").as_deref(),
        Some("ジュウニガツニジュウゴニチ")
    );
    assert!(reading_at("1/31", 0, "1/31").is_some());
    // 手がかりが無い 月 1〜12 / 日 1〜12 も日付に倒す
    assert_eq!(
        reading_at("10/12", 0, "10/12").as_deref(),
        Some("ジュウガツジュウニニチ")
    );
    assert_eq!(
        reading_at("2/3です", 0, "2/3").as_deref(),
        Some("ニガツミッカ")
    );
    // 同じ数どうしも日付 (3/3 = ひなまつり)
    assert_eq!(
        reading_at("3/3", 0, "3/3").as_deref(),
        Some("サンガツミッカ")
    );
}

#[test]
fn slash_pair_that_is_not_a_date_is_a_fraction() {
    // 月 13 以上 / 日 32 以上 は分数 (分母が先)
    assert_eq!(
        reading_at("13/5", 0, "13/5").as_deref(),
        Some("ゴブンノジュウサン")
    );
    assert_eq!(
        reading_at("3/40", 0, "3/40").as_deref(),
        Some("ヨンジュウブンノサン")
    );
    assert_eq!(
        reading_at("12/32", 0, "12/32").as_deref(),
        Some("サンジュウニブンノジュウニ")
    );
    // 0 / 同じ数どうし / 24/7 / 番号は読まない
    assert!(reading_at("0/25", 0, "0/25").is_none());
    assert!(reading_at("25/0", 0, "25/0").is_none());
    assert!(reading_at("50/50", 0, "50/50").is_none());
    assert!(reading_at("24/7", 0, "24/7").is_none());
    // 24/7 だけが特別 (24 か 7 の片方だけなら分数)
    assert_eq!(
        reading_at("24/9", 0, "24/9").as_deref(),
        Some("キュウブンノニジュウヨン")
    );
    assert_eq!(
        reading_at("13/7", 0, "13/7").as_deref(),
        Some("ナナブンノジュウサン")
    );
    for input in ["No.19/20", "vol.13/20", "#13/20", "第13/20"] {
        let start = input.find(|c: char| c.is_ascii_digit()).unwrap();
        assert!(
            reading_at(input, start, &input[start..]).is_none(),
            "{input}"
        );
    }
    // 番号でなければ読む (進捗・個数)
    assert_eq!(
        reading_at("13/54人", 0, "13/54").as_deref(),
        Some("ゴジュウヨンブンノジュウサン")
    );
}

#[test]
fn slash_pair_is_a_fraction_before_a_measure_word() {
    assert_eq!(
        reading_at("1/2サイズ", 0, "1/2").as_deref(),
        Some("ニブンノイチ")
    );
    assert_eq!(
        reading_at("3/4以上", 0, "3/4").as_deref(),
        Some("ヨンブンノサン")
    );
    for w in [
        "倍",
        "割",
        "以下",
        "程度",
        "ほど",
        "くらい",
        "ぐらい",
        "弱",
        "強",
        "ずつ",
    ] {
        let input = format!("1/3{w}");
        assert_eq!(
            reading_at(&input, 0, "1/3").as_deref(),
            Some("サンブンノイチ"),
            "{input}"
        );
        // 日 13 以上の日付よりも分数の手がかりが先
        let input = format!("12/25{w}");
        assert_eq!(
            reading_at(&input, 0, "12/25").as_deref(),
            Some("ニジュウゴブンノジュウニ"),
            "{input}"
        );
    }
    // 分母 0 は分数にしない
    assert!(reading_at("1/0倍", 0, "1/0").is_none());
}

#[test]
fn slash_pair_one_over_small_numbers_is_a_fraction_unless_there_is_a_date_cue() {
    // 手がかりが無い 1/2〜1/10 は分数
    assert_eq!(reading_at("1/2", 0, "1/2").as_deref(), Some("ニブンノイチ"));
    assert_eq!(
        reading_at("1/3", 0, "1/3").as_deref(),
        Some("サンブンノイチ")
    );
    assert_eq!(
        reading_at("1/10", 0, "1/10").as_deref(),
        Some("ジュウブンノイチ")
    );
    // 1/1 と 1/11 以上は日付
    assert_eq!(
        reading_at("1/1", 0, "1/1").as_deref(),
        Some("イチガツツイタチ")
    );
    assert_eq!(
        reading_at("1/11", 0, "1/11").as_deref(),
        Some("イチガツジュウイチニチ")
    );
    // 日付の手がかり: 曜日の括弧 / 時刻 / まで・から / 直前の言い方 / 範囲
    for (input, surface, pos, want) in [
        ("1/3(日)", "1/3", 0, "イチガツミッカ"),
        ("1/3（日）", "1/3", 0, "イチガツミッカ"),
        ("1/3 19:00", "1/3", 0, "イチガツミッカ"),
        ("1/3まで", "1/3", 0, "イチガツミッカ"),
        ("1/3から発売", "1/3", 0, "イチガツミッカ"),
        ("本日1/3は", "1/3", "本日".len(), "イチガツミッカ"),
        ("1/3〜1/5", "1/3", 0, "イチガツミッカ"),
        ("1/3〜1/5", "1/5", "1/3〜".len(), "イチガツイツカ"),
    ] {
        assert_eq!(
            reading_at(input, pos, surface).as_deref(),
            Some(want),
            "{input}"
        );
    }
    // 戻る組 (1/3から1/2) は分数どうし
    assert_eq!(
        reading_at("1/3から1/2やない", 0, "1/3").as_deref(),
        Some("サンブンノイチ")
    );
}

#[test]
fn slash_pair_range_checks_both_ends_are_valid_dates() {
    // 範囲の両端が日付として成り立つ時だけ手がかり (1/3 は分数に倒れる組で確かめる)
    assert_eq!(
        reading_at("1/3〜1/3は", 0, "1/3").as_deref(),
        Some("イチガツミッカ")
    );
    assert_eq!(
        reading_at("1/5〜1/3は", 0, "1/5").as_deref(),
        Some("ゴブンノイチ")
    );
    assert_eq!(
        reading_at("1/3〜13/20は", 0, "1/3").as_deref(),
        Some("サンブンノイチ")
    );
    assert_eq!(
        reading_at("1/3〜2/40は", 0, "1/3").as_deref(),
        Some("サンブンノイチ")
    );
    // 前側の端が日付として成り立たない (月 0 / 日 40) なら、 後ろ側の 1/3 は手がかり無しで分数
    assert_eq!(
        reading_at("0/5〜1/3は", "0/5〜".len(), "1/3").as_deref(),
        Some("サンブンノイチ")
    );
    assert_eq!(
        reading_at("2/40〜1/3は", "2/40〜".len(), "1/3").as_deref(),
        Some("サンブンノイチ")
    );
    assert_eq!(
        reading_at("1/2〜1/3は", "1/2〜".len(), "1/3").as_deref(),
        Some("イチガツミッカ")
    );
    // 戻る範囲は日付にしない
    assert_eq!(
        reading_at("1/3〜1/2は", "1/3〜".len(), "1/2").as_deref(),
        Some("ニブンノイチ")
    );
}

#[test]
fn slash_pair_fraction_cues_before_the_number() {
    // 直前の HP / 確率 は分数 (12/25 でも日付にしない)
    assert_eq!(
        reading_at("HP 12/25", "HP ".len(), "12/25").as_deref(),
        Some("ニジュウゴブンノジュウニ")
    );
    assert_eq!(
        reading_at("確率1/3", "確率".len(), "1/3").as_deref(),
        Some("サンブンノイチ")
    );
    assert_eq!(
        reading_at("9/16くらい", 0, "9/16").as_deref(),
        Some("ジュウロクブンノキュウ")
    );
}

#[test]
fn slash_pair_topic_of_the_sentence_only_matters_for_leaning_pairs() {
    // 分数の話題 (スコア) だけの文は、 手がかりが無い月日どうしを分数に倒す
    assert_eq!(
        reading_at("スコア 10/4", "スコア ".len(), "10/4").as_deref(),
        Some("ヨンブンノジュウ")
    );
    // 日付の話題 (配信) もあれば日付
    assert_eq!(
        reading_at("スコア 10/4の配信", "スコア ".len(), "10/4").as_deref(),
        Some("ジュウガツヨッカ")
    );
    // 日付の手がかりが付けば日付
    assert_eq!(
        reading_at("スコア 10/4(日)", "スコア ".len(), "10/4").as_deref(),
        Some("ジュウガツヨッカ")
    );
    // 話題の語が無ければ日付
    assert_eq!(
        reading_at("10/4に勝った", 0, "10/4").as_deref(),
        Some("ジュウガツヨッカ")
    );
}

#[test]
fn slash_pair_cues_from_the_dictionary_are_added_to_the_builtin_ones() {
    let mut r = rules();
    r.context_cues.slash_pair.date_topic.push("ほげ会".into());
    r.context_cues.slash_pair.date_after.push("やるぞ".into());
    r.context_cues.slash_pair.date_before.push("ふが日".into());
    r.context_cues.slash_pair.fraction_after.push("もの".into());
    r.context_cues
        .slash_pair
        .fraction_before
        .push("ぴよ値".into());
    r.context_cues
        .slash_pair
        .fraction_topic
        .push("ほげ率".into());
    let p = NumberCandidateProvider::new(&r);
    let at = |input: &str, pos: usize, surface: &str| {
        find(&p.candidates_vec(&ctx(input), pos), surface).map(|c| c.reading.to_string())
    };
    // 分数の話題 (辞書) だけの文は分数、 日付の話題 (辞書) もあれば日付
    assert_eq!(
        at("ほげ率 10/4", "ほげ率 ".len(), "10/4").as_deref(),
        Some("ヨンブンノジュウ")
    );
    assert_eq!(
        at("ほげ率 10/4のほげ会", "ほげ率 ".len(), "10/4").as_deref(),
        Some("ジュウガツヨッカ")
    );
    // 日付の直後 / 直前の語 (辞書): 分数に倒れる 1/3 が日付になる
    assert_eq!(at("1/3やるぞ", 0, "1/3").as_deref(), Some("イチガツミッカ"));
    assert_eq!(
        at("ふが日1/3", "ふが日".len(), "1/3").as_deref(),
        Some("イチガツミッカ")
    );
    // 分数の直後 / 直前の語 (辞書): 日付になる組が分数になる
    assert_eq!(at("3/4もの", 0, "3/4").as_deref(), Some("ヨンブンノサン"));
    assert_eq!(
        at("ぴよ値12/25", "ぴよ値".len(), "12/25").as_deref(),
        Some("ニジュウゴブンノジュウニ")
    );
    // 辞書に何も無ければ話題の語は効かない
    assert_eq!(
        reading_at("ほげ率 10/4", "ほげ率 ".len(), "10/4").as_deref(),
        Some("ジュウガツヨッカ")
    );
}

#[test]
fn year_month_day_is_read_as_a_date() {
    for (input, want) in [
        ("2000-10-12", "ニセンネンジュウガツジュウニニチ"),
        (
            "1999/10/12",
            "センキュウヒャクキュウジュウキュウネンジュウガツジュウニニチ",
        ),
        ("2025/10/13", "ニセンニジュウゴネンジュウガツジュウサンニチ"),
        ("2025/1/3", "ニセンニジュウゴネンイチガツミッカ"),
    ] {
        assert_eq!(
            reading_at(input, 0, input).as_deref(),
            Some(want),
            "{input}"
        );
    }
    // 区切りが違う / 月日が範囲外 / 前後が数字・区切りの途中 は読まない
    for input in [
        "2025/10-13",
        "2025/13/1",
        "2025/10/32",
        "12025/10/13",
        "2025/10/133",
        "1/2025/10/13",
    ] {
        let start = input.find("2025").unwrap();
        let surface = &input[start..];
        assert!(reading_at(input, start, surface).is_none(), "{input}");
    }
    assert!(reading_at("2025-10-13-1", 0, "2025-10-13").is_none());
    assert_eq!(
        reading_at("2025-10-13T10:00", 0, "2025-10-13").as_deref(),
        Some("ニセンニジュウゴネンジュウガツジュウサンニチ")
    );
}

#[test]
fn time_after_midnight_notation_reads_hours_up_to_29() {
    assert_eq!(
        reading_at("25:00", 0, "25:00").as_deref(),
        Some("ニジュウゴジ")
    );
    assert_eq!(
        reading_at("29:59", 0, "29:59").as_deref(),
        Some("ニジュウクジゴジュウキュウフン")
    );
    // 30 時以上 / 分が 60 以上は時刻ではなく比
    assert_eq!(
        reading_at("30:00", 0, "30:00").as_deref(),
        Some("サンジュウタイゼロ")
    );
    assert_eq!(
        reading_at("25:60", 0, "25:60").as_deref(),
        Some("ニジュウゴタイロクジュウ")
    );
}

#[test]
fn dotted_year_month_day_is_a_date_only_for_19xx_and_20xx() {
    assert_eq!(
        reading_at("2026.10.12", 0, "2026.10.12").as_deref(),
        Some("ニセンニジュウロクネンジュウガツジュウニニチ")
    );
    assert_eq!(
        reading_at("1999.1.2", 0, "1999.1.2").as_deref(),
        Some("センキュウヒャクキュウジュウキュウネンイチガツフツカ")
    );
    // 19xx / 20xx 以外の年はバージョンかもしれないので日付にしない
    assert!(reading_at("1234.5.6", 0, "1234.5.6").is_none());
    // 4 要素 (2026.10.12.3) / 英字の直後 (v2026.10.12) は版
    assert!(reading_at("2026.10.12.3", 0, "2026.10.12").is_none());
    assert!(reading_at("v2026.10.12", 1, "2026.10.12").is_none());
    assert!(reading_at("ver2026.10.12", 3, "2026.10.12").is_none());
    // 区切りが混ざる形は読まない
    assert!(reading_at("2026.10/12", 0, "2026.10/12").is_none());
}

fn hyphen_reading(input: &str, pos: usize) -> Option<String> {
    let p = provider();
    p.candidates_vec(&ctx(input), pos)
        .iter()
        .find(|c| c.surface == "-")
        .map(|c| c.reading.to_string())
}

#[test]
fn hyphen_between_numbers_before_a_counter_is_a_range() {
    // 10-20人 / 1-2日 は範囲 (から)
    assert_eq!(hyphen_reading("10-20人", 2).as_deref(), Some("から"));
    assert_eq!(hyphen_reading("1-2日", 1).as_deref(), Some("から"));
    assert_eq!(hyphen_reading("2024-2025年", 4).as_deref(), Some("から"));
    assert_eq!(hyphen_reading("１０-２０人", 6).as_deref(), Some("から"));
    // 同じ数 / 戻る数 / 助数詞が無い (スコア) は範囲にしない
    assert_eq!(hyphen_reading("10-10人", 2).as_deref(), Some(""));
    assert_eq!(hyphen_reading("20-10人", 2).as_deref(), Some(""));
    assert_eq!(hyphen_reading("3-20で勝利", 1).as_deref(), Some(""));
    // 助数詞でない漢字は範囲にしない
    assert_eq!(hyphen_reading("3-20手", 1).as_deref(), Some(""));
}

#[test]
fn vulgar_fraction_characters_are_always_fractions() {
    for (input, want) in [
        ("1\u{2044}2", "ニブンノイチ"),
        ("3\u{2044}4", "ヨンブンノサン"),
        ("1\u{2044}3", "サンブンノイチ"),
    ] {
        assert_eq!(
            reading_at(input, 0, input).as_deref(),
            Some(want),
            "{input}"
        );
    }
}

fn symbol_reading(input: &str, pos: usize, surface: &str) -> Option<String> {
    let p = provider();
    p.candidates_vec(&ctx(input), pos)
        .iter()
        .find(|c| c.surface == surface)
        .map(|c| c.reading.to_string())
}

#[test]
fn times_sign_is_kakeru_between_operands_and_batsu_otherwise() {
    // 数字・英字・括弧の間は掛け算
    for (input, pos) in [
        ("2×3", 1),
        ("A×B", 1),
        ("4K×2", 2),
        ("えび×3", "えび".len()),
        ("3×", 1),
        ("(1+2)×(3)", 5),
        ("２×３", "２".len()),
    ] {
        assert_eq!(
            symbol_reading(input, pos, "×").as_deref(),
            Some("カケル"),
            "{input}"
        );
    }
    // それ以外は誤りを表す ばつ
    for (input, pos) in [
        ("返事は×です", "返事は".len()),
        ("×印", 0),
        ("○×クイズ", "○".len()),
        ("ゴミ×", "ゴミ".len()),
        ("予定×", "予定".len()),
    ] {
        assert_eq!(
            symbol_reading(input, pos, "×").as_deref(),
            Some("バツ"),
            "{input}"
        );
    }
}

#[test]
fn decorative_shapes_are_read_only_as_two_symbol_placeholders() {
    let mut r = rules();
    for (c, k) in [('■', "シカク"), ('▲', "サンカク"), ('●', "マル")] {
        r.symbols.entries.insert(c.to_string(), k.to_string());
    }
    let before = NumberCandidateProvider::new(&r);
    let unlisted = before
        .candidates_vec(&ctx("●師匠"), 0)
        .iter()
        .find(|c| c.surface == "●")
        .map(|c| c.reading.to_string());
    assert_eq!(unlisted, None);
    r.context_cues.placeholder.honorifics.push("師匠".into());
    let p = NumberCandidateProvider::new(&r);
    let at = |input: &str, pos: usize, surface: &str| {
        p.candidates_vec(&ctx(input), pos)
            .iter()
            .find(|c| c.surface == surface)
            .map(|c| c.reading.to_string())
    };
    // ちょうど 2 つ並び (後ろは何でもよい) か、 直後が敬称なら読む
    assert_eq!(at("■■", 0, "■").as_deref(), Some("シカク"));
    assert_eq!(at("■■", "■".len(), "■").as_deref(), Some("シカク"));
    assert_eq!(at("▲▲の件", 0, "▲").as_deref(), Some("サンカク"));
    assert_eq!(at("●さん", 0, "●").as_deref(), Some("マル"));
    assert_eq!(at("●様", 0, "●").as_deref(), Some("マル"));
    // 辞書 (context_cues.toml の [placeholder] honorifics) で足した敬称・肩書きも効く
    assert_eq!(at("●師匠", 0, "●").as_deref(), Some("マル"));
    // 1 つだけ / 3 つ以上の装飾は読まない
    assert_eq!(at("■", 0, "■"), None);
    assert_eq!(at("◆■重要■", "◆".len(), "■"), None);
    assert_eq!(at("■■■お知らせ", 0, "■"), None);
    assert_eq!(at("■■■お知らせ", "■■".len(), "■"), None);
    // ○ △ □ は装飾ではないので常に読む (辞書にあれば)
}

#[test]
fn postal_code_is_read_digit_by_digit_with_no() {
    // 〒 の後 (空白は任意)・ハイフンは任意・全角の長音や罫線のハイフンも可
    for (input, pos, surface) in [
        ("〒100-0001", 0, "〒100-0001"),
        ("〒1000001", 0, "〒1000001"),
        ("〒 100-0001東京", 0, "〒 100-0001"),
        ("〒　100-0001", 0, "〒　100-0001"),
        ("〒100‐0001", 0, "〒100‐0001"),
        ("〒100ー0001", 0, "〒100ー0001"),
    ] {
        assert_eq!(
            reading_at(input, pos, surface).as_deref(),
            Some("ユウビンバンゴウイチゼロゼロノゼロゼロゼロイチ"),
            "{input}"
        );
    }
    // 「郵便番号」 の直後 (区切りの空白・コロンは任意) は 〒 無しでも同じ
    for (input, pos) in [
        ("郵便番号100-0001", "郵便番号".len()),
        ("郵便番号 100-0001", "郵便番号 ".len()),
        ("郵便番号：100-0001", "郵便番号：".len()),
        ("郵便番号:1000001", "郵便番号:".len()),
    ] {
        let surface = &input[pos..];
        assert_eq!(
            reading_at(input, pos, surface).as_deref(),
            Some("イチゼロゼロノゼロゼロゼロイチ"),
            "{input}"
        );
    }
    // 桁数が違う / 前後が数字 / 「郵便番号」 が無い 3-4 桁は郵便番号にしない
    assert!(reading_at("〒10-0001", 0, "〒10-0001").is_none());
    assert!(reading_at("〒100-00012", 0, "〒100-00012").is_none());
    assert!(reading_at("〒100-0001", 0, "〒100-0001")
        .is_some_and(|r| r.starts_with("ユウビンバンゴウ")));
    assert!(reading_at("100-0001", 0, "100-0001").is_none_or(|r| !r.contains("ゼロゼロ")));
    assert!(reading_at("1100-0001", 1, "100-0001").is_none());
}

#[test]
fn slash_pair_ignores_longer_chains() {
    let p = provider();
    assert!(find(&p.candidates_vec(&ctx("2024/10/25"), 5), "10/25").is_none());
    assert!(find(&p.candidates_vec(&ctx("2024/10/25"), 0), "20/24").is_none());
    assert!(find(&p.candidates_vec(&ctx("12/25/10"), 0), "12/25").is_none());
    assert!(find(&p.candidates_vec(&ctx("1/2/25"), 0), "1/2").is_none());
    assert!(find(&p.candidates_vec(&ctx("1/2/25"), 2), "2/25").is_none());
    assert!(find(&p.candidates_vec(&ctx("a/12/25"), 2), "12/25").is_none());
}

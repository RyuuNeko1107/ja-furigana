use super::*;
use crate::analyzer::Analyzer;
use crate::scoring::boundary::BoundaryAnalysis;

fn ctx(input: &str) -> ScoringContext<'_> {
    let boundary = Box::leak(Box::new(BoundaryAnalysis::empty()));
    ScoringContext::new(input, boundary)
}

fn analyzer() -> Analyzer {
    Analyzer::new().expect("Analyzer init")
}

#[test]
fn percent_led_symbol_token_gets_suffix_edges() {
    // 55%→60%: Lindera は 「%→」 を 1 token にする。 「55%」 の直後 (→ の位置) から始まる edge が無いと
    // 数 + 助数詞の候補が token の途中で終わって採択できない
    let input = "55%→60%";
    let a = analyzer();
    let p = LinderaFallbackProvider::new(&a, input);
    let mut out = Vec::new();
    p.candidates_at(&ctx(input), "55%".len(), &mut out);
    assert!(out.iter().any(|c| c.range == ("55%".len().."55%→".len())));
    // % で始まらない記号の塊 (顔文字 / 絵文字の続き) は割らない
    let input = "(✨)";
    let p = LinderaFallbackProvider::new(&a, input);
    let mut out = Vec::new();
    p.candidates_at(&ctx(input), "(".len(), &mut out);
    assert!(out.iter().all(|c| c.range.end != "(✨".len()));
}

/// Lindera edge のうち surface が一致するものの読み。
fn edge_reading(a: &Analyzer, input: &str, surface: &str) -> String {
    let p = LinderaFallbackProvider::new(a, input);
    p.edges
        .iter()
        .find(|(s, e, _, _)| &input[*s..*e] == surface)
        .map(|(_, _, r, _)| r.clone())
        .unwrap_or_else(|| panic!("{surface} の edge が無い: {input}"))
}

/// IPADIC の 名詞-接尾 (疲れ=ヅカレ / 書き=ガキ / 勝ち=ガチ) は、 連濁が起きない位置
/// (文頭 / 助詞・接頭詞 お の直後 / ひらがなで終わる未知語の直後 / 記号の直後) では清音に戻す。
#[test]
fn noun_suffix_rendaku_is_devoiced_after_boundary() {
    let a = analyzer();
    assert_eq!(edge_reading(&a, "書きはムリ", "書き"), "カキ");
    assert_eq!(edge_reading(&a, "積みの速さ", "積み"), "ツミ");
    assert_eq!(edge_reading(&a, "ぺこらの勝ちだ", "勝ち"), "カチ");
    assert_eq!(edge_reading(&a, "ぺこちゃんお疲れ様", "疲れ"), "ツカレ");
    assert_eq!(edge_reading(&a, ":_勝ち猫:", "勝ち"), "カチ");
    // 代名詞の後ろも境界 (これ帰り / それ使い)。 名詞の後ろは濁ったまま (仕事帰り)
    assert_eq!(edge_reading(&a, "これ帰りもある", "帰り"), "カエリ");
    assert_eq!(edge_reading(&a, "それ使いやすい", "使い"), "ツカイ");
    assert_eq!(edge_reading(&a, "仕事帰り", "帰り"), "ガエリ");
    // 通り / 沿い は閉じ括弧・未知語の後ろでは名詞の続き
    assert_eq!(
        edge_reading(&a, "「TODO.md」通りに作って", "通り"),
        "ドオリ"
    );
    assert_eq!(edge_reading(&a, "国道４１号）沿いの", "沿い"), "ゾイ");
}

/// 「X食いに行く / 刺しにきてる」 は動詞の連用形 (目的) なので清音。
#[test]
fn noun_suffix_rendaku_is_devoiced_before_ni_motion_verb() {
    let a = analyzer();
    assert_eq!(edge_reading(&a, "ラーメン食いに行こう", "食い"), "クイ");
    assert_eq!(edge_reading(&a, "トドメ刺しにきてる", "刺し"), "サシ");
}

/// 名詞の後ろの連濁 (魔物使い / 野菜嫌い) と、 名詞の後ろで濁るのが普通の語
/// (時間通りに行く / 年越しに) ・ % の後ろ (20%引き) は濁ったまま。
#[test]
fn noun_suffix_rendaku_is_kept_after_noun() {
    let a = analyzer();
    assert_eq!(edge_reading(&a, "魔物使い", "使い"), "ヅカイ");
    assert_eq!(edge_reading(&a, "野菜嫌い", "嫌い"), "ギライ");
    assert_eq!(edge_reading(&a, "時間通りに行く", "通り"), "ドオリ");
    assert_eq!(edge_reading(&a, "20年越しに", "越し"), "ゴシ");
    assert_eq!(edge_reading(&a, "20%引きで", "引き"), "ビキ");
}

/// カタカナ語の直後の 使い (ロング使い / ユミン使い) は つかい。 漢字の語の後ろは ヅカイ のまま。
#[test]
fn tsukai_after_katakana_is_not_voiced() {
    let a = analyzer();
    assert_eq!(edge_reading(&a, "ロング使い", "使い"), "ツカイ");
    assert_eq!(edge_reading(&a, "ユミン使いになったの", "使い"), "ツカイ");
    assert_eq!(
        edge_reading(&a, "カーボンローラーデコ使いです", "使い"),
        "ツカイ"
    );
    // 漢字の語の後ろ (魔物使い / 人使い) は連濁のまま
    assert_eq!(edge_reading(&a, "魔物使い", "使い"), "ヅカイ");
    assert_eq!(edge_reading(&a, "野菜使い", "使い"), "ヅカイ");
}

#[test]
fn devoice_first_kana_only_touches_voiced_head() {
    assert_eq!(devoice_first_kana("ヅカレ").as_deref(), Some("ツカレ"));
    assert_eq!(devoice_first_kana("ビキ").as_deref(), Some("ヒキ"));
    assert_eq!(devoice_first_kana("ツカレ"), None);
    assert_eq!(devoice_first_kana("ハダカ"), None);
    assert_eq!(devoice_first_kana(""), None);
}

#[test]
fn okurigana_form_excludes_single_kanji_and_listed_words() {
    assert!(is_okurigana_form("疲れ"));
    assert!(!is_okurigana_form("代"));
    // 通り 等は境界の直後なら清音に戻す対象、 「に + 移動動詞」 の前では対象外
    assert!(is_okurigana_form("通り"));
    assert!(!is_okurigana_form_voiced_after_noun_excluded("通り"));
    assert!(is_okurigana_form_voiced_after_noun_excluded("疲れ"));
    assert!(!is_okurigana_form("出し"));
    assert!(!is_okurigana_form("ツカレ"));
}

/// かな token の途中から始まる edge が生えること。
///
/// これが無いと 「dict entry が Lindera token の途中で終わる」 入力で、
/// entry を含む path が DP 上構築できず entry が黙って無視される
/// (例: 「お婆ちゃんすげぇ」 で 「お婆ちゃん」 entry が使われない)。
/// Lindera が落とす文字 (例: Ð U+00D0) が混ざっても、 入力全体の読みが消えないこと。
///
/// 以前は gap が空白/制御文字でないと safety net 自体を無効化していたため、
/// 「応援曲個人設定できるんÐな」 のような入力が **空文字列**になっていた。
#[test]
fn unreadable_char_does_not_disable_safety_net() {
    let a = analyzer();
    let input = "応援曲できるんÐな";
    let p = LinderaFallbackProvider::new(&a, input);
    assert!(
        !p.edges.is_empty(),
        "Lindera が落とす文字が入っただけで safety net が全廃されている"
    );
    // 入力全体が edge で覆われていること (= dp が到達できる)
    let covered: usize = {
        let mut max_end = 0usize;
        let mut pos = 0usize;
        loop {
            let next = p
                .edges
                .iter()
                .filter(|(s, _, _, _)| *s == pos)
                .map(|(_, e, _, _)| *e)
                .max();
            match next {
                Some(e) if e > pos => {
                    pos = e;
                    max_end = e;
                }
                _ => break,
            }
        }
        max_end
    };
    assert_eq!(covered, input.len(), "入力末尾まで edge が繋がっていない");
}

#[test]
fn kana_token_emits_suffix_edges() {
    let a = analyzer();
    let input = "お婆ちゃんすげぇ";
    let p = LinderaFallbackProvider::new(&a, input);
    // 「お婆ちゃん」 (= 0..12 bytes) の直後、 「す」 の位置から始まる edge があること
    let pos = "お婆ちゃん".len();
    let cands = p.candidates_vec(&ctx(input), pos);
    assert!(
        !cands.is_empty(),
        "かな token 途中の位置 {pos} から始まる edge が無い (entry の継続先が消える)"
    );
    assert!(
        cands.iter().any(|c| c.surface == "すげぇ"),
        "残り全体 「すげぇ」 を覆う edge が要る: {:?}",
        cands.iter().map(|c| c.surface.as_str()).collect::<Vec<_>>()
    );
}

/// suffix edge の reading は surface と同一 (かなはそのまま読む)。
#[test]
fn kana_suffix_edge_reading_is_identity() {
    let a = analyzer();
    let input = "お婆ちゃんすげぇ";
    let p = LinderaFallbackProvider::new(&a, input);
    let pos = "お婆ちゃん".len();
    let c = p
        .candidates_vec(&ctx(input), pos)
        .into_iter()
        .find(|c| c.surface == "すげぇ")
        .expect("suffix edge");
    assert_eq!(c.reading, "すげぇ");
}

/// 漢字を含む token には suffix edge を足さない (読みを機械的に切れないため)。
#[test]
fn kanji_token_gets_no_suffix_edges() {
    let a = analyzer();
    let input = "図書館";
    let p = LinderaFallbackProvider::new(&a, input);
    // 「書」 の位置 (= token 途中) から Lindera 由来 edge は生えない
    let pos = "図".len();
    assert!(p
        .candidates_vec(&ctx(input), pos)
        .iter()
        .all(|c| c.surface != "書館"));
}

#[test]
fn empty_input_yields_no_edges() {
    let a = analyzer();
    let p = LinderaFallbackProvider::new(&a, "");
    assert_eq!(p.candidates_vec(&ctx(""), 0).len(), 0);
}

#[test]
fn hiragana_particle_emits_band_50_candidate() {
    let a = analyzer();
    let input = "猫が好き";
    let p = LinderaFallbackProvider::new(&a, input);
    // 「猫」 の byte 範囲は 0..3 (UTF-8 3 byte)、 「が」 は 3..6
    let cands_at_3 = p.candidates_vec(&ctx(input), 3);
    assert!(
        !cands_at_3.is_empty(),
        "expected Lindera edge at pos=3 (=が)"
    );
    let ga = cands_at_3.iter().find(|c| c.surface == "が");
    assert!(ga.is_some(), "expected が candidate: {cands_at_3:?}");
    let ga = ga.unwrap();
    assert_eq!(ga.score.band, 50);
    // 助詞 「が」 の Lindera reading は 「ガ」 (カタカナ)
    assert_eq!(ga.reading, "ガ");
}

// (band 値の純粋な大小比較は Score の Ord 契約であり candidate.rs で網羅済。
//  ここでは Provider が実際に出す Candidate.score.band を検証する。)

#[test]
fn kanji_compound_surface_gets_band_150() {
    // 「最近」 (= 2 字純漢字 surface) の Lindera token → band 150 になることを確認。
    // dict に 「最近」 が未登録の前提 (= IPADIC 形態素解析だけが頼り)。
    let a = analyzer();
    let input = "最近の話";
    let p = LinderaFallbackProvider::new(&a, input);
    let cands_at_0 = p.candidates_vec(&ctx(input), 0);
    // find が None だと旧 test は no-op で緑になっていた。候補存在を強制する。
    let saikin = cands_at_0
        .iter()
        .find(|c| c.surface == "最近")
        .unwrap_or_else(|| panic!("IPADIC は「最近」を 1 token で返すはず: {cands_at_0:?}"));
    assert_eq!(
        saikin.score.band, 150,
        "「最近」 (2 字純漢字) は band 150 (= lindera_compound) を期待"
    );
}

#[test]
fn single_kanji_surface_stays_band_50() {
    // 単漢字 surface (= 1 字) は band 50 のまま (= overrides.toml に譲る)。
    let a = analyzer();
    let input = "私";
    let p = LinderaFallbackProvider::new(&a, input);
    let cands = p.candidates_vec(&ctx(input), 0);
    let watashi = cands
        .iter()
        .find(|c| c.surface == "私")
        .unwrap_or_else(|| panic!("「私」 token が出るはず: {cands:?}"));
    assert_eq!(watashi.score.band, 50, "単漢字 surface は band 50 維持");
}

#[test]
fn kanji_okurigana_mixed_stays_band_50() {
    // 漢字 + ひらがな混在 surface (= 「来た」) は band 50 (= [[kanji]] block 経路に譲る)。
    let a = analyzer();
    let input = "来た";
    let p = LinderaFallbackProvider::new(&a, input);
    let cands = p.candidates_vec(&ctx(input), 0);
    // 空 Vec だと旧 for は vacuously pass。少なくとも 1 edge は出るはず。
    assert!(!cands.is_empty(), "「来た」 は edge を生むはず");
    // Lindera が 「来」 + 「た」 と 2 token に分ける場合、 各々 1 字 → 50。
    // 仮に 1 token (「来た」) で返した場合も混在で band 50。
    for c in &cands {
        assert_eq!(
            c.score.band, 50,
            "漢字+okurigana 混在 surface は band 50 維持 ({:?})",
            c.surface
        );
    }
}

#[test]
fn unknown_token_falls_back_to_surface_reading() {
    // 完全に未知の記号列 等は Lindera reading None になるので surface を reading 化、
    // 「読まないが path 上の edge にはなる」 動作 (= path 構築の safety net = gap-passthrough)。
    let a = analyzer();
    let input = "★";
    let p = LinderaFallbackProvider::new(&a, input);
    let cands = p.candidates_vec(&ctx(input), 0);
    // safety net は「必ず」 edge を出す契約 (空だと後段で文字が消える)。
    let first = cands
        .first()
        .unwrap_or_else(|| panic!("safety net は ★ にも edge を出すはず: {cands:?}"));
    assert!(
        !first.reading.is_empty(),
        "reading should fallback to surface"
    );
}

fn first_band(p: &LinderaFallbackProvider, input: &str, surface: &str) -> Option<(u16, String)> {
    p.candidates_vec(&ctx(input), 0)
        .into_iter()
        .find(|c| c.surface == surface)
        .map(|c| (c.score.band, c.reading))
}

/// 1 字だけ切り出された一段動詞 (寝|ん) は、 活用込みの読みを単漢字 band で出す。
/// (故障モデル: 条件を外すと band 50 のままで、 単漢字 default (寝 = シン) に負ける)
#[test]
fn single_kanji_ichidan_verb_stem_gets_kanji_band() {
    let a = analyzer();
    let p = LinderaFallbackProvider::new(&a, "寝んな");
    assert_eq!(
        first_band(&p, "寝んな", "寝"),
        Some((BAND_KANJI, "ネ".to_string()))
    );
}

/// 熟語内の 1 字 (逆|走) / 名詞に付く断定 (免停です) / 除外字 (得) は格上げしない。
#[test]
fn verb_stem_promotion_is_narrow() {
    let a = analyzer();
    for input in ["走ってる", "得すぎる", "お得だね"] {
        let p = LinderaFallbackProvider::new(&a, input);
        for c in p.candidates_vec(&ctx(input), 0) {
            assert_ne!(c.score.band, BAND_KANJI, "{input}: {c:?}");
        }
    }
    // 直前が漢字 (暗|視) の 1 字は格上げしない
    let input = "暗視ない";
    let p = LinderaFallbackProvider::new(&a, input);
    let pos = "暗".len();
    for c in p.candidates_vec(&ctx(input), pos) {
        assert_ne!(c.score.band, BAND_KANJI, "{input}: {c:?}");
    }
}

/// 行き止まり補完: 「断捨離して」 は Lindera が 離し を 1 token にするので、
/// 通常の suffix edge では し の位置に edge が無い (tail 先頭は skip)。
/// `dead_end_candidates_at` は token の残り し を足す。
#[test]
fn dead_end_fills_kana_rest_of_mixed_token() {
    let a = analyzer();
    let input = "断捨離して";
    let p = LinderaFallbackProvider::new(&a, input);
    let pos = "断捨離".len();
    let boundary = Box::leak(Box::new(BoundaryAnalysis::empty()));
    let ctx = ScoringContext::new(input, boundary);
    let mut out = Vec::new();
    p.dead_end_candidates_at(&ctx, pos, &mut out);
    let surfaces: Vec<&str> = out.iter().map(|c| &ctx.input[c.range.clone()]).collect();
    assert!(
        surfaces.contains(&"し"),
        "残り 「し」 が補われる: {surfaces:?}"
    );
    for c in &out {
        assert_eq!(c.score.band, 50, "補完 edge は Lindera と同じ band 50");
    }
}

/// 残りに漢字が混じる位置 / token 境界そのもの では補わない。
#[test]
fn dead_end_does_not_fill_kanji_or_token_boundary() {
    let a = analyzer();
    let input = "断捨離して";
    let p = LinderaFallbackProvider::new(&a, input);
    let boundary = Box::leak(Box::new(BoundaryAnalysis::empty()));
    let ctx = ScoringContext::new(input, boundary);
    // token 境界 (= 0) では何も出さない
    let mut out = Vec::new();
    p.dead_end_candidates_at(&ctx, 0, &mut out);
    assert!(out.is_empty(), "token 先頭は通常 edge の担当: {out:?}");
    // 「捨」 の位置: 残り 「捨離し」 は漢字を含むので出さない
    let mut out = Vec::new();
    p.dead_end_candidates_at(&ctx, "断".len(), &mut out);
    assert!(out.is_empty(), "漢字を含む残りは補わない: {out:?}");
}

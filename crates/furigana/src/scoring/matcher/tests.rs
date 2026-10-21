use super::tokens::MAX_LOGICAL_TOKEN_CHARS;
use super::*;

fn cond_default() -> MatchCondition {
    MatchCondition::default()
}

// ─── 無条件 match ────────────────────────────────────────────────────────

#[test]
fn empty_condition_matches_any_context() {
    let cond = cond_default();
    assert!(cond.matches_context(&MatchContext::empty()));
    assert!(cond.matches_context(&MatchContext::with_prev("前")));
    assert!(cond.matches_context(&MatchContext::with_next("後")));
    assert!(cond.matches_context(&MatchContext::with_both("前", "後")));
}

// ─── prev_eq ─────────────────────────────────────────────────────────────

#[test]
fn prev_eq_matches_when_equal() {
    let cond = MatchCondition {
        prev_eq: Some("階段".into()),
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_prev("階段")));
    assert!(!cond.matches_context(&MatchContext::with_prev("梯子")));
    assert!(!cond.matches_context(&MatchContext::empty()));
}

// ─── prev_eq_any ─────────────────────────────────────────────────────────

#[test]
fn prev_eq_any_matches_when_in_list() {
    let cond = MatchCondition {
        prev_eq_any: vec!["階段".into(), "段".into(), "梯子".into()],
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_prev("階段")));
    assert!(cond.matches_context(&MatchContext::with_prev("段")));
    assert!(cond.matches_context(&MatchContext::with_prev("梯子")));
    assert!(!cond.matches_context(&MatchContext::with_prev("丘")));
    assert!(!cond.matches_context(&MatchContext::empty()));
}

// ─── next_eq ─────────────────────────────────────────────────────────────

#[test]
fn next_eq_matches_when_equal() {
    let cond = MatchCondition {
        next_eq: Some("から".into()),
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_next("から")));
    assert!(!cond.matches_context(&MatchContext::with_next("まで")));
    assert!(!cond.matches_context(&MatchContext::empty()));
}

// ─── next_eq_any ─────────────────────────────────────────────────────────

#[test]
fn next_eq_any_matches_when_in_list() {
    let cond = MatchCondition {
        next_eq_any: vec!["まれ".into(), "まれる".into()],
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_next("まれ")));
    assert!(cond.matches_context(&MatchContext::with_next("まれる")));
    assert!(!cond.matches_context(&MatchContext::with_next("じる")));
}

// ─── context_hits の重み (literal=2 / broad=1) ───────────────────────────
// matches_context (bool) では検出できない hit 重みの累積を固定する。
// (故障モデル: 各条件の `hits += W` を壊すと Score::match_hits 第 3 軸が崩れ、
//  同 band / 同 length で「より厳密な条件で書いた block」が勝てなくなる)

#[test]
fn context_hits_prev_eq_any_is_literal_weight() {
    let cond = MatchCondition {
        prev_eq_any: vec!["段".into()],
        ..Default::default()
    };
    assert_eq!(
        cond.context_hits(&MatchContext::with_prev("段")),
        Some(HIT_WEIGHT_LITERAL)
    );
    assert_eq!(cond.context_hits(&MatchContext::with_prev("丘")), None);
}

#[test]
fn context_hits_prev_ends_any_is_broad_weight() {
    let cond = MatchCondition {
        prev_ends_any: vec!["けて".into()],
        ..Default::default()
    };
    assert_eq!(
        cond.context_hits(&MatchContext::with_prev("かけて")),
        Some(HIT_WEIGHT_BROAD)
    );
    assert_eq!(cond.context_hits(&MatchContext::with_prev("かく")), None);
}

#[test]
fn context_hits_next_starts_any_is_broad_weight() {
    let cond = MatchCondition {
        next_starts_any: vec!["ける".into()],
        ..Default::default()
    };
    assert_eq!(
        cond.context_hits(&MatchContext::with_next("けるかも")),
        Some(HIT_WEIGHT_BROAD)
    );
    assert_eq!(cond.context_hits(&MatchContext::with_next("ない")), None);
}

#[test]
fn context_hits_next_eq_any_is_literal_weight() {
    let cond = MatchCondition {
        next_eq_any: vec!["から".into()],
        ..Default::default()
    };
    assert_eq!(
        cond.context_hits(&MatchContext::with_next("から")),
        Some(HIT_WEIGHT_LITERAL)
    );
    assert_eq!(cond.context_hits(&MatchContext::with_next("まで")), None);
}

#[test]
fn context_hits_next2_starts_any_is_broad_weight() {
    let cond = MatchCondition {
        next2_starts_any: vec!["旅".into()],
        ..Default::default()
    };
    // next2 のみ指定 → with_all(prev=None, next=None, next2=Some)
    assert_eq!(
        cond.context_hits(&MatchContext::with_all(None, None, Some("旅行"))),
        Some(HIT_WEIGHT_BROAD)
    );
    assert_eq!(
        cond.context_hits(&MatchContext::with_all(None, None, Some("集団"))),
        None
    );
}

#[test]
fn context_hits_prev_char_type_is_broad_weight() {
    let cond = MatchCondition {
        prev_char_type: Some(CharType::Kanji),
        ..Default::default()
    };
    // prev token 末尾が漢字 → BROAD hit
    assert_eq!(
        cond.context_hits(&MatchContext::with_prev("漢")),
        Some(HIT_WEIGHT_BROAD)
    );
    // 末尾がひらがな → miss
    assert_eq!(cond.context_hits(&MatchContext::with_prev("あ")), None);
}

// ─── prev_char_type ──────────────────────────────────────────────────────

#[test]
fn prev_char_type_kanji_skips_odoriji() {
    // 畳語 (時々 / 早々) の後ろは新しい語の頭なので 「前が漢字」 に数えない
    let cond = MatchCondition {
        prev_char_type: Some(CharType::Kanji),
        ..Default::default()
    };
    assert!(!cond.matches_context(&MatchContext::with_prev("時々")));
    assert!(!cond.matches_context(&MatchContext::with_prev("々")));
}

#[test]
fn prev_char_type_matches_kanji() {
    let cond = MatchCondition {
        prev_char_type: Some(CharType::Kanji),
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_prev("校"))); // 「高校」 の最後
    assert!(cond.matches_context(&MatchContext::with_prev("高校"))); // 末尾文字 = 「校」
    assert!(!cond.matches_context(&MatchContext::with_prev("きの"))); // ひらがな末尾
}

#[test]
fn next_char_type_matches_hiragana() {
    let cond = MatchCondition {
        next_char_type: Some(CharType::Hiragana),
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_next("じる"))); // 先頭 = 「じ」
    assert!(cond.matches_context(&MatchContext::with_next("から"))); // 先頭 = 「か」
    assert!(!cond.matches_context(&MatchContext::with_next("漢字"))); // 先頭 = 「漢」
    assert!(!cond.matches_context(&MatchContext::empty()));
}

// ─── AND 結合 ────────────────────────────────────────────────────────────

#[test]
fn multiple_conditions_combined_with_and() {
    let cond = MatchCondition {
        prev_eq: Some("生".into()),
        next_eq: Some("じる".into()),
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_both("生", "じる")));
    assert!(!cond.matches_context(&MatchContext::with_both("生", "まれる"))); // next_eq miss
    assert!(!cond.matches_context(&MatchContext::with_both("死", "じる"))); // prev_eq miss
}

#[test]
fn prev_char_type_and_next_eq_combined() {
    let cond = MatchCondition {
        prev_char_type: Some(CharType::Hiragana),
        next_eq: Some("クリーム".into()),
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_both("きの", "クリーム")));
    assert!(!cond.matches_context(&MatchContext::with_both("漢字", "クリーム")));
    assert!(!cond.matches_context(&MatchContext::with_both("きの", "ジュース")));
}

// (classify_char / range 判定の unit test は crate::char_class 側に移動)

// ─── multi-byte token char_type 判定 ─────────────────────────────────────

#[test]
fn prev_char_type_uses_last_char_of_multi_char_token() {
    let cond = MatchCondition {
        prev_char_type: Some(CharType::Kanji),
        ..Default::default()
    };
    // 「きの生」 の末尾は 「生」 = 漢字 → match
    assert!(cond.matches_context(&MatchContext::with_prev("きの生")));
    // 「漢字きの」 の末尾は 「の」 = ひらがな → no match
    assert!(!cond.matches_context(&MatchContext::with_prev("漢字きの")));
}

#[test]
fn next_char_type_uses_first_char_of_multi_char_token() {
    let cond = MatchCondition {
        next_char_type: Some(CharType::Kanji),
        ..Default::default()
    };
    // 「生まれ」 の先頭は 「生」 = 漢字 → match
    assert!(cond.matches_context(&MatchContext::with_next("生まれ")));
    // 「まれ生」 の先頭は 「ま」 = ひらがな → no match
    assert!(!cond.matches_context(&MatchContext::with_next("まれ生")));
}

// ─── prev_ends_any (literal suffix) ─────────────────────────────────────

#[test]
fn prev_ends_any_matches_when_suffix_in_list() {
    let cond = MatchCondition {
        prev_ends_any: vec!["校".into(), "学校".into()],
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_prev("高校"))); // ends with 校
    assert!(cond.matches_context(&MatchContext::with_prev("中学校"))); // ends with 学校
    assert!(!cond.matches_context(&MatchContext::with_prev("高")));
    assert!(!cond.matches_context(&MatchContext::empty()));
}

// ─── next_starts ────────────────────────────────────────────────────────

#[test]
fn next_starts_matches_when_prefix_equal() {
    let cond = MatchCondition {
        next_starts: Some("な".into()),
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_next("ない")));
    assert!(cond.matches_context(&MatchContext::with_next("なんて")));
    assert!(!cond.matches_context(&MatchContext::with_next("だ")));
    assert!(!cond.matches_context(&MatchContext::empty()));
}

// ─── next_starts_any (literal prefix any) ───────────────────────────────

#[test]
fn next_starts_any_matches_when_any_prefix_in_list() {
    let cond = MatchCondition {
        next_starts_any: vec!["な".into(), "無".into()],
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_next("ない")));
    assert!(cond.matches_context(&MatchContext::with_next("無い")));
    assert!(!cond.matches_context(&MatchContext::with_next("だ")));
    assert!(!cond.matches_context(&MatchContext::empty()));
}

// ─── next2_starts_any (1 飛ばし) ────────────────────────────────────────

#[test]
fn next2_starts_any_matches_token_at_idx_plus_2() {
    // 「人気が無い」 のような pattern: surface=人気、 next=が、 next2=無い
    let cond = MatchCondition {
        next_eq: Some("が".into()),
        next2_starts_any: vec!["な".into(), "無".into()],
        ..Default::default()
    };
    let ctx = MatchContext::with_all(None, Some("が"), Some("無い"));
    assert!(cond.matches_context(&ctx));
    // next2 が異なる → no match
    let ctx2 = MatchContext::with_all(None, Some("が"), Some("出る"));
    assert!(!cond.matches_context(&ctx2));
    // next2 不在 → no match
    let ctx3 = MatchContext::with_all(None, Some("が"), None);
    assert!(!cond.matches_context(&ctx3));
}

// ─── prev_month (predicate) ─────────────────────────────────────────────

#[test]
fn prev_month_matches_kanji_month_endings() {
    let cond = MatchCondition {
        prev_month: true,
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_prev("一月"))); // exact
    assert!(cond.matches_context(&MatchContext::with_prev("六月"))); // exact
    assert!(cond.matches_context(&MatchContext::with_prev("十二月")));
    assert!(cond.matches_context(&MatchContext::with_prev("先月の十一月"))); // ends_with
    assert!(!cond.matches_context(&MatchContext::with_prev("月曜日"))); // 月 alone, not month
    assert!(!cond.matches_context(&MatchContext::empty()));
}

#[test]
fn prev_month_matches_arabic_month_endings() {
    let cond = MatchCondition {
        prev_month: true,
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_prev("1月")));
    assert!(cond.matches_context(&MatchContext::with_prev("12月")));
    assert!(cond.matches_context(&MatchContext::with_prev("１月"))); // 全角
    assert!(cond.matches_context(&MatchContext::with_prev("９月"))); // 全角
    assert!(!cond.matches_context(&MatchContext::with_prev("1日"))); // 月 ではない
}

// ─── next_digit (predicate) ─────────────────────────────────────────────

#[test]
fn next_digit_matches_when_starts_with_digit() {
    let cond = MatchCondition {
        next_digit: true,
        ..Default::default()
    };
    assert!(cond.matches_context(&MatchContext::with_next("1日")));
    assert!(cond.matches_context(&MatchContext::with_next("123")));
    assert!(cond.matches_context(&MatchContext::with_next("０時"))); // 全角
    assert!(!cond.matches_context(&MatchContext::with_next("一日"))); // 漢数字は false
    assert!(!cond.matches_context(&MatchContext::with_next("ABC")));
    assert!(!cond.matches_context(&MatchContext::empty()));
}

// ─── input_contains_any (文スコープ) ────────────────────────────────────

#[test]
fn input_contains_any_matches_anywhere_in_sentence() {
    let cond = MatchCondition {
        input_contains_any: vec!["リーチ".into(), "ツモ".into()],
        ..Default::default()
    };
    // 隣接していなくても同一文中にあれば hit
    assert!(cond
        .matches_context(&MatchContext::empty().with_full_input("リーチしたけど清一色が見えてる")));
    assert!(cond.matches_context(&MatchContext::empty().with_full_input("ツモって清一色確定")));
    assert!(
        !cond.matches_context(&MatchContext::empty().with_full_input("この職場は清一色の男性だ"))
    );
}

#[test]
fn input_contains_any_without_full_input_is_no_match() {
    let cond = MatchCondition {
        input_contains_any: vec!["リーチ".into()],
        ..Default::default()
    };
    // full_input 未指定 (= 旧 caller) は必ず miss、 default reading に落ちる
    assert!(!cond.matches_context(&MatchContext::empty()));
    assert!(!cond.matches_context(&MatchContext::with_prev("リーチ")));
}

#[test]
fn input_contains_any_combines_with_adjacent_conditions_as_and() {
    let cond = MatchCondition {
        next_eq: Some("で".into()),
        input_contains_any: vec!["麻雀".into()],
        ..Default::default()
    };
    let ctx = MatchContext::with_next("で").with_full_input("麻雀で清一色で上がった");
    assert!(cond.matches_context(&ctx));
    // 文スコープ側が miss なら AND で全体 miss
    let ctx2 = MatchContext::with_next("で").with_full_input("清一色で揃えた制服");
    assert!(!cond.matches_context(&ctx2));
}

#[test]
fn input_contains_any_is_broad_weight() {
    let cond = MatchCondition {
        input_contains_any: vec!["麻雀".into()],
        ..Default::default()
    };
    assert_eq!(
        cond.context_hits(&MatchContext::empty().with_full_input("麻雀の話")),
        Some(HIT_WEIGHT_BROAD)
    );
}

// ─── context_hits (= match_hits 重み付き累積、 lex 第 3 軸) ─────────────

#[test]
fn context_hits_unconditional_match_is_zero() {
    let cond = cond_default();
    assert_eq!(cond.context_hits(&MatchContext::empty()), Some(0));
}

#[test]
fn context_hits_literal_exact_weighs_two() {
    let cond = MatchCondition {
        next_eq: Some("から".into()),
        ..Default::default()
    };
    assert_eq!(
        cond.context_hits(&MatchContext::with_next("から")),
        Some(HIT_WEIGHT_LITERAL)
    );
    assert_eq!(cond.context_hits(&MatchContext::with_next("まで")), None);
}

#[test]
fn context_hits_broad_condition_weighs_one() {
    let cond = MatchCondition {
        next_char_type: Some(CharType::Hiragana),
        ..Default::default()
    };
    assert_eq!(
        cond.context_hits(&MatchContext::with_next("じる")),
        Some(HIT_WEIGHT_BROAD)
    );
}

#[test]
fn context_hits_accumulates_across_conditions() {
    // literal (prev_eq = 2) + broad (next_char_type = 1) = 3
    let cond = MatchCondition {
        prev_eq: Some("生".into()),
        next_char_type: Some(CharType::Hiragana),
        ..Default::default()
    };
    assert_eq!(
        cond.context_hits(&MatchContext::with_both("生", "じる")),
        Some(HIT_WEIGHT_LITERAL + HIT_WEIGHT_BROAD)
    );
    // AND miss → None (部分 hit は加算しない)
    assert_eq!(
        cond.context_hits(&MatchContext::with_both("死", "じる")),
        None
    );
}

#[test]
fn context_hits_next_starts_is_literal_weight() {
    let cond = MatchCondition {
        next_starts: Some("な".into()),
        ..Default::default()
    };
    assert_eq!(
        cond.context_hits(&MatchContext::with_next("ない")),
        Some(HIT_WEIGHT_LITERAL)
    );
}

#[test]
fn context_hits_predicates_weigh_one() {
    let cond = MatchCondition {
        prev_month: true,
        next_digit: true,
        ..Default::default()
    };
    let ctx = MatchContext::with_both("一月", "1日");
    assert_eq!(cond.context_hits(&ctx), Some(HIT_WEIGHT_BROAD * 2));
}

// ─── resolve_readings の match_hits ──────────────────────────────────────

#[test]
fn resolve_readings_primary_carries_hits_default_is_zero() {
    let matches = vec![MatchBlock {
        reading: "ジョウズ".into(),
        condition: MatchCondition {
            next_eq: Some("だ".into()),
            ..Default::default()
        },
    }];
    // match block hit → primary に重み付き hits
    let hit = resolve_readings(&matches, "ウワテ", &[], &MatchContext::with_next("だ"))
        .collect::<Vec<_>>();
    assert_eq!(hit, vec![("ジョウズ", WEIGHT_DEFAULT, HIT_WEIGHT_LITERAL)]);
    // miss → default、 hits 0
    let miss =
        resolve_readings(&matches, "ウワテ", &[], &MatchContext::empty()).collect::<Vec<_>>();
    assert_eq!(miss, vec![("ウワテ", WEIGHT_DEFAULT, 0)]);
}

#[test]
fn resolve_readings_alt_hits_always_zero() {
    // alt に condition があっても match_hits は 0 (ADR-0004 default-on-path 維持)
    let alts = vec![Alternative {
        reading: "カミテ".into(),
        sense: None,
        weight: 30,
        condition: MatchCondition {
            next_eq: Some("から".into()),
            ..Default::default()
        },
    }];
    let out = resolve_readings(&[], "ウワテ", &alts, &MatchContext::with_next("から"))
        .collect::<Vec<_>>();
    assert_eq!(out, vec![("ウワテ", WEIGHT_DEFAULT, 0), ("カミテ", 30, 0)]);
}

// ─── pseudo-token segmentation (= char-class-based) ─────────────────────

#[test]
fn next_logical_token_grabs_hiragana_run() {
    // 「上手から登場」 の pos 6 (= 「上手」 後) → 「から」 (= ひらがな連続、 漢字 「登」 で切れる)
    assert_eq!(next_logical_token("上手から登場", 6), "から");
}

#[test]
fn next_logical_token_grabs_kanji_run() {
    // 「東方の上手」 の pos 9 (= 「の」 後) → 「上手」 (= 漢字連続、 EOF or 文字種境界で切れる)
    assert_eq!(next_logical_token("東方の上手", 9), "上手");
}

#[test]
fn next_logical_token_empty_at_end() {
    let s = "上手";
    assert_eq!(next_logical_token(s, s.len()), "");
}

#[test]
fn next_logical_token_handles_digit() {
    assert_eq!(next_logical_token("3本", 0), "3");
    assert_eq!(next_logical_token("100km", 0), "100km"); // 英数 + ASCII 連続
}

#[test]
fn next2_logical_token_skips_one() {
    // 「人気が無い」 の pos 6 (= 「人気」 後) → next = 「が」、 next2 = 「無」 (= 単独 1 字 kanji)
    let s = "人気が無い";
    // next1 = 「が」 (ひらがな)、 「無」 で切れる
    assert_eq!(next_logical_token(s, 6), "が");
    // next2 開始 = pos 6 + len("が") = 6 + 3 = 9
    assert_eq!(next2_logical_token(s, 6), "無"); // 「無」 漢字単独、 「い」 ひらがなで切れる
}

#[test]
fn prev_logical_token_grabs_hiragana_run() {
    // 「お上手」 の pos 3 (= 「上手」 の直前 = 「お」 の後) → 「お」 (= ひらがな単独)
    assert_eq!(prev_logical_token("お上手", 3), "お");
}

#[test]
fn prev_logical_token_grabs_kanji_run() {
    // 「中学校生」 の pos 9 (= 「生」 の直前 = 「中学校」 の後) → 「中学校」 (= 漢字連続)
    assert_eq!(prev_logical_token("中学校生", 9), "中学校");
}

#[test]
fn prev_logical_token_empty_at_start() {
    assert_eq!(prev_logical_token("上手", 0), "");
}

#[test]
fn prev_logical_token_with_month_pattern() {
    // 「6月一日」 の pos 6 (= 「一日」 の前 = 「6月」 の後) → 「6月」 だが
    // char-class は 英数 + 漢字 で boundary、 prev は 「月」 漢字 1 字 のみ
    // (= 「6」 と 「月」 は class 違う = 英数 vs 漢字)
    assert_eq!(prev_logical_token("6月一日", 4), "月"); // 1 ("6") + 3 ("月") = 4 bytes
}

#[test]
fn logical_token_caps_pathological_run() {
    // 同一漢字の超長 run でも走査は MAX_LOGICAL_TOKEN_CHARS で打ち切られる
    // (O(N²) 回避)。 上限超の token は如何なる condition とも一致しないので
    // 打ち切っても match 結果は不変。
    let long_run = "鬱".repeat(MAX_LOGICAL_TOKEN_CHARS + 500);
    let fwd = next_logical_token(&long_run, 0);
    assert_eq!(
        fwd.chars().count(),
        MAX_LOGICAL_TOKEN_CHARS,
        "前方走査を上限で打ち切り"
    );
    let bwd = prev_logical_token(&long_run, long_run.len());
    assert_eq!(
        bwd.chars().count(),
        MAX_LOGICAL_TOKEN_CHARS,
        "後方走査を上限で打ち切り"
    );
    // 上限以下の通常 run は従来どおり全体を返す (truncation 無し = 正しさ保持)
    assert_eq!(next_logical_token("漢字熟語", 0), "漢字熟語");
    assert_eq!(prev_logical_token("中学校生", 9), "中学校");
}

//! Matcher 評価 logic — `MatchCondition` が input context にマッチするかを判定。
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §3.4
//!
//! ## semantics
//!
//! - 同 `MatchBlock` 内の condition は **AND** (全 hit で match 成立)
//! - 複数 `MatchBlock` は TOML 順で **第一 hit** 採用 (caller 側で iterate)
//! - condition が 1 つも指定されていない (= 全 None / 空 array) 場合は無条件 match
//!
//! ## char_type 判定
//!
//! `prev_char_type` / `next_char_type` は 「直前 token の最後の文字」 / 「直後 token の最初の文字」
//! を [`classify_char`] で分類して比較する。 token 不在 (= 文頭 / 文末) や
//! 分類不能文字の場合は no match。

use crate::char_class::{classify_char, CharType};
use crate::scoring::candidate::WEIGHT_DEFAULT;
use crate::scoring::format::{Alternative, MatchBlock, MatchCondition};

/// matcher 評価時の周辺 context。
///
/// caller は現在の token 位置で前後 token を参照可能な構造を渡す。
/// 文頭は `prev_token = None`、 文末は `next_token = None`、 `next2_token` は
/// idx+2 token (= 「人気が無い」 で idx+2=「無」、 1 飛ばし参照用) を指す。
#[derive(Debug, Clone, Copy, Default)]
pub struct MatchContext<'a> {
    /// 直前 token surface (文頭は None)
    pub prev_token: Option<&'a str>,
    /// 直後 token surface (文末は None)
    pub next_token: Option<&'a str>,
    /// 直後の更に直後 (idx+2) の token surface (= 1 飛ばし参照用、 None で文末扱い)
    pub next2_token: Option<&'a str>,
}

impl<'a> MatchContext<'a> {
    /// 全条件 None の空 context (= 文単独 token)
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// `prev_token` だけ指定
    #[must_use]
    pub fn with_prev(prev: &'a str) -> Self {
        Self {
            prev_token: Some(prev),
            next_token: None,
            next2_token: None,
        }
    }

    /// `next_token` だけ指定
    #[must_use]
    pub fn with_next(next: &'a str) -> Self {
        Self {
            prev_token: None,
            next_token: Some(next),
            next2_token: None,
        }
    }

    /// 前後両方指定
    #[must_use]
    pub fn with_both(prev: &'a str, next: &'a str) -> Self {
        Self {
            prev_token: Some(prev),
            next_token: Some(next),
            next2_token: None,
        }
    }

    /// prev / next / next2 を全指定 (= 「人気が無い」 のような 3 token 連続を扱う用)
    #[must_use]
    pub fn with_all(prev: Option<&'a str>, next: Option<&'a str>, next2: Option<&'a str>) -> Self {
        Self {
            prev_token: prev,
            next_token: next,
            next2_token: next2,
        }
    }
}

/// literal 完全一致 condition (`prev_eq` / `prev_eq_any` / `next_eq` / `next_eq_any` /
/// `next_starts`) 1 つあたりの match_hits 加算値。
///
/// suffix / prefix list / char_type / 述語 ([`HIT_WEIGHT_BROAD`]) より厳密な指定として
/// 2 倍に評価する。 値を変える場合は CHANGELOG に明記すること。
pub const HIT_WEIGHT_LITERAL: u8 = 2;

/// 広めの condition (`prev_ends_any` / `next_starts_any` / `next2_starts_any` /
/// `prev_char_type` / `next_char_type` / `prev_month` / `next_digit`) 1 つあたりの
/// match_hits 加算値。
pub const HIT_WEIGHT_BROAD: u8 = 1;

impl MatchCondition {
    /// この condition が context にマッチするか判定 (AND semantics)。
    ///
    /// 全 condition が空 (= 全 None / 空 array) の場合は無条件 `true`。
    /// 1 つでも condition が指定されていて、 それが context に hit しない場合は `false`。
    #[must_use]
    pub fn matches_context(&self, ctx: &MatchContext<'_>) -> bool {
        self.context_hits(ctx).is_some()
    }

    /// この condition が context にマッチする場合、 hit した condition 数の
    /// **重み付き累積値** を返す (AND semantics、 miss は `None`)。
    ///
    /// 重み: literal 完全一致 = [`HIT_WEIGHT_LITERAL`]、 それ以外 = [`HIT_WEIGHT_BROAD`]。
    /// 全 condition が空 (= 無条件 match) は `Some(0)`。
    /// 累積値は `Score::match_hits` (lexicographic 第 3 軸) に載り、 同 band ・ 同 length
    /// 内で 「より厳密な条件で書いた block」 を勝たせる (condition 全 12 種 hit でも
    /// 最大 17 なので u8 で十分)。
    #[must_use]
    pub fn context_hits(&self, ctx: &MatchContext<'_>) -> Option<u8> {
        let mut hits: u8 = 0;

        // ─── prev_eq ────────────────────────────────────────────────────────
        if let Some(expected) = &self.prev_eq {
            match ctx.prev_token {
                Some(actual) if actual == expected => hits += HIT_WEIGHT_LITERAL,
                _ => return None,
            }
        }

        // ─── prev_eq_any ────────────────────────────────────────────────────
        if !self.prev_eq_any.is_empty() {
            let prev = ctx.prev_token?; // prev 無いのに list 指定 → no match
            if !self.prev_eq_any.iter().any(|s| s == prev) {
                return None;
            }
            hits += HIT_WEIGHT_LITERAL;
        }

        // ─── next_eq ────────────────────────────────────────────────────────
        if let Some(expected) = &self.next_eq {
            match ctx.next_token {
                Some(actual) if actual == expected => hits += HIT_WEIGHT_LITERAL,
                _ => return None,
            }
        }

        // ─── next_eq_any ────────────────────────────────────────────────────
        if !self.next_eq_any.is_empty() {
            let next = ctx.next_token?;
            if !self.next_eq_any.iter().any(|s| s == next) {
                return None;
            }
            hits += HIT_WEIGHT_LITERAL;
        }

        // ─── prev_ends_any (= prev_token surface ends_with any of) ─────────
        if !self.prev_ends_any.is_empty() {
            let prev = ctx.prev_token?;
            if !self
                .prev_ends_any
                .iter()
                .any(|s| prev.ends_with(s.as_str()))
            {
                return None;
            }
            hits += HIT_WEIGHT_BROAD;
        }

        // ─── next_starts (= next_token surface starts_with) ────────────────
        if let Some(prefix) = &self.next_starts {
            match ctx.next_token {
                Some(actual) if actual.starts_with(prefix.as_str()) => {
                    hits += HIT_WEIGHT_LITERAL;
                }
                _ => return None,
            }
        }

        // ─── next_starts_any (= next_token surface starts_with any of) ─────
        if !self.next_starts_any.is_empty() {
            let next = ctx.next_token?;
            if !self
                .next_starts_any
                .iter()
                .any(|s| next.starts_with(s.as_str()))
            {
                return None;
            }
            hits += HIT_WEIGHT_BROAD;
        }

        // ─── next2_starts_any (= next2_token surface starts_with any of) ───
        if !self.next2_starts_any.is_empty() {
            let next2 = ctx.next2_token?;
            if !self
                .next2_starts_any
                .iter()
                .any(|s| next2.starts_with(s.as_str()))
            {
                return None;
            }
            hits += HIT_WEIGHT_BROAD;
        }

        // ─── prev_char_type ─────────────────────────────────────────────────
        if let Some(expected_type) = self.prev_char_type {
            let last_char = ctx.prev_token.and_then(|s| s.chars().next_back());
            match last_char {
                Some(c) if classify_char(c) == Some(expected_type) => {
                    hits += HIT_WEIGHT_BROAD;
                }
                _ => return None,
            }
        }

        // ─── next_char_type ─────────────────────────────────────────────────
        if let Some(expected_type) = self.next_char_type {
            let first_char = ctx.next_token.and_then(|s| s.chars().next());
            match first_char {
                Some(c) if classify_char(c) == Some(expected_type) => {
                    hits += HIT_WEIGHT_BROAD;
                }
                _ => return None,
            }
        }

        // ─── prev_month (= prev_token ends_with 月名) ──────────────────────
        if self.prev_month {
            let ok = ctx.prev_token.is_some_and(ends_with_month);
            if !ok {
                return None;
            }
            hits += HIT_WEIGHT_BROAD;
        }

        // ─── next_digit (= next_token starts_with 半角/全角数字) ───────────
        if self.next_digit {
            let ok = ctx.next_token.is_some_and(starts_with_digit);
            if !ok {
                return None;
            }
            hits += HIT_WEIGHT_BROAD;
        }

        Some(hits)
    }
}

/// pseudo-token の走査上限 (文字数)。
///
/// 同一 char-class が延々続く入力 (例: 同じ漢字 ×10000) で `next_logical_token` /
/// `prev_logical_token` が run 全体を走査すると、 per-position 呼び出しと合わせて
/// **O(N²)** になり request amplification DoS の梃子になる。 走査をこの長さで
/// 打ち切って線形化する。
///
/// 正しさ: dict の match condition 値は [`crate::sanitize::MAX_DICT_VALUE_CHARS`] 以下
/// (sanitize で保証)。 これを **超える** 長さの token は如何なる `*_eq` / `*_eq_any`
/// とも一致せず、 `*_starts_any` は先頭から最大 condition 長、 `*_char_type` は
/// 境界 1 文字しか見ない。 よって上限 = `MAX_DICT_VALUE_CHARS + 1` で打ち切れば
/// 「condition より確実に長い」 ことが判別でき、 match 結果は一切変わらない。
const MAX_LOGICAL_TOKEN_CHARS: usize = crate::sanitize::MAX_DICT_VALUE_CHARS + 1;

/// 文字種が連続する範囲を 「pseudo-token」 として切り出す helper。
///
/// Smart engine の `DictBridgeProvider` / `KanjiProvider` が path 構築中に
/// MatchContext を build するために使う。 Lindera token segmentation を使わず
/// **文字種境界** (= 漢字 / ひらがな / カタカナ / 英数 / 記号) で切る軽量実装。
///
/// 走査は [`MAX_LOGICAL_TOKEN_CHARS`] 文字で打ち切る (O(N²) 回避、 match 結果不変)。
///
/// 日本語の助詞 / 助動詞 (= ひらがな連続)、 漢字熟語 (= 漢字連続)、 数字
/// (= 英数連続) は概ね同 char_type で連続するため、 token-level segmentation
/// と概ね一致する。 完全一致が要る用途 (= POS-aware tokenization) には不適。
///
/// # 例
///
/// 「上手から登場」 の pos 6 (= 「上手」 の直後) から `next_logical_token` →
/// `"から"` (= ひらがな連続、 「登」 漢字で切れる)。
#[must_use]
pub fn next_logical_token(input: &str, start: usize) -> &str {
    let tail = &input[start..];
    let mut first_class: Option<CharType> = None;
    let mut end = start;
    for (n_chars, (idx, c)) in tail.char_indices().enumerate() {
        if n_chars >= MAX_LOGICAL_TOKEN_CHARS {
            break; // 異常に長い run を打ち切り (O(N²) 回避、 match 結果は不変)
        }
        let class = classify_char(c);
        match (first_class, class) {
            (None, Some(cls)) => {
                first_class = Some(cls);
                end = start + idx + c.len_utf8();
            }
            (Some(fc), Some(cls)) if fc == cls => {
                end = start + idx + c.len_utf8();
            }
            _ => break,
        }
    }
    &input[start..end]
}

/// 「pseudo-token」 の **更にその次** を切り出す (= next_logical_token を 2 回適用)。
///
/// 「人気が無い」 で pos 0 (= 「人気」 後の文脈) → next = 「が」、 next2 = 「無い」 のような
/// 1 飛ばし lookup 用。 next_logical_token と同じ char-class 境界 logic。
#[must_use]
pub fn next2_logical_token(input: &str, start: usize) -> &str {
    let next1 = next_logical_token(input, start);
    let next1_end = start + next1.len();
    next_logical_token(input, next1_end)
}

/// 直前の pseudo-token を切り出す (= byte 位置 `end` の手前から **文字種が連続する範囲**)。
///
/// 「東方の上手」 の pos 9 (= 「上手」 の直前) から `prev_logical_token` → `"の"`
/// (= ひらがな 1 文字、 「方」 漢字で切れる)。 行末 (= end が input.len()) でも動作。
#[must_use]
pub fn prev_logical_token(input: &str, end: usize) -> &str {
    let head = &input[..end];
    let mut last_class: Option<CharType> = None;
    let mut start = end;
    for (n_chars, (idx, c)) in head.char_indices().rev().enumerate() {
        if n_chars >= MAX_LOGICAL_TOKEN_CHARS {
            break; // 異常に長い run を打ち切り (O(N²) 回避、 match 結果は不変)
        }
        let class = classify_char(c);
        match (last_class, class) {
            (None, Some(cls)) => {
                last_class = Some(cls);
                start = idx;
            }
            (Some(lc), Some(cls)) if lc == cls => {
                start = idx;
            }
            _ => break,
        }
    }
    &input[start..end]
}

/// 月名 (一月〜十二月、 1月〜12月、 全角数字含む) で終わるか。
///
/// `prev_month` matcher 専用の self-contained helper。
/// (旧 `crate::reading::context` の同名関数は alpha.15 の chunks 削除で消滅済、 現在は本実装が唯一。)
fn ends_with_month(s: &str) -> bool {
    const MONTHS: &[&str] = &[
        "一月",
        "二月",
        "三月",
        "四月",
        "五月",
        "六月",
        "七月",
        "八月",
        "九月",
        "十月",
        "十一月",
        "十二月",
        "1月",
        "2月",
        "3月",
        "4月",
        "5月",
        "6月",
        "7月",
        "8月",
        "9月",
        "10月",
        "11月",
        "12月",
        "１月",
        "２月",
        "３月",
        "４月",
        "５月",
        "６月",
        "７月",
        "８月",
        "９月",
    ];
    MONTHS.iter().any(|m| s.ends_with(m))
}

/// 半角・全角の数字で始まるか。
fn starts_with_digit(s: &str) -> bool {
    s.chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || ('０'..='９').contains(&c))
}

/// `(reading, weight, match_hits)` の組。 [`resolve_readings`] の返り値要素。
///
/// primary (= match block hit または default) は weight = [`WEIGHT_DEFAULT`]、
/// alt 候補は dict 指定 weight。 match_hits は hit した match block の重み付き
/// condition 数 ([`MatchCondition::context_hits`])、 default / alt は常に 0。
pub type ResolvedReading<'a> = (&'a str, u8, u8);

/// match block + default + alt 候補を context に対して解決し、 採用すべき
/// `(reading, weight, match_hits)` 列を返す。
///
/// Entry / `[[kanji]]` block 双方の reading 解決を 1 箇所に集約する
/// (= DictBridgeProvider の emit で entry / kanji / alt にコピペされていたロジック)。
///
/// 1. `matches` を TOML 順で評価し、 第一 hit の reading。 hit なしなら `default`。
///    → primary として weight [`WEIGHT_DEFAULT`] で先頭に置く。 match_hits には
///    hit block の重み付き condition 数を載せる (default は 0)。
/// 2. `alts` のうち condition が hit するものを dict 指定 weight で続けて列挙 (ADR-0004)。
///    alt の match_hits は **常に 0**: match_hits は lexicographic で weight より上位の
///    軸なので、 alt に載せると path 選択で default を逆転しうる
///    (= ADR-0004 の 「default が常に path に乗る」 不変条件が壊れる)。
#[must_use]
pub fn resolve_readings<'a>(
    matches: &'a [MatchBlock],
    default: &'a str,
    alts: &'a [Alternative],
    ctx: &MatchContext<'_>,
) -> Vec<ResolvedReading<'a>> {
    let (primary, primary_hits) = matches
        .iter()
        .find_map(|m| {
            m.condition
                .context_hits(ctx)
                .map(|hits| (m.reading.as_str(), hits))
        })
        .unwrap_or((default, 0));
    let mut out = vec![(primary, WEIGHT_DEFAULT, primary_hits)];
    for alt in alts {
        if alt.condition.matches_context(ctx) {
            out.push((alt.reading.as_str(), alt.weight, 0));
        }
    }
    out
}

#[cfg(test)]
mod tests {
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

    // ─── prev_char_type ──────────────────────────────────────────────────────

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
        let hit = resolve_readings(&matches, "ウワテ", &[], &MatchContext::with_next("だ"));
        assert_eq!(hit, vec![("ジョウズ", WEIGHT_DEFAULT, HIT_WEIGHT_LITERAL)]);
        // miss → default、 hits 0
        let miss = resolve_readings(&matches, "ウワテ", &[], &MatchContext::empty());
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
        let out = resolve_readings(&[], "ウワテ", &alts, &MatchContext::with_next("から"));
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
}

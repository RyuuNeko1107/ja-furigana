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
//! ## 文スコープ condition
//!
//! `input_contains_any` だけは隣接 token でなく **入力文全体** を見る (位置非依存の
//! 部分一致)。 「麻雀の話をしている文中の 清一色」 のように、 隣接語では届かない
//! 話題文脈で読みを切り替えるための条件 (ADR-0010)。 誤爆しやすいので一般語には使わない。
//!
//! ## char_type 判定
//!
//! `prev_char_type` / `next_char_type` は 「直前 token の最後の文字」 / 「直後 token の最初の文字」
//! を [`classify_char`] で分類して比較する。 token 不在 (= 文頭 / 文末) や
//! 分類不能文字の場合は no match。

use crate::char_class::{classify_char, CharType};
use crate::scoring::candidate::WEIGHT_DEFAULT;
use crate::scoring::format::{Alternative, MatchBlock, MatchCondition};

mod tokens;

pub use tokens::{next2_logical_token, next_logical_token, prev_logical_token};

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
    /// 解析中の **入力文全体** (= `input_contains_any` の判定対象、 None で no match)
    pub full_input: Option<&'a str>,
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
            full_input: None,
        }
    }

    /// `next_token` だけ指定
    #[must_use]
    pub fn with_next(next: &'a str) -> Self {
        Self {
            prev_token: None,
            next_token: Some(next),
            next2_token: None,
            full_input: None,
        }
    }

    /// 前後両方指定
    #[must_use]
    pub fn with_both(prev: &'a str, next: &'a str) -> Self {
        Self {
            prev_token: Some(prev),
            next_token: Some(next),
            next2_token: None,
            full_input: None,
        }
    }

    /// prev / next / next2 を全指定 (= 「人気が無い」 のような 3 token 連続を扱う用)
    #[must_use]
    pub fn with_all(prev: Option<&'a str>, next: Option<&'a str>, next2: Option<&'a str>) -> Self {
        Self {
            prev_token: prev,
            next_token: next,
            next2_token: next2,
            full_input: None,
        }
    }

    /// 入力文全体を付与した context を返す (= `input_contains_any` 判定を有効化)。
    #[must_use]
    pub fn with_full_input(mut self, input: &'a str) -> Self {
        self.full_input = Some(input);
        self
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
                // 踊り字 々 は漢字に数えない: 畳語 (時々 / 早々 / 人々) の後ろは新しい語の頭で、
                // 「漢字の後ろ = 熟語の続き = 音読み」 の規則が当たらない (帰宅早々雨 = う になっていた、 2026-09-27)
                Some('々') if expected_type == CharType::Kanji => return None,
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

        // ─── input_contains_any (= 入力文全体の部分一致、 文スコープ) ───────
        if !self.input_contains_any.is_empty() {
            let full = ctx.full_input?; // 文全体が渡っていない → no match
            if !self
                .input_contains_any
                .iter()
                .any(|s| full.contains(s.as_str()))
            {
                return None;
            }
            hits += HIT_WEIGHT_BROAD;
        }

        Some(hits)
    }
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
///
/// 候補生成の hot path で毎回 `Vec` を確保しないよう iterator で返す
/// (primary は即時評価、 alt の condition は iterate 時に評価)。
pub fn resolve_readings<'a, 'c>(
    matches: &'a [MatchBlock],
    default: &'a str,
    alts: &'a [Alternative],
    ctx: &'c MatchContext<'_>,
) -> impl Iterator<Item = ResolvedReading<'a>> + 'c
where
    'a: 'c,
{
    let (primary, primary_hits) = matches
        .iter()
        .find_map(|m| {
            m.condition
                .context_hits(ctx)
                .map(|hits| (m.reading.as_str(), hits))
        })
        .unwrap_or((default, 0));
    std::iter::once((primary, WEIGHT_DEFAULT, primary_hits)).chain(
        alts.iter()
            .filter(move |alt| alt.condition.matches_context(ctx))
            .map(|alt| (alt.reading.as_str(), alt.weight, 0)),
    )
}

#[cfg(test)]
mod tests;

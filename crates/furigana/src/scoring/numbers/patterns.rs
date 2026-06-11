//! 数字系 candidate の **regex 定義 + 構築** (matching logic は `mod.rs` 側)。
//!
//! - 静的 pattern (data 非依存): [`TIME_COLON_RE`] / [`TIME_JP_FULL_RE`] /
//!   [`DATE_KANJI_FULL_RE`] / [`DATE_KANJI_MD_RE`] / [`DIGIT_RE`]
//! - 動的 builder (rules data 依存、 [`super::NumberCandidateProvider::new`] で 1 回だけ
//!   compile): [`build_counter_regexes`] / [`build_scale_regex`] / [`build_si_unit_regex`]
//!
//! builder は空 rules で `None` を返す (= never-match pattern を作らない、
//! 旧 NumberChunker の 51 GB alloc 暴走の教訓)。

use crate::rules::{CounterMode, CountersData, ScalesData, UnitsData};
use once_cell::sync::Lazy;
use regex::{Captures, Regex};

// ─── 静的 regex (data 非依存) ────────────────────────────────────────────────

/// 数値パターン (符号付き、 カンマ・小数対応)。
pub(super) const NUM_PAT: &str =
    r"[+\-\u{2212}\u{FF0D}\u{FF0B}]?[0-9０-９]+(?:,[0-9０-９]{3})*(?:\.[0-9０-９]+)?";

/// 日付・月日 用の 「Arabic 1〜4 桁 OR 漢数字 1〜3 文字」 pattern。
const DATE_NUM_PAT: &str = r"(?:[0-9０-９]{1,4}|[一二三四五六七八九十〇零]{1,3})";

/// 漢数字 pattern (= 末尾再帰助数詞 「N 個目」 の漢数字版用)。 `kansuji_to_arabic` が
/// 解釈できる範囲 (一〜九十百千 + 〇零)。 「一個目」 「十二回目」 等を catch する。
const KANJI_NUM_PAT: &str = r"[一二三四五六七八九十百千〇零]{1,6}";

pub(super) static TIME_COLON_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([0-9０-９]{1,2})[:：]([0-9０-９]{2})(?:[:：]([0-9０-９]{2}))?")
        .expect("scoring TIME_COLON regex build failed")
});

pub(super) static TIME_JP_FULL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([0-9０-９]{1,2})時(?:([0-9０-９]{1,2})分)?(?:([0-9０-９]{1,2})秒)?")
        .expect("scoring TIME_JP regex build failed")
});

pub(super) static DATE_KANJI_FULL_RE: Lazy<Regex> = Lazy::new(|| {
    let pat = format!(r"({DATE_NUM_PAT})年({DATE_NUM_PAT})月({DATE_NUM_PAT})日");
    Regex::new(&pat).expect("scoring DATE_KANJI_FULL regex build failed")
});

pub(super) static DATE_KANJI_MD_RE: Lazy<Regex> = Lazy::new(|| {
    let pat = format!(r"({DATE_NUM_PAT})月({DATE_NUM_PAT})日");
    Regex::new(&pat).expect("scoring DATE_KANJI_MD regex build failed")
});

pub(super) static DIGIT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(NUM_PAT).expect("scoring DIGIT regex build failed"));

// ─── 動的 regex builders (data 依存) ─────────────────────────────────────────

/// 助数詞 list から 2 本の regex を構築:
/// - **arabic**: `(NUM)(base)(recursive)?` (= 算用 / 全角数字 + 助数詞、 recursive 任意)
/// - **kanji_recursive**: `(KANJI_NUM)(base)(recursive)` (= 漢数字 + 助数詞 + 「目」、 recursive **必須**)
///
/// recursive-mode の助数詞 (= 「目」) は base alternation に混ぜず trailing group にする。
/// こうしないと 「2 個目」 が 「2 個」 で止まり、 残った 「目」 が単漢字 fallback で
/// 「モク」 と誤読される (= 「ニコモク」)。group が match したら counter section で
/// `base + recursive` を合成して `euphonic_counter_read` の末尾再帰 (strip_suffix '目')
/// に委ねる。
///
/// 漢数字側は **recursive を必須** にする (= 「一個目」 のみ対象、 「一個」 「一日」 等の
/// bare 漢数字 + 助数詞 は従来通り Lindera / chunker に委ねる)。 こうしないと
/// 「一日中」 の 「一日」 が counter candidate 化して chunker 互換が崩れる
/// (= `tests::single_counter_kansuji_only_in_date_pattern`)。
pub(super) fn build_counter_regexes(counters: &CountersData) -> (Option<Regex>, Option<Regex>) {
    let mut base: Vec<String> = counters.simple.keys().cloned().collect();
    let mut recursive: Vec<String> = Vec::new();
    // 漢数字 bare 対応に opt-in した助数詞 (= `kanji_numeral = true`)。
    let mut kanji_optin = false;
    for (key, rule) in &counters.counter {
        if rule.mode == Some(CounterMode::Recursive) {
            recursive.push(key.clone());
        } else {
            base.push(key.clone());
        }
        if rule.kanji_numeral {
            kanji_optin = true;
        }
    }
    if base.is_empty() {
        return (None, None);
    }
    // 長い順に並べて alternation で longest-first match (= 「番目」 を 「番」 より先に)
    base.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    let base_alts: Vec<String> = base.iter().map(|s| regex::escape(s)).collect();
    let base_joined = base_alts.join("|");

    // 漢数字 regex は recursive 助数詞 (「目」) か opt-in 助数詞のどちらかが在れば構築する。
    // recursive group は optional 化し、 bare match (recursive 無し) の採否は matcher 側で
    // `kanji_numeral` フラグにより gate する (= 「一日中」 の 「一日」 等の誤 counter 化を防ぐ)。
    if recursive.is_empty() {
        let arabic = Regex::new(&format!(r"({NUM_PAT})({base_joined})"))
            .expect("scoring counter regex build failed");
        let kanji = if kanji_optin {
            Some(
                Regex::new(&format!(r"({KANJI_NUM_PAT})({base_joined})"))
                    .expect("scoring kanji counter regex build failed"),
            )
        } else {
            None
        };
        return (Some(arabic), kanji);
    }

    recursive.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    let rec_alts: Vec<String> = recursive.iter().map(|s| regex::escape(s)).collect();
    let rec_joined = rec_alts.join("|");

    let arabic = Regex::new(&format!(r"({NUM_PAT})({base_joined})({rec_joined})?"))
        .expect("scoring counter regex build failed");
    // recursive group を optional 化 (= 「一個目」 の recursive 形 + opt-in 助数詞の bare 形 両対応)。
    let kanji = Regex::new(&format!(r"({KANJI_NUM_PAT})({base_joined})({rec_joined})?"))
        .expect("scoring kanji counter regex build failed");
    (Some(arabic), Some(kanji))
}

/// 大数スケール (+ optional 末尾漢字 unit / counter) の regex を構築。 空 list なら `None`。
///
/// trailing 候補 (= 末尾の 1 字漢字 unit) は:
/// - units.toml の漢字 1 字 entries (= 円 / 度 等)
/// - counters の漢字 1 字 entries (= 個 / 歩 / 回 等) ★alpha.21 round 7 拡張
///
/// 「1 万歩 → イチマンポ」 「3 千個 → サンゼンコ」 のような scale + counter 連結を
/// scale section で 1 token 化するため、 counter キーも trailing pattern に含める。
pub(super) fn build_scale_regex(
    scales: &ScalesData,
    units: &UnitsData,
    counters: &CountersData,
) -> Option<Regex> {
    let kanjis: Vec<String> = scales.entries.iter().map(|e| e.kanji.clone()).collect();
    if kanjis.is_empty() {
        return None;
    }
    let mut sorted_scales = kanjis;
    sorted_scales.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    let scale_alts: Vec<String> = sorted_scales.iter().map(|s| regex::escape(s)).collect();

    let is_single_non_ascii_kanji = |s: &str| {
        s.chars().count() == 1 && s.chars().next().is_some_and(|c| !c.is_ascii_alphanumeric())
    };

    // unit + counter 両方から漢字 1 字 trailing を集めて merge (dedupe)。
    let mut trailing_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for u in units
        .entries
        .keys()
        .filter(|s| is_single_non_ascii_kanji(s))
    {
        trailing_set.insert(u.clone());
    }
    for c in counters
        .counter
        .keys()
        .filter(|s| is_single_non_ascii_kanji(s))
    {
        trailing_set.insert(c.clone());
    }
    let trailing: Vec<String> = trailing_set.iter().map(|s| regex::escape(s)).collect();

    let pat = if trailing.is_empty() {
        format!(r"({NUM_PAT})({})", scale_alts.join("|"))
    } else {
        format!(
            r"({NUM_PAT})({})({})?",
            scale_alts.join("|"),
            trailing.join("|")
        )
    };
    Some(Regex::new(&pat).expect("scoring scale regex build failed"))
}

/// SI 単位の regex を構築 (case-insensitive)。 空 list なら `None`。
pub(super) fn build_si_unit_regex(units: &UnitsData) -> Option<Regex> {
    let symbols: Vec<String> = units.entries.keys().cloned().collect();
    if symbols.is_empty() {
        return None;
    }
    let mut sorted = symbols;
    sorted.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    let alts: Vec<String> = sorted.iter().map(|s| regex::escape(s)).collect();
    let pat = format!(r"(?i)({NUM_PAT})({})", alts.join("|"));
    Some(Regex::new(&pat).expect("scoring si_unit regex build failed"))
}

/// `re` が `hay` の先頭から (start == 0) match した場合のみ Captures を返す。
pub(super) fn at_start<'h>(re: &Regex, hay: &'h str) -> Option<Captures<'h>> {
    re.captures(hay)
        .filter(|c| c.get(0).is_some_and(|m| m.start() == 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::RulesData;

    #[test]
    fn empty_counters_yield_none() {
        let rules = RulesData::default();
        let (arabic, kanji) = build_counter_regexes(&rules.counters);
        assert!(
            arabic.is_none(),
            "空 counters は None (never-match を作らない)"
        );
        assert!(kanji.is_none());
    }

    #[test]
    fn empty_scales_and_units_yield_none() {
        let rules = RulesData::default();
        assert!(build_scale_regex(&rules.scales, &rules.units, &rules.counters).is_none());
        assert!(build_si_unit_regex(&rules.units).is_none());
    }

    #[test]
    fn kanji_optin_builds_bare_kanji_regex_without_recursive() {
        // recursive 助数詞 (「目」) が無くても、 kanji_numeral opt-in 助数詞が在れば
        // 漢数字 regex が構築され bare match する。
        let toml_str = r#"
            [counter."匹"]
            default = "ヒキ"
            kanji_numeral = true
        "#;
        let counters: crate::rules::CountersData = toml::from_str(toml_str).unwrap();
        let (arabic, kanji) = build_counter_regexes(&counters);
        assert!(arabic.is_some());
        let kanji = kanji.expect("opt-in 助数詞で漢数字 regex が構築される");
        assert!(at_start(&kanji, "五匹").is_some(), "五匹 が bare match する");
    }

    #[test]
    fn non_optin_counter_yields_no_kanji_regex_without_recursive() {
        // opt-in も recursive も無ければ漢数字 regex は None (従来挙動)。
        let toml_str = r#"
            [counter."杯"]
            default = "ハイ"
        "#;
        let counters: crate::rules::CountersData = toml::from_str(toml_str).unwrap();
        let (_arabic, kanji) = build_counter_regexes(&counters);
        assert!(kanji.is_none(), "opt-in 無しは漢数字 regex を作らない");
    }

    #[test]
    fn at_start_rejects_mid_string_match() {
        let re = Regex::new(r"[0-9]+").unwrap();
        assert!(at_start(&re, "12x").is_some());
        assert!(
            at_start(&re, "x12").is_none(),
            "先頭以外の match は採用しない"
        );
    }
}

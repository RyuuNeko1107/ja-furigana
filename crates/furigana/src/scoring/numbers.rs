//! 数字 + 助数詞 / 漢数字 / 数字読み の Smart engine 統合 (C3)。
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §5.6
//!
//! ## 役割
//!
//! 既存 [`crate::chunks::NumberChunker::split`] の数値関連 logic を
//! [`CandidateProvider`] として再実装し、 Smart engine path に band [`BAND_SPECIAL`]
//! (= 950) candidate として乗せる。 dict 完全一致 (band 1000) には常に負け、
//! 漢字辞書 (band 100) / Lindera (band 50) には常勝。
//!
//! ## カバー範囲
//!
//! 入力 byte 位置 `pos` から始まる数字系 surface に対し、 以下の優先順を **band 950 候補** として
//! 並列に提案する (path 選択は Smart engine の DP に委ねる):
//!
//! 1. 和式日付 `YYYY年MM月DD日` / `MM月DD日`
//! 2. 和式時刻 `H時M分S秒` / `H時M分` / `H時`
//! 3. 時刻 `HH:MM(:SS)`
//! 4. 数値 + 大数スケール (+ 末尾漢字単位) 例: `3万円`
//! 5. 数値 + SI 単位 例: `100km`
//! 6. 数値 + 単一助数詞 例: `3本` / `1日` / `12月`
//! 7. 記号 1 文字 (`[symbols]` table の entry のみ)
//! 8. 素の数字 例: `12345` → `イチマンニセンサンビャクヨンジュウゴ`
//!
//! 同位置から複数候補が出た場合は、 path レベル (= 末端まで覆える 1 edge 候補) が
//! [`crate::scoring::engine::PathScore`] の `edge_count` 軸で勝つので、 longer match が
//! 自然に選ばれる (例: 「1月1日」 で date MD candidate (1 edge) が 「1月」+「1日」 (2 edges)
//! を上回る)。
//!
//! ## scope 外 (他 provider 担当)
//!
//! - URL / Email / 絵文字 → [`crate::scoring::special::ProtectTokenProvider`] (band 2000)
//! - アルファベット token → [`crate::scoring::special::AlphabetPassthroughProvider`]
//!   (lookup hit は band 1000、 miss は band 100)
//! - 単語 / 漢字辞書 → `DictBridgeProvider` (api.rs)
//! - 踊り字 「々」 → [`crate::scoring::odoriji::OdorijiProvider`]
//! - jukugo super-set check は **不要** (Smart engine DP が band 1000 dict entry を自然に優先)
//!
//! ## 注意
//!
//! - 既存 chunker と独立 implementation (= scoring/special.rs URL_RE 重複と同方針)。
//!   0.2.0+ で `crate::chunks` 削除と coordinated に整理予定。
//! - `numeric_phrases` (`二十歳=ハタチ` 等) は別 provider 化が望ましいが C3 scope 外。
//!   alpha.10〜rc1 で必要なら追加。

use crate::numbers::{
    euphonic_counter_read, kansuji_to_arabic, number_to_katakana, scale_reading, si_unit_reading,
    symbol_char_reading,
};
use crate::rules::{
    CounterMode, CountersData, DaysData, RulesData, ScalesData, SymbolsData, UnitsData,
};
use crate::scoring::candidate::{
    Candidate, CandidateProvider, Score, ScoringContext, BAND_SPECIAL,
};
use once_cell::sync::Lazy;
use regex::{Captures, Regex};

// ─── 静的 regex (data 非依存) ────────────────────────────────────────────────

/// 数値パターン (符号付き、 カンマ・小数対応)。
const NUM_PAT: &str =
    r"[+\-\u{2212}\u{FF0D}\u{FF0B}]?[0-9０-９]+(?:,[0-9０-９]{3})*(?:\.[0-9０-９]+)?";

/// 日付・月日 用の 「Arabic 1〜4 桁 OR 漢数字 1〜3 文字」 pattern。
const DATE_NUM_PAT: &str = r"(?:[0-9０-９]{1,4}|[一二三四五六七八九十〇零]{1,3})";

/// 漢数字 pattern (= 末尾再帰助数詞 「N 個目」 の漢数字版用)。 `kansuji_to_arabic` が
/// 解釈できる範囲 (一〜九十百千 + 〇零)。 「一個目」 「十二回目」 等を catch する。
const KANJI_NUM_PAT: &str = r"[一二三四五六七八九十百千〇零]{1,6}";

static TIME_COLON_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([0-9０-９]{1,2})[:：]([0-9０-９]{2})(?:[:：]([0-9０-９]{2}))?")
        .expect("scoring TIME_COLON regex build failed")
});

static TIME_JP_FULL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([0-9０-９]{1,2})時(?:([0-9０-９]{1,2})分)?(?:([0-9０-９]{1,2})秒)?")
        .expect("scoring TIME_JP regex build failed")
});

static DATE_KANJI_FULL_RE: Lazy<Regex> = Lazy::new(|| {
    let pat = format!(r"({DATE_NUM_PAT})年({DATE_NUM_PAT})月({DATE_NUM_PAT})日");
    Regex::new(&pat).expect("scoring DATE_KANJI_FULL regex build failed")
});

static DATE_KANJI_MD_RE: Lazy<Regex> = Lazy::new(|| {
    let pat = format!(r"({DATE_NUM_PAT})月({DATE_NUM_PAT})日");
    Regex::new(&pat).expect("scoring DATE_KANJI_MD regex build failed")
});

static DIGIT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(NUM_PAT).expect("scoring DIGIT regex build failed"));

// ─── 動的 regex builders (data 依存) ─────────────────────────────────────────

/// 助数詞 list から 2 本の regex を構築:
/// - **arabic**: `(NUM)(base)(recursive)?` (= 算用 / 全角数字 + 助数詞、 recursive 任意)
/// - **kanji_recursive**: `(KANJI_NUM)(base)(recursive)` (= 漢数字 + 助数詞 + 「目」、 recursive **必須**)
///
/// recursive-mode の助数詞 (= 「目」) は base alternation に混ぜず trailing group にする。
/// こうしないと 「2 個目」 が 「2 個」 で止まり、 残った 「目」 が単漢字 fallback で
/// 「モク」 と誤読される (= 「ニコモク」)。group が match したら section 6 で
/// `base + recursive` を合成して [`euphonic_counter_read`] の末尾再帰 (strip_suffix '目')
/// に委ねる。
///
/// 漢数字側は **recursive を必須** にする (= 「一個目」 のみ対象、 「一個」 「一日」 等の
/// bare 漢数字 + 助数詞 は従来通り Lindera / chunker に委ねる)。 こうしないと
/// 「一日中」 の 「一日」 が counter candidate 化して chunker 互換が崩れる
/// (= [`tests::single_counter_kansuji_only_in_date_pattern`])。
fn build_counter_regexes(counters: &CountersData) -> (Option<Regex>, Option<Regex>) {
    let mut base: Vec<String> = counters.simple.keys().cloned().collect();
    let mut recursive: Vec<String> = Vec::new();
    for (key, rule) in &counters.counter {
        if rule.mode == Some(CounterMode::Recursive) {
            recursive.push(key.clone());
        } else {
            base.push(key.clone());
        }
    }
    if base.is_empty() {
        return (None, None);
    }
    // 長い順に並べて alternation で longest-first match (= 「番目」 を 「番」 より先に)
    base.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    let base_alts: Vec<String> = base.iter().map(|s| regex::escape(s)).collect();
    let base_joined = base_alts.join("|");

    if recursive.is_empty() {
        let arabic = Regex::new(&format!(r"({NUM_PAT})({base_joined})"))
            .expect("scoring counter regex build failed");
        return (Some(arabic), None);
    }

    recursive.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    let rec_alts: Vec<String> = recursive.iter().map(|s| regex::escape(s)).collect();
    let rec_joined = rec_alts.join("|");

    let arabic = Regex::new(&format!(r"({NUM_PAT})({base_joined})({rec_joined})?"))
        .expect("scoring counter regex build failed");
    let kanji = Regex::new(&format!(r"({KANJI_NUM_PAT})({base_joined})({rec_joined})"))
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
/// section 4 で 1 token 化するため、 counter キーも trailing pattern に含める。
fn build_scale_regex(
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
fn build_si_unit_regex(units: &UnitsData) -> Option<Regex> {
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
fn at_start<'h>(re: &Regex, hay: &'h str) -> Option<Captures<'h>> {
    re.captures(hay)
        .filter(|c| c.get(0).is_some_and(|m| m.start() == 0))
}

// ─── NumberCandidateProvider ────────────────────────────────────────────────

/// 数値 + 助数詞 / 大数スケール / SI 単位 / 日付 / 時刻 / 記号 / 素の数字 を
/// band [`BAND_SPECIAL`] (950) candidate として供給する [`CandidateProvider`]。
///
/// 構築時に [`RulesData`] を clone して保持、 candidate 生成は `candidates_at(pos)` で
/// その位置から始まる候補を全提案する。 path 選択は Smart engine の DP に委ねる。
#[derive(Debug, Clone)]
pub struct NumberCandidateProvider {
    counters: CountersData,
    scales: ScalesData,
    units: UnitsData,
    symbols: SymbolsData,
    days: DaysData,
    /// `(NUM)(base)(recursive?)` pattern (算用 / 全角数字)。 counter / simple table が空なら `None`。
    counter_re: Option<Regex>,
    /// `(KANJI_NUM)(base)(recursive)` pattern (漢数字 + 末尾再帰 「目」 必須)。
    /// recursive counter が無い (= 「目」 未定義) なら `None`。
    counter_kanji_re: Option<Regex>,
    /// `(NUM)(scale)(unit?)` pattern。 scales 空なら `None`。
    scale_re: Option<Regex>,
    /// `(NUM)(si_unit)` pattern。 units 空なら `None`。
    si_unit_re: Option<Regex>,
}

impl NumberCandidateProvider {
    /// [`RulesData`] から regex を pre-compile して provider を構築する。
    ///
    /// rules が空 (= [`RulesData::default`]) でも安全 (regex 全 `None` で全 candidate 抑制)。
    #[must_use]
    pub fn new(rules: &RulesData) -> Self {
        let (counter_re, counter_kanji_re) = build_counter_regexes(&rules.counters);
        let scale_re = build_scale_regex(&rules.scales, &rules.units, &rules.counters);
        let si_unit_re = build_si_unit_regex(&rules.units);
        Self {
            counters: rules.counters.clone(),
            scales: rules.scales.clone(),
            units: rules.units.clone(),
            symbols: rules.symbols.clone(),
            days: rules.days.clone(),
            counter_re,
            counter_kanji_re,
            scale_re,
            si_unit_re,
        }
    }

    /// `(surface, reading)` から band [`BAND_SPECIAL`] candidate を 1 つ生成。
    fn make(&self, input: &str, pos: usize, m_end: usize, reading: String) -> Candidate {
        let surface = &input[pos..pos + m_end];
        let char_count = surface.chars().count();
        let length = u8::try_from(char_count).unwrap_or(u8::MAX);
        Candidate::new(
            surface.to_string(),
            reading,
            pos..pos + m_end,
            Score::new(BAND_SPECIAL, length, 0, 0),
        )
    }

    /// 数値 + 助数詞 を読みに変換 (= chunker の `read_counter` と同 logic、 「N日」 単独は期間扱い)。
    fn read_counter(&self, raw_num: &str, counter: &str) -> String {
        let normalized = kansuji_to_arabic(raw_num).unwrap_or_else(|| raw_num.to_string());
        let nk = number_to_katakana(&normalized);

        if counter == "日" {
            if let Some(rule) = self.counters.counter.get("日") {
                if let Some(default) = &rule.default {
                    return format!("{nk}{default}");
                }
            }
            return format!("{nk}ニチ");
        }
        euphonic_counter_read(&nk, counter, &normalized, &self.counters, &self.days)
    }

    /// 数値 + 助数詞 を読みに変換 (= 日付内、 「N日」 が days.toml 特殊読みを採用)。
    fn read_counter_in_date(&self, raw_num: &str, counter: &str) -> String {
        let normalized = kansuji_to_arabic(raw_num).unwrap_or_else(|| raw_num.to_string());
        let nk = number_to_katakana(&normalized);
        euphonic_counter_read(&nk, counter, &normalized, &self.counters, &self.days)
    }
}

/// 数値 + scale + 助数詞 (= 「1 万歩」 「3 千個」 等) の trailing counter から suffix
/// を引く (= last_digit に応じた連濁 / 促音化済の suffix string)。
///
/// `scale_reading` で 「イチマン」 等の本体を作った後、 末尾の counter 部分のみを
/// この helper で取得して append する用途。 SI units lookup で miss した時の
/// fallback 経路。
fn scale_trailing_counter_suffix(
    counter: &str,
    num: &str,
    counters: &CountersData,
) -> Option<String> {
    let rule = counters.counter.get(counter)?;
    let normalized = kansuji_to_arabic(num).unwrap_or_else(|| num.to_string());
    let last = crate::numbers::helpers::last_digit(&normalized);
    // last_digit が rule に match すればその suffix を、 無ければ default を採用
    for r in &rule.rules {
        if r.last_digit.contains(&last) {
            return Some(r.suffix.clone());
        }
    }
    rule.default.clone()
}

/// range marker (`〜` / `~` / `～`) が **数字 / 漢数字 / 全角数字 と隣接** しているか。
///
/// 「2〜3回」 「100〜200円」 のような range 用途では `prev` / `next` どちらかが
/// 数字なので 「から」 reading を採用する。 一方 「がんばれ〜」 「も〜むりすぎ」 のような
/// kana / 漢字 / 文末 context では range ではなく vowel extension / 強調用途なので
/// 「から」 は誤読 (= caller 側で空 reading に置換し、 読み上げず surface のみ消費)。
///
/// 判定: 直前 / 直後 (UTF-8 char 単位で 1 文字遡る or 進む) のいずれかが数字
/// (ASCII 0-9、 全角 ０-９、 漢数字 一〜十百千) なら range context。
fn range_marker_in_numeric_context(input: &str, pos: usize, ch: char) -> bool {
    let prev_is_digit = input[..pos]
        .chars()
        .next_back()
        .is_some_and(is_digit_like_char);
    let next_pos = pos + ch.len_utf8();
    let next_is_digit = input[next_pos..]
        .chars()
        .next()
        .is_some_and(is_digit_like_char);
    prev_is_digit || next_is_digit
}

/// 数字らしい char か (= ASCII 0-9 / 全角 0-9 / 漢数字 一〜十百千万億兆)。
fn is_digit_like_char(c: char) -> bool {
    matches!(c,
        '0'..='9' | '０'..='９' |
        '〇' | '零' |
        '一' | '二' | '三' | '四' | '五' |
        '六' | '七' | '八' | '九' | '十' |
        '百' | '千' | '万' | '億' | '兆'
    )
}

impl CandidateProvider for NumberCandidateProvider {
    fn candidates_at(&self, ctx: &ScoringContext, pos: usize) -> Vec<Candidate> {
        let input = ctx.input;
        let mut out: Vec<Candidate> = Vec::new();
        let rest = &input[pos..];
        if rest.is_empty() {
            return out;
        }

        // ─── 1. 和式日付 (full / MD) ─────────────────────────────────────────
        if let Some(caps) = at_start(&DATE_KANJI_FULL_RE, rest) {
            let m_end = caps.get(0).unwrap().end();
            let y = caps.get(1).unwrap().as_str();
            let mo = caps.get(2).unwrap().as_str();
            let d = caps.get(3).unwrap().as_str();
            let reading = format!(
                "{}{}{}",
                self.read_counter_in_date(y, "年"),
                self.read_counter_in_date(mo, "月"),
                self.read_counter_in_date(d, "日"),
            );
            out.push(self.make(input, pos, m_end, reading));
        } else if let Some(caps) = at_start(&DATE_KANJI_MD_RE, rest) {
            let m_end = caps.get(0).unwrap().end();
            let mo = caps.get(1).unwrap().as_str();
            let d = caps.get(2).unwrap().as_str();
            let reading = format!(
                "{}{}",
                self.read_counter_in_date(mo, "月"),
                self.read_counter_in_date(d, "日"),
            );
            out.push(self.make(input, pos, m_end, reading));
        }

        // ─── 2. 和式時刻 (H時M分S秒 / H時M分 / H時) ─────────────────────────
        if let Some(caps) = at_start(&TIME_JP_FULL_RE, rest) {
            let m_end = caps.get(0).unwrap().end();
            let h = caps.get(1).unwrap().as_str();
            let mo = caps.get(2).map(|m| m.as_str());
            let se = caps.get(3).map(|m| m.as_str());
            let mut reading = self.read_counter(h, "時");
            if let Some(m_str) = mo {
                reading.push_str(&self.read_counter(m_str, "分"));
            }
            if let Some(s_str) = se {
                reading.push_str(&self.read_counter(s_str, "秒"));
            }
            out.push(self.make(input, pos, m_end, reading));
        }

        // ─── 3. 時刻 HH:MM(:SS) ──────────────────────────────────────────────
        if let Some(caps) = at_start(&TIME_COLON_RE, rest) {
            let m_end = caps.get(0).unwrap().end();
            let h = caps.get(1).unwrap().as_str();
            let mo = caps.get(2).unwrap().as_str();
            let se = caps.get(3).map(|m| m.as_str());
            let mut reading = self.read_counter(h, "時");
            reading.push_str(&self.read_counter(mo, "分"));
            if let Some(s_str) = se {
                reading.push_str(&self.read_counter(s_str, "秒"));
            }
            out.push(self.make(input, pos, m_end, reading));
        }

        // ─── 4. 数値 + 大数スケール (+ 末尾漢字 unit) ────────────────────────
        if let Some(re) = &self.scale_re {
            if let Some(caps) = at_start(re, rest) {
                let m_end = caps.get(0).unwrap().end();
                let num = caps.get(1).unwrap().as_str();
                let scale = caps.get(2).unwrap().as_str();
                let trailing_unit = caps.get(3).map(|m| m.as_str());
                let mut reading = scale_reading(num, scale, &self.scales);
                if let Some(u) = trailing_unit {
                    if let Some(unit_kana) = self.units.lookup(u) {
                        reading.push_str(unit_kana);
                    } else if let Some(suffix) =
                        scale_trailing_counter_suffix(u, num, &self.counters)
                    {
                        // ★alpha.21 round 7: trailing unit が SI units に無いとき
                        // counter rules から suffix を append (= 「1 万歩」 → イチマン+ポ)。
                        reading.push_str(&suffix);
                    } else {
                        reading.push_str(u);
                    }
                }
                out.push(self.make(input, pos, m_end, reading));
            }
        }

        // ─── 5. 数値 + SI 単位 ───────────────────────────────────────────────
        if let Some(re) = &self.si_unit_re {
            if let Some(caps) = at_start(re, rest) {
                let m_end = caps.get(0).unwrap().end();
                let num = caps.get(1).unwrap().as_str();
                let unit = caps.get(2).unwrap().as_str();
                let reading = si_unit_reading(num, unit, &self.units);
                out.push(self.make(input, pos, m_end, reading));
            }
        }

        // ─── 6. 数値 + 単一助数詞 ────────────────────────────────────────────
        if let Some(re) = &self.counter_re {
            if let Some(caps) = at_start(re, rest) {
                let m_end = caps.get(0).unwrap().end();
                let num = caps.get(1).unwrap().as_str();
                let base = caps.get(2).unwrap().as_str();
                // group 3 = optional 末尾再帰助数詞 (= 「目」)。 match したら
                // 「個」 + 「目」 = 「個目」 を read_counter に渡し、 euphonic_counter_read
                // の strip_suffix('目') 再帰で 「ニコメ」 等を得る。
                let combined;
                let counter = if let Some(rec) = caps.get(3) {
                    combined = format!("{base}{}", rec.as_str());
                    combined.as_str()
                } else {
                    base
                };
                let reading = self.read_counter(num, counter);
                out.push(self.make(input, pos, m_end, reading));
            }
        }

        // ─── 6b. 漢数字 + 助数詞 + 末尾再帰 「目」 ────────────────────────────
        // 「一個目」 「十二回目」 等。 recursive 「目」 を必須にして、 bare 漢数字 +
        // 助数詞 (= 「一個」 「一日」) は従来通り Lindera / chunker に委ねる。
        if let Some(re) = &self.counter_kanji_re {
            if let Some(caps) = at_start(re, rest) {
                let m_end = caps.get(0).unwrap().end();
                let num = caps.get(1).unwrap().as_str();
                let base = caps.get(2).unwrap().as_str();
                let rec = caps.get(3).unwrap().as_str();
                let counter = format!("{base}{rec}");
                let reading = self.read_counter(num, &counter);
                out.push(self.make(input, pos, m_end, reading));
            }
        }

        // ─── 7. 記号 1 文字 ─────────────────────────────────────────────────
        if let Some(ch) = rest.chars().next() {
            if let Some(read) = symbol_char_reading(ch, &self.symbols) {
                // 〜 / ~ / ～ は **range marker と vowel extension の dual use**。
                // 数字 context (= 「2〜3」 「100〜200円」) なら従来通り 「から」 で concat、
                // kana / 漢字 / 文末 context (= 「がんばれ〜」 「も〜むりすぎ」) では range
                // ではなく長音・強調用途なので 「から」 は誤読。 空 reading で surface のみ
                // 消費し、 読み上げない (= 「がんばれ〜」 → 「がんばれ」)。
                let is_range_marker = matches!(ch, '〜' | '~' | '～');
                let final_read =
                    if is_range_marker && !range_marker_in_numeric_context(input, pos, ch) {
                        String::new()
                    } else {
                        read
                    };
                out.push(self.make(input, pos, ch.len_utf8(), final_read));
            }
        }

        // ─── 8. 素の数字 ────────────────────────────────────────────────────
        if let Some(m) = at_start(&DIGIT_RE, rest) {
            let m_end = m.get(0).unwrap().end();
            let num = m.get(0).unwrap().as_str();
            out.push(self.make(input, pos, m_end, number_to_katakana(num)));
        }

        out
    }
}

#[cfg(test)]
mod tests {
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
}

//! 数字 + 助数詞 / 漢数字 / 数字読み の Smart engine 統合 (C3)。
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §5.6
//!
//! ## 役割
//!
//! 数値関連 logic (旧 `chunks::NumberChunker::split`、 alpha.15 で削除済) を
//! [`CandidateProvider`] として再実装し、 Smart engine path に band [`BAND_SPECIAL`]
//! (= 950) candidate として乗せる。 dict 完全一致 (band 1000) には常に負け、
//! 漢字辞書 (band 100) / Lindera (band 50) には常勝。
//!
//! ## 内部構成
//!
//! - [`patterns`]: regex 定義 + rules data からの動的構築 (matching とは分離)
//! - 本 module: provider 本体 + 候補種別ごとの `try_*` matcher
//!
//! ## カバー範囲
//!
//! 入力 byte 位置 `pos` から始まる数字系 surface に対し、 以下の優先順を **band 950 候補** として
//! 並列に提案する (path 選択は Smart engine の DP に委ねる)。 適用順は
//! [`NumberCandidateProvider::candidates_at`] の `try_*` 呼び出し列が data として明示する:
//!
//! 0. 数詞慣用語句 `二十歳=ハタチ` / `明後日=アサッテ` 等 — [`NumberCandidateProvider::try_phrase`]
//!    (★0.2.0 残件の再統合。 「明後日」 等 数字以外の先頭もあるため numeric lead 判定より前に評価)
//! 1. 和式日付 `YYYY年MM月DD日` / `MM月DD日` — [`NumberCandidateProvider::try_date`]
//! 2. 和式時刻 `H時M分S秒` / `H時M分` / `H時` — [`NumberCandidateProvider::try_time_jp`]
//! 3. 時刻 `HH:MM(:SS)` — [`NumberCandidateProvider::try_time_colon`]
//! 4. 数値 + 大数スケール (+ 末尾漢字単位) 例: `3万円` — [`NumberCandidateProvider::try_scale`]
//! 5. 数値 + SI 単位 例: `100km` — [`NumberCandidateProvider::try_si_unit`]
//! 6. 数値 + 単一助数詞 例: `3本` / `1日` / `12月` — [`NumberCandidateProvider::try_counter`]
//!    (+ 漢数字 + 「目」 版 [`NumberCandidateProvider::try_counter_kanji`])
//! 7. 記号 1 文字 (`[symbols]` table の entry のみ) — [`NumberCandidateProvider::emit_symbol`]
//! 8. 素の数字 例: `12345` → `イチマンニセンサンビャクヨンジュウゴ` — [`NumberCandidateProvider::try_digit`]
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
//! - 単語 / 漢字辞書 → [`crate::scoring::dict_bridge::DictBridgeProvider`]
//! - 踊り字 「々」 → [`crate::scoring::odoriji::OdorijiProvider`]
//! - jukugo super-set check は **不要** (Smart engine DP が band 1000 dict entry を自然に優先)
//!
mod patterns;

#[cfg(test)]
mod tests;

use crate::numbers::{
    euphonic_counter_read, kansuji_to_arabic, number_to_katakana, scale_reading, si_unit_reading,
    symbol_char_reading,
};
use crate::rules::{CountersData, DaysData, RulesData, ScalesData, SymbolsData, UnitsData};
use crate::scoring::candidate::{
    Candidate, CandidateProvider, Score, ScoringContext, BAND_SPECIAL,
};
use patterns::{
    at_start, build_counter_regexes, build_scale_regex, build_si_unit_regex, DATE_KANJI_FULL_RE,
    DATE_KANJI_MD_RE, DIGIT_RE, TIME_COLON_RE, TIME_JP_FULL_RE,
};
use regex::Regex;

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
    /// 数詞慣用語句 (numeric_phrases.toml) の先頭 char bucket index。
    /// key = surface 先頭 char、 value = (surface, reading) を **surface 長降順** sort 済
    /// (= emit 順を deterministic にするため。 候補は全 emit、 採択は DP の length 軸)。
    phrase_index: std::collections::HashMap<char, Vec<(String, String)>>,
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

        // 数詞慣用語句を先頭 char で bucket 化 (= 非 hit 位置のコストを HashMap lookup
        // 1 回に抑える)。 HashMap iteration は順序不定なので、 deterministic 出力のため
        // bucket 内を surface 長降順 + 同長は辞書順で sort する。
        let mut phrase_index: std::collections::HashMap<char, Vec<(String, String)>> =
            std::collections::HashMap::new();
        for (surface, reading) in &rules.numeric_phrases.entries {
            if let Some(first) = surface.chars().next() {
                phrase_index
                    .entry(first)
                    .or_default()
                    .push((surface.clone(), reading.clone()));
            }
        }
        for bucket in phrase_index.values_mut() {
            bucket.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        }

        Self {
            counters: rules.counters.clone(),
            scales: rules.scales.clone(),
            units: rules.units.clone(),
            symbols: rules.symbols.clone(),
            days: rules.days.clone(),
            phrase_index,
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
            Score::new(BAND_SPECIAL, length, 0),
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

// ─── 候補種別ごとの matcher (適用順は candidates_at が所有) ──────────────────

impl NumberCandidateProvider {
    /// section 0: 数詞慣用語句 (二十歳=ハタチ / 明後日=アサッテ 等)。
    ///
    /// 「明後日」 のように数字以外の先頭を持つ語句もあるため、 caller は numeric lead
    /// 判定より **前に** 呼ぶこと。 同位置で複数 hit (= 「一人前」 と 「一人」) は
    /// 全部 emit し、 採択は DP の length / edge_count 軸に委ねる (長い方が勝つ)。
    /// dict 完全一致 (band 1000) には負ける = dict 側で個別 override 可能。
    fn try_phrase(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        first_char: char,
        out: &mut Vec<Candidate>,
    ) {
        let Some(bucket) = self.phrase_index.get(&first_char) else {
            return;
        };
        for (surface, reading) in bucket {
            if rest.starts_with(surface.as_str()) {
                out.push(self.make(input, pos, surface.len(), reading.clone()));
            }
        }
    }

    /// section 1: 和式日付 (full → MD の優先順、 full が match したら MD は試さない)。
    fn try_date(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<Candidate>) {
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
    }

    /// section 2: 和式時刻 (H時M分S秒 / H時M分 / H時)。
    fn try_time_jp(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<Candidate>) {
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
    }

    /// section 3: 時刻 HH:MM(:SS)。
    fn try_time_colon(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<Candidate>) {
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
    }

    /// section 4: 数値 + 大数スケール (+ 末尾漢字 unit)。
    fn try_scale(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<Candidate>) {
        let Some(re) = &self.scale_re else { return };
        if let Some(caps) = at_start(re, rest) {
            let m_end = caps.get(0).unwrap().end();
            let num = caps.get(1).unwrap().as_str();
            let scale = caps.get(2).unwrap().as_str();
            let trailing_unit = caps.get(3).map(|m| m.as_str());
            let mut reading = scale_reading(num, scale, &self.scales);
            if let Some(u) = trailing_unit {
                if let Some(unit_kana) = self.units.lookup(u) {
                    reading.push_str(unit_kana);
                } else if let Some(suffix) = scale_trailing_counter_suffix(u, num, &self.counters) {
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

    /// section 5: 数値 + SI 単位。
    fn try_si_unit(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<Candidate>) {
        let Some(re) = &self.si_unit_re else { return };
        if let Some(caps) = at_start(re, rest) {
            let m_end = caps.get(0).unwrap().end();
            let num = caps.get(1).unwrap().as_str();
            let unit = caps.get(2).unwrap().as_str();
            let reading = si_unit_reading(num, unit, &self.units);
            out.push(self.make(input, pos, m_end, reading));
        }
    }

    /// section 6: 数値 + 単一助数詞 (+ optional 末尾再帰 「目」)。
    fn try_counter(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<Candidate>) {
        let Some(re) = &self.counter_re else { return };
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

    /// section 6b: 漢数字 + 助数詞 (+ optional 末尾再帰 「目」)。
    ///
    /// - recursive 形 (= 「一個目」「十二回目」): group 3 が match。常に採用。
    /// - bare 形 (= 「五匹」「三羽」): group 3 不在。base 助数詞が `kanji_numeral = true` に
    ///   opt-in している時のみ採用 (= 「一日中」 の 「一日」 等の誤 counter 化を防ぐ)。
    ///   euphony は `read_counter` 内の `kansuji_to_arabic` + `euphonic_counter_read` が
    ///   担うので、 連濁 (三羽→さんば) / 促音 (六匹→ろっぴき) も自動で効く。
    fn try_counter_kanji(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<Candidate>) {
        let Some(re) = &self.counter_kanji_re else {
            return;
        };
        if let Some(caps) = at_start(re, rest) {
            let m_end = caps.get(0).unwrap().end();
            let num = caps.get(1).unwrap().as_str();
            let base = caps.get(2).unwrap().as_str();
            let counter = if let Some(rec) = caps.get(3) {
                // recursive 形 (「目」) は常に採用
                format!("{base}{}", rec.as_str())
            } else {
                // bare 形は opt-in 助数詞のみ採用
                let opted_in = self
                    .counters
                    .counter
                    .get(base)
                    .is_some_and(|r| r.kanji_numeral);
                if !opted_in {
                    return;
                }
                base.to_string()
            };
            let reading = self.read_counter(num, &counter);
            out.push(self.make(input, pos, m_end, reading));
        }
    }

    /// section 7: 記号 1 文字。 candidates_at から、 数値系を skip する非数値
    /// lead 経路と通常経路の双方から呼ぶ (= 記号判定は数値系の有無に依らず常に行う)。
    fn emit_symbol(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<Candidate>) {
        let Some(ch) = rest.chars().next() else {
            return;
        };
        if let Some(read) = symbol_char_reading(ch, &self.symbols) {
            // 〜 / ~ / ～ は **range marker と vowel extension の dual use**。
            // 数字 context (= 「2〜3」 「100〜200円」) なら従来通り 「から」 で concat、
            // kana / 漢字 / 文末 context (= 「がんばれ〜」 「も〜むりすぎ」) では range
            // ではなく長音・強調用途なので 「から」 は誤読。 空 reading で surface のみ
            // 消費し、 読み上げない (= 「がんばれ〜」 → 「がんばれ」)。
            let is_range_marker = matches!(ch, '〜' | '~' | '～');
            let final_read = if is_range_marker && !range_marker_in_numeric_context(input, pos, ch)
            {
                String::new()
            } else {
                read
            };
            out.push(self.make(input, pos, ch.len_utf8(), final_read));
        }
    }

    /// section 8: 素の数字。
    fn try_digit(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<Candidate>) {
        if let Some(m) = at_start(&DIGIT_RE, rest) {
            let m_end = m.get(0).unwrap().end();
            let num = m.get(0).unwrap().as_str();
            out.push(self.make(input, pos, m_end, number_to_katakana(num)));
        }
    }
}

impl CandidateProvider for NumberCandidateProvider {
    fn candidates_at(&self, ctx: &ScoringContext, pos: usize) -> Vec<Candidate> {
        let input = ctx.input;
        let mut out: Vec<Candidate> = Vec::new();
        let rest = &input[pos..];
        let Some(first_char) = rest.chars().next() else {
            return out;
        };

        // section 0: 数詞慣用語句。 「明後日」 等 数字以外の先頭もあるため
        // numeric lead 判定より前に評価する (非 hit 位置は HashMap lookup 1 回)。
        self.try_phrase(input, pos, rest, first_char, &mut out);

        // ─── 先頭文字 dispatch (hot path 最適化) ─────────────────────────────
        // 数値系正規表現は全て **数字系の先頭文字** を要求する (NUM_PAT = 任意符号 +
        // [0-9０-９]、 DATE/KANJI_NUM = 漢数字)。 先頭がそれ以外なら必ず空振りするので、
        // 記号系 (emit_symbol) だけ評価して即 return する。 これで全 byte 位置で 6+ regex
        // を試行する無駄を消す (dict prefix index 化で dict scan が消えた後の相対的
        // hot path)。 符号始まり (= 「-5本」) も拾うため digit-like に符号 5 種を加えた
        // 集合で判定。
        let numeric_lead = is_digit_like_char(first_char)
            || matches!(first_char, '+' | '-' | '\u{2212}' | '\u{FF0D}' | '\u{FF0B}');
        if !numeric_lead {
            self.emit_symbol(input, pos, rest, &mut out);
            return out;
        }

        // 適用順 = 優先順 (module doc の 1〜8 と対応)。
        self.try_date(input, pos, rest, &mut out);
        self.try_time_jp(input, pos, rest, &mut out);
        self.try_time_colon(input, pos, rest, &mut out);
        self.try_scale(input, pos, rest, &mut out);
        self.try_si_unit(input, pos, rest, &mut out);
        self.try_counter(input, pos, rest, &mut out);
        self.try_counter_kanji(input, pos, rest, &mut out);
        self.emit_symbol(input, pos, rest, &mut out);
        self.try_digit(input, pos, rest, &mut out);

        out
    }
}

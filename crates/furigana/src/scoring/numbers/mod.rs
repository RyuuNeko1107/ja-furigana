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
    euphonic_counter_read, kansuji_to_arabic, kansuji_to_arabic_date, number_to_katakana,
    scale_reading, si_unit_reading, symbol_char_reading,
};
use crate::rules::{CountersData, DaysData, RulesData, ScalesData, SymbolsData, UnitsData};
use crate::scoring::candidate::{
    CandidateProvider, RawCandidate, Score, ScoringContext, BAND_SPECIAL,
};
use crate::scoring::digit_reading::DigitStyle;
use crate::scoring::model_number::CompiledModels;
use patterns::{
    at_start, build_counter_regexes, build_grouped_kanji_counter_regex, build_scale_regex,
    build_si_unit_regex, HeadRegex, DATE_KANJI_FULL_RE, DATE_KANJI_MD_RE, DIGIT_RE, FRACTION_RE,
    KANJI_DECIMAL_RE, KANJI_DECIMAL_SHORT_RE, KANJI_DECIMAL_UNIT_RE, PHONE_RE, POSTAL_RE, RATIO_RE,
    SLASH_DATE_AFTER_RE, SLASH_DATE_BEFORE_RE, SLASH_FRACTION_AFTER_RE, SLASH_FRACTION_BEFORE_RE,
    SLASH_PAIR_RE, SLASH_RANGE_AFTER_RE, SLASH_RANGE_BEFORE_RE, SLASH_SENTENCE_DATE_RE,
    SLASH_SENTENCE_FRACTION_RE, TIME_COLON_RE, TIME_JP_FULL_RE, TIME_JP_KANJI_RE, VERSION_RE,
    YMD_RE, YM_RE,
};
use regex::Regex;

/// 辞書で足された `N/M` の手がかり語 (語の一覧を 1 本の正規表現にしたもの)。 空なら `None`
#[derive(Debug, Default, Clone)]
struct SlashCues {
    date_after: Option<Regex>,
    date_before: Option<Regex>,
    date_topic: Option<Regex>,
    fraction_after: Option<Regex>,
    fraction_before: Option<Regex>,
    fraction_topic: Option<Regex>,
}

impl SlashCues {
    fn new(cues: &crate::rules::SlashPairCues) -> Self {
        // 語は正規表現として解釈しない (escape)。 直後は先頭一致、 直前は末尾一致、 話題は部分一致
        let build = |words: &[String], head: &str, tail: &str| {
            let mut words: Vec<&str> = words
                .iter()
                .map(|w| w.trim())
                .filter(|w| !w.is_empty())
                .collect();
            if words.is_empty() {
                return None;
            }
            words.sort_by_key(|w| std::cmp::Reverse(w.len()));
            let alt: Vec<String> = words.iter().map(|w| regex::escape(w)).collect();
            Regex::new(&format!("{head}(?:{}){tail}", alt.join("|"))).ok()
        };
        Self {
            date_after: build(&cues.date_after, "^", ""),
            date_before: build(&cues.date_before, "", "[:：は ]?[ \\u3000]?$"),
            date_topic: build(&cues.date_topic, "", ""),
            fraction_after: build(&cues.fraction_after, "^", ""),
            fraction_before: build(&cues.fraction_before, "", "[ \\u3000]?$"),
            fraction_topic: build(&cues.fraction_topic, "", ""),
        }
    }
}

fn extra_match(re: &Option<Regex>, text: &str) -> bool {
    re.as_ref().is_some_and(|r| r.is_match(text))
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
    /// 数詞慣用語句 (numeric_phrases.toml) の先頭 char bucket index。
    /// key = surface 先頭 char、 value = (surface, reading) を **surface 長降順** sort 済
    /// (= emit 順を deterministic にするため。 候補は全 emit、 採択は DP の length 軸)。
    phrase_index: std::collections::HashMap<char, Vec<(String, String)>>,
    /// `(NUM)(base)(recursive?)` pattern (算用 / 全角数字)。 counter / simple table が空なら `None`。
    counter_re: Option<HeadRegex>,
    /// `(KANJI_NUM)(base)(recursive)` pattern (漢数字 + 末尾再帰 「目」 必須)。
    /// recursive counter が無い (= 「目」 未定義) なら `None`。
    counter_kanji_re: Option<HeadRegex>,
    /// 読点区切りの漢数字 + 助数詞 (`一、〇〇〇円`)。 counter が空なら `None`
    counter_kanji_grouped_re: Option<HeadRegex>,
    /// `(NUM)(scale)(unit?)` pattern。 scales 空なら `None`。
    scale_re: Option<HeadRegex>,
    /// `(NUM)(si_unit)` pattern。 units 空なら `None`。
    si_unit_re: Option<HeadRegex>,
    /// SI 単位記号 (小文字化済) の集合。 構築時に 1 度だけ作り、 解析毎の
    /// 再構築 (= units 全 entry の `to_ascii_lowercase` + `HashSet` alloc) を避ける。
    unit_symbols: std::sync::Arc<std::collections::HashSet<String>>,
    /// `/` を含む単位記号 (km/h 等)。 数字なしの単独表記を読むために長い順に持つ
    slash_units: Vec<String>,
    /// 型番パターン (models.toml)
    models: CompiledModels,
    /// 辞書 (context_cues.toml) で足された `N/M` の手がかり語
    slash_cues: SlashCues,
    /// 絵文字の読み (emoji.toml)。 キーは 完全一致用の原形と、 VS16 / 肌色を除いた形の両方
    emoji: std::sync::Arc<std::collections::HashMap<String, crate::rules::emoji::EmojiEntry>>,
    /// 伏せ字の図形記号の直後に付く敬称・肩書き (組み込み + 辞書)
    honorifics: Vec<String>,
}

impl NumberCandidateProvider {
    /// [`RulesData`] から regex を pre-compile して provider を構築する。
    ///
    /// rules が空 (= [`RulesData::default`]) でも安全 (regex 全 `None` で全 candidate 抑制)。
    #[must_use]
    pub fn new(rules: &RulesData) -> Self {
        let (counter_re, counter_kanji_re) = build_counter_regexes(&rules.counters);
        let counter_kanji_grouped_re = build_grouped_kanji_counter_regex(&rules.counters);
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

        let unit_symbols: std::collections::HashSet<String> = rules
            .units
            .entries
            .keys()
            .map(|k| k.to_ascii_lowercase())
            .collect();

        let mut slash_units: Vec<String> = rules
            .units
            .entries
            .keys()
            .filter(|k| k.contains('/'))
            .cloned()
            .collect();
        slash_units.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));

        Self {
            counters: rules.counters.clone(),
            scales: rules.scales.clone(),
            units: rules.units.clone(),
            symbols: rules.symbols.clone(),
            days: rules.days.clone(),
            phrase_index,
            counter_re,
            counter_kanji_re,
            counter_kanji_grouped_re,
            scale_re,
            si_unit_re,
            unit_symbols: std::sync::Arc::new(unit_symbols),
            slash_units,
            models: CompiledModels::new(&rules.models),
            slash_cues: SlashCues::new(&rules.context_cues.slash_pair),
            emoji: std::sync::Arc::new(build_emoji_table(&rules.emoji)),
            honorifics: BUILTIN_HONORIFICS
                .iter()
                .map(ToString::to_string)
                .chain(
                    rules
                        .context_cues
                        .placeholder
                        .honorifics
                        .iter()
                        .map(|w| w.trim().to_string())
                        .filter(|w| !w.is_empty()),
                )
                .collect(),
        }
    }

    /// `(surface, reading)` から band [`BAND_SPECIAL`] candidate を 1 つ生成。
    /// SI 単位記号の集合 (小文字化済)。 英数 passthrough 側が
    /// 「数字 + 単位」 span を skip するために使う。
    #[must_use]
    pub fn unit_symbols(&self) -> std::sync::Arc<std::collections::HashSet<String>> {
        std::sync::Arc::clone(&self.unit_symbols)
    }

    /// 絵文字の読み表 (保護 token の読みに使う)
    #[must_use]
    pub(crate) fn emoji_table(
        &self,
    ) -> std::sync::Arc<std::collections::HashMap<String, crate::rules::emoji::EmojiEntry>> {
        std::sync::Arc::clone(&self.emoji)
    }

    fn make(
        &self,
        input: &str,
        pos: usize,
        m_end: usize,
        reading: String,
    ) -> RawCandidate<'static> {
        self.make_with_hits(input, pos, m_end, reading, 0)
    }

    /// 助数詞 / 日付 / 時刻 / 単位 のように **rule が形を当てた** 候補。
    ///
    /// `match_hits = 1` を付けて、 band と edge 数が並ぶ時に勝てるようにする。
    /// 「頭3つね」 は Lindera が 「つね」 を 1 語にするため 3つ + ね と 3 + つね が
    /// どちらも 2 edge / 最弱 band 50 で並び、 hits が無いと列挙順で さんつね になる。
    /// 素の数字 (try_digit) と記号は 0 のまま (= 形を当てていない)。
    fn make_hit(
        &self,
        input: &str,
        pos: usize,
        m_end: usize,
        reading: String,
    ) -> RawCandidate<'static> {
        self.make_with_hits(input, pos, m_end, reading, 1)
    }

    fn make_with_hits(
        &self,
        input: &str,
        pos: usize,
        m_end: usize,
        reading: String,
        hits: u8,
    ) -> RawCandidate<'static> {
        let surface = &input[pos..pos + m_end];
        let char_count = surface.chars().count();
        let length = u8::try_from(char_count).unwrap_or(u8::MAX);
        RawCandidate::new(
            reading,
            pos..pos + m_end,
            Score::new(BAND_SPECIAL, length, hits),
        )
    }

    /// 数値 + 助数詞 を読みに変換 (= chunker の `read_counter` と同 logic、 「N日」 単独は期間扱い)。
    fn read_counter(&self, raw_num: &str, counter: &str) -> String {
        let normalized = kansuji_to_arabic(raw_num).unwrap_or_else(|| raw_num.to_string());
        // 月・日は 09月 / 05日 のようにゼロ埋めで書かれる (くがつ / いつか)。 ほかの助数詞 (007号 など) は従来どおり
        let normalized = if matches!(counter, "月" | "日") {
            trim_leading_zeros(&normalized)
        } else {
            normalized
        };
        let nk = number_to_katakana(&normalized);

        if counter == "日" {
            if let Some(rule) = self.counters.counter.get("日") {
                // ★2026-09-11: 単独 「N 日」 は暦の日付ではないので days.toml を
                // 使わないが、 counter 側に書かれた specials は尊重する。
                // 「2 日」 は期間表現でも フツカ であり、 ニニチ とは読まない。
                if let Some(special) = rule.specials.get(&normalized) {
                    return special.clone();
                }
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
        let normalized = kansuji_to_arabic_date(raw_num).unwrap_or_else(|| raw_num.to_string());
        // 先頭のゼロは読まない (2026/09/05 の 09 → 9月 = くがつ、 05 → 5日 = いつか)
        let normalized = trim_leading_zeros(&normalized);
        let nk = number_to_katakana(&normalized);
        euphonic_counter_read(&nk, counter, &normalized, &self.counters, &self.days)
    }
}

/// 先頭のゼロを落とす (`09` → `9`)。 全部ゼロ (`00`) ならそのまま
fn trim_leading_zeros(s: &str) -> String {
    match s.trim_start_matches('0') {
        "" => s.to_string(),
        trimmed => trimmed.to_string(),
    }
}

/// 読み上げない終端記号 (= 記号候補に巻き取ってよい文字)。
/// `2026.1.16` のような 西暦 4 桁 + 月 + 日 のドット区切り。 バージョンとも日付とも取れるので
/// バージョン規則の対象にしない (従来どおり)。
fn looks_like_dotted_date(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    let [y, m, d] = parts[..] else {
        return false;
    };
    let num = |p: &str| {
        p.chars()
            .filter_map(|c| c.to_digit(10))
            .fold(0u32, |a, d| a * 10 + d)
    };
    y.chars().count() == 4
        && (1..=2).contains(&m.chars().count())
        && (1..=2).contains(&d.chars().count())
        && (1..=12).contains(&num(m))
        && (1..=31).contains(&num(d))
}

fn is_trailing_punctuation(c: char) -> bool {
    matches!(c, '。' | '、' | '！' | '？' | '…' | '!' | '?' | ',' | '.')
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
    scale: &str,
    counters: &CountersData,
) -> Option<String> {
    let rule = counters.counter.get(counter)?;
    let normalized = kansuji_to_arabic(num).unwrap_or_else(|| num.to_string());
    // 数の読みが 万 / 千 (まん / せん) の「ん」で終わる時は 3 と同じ連濁にする (5万本 = ごまんぼん / 10万匹 = じゅうまんびき)。
    // 億 以上は直前の数字で決める (従来どおり)
    let last = if matches!(scale, "万" | "千") {
        3
    } else {
        crate::numbers::helpers::last_digit(&normalized)
    };
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
    prev_is_digit || next_is_digit || between_single_capitals(input, pos, ch)
}

/// `A〜Z` のように、 単独の英大文字ではさまれているか (Hello〜World は対象外)
fn between_single_capitals(input: &str, pos: usize, ch: char) -> bool {
    let mut before = input[..pos].chars().rev();
    let mut after = input[pos + ch.len_utf8()..].chars();
    let (p1, p2) = (before.next(), before.next());
    let (n1, n2) = (after.next(), after.next());
    p1.is_some_and(|c| c.is_ascii_uppercase())
        && n1.is_some_and(|c| c.is_ascii_uppercase())
        && !p2.is_some_and(|c| c.is_ascii_alphabetic())
        && !n2.is_some_and(|c| c.is_ascii_alphabetic())
}

/// `9時-17時` の `-`: 時 / 分 / 秒の直後で、 数字が続く時は範囲の区切り (から)
fn hyphen_is_time_range(input: &str, pos: usize, ch: char) -> bool {
    input[..pos]
        .chars()
        .next_back()
        .is_some_and(|c| matches!(c, '時' | '分' | '秒'))
        && input[pos + ch.len_utf8()..]
            .chars()
            .next()
            .is_some_and(is_digit_like_char)
}

/// `1534年-1582年` / `872年頃-945年頃` / `20歳-30歳` の `-`: 年・歳 (と、 年のあとの 頃) の直後で、 数字が続き、 その数字の直後が同じ単位の時は
/// 範囲の区切り (から)。 両側に同じ単位が揃う時だけ (`9日-17` のような片側だけの形は従来どおり読まない)
fn hyphen_is_unit_range(input: &str, pos: usize, ch: char) -> bool {
    let before = &input[..pos];
    let unit = match before.chars().next_back() {
        Some(c @ ('年' | '歳')) => c,
        Some('頃') if before.trim_end_matches('頃').ends_with('年') => '年',
        _ => return false,
    };
    let after = &input[pos + ch.len_utf8()..];
    let digits = after.chars().take_while(|&c| is_digit_like_char(c)).count();
    digits > 0 && after.chars().nth(digits).is_some_and(|c| c == unit)
}

fn digit_value(c: char) -> Option<u32> {
    match c {
        '0'..='9' => Some(u32::from(c) - u32::from('0')),
        '\u{FF10}'..='\u{FF19}' => Some(u32::from(c) - u32::from('\u{FF10}')),
        _ => None,
    }
}

/// 文字列の末尾の数字 (最大 4 桁) の値と、 そのバイト長
fn trailing_number(s: &str) -> Option<(u32, usize)> {
    let mut value = 0;
    let mut place = 1;
    let mut bytes = 0;
    let mut count = 0;
    for c in s.chars().rev() {
        let Some(d) = digit_value(c) else { break };
        if count == 4 {
            return None;
        }
        value += d * place;
        place *= 10;
        bytes += c.len_utf8();
        count += 1;
    }
    (count > 0).then_some((value, bytes))
}

/// 文字列の先頭の数字 (最大 4 桁) の値と、 そのバイト長
fn leading_number(s: &str) -> Option<(u32, usize)> {
    let mut value = 0;
    let mut bytes = 0;
    let mut count = 0;
    for c in s.chars() {
        let Some(d) = digit_value(c) else { break };
        if count == 4 {
            return None;
        }
        value = value * 10 + d;
        bytes += c.len_utf8();
        count += 1;
    }
    (count > 0).then_some((value, bytes))
}

/// `10/31-11/1` / `9/26-27` / `10月31日-11月1日` の `-`: 日付 (月/日 か N日) の直後で、 後ろも日付 (月/日・N月・同じ月の日) の時は
/// 範囲の区切り (から)。 月は 1〜12、 日は 1〜31 の時だけ (分数や点数と区別する)。 `〜` と同じ扱い
fn hyphen_is_date_range(input: &str, pos: usize, ch: char) -> bool {
    let before = &input[..pos];
    let after = &input[pos + ch.len_utf8()..];
    // 前: `N日` か `M/D` (年/月/日 の前に 4 桁の年が付いてもよい)
    let mut slash_day = None;
    if let Some(rest) = before.strip_suffix('\u{65E5}') {
        match trailing_number(rest) {
            Some((d, _)) if (1..=31).contains(&d) => {}
            _ => return false,
        }
    } else {
        let Some((day, day_len)) = trailing_number(before) else {
            return false;
        };
        let Some(rest) = before[..before.len() - day_len].strip_suffix('/') else {
            return false;
        };
        let Some((month, month_len)) = trailing_number(rest) else {
            return false;
        };
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return false;
        }
        let head = &rest[..rest.len() - month_len];
        let head_ok = match head.chars().next_back() {
            None => true,
            Some('/') => trailing_number(&head[..head.len() - 1]).is_some_and(|(_, n)| n == 4),
            Some(c) => digit_value(c).is_none(),
        };
        if !head_ok {
            return false;
        }
        slash_day = Some(day);
    }
    let Some((first, first_len)) = leading_number(after) else {
        return false;
    };
    let tail = &after[first_len..];
    // 後ろ: `N月` / `M/D` / (前が M/D の時だけ) 同じ月の日 D
    if tail.starts_with('\u{6708}') {
        return (1..=12).contains(&first);
    }
    if let Some(rest) = tail.strip_prefix('/') {
        let Some((day, day_len)) = leading_number(rest) else {
            return false;
        };
        let next = rest[day_len..].chars().next();
        return (1..=12).contains(&first)
            && (1..=31).contains(&day)
            && !next.is_some_and(|c| c == '/' || digit_value(c).is_some());
    }
    match slash_day {
        Some(from) => {
            (from < first && first <= 31)
                && !tail
                    .chars()
                    .next()
                    .is_some_and(|c| matches!(c, ':' | '.' | '%' | '\u{FF1A}'))
        }
        None => false,
    }
}

/// 装飾にも使われる図形記号。 伏せ字の書き方 (`■■さん` / `△△`) の時だけ読む
const DECORATIVE_SHAPES: &str = "●■▲▼◆◇";

/// 郵便番号の 3 桁 + 4 桁を 1 桁ずつ読む (間に ノ)
fn postal_digits(head: &str, tail: &str) -> String {
    format!(
        "{}ノ{}",
        DigitStyle::Each.render(head).unwrap_or_default(),
        DigitStyle::Each.render(tail).unwrap_or_default()
    )
}

/// 伏せ字の直後に付く敬称・肩書きの組み込み分 (辞書の `[placeholder] honorifics` で足せる)
const BUILTIN_HONORIFICS: [&str; 8] = ["さん", "くん", "君", "ちゃん", "様", "氏", "先生", "殿"];

/// 図形記号が伏せ字として使われているか: ちょうど 2 つ並ぶ (`■■`) か、 直後が敬称 (`●さん`)。
/// 1 つだけ / 3 つ以上並ぶ装飾 (`★★★お知らせ★★★`) は伏せ字ではない
fn is_placeholder_shape(input: &str, pos: usize, ch: char, honorifics: &[String]) -> bool {
    let before = input[..pos].chars().rev().take_while(|&c| c == ch).count();
    let after_str = &input[pos + ch.len_utf8()..];
    let after = after_str.chars().take_while(|&c| c == ch).count();
    if before + 1 + after == 2 {
        return true;
    }
    let tail = after_str.trim_start_matches(ch);
    honorifics.iter().any(|h| tail.starts_with(h.as_str()))
}

/// `×` の読み。 数字・英字・括弧の間 (`2×3` / `A×B` / `4K×2` / `えび×3`) は掛け算の かける、
/// それ以外 (`返事は×です` / `×印` / `○×クイズ`) は誤りを表す ばつ
fn times_sign_reading(input: &str, pos: usize, ch: char) -> &'static str {
    let prev = input[..pos].chars().next_back();
    let next = input[pos + ch.len_utf8()..].chars().next();
    let is_operand =
        |c: char| is_digit_like_char(c) || c.is_ascii_alphabetic() || matches!(c, '(' | '（');
    // 空白で挟んだ `A × B` (#杉浦太陽 × #松井玲奈 のようなコラボ表記) は かける。 両側に語があるときだけ
    let spaced = |c: Option<char>| c.is_some_and(|c| c == ' ' || c == '\u{3000}');
    let before_str = input[..pos].trim_end_matches([' ', '\u{3000}']);
    let after_str = input[pos + ch.len_utf8()..].trim_start_matches([' ', '\u{3000}']);
    if spaced(prev) && spaced(next) && !before_str.is_empty() && !after_str.is_empty() {
        return "カケル";
    }
    // 語どうしの `A×B` (ファイターズ×イチモニ / 高橋×佐藤 / A×B) = コラボ・組み合わせの かける。
    // 両側が 漢字・カタカナ・英数字。 `○×クイズ` や `返事は×です` (片側が記号・ひらがな) は ばつ
    let wordish = |c: Option<char>| {
        c.is_some_and(|c| {
            crate::char_class::is_kanji_char(c)
                || (crate::char_class::is_katakana_loose_char(c) && c != 'ー')
                || c.is_ascii_alphanumeric()
        })
    };
    if wordish(prev) && wordish(next) {
        return "カケル";
    }
    if next.is_some_and(is_operand) || prev.is_some_and(is_digit_like_char) {
        "カケル"
    } else {
        "バツ"
    }
}

/// `9:00-17:00` の `-`: 時刻 (H:MM) と時刻の間は範囲の区切り (から)
fn hyphen_is_clock_range(input: &str, pos: usize, ch: char) -> bool {
    let before = &input[..pos];
    let after = &input[pos + ch.len_utf8()..];
    let ends_with_clock = {
        let b = before.as_bytes();
        let n = b.len();
        n >= 4
            && b[n - 1].is_ascii_digit()
            && b[n - 2].is_ascii_digit()
            && b[n - 3] == b':'
            && b[n - 4].is_ascii_digit()
    };
    let a = after.as_bytes();
    let starts_with_clock = a.len() >= 4
        && a[0].is_ascii_digit()
        && (a[1] == b':' && a[2].is_ascii_digit() && a[3].is_ascii_digit()
            || a[1].is_ascii_digit() && a[2] == b':' && a[3].is_ascii_digit());
    ends_with_clock && starts_with_clock
}

/// `2024-2025` の `-`: 西暦 (1900〜2100) の 4 桁どうしで後ろが大きい時は範囲の区切り (から)
fn hyphen_is_year_range(input: &str, pos: usize, ch: char) -> bool {
    let digits_before: String = input[..pos]
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let after = &input[pos + ch.len_utf8()..];
    let digits_after: String = after.chars().take_while(char::is_ascii_digit).collect();
    if digits_before.len() != 4 || digits_after.len() != 4 {
        return false;
    }
    // 前後にさらに数字や英字 (電話番号・型番・ISBN) が続く形は範囲ではない
    let outer_before = input[..pos - 4].chars().next_back();
    let outer_after = after[4..].chars().next();
    if outer_before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
        || outer_after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return false;
    }
    match (digits_before.parse::<u32>(), digits_after.parse::<u32>()) {
        (Ok(x), Ok(y)) => (1900..=2100).contains(&x) && (1900..=2100).contains(&y) && x < y,
        _ => false,
    }
}

/// 区切り (`〜` / `~` / `～`) が `N〜M<助数詞>` の形で、 N / M がともに 1 桁で M = N + 1 か (2〜3時間 / 3~4人)。
/// 話し言葉では 「にさんじかん」 のように詰めるので、 区切りの 「から」 を読まない時に使う。
/// 2 桁以上 (10〜20人) や離れた数 (2〜5回) は 「から」 を残す
fn range_is_adjacent_small(input: &str, pos: usize, ch: char) -> bool {
    const COUNTERS: &str = "人日個回名枚本冊台件歳年月時間分秒度倍点位階匹頭羽";
    let before = &input[..pos];
    let after = &input[pos + ch.len_utf8()..];
    let mut b = before.chars().rev();
    let (Some(d1), prev2) = (b.next(), b.next()) else {
        return false;
    };
    let mut a = after.chars();
    let (Some(d2), next2) = (a.next(), a.next()) else {
        return false;
    };
    if prev2.is_some_and(is_digit_like_char) || !next2.is_some_and(|c| COUNTERS.contains(c)) {
        return false;
    }
    let (Some(x), Some(y)) = (
        normalize_digits(&d1.to_string()).parse::<u32>().ok(),
        normalize_digits(&d2.to_string()).parse::<u32>().ok(),
    ) else {
        return false;
    };
    (1..=8).contains(&x) && y == x + 1
}

/// `10-20人` の `-`: 数字 - 数字 の直後に助数詞が続き、 後ろの数が大きい時は範囲の区切り (から)。
/// `3-2` (スコア) や `2024-10-12` (日付) のように助数詞が無い / 後ろが小さい組は従来どおり読まない
fn hyphen_is_count_range(input: &str, pos: usize, ch: char) -> bool {
    const COUNTERS: &str = "人日個回名枚本冊台件円歳才年月時間分秒度倍点位階匹頭羽";
    let first: String = input[..pos]
        .chars()
        .rev()
        .take_while(|&c| is_digit_like_char(c))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let after = &input[pos + ch.len_utf8()..];
    let second: String = after
        .chars()
        .take_while(|&c| is_digit_like_char(c))
        .collect();
    let (Ok(a), Ok(b)) = (
        normalize_digits(&first).parse::<u64>(),
        normalize_digits(&second).parse::<u64>(),
    ) else {
        return false;
    };
    a < b
        && after[second.len()..]
            .chars()
            .next()
            .is_some_and(|c| COUNTERS.contains(c))
}

/// `-` が **負の符号** か (= 「マイナス」 と読むべきか)。
///
/// 符号なら直後が数値で、 かつ直前は数値ではない (「-3」 「弾道 -3」)。
/// 直前が数字ならスコア / 面数 / 年範囲の区切りであり (「3-1」 「2024-2025」
/// 「1-2-Switch」)、 減算の意味は持たない。
fn minus_is_sign(input: &str, pos: usize, ch: char) -> bool {
    let prev = input[..pos].chars().next_back();
    let prev_is_digit = prev.is_some_and(is_digit_like_char);
    // 英字の直後も符号ではなく型番ハイフン (RX-78-2 / ak-74)。
    let prev_is_latin = prev.is_some_and(|c| c.is_ascii_alphabetic());
    let next_pos = pos + ch.len_utf8();
    let next_is_digit = input[next_pos..]
        .chars()
        .next()
        .is_some_and(is_digit_like_char);
    let prev_is_time_unit = prev.is_some_and(|c| matches!(c, '時' | '分' | '秒'));
    next_is_digit && !prev_is_digit && !prev_is_latin && !prev_is_time_unit
}

/// 全角数字を半角にする (時刻の範囲検査用)
fn normalize_digits(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '０'..='９' => {
                char::from(b'0' + u8::try_from(u32::from(c) - u32::from('０')).unwrap_or(0))
            }
            _ => c,
        })
        .collect()
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
        out: &mut Vec<RawCandidate<'_>>,
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
    fn try_date(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
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
    fn try_time_jp(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
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
        self.try_time_jp_kanji(input, pos, rest, out);
    }

    /// 漢数字の和式時刻 (`午後三時五分` = さんじごふん)。 時 0〜24 / 分・秒 0〜59 の時だけ
    fn try_time_jp_kanji(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        let Some(caps) = at_start(&TIME_JP_KANJI_RE, rest) else {
            return;
        };
        let num = |i: usize| caps.get(i).and_then(|m| kansuji_to_arabic(m.as_str()));
        let (Some(h), Some(mi)) = (num(1), num(2)) else {
            return;
        };
        let se = caps.get(3).map(|_| num(3));
        let in_range = |s: &str, max: u32| s.parse::<u32>().is_ok_and(|v| v <= max);
        if !in_range(&h, 24)
            || !in_range(&mi, 59)
            || se
                .as_ref()
                .is_some_and(|s| !s.as_deref().is_some_and(|s| in_range(s, 59)))
        {
            return;
        }
        let m_end = caps.get(0).unwrap().end();
        // 直後が の (三時四十五分の事件) は Lindera の 分の (分数) に続けられず行き止まりになるので出さない
        // (従来の経路で よんじゅうごふん と読める)
        if rest[m_end..].starts_with('の') {
            return;
        }
        let mut reading = self.read_counter(&h, "時");
        reading.push_str(&self.read_counter(&mi, "分"));
        if let Some(Some(s)) = se {
            reading.push_str(&self.read_counter(&s, "秒"));
        }
        out.push(self.make(input, pos, m_end, reading));
    }

    /// section 3: 時刻 HH:MM(:SS)。 時は 0〜29 (25:00 = 深夜の 25 時表記)、 分・秒は 0〜59 の時だけ時刻 (10:00 は 「じゅうじ」)。
    /// 時刻にならない `数字:数字` (1:1 / 50:50 / 3:7) は比・スコアとして 「たい」 で読む
    fn try_time_colon(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
        if let Some(caps) = at_start(&TIME_COLON_RE, rest) {
            let m_end = caps.get(0).unwrap().end();
            let h = caps.get(1).unwrap().as_str();
            let mo = caps.get(2).unwrap().as_str();
            let se = caps.get(3).map(|m| m.as_str());
            let in_range =
                |s: &str, max: u32| normalize_digits(s).parse::<u32>().is_ok_and(|n| n <= max);
            if in_range(h, 29) && in_range(mo, 59) && se.is_none_or(|s| in_range(s, 59)) {
                let mut reading = self.read_counter(h, "時");
                if se.is_some() || normalize_digits(mo) != "00" {
                    reading.push_str(&self.read_counter(mo, "分"));
                }
                if let Some(s_str) = se {
                    reading.push_str(&self.read_counter(s_str, "秒"));
                }
                out.push(self.make(input, pos, m_end, reading));
                return;
            }
        }
        // 比 / スコア。 前が数字・コロンなら途中なので出さない
        let prev_blocks = input[..pos]
            .chars()
            .next_back()
            .is_some_and(|c| is_digit_like_char(c) || matches!(c, ':' | '：'));
        if prev_blocks {
            return;
        }
        if let Some(caps) = at_start(&RATIO_RE, rest) {
            let whole = caps.get(0).unwrap();
            let after = &rest[whole.end()..];
            if after.starts_with(|c: char| is_digit_like_char(c) || matches!(c, ':' | '：')) {
                return;
            }
            let reading = format!(
                "{}タイ{}",
                number_to_katakana(caps.get(1).unwrap().as_str()),
                number_to_katakana(caps.get(2).unwrap().as_str())
            );
            out.push(self.make(input, pos, whole.end(), reading));
        }
    }

    /// section 4: 数値 + 大数スケール (+ 末尾漢字 unit)。
    fn try_scale(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
        let Some(re) = &self.scale_re else { return };
        if let Some(caps) = at_start(re, rest) {
            let m_end = caps.get(0).unwrap().end();
            let num = caps.get(1).unwrap().as_str();
            let scale = caps.get(2).unwrap().as_str();
            let mut m_end = m_end;
            let mut trailing_unit = caps.get(3).map(|m| m.as_str());
            // 末尾の助数詞が動詞の語幹を兼ねる時 (1000万行くな 等) は、 助数詞を外した span で出す
            if let Some(u) = trailing_unit {
                if self.counter_blocked(u, &rest[m_end..]) {
                    m_end = caps.get(3).unwrap().start();
                    trailing_unit = None;
                }
            }
            let mut reading = scale_reading(num, scale, &self.scales);
            if let Some(u) = trailing_unit {
                if let Some(unit_kana) = self.units.lookup(u) {
                    reading.push_str(unit_kana);
                } else if let Some(suffix) =
                    scale_trailing_counter_suffix(u, num, scale, &self.counters)
                {
                    // ★alpha.21 round 7: trailing unit が SI units に無いとき
                    // counter rules から suffix を append (= 「1 万歩」 → イチマン+ポ)。
                    reading.push_str(&suffix);
                } else {
                    // 読みを解決できない trailing unit は **候補ごと捨てる**。
                    // 以前は surface をそのまま reading に混ぜていたため、
                    // 「100万目指せる」 が 「ヒャクマン目サセル」 のように
                    // **漢字が読みに残った** (★2026-09-11 実コーパス 704 万行で検出、
                    // 「N万目標 / N万目前 / N万目指す」 が全滅していた)。
                    // 候補を出さなければ 「100万」 + 「目指せる」 の path が選ばれる。
                    return;
                }
            }
            out.push(self.make(input, pos, m_end, reading));
        }
    }

    /// section 5: 数値 + SI 単位。
    fn try_si_unit(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
        let Some(re) = &self.si_unit_re else { return };
        // 数字と記号始まりの単位の間の半角空白 1 つ (Wikipedia 表記の 「50 °C」) は詰めて照合する。
        // 英字の単位 (in / m) は英文 (Top 5 in) を巻き込むので空白を許さない。
        let spaced = DIGIT_RE
            .find(rest)
            .filter(|m| m.start() == 0)
            .and_then(|m| {
                let after = rest.get(m.end()..)?.strip_prefix(' ')?;
                after
                    .chars()
                    .next()
                    // % は助数詞側 (促音の規則付き) で読むので対象外
                    .is_some_and(|c| {
                        !c.is_alphanumeric() && !c.is_whitespace() && !matches!(c, '%' | '％' | '‰')
                    })
                    .then(|| (m.end(), format!("{}{}", m.as_str(), after)))
            });
        if let Some((num_end, joined)) = spaced {
            if let Some(caps) = at_start(re, &joined) {
                let num = caps.get(1).unwrap().as_str();
                let unit = caps.get(2).unwrap().as_str();
                if caps.get(1).unwrap().end() == num_end && self.units.lookup(unit).is_some() {
                    let m_end = caps.get(0).unwrap().end() + 1; // 詰めた空白 1 つ分を戻す
                    out.push(self.make(input, pos, m_end, si_unit_reading(num, unit, &self.units)));
                }
            }
        }
        if let Some(caps) = at_start(re, rest) {
            let m_end = caps.get(0).unwrap().end();
            let num = caps.get(1).unwrap().as_str();
            let unit = caps.get(2).unwrap().as_str();
            // ★2026-09-11: regex は case-insensitive で span を取るが、 units 側が
            // `ci = false` (= 小文字限定の 「g」 「m」 「t」) だと lookup が miss する。
            // 以前はそのまま空文字を連結していたので 「4G」 が 「ヨン」 になり、
            // **単位文字が黙って消えていた**。 解決できない span は候補ごと捨て、
            // alphabet passthrough に任せる (try_scale の trailing unit と同じ扱い)。
            if self.units.lookup(unit).is_none() {
                return;
            }
            let reading = si_unit_reading(num, unit, &self.units);
            out.push(self.make(input, pos, m_end, reading));
        }
    }

    /// 助数詞 `counter` の `not_before` に直後の文字列 `after` が当たるか
    fn counter_blocked(&self, counter: &str, after: &str) -> bool {
        self.counters
            .counter
            .get(counter)
            .is_some_and(|r| r.not_before.iter().any(|p| after.starts_with(p.as_str())))
    }

    /// section 5.5: 分数 「N分のM」 (間の半角空白は任意)。 「3分の1」 は従来も 分の = ぶんの だったが、
    /// それは 「3分」 (さんぷん) の後の 「の」 に候補が無く行き止まりになる偶然だった。 明示の候補にする
    fn try_fraction(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
        if let Some(caps) = at_start(&FRACTION_RE, rest) {
            let m_end = caps.get(0).unwrap().end();
            let den = caps.get(1).unwrap().as_str();
            let num = caps.get(2).unwrap().as_str();
            let reading = format!(
                "{}ブンノ{}",
                number_to_katakana(den),
                number_to_katakana(num)
            );
            out.push(self.make(input, pos, m_end, reading));
        }
    }

    /// 助数詞の付かない漢数字の列 (七百七十三 / 千七百 / 百九億)。 2026-09-27
    ///
    /// 従来は 1 字ずつ dict の漢字規則で読まれ、 助数詞向けの促音規則が数の中に当たっていた
    /// (七百 = しっぴゃく / 百九 = ひゃっきゅう / 七千 = しちせん)。 数として読む候補を出す。
    /// 対象は 2 字以上で、 位の字 (十百千万億) を含むか 〇 を含む位取り 3 桁以上のもの。
    /// 先頭が 万 / 億 (万一)、 数字が 2 つ続く概数 (三十七八 / 八九)、 漢数字の列の途中、
    /// 直後が漢字 (助数詞: 十一兆 / 十二月 / 第十一項 は既存の促音・特殊読みに任せる) の時は出さない
    fn try_kanji_bare_number(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        const DIGITS: &str = "〇一二三四五六七八九";
        const UNITS: &str = "十百千万億";
        let run: String = rest
            .chars()
            .take_while(|&c| DIGITS.contains(c) || UNITS.contains(c))
            .collect();
        let chars: Vec<char> = run.chars().collect();
        if chars.len() < 2 || matches!(chars[0], '万' | '億') {
            return;
        }
        // 数の形をした慣用語は形態素解析の読みに任せる (八百万 = やおよろず)
        if matches!(run.as_str(), "八百万") {
            return;
        }
        if input[..pos]
            .chars()
            .next_back()
            .is_some_and(|c| DIGITS.contains(c) || UNITS.contains(c))
        {
            return;
        }
        let has_unit = chars.iter().any(|&c| UNITS.contains(c));
        // 2 桁は カタカナに挟まった字 (ラ三一ヌ) を除く。 同じ字の繰り返し (⊂二二 / 三三 / 八八ノ = 顔文字・効果線) は
        // 第一一 / （一一） / 一一―四 / 二二キロワット のように 第 の後・括弧で閉じた番号・ダッシュの前後・単位 (キロ / GHz) の前だけ数にする。
        // 助数詞の付く形 (二二年 / 三三％) は counter 側で読む
        let dai = after_dai(input, pos);
        let after = &rest[run.len()..];
        let numeric_repeat = dai
            || after.starts_with(['―', '－', '-'])
            || input[..pos].ends_with(['―', '－', '-'])
            || after.starts_with(|c: char| c.is_ascii_alphabetic())
            || (after.starts_with(['）', ')']) && input[..pos].ends_with(['（', '(']))
            || [
                "キロ",
                "メートル",
                "ミリ",
                "センチ",
                "グラム",
                "トン",
                "ワット",
                "ボルト",
                "ヘルツ",
                "パーセント",
            ]
            .iter()
            .any(|u| after.starts_with(u));
        let two_digit = two_digit_positional(&run, dai)
            && (chars[0] != chars[1] || numeric_repeat)
            && !input[..pos]
                .chars()
                .next_back()
                .is_some_and(crate::kana::is_katakana_char);
        // 3 桁以上で英字の単位が続く (一四二MHz / 二一五ｋＨｚ) なら 〇 が無くても位取り。 2026-09-29
        let before_latin = rest[run.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, 'Ａ'..='Ｚ' | 'ａ'..='ｚ'));
        let positional = !has_unit
            && ((chars.len() >= 3 && (chars.contains(&'〇') || before_latin)) || two_digit);
        if !(has_unit || positional) {
            return;
        }
        let consecutive_digits = chars.windows(2).any(|w| {
            w[0] != '〇' && w[1] != '〇' && DIGITS.contains(w[0]) && DIGITS.contains(w[1])
        });
        if has_unit && consecutive_digits {
            return;
        }
        // 同じ位置からもっと長い候補 (助数詞付き: 十一兆 / 十二月 / 三十八分 / 辞書の 八百屋) が既にあれば出さない。
        // 数だけの候補が 助数詞の促音・特殊読み (じゅういっちょう / じゅうにがつ) に勝ってしまうため
        if out
            .iter()
            .any(|c| c.range.start == pos && c.range.end >= pos + run.len())
        {
            return;
        }
        // 直後が漢字なら助数詞 (年 / 兆 / 項 / 号 …) として既存の読み (促音・特殊読み: じゅういっちょう /
        // じゅうにがつ / じゅういっこう) に任せる。 後ろがかな・カタカナ・記号・行末の数だけを対象にする
        // ただし 〇 を含む位取り (一〇以上 / 二〇平方メートル) は 1 字ずつ読むと いちれい になるだけなので出す。 2026-09-29
        // 型式 (九〇式 = きゅうまるしき) / 〇 始まり / 小数部 (秒・〇八三〇東経) は従来どおり
        let next = rest[run.len()..].chars().next();
        let zero_positional = positional
            && chars.contains(&'〇')
            && chars[0] != '〇'
            && next != Some('式')
            && !input[..pos].ends_with(['・', '．', '.']);
        if !zero_positional && next.is_some_and(crate::kana::is_kanji_char) {
            return;
        }
        let arabic = if two_digit {
            positional_digits(&run)
        } else {
            crate::numbers::helpers::kansuji_to_arabic(&run)
        };
        let Some(arabic) = arabic else {
            return;
        };
        out.push(self.make(input, pos, run.len(), number_to_katakana(&arabic)));
    }

    /// 位取りの漢数字の小数 (一・〇〇七 = イチテンゼロゼロナナ / 〇・五〇 = ゼロテンゴゼロ)。
    /// 漢数字の列の途中 (直前が漢数字 / ・) からは出さない。 2026-09-27
    fn try_kanji_decimal(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        let Some(caps) = at_start(&KANJI_DECIMAL_RE, rest)
            .or_else(|| {
                // 小数部 1 桁 (三八・四％) は 整数部が位取り 2 桁の時だけ。 一・二 / 三・四 は並列の列挙
                at_start(&KANJI_DECIMAL_SHORT_RE, rest)
                    .filter(|c| two_digit_positional(c.get(1).unwrap().as_str(), false))
            })
            .or_else(|| {
                // 1 桁・1 桁でも、 直後に単位 (％ / 倍 / 万円 …) が付けば小数 (二・五％ / 一・八倍)。
                // 三・四 のように 1 つ違いで上がる並びは 並列・範囲 (3 と 4) の可能性が高いので除く (〇・一 は小数)
                at_start(&KANJI_DECIMAL_UNIT_RE, rest).filter(|c| {
                    let d = |i: usize| {
                        c.get(i)
                            .unwrap()
                            .as_str()
                            .chars()
                            .next()
                            .and_then(kanji_digit)
                            .and_then(|x| x.to_digit(10))
                    };
                    match (d(1), d(2)) {
                        (Some(a), Some(b)) => a == 0 || b != a + 1,
                        _ => false,
                    }
                })
            })
        else {
            return;
        };
        if input[..pos]
            .chars()
            .next_back()
            .is_some_and(|c| c == '・' || kanji_digit(c).is_some())
        {
            return;
        }
        // 数の終わり (単位つきの形では単位は数に含めない)
        let m_end = caps.get(2).unwrap().end();
        let to_arabic = |s: &str| s.chars().filter_map(kanji_digit).collect::<String>();
        let num = format!(
            "{}.{}",
            to_arabic(caps.get(1).unwrap().as_str()),
            to_arabic(caps.get(2).unwrap().as_str())
        );
        out.push(self.make(input, pos, m_end, number_to_katakana(&num)));
    }

    /// section 6: 数値 + 単一助数詞 (+ optional 末尾再帰 「目」)。
    fn try_counter(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
        let Some(re) = &self.counter_re else { return };
        if let Some(caps) = at_start(re, rest) {
            let m_end = caps.get(0).unwrap().end();
            let num = caps.get(1).unwrap().as_str();
            let base = caps.get(2).unwrap().as_str();
            // group 3 = optional 末尾再帰助数詞 (= 「目」)。 match したら
            // 「個」 + 「目」 = 「個目」 を read_counter に渡し、 euphonic_counter_read
            // の strip_suffix('目') 再帰で 「ニコメ」 等を得る。
            // 助数詞の字が動詞の語幹を兼ねる時 (行 + って 等) は候補を出さない
            if caps.get(3).is_none() && self.counter_blocked(base, &rest[m_end..]) {
                return;
            }
            // 数字と助数詞の間に空白を挟む書き方 (「5 分」) は、 助数詞の直後が漢字なら別の語の頭
            // (「15 本体」 / 「3 人間」) なので助数詞にしない (2026-09-27)
            let spaced = caps.get(1).unwrap().end() < caps.get(2).unwrap().start();
            if spaced
                && caps.get(3).is_none()
                && rest[m_end..]
                    .chars()
                    .next()
                    .is_some_and(crate::scoring::lindera_fallback::is_real_cjk_ideograph)
            {
                return;
            }
            let combined;
            let counter = if let Some(rec) = caps.get(3) {
                combined = format!("{base}{}", rec.as_str());
                combined.as_str()
            } else {
                base
            };
            let mut reading = self.read_counter(num, counter);
            // % / ‰ の直後が閉じ括弧 ((100%) / [5%] / 「50%」) だと、 Lindera が 「%)」 を 1 つの記号 token にまとめて、
            // 助数詞の候補が 「%」 の位置で token 境界に着かず落ちていた (% が読まれず素通し)。 閉じ括弧も一緒に取る
            let m_end = match (base, rest[m_end..].chars().next()) {
                ("%" | "％" | "‰", Some(c)) if is_closing_bracket(c) => {
                    reading.push(c);
                    m_end + c.len_utf8()
                }
                _ => m_end,
            };
            // かなの助数詞 (2つ / 3つ) だけ match_hits を付ける。
            // Lindera が 「つね」 「つまで」 を 1 語にすると 3つ + ね と 3 + つね が
            // どちらも 2 edge / 最弱 band 50 で並び、 hits が無いと列挙順で さんつね になる。
            // 漢字の助数詞 (分 / 位 / 匹 / 月) に付けると 腹八分目 = はっぷんめ /
            // 3時間位 = さんじかんい のように dict entry を押しのけるので、 かなに限る
            // (★2026-09-18 corpus で検出)。
            let kana_counter = counter.chars().all(crate::kana::is_hiragana_char);
            let cand = if kana_counter {
                self.make_hit(input, pos, m_end, reading)
            } else {
                self.make(input, pos, m_end, reading)
            };
            out.push(cand);
        }
    }

    /// section 6a: 読点区切りの漢数字 + 助数詞 (`一、〇〇〇円` = センエン / `三、五〇〇円`)。
    /// 法令・判例は金額を漢数字で 3 桁ごとに読点で区切る。 区切りを外すと位取りの漢数字になるので
    /// [`Self::read_counter`] (内部で位取りとして数に直す) にそのまま渡す (★2026-09-24、 旧 いち、れいれいれいえん)
    fn try_counter_kanji_grouped(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        let Some(re) = &self.counter_kanji_grouped_re else {
            return;
        };
        if let Some(caps) = at_start(re, rest) {
            let m_end = caps.get(0).unwrap().end();
            // 漢数字の桁を算用数字に直し、 万 / 億 / 兆 が挟まれば 0 を足す (一、〇〇〇万円 = 10000000 円)
            let mut num: String = caps
                .get(1)
                .unwrap()
                .as_str()
                .chars()
                .filter_map(|c| "〇一二三四五六七八九".chars().position(|d| d == c))
                .map(|d| char::from(b'0' + d as u8))
                .collect();
            let base = caps.get(3).unwrap().as_str();
            if self.counter_blocked(base, &rest[m_end..]) {
                return;
            }
            let scale = caps.get(2).map(|m| m.as_str());
            if let Some(scale) = scale.filter(|s| *s != "千") {
                num.push_str(match scale {
                    "万" => "0000",
                    "億" => "00000000",
                    _ => "000000000000",
                });
            }
            // 千円単位の表記 (一〇、〇〇〇千円 = 10,000 千円 = いちまんせんえん) は 数 + 「千 + 助数詞」 と読む (2026-09-27)
            let reading = if scale == Some("千") {
                format!(
                    "{}{}",
                    number_to_katakana(&num),
                    self.read_counter("1000", base)
                )
            } else {
                self.read_counter(&num, base)
            };
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
    fn try_counter_kanji(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        // 何百件 / 何千円 = 何 の位置から 1 つの候補にする (2026-09-28)。 百 / 千 から出すと 何 が単独で なに に
        // なり連濁も落ちる (なにひゃっけん / なにせんえん)。 何 の連濁・促音は 三 と同じ (なんびゃく / さんびゃく、
        // なんぜん / さんぜん) なので、 三 に置き換えて読み、 頭の サン を ナン に戻す (三 と 何 は同じ 3 byte)
        if let Some(after) = rest.strip_prefix('何') {
            if after.starts_with(['百', '千']) {
                let alt = format!("三{after}");
                if let Some((m_end, reading)) = self.counter_kanji_reading(&alt, 0, &alt) {
                    if let Some(tail) = reading.strip_prefix("サン") {
                        out.push(self.make(input, pos, m_end, format!("ナン{tail}")));
                    }
                }
            }
            return;
        }
        if let Some((m_end, reading)) = self.counter_kanji_reading(input, pos, rest) {
            out.push(self.make(input, pos, m_end, reading));
        }
    }

    /// [`Self::try_counter_kanji`] の本体: 候補を出すなら (長さ, 読み)。
    fn counter_kanji_reading(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
    ) -> Option<(usize, String)> {
        let re = self.counter_kanji_re.as_ref()?;
        let caps = at_start(re, rest)?;
        {
            let m_end = caps.get(0).unwrap().end();
            let num = caps.get(1).unwrap().as_str();
            let base = caps.get(2).unwrap().as_str();
            // 万 / 億 / 兆 から始まる数は 「一万」 ではなく前の語 (数万件 / 何万円 / 3桁万円) の一部なので counter にしない
            // (数万件 = すういちまんけん になっていた、 2026-09-27)。 大数の読みは 万 / 億 の漢字側に任せる
            if num.starts_with(['万', '億', '兆']) {
                return None;
            }
            // 算用数字に直せない並び (八億二七〇〇〇万 のような崩れた表記) は候補にしない。
            // read_counter は直せないと漢数字をそのまま読みに使うので、 漢字が読みに残っていた (2026-09-27)
            kansuji_to_arabic(num)?;
            // 数の途中 (一・九六億円 の 九六億円 / 一〇八六億円 の 〇八六億円) からは出さない
            let mut before = input[..pos].chars().rev();
            let prev = before.next();
            let prev2 = before.next();
            let is_digit = |c: char| "〇一二三四五六七八九十百千".contains(c);
            if prev.is_some_and(is_digit)
                || (matches!(prev, Some('・' | '、' | '．')) && prev2.is_some_and(is_digit))
            {
                return None;
            }
            // 何百 / 何千 の 百 / 千 からは出さない (何 の位置で出す、 上の try_counter_kanji)
            if prev == Some('何') && num.starts_with(['百', '千']) {
                return None;
            }
            // 概数 (二三日 / 四五回 = 2〜3 日 / 4〜5 回) は 1 つの数ではない。 read_counter に渡すと
            // 後ろの桁だけ残って みっか / ごかい / さんけん になっていた (2026-10-02)。
            // 第 の後・％ の前・条 / 項 / 号 の前は番号なので従来どおり (第二三条 / 八九条)
            if approx_pair(num) && !ordinal_context(prev, base) {
                return None;
            }
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
                // 位取りの漢数字 (〇〜九 だけの 3 桁以上 = 一九九九 / 二〇二三) は数としか読めないので、
                // opt-in していない助数詞 (年 等) でも counter にする (★2026-09-24、 旧 いっきゅうくくねん)
                let positional = num.chars().count() >= 3
                    && num.chars().all(|c| {
                        matches!(
                            c,
                            '〇' | '零'
                                | '一'
                                | '二'
                                | '三'
                                | '四'
                                | '五'
                                | '六'
                                | '七'
                                | '八'
                                | '九'
                        )
                    });
                let positional =
                    positional || two_digit_positional(num, ordinal_context(prev, base));
                if !(opted_in || positional) || self.counter_blocked(base, &rest[m_end..]) {
                    return None;
                }
                base.to_string()
            };
            // 第一二号 のような 1 つ違いの並びは kansuji_to_arabic が概数として扱うので、 桁を並べた数で渡す
            let digits = (ordinal_context(prev, base) && two_digit_positional(num, true))
                .then(|| positional_digits(num))
                .flatten();
            Some((
                m_end,
                self.read_counter(digits.as_deref().unwrap_or(num), &counter),
            ))
        }
    }

    /// section 7: 記号 1 文字。 candidates_at から、 数値系を skip する非数値
    /// lead 経路と通常経路の双方から呼ぶ (= 記号判定は数値系の有無に依らず常に行う)。
    fn emit_symbol(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
        let Some(ch) = rest.chars().next() else {
            return;
        };
        let symbol_read = if ch == '×' {
            Some(times_sign_reading(input, pos, ch).to_string())
        } else if DECORATIVE_SHAPES.contains(ch)
            && !is_placeholder_shape(input, pos, ch, &self.honorifics)
        {
            None
        } else {
            symbol_char_reading(ch, &self.symbols)
        };
        if let Some(read) = symbol_read {
            // 〜 / ~ / ～ は **range marker と vowel extension の dual use**。
            // 数字 context (= 「2〜3」 「100〜200円」) なら従来通り 「から」 で concat、
            // kana / 漢字 / 文末 context (= 「がんばれ〜」 「も〜むりすぎ」) では range
            // ではなく長音・強調用途なので 「から」 は誤読。 空 reading で surface のみ
            // 消費し、 読み上げない (= 「がんばれ〜」 → 「がんばれ」)。
            let is_range_marker = matches!(ch, '〜' | '~' | '～');
            // `-` も同様に **マイナスとハイフンの dual use**。 「-3」 「3-1」 のような数字
            // context のみ 「マイナス」、 それ以外 (英字語の Wi-Fi / 区切りの 「あ-い」 /
            // 文末の 「終わり-」) はハイフンなので読まない。
            let is_minus = matches!(ch, '-' | '\u{FF0D}' | '\u{2212}');
            // 2026-09-11: 数字に挟まれたハイフンは減算ではなく区切り。
            // 実データ 704 万行では 3-1 / 5-5 / 22-0 (スコア) や マリオ 2-1 (面数)、
            // 2024-2025 (年範囲) ばかりで、 マイナス と読むべき例は無い。
            // 負数は -3 のように前が数字でない形で書かれるので、
            // 前が数字のときだけ読まない (区切りとして surface のみ消費する)。
            // 隣り合う 1 桁の数 + 助数詞 (2〜3時間 / 3~4人) は、 話し言葉では 「にさんじかん」 と詰める。
            // 「から」 を入れると 「にからさんじかん」 と硬い (user 方針、 2026-10-19)。 離れた数 (2〜5回) や 2 桁 (10〜20人) は 「から」 のまま。
            // ハイフン (1-2日 / 3-4人) はスコア・面数と紛れるので対象外 (従来どおり hyphen_is_count_range で 「から」)
            let final_read = if is_range_marker && range_is_adjacent_small(input, pos, ch) {
                String::new()
            } else if is_minus
                && (hyphen_is_time_range(input, pos, ch)
                    || hyphen_is_count_range(input, pos, ch)
                    || hyphen_is_clock_range(input, pos, ch)
                    || hyphen_is_year_range(input, pos, ch)
                    || hyphen_is_unit_range(input, pos, ch)
                    || hyphen_is_date_range(input, pos, ch))
            {
                "から".to_string()
            } else if (is_minus && !minus_is_sign(input, pos, ch))
                || (is_range_marker && !range_marker_in_numeric_context(input, pos, ch))
            {
                String::new()
            } else {
                read
            };
            out.push(self.make(input, pos, ch.len_utf8(), final_read.clone()));

            // 同じ記号が続く場合 (C++ / g++ の 「++」)、 列全体を覆う候補も出す。 Lindera は 「++」 を
            // 1 つの未知語 token (reading = surface) にまとめるので、 1 文字ずつの候補では edge 数で負けて
            // 「C++と」 が 「++」 のまま素通しになっていた (「C++で」 だけ分かれ方が違って読めていた、 2026-09-27)。
            // 対象は + だけ: 「///」 は配信コメントの照れ表現 / 「//」 はコメント記号で、 読み上げると
            // すらっしゅ の連呼になる (実コーパス A/B で確認)。 - / 〜 は文脈で読みが変わる兼用記号
            if matches!(ch, '+' | '＋') && !final_read.is_empty() {
                let run = rest.chars().take_while(|&c| c == ch).count();
                if run > 1 {
                    out.push(self.make(input, pos, ch.len_utf8() * run, final_read.repeat(run)));
                }
            }

            // 記号の直後に句読点が続く場合、 **記号 + 句読点** を覆う候補も出す。
            //
            // Lindera は 「%。」 のような列を 1 つの未知語 token (band 50、 reading = surface)
            // にまとめることがある。 記号単体の候補 (band 950) + 句読点の edge という path は
            // edge 数が 2 になるため、 longest-match 優先で **band 50 の passthrough に負け**、
            // 「50%。」 が 「ごじゅう%。」 と読まれていた (★2026-09-11 文脈不変検査で検出)。
            // 句読点は読み上げないので reading は記号の読みのまま。
            let mut consumed = ch.len_utf8();
            for next in rest[consumed..].chars() {
                if !is_trailing_punctuation(next) {
                    break;
                }
                consumed += next.len_utf8();
            }
            if consumed > ch.len_utf8() {
                // 句読点は surface のまま読みに残す (TTS の pause 情報を落とさない)。
                let punct = &rest[ch.len_utf8()..consumed];
                out.push(self.make(input, pos, consumed, format!("{final_read}{punct}")));
            }
        }
    }

    /// バージョン表記 / IP (`1.2.3`): 各要素を数として読み 「テン」 で繋ぐ。
    fn try_version(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
        if let Some(m) = at_start(&VERSION_RE, rest) {
            let whole = m.get(0).unwrap();
            if looks_like_dotted_date(whole.as_str()) {
                return;
            }
            let reading = whole
                .as_str()
                .split('.')
                .map(number_to_katakana)
                .collect::<Vec<_>>()
                .join("テン");
            out.push(self.make_hit(input, pos, whole.end(), reading));
        }
    }

    /// 通貨記号が数字の前に付く形 (`¥1000` / `$100` / `€5`)。 数字 + 通貨名の語順に直して読む
    /// (¥1000 → センエン / $100 → ヒャクドル)。 記号が無ければ何もしない。
    fn try_currency_prefix(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        first_char: char,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        let unit = match first_char {
            '¥' | '￥' => "エン",
            '$' | '＄' => "ドル",
            '€' => "ユーロ",
            '£' => "ポンド",
            _ => return,
        };
        let after = &rest[first_char.len_utf8()..];
        // 位取りのカンマ (¥1,337 / $12,500) は外して 1 つの数として読む
        if let Some((len, digits)) = comma_grouped_number(after) {
            let reading = format!("{}{unit}", number_to_katakana(&digits));
            out.push(self.make_hit(input, pos, first_char.len_utf8() + len, reading));
            return;
        }
        if let Some(m) = at_start(&DIGIT_RE, after) {
            let whole = m.get(0).unwrap();
            // 符号付き (¥-5) は扱わない。 数字で始まる時だけ。
            if !after.starts_with(|c: char| c.is_ascii_digit() || ('０'..='９').contains(&c)) {
                return;
            }
            let reading = format!("{}{unit}", number_to_katakana(whole.as_str()));
            out.push(self.make_hit(input, pos, first_char.len_utf8() + whole.end(), reading));
        }
    }

    /// 入力中の型番が始まる byte 位置 (英字 range を切るために使う)。 型番パターンが無ければ空
    #[must_use]
    pub fn model_starts(&self, input: &str) -> Vec<usize> {
        if self.models.is_empty() {
            return Vec::new();
        }
        // 語頭 (直前が英数字でない英字) だけを調べる
        let mut prev_alnum = false;
        let mut starts = Vec::new();
        for (i, c) in input.char_indices() {
            if c.is_ascii_alphabetic() && !prev_alnum && self.models.match_at(input, i).is_some() {
                starts.push(i);
            }
            prev_alnum = c.is_ascii_alphanumeric();
        }
        starts
    }

    /// 型番 (`RTX 4070 Ti` / `Ryzen 7 5800X3D` / `i9-13900K`)。 `models.toml` のパターンに当たる時だけ
    fn try_model(
        &self,
        input: &str,
        pos: usize,
        first_char: char,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        if !first_char.is_ascii_alphabetic() || self.models.is_empty() {
            return;
        }
        if let Some((end, reading)) = self.models.match_at(input, pos) {
            out.push(self.make_hit(input, pos, end - pos, reading));
        }
    }

    /// 電話番号 (`03-1234-5678` / `090 1234 5678` / `0120-123-456` / `+81-90-1234-5678` / `+1 555-123-4567` /
    /// `09012345678`)。 桁を 1 つずつ読み、 区切りの位置は間 (空白) にする。 数 (さんぜん…) としては読まない。
    /// 国内は 10〜11 桁、 国際 (`+` から) は 8〜15 桁 (E.164)
    fn try_phone(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
        let Some(m) = at_start(&PHONE_RE, rest) else {
            return;
        };
        let text = m.get(0).unwrap().as_str();
        let end = m.get(0).unwrap().end();
        let digits = text.bytes().filter(u8::is_ascii_digit).count();
        let international = text.starts_with('+');
        let ok = if international {
            (8..=15).contains(&digits)
        } else {
            (10..=11).contains(&digits)
        };
        // 長い数字列 / 区切りつなぎの途中 (日付 2024-03-1234 の後半など) は電話番号にしない。
        // 国際の + は英数字の直後 (A+1234 5678) なら足し算なので除く
        let prev = input[..pos].chars().next_back();
        let prev_blocks = prev.is_some_and(|c| {
            c.is_ascii_digit() || c == '-' || (international && c.is_ascii_alphabetic())
        });
        let next_blocks = rest[end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || c == '-');
        if !ok || prev_blocks || next_blocks {
            return;
        }
        // ハイフンなしの携帯 (09012345678) は 3-4-4 に区切る
        let spaced;
        let text = if text.contains(['-', ' ']) {
            text
        } else {
            spaced = format!("{}-{}-{}", &text[..3], &text[3..7], &text[7..]);
            spaced.as_str()
        };
        let mut reading = String::new();
        for (i, group) in text.split(['-', ' ']).enumerate() {
            if i > 0 {
                reading.push(' ');
            }
            let group = match group.strip_prefix('+') {
                Some(g) => {
                    reading.push_str("プラス");
                    g
                }
                None => group,
            };
            reading.push_str(&DigitStyle::Each.render(group).unwrap_or_default());
        }
        out.push(self.make(input, pos, end, reading));
    }

    /// 英字・記号で書く数の形: 累乗 `x^2` / `2^10`、 倍率 `x2` / `2x`、 版 `v2.0`
    fn try_ascii_forms(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        first_char: char,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        let prev = input[..pos].chars().next_back();
        let is_word = |c: char| c.is_ascii_alphanumeric();
        match first_char {
            // 和暦の略記: R7.10.13 / H30.4.1 / S64.1.7 / T15.12.25 / M45.7.30、 R7年 / R7年度 / S50年代
            'M' | 'T' | 'S' | 'H' | 'R' => {
                if prev.is_some_and(is_word) {
                    return;
                }
                if let Some((len, reading)) = self.era_abbreviation(rest) {
                    out.push(self.make_hit(input, pos, len, reading));
                }
            }
            // 累乗: 直前が数字・英字・閉じ括弧、 指数は 1〜3 桁 (x^2 → エックスノニジョウ の 「ノニジョウ」)
            '^' => {
                if !prev.is_some_and(|c| is_word(c) || matches!(c, ')' | '）')) {
                    return;
                }
                let digits: String = rest[1..].chars().take_while(char::is_ascii_digit).collect();
                if digits.is_empty()
                    || digits.len() > 3
                    || rest[1 + digits.len()..].starts_with(|c: char| is_word(c) || c == '乗')
                {
                    return;
                }
                out.push(self.make_hit(
                    input,
                    pos,
                    1 + digits.len(),
                    format!("ノ{}ジョウ", number_to_katakana(&digits)),
                ));
            }
            'x' | 'X' => {
                let after = &rest[1..];
                // 2x / 1.5x: 数字の直後の x は 「倍」 (0x1F や 2x2 は除く)
                if prev.is_some_and(|c| c.is_ascii_digit()) {
                    if !after.starts_with(|c: char| is_word(c) || c == '×') {
                        out.push(self.make_hit(input, pos, 1, "バイ".to_string()));
                    }
                    return;
                }
                // x2: 英字・数字の直後ではない x + 1〜2 桁 (x86 / x64 / x32 はアーキテクチャなので除く)
                if prev.is_some_and(is_word) {
                    return;
                }
                let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
                if digits.is_empty()
                    || digits.len() > 2
                    || matches!(digits.as_str(), "32" | "64" | "86")
                {
                    return;
                }
                if after[digits.len()..].starts_with(|c: char| is_word(c) || c == '.') {
                    return;
                }
                out.push(self.make_hit(
                    input,
                    pos,
                    1 + digits.len(),
                    format!("{}バイ", number_to_katakana(&digits)),
                ));
            }
            // v2.0: 英字の直後ではない v + 版番号
            'v' | 'V' => {
                // 1v1 / 3v3: 数字に挟まれた v は ブイ (いちぶいいち)
                if prev.is_some_and(|c| c.is_ascii_digit())
                    && rest[1..].starts_with(|c: char| c.is_ascii_digit())
                {
                    out.push(self.make_hit(input, pos, 1, "ブイ".to_string()));
                    return;
                }
                if prev.is_some_and(is_word) {
                    return;
                }
                // 数字 (. 数字)+ を版番号として取る (2 要素の v2.0 も)
                let body = &rest[1..];
                let mut end = 0;
                let mut parts = 0;
                loop {
                    let digits = body[end..].chars().take_while(char::is_ascii_digit).count();
                    if digits == 0 || digits > 3 {
                        break;
                    }
                    end += digits;
                    parts += 1;
                    if body[end..].starts_with('.')
                        && body[end + 1..].starts_with(|c: char| c.is_ascii_digit())
                    {
                        end += 1;
                    } else {
                        break;
                    }
                }
                if parts < 2 || body[end..].starts_with(|c: char| is_word(c)) {
                    return;
                }
                let whole = &body[..end];
                if looks_like_dotted_date(whole) {
                    return;
                }
                let reading = whole
                    .split('.')
                    .map(number_to_katakana)
                    .collect::<Vec<_>>()
                    .join("テン");
                out.push(self.make_hit(input, pos, 1 + end, format!("ブイ{reading}")));
            }
            _ => {}
        }
    }

    /// 郵便番号 `〒100-0001` / `〒 1000001`。 〒 から数字までを 1 語にして 7 桁を 1 桁ずつ読む
    /// (イチゼロゼロ ノ ゼロゼロゼロイチ)
    fn try_postal_mark(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        let after_mark = &rest['〒'.len_utf8()..];
        let skip = after_mark
            .chars()
            .take_while(|&c| c == ' ' || c == '\u{3000}')
            .map(char::len_utf8)
            .sum::<usize>();
        let Some(caps) = at_start(&POSTAL_RE, &after_mark[skip..]) else {
            return;
        };
        let end = '〒'.len_utf8() + skip + caps.get(0).unwrap().end();
        if rest[end..].starts_with(|c: char| is_digit_like_char(c)) {
            return;
        }
        let reading = format!(
            "ユウビンバンゴウ{}",
            postal_digits(caps.get(1).unwrap().as_str(), caps.get(2).unwrap().as_str())
        );
        out.push(self.make(input, pos, end, reading));
    }

    /// `郵便番号 100-0001` のように、 直前が 「郵便番号」 (+ 区切り) の 3 桁-4 桁を 1 桁ずつ読む
    fn try_postal_after_label(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        let before = input[..pos].trim_end_matches([' ', '\u{3000}', ':', '：']);
        if !before.ends_with("郵便番号") {
            return;
        }
        let Some(caps) = at_start(&POSTAL_RE, rest) else {
            return;
        };
        let end = caps.get(0).unwrap().end();
        if rest[end..].starts_with(|c: char| is_digit_like_char(c)) {
            return;
        }
        let reading = postal_digits(caps.get(1).unwrap().as_str(), caps.get(2).unwrap().as_str());
        out.push(self.make(input, pos, end, reading));
    }

    /// 年付きの日付 (`2025/10/13` / `2000-10-12`)。 年 4 桁 + 月 1〜12 + 日 1〜31 で、 区切りは 2 つとも同じ。
    /// 数字・区切りの途中 (1/2/2025/10/13 の後半、 2025-10-13-1) や、 時刻・IP のような長い数字列の一部は読まない
    fn try_ymd(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
        let Some(caps) = at_start(&YMD_RE, rest) else {
            return;
        };
        let whole = caps.get(0).unwrap();
        let (y, sep1, m, sep2, d) = (
            caps.get(1).unwrap().as_str(),
            caps.get(2).unwrap().as_str(),
            caps.get(3).unwrap().as_str(),
            caps.get(4).unwrap().as_str(),
            caps.get(5).unwrap().as_str(),
        );
        let (Ok(mn), Ok(dn)) = (m.parse::<u32>(), d.parse::<u32>()) else {
            return;
        };
        let prev = input[..pos].chars().next_back();
        let next = rest[whole.end()..].chars().next();
        // ドット区切りは年が 19xx / 20xx の時だけ (バージョンの 1234.5.6 と紛れない)。
        // 英字の直後 (v2026.10.12) は版なので読まない
        let dotted_year_ok = sep1 != "." || y.starts_with("19") || y.starts_with("20");
        if sep1 != sep2
            || !dotted_year_ok
            || prev.is_some_and(|c| c.is_ascii_alphabetic())
            || !(1..=12).contains(&mn)
            || !(1..=31).contains(&dn)
            || prev.is_some_and(|c| c.is_ascii_digit() || matches!(c, '/' | '-' | '.'))
            || next.is_some_and(|c| c.is_ascii_digit() || matches!(c, '/' | '-' | '.'))
        {
            return;
        }
        let reading = format!(
            "{}{}{}",
            self.read_counter_in_date(y, "年"),
            self.read_counter_in_date(m, "月"),
            self.read_counter_in_date(d, "日")
        );
        out.push(self.make(input, pos, whole.end(), reading));
    }

    /// 年月 (`2026/09`)。 年 19xx / 20xx + 月 1〜12。 年月日 (`2026/09/15`) は `try_ymd` が読む。
    /// 数字・区切りの途中 (`1/2026/09` / `2026/091` / `2026/09/15`) や、 英字の直後 (版・型番) は読まない
    fn try_year_month(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
        let Some(caps) = at_start(&YM_RE, rest) else {
            return;
        };
        let whole = caps.get(0).unwrap();
        let (y, m) = (caps.get(1).unwrap().as_str(), caps.get(2).unwrap().as_str());
        let Ok(mn) = m.parse::<u32>() else {
            return;
        };
        let prev = input[..pos].chars().next_back();
        let after = &rest[whole.end()..];
        if !(1..=12).contains(&mn)
            || prev.is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '.'))
            || after.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '/' | '-'))
        {
            return;
        }
        let reading = format!(
            "{}{}",
            self.read_counter_in_date(y, "年"),
            self.read_counter_in_date(m, "月")
        );
        out.push(self.make(input, pos, whole.end(), reading));
    }

    /// `N/M` を日付か分数か決める。 雑談では日付のほうが多いので、 手がかりが無ければ日付に倒す。
    /// - 分数の手がかり (直後: サイズ / 倍 / 割 / くらい / ぐらい / ずつ …、 直前: HP / MP / 確率 / ゲージ …) が最優先
    ///   (1/2サイズ = にぶんのいちさいず / HP 12/25 = にじゅうごぶんのじゅうに)
    /// - 月 1〜12 / 日 13〜31 は日付 (12/25 = じゅうにがつにじゅうごにち)
    /// - 月 1〜12 / 日 1〜12 は日付に倒す。 ただし 分子 1 の 1/2〜1/10 (1/3 = さんぶんのいち)、
    ///   分数の話題の語 (スコア / ページ …) だけがある文、 戻る範囲 (1/3から1/2) は分数。
    ///   これらも 日付の手がかり (曜日の括弧 / 時刻 / まで・から・発売 / 範囲 `6/26〜7/2` / 直前の 本日・締切) が
    ///   あれば日付 (1/3(日) / 1/3 19:00)
    /// - 日付にならない組 (13/5 / 3/40) は分数 (分母が先)。 同じ数どうし (50/50) / 24/7 / 番号 (No.19/20) は読まない
    /// - 3 要素 (2024/10/12) の一部は読まない (年付きは `try_ymd`)
    fn try_slash_pair(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
        let Some(caps) = at_start(&SLASH_PAIR_RE, rest) else {
            return;
        };
        let whole = caps.get(0).unwrap();
        let (a, b) = (caps.get(1).unwrap().as_str(), caps.get(2).unwrap().as_str());
        let (Ok(x), Ok(y)) = (a.parse::<u32>(), b.parse::<u32>()) else {
            return;
        };
        let before = &input[..pos];
        let prev = before.chars().next_back();
        let after = &rest[whole.end()..];
        // 数字・スラッシュの途中 (2024/10/12 / 1/2/3) は触らない
        if prev.is_some_and(|c| c.is_ascii_digit() || c == '/' || c == '\u{2044}')
            || after.starts_with(|c: char| c.is_ascii_digit() || c == '/' || c == '\u{2044}')
        {
            return;
        }
        let fraction = format!("{}ブンノ{}", number_to_katakana(b), number_to_katakana(a));
        let date = || {
            format!(
                "{}{}",
                self.read_counter_in_date(a, "月"),
                self.read_counter_in_date(b, "日")
            )
        };
        let fraction_cue = SLASH_FRACTION_AFTER_RE.is_match(after)
            || extra_match(&self.slash_cues.fraction_after, after)
            || SLASH_FRACTION_BEFORE_RE.is_match(before)
            || extra_match(&self.slash_cues.fraction_before, before);
        // ½ ¾ のような 1 字の分数 (NFKC で U+2044 の分数スラッシュになる) は常に分数
        let vulgar = whole.as_str().contains('\u{2044}');
        let reading = if (fraction_cue || vulgar) && y > 0 {
            fraction
        } else if (1..=12).contains(&x) && (13..=31).contains(&y) {
            date()
        } else if (1..=12).contains(&x) && (1..=12).contains(&y) {
            // 分数に倒れやすい組。 日付の手がかりがあれば日付
            let fraction_leaning = (x == 1 && (2..=10).contains(&y))
                || (after_falls(after) || self.fraction_topic_only(input));
            if !fraction_leaning || (self.date_cue(before, after, x, y) && !after_falls(after)) {
                date()
            } else {
                fraction
            }
        } else if x > 0 && y > 0 && x != y && !(x == 24 && y == 7) && !is_serial_prefix(before) {
            fraction
        } else {
            return;
        };
        out.push(self.make(input, pos, whole.end(), reading));
    }

    /// 日付の手がかり: 直後 (曜日の括弧 / 時刻 / まで・から・発売 …)、 直前 (本日 / 締切 …)、 範囲 `6/26〜7/2` の両端
    fn date_cue(&self, before: &str, after: &str, x: u32, y: u32) -> bool {
        SLASH_DATE_AFTER_RE.is_match(after)
            || extra_match(&self.slash_cues.date_after, after)
            || SLASH_DATE_BEFORE_RE.is_match(before)
            || extra_match(&self.slash_cues.date_before, before)
            || date_range(before, after, x, y)
    }

    /// 分数の話題の語 (スコア / ページ …) があり、 日付の話題の語が無い文か
    fn fraction_topic_only(&self, input: &str) -> bool {
        (SLASH_SENTENCE_FRACTION_RE.is_match(input)
            || extra_match(&self.slash_cues.fraction_topic, input))
            && !SLASH_SENTENCE_DATE_RE.is_match(input)
            && !extra_match(&self.slash_cues.date_topic, input)
    }

    /// 数字なしの `km/h` / `m/s` (units に `/` 付きで登録した単位)。 語頭で、 直後が英数字でない時だけ。
    /// 数字つき (60km/h) は si_unit の regex が読む
    fn try_slash_unit(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        first_char: char,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        if self.slash_units.is_empty() || !first_char.is_ascii_alphabetic() {
            return;
        }
        // URL / パス / メールの一部 (a.com/km/h、 ?v=m/s、 foo@m/s) は読まない
        if input[..pos].chars().next_back().is_some_and(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '/' | '.' | '_' | '-' | ':' | '@' | '=' | '?' | '&' | '%' | '#' | '~'
                )
        }) {
            return;
        }
        for key in &self.slash_units {
            let Some(head) = rest.get(..key.len()) else {
                continue;
            };
            if !head.eq_ignore_ascii_case(key) {
                continue;
            }
            if rest[key.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-'))
            {
                continue;
            }
            if let Some(kana) = self.units.lookup(head) {
                out.push(self.make_hit(input, pos, key.len(), kana.to_string()));
                return;
            }
        }
    }

    /// section 8: 素の数字。
    fn try_digit(&self, input: &str, pos: usize, rest: &str, out: &mut Vec<RawCandidate<'_>>) {
        if let Some(m) = at_start(&DIGIT_RE, rest) {
            let m_end = m.get(0).unwrap().end();
            let num = m.get(0).unwrap().as_str();
            // 8 が 3 つ以上並ぶだけの `888` / `88888` は拍手 (パチパチ)。 数として読むと 「はっぴゃく…」 になる。
            // 前後が数字・英字・漢字・小数点・位取りの区切りではない時だけ (888円 は助数詞の候補が読む)
            if num.len() >= 3
                && num.bytes().all(|b| b == b'8')
                && !input[..pos]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || matches!(c, '.' | ',' | '，'))
                && !rest[m_end..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || matches!(c, '.' | ',' | '，'))
            {
                out.push(self.make_hit(input, pos, m_end, "パチ".repeat(num.len())));
                return;
            }
            out.push(self.make(input, pos, m_end, number_to_katakana(num)));
        }
    }
}

/// `1/3から1/2` のように、 まで / から の直後にもう 1 組の `N/M` が続く (戻る範囲の言い方)
fn after_falls(after: &str) -> bool {
    (after.starts_with("から") || after.starts_with("まで"))
        && at_start(&SLASH_PAIR_RE, &after["から".len()..]).is_some()
}

/// 番号の前置き (No.19/20 / #3/5 / 第3/5 / vol.1/2)
fn is_serial_prefix(before: &str) -> bool {
    before.ends_with(['#', '＃', '第'])
        || [
            "No.", "No", "NO", "no.", "vol.", "Vol.", "VOL.", "Ep.", "ep.", "EP.",
        ]
        .iter()
        .any(|p| before.ends_with(p))
}

/// 範囲 `N/M〜N/M` の両端は日付 (後ろ ≥ 前)。 1/3から1/2 のように戻る組は分数どうしなので除く
fn date_range(before: &str, after: &str, x: u32, y: u32) -> bool {
    let valid = |x: u32, y: u32| (1..=12).contains(&x) && (1..=31).contains(&y);
    let pair = |c: &regex::Captures<'_>| -> Option<(u32, u32)> {
        Some((
            c.get(1)?.as_str().parse().ok()?,
            c.get(2)?.as_str().parse().ok()?,
        ))
    };
    if SLASH_RANGE_AFTER_RE
        .captures(after)
        .and_then(|c| pair(&c))
        .is_some_and(|(x2, y2)| valid(x2, y2) && (x2, y2) >= (x, y))
    {
        return true;
    }
    SLASH_RANGE_BEFORE_RE
        .captures(before)
        .and_then(|c| pair(&c))
        .is_some_and(|(x0, y0)| valid(x0, y0) && (x, y) >= (x0, y0))
}

impl CandidateProvider for NumberCandidateProvider {
    fn candidates_at<'b>(
        &'b self,
        ctx: &ScoringContext<'b>,
        pos: usize,
        out: &mut Vec<RawCandidate<'b>>,
    ) {
        let input = ctx.input;
        let rest = &input[pos..];
        let Some(first_char) = rest.chars().next() else {
            return;
        };

        // section 0: 数詞慣用語句。 「明後日」 等 数字以外の先頭もあるため
        // numeric lead 判定より前に評価する (非 hit 位置は HashMap lookup 1 回)。
        self.try_phrase(input, pos, rest, first_char, out);
        self.try_currency_prefix(input, pos, rest, first_char, out);
        self.try_slash_unit(input, pos, rest, first_char, out);
        self.try_model(input, pos, first_char, out);

        // ─── 先頭文字 dispatch (hot path 最適化) ─────────────────────────────
        // 数値系正規表現は全て **数字系の先頭文字** を要求する (NUM_PAT = 任意符号 +
        // [0-9０-９]、 DATE/KANJI_NUM = 漢数字)。 先頭がそれ以外なら必ず空振りするので、
        // 記号系 (emit_symbol) だけ評価して即 return する。 これで全 byte 位置で 6+ regex
        // を試行する無駄を消す (dict prefix index 化で dict scan が消えた後の相対的
        // hot path)。 符号始まり (= 「-5本」) も拾うため digit-like に符号 5 種を加えた
        // 集合で判定。
        let numeric_lead = is_digit_like_char(first_char)
            || matches!(first_char, '+' | '-' | '\u{2212}' | '\u{FF0D}' | '\u{FF0B}');
        // 2026-09-11: 符号始まりでも **前が数字なら符号ではない**。
        // 3-1 / 22-0 / 2024-2025 / 7-80% のような区切りを 「マイナスイチ」 と
        // 読んでいた (実データ 704 万行のハイフンはスコア・面数・年範囲ばかりで、
        // 減算の例は無い)。 負数は 「-3」 「弾道 -3」 のように前が数字でない形。
        // 記号 edge (読み無し) だけ出し、 数字は次の位置で読ませる。
        let sign_is_separator = matches!(first_char, '+' | '-' | '\u{2212}' | '\u{FF0D}' | '\u{FF0B}')
                && input[..pos].chars().next_back().is_some_and(|c| {
                    is_digit_like_char(c)
                        || c.is_ascii_alphabetic()
                        || matches!(c, '時' | '分' | '秒')
                })
                // 1534年-1582年 / 20歳-30歳 の - も範囲の区切り (負数 -1582 ではない)
                || (matches!(first_char, '-' | '\u{2212}' | '\u{FF0D}')
                    && (hyphen_is_unit_range(input, pos, first_char)
                        || hyphen_is_date_range(input, pos, first_char)));
        if !numeric_lead || sign_is_separator {
            // 何百件 / 何千円 は 何 の位置から 1 候補にする (try_counter_kanji、 2026-09-28)
            if first_char == '何' {
                self.try_counter_kanji(input, pos, rest, out);
            }
            if first_char == '〒' {
                self.try_postal_mark(input, pos, rest, out);
            }
            // 価格:¥1,337 / 料金：$5 — 形態素解析は 「:¥」 を 1 つの記号 token にするので、 通貨記号の位置からは候補が使えない。
            // コロンから始めて通貨つきの数まで 1 つの候補にする (コロンは読みに残す)
            if matches!(
                first_char,
                ':' | '：' | '[' | '［' | '(' | '（' | '【' | '「' | '『' | '=' | '＝' | '<' | '＜'
            ) {
                let after = &rest[first_char.len_utf8()..];
                if let Some(next) = after.chars().next() {
                    let mut tmp = Vec::new();
                    self.try_currency_prefix(
                        input,
                        pos + first_char.len_utf8(),
                        after,
                        next,
                        &mut tmp,
                    );
                    for c in tmp {
                        let end = c.range.end - pos;
                        let reading = format!("{first_char}{}", c.reading);
                        out.push(self.make_hit(input, pos, end, reading));
                    }
                }
            }
            self.try_ascii_forms(input, pos, rest, first_char, out);
            self.emit_symbol(input, pos, rest, out);
            return;
        }

        // 適用順 = 優先順 (module doc の 1〜8 と対応)。
        // 先頭が算用数字 (と符号) か漢数字かで、 空振りが確定する matcher を飛ばす (どの matcher も先頭の
        // 字種を要求する regex で始まるので、 飛ばしても候補は変わらない。 並びの順序も保つ)。
        let arabic_lead = matches!(
            first_char,
            '0'..='9' | '０'..='９' | '+' | '-' | '\u{2212}' | '\u{FF0D}' | '\u{FF0B}'
        );
        self.try_date(input, pos, rest, out);
        if arabic_lead {
            self.try_time_jp(input, pos, rest, out);
            self.try_time_colon(input, pos, rest, out);
            self.try_scale(input, pos, rest, out);
            self.try_si_unit(input, pos, rest, out);
            self.try_fraction(input, pos, rest, out);
            self.try_phone(input, pos, rest, out);
            self.try_ymd(input, pos, rest, out);
            self.try_year_month(input, pos, rest, out);
            self.try_postal_after_label(input, pos, rest, out);
            self.try_slash_pair(input, pos, rest, out);
            self.try_version(input, pos, rest, out);
            self.try_counter(input, pos, rest, out);
        } else {
            self.try_time_jp_kanji(input, pos, rest, out);
            self.try_kanji_decimal(input, pos, rest, out);
            self.try_counter_kanji_grouped(input, pos, rest, out);
            self.try_counter_kanji(input, pos, rest, out);
            self.try_kanji_bare_number(input, pos, rest, out);
        }
        self.emit_symbol(input, pos, rest, out);
        if arabic_lead {
            self.try_digit(input, pos, rest, out);
        }
    }
}

/// 位取りで書いた 2 桁の漢数字 (三五 / 三〇 / 一七 = 35 / 30 / 17)。 2026-09-28
///
/// 並んだ数の概数 (二三 / 七八 = 2〜3 / 7〜8) と区別するため、 1 つ違いで上がる並びは除く
/// (`ordinal` = 直前が 第 の時は概数にならないので除かない: 第一二号 / 第二三条)。
/// 先頭が 〇 (〇〇 = まるまる / 〇一) も除く
fn two_digit_positional(num: &str, ordinal: bool) -> bool {
    let mut it = num.chars().map(kanji_digit);
    match (it.next(), it.next(), it.next()) {
        (Some(Some(a)), Some(Some(b)), None) => {
            let (a, b) = (a.to_digit(10).unwrap_or(0), b.to_digit(10).unwrap_or(0));
            a != 0 && (ordinal || b == 0 || b != a + 1)
        }
        _ => false,
    }
}

/// 1 つ違いで上がる 2 桁の漢数字 (二三 / 七八 / 八九) = 並んだ数の概数の形
fn approx_pair(num: &str) -> bool {
    let mut it = num.chars().map(kanji_digit);
    match (it.next(), it.next(), it.next()) {
        (Some(Some(a)), Some(Some(b)), None) => {
            let (a, b) = (a.to_digit(10).unwrap_or(0), b.to_digit(10).unwrap_or(0));
            a != 0 && b == a + 1
        }
        _ => false,
    }
}

/// 閉じ括弧か (`)` `]` `}` と全角・かぎ括弧・隅付き括弧)
fn is_closing_bracket(c: char) -> bool {
    matches!(
        c,
        ')' | ']' | '}' | '）' | '］' | '｝' | '」' | '』' | '】' | '〕' | '〉' | '》'
    )
}

/// 〇〜九 だけの列を桁を並べた算用数字に (一二 = 12)
fn positional_digits(num: &str) -> Option<String> {
    num.chars().map(kanji_digit).collect()
}

/// 1 つ違いの 2 桁 (一二 / 四五) を概数でなく数として読む文脈: 第 の後 (第一二号)、 ％ の前 (四五％)、 条 / 項 / 号 の前 (八九条)。
/// ％ の概数表記はほぼ無く、 国会・白書では 四五％ = 45% (2026-09-29)
fn ordinal_context(prev: Option<char>, base: &str) -> bool {
    // 条 / 項 / 号 の前は条番号 (民訴法八九条 = 89 条)。 概数の 二三条 はほぼ無い (2026-10-02)
    prev == Some('第') || matches!(base, "%" | "％" | "条" | "項" | "号")
}

/// 直前の字が 第 か
fn after_dai(input: &str, pos: usize) -> bool {
    input[..pos].ends_with('第')
}

/// 漢数字 1 字 (〇〜九) を算用数字 1 字に
fn kanji_digit(c: char) -> Option<char> {
    let i = "〇一二三四五六七八九".chars().position(|k| k == c)?;
    char::from_digit(u32::try_from(i).ok()?, 10)
}

/// 位取りのカンマつきの数 (`1,337` / `12,500,000`) を先頭から探す。 戻り値は (消費する byte 数, カンマを除いた数字)。
/// 1〜3 桁 + (`,` + ちょうど 3 桁) の繰り返しで、 カンマが 1 つも無ければ `None` (普通の数は別の経路が読む)
fn comma_grouped_number(s: &str) -> Option<(usize, String)> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && i < 3 && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i >= b.len() || b[i] != b',' {
        return None;
    }
    let mut digits = s[..i].to_string();
    let mut groups = 0;
    while i + 3 < b.len() && b[i] == b',' && b[i + 1..i + 4].iter().all(u8::is_ascii_digit) {
        digits.push_str(&s[i + 1..i + 4]);
        i += 4;
        groups += 1;
        if i >= b.len() {
            break;
        }
    }
    // 直後に数字が続く (¥1,3375) 形は位取りではない
    if groups == 0 || b.get(i).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    Some((i, digits))
}

/// emoji.toml の表を引き用に整える: 書かれたキーそのまま (肌色つき専用の読みなど) と、
/// VS16 / 肌色を除いた形 (⏰ と ⏰️ を同じに引く) の両方を持つ。 除いた形は、 同じキーが明示されていれば上書きしない
fn build_emoji_table(
    data: &crate::rules::EmojiData,
) -> std::collections::HashMap<String, crate::rules::emoji::EmojiEntry> {
    let mut table = data.entries.clone();
    for (k, v) in &data.entries {
        let stripped = crate::rules::emoji::strip_variation(k);
        table.entry(stripped).or_insert_with(|| v.clone());
    }
    table
}

impl NumberCandidateProvider {
    /// 和暦の略記を先頭から読む: `R7.10.13` (年.月.日、 区切りは `.` か `/`) と `R7年` / `R7年度` / `S50年代`。
    /// 戻り値は (消費する byte 数, 読み)。 年が元号の範囲を超える / 月日が正しくない / 直後に英数字が続く形は `None`
    /// (`R7` だけ・`R7.10` だけ は Radeon R7 や版番号と区別できないので読まない)
    fn era_abbreviation(&self, rest: &str) -> Option<(usize, String)> {
        let b = rest.as_bytes();
        let (name, max_year) = match b.first()? {
            b'M' => ("メイジ", 45),
            b'T' => ("タイショウ", 15),
            b'S' => ("ショウワ", 64),
            b'H' => ("ヘイセイ", 31),
            b'R' => ("レイワ", 99),
            _ => return None,
        };
        let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
        let ylen = digits(1);
        if !(1..=2).contains(&ylen) {
            return None;
        }
        let year: u32 = rest[1..1 + ylen].parse().ok()?;
        if year == 0 || year > max_year {
            return None;
        }
        let year_reading = if year == 1 {
            "ガンネン".to_string()
        } else {
            format!("{}ネン", number_to_katakana(&year.to_string()))
        };
        let after_year = 1 + ylen;
        // R7年 / R7年度 / S50年代
        if rest[after_year..].starts_with('年') {
            let mut end = after_year + '年'.len_utf8();
            let mut tail = "";
            if rest[end..].starts_with('度') {
                tail = "ド";
                end += '度'.len_utf8();
            } else if rest[end..].starts_with('代') {
                tail = "ダイ";
                end += '代'.len_utf8();
            }
            let reading = if tail.is_empty() {
                format!("{name}{year_reading}")
            } else {
                // 年度・年代の ネン は 「ネンド」 「ネンダイ」 (ガンネン + ド にはしない: 元年度は稀)
                format!("{name}{}ネン{tail}", number_to_katakana(&year.to_string()))
            };
            return Some((end, reading));
        }
        // R7.10.13 (同じ区切りで 3 つ)
        let sep = *b.get(after_year)?;
        if !matches!(sep, b'.' | b'/') {
            return None;
        }
        let mlen = digits(after_year + 1);
        if !(1..=2).contains(&mlen) {
            return None;
        }
        let m_end = after_year + 1 + mlen;
        if b.get(m_end) != Some(&sep) {
            return None;
        }
        let dlen = digits(m_end + 1);
        if !(1..=2).contains(&dlen) {
            return None;
        }
        let end = m_end + 1 + dlen;
        if b.get(end)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == sep)
        {
            return None;
        }
        let month: u32 = rest[after_year + 1..m_end].parse().ok()?;
        let day: u32 = rest[m_end + 1..end].parse().ok()?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        let reading = format!(
            "{name}{year_reading}{}{}",
            self.read_counter_in_date(&month.to_string(), "月"),
            self.read_counter_in_date(&day.to_string(), "日")
        );
        Some((end, reading))
    }
}

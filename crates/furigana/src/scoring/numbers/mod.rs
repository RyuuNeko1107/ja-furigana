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
mod context;
mod counter;
mod date;
mod math;
mod numeric;
mod patterns;
mod symbols;
mod time;

#[cfg(test)]
mod tests;

use self::context::hyphen_is_date_range;
use self::context::hyphen_is_unit_range;
use self::context::is_digit_like_char;
use self::context::trim_leading_zeros;
use self::context::BUILTIN_HONORIFICS;
use crate::numbers::{
    euphonic_counter_read, kansuji_to_arabic, kansuji_to_arabic_date, number_to_katakana,
};
use crate::rules::{CountersData, DaysData, RulesData, ScalesData, SymbolsData, UnitsData};
use crate::scoring::candidate::{
    CandidateProvider, RawCandidate, Score, ScoringContext, BAND_SPECIAL,
};
use crate::scoring::model_number::CompiledModels;
use patterns::{
    build_counter_regexes, build_grouped_kanji_counter_regex, build_scale_regex,
    build_si_unit_regex, HeadRegex,
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

pub(super) fn extra_match(re: &Option<Regex>, text: &str) -> bool {
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

// ─── 候補種別ごとの matcher (適用順は candidates_at が所有) ──────────────────

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
        self.try_math(input, pos, out);

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
            self.try_dot_date(input, pos, rest, out);
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

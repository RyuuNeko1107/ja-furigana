//! 数字 + 助数詞 / 大数スケール / SI 単位 の matcher
//!
//! `NumberCandidateProvider::candidates_at` (mod.rs) が適用順を持ち、 ここは各 `try_*` の中身だけ。

use super::context::approx_pair;
use super::context::is_closing_bracket;
use super::context::ordinal_context;
use super::context::positional_digits;
use super::context::scale_trailing_counter_suffix;
use super::context::two_digit_positional;
use super::patterns::{at_start, DIGIT_RE};
use super::NumberCandidateProvider;
use crate::numbers::{kansuji_to_arabic, number_to_katakana, scale_reading, si_unit_reading};
use crate::scoring::candidate::RawCandidate;

impl NumberCandidateProvider {
    /// section 6: 数値 + 単一助数詞 (+ optional 末尾再帰 「目」)。
    pub(super) fn try_counter(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
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
    pub(super) fn try_counter_kanji_grouped(
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
    pub(super) fn try_counter_kanji(
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
    pub(super) fn counter_kanji_reading(
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

    /// 助数詞 `counter` の `not_before` に直後の文字列 `after` が当たるか
    pub(super) fn counter_blocked(&self, counter: &str, after: &str) -> bool {
        self.counters
            .counter
            .get(counter)
            .is_some_and(|r| r.not_before.iter().any(|p| after.starts_with(p.as_str())))
    }

    /// section 4: 数値 + 大数スケール (+ 末尾漢字 unit)。
    pub(super) fn try_scale(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
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
    pub(super) fn try_si_unit(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
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
}

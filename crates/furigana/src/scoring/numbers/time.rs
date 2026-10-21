//! 時刻 (和式・コロン) の matcher
//!
//! `NumberCandidateProvider::candidates_at` (mod.rs) が適用順を持ち、 ここは各 `try_*` の中身だけ。

use super::context::is_digit_like_char;
use super::context::normalize_digits;
use super::patterns::{at_start, RATIO_RE, TIME_COLON_RE, TIME_JP_FULL_RE, TIME_JP_KANJI_RE};
use super::NumberCandidateProvider;
use crate::numbers::{kansuji_to_arabic, number_to_katakana};
use crate::scoring::candidate::RawCandidate;

impl NumberCandidateProvider {
    /// section 2: 和式時刻 (H時M分S秒 / H時M分 / H時)。
    pub(super) fn try_time_jp(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
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
    pub(super) fn try_time_jp_kanji(
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
    pub(super) fn try_time_colon(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
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
}

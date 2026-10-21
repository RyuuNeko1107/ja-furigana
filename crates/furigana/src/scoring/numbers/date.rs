//! 日付 (和式・ISO・スラッシュ・ドット・和暦略記) の matcher
//!
//! `NumberCandidateProvider::candidates_at` (mod.rs) が適用順を持ち、 ここは各 `try_*` の中身だけ。

use super::extra_match;
use super::patterns::{
    at_start, DATE_KANJI_FULL_RE, DATE_KANJI_MD_RE, DOT_DATE_WEEKDAY_RE, SLASH_DATE_AFTER_RE,
    SLASH_DATE_BEFORE_RE, SLASH_FRACTION_AFTER_RE, SLASH_FRACTION_BEFORE_RE, SLASH_PAIR_RE,
    SLASH_RANGE_AFTER_RE, SLASH_RANGE_BEFORE_RE, YMD_RE, YM_RE,
};
use super::NumberCandidateProvider;
use crate::numbers::number_to_katakana;
use crate::scoring::candidate::RawCandidate;

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

impl NumberCandidateProvider {
    /// section 1: 和式日付 (full → MD の優先順、 full が match したら MD は試さない)。
    pub(super) fn try_date(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
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

    /// 年付きの日付 (`2025/10/13` / `2000-10-12`)。 年 4 桁 + 月 1〜12 + 日 1〜31 で、 区切りは 2 つとも同じ。
    /// 数字・区切りの途中 (1/2/2025/10/13 の後半、 2025-10-13-1) や、 時刻・IP のような長い数字列の一部は読まない
    pub(super) fn try_ymd(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
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
    pub(super) fn try_year_month(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
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
    pub(super) fn try_slash_pair(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
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

    /// `10.14(水)` — 曜日の括弧が続く `月.日` は日付 (じゅうがつじゅうよっか)。 それまでは小数 (じゅうてんいちよん) だった。
    /// 数字・ドットの途中 (2026.10.14(水) / 1.10.14) は触らない
    pub(super) fn try_dot_date(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        let Some(caps) = at_start(&DOT_DATE_WEEKDAY_RE, rest) else {
            return;
        };
        let (a, b) = (caps.get(1).unwrap().as_str(), caps.get(2).unwrap().as_str());
        let (Ok(x), Ok(y)) = (a.parse::<u32>(), b.parse::<u32>()) else {
            return;
        };
        if !(1..=12).contains(&x) || !(1..=31).contains(&y) {
            return;
        }
        let end = caps.get(2).unwrap().end();
        if input[..pos]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_digit() || c == '.')
        {
            return;
        }
        let reading = format!(
            "{}{}",
            self.read_counter_in_date(a, "月"),
            self.read_counter_in_date(b, "日")
        );
        out.push(self.make(input, pos, end, reading));
    }

    /// 日付の手がかり: 直後 (曜日の括弧 / 時刻 / まで・から・発売 …)、 直前 (本日 / 締切 …)、 範囲 `6/26〜7/2` の両端
    pub(super) fn date_cue(&self, before: &str, after: &str, x: u32, y: u32) -> bool {
        SLASH_DATE_AFTER_RE.is_match(after)
            || extra_match(&self.slash_cues.date_after, after)
            || SLASH_DATE_BEFORE_RE.is_match(before)
            || extra_match(&self.slash_cues.date_before, before)
            || date_range(before, after, x, y)
    }

    /// 和暦の略記を先頭から読む: `R7.10.13` (年.月.日、 区切りは `.` か `/`) と `R7年` / `R7年度` / `S50年代`。
    /// 戻り値は (消費する byte 数, 読み)。 年が元号の範囲を超える / 月日が正しくない / 直後に英数字が続く形は `None`
    /// (`R7` だけ・`R7.10` だけ は Radeon R7 や版番号と区別できないので読まない)
    pub(super) fn era_abbreviation(&self, rest: &str) -> Option<(usize, String)> {
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

//! 記号・版番号・通貨・型番・数式・電話・郵便番号の matcher
//!
//! `NumberCandidateProvider::candidates_at` (mod.rs) が適用順を持ち、 ここは各 `try_*` の中身だけ。

use super::context::comma_grouped_number;
use super::context::hyphen_is_clock_range;
use super::context::hyphen_is_count_range;
use super::context::hyphen_is_date_range;
use super::context::hyphen_is_time_range;
use super::context::hyphen_is_unit_range;
use super::context::hyphen_is_year_range;
use super::context::is_digit_like_char;
use super::context::is_placeholder_shape;
use super::context::is_trailing_punctuation;
use super::context::looks_like_dotted_date;
use super::context::minus_is_sign;
use super::context::postal_digits;
use super::context::range_is_adjacent_small;
use super::context::range_marker_in_numeric_context;
use super::context::times_sign_reading;
use super::context::DECORATIVE_SHAPES;
use super::extra_match;
use super::math;
use super::patterns::{
    at_start, DIGIT_RE, PHONE_RE, POSTAL_RE, SLASH_SENTENCE_DATE_RE, SLASH_SENTENCE_FRACTION_RE,
    VERSION_RE,
};
use super::NumberCandidateProvider;
use crate::numbers::{number_to_katakana, symbol_char_reading};
use crate::scoring::candidate::RawCandidate;
use crate::scoring::digit_reading::DigitStyle;

impl NumberCandidateProvider {
    /// section 7: 記号 1 文字。 candidates_at から、 数値系を skip する非数値
    /// lead 経路と通常経路の双方から呼ぶ (= 記号判定は数値系の有無に依らず常に行う)。
    pub(super) fn emit_symbol(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
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
    pub(super) fn try_version(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
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
    pub(super) fn try_currency_prefix(
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
    pub(super) fn try_model(
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

    /// 数式 (`x²-5x+b=0` / `y=1/x` / `√2`)。 変数・数に `=` / 不等号 / 累乗 / `√` が付く範囲を 1 候補で読む
    pub(super) fn try_math(&self, input: &str, pos: usize, out: &mut Vec<RawCandidate<'_>>) {
        if let Some((end, reading)) = math::read_math(input, pos) {
            out.push(self.make_hit(input, pos, end - pos, reading));
        }
    }

    /// 電話番号 (`03-1234-5678` / `090 1234 5678` / `0120-123-456` / `+81-90-1234-5678` / `+1 555-123-4567` /
    /// `09012345678`)。 桁を 1 つずつ読み、 区切りの位置は間 (空白) にする。 数 (さんぜん…) としては読まない。
    /// 国内は 10〜11 桁、 国際 (`+` から) は 8〜15 桁 (E.164)
    pub(super) fn try_phone(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
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
    pub(super) fn try_ascii_forms(
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
    pub(super) fn try_postal_mark(
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
    pub(super) fn try_postal_after_label(
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

    /// 分数の話題の語 (スコア / ページ …) があり、 日付の話題の語が無い文か
    pub(super) fn fraction_topic_only(&self, input: &str) -> bool {
        (SLASH_SENTENCE_FRACTION_RE.is_match(input)
            || extra_match(&self.slash_cues.fraction_topic, input))
            && !SLASH_SENTENCE_DATE_RE.is_match(input)
            && !extra_match(&self.slash_cues.date_topic, input)
    }
}

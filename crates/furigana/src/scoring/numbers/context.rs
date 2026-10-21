//! 数字の前後の文脈を見る小さな判定関数 (範囲のハイフン・全角半角・序数・位取りなど)

use crate::numbers::kansuji_to_arabic;
use crate::rules::CountersData;
use crate::scoring::digit_reading::DigitStyle;

/// 先頭のゼロを落とす (`09` → `9`)。 全部ゼロ (`00`) ならそのまま
pub(super) fn trim_leading_zeros(s: &str) -> String {
    match s.trim_start_matches('0') {
        "" => s.to_string(),
        trimmed => trimmed.to_string(),
    }
}

/// 読み上げない終端記号 (= 記号候補に巻き取ってよい文字)。
/// `2026.1.16` のような 西暦 4 桁 + 月 + 日 のドット区切り。 バージョンとも日付とも取れるので
/// バージョン規則の対象にしない (従来どおり)。
pub(super) fn looks_like_dotted_date(s: &str) -> bool {
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

pub(super) fn is_trailing_punctuation(c: char) -> bool {
    matches!(c, '。' | '、' | '！' | '？' | '…' | '!' | '?' | ',' | '.')
}

/// 数値 + scale + 助数詞 (= 「1 万歩」 「3 千個」 等) の trailing counter から suffix
/// を引く (= last_digit に応じた連濁 / 促音化済の suffix string)。
///
/// `scale_reading` で 「イチマン」 等の本体を作った後、 末尾の counter 部分のみを
/// この helper で取得して append する用途。 SI units lookup で miss した時の
/// fallback 経路。
pub(super) fn scale_trailing_counter_suffix(
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
pub(super) fn range_marker_in_numeric_context(input: &str, pos: usize, ch: char) -> bool {
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
pub(super) fn hyphen_is_time_range(input: &str, pos: usize, ch: char) -> bool {
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
pub(super) fn hyphen_is_unit_range(input: &str, pos: usize, ch: char) -> bool {
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

pub(super) fn digit_value(c: char) -> Option<u32> {
    match c {
        '0'..='9' => Some(u32::from(c) - u32::from('0')),
        '\u{FF10}'..='\u{FF19}' => Some(u32::from(c) - u32::from('\u{FF10}')),
        _ => None,
    }
}

/// 文字列の末尾の数字 (最大 4 桁) の値と、 そのバイト長
pub(super) fn trailing_number(s: &str) -> Option<(u32, usize)> {
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
pub(super) fn leading_number(s: &str) -> Option<(u32, usize)> {
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
pub(super) fn hyphen_is_date_range(input: &str, pos: usize, ch: char) -> bool {
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
pub(super) const DECORATIVE_SHAPES: &str = "●■▲▼◆◇";

/// 郵便番号の 3 桁 + 4 桁を 1 桁ずつ読む (間に ノ)
pub(super) fn postal_digits(head: &str, tail: &str) -> String {
    format!(
        "{}ノ{}",
        DigitStyle::Each.render(head).unwrap_or_default(),
        DigitStyle::Each.render(tail).unwrap_or_default()
    )
}

/// 伏せ字の直後に付く敬称・肩書きの組み込み分 (辞書の `[placeholder] honorifics` で足せる)
pub(super) const BUILTIN_HONORIFICS: [&str; 8] =
    ["さん", "くん", "君", "ちゃん", "様", "氏", "先生", "殿"];

/// 図形記号が伏せ字として使われているか: ちょうど 2 つ並ぶ (`■■`) か、 直後が敬称 (`●さん`)。
/// 1 つだけ / 3 つ以上並ぶ装飾 (`★★★お知らせ★★★`) は伏せ字ではない
pub(super) fn is_placeholder_shape(
    input: &str,
    pos: usize,
    ch: char,
    honorifics: &[String],
) -> bool {
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
pub(super) fn times_sign_reading(input: &str, pos: usize, ch: char) -> &'static str {
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
pub(super) fn hyphen_is_clock_range(input: &str, pos: usize, ch: char) -> bool {
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
pub(super) fn hyphen_is_year_range(input: &str, pos: usize, ch: char) -> bool {
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
pub(super) fn range_is_adjacent_small(input: &str, pos: usize, ch: char) -> bool {
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
pub(super) fn hyphen_is_count_range(input: &str, pos: usize, ch: char) -> bool {
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
pub(super) fn minus_is_sign(input: &str, pos: usize, ch: char) -> bool {
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
pub(super) fn normalize_digits(s: &str) -> String {
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
pub(super) fn is_digit_like_char(c: char) -> bool {
    matches!(c,
        '0'..='9' | '０'..='９' |
        '〇' | '零' |
        '一' | '二' | '三' | '四' | '五' |
        '六' | '七' | '八' | '九' | '十' |
        '百' | '千' | '万' | '億' | '兆'
    )
}

/// 位取りで書いた 2 桁の漢数字 (三五 / 三〇 / 一七 = 35 / 30 / 17)。 2026-09-28
///
/// 並んだ数の概数 (二三 / 七八 = 2〜3 / 7〜8) と区別するため、 1 つ違いで上がる並びは除く
/// (`ordinal` = 直前が 第 の時は概数にならないので除かない: 第一二号 / 第二三条)。
/// 先頭が 〇 (〇〇 = まるまる / 〇一) も除く
pub(super) fn two_digit_positional(num: &str, ordinal: bool) -> bool {
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
pub(super) fn approx_pair(num: &str) -> bool {
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
pub(super) fn is_closing_bracket(c: char) -> bool {
    matches!(
        c,
        ')' | ']' | '}' | '）' | '］' | '｝' | '」' | '』' | '】' | '〕' | '〉' | '》'
    )
}

/// 〇〜九 だけの列を桁を並べた算用数字に (一二 = 12)
pub(super) fn positional_digits(num: &str) -> Option<String> {
    num.chars().map(kanji_digit).collect()
}

/// 1 つ違いの 2 桁 (一二 / 四五) を概数でなく数として読む文脈: 第 の後 (第一二号)、 ％ の前 (四五％)、 条 / 項 / 号 の前 (八九条)。
/// ％ の概数表記はほぼ無く、 国会・白書では 四五％ = 45% (2026-09-29)
pub(super) fn ordinal_context(prev: Option<char>, base: &str) -> bool {
    // 条 / 項 / 号 の前は条番号 (民訴法八九条 = 89 条)。 概数の 二三条 はほぼ無い (2026-10-02)
    prev == Some('第') || matches!(base, "%" | "％" | "条" | "項" | "号")
}

/// 直前の字が 第 か
pub(super) fn after_dai(input: &str, pos: usize) -> bool {
    input[..pos].ends_with('第')
}

/// 漢数字 1 字 (〇〜九) を算用数字 1 字に
pub(super) fn kanji_digit(c: char) -> Option<char> {
    let i = "〇一二三四五六七八九".chars().position(|k| k == c)?;
    char::from_digit(u32::try_from(i).ok()?, 10)
}

/// 位取りのカンマつきの数 (`1,337` / `12,500,000`) を先頭から探す。 戻り値は (消費する byte 数, カンマを除いた数字)。
/// 1〜3 桁 + (`,` + ちょうど 3 桁) の繰り返しで、 カンマが 1 つも無ければ `None` (普通の数は別の経路が読む)
pub(super) fn comma_grouped_number(s: &str) -> Option<(usize, String)> {
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

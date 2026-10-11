//! 英字語に直接くっついた数字 (`Windows11` / `RTX4090` / `iPhone15`) の読み方。
//!
//! 数字の読みは語ごとに違う (Windows11 = イレブン / RTX4090 = 1 桁ずつ / iPhone15 = じゅうご)。
//! loanwords の `[digits]` table で 「語 → 読み方の種類」 を宣言し、 本 module が
//! 種類ごとに数字列をカタカナにする。 未登録の語は従来どおり素通し。
//!
//! 種類 ([`DigitStyle`]):
//! - `place`: 位取り (15 = ジュウゴ)。 5 桁以上 / 先頭 0 は `each`
//! - `each`: 1 桁ずつ日本語 (4090 = ヨンゼロキュウゼロ)
//! - `each_en`: 1 桁ずつ英語 (4090 = フォーゼロナインゼロ)
//! - `english`: 英語の数詞 0〜99 (11 = イレブン)。 範囲外は `each_en`
//! - `pair`: 下から 2 桁ずつ位取り (4090 = ヨンジュウキュウジュウ)
//! - `pair_en`: 下から 2 桁ずつ英語 (4090 = フォーティーナインティ)

/// 数字列の読み方の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigitStyle {
    Place,
    Each,
    EachEn,
    English,
    Pair,
    PairEn,
}

impl DigitStyle {
    /// `[digits]` table の値から種類を引く。 未知なら `None` (load 時にエラーにする)。
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "place" => Self::Place,
            "each" => Self::Each,
            "each_en" => Self::EachEn,
            "english" => Self::English,
            "pair" => Self::Pair,
            "pair_en" => Self::PairEn,
            _ => return None,
        })
    }

    /// ASCII 数字だけの列をカタカナで読む。 空 / 数字以外を含めば `None`。
    #[must_use]
    pub fn render(self, digits: &str) -> Option<String> {
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(match self {
            Self::Place => place(digits).unwrap_or_else(|| each(digits, &JA_DIGITS)),
            Self::Each => each(digits, &JA_DIGITS),
            Self::EachEn => each(digits, &EN_DIGITS),
            Self::English => english(digits).unwrap_or_else(|| each(digits, &EN_DIGITS)),
            Self::Pair => pairs(digits, |c| place(c).unwrap_or_else(|| each(c, &JA_DIGITS))),
            Self::PairEn => pairs(digits, |c| {
                english(c).unwrap_or_else(|| each(c, &EN_DIGITS))
            }),
        })
    }
}

const JA_DIGITS: [&str; 10] = [
    "ゼロ",
    "イチ",
    "ニ",
    "サン",
    "ヨン",
    "ゴ",
    "ロク",
    "ナナ",
    "ハチ",
    "キュウ",
];
const EN_DIGITS: [&str; 10] = [
    "ゼロ",
    "ワン",
    "ツー",
    "スリー",
    "フォー",
    "ファイブ",
    "シックス",
    "セブン",
    "エイト",
    "ナイン",
];
const EN_TEENS: [&str; 10] = [
    "テン",
    "イレブン",
    "トゥエルブ",
    "サーティーン",
    "フォーティーン",
    "フィフティーン",
    "シックスティーン",
    "セブンティーン",
    "エイティーン",
    "ナインティーン",
];
const EN_TENS: [&str; 10] = [
    "",
    "",
    "トゥエンティー",
    "サーティー",
    "フォーティー",
    "フィフティー",
    "シックスティー",
    "セブンティー",
    "エイティー",
    "ナインティー",
];

fn each(digits: &str, table: &[&str; 10]) -> String {
    digits
        .bytes()
        .map(|b| table[usize::from(b - b'0')])
        .collect()
}

/// 位取り (1〜4 桁、 先頭 0 なし)。 範囲外は `None`。
fn place(digits: &str) -> Option<String> {
    if digits.len() > 4 || (digits.len() > 1 && digits.starts_with('0')) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    if n == 0 {
        return Some("ゼロ".to_string());
    }
    let (th, hu, te, on) = (n / 1000, n / 100 % 10, n / 10 % 10, n % 10);
    let mut out = String::new();
    out.push_str(match th {
        0 => "",
        1 => "セン",
        3 => "サンゼン",
        8 => "ハッセン",
        d => return Some(format!("{}セン", JA_DIGITS[d as usize]) + &place_rest(hu, te, on)),
    });
    out.push_str(&place_rest(hu, te, on));
    Some(out)
}

fn place_rest(hu: u32, te: u32, on: u32) -> String {
    let mut out = String::new();
    out.push_str(match hu {
        0 => "",
        1 => "ヒャク",
        3 => "サンビャク",
        6 => "ロッピャク",
        8 => "ハッピャク",
        d => return format!("{}ヒャク", JA_DIGITS[d as usize]) + &place_tail(te, on),
    });
    out.push_str(&place_tail(te, on));
    out
}

fn place_tail(te: u32, on: u32) -> String {
    let mut out = String::new();
    match te {
        0 => {}
        1 => out.push_str("ジュウ"),
        d => {
            out.push_str(JA_DIGITS[d as usize]);
            out.push_str("ジュウ");
        }
    }
    if on > 0 {
        out.push_str(JA_DIGITS[on as usize]);
    }
    out
}

/// 英語の数詞 (0〜99、 先頭 0 なし)。 範囲外は `None`。
fn english(digits: &str) -> Option<String> {
    if digits.len() > 2 || (digits.len() > 1 && digits.starts_with('0')) {
        return None;
    }
    let n: usize = digits.parse().ok()?;
    Some(match n {
        0..=9 => EN_DIGITS[n].to_string(),
        10..=19 => EN_TEENS[n - 10].to_string(),
        _ => match n % 10 {
            0 => EN_TENS[n / 10].to_string(),
            one => format!("{}{}", EN_TENS[n / 10], EN_DIGITS[one]),
        },
    })
}

/// 下から 2 桁ずつ区切って `f` で読み、 連結する。
fn pairs(digits: &str, f: impl Fn(&str) -> String) -> String {
    let first = digits.len() % 2;
    let mut out = String::new();
    if first == 1 {
        out.push_str(&f(&digits[..1]));
    }
    let mut i = first;
    while i < digits.len() {
        out.push_str(&f(&digits[i..i + 2]));
        i += 2;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(style: &str, d: &str) -> String {
        DigitStyle::parse(style).unwrap().render(d).unwrap()
    }

    #[test]
    fn place_reads_positional() {
        assert_eq!(r("place", "15"), "ジュウゴ");
        assert_eq!(r("place", "10"), "ジュウ");
        assert_eq!(r("place", "300"), "サンビャク");
        assert_eq!(r("place", "1999"), "センキュウヒャクキュウジュウキュウ");
        assert_eq!(r("place", "0"), "ゼロ");
        // 先頭 0 / 5 桁以上は 1 桁ずつ
        assert_eq!(r("place", "05"), "ゼロゴ");
        assert_eq!(r("place", "12345"), "イチニサンヨンゴ");
    }

    #[test]
    fn each_and_each_en() {
        assert_eq!(r("each", "4090"), "ヨンゼロキュウゼロ");
        assert_eq!(r("each_en", "4090"), "フォーゼロナインゼロ");
    }

    #[test]
    fn english_numerals() {
        assert_eq!(r("english", "11"), "イレブン");
        assert_eq!(r("english", "10"), "テン");
        assert_eq!(r("english", "2"), "ツー");
        assert_eq!(r("english", "21"), "トゥエンティーワン");
        assert_eq!(r("english", "90"), "ナインティー");
        // 範囲外 / 先頭 0 は 1 桁ずつ英語
        assert_eq!(r("english", "100"), "ワンゼロゼロ");
        assert_eq!(r("english", "07"), "ゼロセブン");
    }

    #[test]
    fn pair_styles() {
        assert_eq!(r("pair", "4090"), "ヨンジュウキュウジュウ");
        assert_eq!(r("pair_en", "4090"), "フォーティーナインティー");
        assert_eq!(r("pair_en", "1060"), "テンシックスティー");
        // 奇数桁は先頭が 1 桁
        assert_eq!(r("pair_en", "750"), "セブンフィフティー");
    }

    #[test]
    fn rejects_non_digits_and_unknown_style() {
        assert!(DigitStyle::parse("nope").is_none());
        assert!(DigitStyle::Place.render("").is_none());
        assert!(DigitStyle::Place.render("1a").is_none());
    }
}

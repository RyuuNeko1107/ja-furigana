//! 数値処理 module 内専用の小ヘルパ
//!
//! 公開 API ではない (`pub(crate)`)。`number_to_katakana` 等から呼ばれる。

/// 全角英数字・記号 → 半角 (本 module 内専用、[`crate::kana::zen_to_han`] の縮小版)
pub(crate) fn zen2han(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c),
            '．' => '.',
            '，' => ',',
            '％' => '%',
            '＋' => '+',
            '－' | '\u{2212}' | '\u{2013}' | '\u{2014}' => '-',
            '〜' | '～' => '~',
            '／' => '/',
            _ => c,
        })
        .collect()
}

/// `zen2han` した上でカンマを除去
pub(crate) fn norm_num(s: &str) -> String {
    zen2han(s).replace(',', "")
}

/// 漢数字 (一〜九十、数十、十数、二十一 等) を Arabic 数字文字列に変換する。
///
/// サポート範囲は 0〜99 (日付・月の用途で十分)。複雑なケース (百二十三 等) は
/// 未対応で `None` を返す。
///
/// 主に [`crate::scoring::numbers`] (NumberCandidateProvider) の日付/月日処理から
/// 呼ばれ、 「6月**一**日」「**二**月**十**日」のような漢数字混在パターンを
/// `read_counter` に投入できるようにするのが目的。
pub(crate) fn kansuji_to_arabic(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    match chars.len() {
        // 単漢字: 一〜九 / 十
        1 => digit_of_kansuji(chars[0]).map(|d| d.to_string()),
        // 二字: 「十X」「X十」「二十」「数十」 等
        2 => match (chars[0], chars[1]) {
            // 「十X」 = 10 + X (例: 十一=11)
            ('十', c) => digit_of_kansuji(c).map(|d| format!("1{d}")),
            // 「X十」 = X * 10 (例: 二十=20、九十=90)
            (c, '十') => digit_of_kansuji(c).map(|d| format!("{d}0")),
            _ => None,
        },
        // 三字: 「X十Y」 = X*10 + Y (例: 二十一=21)
        3 => match (chars[0], chars[1], chars[2]) {
            (a, '十', b) => match (digit_of_kansuji(a), digit_of_kansuji(b)) {
                (Some(da), Some(db)) if (1..=9).contains(&da) && (1..=9).contains(&db) => {
                    Some(format!("{da}{db}"))
                }
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// 漢数字 1 文字 → 0〜10 の int
fn digit_of_kansuji(c: char) -> Option<u8> {
    match c {
        '〇' | '零' => Some(0),
        '一' => Some(1),
        '二' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        '十' => Some(10),
        _ => None,
    }
}

/// 数値文字列 → i64 (全角・カンマ対応、不正なら `None`)
pub(crate) fn to_int(s: &str) -> Option<i64> {
    norm_num(s).parse::<i64>().ok()
}

/// 文字列末尾の 1 桁を返す。数字が無ければ `0`。
pub(crate) fn last_digit(s: &str) -> u32 {
    let norm = norm_num(s);
    for ch in norm.chars().rev() {
        if ch.is_ascii_digit() {
            return ch.to_digit(10).unwrap_or(0);
        }
    }
    0
}

/// カタカナ末尾を促音化 (イチ→イッ、ロク→ロッ、ハチ→ハッ、ジュウ→ジュッ)。
/// 該当しなければそのまま返す。
pub(crate) fn sokuonize_last(num_kata: &str) -> String {
    for (src, dst) in &[
        ("イチ", "イッ"),
        ("ロク", "ロッ"),
        ("ハチ", "ハッ"),
        ("ジュウ", "ジュッ"),
    ] {
        if let Some(stripped) = num_kata.strip_suffix(src) {
            return format!("{stripped}{dst}");
        }
    }
    num_kata.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zen2han_works() {
        assert_eq!(zen2han("１２３"), "123");
        assert_eq!(zen2han("１，２３４"), "1,234");
        assert_eq!(zen2han("－５"), "-5");
    }

    #[test]
    fn norm_num_strips_commas() {
        assert_eq!(norm_num("1,234,567"), "1234567");
        assert_eq!(norm_num("１，２３４"), "1234");
    }

    #[test]
    fn to_int_handles_zenkaku() {
        assert_eq!(to_int("１２３"), Some(123));
        assert_eq!(to_int("-５"), Some(-5));
        assert_eq!(to_int("abc"), None);
    }

    #[test]
    fn last_digit_works() {
        assert_eq!(last_digit("123"), 3);
        assert_eq!(last_digit("100"), 0);
        assert_eq!(last_digit("1,234"), 4);
        assert_eq!(last_digit("abc"), 0);
    }

    #[test]
    fn sokuonize_last_works() {
        assert_eq!(sokuonize_last("イチ"), "イッ");
        assert_eq!(sokuonize_last("ロク"), "ロッ");
        assert_eq!(sokuonize_last("ハチ"), "ハッ");
        assert_eq!(sokuonize_last("ジュウ"), "ジュッ");
        assert_eq!(sokuonize_last("ニ"), "ニ");
    }
}

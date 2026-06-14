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

/// 漢数字 (一〜九、十百千万億 の additive 表記) を Arabic 数字文字列に変換する。
///
/// 例: 「三百」=300、「二十一」=21、「千二百三十四」=1234、「一万」=10000。
/// `〇`/`零` を含む **positional 表記** (例: 「二〇二五」) は体系が異なるため対象外で
/// `None` を返す (単独 `〇`/`零` のみ 0)。 非漢数字・桁あふれも `None`。
///
/// 主に [`crate::scoring::numbers`] (NumberCandidateProvider) の日付/月日・助数詞処理
/// から呼ばれる。 None 時に literal 漢数字が `number_to_katakana` (Arabic 入力前提) に
/// 漏れて化ける経路があったため (例: 旧実装で 「三百回目」→「三百かいめ」)、 百/千/万を
/// 正しく解けるよう一般化した。
pub(crate) fn kansuji_to_arabic(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let has_unit = chars
        .iter()
        .any(|&c| matches!(c, '十' | '百' | '千' | '万' | '億'));
    // 〇/零 を含み単位を含まないものは positional 表記 (二〇二五 = 2025)。
    // 各文字を桁として連結する。 〇 と単位の混在 (非標準) は解さず None。
    if chars.iter().any(|&c| matches!(c, '〇' | '零')) {
        if has_unit {
            return None;
        }
        let mut out = String::with_capacity(chars.len());
        for &c in &chars {
            let d = match c {
                '〇' | '零' => 0u8,
                _ => digit_of_kansuji(c).filter(|&d| (1..=9).contains(&d))?,
            };
            out.push(char::from(b'0' + d));
        }
        return Some(out);
    }

    // additive: current=単位直前の桁、 section=万未満の累積、 total=万以上の累積。
    let mut total: u64 = 0;
    let mut section: u64 = 0;
    let mut current: u64 = 0;
    for &c in &chars {
        if let Some(d @ 1..=9) = digit_of_kansuji(c) {
            current = u64::from(d);
            continue;
        }
        let unit: u64 = match c {
            '十' => 10,
            '百' => 100,
            '千' => 1000,
            '万' => 10_000,
            '億' => 100_000_000,
            _ => return None, // 不明文字
        };
        if unit >= 10_000 {
            // 万/億: ここまでの section+current を 1 グループとして繰り上げる
            let group = section.checked_add(current)?;
            let group = if group == 0 { 1 } else { group };
            total = total.checked_add(group.checked_mul(unit)?)?;
            section = 0;
            current = 0;
        } else {
            // 十/百/千: 直前桁 (無ければ 1) × 単位を section に積む
            let n = if current == 0 { 1 } else { current };
            section = section.checked_add(n.checked_mul(unit)?)?;
            current = 0;
        }
    }
    let result = total.checked_add(section)?.checked_add(current)?;
    Some(result.to_string())
}

/// 漢数字 1 文字 (一〜九) → 1〜9 の int。
///
/// 0 (`〇`/`零`) は positional 分岐、 単位 (`十`/`百`/…) は kansuji_to_arabic 側で
/// 個別処理するため、 ここでは 1〜9 のみ扱う (呼び出し側はいずれも 1..=9 のみ使用)。
fn digit_of_kansuji(c: char) -> Option<u8> {
    match c {
        '一' => Some(1),
        '二' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
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
        // 全角記号の各変換を網羅 (. , % + - ~ /)。各 arm を個別に固定。
        assert_eq!(zen2han("．"), ".");
        assert_eq!(zen2han("％"), "%");
        assert_eq!(zen2han("＋"), "+");
        assert_eq!(zen2han("〜"), "~");
        assert_eq!(zen2han("～"), "~");
        assert_eq!(zen2han("／"), "/");
        // ダッシュ各種 → '-'
        assert_eq!(zen2han("\u{2212}\u{2013}\u{2014}"), "---");
        // 非対象文字は不変
        assert_eq!(zen2han("漢ア"), "漢ア");
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
    fn kansuji_to_arabic_handles_hundreds_and_above() {
        // ≥100 の漢数字 (百/千/万) を正しく additive parse する。
        assert_eq!(kansuji_to_arabic("三百").as_deref(), Some("300"));
        assert_eq!(kansuji_to_arabic("百").as_deref(), Some("100"));
        assert_eq!(kansuji_to_arabic("三百二十一").as_deref(), Some("321"));
        assert_eq!(kansuji_to_arabic("千").as_deref(), Some("1000"));
        assert_eq!(kansuji_to_arabic("千二百").as_deref(), Some("1200"));
        assert_eq!(kansuji_to_arabic("一千二百").as_deref(), Some("1200"));
        assert_eq!(kansuji_to_arabic("一万").as_deref(), Some("10000"));
        assert_eq!(
            kansuji_to_arabic("一万二千三百四十五").as_deref(),
            Some("12345")
        );
        assert_eq!(kansuji_to_arabic("一億").as_deref(), Some("100000000"));
        // 単位のみ (直前桁無し → 暗黙の 1) も解く (group==0 分岐)。
        assert_eq!(kansuji_to_arabic("万").as_deref(), Some("10000"));
        assert_eq!(kansuji_to_arabic("億").as_deref(), Some("100000000"));
    }

    #[test]
    fn kansuji_to_arabic_covers_all_digits() {
        // 一〜九 を全て個別に固定 (digit_of_kansuji の各 arm を網羅)。
        for (k, n) in [
            ("一", "1"),
            ("二", "2"),
            ("三", "3"),
            ("四", "4"),
            ("五", "5"),
            ("六", "6"),
            ("七", "7"),
            ("八", "8"),
            ("九", "9"),
        ] {
            assert_eq!(kansuji_to_arabic(k).as_deref(), Some(n), "digit {k}");
        }
    }

    #[test]
    fn kansuji_to_arabic_preserves_0_to_99() {
        // 既存 0-99 挙動を維持 (date/月日 経路の回帰防止)。
        assert_eq!(kansuji_to_arabic("一").as_deref(), Some("1"));
        assert_eq!(kansuji_to_arabic("九").as_deref(), Some("9"));
        assert_eq!(kansuji_to_arabic("十").as_deref(), Some("10"));
        assert_eq!(kansuji_to_arabic("十一").as_deref(), Some("11"));
        assert_eq!(kansuji_to_arabic("二十").as_deref(), Some("20"));
        assert_eq!(kansuji_to_arabic("二十一").as_deref(), Some("21"));
        assert_eq!(kansuji_to_arabic("三十").as_deref(), Some("30"));
        assert_eq!(kansuji_to_arabic("三十一").as_deref(), Some("31"));
        assert_eq!(kansuji_to_arabic("零").as_deref(), Some("0"));
    }

    #[test]
    fn kansuji_to_arabic_handles_positional_zero_notation() {
        // 〇 positional 表記 (二〇二五 = 2025) を桁ごとに連結して解く。
        assert_eq!(kansuji_to_arabic("二〇二五").as_deref(), Some("2025"));
        assert_eq!(kansuji_to_arabic("一九九〇").as_deref(), Some("1990"));
        assert_eq!(kansuji_to_arabic("〇").as_deref(), Some("0"));
        assert_eq!(kansuji_to_arabic("零").as_deref(), Some("0"));
    }

    #[test]
    fn kansuji_to_arabic_rejects_invalid() {
        // 〇 と単位の混在 (非標準) は解さない。
        assert_eq!(kansuji_to_arabic("千〇"), None);
        // 非漢数字・空は None。
        assert_eq!(kansuji_to_arabic("あ"), None);
        assert_eq!(kansuji_to_arabic(""), None);
        assert_eq!(kansuji_to_arabic("三X"), None);
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

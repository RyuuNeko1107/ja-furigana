//! 数値文字列 → カタカナ読み (data 非依存の純粋アルゴリズム)
//!
//! 全角→半角・カンマ除去・符号 (+/−/±)・小数点 (.) に対応。
//! 大数スケールは `万 / 億 / 兆 / 京 / 垓 / 秭 / 穣 / 溝 / 澗` (10^36) まで内蔵。
//!
//! 兆・万等の **連濁** はここでは適用しない (`scale_reading` 等で別途処理)。

use super::helpers::{norm_num, zen2han};

/// 0〜9999 のカタカナ読み (4 桁単位の内訳)
fn num_under_10000(n: u64) -> String {
    const ICHI: &[&str] = &[
        "",
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
    const JUU: &[&str] = &[
        "",
        "ジュウ",
        "ニジュウ",
        "サンジュウ",
        "ヨンジュウ",
        "ゴジュウ",
        "ロクジュウ",
        "ナナジュウ",
        "ハチジュウ",
        "キュウジュウ",
    ];
    const HYAKU: &[&str] = &[
        "",
        "ヒャク",
        "ニヒャク",
        "サンビャク",
        "ヨンヒャク",
        "ゴヒャク",
        "ロッピャク",
        "ナナヒャク",
        "ハッピャク",
        "キュウヒャク",
    ];
    const SEN: &[&str] = &[
        "",
        "セン",
        "ニセン",
        "サンゼン",
        "ヨンセン",
        "ゴセン",
        "ロクセン",
        "ナナセン",
        "ハッセン",
        "キュウセン",
    ];

    let s = SEN[((n / 1000) % 10) as usize];
    let h = HYAKU[((n / 100) % 10) as usize];
    let j = JUU[((n / 10) % 10) as usize];
    let i = ICHI[(n % 10) as usize];
    format!("{s}{h}{j}{i}")
}

/// 大数スケール (10^4 刻み) — 万から澗 (10^36) まで。
/// `u128::MAX ≈ 3.4×10^38` なので、これ以上のスケールは扱わない。
const SCALE_UNITS: &[(&str, u128)] = &[
    ("カン", 10_u128.pow(36)),
    ("コウ", 10_u128.pow(32)),
    ("ジョウ", 10_u128.pow(28)),
    ("ジョ", 10_u128.pow(24)),
    ("ガイ", 10_u128.pow(20)),
    ("ケイ", 10_u128.pow(16)),
    ("チョウ", 10_u128.pow(12)),
    ("オク", 10_u128.pow(8)),
    ("マン", 10_u128.pow(4)),
];

/// 任意の数値文字列をカタカナ読みに変換する。
///
/// - 全角→半角・カンマ除去・符号 (+/−/±)・小数点 (.) に対応
/// - 不正な形式は元の文字列を [`crate::kana::hira_to_kata`] したものを返す
#[must_use]
pub fn number_to_katakana(num_str: &str) -> String {
    let s = norm_num(num_str);

    // ─── 符号処理 ──────────────────────────────────────────────────────────
    let (sign_read, s) = if let Some(rest) = s.strip_prefix('±') {
        ("プラスマイナス", rest.to_string())
    } else if let Some(rest) = s.strip_prefix('+') {
        ("プラス", rest.to_string())
    } else if let Some(rest) = s.strip_prefix('-') {
        ("マイナス", rest.to_string())
    } else {
        ("", s)
    };

    // ─── 不正な形式の早期 return ────────────────────────────────────────────
    let dot_count = s.chars().filter(|&c| c == '.').count();
    if dot_count > 1 || !s.replace('.', "").chars().all(|c| c.is_ascii_digit()) || s.is_empty() {
        let fallback = crate::kana::hira_to_kata(&zen2han(num_str));
        return format!("{sign_read}{fallback}");
    }

    // 先頭 . → 0. に補完
    let s = if s.starts_with('.') {
        format!("0{s}")
    } else {
        s
    };

    let (int_part, frac) = if let Some(idx) = s.find('.') {
        (&s[..idx], Some(&s[idx + 1..]))
    } else {
        (s.as_str(), None)
    };

    // ─── 整数部 ───────────────────────────────────────────────────────────
    let int_read = if int_part.is_empty() || int_part == "0" {
        "ゼロ".to_string()
    } else {
        let Ok(n) = int_part.parse::<u128>() else {
            let fallback = crate::kana::hira_to_kata(&zen2han(num_str));
            return format!("{sign_read}{fallback}");
        };
        if n == 0 {
            "ゼロ".to_string()
        } else {
            let mut parts = Vec::new();
            let mut rem = n;
            for &(label, base) in SCALE_UNITS {
                let q = rem / base;
                rem %= base;
                if q > 0 {
                    parts.push(format!("{}{}", num_under_10000(q as u64), label));
                }
            }
            if rem > 0 {
                parts.push(num_under_10000(rem as u64));
            }
            if parts.is_empty() {
                "ゼロ".to_string()
            } else {
                parts.join("")
            }
        }
    };

    // ─── 小数部 ───────────────────────────────────────────────────────────
    let frac_read = match frac {
        Some(f) if !f.is_empty() => {
            const DIGIT: &[&str] = &[
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
            let digits: String = f
                .chars()
                .filter_map(|c| c.to_digit(10).map(|d| DIGIT[d as usize]))
                .collect::<Vec<_>>()
                .join("");
            format!("テン{digits}")
        }
        _ => String::new(),
    };

    format!("{sign_read}{int_read}{frac_read}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_to_katakana_edges() {
        // 小数 (テン 区切り)
        assert_eq!(number_to_katakana("1.5"), "イチテンゴ");
        // ゼロ群を含む桁上がり (空の百/十群を「ゼロ」で埋めない)
        assert_eq!(number_to_katakana("10001"), "イチマンイチ");
        assert_eq!(number_to_katakana("100010000"), "イチオクイチマン");
        // 不正形式 (複数ドット) は passthrough fallback
        assert_eq!(number_to_katakana("1.2.3"), "1.2.3");
        // 末尾ドット (空の小数部) は小数部なし
        assert_eq!(number_to_katakana("1."), "イチ");
    }

    #[test]
    fn basic() {
        assert_eq!(number_to_katakana("0"), "ゼロ");
        assert_eq!(number_to_katakana("1"), "イチ");
        assert_eq!(number_to_katakana("10"), "ジュウ");
        assert_eq!(number_to_katakana("100"), "ヒャク");
        assert_eq!(number_to_katakana("300"), "サンビャク");
        assert_eq!(number_to_katakana("600"), "ロッピャク");
        assert_eq!(number_to_katakana("800"), "ハッピャク");
        assert_eq!(number_to_katakana("1000"), "セン");
        assert_eq!(number_to_katakana("3000"), "サンゼン");
        assert_eq!(number_to_katakana("8000"), "ハッセン");
    }

    #[test]
    fn large_no_sokuon_here() {
        assert_eq!(number_to_katakana("10000"), "イチマン");
        assert_eq!(number_to_katakana("100000000"), "イチオク");
        // number_to_katakana 単体では兆の促音化は適用しない
        // (兆は scale_reading 側で「イッチョウ」になる)
        assert_eq!(number_to_katakana("1000000000000"), "イチチョウ");
    }

    #[test]
    fn decimal() {
        assert_eq!(number_to_katakana("3.14"), "サンテンイチヨン");
        assert_eq!(number_to_katakana(".5"), "ゼロテンゴ");
    }

    #[test]
    fn signs() {
        assert_eq!(number_to_katakana("-10"), "マイナスジュウ");
        assert_eq!(number_to_katakana("+5"), "プラスゴ");
        assert_eq!(number_to_katakana("±3"), "プラスマイナスサン");
    }

    #[test]
    fn fullwidth() {
        assert_eq!(number_to_katakana("１２３"), "ヒャクニジュウサン");
    }

    #[test]
    fn with_commas() {
        assert_eq!(number_to_katakana("1,234"), "センニヒャクサンジュウヨン");
    }
}

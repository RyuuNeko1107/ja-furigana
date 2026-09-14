//! 補助変換: スケール / SI 単位 / 記号の単発読み
//!
//! [`crate::scoring::numbers`] (NumberCandidateProvider) から呼ばれる、
//! 引数を受けて 1 つの読みを返す軽量関数群。

use super::digit::number_to_katakana;
use super::helpers::{last_digit, sokuonize_last};
use crate::rules::{ScalesData, SymbolsData, UnitsData};

/// 記号 1 文字の読みを引く (全角/半角を正規化してから lookup)
#[must_use]
pub fn symbol_char_reading(ch: char, symbols: &SymbolsData) -> Option<String> {
    let normalized = match ch {
        '＋' => '+',
        '－' | '\u{2212}' => '-',
        '％' => '%',
        '／' => '/',
        _ => ch,
    };
    symbols.lookup_char(normalized).map(ToString::to_string)
}

/// 数値 + SI 単位 → カタカナ読み
///
/// 単位が `units` に無ければ数値カナのみ返す (単位部は脱落)。
#[must_use]
pub fn si_unit_reading(num_str: &str, unit: &str, units: &UnitsData) -> String {
    let nk = number_to_katakana(num_str);
    let read = units.lookup(unit).map(str::to_string).unwrap_or_default();
    // カ行で始まる単位 (キロ / カロリー 等) の前では 6 / 8 / 10 / 100 が促音化する
    // (ロッキロ / ハッキロ / ジュッキロ / ヒャッキロ)。 1 は イチキロ が一般的なので変えない。
    // (★2026-09-15 精度評価で検出: 「100kg」 → ヒャクキログラム)
    let nk = if read.starts_with(['カ', 'キ', 'ク', 'ケ', 'コ']) {
        sokuonize_before_k_unit(&nk)
    } else {
        nk
    };
    format!("{nk}{read}")
}

/// カ行で始まる単位の前の促音化。
///
/// [`sokuonize_last`] と違い **イチ は対象外** (1キロ = イチキロ)、 百の位の
/// ヒャク / ピャク / ビャク を含む (100キロ = ヒャッキロ、 600キロ = ロッピャッキロ)。
fn sokuonize_before_k_unit(num_kata: &str) -> String {
    for (src, dst) in &[
        ("ロク", "ロッ"),
        ("ハチ", "ハッ"),
        ("ジュウ", "ジュッ"),
        ("ヒャク", "ヒャッ"),
        ("ピャク", "ピャッ"),
        ("ビャク", "ビャッ"),
    ] {
        if let Some(stripped) = num_kata.strip_suffix(src) {
            return format!("{stripped}{dst}");
        }
    }
    num_kata.to_string()
}

/// 数値 + 大数スケール (万/億/兆…) → カタカナ読み
///
/// 兆のみ末尾 1/8/0 で促音化 (イチ→イッチョウ 等)。
#[must_use]
pub fn scale_reading(num_str: &str, scale: &str, scales: &ScalesData) -> String {
    let nk = number_to_katakana(num_str);
    let scale_kana = scales.lookup(scale).unwrap_or("");

    let last = last_digit(num_str);
    let nk_adj = if scale == "兆" && matches!(last, 1 | 8 | 0) {
        sokuonize_last(&nk)
    } else {
        nk
    };

    format!("{nk_adj}{scale_kana}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::parse_toml;

    #[test]
    fn scale_basic() {
        let raw = include_str!("../../tests/fixtures/rules/scales.toml");
        let scales: ScalesData = parse_toml(raw, "scales.toml").unwrap();
        assert_eq!(scale_reading("3", "万", &scales), "サンマン");
        assert_eq!(scale_reading("1", "兆", &scales), "イッチョウ");
        assert_eq!(scale_reading("8", "兆", &scales), "ハッチョウ");
        assert_eq!(scale_reading("2", "兆", &scales), "ニチョウ"); // 連濁なし
    }

    #[test]
    fn si_unit_basic() {
        let raw = include_str!("../../tests/fixtures/rules/units.toml");
        let units: UnitsData = parse_toml(raw, "units.toml").unwrap();
        assert_eq!(si_unit_reading("100", "km", &units), "ヒャッキロメートル");
        assert_eq!(si_unit_reading("3", "L", &units), "サンリットル");
    }

    #[test]
    fn si_unit_sokuon_before_k_row_unit() {
        // カ行で始まる単位 (キロ) の前だけ 6 / 8 / 10 / 100 / 百の位 が促音化する。
        let raw = include_str!("../../tests/fixtures/rules/units.toml");
        let units: UnitsData = parse_toml(raw, "units.toml").unwrap();
        assert_eq!(si_unit_reading("6", "km", &units), "ロッキロメートル");
        assert_eq!(si_unit_reading("8", "km", &units), "ハッキロメートル");
        assert_eq!(si_unit_reading("10", "km", &units), "ジュッキロメートル");
        assert_eq!(
            si_unit_reading("600", "km", &units),
            "ロッピャッキロメートル"
        );
        assert_eq!(
            si_unit_reading("300", "km", &units),
            "サンビャッキロメートル"
        );
        // 1 は イチキロ が一般的なので促音化しない。 促音にならない数字もそのまま。
        assert_eq!(si_unit_reading("1", "km", &units), "イチキロメートル");
        assert_eq!(si_unit_reading("3", "km", &units), "サンキロメートル");
        // カ行以外の単位には効かない。
        assert_eq!(si_unit_reading("6", "L", &units), "ロクリットル");
        assert_eq!(si_unit_reading("10", "L", &units), "ジュウリットル");
    }

    #[test]
    fn symbol_basic() {
        let raw = include_str!("../../tests/fixtures/rules/symbols.toml");
        let symbols: SymbolsData = parse_toml(raw, "symbols.toml").unwrap();
        assert_eq!(
            symbol_char_reading('+', &symbols).as_deref(),
            Some("プラス")
        );
        assert_eq!(
            symbol_char_reading('％', &symbols).as_deref(),
            Some("パーセント")
        );
        assert_eq!(symbol_char_reading('a', &symbols), None);
    }
}

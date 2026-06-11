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
    format!("{nk}{read}")
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
        assert_eq!(si_unit_reading("100", "km", &units), "ヒャクキロメートル");
        assert_eq!(si_unit_reading("3", "L", &units), "サンリットル");
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

//! 文字種 ([`CharType`]) 分類 — Unicode range 知識の single home。
//!
//! 漢字 / ひらがな / カタカナ / 英数 / 記号 / 絵文字 の range 表と判定 predicate を
//! 本 module に集約する。 [`crate::kana`] の公開判定 3 関数 (`is_kanji_char` 等) は
//! 本 module への delegate、 scoring 側 (matcher / special) も本 module を参照する。
//!
//! 「実用カタカナ」 (長音 ー / 半角カナ / カタカナ拡張込み) のような広い判定は
//! strict range ([`is_katakana_char`]) + 拡張 range ([`is_extended_katakana_char`]) の
//! **合成** ([`is_katakana_loose_char`]) として定義し、 判定の差分を構造で表す
//! (= 同じ range 表を複数 file にコピーして手動同期しない)。

use serde::Deserialize;

// ─── 範囲定数 ────────────────────────────────────────────────────────────────

/// ひらがな範囲: ぁ(0x3041) 〜 ん(0x3093)
pub(crate) const HIRAGANA_START: u32 = 0x3041;
pub(crate) const HIRAGANA_END: u32 = 0x3093;

/// カタカナ範囲: ァ(0x30A1) 〜 ン(0x30F3)
pub(crate) const KATAKANA_START: u32 = 0x30A1;
pub(crate) const KATAKANA_END: u32 = 0x30F3;

// ─── CharType ────────────────────────────────────────────────────────────────

/// 文字種列挙 (matcher の `prev_char_type` / `next_char_type` の値型)。
///
/// TOML では文字列で書く: `"漢字"` / `"ひらがな"` / `"カタカナ"` / `"英数"` / `"記号"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub enum CharType {
    /// 漢字 (CJK Unified Ideographs)
    #[serde(rename = "漢字")]
    Kanji,
    /// ひらがな
    #[serde(rename = "ひらがな")]
    Hiragana,
    /// カタカナ (全角・半角)
    #[serde(rename = "カタカナ")]
    Katakana,
    /// 英数 (ASCII / 全角英数)
    #[serde(rename = "英数")]
    Alphanumeric,
    /// 記号 (句読点 / 括弧 / その他記号)
    #[serde(rename = "記号")]
    Symbol,
}

// ─── 単文字判定 (strict) ─────────────────────────────────────────────────────

/// ひらがな 1 文字か (ぁ〜ん + ゔ)
#[must_use]
pub fn is_hiragana_char(c: char) -> bool {
    let cp = c as u32;
    (HIRAGANA_START..=HIRAGANA_END).contains(&cp) || c == 'ゔ'
}

/// カタカナ 1 文字か (ァ〜ン + ヴ)
#[must_use]
pub fn is_katakana_char(c: char) -> bool {
    let cp = c as u32;
    (KATAKANA_START..=KATAKANA_END).contains(&cp) || c == 'ヴ'
}

/// 漢字 1 文字か (CJK 統合漢字 + 拡張 A + 互換 + 々〆ヶ)
#[must_use]
pub fn is_kanji_char(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4DBF}' |   // CJK 拡張 A
        '\u{4E00}'..='\u{9FFF}' |   // CJK 統合漢字
        '\u{F900}'..='\u{FAFF}' |   // CJK 互換
        '々' | '〆' | 'ヶ'
    )
}

// ─── 単文字判定 (拡張 / 合成) ────────────────────────────────────────────────

/// カタカナ拡張判定 ([`is_katakana_char`] に含まれない長音 / 半角カナ等)。
///
/// - 長音記号 ー (U+30FC)
/// - 半角カタカナ (U+FF65〜U+FF9F)
/// - カタカナ拡張 (U+31F0〜U+31FF)
#[must_use]
pub fn is_extended_katakana_char(c: char) -> bool {
    matches!(c,
        '\u{30FC}'                  // 長音記号 ー
        | '\u{FF65}'..='\u{FF9F}'   // 半角カタカナ
        | '\u{31F0}'..='\u{31FF}'   // カタカナ拡張
    )
}

/// 実用カタカナ判定 (= strict + 拡張)。
///
/// 「カタカナとして扱うべき文字」 の実用集合。 [`classify_char`] の Katakana 判定や
/// 「surface が全部 kana か」 系の判定はこちらを使う。
#[must_use]
pub fn is_katakana_loose_char(c: char) -> bool {
    is_katakana_char(c) || is_extended_katakana_char(c)
}

/// 英数判定 (ASCII alphanumeric + 全角英数)。
#[must_use]
pub fn is_alphanumeric_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c,
            '\u{FF10}'..='\u{FF19}'   // 全角数字 0-9
            | '\u{FF21}'..='\u{FF3A}' // 全角大文字 A-Z
            | '\u{FF41}'..='\u{FF5A}' // 全角小文字 a-z
        )
}

/// digit 1 字判定 (= ASCII 0-9 / 全角０-９)。
#[must_use]
pub fn is_digit_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '\u{FF10}'..='\u{FF19}')
}

/// 記号判定 (kanji / kana / 英数 でなく、 punctuation 系の文字)。
///
/// 制御文字 / 空白は除外 (= None 扱い)、 句読点 / 括弧 / その他記号のみ Symbol 扱い。
#[must_use]
pub fn is_symbol_char(c: char) -> bool {
    if c.is_control() || c.is_whitespace() {
        return false;
    }
    // 既知の punctuation / symbol range をざっくり include
    matches!(c,
        // ASCII punctuation
        '\u{0021}'..='\u{002F}'
        | '\u{003A}'..='\u{0040}'
        | '\u{005B}'..='\u{0060}'
        | '\u{007B}'..='\u{007E}'
        // 日本語句読点 / 括弧
        | '\u{3000}'..='\u{303F}'
        // 全角記号 (`！` 〜 `／` の前半部、 数字英字以外)
        | '\u{FF01}'..='\u{FF0F}'
        | '\u{FF1A}'..='\u{FF20}'
        | '\u{FF3B}'..='\u{FF40}'
        | '\u{FF5B}'..='\u{FF65}'
        // 一般 punctuation (U+2030..U+205E は U+2000..U+206F に含まれるので重複削除済)
        | '\u{2000}'..='\u{206F}'
    )
}

/// 絵文字判定 (Unicode emoji range の主要部をカバー)。
///
/// 完全な Unicode Emoji 仕様 (combining sequence / ZWJ joiner 等) は対応しない、
/// 主要 char range のみで実用十分。 必要なら 0.2.0+ で精緻化。
///
/// ## カバー範囲
///
/// - U+1F300..U+1F5FF: Misc Symbols and Pictographs
/// - U+1F600..U+1F64F: Emoticons
/// - U+1F680..U+1F6FF: Transport and Map
/// - U+1F700..U+1F77F: Alchemical Symbols
/// - U+1F900..U+1F9FF: Supplemental Symbols and Pictographs
/// - U+1FA00..U+1FA6F: Symbols and Pictographs Extended-A
/// - U+1FA70..U+1FAFF: Symbols and Pictographs Extended-B
/// - U+2600..U+26FF: Misc Symbols
/// - U+2700..U+27BF: Dingbats
#[must_use]
pub fn is_emoji_char(c: char) -> bool {
    matches!(c,
        '\u{1F300}'..='\u{1F5FF}'
        | '\u{1F600}'..='\u{1F64F}'
        | '\u{1F680}'..='\u{1F6FF}'
        | '\u{1F700}'..='\u{1F77F}'
        | '\u{1F900}'..='\u{1F9FF}'
        | '\u{1FA00}'..='\u{1FA6F}'
        | '\u{1FA70}'..='\u{1FAFF}'
        | '\u{2600}'..='\u{26FF}'
        | '\u{2700}'..='\u{27BF}'
    )
}

// ─── 分類 ────────────────────────────────────────────────────────────────────

/// 文字を [`CharType`] に分類。
///
/// 分類不能 (= 制御文字 / 空白 等) は `None`。
///
/// ## 分類順序 (mutually exclusive)
///
/// 1. 漢字 (CJK Unified Ideographs 等)
/// 2. ひらがな
/// 3. カタカナ (strict + 拡張 = [`is_katakana_loose_char`])
/// 4. 英数 (ASCII alphanumeric / 全角英数)
/// 5. 記号 (上記以外の punctuation 等)
#[must_use]
pub fn classify_char(c: char) -> Option<CharType> {
    if is_kanji_char(c) {
        Some(CharType::Kanji)
    } else if is_hiragana_char(c) {
        Some(CharType::Hiragana)
    } else if is_katakana_loose_char(c) {
        Some(CharType::Katakana)
    } else if is_alphanumeric_char(c) {
        Some(CharType::Alphanumeric)
    } else if is_symbol_char(c) {
        Some(CharType::Symbol)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── classify_char ───────────────────────────────────────────────────────

    #[test]
    fn classify_char_kanji() {
        assert_eq!(classify_char('生'), Some(CharType::Kanji));
        assert_eq!(classify_char('漢'), Some(CharType::Kanji));
        assert_eq!(classify_char('魔'), Some(CharType::Kanji));
        assert_eq!(classify_char('々'), Some(CharType::Kanji)); // 踊り字
        assert_eq!(classify_char('〆'), Some(CharType::Kanji));
        assert_eq!(classify_char('ヶ'), Some(CharType::Kanji));
    }

    #[test]
    fn classify_char_hiragana() {
        assert_eq!(classify_char('あ'), Some(CharType::Hiragana));
        assert_eq!(classify_char('ん'), Some(CharType::Hiragana));
        assert_eq!(classify_char('ゃ'), Some(CharType::Hiragana));
        assert_eq!(classify_char('ゔ'), Some(CharType::Hiragana));
    }

    #[test]
    fn classify_char_katakana() {
        assert_eq!(classify_char('ア'), Some(CharType::Katakana));
        assert_eq!(classify_char('ン'), Some(CharType::Katakana));
        assert_eq!(classify_char('ヴ'), Some(CharType::Katakana));
        assert_eq!(classify_char('ー'), Some(CharType::Katakana)); // 長音
        assert_eq!(classify_char('ｱ'), Some(CharType::Katakana)); // 半角
        assert_eq!(classify_char('ㇰ'), Some(CharType::Katakana)); // カタカナ拡張
    }

    #[test]
    fn classify_char_alphanumeric() {
        assert_eq!(classify_char('A'), Some(CharType::Alphanumeric));
        assert_eq!(classify_char('z'), Some(CharType::Alphanumeric));
        assert_eq!(classify_char('5'), Some(CharType::Alphanumeric));
        assert_eq!(classify_char('Ａ'), Some(CharType::Alphanumeric)); // 全角
        assert_eq!(classify_char('１'), Some(CharType::Alphanumeric)); // 全角数字
    }

    #[test]
    fn classify_char_symbol() {
        assert_eq!(classify_char('!'), Some(CharType::Symbol));
        assert_eq!(classify_char('、'), Some(CharType::Symbol));
        assert_eq!(classify_char('。'), Some(CharType::Symbol));
        assert_eq!(classify_char('「'), Some(CharType::Symbol));
        assert_eq!(classify_char('】'), Some(CharType::Symbol));
    }

    #[test]
    fn classify_char_unknown_returns_none() {
        // 制御文字 / 空白 / 未割当 は None
        assert_eq!(classify_char(' '), None);
        assert_eq!(classify_char('\t'), None);
        assert_eq!(classify_char('\n'), None);
    }

    // ─── range 境界 ──────────────────────────────────────────────────────────

    #[test]
    fn hiragana_range_boundaries() {
        assert!(is_hiragana_char('ぁ')); // U+3041 = start
        assert!(is_hiragana_char('ん')); // U+3093 = end
        assert!(!is_hiragana_char('\u{3040}')); // start - 1 (未割当)
        assert!(!is_hiragana_char('゛')); // U+309B 濁点 (= ひらがな block 内だが範囲外)
    }

    #[test]
    fn katakana_range_boundaries() {
        assert!(is_katakana_char('ァ')); // U+30A1 = start
        assert!(is_katakana_char('ン')); // U+30F3 = end
        assert!(!is_katakana_char('゠')); // U+30A0 (start - 1)
        assert!(!is_katakana_char('ー')); // 長音は strict 範囲外 → loose 側
        assert!(is_katakana_loose_char('ー'));
        assert!(is_katakana_loose_char('ｶ')); // 半角
        assert!(!is_katakana_loose_char('か'));
    }

    #[test]
    fn extended_katakana_boundaries() {
        assert!(is_extended_katakana_char('\u{FF65}')); // ･ 半角中点 = start
        assert!(is_extended_katakana_char('\u{FF9F}')); // ﾟ = end
        assert!(is_extended_katakana_char('\u{31F0}')); // ㇰ = start
        assert!(is_extended_katakana_char('\u{31FF}')); // ㇿ = end
        assert!(!is_extended_katakana_char('ア')); // strict 側は含まない
    }

    // ─── is_emoji_char ───────────────────────────────────────────────────────

    #[test]
    fn emoji_char_detects_common_emoji() {
        assert!(is_emoji_char('😀')); // U+1F600
        assert!(is_emoji_char('🎉')); // U+1F389
        assert!(is_emoji_char('🚀')); // U+1F680
        assert!(is_emoji_char('☀')); // U+2600
        assert!(is_emoji_char('✨')); // U+2728
    }

    #[test]
    fn emoji_char_rejects_non_emoji() {
        assert!(!is_emoji_char('a'));
        assert!(!is_emoji_char('猫'));
        assert!(!is_emoji_char('あ'));
        assert!(!is_emoji_char('ア'));
        assert!(!is_emoji_char('1'));
        assert!(!is_emoji_char(' '));
    }
}

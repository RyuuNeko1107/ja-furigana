//! 辞書 load 経路の string sanitize layer (任意コード埋め込み防御)
//!
//! TOML 自体の deserialize で RCE は起きないが、 entries の **value** に紛れ
//! 込ませることで間接的に害を与える経路を構造的に塞ぐ:
//!
//! - **C0 制御文字 / DEL**: log injection / display 破壊 / TOML 書き戻し時の
//!   parse 全体破壊 (self-DoS)
//! - **Unicode bidi override** (U+202A..U+202E / U+2066..U+2069): Trojan Source
//!   攻撃 — PR review 時にコード意味と見た目が乖離して見落としが発生
//! - **Zero-width / invisible char** (U+200B..U+200F / U+FEFF): homoglyph 詐称、
//!   一見同じ文字でも別 surface になり辞書 lookup の sneaky bypass
//! - **excessive length per entry**: 1 entry に 1 GB string で OOM
//!
//! Dict / Loanwords / SingleOverrides の load 経路で本 module の関数を呼び、
//! 上記カテゴリの char を含む / 過大長の entry は **load 時に reject** する。
//! 公開 ja-furigana-dict 配布物は CJK + kana + ASCII + 通常記号のみなので
//! 影響なし、 user dict や悪意ある PR 経由での混入を構造的に防御する。
//!
//! `\t` (U+0009) / `\n` (U+000A) / `\r` (U+000D) は通常用途 (multi-line
//! reading 等) で許容されうるので reject 対象外。

use crate::error::{FuriganaError, Result};

/// 1 entry の string field 上限 (surface / reading 双方)。
///
/// 通常用途 (jukugo ~10 chars、 reading ~30 chars、 慣用句 ~50 chars) を
/// 十分カバーしつつ、 攻撃者が dict 内に巨大 string を仕込んで OOM させる
/// 経路を塞ぐ。
pub(crate) const MAX_DICT_VALUE_CHARS: usize = 1024;

/// 辞書 string field (surface / reading 等) に対する sanitize check。
///
/// 通過条件: 文字数 ≤ [`MAX_DICT_VALUE_CHARS`]、 禁止文字を含まない。
///
/// `field` は error message の文脈付け用 (例: `"jukugo surface"`、
/// `"loanword reading"`)。
///
/// # Errors
/// 上限超過 / 禁止文字混入時 [`FuriganaError::Validation`]。
pub(crate) fn sanitize_dict_value(field: &str, value: &str) -> Result<()> {
    let n_chars = value.chars().count();
    if n_chars > MAX_DICT_VALUE_CHARS {
        return Err(FuriganaError::Validation(format!(
            "{field} 文字数 {n_chars} が上限 {MAX_DICT_VALUE_CHARS} を超過"
        )));
    }
    for c in value.chars() {
        let code = c as u32;
        let bad =
            // C0 control char (0x00-0x1F) を `\t` `\n` `\r` 以外で reject
            (code < 0x20 && code != 0x09 && code != 0x0A && code != 0x0D)
            // DEL
            || code == 0x7F
            // Unicode bidi override (Trojan Source 攻撃)
            || matches!(code, 0x202A..=0x202E | 0x2066..=0x2069)
            // Zero-width / invisible char (homoglyph 詐称)
            || matches!(code, 0x200B..=0x200F | 0xFEFF);
        if bad {
            return Err(FuriganaError::Validation(format!(
                "{field} に禁止文字 U+{code:04X} を含む: {value:?}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_dict_values() {
        sanitize_dict_value("surface", "灰桜").unwrap();
        sanitize_dict_value("reading", "ハイザクラ").unwrap();
        sanitize_dict_value("reading", "あいうえお").unwrap();
        sanitize_dict_value("loanword", "Kubernetes").unwrap();
        sanitize_dict_value("phrase", "こんにちは、世界。").unwrap();
        // tab / newline / cr は許容
        sanitize_dict_value("multi", "a\tb\nc\rd").unwrap();
        // 半角スペース (0x20) は制御文字でない (境界 `code < 0x20`) ので許可
        sanitize_dict_value("spaced", "a b c").unwrap();
        sanitize_dict_value("spaced", "ア イ").unwrap();
    }

    #[test]
    fn rejects_control_chars() {
        // NULL byte
        assert!(sanitize_dict_value("x", "abc\0def").is_err());
        // C0 control (BEL)
        assert!(sanitize_dict_value("x", "abc\x07def").is_err());
        // DEL
        assert!(sanitize_dict_value("x", "abc\x7fdef").is_err());
    }

    #[test]
    fn rejects_bidi_override() {
        // U+202E RIGHT-TO-LEFT OVERRIDE (Trojan Source の代表例)
        assert!(sanitize_dict_value("x", "abc\u{202E}def").is_err());
        // U+2066 LEFT-TO-RIGHT ISOLATE
        assert!(sanitize_dict_value("x", "abc\u{2066}def").is_err());
    }

    #[test]
    fn rejects_zero_width() {
        // U+200B ZERO WIDTH SPACE
        assert!(sanitize_dict_value("x", "abc\u{200B}def").is_err());
        // U+FEFF BOM
        assert!(sanitize_dict_value("x", "abc\u{FEFF}def").is_err());
    }

    #[test]
    fn rejects_excessive_length() {
        let long = "あ".repeat(MAX_DICT_VALUE_CHARS + 1);
        assert!(sanitize_dict_value("x", &long).is_err());
        let ok = "あ".repeat(MAX_DICT_VALUE_CHARS);
        sanitize_dict_value("x", &ok).unwrap();
    }
}

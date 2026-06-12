//! 文脈依存 reading の post-pass (path 確定後に token 列全体を見て補正)。
//!
//! scoring engine の matcher は前後 **1 token** しか参照しないため、 2 token 以上
//! 離れた文脈に依存する読みは matcher では表現できない。 本 module は path 確定後の
//! [`AnalyzeResult`] token 列を走査し、 そうした case を補正する post-pass を提供する
//! ([`crate::scoring::odoriji::apply_rendaku_inplace`] と同じ層)。
//!
//! ## 含まれる補正
//!
//! - [`apply_hara_suku_inplace`]: 「腹 / お腹 / 小腹 + (助詞) + 空く活用」 を
//!   「すく」 系読みに補正 (= 空腹の意。 「席が空いた」 等 腹文脈なしの 「あく」 は不変)。

use crate::scoring::analyze::Token;

/// surface が 「腹」 で終わるか (= 腹 / お腹 / 小腹 / 空き腹 等)。
fn is_hara_surface(s: &str) -> bool {
    s.ends_with('腹')
}

/// 空く活用 token (surface = 空 + あ-読み) なら す-読みを返す。
///
/// 「空く」 は あく / すく の両義。 surface が 「空」 で始まり reading が あ-読み
/// (ア / あ で始まる) の動詞活用 (空く / 空い(た|て) / 空き / 空かせ) のとき、
/// 先頭の あ → す に倒した読みを返す。 「空ける」 (= あける、 開ける義) は対象外。
fn suku_reading_for(surface: &str, reading: &str) -> Option<String> {
    let mut chars = surface.chars();
    if chars.next() != Some('空') {
        return None;
    }
    // 2 文字目が く / い / き / か のみ対象 (= 空ける=あける を除外)。
    // surface が 「空」 単独 (2 文字目なし) も許可 (= token 分割で 空 単独になる case)。
    match chars.next() {
        None | Some('く' | 'い' | 'き' | 'か') => {}
        Some(_) => return None,
    }
    for (a, s) in [('ア', 'ス'), ('あ', 'す')] {
        if let Some(rest) = reading.strip_prefix(a) {
            return Some(format!("{s}{rest}"));
        }
    }
    None
}

/// 「腹系 + (助詞) + 空く活用」 を す-読みに補正する in-place post-pass。
///
/// 空く活用 token の直前 1〜2 token に 「腹」 で終わる surface があれば空腹の意と
/// 判断し、 当該 token の あ-読みを す-読みに置換する (例: 「小腹が空いた」 →
/// こばら**が**すいた、 「お腹空いた」 → おなかすいた)。
///
/// 直前 2 token まで見るのは間に助詞 1 token (が / は / も 等) が入る形を拾うため。
pub fn apply_hara_suku_inplace(tokens: &mut [Token]) {
    for i in 0..tokens.len() {
        let Some(new_reading) = suku_reading_for(&tokens[i].surface, &tokens[i].reading) else {
            continue;
        };
        let has_hara = (1..=2)
            .filter_map(|back| i.checked_sub(back))
            .any(|j| is_hara_surface(&tokens[j].surface));
        if has_hara {
            tokens[i].reading = new_reading;
        }
    }
}

/// 腹+空く 文脈補正 post-pass の adapter ([`crate::scoring::postpass::ReadingPostPass`])。
#[derive(Debug, Clone, Copy)]
pub struct HaraSukuPass;

impl crate::scoring::postpass::ReadingPostPass for HaraSukuPass {
    fn apply(&self, tokens: &mut Vec<Token>) {
        apply_hara_suku_inplace(tokens);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(surface: &str, reading: &str) -> Token {
        Token {
            surface: surface.to_string(),
            reading: reading.to_string(),
            range: 0..surface.len(),
            accent_phrases: Vec::new(),
            ambiguous: false,
            alternatives: Vec::new(),
        }
    }

    #[test]
    fn kobara_ga_suita() {
        // 小腹 | が | 空い | た → 空い が スイ に
        let mut t = vec![
            token("小腹", "コバラ"),
            token("が", "ガ"),
            token("空い", "アイ"),
            token("た", "タ"),
        ];
        apply_hara_suku_inplace(&mut t);
        assert_eq!(t[2].reading, "スイ");
    }

    #[test]
    fn onaka_suita_no_particle() {
        // お腹 | 空い | た (助詞なし、 直前 1 token)
        let mut t = vec![
            token("お腹", "オナカ"),
            token("空い", "アイ"),
            token("た", "タ"),
        ];
        apply_hara_suku_inplace(&mut t);
        assert_eq!(t[1].reading, "スイ");
    }

    #[test]
    fn hara_ga_suku() {
        let mut t = vec![
            token("腹", "ハラ"),
            token("が", "ガ"),
            token("空く", "アク"),
        ];
        apply_hara_suku_inplace(&mut t);
        assert_eq!(t[2].reading, "スク");
    }

    #[test]
    fn seki_ga_aita_unchanged() {
        // 席 | が | 空い | た → 腹文脈なしなので アイ のまま
        let mut t = vec![
            token("席", "セキ"),
            token("が", "ガ"),
            token("空い", "アイ"),
            token("た", "タ"),
        ];
        apply_hara_suku_inplace(&mut t);
        assert_eq!(t[2].reading, "アイ");
    }

    #[test]
    fn hara_too_far_unchanged() {
        // 腹 が 3 token 以上前 (window 外) なら不変
        let mut t = vec![
            token("腹", "ハラ"),
            token("が", "ガ"),
            token("すごく", "スゴク"),
            token("空い", "アイ"),
        ];
        apply_hara_suku_inplace(&mut t);
        assert_eq!(t[3].reading, "アイ");
    }

    #[test]
    fn akeru_not_touched() {
        // 空ける (= あける) は 2 文字目 「け」 で対象外
        let mut t = vec![token("お腹", "オナカ"), token("空け", "アケ")];
        apply_hara_suku_inplace(&mut t);
        assert_eq!(t[1].reading, "アケ");
    }

    #[test]
    fn hiragana_reading_supported() {
        // reading がひらがなでも (あ → す)
        let mut t = vec![
            token("お腹", "おなか"),
            token("空い", "あい"),
            token("た", "た"),
        ];
        apply_hara_suku_inplace(&mut t);
        assert_eq!(t[1].reading, "すい");
    }
}

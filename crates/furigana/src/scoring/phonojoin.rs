//! OOV 漢字複合語 fallback 読みの促音便 join (ADR-0008)。
//!
//! dict にも IPADIC にも無い漢字複合語は per-char default 読みのべた連結になる
//! (学+校 が OOV なら ガクコウ)。 本 pass は **隣接する単漢字 token 同士の連結部**
//! (= OOV per-char chain の signature) に標準的な促音便を適用し、 音韻的に
//! それっぽい読みへ寄せる。
//!
//! ## 発火条件
//!
//! - 両 token とも surface が **単一の実漢字** (々/〆/ヶ は専用 provider の領分なので除外)
//! - input 上で byte 隣接 (= 間に何も挟まっていない)
//! - 両 reading がカタカナ (ひらがな読みには適用しない = 訓読み単字の多くを自然に回避)
//!
//! multi-char token が絡む join は dict / IPADIC が確定した実単語境界なので触らない。
//!
//! ## 促音便 rule (標準的な音読み gemination のみ)
//!
//! | 前 token 末尾 | 後 token 先頭 | 変換 |
//! |---|---|---|
//! | ク | カ行 (清音) | ク → ッ (学+校 → ガッコウ) |
//! | チ / ツ | カ・サ・タ行 (清音) | チ/ツ → ッ (一+気 → イッキ、 発+想 → ハッソウ) |
//! | チ / ツ | ハ行 | チ/ツ → ッ + ハ行 → パ行 (一+杯 → イッパイ) |
//!
//! キ は含めない (的確 = テキカク のように gemination しない例が多い)。
//! 連濁・ン+ハ行 の音変化も含めない (語彙依存で規則化できない、 ADR-0008)。

use crate::char_class::is_kanji_char;
use crate::scoring::analyze::Token;
use crate::scoring::postpass::ReadingPostPass;

/// 促音便対象の後続 reading 先頭 (清音 カ・サ・タ行)。
fn geminates_before(c: char) -> bool {
    matches!(
        c,
        'カ' | 'キ'
            | 'ク'
            | 'ケ'
            | 'コ'
            | 'サ'
            | 'シ'
            | 'ス'
            | 'セ'
            | 'ソ'
            | 'タ'
            | 'チ'
            | 'ツ'
            | 'テ'
            | 'ト'
    )
}

/// ハ行 → パ行 (促音便に伴う半濁音化)。
fn ha_to_pa(c: char) -> Option<char> {
    Some(match c {
        'ハ' => 'パ',
        'ヒ' => 'ピ',
        'フ' => 'プ',
        'ヘ' => 'ペ',
        'ホ' => 'ポ',
        _ => return None,
    })
}

/// 「単一の実漢字 surface」 判定。 々/〆/ヶ は除外 (専用 provider の領分)。
fn is_single_real_kanji(surface: &str) -> bool {
    let mut chars = surface.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => is_kanji_char(c) && !matches!(c, '々' | '〆' | 'ヶ'),
        _ => false,
    }
}

/// reading が非空かつ全カタカナ (長音符含む) か。
fn is_katakana_reading(reading: &str) -> bool {
    !reading.is_empty() && reading.chars().all(|c| matches!(c, 'ァ'..='ヶ' | 'ー'))
}

/// OOV 漢字 chain の隣接 join に促音便を適用する post-pass (ADR-0008)。
pub struct SokuonJoinPass;

impl ReadingPostPass for SokuonJoinPass {
    fn apply(&self, tokens: &mut Vec<Token>) {
        for i in 0..tokens.len().saturating_sub(1) {
            let (a, b) = {
                let (left, right) = tokens.split_at_mut(i + 1);
                (&mut left[i], &right[0])
            };
            if !is_single_real_kanji(&a.surface)
                || !is_single_real_kanji(&b.surface)
                || a.range.end != b.range.start
                || !is_katakana_reading(&a.reading)
                || !is_katakana_reading(&b.reading)
                || a.reading.chars().count() < 2
            {
                continue;
            }
            let Some(last) = a.reading.chars().last() else {
                continue;
            };
            let Some(next_first) = b.reading.chars().next() else {
                continue;
            };
            let geminate = match last {
                'ク' => matches!(next_first, 'カ' | 'キ' | 'ク' | 'ケ' | 'コ'),
                'チ' | 'ツ' => geminates_before(next_first) || ha_to_pa(next_first).is_some(),
                _ => false,
            };
            if !geminate {
                continue;
            }
            // 前 token 末尾を ッ に置換
            let mut a_new: String = a.reading.chars().collect();
            a_new.pop();
            a_new.push('ッ');
            a.reading = a_new;
            // dict bracket 由来の accent 情報は reading と不整合になるので捨てる
            // (OOV chain の単字はそもそも bracket を持たない想定、 defensive)
            a.accent_phrases.clear();
            // ハ行 → パ行 (後 token 先頭)
            if let Some(pa) = ha_to_pa(next_first) {
                let b = &mut tokens[i + 1];
                let mut b_new = String::with_capacity(b.reading.len());
                let mut chars = b.reading.chars();
                chars.next();
                b_new.push(pa);
                b_new.extend(chars);
                b.reading = b_new;
                b.accent_phrases.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::analyze::Token;
    use crate::scoring::candidate::{Candidate, Score};

    /// range を input 上の連続 byte 位置で組む token 列 helper。
    fn chain(entries: &[(&str, &str)]) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut pos = 0;
        for (surface, reading) in entries {
            let end = pos + surface.len();
            tokens.push(Token::from_candidate(&Candidate::new(
                *surface,
                *reading,
                pos..end,
                Score::kanji(1),
            )));
            pos = end;
        }
        tokens
    }

    fn readings(tokens: &[Token]) -> Vec<&str> {
        tokens.iter().map(|t| t.reading.as_str()).collect()
    }

    #[test]
    fn ku_before_ka_row_geminates() {
        // 学+校 (OOV 仮定) → ガッ + コウ
        let mut tokens = chain(&[("学", "ガク"), ("校", "コウ")]);
        SokuonJoinPass.apply(&mut tokens);
        assert_eq!(readings(&tokens), vec!["ガッ", "コウ"]);
    }

    #[test]
    fn tsu_before_sa_row_geminates() {
        // 発+想 → ハッ + ソウ
        let mut tokens = chain(&[("発", "ハツ"), ("想", "ソウ")]);
        SokuonJoinPass.apply(&mut tokens);
        assert_eq!(readings(&tokens), vec!["ハッ", "ソウ"]);
    }

    #[test]
    fn chi_before_ha_row_geminates_and_voices_to_pa() {
        // 一+杯 → イッ + パイ
        let mut tokens = chain(&[("一", "イチ"), ("杯", "ハイ")]);
        SokuonJoinPass.apply(&mut tokens);
        assert_eq!(readings(&tokens), vec!["イッ", "パイ"]);
    }

    #[test]
    fn ku_before_ha_row_does_not_geminate() {
        // ク + ハ行 は促音化しない (悪+筆 = アクヒツ)
        let mut tokens = chain(&[("悪", "アク"), ("筆", "ヒツ")]);
        SokuonJoinPass.apply(&mut tokens);
        assert_eq!(readings(&tokens), vec!["アク", "ヒツ"]);
    }

    #[test]
    fn ki_is_excluded() {
        // キ は gemination 対象外 (的+確 = テキカク)
        let mut tokens = chain(&[("的", "テキ"), ("確", "カク")]);
        SokuonJoinPass.apply(&mut tokens);
        assert_eq!(readings(&tokens), vec!["テキ", "カク"]);
    }

    #[test]
    fn voiced_next_does_not_geminate() {
        // 学+芸 = ガクゲイ (濁音先頭は促音化しない)
        let mut tokens = chain(&[("学", "ガク"), ("芸", "ゲイ")]);
        SokuonJoinPass.apply(&mut tokens);
        assert_eq!(readings(&tokens), vec!["ガク", "ゲイ"]);
    }

    #[test]
    fn multi_char_token_join_untouched() {
        // multi-char token (実単語境界) が絡む join は触らない
        let mut tokens = chain(&[("売却", "バイキャク"), ("画", "カク")]);
        SokuonJoinPass.apply(&mut tokens);
        assert_eq!(readings(&tokens), vec!["バイキャク", "カク"]);
    }

    #[test]
    fn non_adjacent_ranges_untouched() {
        // input 上で隣接していなければ触らない
        let mut tokens = chain(&[("学", "ガク"), ("、", "、"), ("校", "コウ")]);
        SokuonJoinPass.apply(&mut tokens);
        assert_eq!(readings(&tokens), vec!["ガク", "、", "コウ"]);
    }

    #[test]
    fn hiragana_kun_reading_untouched() {
        // ひらがな読み (訓読み単字が主) はカタカナ判定で弾く
        let mut tokens = chain(&[("靴", "くつ"), ("籠", "かご")]);
        SokuonJoinPass.apply(&mut tokens);
        assert_eq!(readings(&tokens), vec!["くつ", "かご"]);
    }

    #[test]
    fn single_kana_reading_untouched() {
        // reading 1 文字は促音化で読みが消えるので触らない
        let mut tokens = chain(&[("九", "ク"), ("階", "カイ")]);
        SokuonJoinPass.apply(&mut tokens);
        assert_eq!(readings(&tokens), vec!["ク", "カイ"]);
    }

    #[test]
    fn odoriji_placeholder_excluded() {
        // 々 は専用 provider の領分
        let mut tokens = chain(&[("学", "ガク"), ("々", "ガク")]);
        SokuonJoinPass.apply(&mut tokens);
        assert_eq!(readings(&tokens), vec!["ガク", "ガク"]);
    }

    #[test]
    fn three_char_chain_applies_pairwise() {
        // 3 連 chain は左から pairwise (実在語でなく合成例)
        let mut tokens = chain(&[("一", "イチ"), ("刻", "コク"), ("家", "カ")]);
        SokuonJoinPass.apply(&mut tokens);
        // イチ+コク → イッコク、 コク+カ → コッカ
        assert_eq!(readings(&tokens), vec!["イッ", "コッ", "カ"]);
    }
}

//! 括弧読み post-pass: `漢字語（ひらがな）` の括弧内の読みを、 直前の漢字語の読みに採用する。
//!
//! Wikipedia や解説文は 「雲州（うんしゅう）」 「植松正（うえまつただし）」 のように、 語の直後の括弧に
//! 読みを書く。 形態素解析の読みが括弧の読みと食い違うとき (雲州 = くもしゅう) に、 書き手が示した読みを使う。
//!
//! ## 採用の条件 (誤爆を避けるため厳しめ)
//!
//! - 括弧の直前が漢字だけの 1〜10 字 (括弧とは input 上で隣接) で、 括弧の中がひらがな 2〜16 字だけ
//! - 括弧の中が現在の読みと **ちがう** こと (同じなら何もしない)
//! - 括弧の読みの長さが現在の読みの 0.7〜1.5 倍であること (名の一部だけを括弧で読む 「西村博之（ひろゆき）」 や、
//!   前の語まで含む 「国の一覧（くにのいちらん）」 を除く)
//! - 現在の読みと括弧の読みの共通部分 (最長共通部分列) が長い方の半分以上あること。
//!   「価格（ねだん）」 のように読みが無関係な言い換えの括弧は採用しない
//!
//! 括弧そのものは取り除かず、 そのまま残す (読みが 2 度読まれるのは従来どおり)。

use crate::kana::{hira_to_kata, is_hiragana_char, is_kanji_char, kata_to_hira};
use crate::scoring::analyze::Token;
use crate::scoring::postpass::ReadingPostPass;

/// 括弧の直前の漢字語の最大字数
const MAX_KANJI_CHARS: usize = 10;
/// 括弧の中の読みの最小 / 最大字数
const MIN_PAREN_CHARS: usize = 2;
const MAX_PAREN_CHARS: usize = 16;
/// 括弧の中に含められる token 数の上限
const MAX_PAREN_TOKENS: usize = 8;

/// `漢字語（ひらがな）` の括弧内の読みを採用する post-pass。
#[derive(Debug, Default, Clone, Copy)]
pub struct ParenReadingPass;

fn is_open(s: &str) -> bool {
    s == "（" || s == "("
}

fn is_close(s: &str) -> bool {
    s == "）" || s == ")"
}

fn all_kanji(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_kanji_char)
}

/// 2 つの読みの最長共通部分列の長さ
fn lcs_len(a: &[char], b: &[char]) -> usize {
    let mut prev = vec![0usize; b.len() + 1];
    for &x in a {
        let mut cur = vec![0usize; b.len() + 1];
        for (j, &y) in b.iter().enumerate() {
            cur[j + 1] = if x == y {
                prev[j] + 1
            } else {
                prev[j + 1].max(cur[j])
            };
        }
        prev = cur;
    }
    prev[b.len()]
}

/// 括弧の中の読みが、 現在の読みの代わりに採用できるか
fn acceptable(current_hira: &str, paren: &str) -> bool {
    if current_hira == paren {
        return false;
    }
    let a: Vec<char> = current_hira.chars().collect();
    let b: Vec<char> = paren.chars().collect();
    // 長さが近いこと (0.7〜1.5 倍)。 括弧が名の一部だけ (西村博之 → ひろゆき) や、 前の語まで含む (一覧 → くにのいちらん) 形は採用しない
    let (la, lb) = (a.len(), b.len());
    if la == 0 || lb * 10 < la * 7 || lb * 2 > la * 3 {
        return false;
    }
    lcs_len(&a, &b) * 2 >= la.max(lb)
}

impl ReadingPostPass for ParenReadingPass {
    fn apply(&self, tokens: &mut Vec<Token>) {
        let mut p = 1;
        while p < tokens.len() {
            if !is_open(&tokens[p].surface) {
                p += 1;
                continue;
            }
            // 括弧の中: 閉じ括弧までの token がすべてひらがなで隣接している
            let mut close = None;
            let mut paren = String::new();
            let mut prev_end = tokens[p].range.end;
            for (off, t) in tokens[p + 1..].iter().take(MAX_PAREN_TOKENS).enumerate() {
                if t.range.start != prev_end {
                    break;
                }
                if is_close(&t.surface) {
                    close = Some(p + 1 + off);
                    break;
                }
                if !t.surface.chars().all(is_hiragana_char) {
                    break;
                }
                paren.push_str(&t.surface);
                prev_end = t.range.end;
            }
            let n = paren.chars().count();
            if close.is_none() || !(MIN_PAREN_CHARS..=MAX_PAREN_CHARS).contains(&n) {
                p += 1;
                continue;
            }
            // 括弧の直前の漢字語: 漢字だけの token を、 input 上で隣接している限りさかのぼる
            let mut k = p;
            let mut chars = 0usize;
            let mut next_start = tokens[p].range.start;
            while k > 0 {
                let t = &tokens[k - 1];
                if t.range.end != next_start || !all_kanji(&t.surface) {
                    break;
                }
                chars += t.surface.chars().count();
                next_start = t.range.start;
                k -= 1;
            }
            if k == p || chars == 0 || chars > MAX_KANJI_CHARS {
                p += 1;
                continue;
            }
            let current: String = tokens[k..p].iter().map(|t| t.reading.as_str()).collect();
            if !acceptable(&kata_to_hira(&current), &paren) {
                p += 1;
                continue;
            }
            // 漢字語の token 列を 1 つにまとめ、 読みを括弧の読みにする
            let surface: String = tokens[k..p].iter().map(|t| t.surface.as_str()).collect();
            let end = tokens[p - 1].range.end;
            let mut merged = tokens[k].clone();
            merged.surface = surface;
            merged.reading = hira_to_kata(&paren);
            merged.range = merged.range.start..end;
            merged.accent_phrases = Vec::new();
            merged.ambiguous = false;
            merged.alternatives.clear();
            merged.why = Some("paren reading".to_string());
            merged.band = 1000;
            tokens.splice(k..p, std::iter::once(merged));
            // 併合で p が (p - k - 1) 個前にずれた。 括弧の次から続ける
            p = k + 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    fn token(surface: &str, reading: &str, range: Range<usize>) -> Token {
        Token {
            surface: surface.to_string(),
            reading: reading.to_string(),
            range,
            accent_phrases: Vec::new(),
            ambiguous: false,
            alternatives: Vec::new(),
            is_name: false,
            band: 50,
            why: None,
        }
    }

    fn run(tokens: &mut Vec<Token>) -> String {
        ParenReadingPass.apply(tokens);
        tokens.iter().map(|t| t.reading.as_str()).collect()
    }

    #[test]
    fn paren_reading_replaces_a_misread_word() {
        // 雲州（うんしゅう）: 雲州 = クモシュウ → ウンシュウ
        let mut t = vec![
            token("雲州", "クモシュウ", 0..6),
            token("（", "（", 6..9),
            token("うんしゅう", "ウンシュウ", 9..24),
            token("）", "）", 24..27),
        ];
        assert_eq!(run(&mut t), "ウンシュウ（ウンシュウ）");
        assert_eq!(t[0].why.as_deref(), Some("paren reading"));
    }

    #[test]
    fn paren_reading_merges_a_multi_token_word() {
        // 植松 + 正（うえまつただし）: 2 token → 1 token
        let mut t = vec![
            token("植松", "ウエマツ", 0..6),
            token("正", "セイ", 6..9),
            token("(", "(", 9..10),
            token("うえまつただし", "ウエマツタダシ", 10..31),
            token(")", ")", 31..32),
        ];
        assert_eq!(run(&mut t), "ウエマツタダシ(ウエマツタダシ)");
        assert_eq!(t.len(), 4);
        assert_eq!(t[0].surface, "植松正");
        assert_eq!(t[0].range, 0..9);
    }

    #[test]
    fn unrelated_gloss_in_paren_is_not_adopted() {
        // 価格（ねだん）: 読みが無関係な言い換えは採用しない
        let mut t = vec![
            token("価格", "カカク", 0..6),
            token("（", "（", 6..9),
            token("ねだん", "ネダン", 9..18),
            token("）", "）", 18..21),
        ];
        assert_eq!(run(&mut t), "カカク（ネダン）");
    }

    #[test]
    fn partial_or_overlong_paren_is_not_adopted() {
        // 西村博之（ひろゆき）: 名の一部だけ → 西村が消えるので採用しない
        let mut part = vec![
            token("西村博之", "ニシムラヒロユキ", 0..12),
            token("（", "（", 12..15),
            token("ひろゆき", "ヒロユキ", 15..27),
            token("）", "）", 27..30),
        ];
        assert_eq!(run(&mut part), "ニシムラヒロユキ（ヒロユキ）");
        // 一覧（くにのいちらん）: 前の語まで含む → 重複するので採用しない
        let mut long = vec![
            token("一覧", "イチラン", 0..6),
            token("（", "（", 6..9),
            token("くにのいちらん", "クニノイチラン", 9..30),
            token("）", "）", 30..33),
        ];
        assert_eq!(run(&mut long), "イチラン（クニノイチラン）");
    }

    #[test]
    fn same_reading_and_non_kana_paren_are_left_alone() {
        let mut same = vec![
            token("東京", "トウキョウ", 0..6),
            token("（", "（", 6..9),
            token("とうきょう", "トウキョウ", 9..24),
            token("）", "）", 24..27),
        ];
        assert_eq!(run(&mut same), "トウキョウ（トウキョウ）");
        assert_eq!(same[0].why, None);
        // 括弧の中が漢字 / 1 字だけ / 括弧の前がかな
        let mut kanji = vec![
            token("雲州", "クモシュウ", 0..6),
            token("（", "（", 6..9),
            token("島根", "シマネ", 9..15),
            token("）", "）", 15..18),
        ];
        assert_eq!(run(&mut kanji), "クモシュウ（シマネ）");
        let mut kana_before = vec![
            token("ほげ", "ホゲ", 0..6),
            token("（", "（", 6..9),
            token("うんしゅう", "ウンシュウ", 9..24),
            token("）", "）", 24..27),
        ];
        assert_eq!(run(&mut kana_before), "ホゲ（ウンシュウ）");
    }
}

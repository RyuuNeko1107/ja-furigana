//! 人名+敬称 suffix の token 衝突補正 post-pass ([`NameBoundaryPass`])。
//!
//! 「白上さん」 が 白 | 上さん (= IPADIC 「上さん (カミサン)」)、 「戌神様」 が
//! 戌 | 神様 のように、 **名前の末尾文字 + 敬称** が 1 語として辞書に存在すると、
//! Viterbi path が名前の境界を割ってしまい、 dict / IPADIC に正しい姓 entry が
//! あっても負ける (= bare 「白上」 「戌神ころね」 は正読なのに suffix 付きだけ化ける)。
//!
//! matcher (1 token 窓) では 「path が既に割れている」 ことを観測できないため、
//! ADR-0005 に従い path 確定後の post-pass で補正する。 対象は 2 形:
//!
//! - **合成 suffix 形** 「白 | 上さん」: `t[i]` = 「漢字列 X + 敬称 S」。 直前 token と
//!   X を結合して照会し、 hit すれば `t[i-1] = prev+X` / `t[i] = S` に書き換え
//!   (token 数不変)
//! - **bare suffix 形** 「白 | 上 | 氏」: `t[i]` = 敬称そのもので、 直前 2 token が
//!   漢字。 2 token の結合 surface を照会し、 hit して **読みが現状と異なる** 場合
//!   のみ 1 token に merge する (= 読みが同じなら構造を触らない)
//!
//! 結合 surface の読み source は (a) dict 熟語 (works 含む) exact hit、
//! (b) IPADIC で単一 token かつ 固有名詞、 の順。
//!
//! 単漢字の名前読み切替 (舞さん=まい / 菅さん=すが 等) は本 pass の対象外で、
//! dict の `[[kanji]]` block 人名 suffix match が担当する (= 正しい読みの選択は
//! data 側、 境界の修復は lib 側、 という分担)。

use crate::analyzer::Analyzer;
use crate::dict::Dict;
use crate::kana::{hira_to_kata, is_kanji_char};
use crate::scoring::analyze::Token;
use crate::scoring::bracket::parse_bracket_notation;
use crate::scoring::postpass::ReadingPostPass;

/// 敬称 suffix と確定 reading (カタカナ)。
///
/// 「君」 は 暴君 / 諸君 等の非敬称があるが、 本 pass は 「X+君 の X を直前 token と
/// 結合すると辞書 / 固有名詞 hit する」 場合のみ発火するため誤爆面では同列。
const SUFFIXES: &[(&str, &str)] = &[
    ("さん", "サン"),
    ("ちゃん", "チャン"),
    ("くん", "クン"),
    ("君", "クン"),
    ("様", "サマ"),
    ("氏", "シ"),
];

/// 名前側 (直前 token / 結合部 X) として許す最大文字数。
///
/// 日本語の姓 / 名は実用上 1〜3 字 + 結合部 1〜3 字 (= 結合後 2〜4 字) で足りる。
/// 上限を絞るのは 「漢字連続の偶然の結合」 での誤爆を避けるため。
const MAX_NAME_CHARS: usize = 3;

fn all_kanji(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_kanji_char)
}

/// `surface` を 「漢字列 X + 敬称 S」 に分解できれば `(X, S, S の確定 reading)` を返す。
fn split_name_suffix(surface: &str) -> Option<(&str, &'static str, &'static str)> {
    for (suffix, reading) in SUFFIXES {
        if let Some(head) = surface.strip_suffix(suffix) {
            if all_kanji(head) && head.chars().count() <= MAX_NAME_CHARS {
                return Some((head, suffix, reading));
            }
        }
    }
    None
}

/// 人名+敬称 suffix の token 衝突補正 post-pass (ADR-0005 第 3 adapter)。
///
/// dict (熟語 / works) と IPADIC (固有名詞) を結合 surface の読み source に使うため、
/// 他の post-pass と違い参照を持つ (= [`crate::scoring::pipeline::Pipeline`] が
/// 解析ごとに構築する)。
pub struct NameBoundaryPass<'a> {
    dict: &'a Dict,
    analyzer: &'a Analyzer,
}

impl<'a> NameBoundaryPass<'a> {
    #[must_use]
    pub fn new(dict: &'a Dict, analyzer: &'a Analyzer) -> Self {
        Self { dict, analyzer }
    }

    /// 結合 surface の読みを返す。 dict 優先、 次に IPADIC 固有名詞。
    fn combined_reading(&self, combined: &str) -> Option<String> {
        if let Some(reading) = self.dict.lookup_jukugo(combined) {
            return Some(reading.to_string());
        }
        // IPADIC で 1 token + 固有名詞 (姓 / 名 / 地域等) に解析される場合のみ採用。
        // 普通名詞を除外するのは 「偶然隣接した漢字 2 字が一般語になる」 誤爆を避けるため
        // (一般語なら Viterbi が最初からその path を選べたはずで、 選ばなかった = 文脈が違う)。
        let morphs = self.analyzer.tokenize(combined);
        match morphs.as_slice() {
            [m] if m.pos.as_deref() == Some("名詞")
                && m.pos_detail.as_deref() == Some("固有名詞") =>
            {
                m.reading.clone()
            }
            _ => None,
        }
    }
}

impl NameBoundaryPass<'_> {
    /// 合成 suffix 形 「白 | 上さん」 → 「白上 | さん」。 補正したら true。
    fn try_merged_suffix(&self, tokens: &mut [Token], i: usize) -> bool {
        let Some((head, suffix, suffix_reading)) =
            split_name_suffix(&tokens[i].surface).map(|(h, s, r)| (h.to_string(), s, r))
        else {
            return false;
        };
        if head.is_empty() {
            return false;
        }
        let prev = &tokens[i - 1];
        // 直前 token は漢字 1〜3 字、 かつ input 上で隣接していること
        if !all_kanji(&prev.surface)
            || prev.surface.chars().count() > MAX_NAME_CHARS
            || prev.range.end != tokens[i].range.start
        {
            return false;
        }
        let combined = format!("{}{}", prev.surface, head);
        let Some(raw_reading) = self.combined_reading(&combined) else {
            return false;
        };
        let parsed = parse_bracket_notation(&raw_reading);
        let split_at = tokens[i].range.start + head.len();

        let prev = &mut tokens[i - 1];
        prev.surface = combined;
        prev.reading = parsed.reading;
        prev.range = prev.range.start..split_at;
        prev.accent_phrases = parsed.accent_phrases;
        prev.ambiguous = false;
        prev.alternatives.clear();

        let cur = &mut tokens[i];
        cur.surface = suffix.to_string();
        cur.reading = suffix_reading.to_string();
        cur.range = split_at..cur.range.end;
        cur.accent_phrases = Vec::new();
        cur.ambiguous = false;
        cur.alternatives.clear();
        true
    }

    /// bare suffix 形 「白 | 上 | 氏」 → 「白上 | 氏」 (t[i-1] を t[i-2] に merge)。
    /// 補正したら true (= token が 1 つ減る)。
    fn try_bare_suffix(&self, tokens: &mut Vec<Token>, i: usize) -> bool {
        if i < 2 {
            return false;
        }
        let Some(&(_, suffix_reading)) = SUFFIXES.iter().find(|(s, _)| *s == tokens[i].surface)
        else {
            return false;
        };
        let (a, b) = (&tokens[i - 2], &tokens[i - 1]);
        if !all_kanji(&a.surface)
            || !all_kanji(&b.surface)
            || a.surface.chars().count() > MAX_NAME_CHARS
            || b.surface.chars().count() > MAX_NAME_CHARS
            || a.surface.chars().count() + b.surface.chars().count() > MAX_NAME_CHARS + 1
            || a.range.end != b.range.start
            || b.range.end != tokens[i].range.start
        {
            return false;
        }
        let combined = format!("{}{}", a.surface, b.surface);
        let Some(raw_reading) = self.combined_reading(&combined) else {
            return false;
        };
        let parsed = parse_bracket_notation(&raw_reading);
        // 読みが現状の連結と同じなら構造を触らない (= no-op merge を避ける)
        let current = hira_to_kata(&format!("{}{}", a.reading, b.reading));
        if hira_to_kata(&parsed.reading) == current {
            return false;
        }
        let merged_end = tokens[i - 1].range.end;

        let name = &mut tokens[i - 2];
        name.surface = combined;
        name.reading = parsed.reading;
        name.range = name.range.start..merged_end;
        name.accent_phrases = parsed.accent_phrases;
        name.ambiguous = false;
        name.alternatives.clear();
        tokens.remove(i - 1);

        let cur = &mut tokens[i - 1];
        cur.reading = suffix_reading.to_string();
        cur.accent_phrases = Vec::new();
        cur.ambiguous = false;
        cur.alternatives.clear();
        true
    }
}

impl ReadingPostPass for NameBoundaryPass<'_> {
    fn apply(&self, tokens: &mut Vec<Token>) {
        let mut i = 1;
        while i < tokens.len() {
            if !self.try_merged_suffix(tokens, i) && self.try_bare_suffix(tokens, i) {
                // merge で後続が 1 つ左に詰まったので、 i は据え置きで次の token を見る
                continue;
            }
            i += 1;
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
        }
    }

    fn analyzer() -> Analyzer {
        Analyzer::new().expect("Analyzer init failed")
    }

    #[test]
    fn inugami_sama_resegmented_via_dict() {
        // 戌 | 神様 → (dict 戌神=いぬがみ) → 戌神 | 様
        let mut dict = Dict::new();
        dict.insert("戌神", "いぬがみ");
        let a = analyzer();
        let pass = NameBoundaryPass::new(&dict, &a);
        let mut t = vec![token("戌", "イヌ", 0..3), token("神様", "カミサマ", 3..9)];
        pass.apply(&mut t);
        assert_eq!(t[0].surface, "戌神");
        assert_eq!(t[0].reading, "いぬがみ");
        assert_eq!(t[0].range, 0..6);
        assert_eq!(t[1].surface, "様");
        assert_eq!(t[1].reading, "サマ");
        assert_eq!(t[1].range, 6..9);
    }

    #[test]
    fn shirakami_san_resegmented_via_ipadic() {
        // 白 | 上さん → (IPADIC 白上 = 固有名詞 シラカミ) → 白上 | さん
        let dict = Dict::new();
        let a = analyzer();
        let pass = NameBoundaryPass::new(&dict, &a);
        let mut t = vec![
            token("白", "シロ", 0..3),
            token("上さん", "ウエサン", 3..12),
        ];
        pass.apply(&mut t);
        assert_eq!(t[0].surface, "白上");
        assert_eq!(t[0].reading, "シラカミ");
        assert_eq!(t[1].surface, "さん");
        assert_eq!(t[1].reading, "サン");
    }

    #[test]
    fn shirakami_shi_bare_suffix_merged() {
        // 白 | 上 | 氏 → (IPADIC 白上 = 固有名詞 シラカミ ≠ シロ+ウエ) → 白上 | 氏
        let dict = Dict::new();
        let a = analyzer();
        let pass = NameBoundaryPass::new(&dict, &a);
        let mut t = vec![
            token("白", "シロ", 0..3),
            token("上", "ウエ", 3..6),
            token("氏", "シ", 6..9),
        ];
        pass.apply(&mut t);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].surface, "白上");
        assert_eq!(t[0].reading, "シラカミ");
        assert_eq!(t[0].range, 0..6);
        assert_eq!(t[1].surface, "氏");
        assert_eq!(t[1].reading, "シ");
        assert_eq!(t[1].range, 6..9);
    }

    #[test]
    fn equal_reading_merge_skipped() {
        // 結合読みが現状の連結と同じなら構造を触らない (毎日 | 新聞 | さん)
        let dict = Dict::new();
        let a = analyzer();
        let pass = NameBoundaryPass::new(&dict, &a);
        let mut t = vec![
            token("毎日", "マイニチ", 0..6),
            token("新聞", "シンブン", 6..12),
            token("さん", "サン", 12..18),
        ];
        pass.apply(&mut t);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].surface, "毎日");
        assert_eq!(t[1].surface, "新聞");
    }

    #[test]
    fn plain_kamisama_untouched() {
        // 直前が漢字でなければ発火しない (の | 神様)
        let dict = Dict::new();
        let a = analyzer();
        let pass = NameBoundaryPass::new(&dict, &a);
        let mut t = vec![token("の", "ノ", 0..3), token("神様", "カミサマ", 3..9)];
        pass.apply(&mut t);
        assert_eq!(t[1].surface, "神様");
        assert_eq!(t[1].reading, "カミサマ");
    }

    #[test]
    fn bare_suffix_token_untouched() {
        // t[i] が敬称そのもの (X 空) なら対象外 (お疲れ | 様)
        let dict = Dict::new();
        let a = analyzer();
        let pass = NameBoundaryPass::new(&dict, &a);
        let mut t = vec![token("疲れ", "ツカレ", 0..6), token("様", "サマ", 6..9)];
        pass.apply(&mut t);
        assert_eq!(t[0].surface, "疲れ");
        assert_eq!(t[1].surface, "様");
    }

    #[test]
    fn unknown_combination_untouched() {
        // 結合 surface が dict にも IPADIC 固有名詞にも無ければ何もしない
        let dict = Dict::new();
        let a = analyzer();
        let pass = NameBoundaryPass::new(&dict, &a);
        let mut t = vec![token("机", "ツクエ", 0..3), token("神様", "カミサマ", 3..9)];
        pass.apply(&mut t);
        assert_eq!(t[0].surface, "机");
        assert_eq!(t[1].surface, "神様");
        assert_eq!(t[1].reading, "カミサマ");
    }

    #[test]
    fn non_adjacent_tokens_untouched() {
        // range が隣接していなければ発火しない (= 間に protect token 等が居た形跡)
        let mut dict = Dict::new();
        dict.insert("戌神", "いぬがみ");
        let a = analyzer();
        let pass = NameBoundaryPass::new(&dict, &a);
        let mut t = vec![token("戌", "イヌ", 0..3), token("神様", "カミサマ", 6..12)];
        pass.apply(&mut t);
        assert_eq!(t[0].surface, "戌");
        assert_eq!(t[1].surface, "神様");
    }

    #[test]
    fn long_prev_untouched() {
        // 直前 token が 4 字以上なら名前姓部とみなさない
        let mut dict = Dict::new();
        dict.insert("春夏秋冬神", "しゅんかしゅうとうがみ");
        let a = analyzer();
        let pass = NameBoundaryPass::new(&dict, &a);
        let mut t = vec![
            token("春夏秋冬", "シュンカシュウトウ", 0..12),
            token("神様", "カミサマ", 12..18),
        ];
        pass.apply(&mut t);
        assert_eq!(t[1].surface, "神様");
    }
}

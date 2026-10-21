//! 解析結果から OpenJTalk 方式のアクセント句列を作る (連結規則の結線)。
//!
//! 流れ:
//! 1. 入力全体を Lindera (IPADIC) で形態素解析して品詞を得る (文脈つき。 「で」 のような語の品詞が決まる)
//! 2. こちらの採択 token を形態素に重ねる。 token の読みが形態素の読みの連結と同じなら **形態素ごとのノード** に展開
//!    (OpenJTalk の切り方に近づく)、 違う (辞書で読みを直した語) なら token 全体を 1 ノードにする
//! 3. ノードごとに 語単体の核 / 拍数 (アクセント表) と 結合型 (付属語の表 + 内容語の既定 + 例外の表) を付ける
//! 4. `crate::accent_chain::build_phrases` で句を組み立てる
//!
//! 表は dict の `role = "accent_chain"` (`tools/gen_accent_chain.py` が naist-jdic から生成)。

use self::nodes::make_nodes;
use self::table::pos_of;
use crate::accent_chain::{build_phrases, MeishiKind, Pos};
use crate::analyzer::MorphToken;
use crate::scoring::accent_lexicon::AccentLexicon;
use crate::scoring::analyze::Token;

/// 組み立てたアクセント句 1 つ。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChainedPhrase {
    /// 句の表記 (入力の部分文字列)
    pub surface: String,
    /// 句の読み (カタカナ)
    pub reading: String,
    /// 拍数
    pub mora: usize,
    /// 核位置 (0 = 平板、 n = n 拍目で下がる)
    pub accent: usize,
    /// 核に推定が混ざっているか (語単体の核が表に無く 平板で埋めた / rule 推定の核を使った)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub estimated: bool,
    /// 発音表記の読み (助詞の は / へ / を を ワ / エ / オ にしたもの。 TTS の記号列用)
    #[serde(skip)]
    pub sound: String,
    /// この句の前にあった、 読みの無い token (句読点・空白・記号) の表記。 句の切れ目の強さの元
    #[serde(skip)]
    pub gap_before: String,
}

mod nodes;
mod numerals;
mod table;

pub(crate) use table::ChainTable;

/// IPADIC の品詞 → 連結規則の品詞。 戻り値の bool = 連用形
pub(super) fn map_pos(m: &MorphToken) -> (Pos, bool) {
    let p1 = m.pos.as_deref().unwrap_or("");
    let d1 = m.pos_detail.as_deref().unwrap_or("");
    let d2 = m.pos_detail2.as_deref().unwrap_or("");
    let d3 = m.pos_detail3.as_deref().unwrap_or("");
    let renyou = m
        .conjugation_form
        .as_deref()
        .is_some_and(|f| f.starts_with("連用"));
    let pos = match (p1, d1, d2, d3) {
        ("名詞", "固有名詞", "人名", "姓") => Pos::Meishi(MeishiKind::Sei),
        ("名詞", "固有名詞", "人名", "名") => Pos::Meishi(MeishiKind::Mei),
        _ => pos_of(p1, d1),
    };
    (pos, renyou)
}

pub(super) fn is_kana_char(c: char) -> bool {
    matches!(c, 'ァ'..='ヶ' | 'ー')
}

/// 拍数 (カナだけを数える。 小書きの拗音は前の拍に含める。 句読点などの記号は 0 拍)
pub(super) fn count_mora(kata: &str) -> usize {
    kata.chars()
        .filter(|&c| {
            is_kana_char(c) && !matches!(c, 'ャ' | 'ュ' | 'ョ' | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ')
        })
        .count()
}

/// 形態素を 入力テキスト上の byte 範囲に並べる (順に surface を探す)。 見つからない形態素は `None`
pub(super) fn align(text: &str, morphs: &[MorphToken]) -> Vec<Option<(usize, usize)>> {
    let mut cursor = 0usize;
    let mut out = Vec::with_capacity(morphs.len());
    for m in morphs {
        match text[cursor..].find(m.surface.as_str()) {
            Some(off) if off <= 8 => {
                let s = cursor + off;
                let e = s + m.surface.len();
                out.push(Some((s, e)));
                cursor = e;
            }
            _ => out.push(None),
        }
    }
    out
}

/// 採択 token 列 + 形態素列から アクセント句を作る。
///
/// - `text` = 解析に使った (正規化後の) 入力
/// - `tokens` = その上の採択 token (range は `text` 上の byte 範囲)
pub(crate) fn build_chained_phrases(
    text: &str,
    tokens: &[Token],
    morphs: &[MorphToken],
    lexicon: &AccentLexicon,
    table: &ChainTable,
) -> Vec<ChainedPhrase> {
    build_chained_with_gap(text, tokens, morphs, lexicon, table).0
}

/// [`build_chained_phrases`] に加えて、 最後の句の後ろに残った読みの無い token の表記 (入力末の句読点など) を返す。
pub(crate) fn build_chained_with_gap(
    text: &str,
    tokens: &[Token],
    morphs: &[MorphToken],
    lexicon: &AccentLexicon,
    table: &ChainTable,
) -> (Vec<ChainedPhrase>, String) {
    let (mut nodes, meta) = make_nodes(text, tokens, morphs, lexicon, table);
    if nodes.is_empty() {
        return (Vec::new(), String::new());
    }
    let phrases = build_phrases(&mut nodes);
    let mut out = Vec::new();
    let mut gap = String::new();
    for p in phrases {
        let part = &meta[p.start..p.end];
        if p.morae == 0 {
            gap.extend(part.iter().map(|m| m.surface.as_str()));
            continue;
        }
        let sound: String = part
            .iter()
            .map(|m| match (m.surface.as_str(), m.pos.starts_with("助詞")) {
                ("は", true) => "ワ",
                ("へ", true) => "エ",
                ("を", true) => "オ",
                _ => m.reading.as_str(),
            })
            .collect();
        out.push(ChainedPhrase {
            surface: part.iter().map(|m| m.surface.as_str()).collect(),
            reading: part.iter().map(|m| m.reading.as_str()).collect(),
            mora: p.morae,
            accent: p.accent,
            estimated: part.iter().any(|m| m.estimated),
            sound,
            gap_before: std::mem::take(&mut gap),
        });
    }
    (out, gap)
}

/// 診断用: 語単体の核が表に無いノードの (表記, 読み, 品詞)
pub(crate) fn missing_accents(
    text: &str,
    tokens: &[Token],
    morphs: &[MorphToken],
    lexicon: &AccentLexicon,
    table: &ChainTable,
) -> Vec<(String, String, String)> {
    let (_, meta) = make_nodes(text, tokens, morphs, lexicon, table);
    meta.into_iter()
        .filter(|m| m.estimated && !m.reading.is_empty())
        .map(|m| (m.surface, m.reading, m.pos))
        .collect()
}

/// 診断用: 各ノードの (表記, 読み, 品詞, 前の語にくっついたか)
pub(crate) fn debug_nodes(
    text: &str,
    tokens: &[Token],
    morphs: &[MorphToken],
    lexicon: &AccentLexicon,
    table: &ChainTable,
) -> Vec<(String, String, String, bool)> {
    let (mut nodes, meta) = make_nodes(text, tokens, morphs, lexicon, table);
    if nodes.is_empty() {
        return Vec::new();
    }
    build_phrases(&mut nodes);
    nodes
        .iter()
        .zip(meta)
        .map(|(n, m)| (m.surface, m.reading, m.pos, n.chain_flag == Some(true)))
        .collect()
}

/// 診断用: 各ノードの (表記, 読み, 品詞, 拍数, 語単体の核, 推定か)。 句の連結の前の値
pub(crate) fn debug_nodes_detail(
    text: &str,
    tokens: &[Token],
    morphs: &[MorphToken],
    lexicon: &AccentLexicon,
    table: &ChainTable,
) -> Vec<(String, String, String, usize, usize, bool)> {
    let (nodes, meta) = make_nodes(text, tokens, morphs, lexicon, table);
    nodes
        .iter()
        .zip(meta)
        .map(|(n, m)| (m.surface, m.reading, m.pos, n.morae, n.accent, m.estimated))
        .collect()
}

/// ノード 1 つの付随情報 (出力の組み立てと、 欠けている核の診断に使う)
pub(crate) struct NodeMeta {
    pub surface: String,
    pub reading: String,
    pub estimated: bool,
    /// 品詞 (IPADIC の 大分類,細分類1)。 診断用
    pub pos: String,
}

#[cfg(test)]
mod tests;

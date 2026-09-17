//! コスト lattice engine (ADR-0011) — IPADIC の単語 + 連接コストと dict 候補を
//! 1 本の lattice に並べ、 コスト最小 path を選ぶ。
//!
//! ## 現行 (band) との関係
//!
//! band の序列はそのまま **コストの序列** に翻訳する:
//!
//! | 候補 | コスト |
//! |---|---|
//! | IPADIC 語 | `word_cost` + 連接コスト (そのまま) |
//! | dict entry (2 字以上) で IPADIC に同じ区切りがある | その語のコスト (= 読みの上書きだけ) |
//! | dict entry (2 字以上) で IPADIC に無い区切り | [`NEW_WORD_COST`] (= 区切りの主張) |
//! | dict の単漢字 / `[[kanji]]` default | 単独の edge にせず、 **同じ区切りの IPADIC 語の読みを上書き** |
//! | 上記が無い 1 文字 | [`UNK_COST`] (最後の手段) |
//!
//! 単漢字を独立した edge として競わせると 「何ですか」 が IPADIC の ナニ に負け、
//! 逆に割引を与えると語が砕ける (北海道 → キタウミミチ)。 読みの上書きに限るのが
//! 現行の 「band 100 [[kanji]] > band 50 Lindera」 と同じ意味になる (ADR-0011)。

use crate::analyzer::Analyzer;
use crate::scoring::candidate::{
    Candidate, CandidateProvider, EdgeCost, RawCandidate, Score, ScoringContext, BAND_KANJI,
    BAND_LINDERA_COMPOUND,
};
use lindera::dictionary::Dictionary;

/// dict entry が IPADIC に無い区切りを主張する時の語コスト。
///
/// IPADIC の実語コストは概ね 3,000〜13,000 なので、 「確実にある語」 として
/// その下限付近に置く。
const NEW_WORD_COST: i32 = 2500;

/// dict / 数字 / 保護 token 由来の候補に与える割引。
///
/// 現行の band (1000 / 950 = dict 由来が IPADIC 語より常に優先) をコストで近似する。
/// 割引が小さいと 所為 = しょい / 六回戦 = ろくかいせん のように IPADIC 側が採られ、
/// 大きすぎると語が砕ける。 `FURIGANA_COST_DICT_DISCOUNT` で実験できる。
fn dict_discount() -> i32 {
    std::env::var("FURIGANA_COST_DICT_DISCOUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DICT_DISCOUNT)
}

const DICT_DISCOUNT: i32 = 4000;

/// どの辞書にも無い 1 文字 (最後の手段) の語コスト。
const UNK_COST: i32 = 30000;

/// dict 候補に割り当てる連接 id を引く surface (= 名詞,一般 相当)。
const NOUN_ID_SAMPLE: &str = "猫";

/// IPADIC の全単語を edge として保持する provider。
///
/// 構築時に **入力の全位置** を 1 回走査して前方一致を集める
/// (`candidates_at` は start 昇順配列の二分探索)。 Lindera の最良解 1 本ではなく
/// **lattice 全体** を渡すのが現行 [`crate::scoring::lindera_fallback`] との違い。
pub struct IpadicLatticeProvider {
    /// (start, end, reading, is_name, cost) の edge 配列 (start 昇順)
    edges: Vec<IpadicEdge>,
    /// dict 候補に使う (left, right)
    noun_ids: (u16, u16),
}

struct IpadicEdge {
    start: usize,
    end: usize,
    reading: String,
    is_name: bool,
    /// 品詞 = 動詞 / 形容詞 (活用を踏まえた読みなので単漢字 override を当てない)
    is_inflected: bool,
    cost: EdgeCost,
}

impl IpadicLatticeProvider {
    /// `input` の全位置で IPADIC 前方一致を集めて provider を作る。
    #[must_use]
    pub fn new(analyzer: &Analyzer, input: &str) -> Self {
        analyzer.with_dictionary(|dict| Self::build(dict, input))
    }

    fn build(dict: &Dictionary, input: &str) -> Self {
        let prefix = &dict.prefix_dictionary;
        let noun_ids = prefix
            .prefix(NOUN_ID_SAMPLE)
            .next()
            .map_or((1285, 1285), |(_, e)| (e.left_id, e.right_id));

        let mut edges: Vec<IpadicEdge> = Vec::new();
        for (pos, _) in input.char_indices() {
            for (end_rel, entry) in prefix.prefix(&input[pos..]) {
                let details = dict.word_details(entry.word_id.id as usize);
                let surface = &input[pos..pos + end_rel];
                let reading = details
                    .get(FIELD_READING)
                    .filter(|r| **r != "*" && !r.is_empty())
                    .map_or_else(|| surface.to_string(), |r| (*r).to_string());
                let is_name = details.first() == Some(&"名詞")
                    && details.get(1) == Some(&"固有名詞")
                    && details.get(2) == Some(&"人名");
                // 送り仮名なしで 1 字になる動詞語幹は一段動詞に限られる (寝る / 出る / 見る)。
                // この形だけ IPADIC の活用込みの読みを尊重し、 単漢字 default で潰さない
                // (2026-09-18 の band engine 側 fix と同じ条件)。
                let is_inflected = details.first() == Some(&"動詞")
                    && details.get(1) == Some(&"自立")
                    && details.get(4).is_some_and(|t| t.starts_with("一段"))
                    && surface.chars().count() == 1;
                edges.push(IpadicEdge {
                    start: pos,
                    end: pos + end_rel,
                    reading,
                    is_name,
                    is_inflected,
                    cost: EdgeCost {
                        word_cost: i32::from(entry.word_cost),
                        left: entry.left_id,
                        right: entry.right_id,
                    },
                });
            }
        }
        edges.sort_by_key(|e| e.start);
        Self { edges, noun_ids }
    }

    /// dict 候補に割り当てる連接 id (名詞,一般 相当)。
    #[must_use]
    pub fn noun_ids(&self) -> (u16, u16) {
        self.noun_ids
    }

    /// `pos` から始まる IPADIC edge の範囲。
    fn range_at(&self, pos: usize) -> &[IpadicEdge] {
        let lo = self.edges.partition_point(|e| e.start < pos);
        let hi = lo + self.edges[lo..].partition_point(|e| e.start == pos);
        &self.edges[lo..hi]
    }
}

/// IPADIC details の reading field (IPADIC は details[7] = カタカナ読み)。
#[cfg(all(feature = "dict-ipadic", not(feature = "dict-unidic")))]
const FIELD_READING: usize = 7;
#[cfg(all(feature = "dict-unidic", not(feature = "dict-ipadic")))]
const FIELD_READING: usize = 9;

impl CandidateProvider for IpadicLatticeProvider {
    fn candidates_at<'b>(
        &'b self,
        ctx: &ScoringContext<'b>,
        pos: usize,
        out: &mut Vec<RawCandidate<'b>>,
    ) {
        // どの layer も覆わない位置で path が切れないよう、 1 文字 passthrough を常に置く
        // (コストは最大なので実語があれば必ず負ける)。
        if let Some(c) = ctx.input[pos..].chars().next() {
            let end = pos + c.len_utf8();
            out.push(
                RawCandidate::new(&ctx.input[pos..end], pos..end, Score::lindera(1))
                    .with_edge_cost(EdgeCost {
                        word_cost: UNK_COST,
                        left: self.noun_ids.0,
                        right: self.noun_ids.1,
                    }),
            );
        }
        for e in self.range_at(pos) {
            let length = u8::try_from(e.end - e.start).unwrap_or(u8::MAX);
            // 用言 (動詞 / 形容詞) は活用込みの読みなので、 単漢字 override の対象外に
            // するため band を分ける (BAND_LINDERA_COMPOUND = 150 を流用)。
            let score = if e.is_inflected {
                Score::lindera_compound(length)
            } else {
                Score::lindera(length)
            };
            out.push(
                RawCandidate::new(e.reading.as_str(), e.start..e.end, score)
                    .with_name_flag(e.is_name)
                    .with_edge_cost(e.cost),
            );
        }
    }
}

/// コスト最小 path を解く (現行 [`crate::scoring::engine::solve_path`] のコスト版)。
///
/// `noun_ids` は dict 候補に割り当てる連接 id。 連接コストは IPADIC の連接表から引く。
#[must_use]
pub fn solve_path_cost<'a>(
    ctx: &ScoringContext<'a>,
    providers: &[&'a dyn CandidateProvider],
    analyzer: &Analyzer,
    noun_ids: (u16, u16),
) -> Vec<Candidate> {
    let n = ctx.input.len();
    if n == 0 {
        return Vec::new();
    }
    analyzer.with_dictionary(|dict| {
        let conn = &dict.connection_cost_matrix;
        let discount = dict_discount();
        // dp[i] = (到達コスト, 直前 edge の right_id, その edge が dict 由来か)
        let mut dp: Vec<Option<(i32, u16)>> = vec![None; n + 1];
        // 同コスト時の決着用: (dict 由来か, match_hits, weight)
        let mut dp_rank: Vec<(bool, u8, u8)> = vec![(false, 0, 0); n + 1];
        dp[0] = Some((0, 0));
        let mut parent: Vec<Option<(usize, RawCandidate<'a>)>> = vec![None; n + 1];
        let mut all: Vec<RawCandidate<'a>> = Vec::new();

        for pos in 0..n {
            let Some((cur_cost, cur_right)) = dp[pos] else {
                continue;
            };
            all.clear();
            for provider in providers {
                provider.candidates_at(ctx, pos, &mut all);
            }
            apply_single_char_overrides(&mut all);
            // dict / 数字 / 保護 token 由来か (= コスト割り当て前に `edge` が無いもの)
            let authored: Vec<bool> = all.iter().map(|c| c.edge.is_none()).collect();
            assign_costs(&mut all, noun_ids, discount);

            for (cand, authored) in all.drain(..).zip(authored) {
                if cand.range.start != pos {
                    continue;
                }
                let next = cand.range.end;
                if next > n || next <= pos {
                    continue;
                }
                let edge = cand.edge.unwrap_or(EdgeCost {
                    word_cost: UNK_COST,
                    left: noun_ids.0,
                    right: noun_ids.1,
                });
                let cost = cur_cost
                    + conn.cost(u32::from(cur_right), u32::from(edge.left))
                    + edge.word_cost;
                // 同コストの決着:
                // 1. dict / 数字 / 保護 token 由来を優先する (= 辞書が書いた読みが勝つ。
                //    同区切りの IPADIC 語とはコストが同じになるため、 これが無いと
                //    所為 = しょい / 五日 = ごにち のように IPADIC 側が採られる)
                // 2. どちらも同じ出自なら後勝ち (IPADIC は 剥がさ に ヘガサ / ハガサ を
                //    同コストで持ち、 先勝ちだと Lindera 本体と違う方を選ぶ)
                let rank = (authored, cand.score.match_hits, cand.score.weight);
                let better = match dp[next] {
                    None => true,
                    Some((old, _)) if cost < old => true,
                    Some((old, _)) if cost > old => false,
                    // 同コスト: dict 由来 → match_hits → weight の順で決める
                    // (現行 band engine の tie-break と同じ軸)。 それも同じなら後勝ち
                    // (IPADIC が 剥がさ に ヘガサ / ハガサ を同コストで持つケース)。
                    _ => rank >= dp_rank[next],
                };
                if better {
                    dp[next] = Some((cost, edge.right));
                    dp_rank[next] = rank;
                    parent[next] = Some((pos, cand));
                }
            }
        }

        if dp[n].is_none() {
            return Vec::new();
        }
        let mut path: Vec<Candidate> = Vec::new();
        let mut pos = n;
        while pos > 0 {
            let Some((prev, cand)) = parent[pos].take() else {
                return Vec::new();
            };
            path.push(cand.into_candidate(ctx.input));
            pos = prev;
        }
        path.reverse();
        path
    })
}

/// dict の単漢字候補 (band ≤ 100) は独立した edge にせず、 **同じ区切りの IPADIC 語の
/// 読みを上書き** する (ADR-0011)。 同じ区切りの IPADIC 語が無い時だけ edge として残す。
fn apply_single_char_overrides(all: &mut Vec<RawCandidate<'_>>) {
    let singles: Vec<(std::ops::Range<usize>, String)> = all
        .iter()
        .filter(|c| c.edge.is_none() && c.score.band <= BAND_KANJI && c.score.length == 1)
        .map(|c| (c.range.clone(), c.reading.to_string()))
        .collect();
    if singles.is_empty() {
        return;
    }
    for (range, reading) in &singles {
        let mut overrode = false;
        for c in all.iter_mut() {
            // 用言 (band 150 で印をつけた IPADIC 語) は上書きしない
            // (寝 = ネ / 見 = ミ / 経 = ヘ を単漢字 default の シン / ケン / ケイ で潰さない)
            // 一段動詞語幹 (band 150 で印をつけた IPADIC 語) は上書きしない
            // (寝 = ネ / 見 = ミ / 経 = ヘ を単漢字 default の シン / ケン / ケイ で潰さない)
            if c.edge.is_some() && c.range == *range && c.score.band != BAND_LINDERA_COMPOUND {
                c.reading = reading.clone().into();
                overrode = true;
            }
        }
        if overrode {
            all.retain(|c| !(c.edge.is_none() && c.range == *range && c.score.length == 1));
        }
    }
}

/// dict / 数字 / 保護 token 由来の候補 (= `edge` が無い) にコストを割り当てる。
///
/// 同じ区切りの IPADIC 語があるならそのコストを使う (= 読みの上書きだけ)。
/// 無ければ [`NEW_WORD_COST`] で 1 語として置く (= 区切りの主張)。
fn assign_costs(all: &mut Vec<RawCandidate<'_>>, noun_ids: (u16, u16), discount: i32) {
    let spans: Vec<(std::ops::Range<usize>, EdgeCost)> = all
        .iter()
        .filter_map(|c| c.edge.map(|e| (c.range.clone(), e)))
        .collect();
    for c in all.iter_mut() {
        if c.edge.is_some() {
            continue;
        }
        let same = spans
            .iter()
            .filter(|(r, _)| *r == c.range)
            .min_by_key(|(_, e)| e.word_cost);
        // 長い entry ほど強く優先する (= 味噌汁 が 味噌 + 汁 の分割に負けないように)。
        let scale = if std::env::var("FURIGANA_COST_NO_SCALE").is_ok() {
            1
        } else {
            i32::from(c.score.length.max(1))
        };
        let discount = discount * scale;
        c.edge = Some(match same {
            // 同じ区切りの IPADIC 語がある → その連接 id を借りて読みだけ差し替える
            Some((_, e)) => EdgeCost {
                word_cost: e.word_cost - discount,
                ..*e
            },
            None => EdgeCost {
                word_cost: NEW_WORD_COST - discount,
                left: noun_ids.0,
                right: noun_ids.1,
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::Furigana;

    fn engines(input: &str) -> (String, String) {
        let band = Furigana::minimal().expect("band");
        band.preload().expect("preload");
        let cost = Furigana::builder().cost_engine(true).build().expect("cost");
        cost.preload().expect("preload");
        (band.to_hiragana(input), cost.to_hiragana(input))
    }

    /// cost engine でも入力は必ず覆われる (path が組めず空を返さない)。
    /// 記号 / 英字 / 絵文字記法のように IPADIC に語が無い位置でも 1 文字 passthrough で繋ぐ。
    #[test]
    fn cost_engine_covers_every_input() {
        for input in [
            "猫が好き",
            "これは何ですか?",
            "https://example.com を見て",
            "あ",
        ] {
            let (_, cost) = engines(input);
            assert!(!cost.is_empty(), "cost engine が空を返した: {input:?}");
        }
    }

    /// IPADIC だけで読める文は band engine と一致する (minimal dict = dict 由来の差が無い)。
    #[test]
    fn cost_engine_matches_band_engine_on_plain_sentence() {
        for input in ["猫が好きです", "今日は良い天気だ", "本を読んでいる"] {
            let (band, cost) = engines(input);
            assert_eq!(band, cost, "{input:?}");
        }
    }
}

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
};
use crate::scoring::lindera_fallback::is_real_cjk_ideograph;
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

const DICT_DISCOUNT: i32 = 2000;

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
            // band は現行 [`crate::scoring::lindera_fallback`] と同じ規則:
            // 2 字以上 + 全 char 漢字の語だけ 150、 それ以外は 50。
            let surface = &ctx.input[e.start..e.end];
            let char_count = surface.chars().count();
            let score = if char_count >= 2 && surface.chars().all(is_real_cjk_ideograph) {
                Score::lindera_compound(u8::try_from(char_count).unwrap_or(u8::MAX))
            } else {
                Score::lindera(u8::try_from(char_count).unwrap_or(u8::MAX))
            };
            let mut cand = RawCandidate::new(e.reading.as_str(), e.start..e.end, score)
                .with_name_flag(e.is_name)
                .with_edge_cost(e.cost);
            if e.is_inflected {
                // 一段動詞語幹は活用込みの読みなので単漢字 default で上書きしない
                cand = cand.keep_reading();
            }
            out.push(cand);
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

        // ── 1. 全位置の候補を集める ────────────────────────────────────────
        // 候補は **1 つずつが lattice の node**。 位置ごとに 1 状態へ潰すと、
        // 連接コストに要る 「直前の語の品詞 (right_id)」 が失われ、 同じ区間の
        // 読み違い (山中 = ヤマナカ / サンチュウ) を選び分けられない。
        let mut cands: Vec<Node<'a>> = Vec::new();
        let mut starts_at: Vec<Vec<usize>> = vec![Vec::new(); n + 1];
        let mut ends_at: Vec<Vec<usize>> = vec![Vec::new(); n + 1];
        let mut buf: Vec<RawCandidate<'a>> = Vec::new();
        for (pos, _) in ctx.input.char_indices() {
            buf.clear();
            for provider in providers {
                provider.candidates_at(ctx, pos, &mut buf);
            }
            apply_single_char_overrides(&mut buf);
            let authored: Vec<bool> = buf.iter().map(|c| c.edge.is_none()).collect();
            assign_costs(&mut buf, noun_ids, discount);
            for (cand, authored) in buf.drain(..).zip(authored) {
                if cand.range.start != pos || cand.range.end > n || cand.range.end <= pos {
                    continue;
                }
                let edge = cand.edge.unwrap_or(EdgeCost {
                    word_cost: UNK_COST,
                    left: noun_ids.0,
                    right: noun_ids.1,
                });
                let end = cand.range.end;
                starts_at[pos].push(cands.len());
                ends_at[end].push(cands.len());
                cands.push(Node {
                    cand,
                    edge,
                    authored,
                    best: None,
                    prev: None,
                });
            }
        }

        // ── 2. lattice Viterbi (node = 候補 edge) ──────────────────────────
        #[allow(clippy::needless_range_loop)] // 位置順に走査する必要がある
        for pos in 0..n {
            #[allow(clippy::needless_range_loop)] // starts_at / cands を同時に触るため index で回す
            for i in 0..starts_at[pos].len() {
                let idx = starts_at[pos][i];
                let (edge, band, hits, authored, weight) = {
                    let node = &cands[idx];
                    (
                        node.edge,
                        node.cand.score.band,
                        node.cand.score.match_hits,
                        node.authored,
                        node.cand.score.weight,
                    )
                };
                let mut best: Option<(PathCost, Option<usize>)> = None;
                if pos == 0 {
                    // BOS (連接 id 0)
                    let c = PathCost::START.add_edge(
                        band,
                        conn.cost(0, u32::from(edge.left)) + edge.word_cost,
                        hits,
                        authored,
                        weight,
                    );
                    best = Some((c, None));
                }
                for &prev_idx in &ends_at[pos] {
                    let Some(prev_best) = cands[prev_idx].best else {
                        continue;
                    };
                    let prev_right = cands[prev_idx].edge.right;
                    let c = prev_best.add_edge(
                        band,
                        conn.cost(u32::from(prev_right), u32::from(edge.left)) + edge.word_cost,
                        hits,
                        authored,
                        weight,
                    );
                    // 同着は後勝ち (IPADIC が同コストで 2 通りの読みを持つ場合)
                    let take = best.is_none_or(|(old, _)| !old.better_than(&c));
                    if take {
                        best = Some((c, Some(prev_idx)));
                    }
                }
                if let Some((c, prev)) = best {
                    cands[idx].best = Some(c);
                    cands[idx].prev = prev;
                }
            }
        }

        // ── 3. EOS で最良を選んで backtrack ────────────────────────────────
        let mut end_best: Option<(PathCost, usize)> = None;
        for &idx in &ends_at[n] {
            let Some(best) = cands[idx].best else {
                continue;
            };
            // EOS への連接コストを足して比較 (同着は後勝ち)
            let c = best.add_edge(
                u16::MAX,
                conn.cost(u32::from(cands[idx].edge.right), 0),
                0,
                false,
                0,
            );
            if end_best.is_none_or(|(old, _)| !old.better_than(&c)) {
                end_best = Some((c, idx));
            }
        }
        let Some((_, mut idx)) = end_best else {
            return Vec::new();
        };
        let mut path_idx = vec![idx];
        while let Some(prev) = cands[idx].prev {
            idx = prev;
            path_idx.push(idx);
        }
        path_idx.reverse();
        let mut taken: Vec<Option<Node<'a>>> = cands.into_iter().map(Some).collect();
        path_idx
            .into_iter()
            .filter_map(|i| taken[i].take())
            .map(|node| node.cand.into_candidate(ctx.input))
            .collect()
    })
}

/// lattice の node (= 候補 edge 1 つ) と、 そこへ到達する最良 path。
struct Node<'a> {
    cand: RawCandidate<'a>,
    edge: EdgeCost,
    /// dict / 数字 / 保護 token 由来か
    authored: bool,
    /// この node で終わる最良 path の評価値
    best: Option<PathCost>,
    /// その path の 1 つ前の node
    prev: Option<usize>,
}

/// path の評価値: **最弱 band → 総コスト** の 2 段。
///
/// 第 1 軸は現行 band engine と同じ 「path 中で最も弱い band」。 dict entry (1000) や
/// 助数詞 (950) を含む path は、 IPADIC 語だけの path に band で勝つ
/// (= 五日 = イツカ が 五 + 日 に負けない)。 band が並ぶところ
/// (= 現行 engine が edge 数で誤っていた領域) を IPADIC の総コストで裁く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PathCost {
    /// path 中の最小 band (大きいほど良い)
    weakest_band: u16,
    /// edge 数 (少ないほど良い)
    edges: u32,
    /// IPADIC の語コスト + 連接コストの合計 (小さいほど良い)
    total: i32,
    /// dict match の hit 数の合計 (大きいほど良い)
    hits: u32,
    /// dict / 数字 / 保護 token 由来 edge の数 (大きいほど良い)
    authored: u32,
    /// dict weight の合計 (大きいほど良い。 primary 100 / alt は dict 指定値)
    weight: u32,
}

impl PathCost {
    const START: Self = Self {
        weakest_band: u16::MAX,
        edges: 0,
        total: 0,
        hits: 0,
        authored: 0,
        weight: 0,
    };

    fn add_edge(self, band: u16, cost: i32, hits: u8, authored: bool, weight: u8) -> Self {
        Self {
            weakest_band: self.weakest_band.min(band),
            edges: self.edges + 1,
            total: self.total + cost,
            hits: self.hits + u32::from(hits),
            authored: self.authored + u32::from(authored),
            weight: self.weight + u32::from(weight),
        }
    }

    /// 比較は **最弱 band → 総コスト → match_hits → dict 由来 edge 数** の順。
    ///
    /// コストが同点になるのは 「dict entry が同じ区切りの IPADIC 語のコストを借りている」
    /// 場合で、 そこは band engine と同じく match_hits と dict 由来を優先する
    /// (= 所為 = セイ / 五日 = イツカ が IPADIC の読みに負けない)。
    /// 比較は **最弱 band → edge 数 (少ない) → 総コスト → match_hits → weight → dict 由来数**。
    ///
    /// 前 2 軸は現行 band engine と同じ (= 辞書の序列と 「長い語を優先」)。
    /// コストを edge 数より先にすると、 数詞の連鎖が安いため 三百円 が 三 + 百 + 円 に割れる。
    /// 「dict entry が覆った文字数」 を第 2 軸にする案も試したが corpus 98.3% と悪化した。
    /// **コストはその次**: 上 2 軸が並ぶところ (= 現行 engine が決め手を持たず列挙順で
    /// 決めていた領域、 数値上げ = 数値 + 上げ と 数 + 値上げ が典型) を IPADIC の
    /// 語コスト + 連接コストで裁く。
    fn better_than(&self, other: &Self) -> bool {
        (
            self.weakest_band,
            std::cmp::Reverse(self.edges),
            std::cmp::Reverse(self.total),
            self.hits,
            self.weight,
            self.authored,
        ) > (
            other.weakest_band,
            std::cmp::Reverse(other.edges),
            std::cmp::Reverse(other.total),
            other.hits,
            other.weight,
            other.authored,
        )
    }
}

/// dict の単漢字候補 (band ≤ 100) は独立した edge にせず、 **同じ区切りの IPADIC 語の
/// 読みを上書き** する (ADR-0011)。 同じ区切りの IPADIC 語が無い時だけ edge として残す。
fn apply_single_char_overrides(all: &mut Vec<RawCandidate<'_>>) {
    let singles: Vec<(std::ops::Range<usize>, String, u16)> = all
        .iter()
        .filter(|c| c.edge.is_none() && c.score.band <= BAND_KANJI && c.score.length == 1)
        .map(|c| (c.range.clone(), c.reading.to_string(), c.score.band))
        .collect();
    if singles.is_empty() {
        return;
    }
    for (range, reading, band) in &singles {
        let mut overrode = false;
        for c in all.iter_mut() {
            // 用言 (band 150 で印をつけた IPADIC 語) は上書きしない
            // (寝 = ネ / 見 = ミ / 経 = ヘ を単漢字 default の シン / ケン / ケイ で潰さない)
            // 一段動詞語幹は活用込みの読みなので上書きしない
            // (寝 = ネ / 見 = ミ / 経 = ヘ を単漢字 default の シン / ケン / ケイ で潰さない)
            if c.edge.is_some() && c.range == *range && !c.keep_reading {
                c.reading = reading.clone().into();
                // band も dict 側 (= 100) を引き継ぐ: path 比較の第 1 軸が band なので、
                // 「dict が読みを決めた 1 字」 は IPADIC 語 (50) ではなく 100 として扱う。
                c.score.band = *band;
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
        let scale = i32::from(c.score.length.max(1));
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

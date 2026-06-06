//! Smart engine — Viterbi-like path 選択で input 全体の最良 candidate path を解く。
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §2 / §4
//!
//! ## algorithm
//!
//! 1. input 上の各 byte 位置で、 全 [`CandidateProvider`] から候補を収集
//! 2. DP table `dp[i]` = 位置 i に到達する最良 path score
//! 3. 各候補 edge `(pos → pos + len)` で `dp[pos + len]` を更新 (score 比較で勝者採用)
//! 4. `dp[n]` (input 末尾) から parent backtrack で path 復元
//!
//! 「最良」 は [`PathScore`] の lexicographic 比較で決定 (weakest_band → edge_count → match_hits)。
//!
//! ## 注意
//!
//! - input が candidate で覆い切れない (= dp[n] が unreachable) 場合は空 Vec を返す
//! - candidate range は valid (= `range.start <= range.end <= input.len()`) であること、 invalid range は skip
//! - 同 path score の場合は **第一発見** が勝つ (TOML 出現順 / provider 順依存)

use crate::scoring::candidate::{Candidate, CandidateProvider, Score, ScoringContext};
use std::cmp::Ordering;

/// path 全体の累積 score。
///
/// 各 edge の [`Score`] field を集約。 純 sum ではなく **weakest band + edge count**
/// ベース集約 (= longest match を自然に表現)。
///
/// ## 比較順 (lexicographic、 path 全体で同 endpoint の場合)
///
/// 1. `weakest_band` 大 — path 中の最低 band edge が高いほど勝ち (= 弱い edge を含まない)
/// 2. `edge_count` 小 — edge 数が少ない path が勝ち (= longest match preference、 fragmentation 回避)
/// 3. `total_match_hits` 多 — inline match condition hit 累積
///
/// ### なぜ純 sum ではないか
///
/// `total_band = sum(edge.band)` 集約だと、 同 band の edge が多い path (= 細かく分割された path)
/// の方が total が大きくなり、 longest match 思想と逆になる。 例: input 「魔理沙」 で
/// 「魔理沙」 単独 (band 1000) より 「魔」+「理沙」 (band 1000 × 2) の方が total_band 大で勝ってしまう。
///
/// `weakest_band` (= 最低 band) で比較することで 「弱い edge を含まない path 勝ち」 を表現、
/// 同 weakest なら `edge_count` で fragmentation 抑制 (= 過分割を抑える (a) longest match)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathScore {
    /// path 中の最低 band edge の band 値。 path が空の時は `u16::MAX` (= 弱点なし)。
    pub weakest_band: u16,
    /// path に含まれる edge 数。 fewer = better (longest match preference)。
    pub edge_count: u32,
    /// edge の `match_hits` 累積
    pub total_match_hits: u32,
}

impl PathScore {
    /// 空 path (= 何も edge を含まない) の score。 path 開始時の初期値。
    /// `weakest_band = u16::MAX` で 「弱点なし」 状態を表現。
    pub const ZERO: Self = Self {
        weakest_band: u16::MAX,
        edge_count: 0,
        total_match_hits: 0,
    };

    /// この path に edge `score` を追加した新 PathScore を返す。
    #[must_use]
    pub fn add_edge(self, score: &Score) -> Self {
        Self {
            weakest_band: self.weakest_band.min(score.band),
            edge_count: self.edge_count + 1,
            total_match_hits: self.total_match_hits + u32::from(score.match_hits),
        }
    }
}

impl Ord for PathScore {
    fn cmp(&self, other: &Self) -> Ordering {
        self.weakest_band
            .cmp(&other.weakest_band)
            // edge_count: 小 = better、 比較は逆方向
            .then(other.edge_count.cmp(&self.edge_count))
            .then(self.total_match_hits.cmp(&other.total_match_hits))
    }
}

impl PartialOrd for PathScore {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// `input` を覆う最良 candidate path を解く (Viterbi-like DP)。
///
/// 全 [`CandidateProvider`] から candidate を収集、 各 byte 位置で best path を更新、
/// 末尾 (`input.len()`) から backtrack で path 復元。
///
/// ## 戻り値
///
/// - 入力を覆う path が存在 → `Vec<Candidate>` (start から end への順)
/// - 空 input → 空 Vec
/// - 入力を覆い切る path が無い (= unreachable region あり) → 空 Vec
///
/// ## 複雑度
///
/// O(N × C) — N = input byte 長、 C = 各位置での平均候補数。
/// providers の中身次第、 通常は十分高速。
pub fn solve_path(ctx: &ScoringContext, providers: &[&dyn CandidateProvider]) -> Vec<Candidate> {
    let n = ctx.input.len();
    if n == 0 {
        return Vec::new();
    }

    // dp[i] = i に到達する最良 PathScore (None = 未到達)
    let mut dp: Vec<Option<PathScore>> = vec![None; n + 1];
    dp[0] = Some(PathScore::ZERO);

    // parent[i] = (prev_pos, candidate) — i に到達するために使った edge
    let mut parent: Vec<Option<(usize, Candidate)>> = vec![None; n + 1];

    for pos in 0..n {
        let Some(current_score) = dp[pos] else {
            continue; // 到達不能位置
        };

        // この位置から始まる候補を全 provider から収集
        let mut all_candidates: Vec<Candidate> = Vec::new();
        for provider in providers {
            all_candidates.extend(provider.candidates_at(ctx, pos));
        }

        for cand in all_candidates {
            // valid range の確認
            if cand.range.start != pos {
                continue; // provider が間違った位置の candidate を返した
            }
            let next_pos = cand.range.end;
            if next_pos > n || next_pos <= pos {
                continue; // overflow / 0-length / 後退は skip
            }

            let new_score = current_score.add_edge(&cand.score);

            // dp[next_pos] と比較、 better なら更新
            let better = match dp[next_pos] {
                None => true,
                Some(existing) => new_score > existing,
            };
            if better {
                dp[next_pos] = Some(new_score);
                parent[next_pos] = Some((pos, cand));
            }
        }
    }

    // dp[n] が未到達なら path 構築不能 (= 全体を覆う path がない)
    if dp[n].is_none() {
        return Vec::new();
    }

    // parent backtrack で path 復元
    let mut path: Vec<Candidate> = Vec::new();
    let mut pos = n;
    while pos > 0 {
        let Some((prev_pos, cand)) = parent[pos].take() else {
            // 矛盾: dp[pos] あるが parent なし → 空 Vec で諦める
            return Vec::new();
        };
        path.push(cand);
        pos = prev_pos;
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::boundary::BoundaryAnalysis;
    use crate::scoring::candidate::{Score, ScoringContext, BAND_DICT_EXACT, BAND_KANJI};

    // ─── PathScore lexicographic 比較 ────────────────────────────────────────

    #[test]
    fn path_score_zero_has_max_weakest_band() {
        // ZERO は 「弱点なし」 (= weakest_band = u16::MAX) で edge_count 0
        let zero = PathScore::ZERO;
        assert_eq!(zero.weakest_band, u16::MAX);
        assert_eq!(zero.edge_count, 0);
    }

    #[test]
    fn path_score_adding_edge_lowers_weakest_band() {
        let after = PathScore::ZERO.add_edge(&Score::kanji(1));
        // edge band = 100 → weakest = min(MAX, 100) = 100
        assert_eq!(after.weakest_band, 100);
        assert_eq!(after.edge_count, 1);
    }

    #[test]
    fn path_score_higher_weakest_band_wins() {
        // 単独 dict 1000 path vs 単独 kanji 100 path
        let a = PathScore::ZERO.add_edge(&Score::dict_exact(2));
        let b = PathScore::ZERO.add_edge(&Score::kanji(5));
        assert!(a > b, "weakest 1000 > weakest 100");
    }

    #[test]
    fn path_score_fewer_edges_wins_with_same_weakest_band() {
        // 単独 dict 1000 (1 edge) vs 同 band 2 edges
        let single = PathScore::ZERO.add_edge(&Score::dict_exact(3));
        let split = PathScore::ZERO
            .add_edge(&Score::dict_exact(1))
            .add_edge(&Score::dict_exact(2));
        // weakest_band は同じ (1000)、 edge_count で 1 < 2 → single 勝ち
        assert_eq!(single.weakest_band, split.weakest_band);
        assert!(single > split, "fewer edges wins on weakest_band tie");
    }

    #[test]
    fn path_score_weakest_band_dominated_by_lowest_edge() {
        // 1 edge は band 1000、 もう 1 edge は band 100 の混在 path
        // → weakest = 100
        let mixed = PathScore::ZERO
            .add_edge(&Score::dict_exact(2))
            .add_edge(&Score::kanji(1));
        assert_eq!(mixed.weakest_band, 100);

        // 純 1000 path との比較で mixed は負ける
        let pure = PathScore::ZERO.add_edge(&Score::dict_exact(3));
        assert!(pure > mixed);
    }

    #[test]
    fn path_score_more_match_hits_wins() {
        let with_hit = PathScore::ZERO.add_edge(&Score::new(BAND_DICT_EXACT, 2, 1));
        let no_hit = PathScore::ZERO.add_edge(&Score::new(BAND_DICT_EXACT, 2, 0));
        assert!(with_hit > no_hit);
    }

    // ─── solve_path ──────────────────────────────────────────────────────────

    /// dummy provider: 指定 surface→reading mapping を 全位置で試行
    struct DictProvider {
        entries: Vec<(String, String, Score)>,
    }

    fn ctx(input: &str) -> ScoringContext<'_> {
        let boundary = Box::leak(Box::new(BoundaryAnalysis::empty()));
        ScoringContext { input, boundary }
    }

    impl CandidateProvider for DictProvider {
        fn candidates_at(&self, ctx: &ScoringContext, pos: usize) -> Vec<Candidate> {
            let mut out = Vec::new();
            for (surface, reading, score) in &self.entries {
                if ctx.input[pos..].starts_with(surface.as_str()) {
                    let end = pos + surface.len();
                    out.push(Candidate::new(
                        surface.clone(),
                        reading.clone(),
                        pos..end,
                        *score,
                    ));
                }
            }
            out
        }
    }

    /// dummy provider: 各 1 文字を band 100 (kanji 相当) で返す全位置 fallback
    struct CharProvider {
        score: Score,
    }

    impl CandidateProvider for CharProvider {
        fn candidates_at(&self, ctx: &ScoringContext, pos: usize) -> Vec<Candidate> {
            let Some(c) = ctx.input[pos..].chars().next() else {
                return Vec::new();
            };
            let len = c.len_utf8();
            vec![Candidate::new(
                c.to_string(),
                c.to_string(), // dummy reading = surface
                pos..pos + len,
                self.score,
            )]
        }
    }

    #[test]
    fn solve_path_empty_input_returns_empty() {
        let dict = DictProvider { entries: vec![] };
        let path = solve_path(&ctx(""), &[&dict]);
        assert!(path.is_empty());
    }

    #[test]
    fn solve_path_no_providers_returns_empty() {
        let path = solve_path(&ctx("猫が好き"), &[]);
        assert!(path.is_empty(), "no providers → no candidates → no path");
    }

    #[test]
    fn solve_path_single_full_match() {
        let dict = DictProvider {
            entries: vec![("猫".into(), "ネコ".into(), Score::dict_exact(1))],
        };
        let path = solve_path(&ctx("猫"), &[&dict]);
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].surface, "猫");
        assert_eq!(path[0].reading, "ネコ");
    }

    #[test]
    fn solve_path_chained_matches() {
        let dict = DictProvider {
            entries: vec![
                ("魔理沙".into(), "マリサ".into(), Score::dict_exact(3)),
                ("が".into(), "ガ".into(), Score::dict_exact(1)),
                ("好き".into(), "スキ".into(), Score::dict_exact(2)),
            ],
        };
        let path = solve_path(&ctx("魔理沙が好き"), &[&dict]);
        assert_eq!(path.len(), 3);
        assert_eq!(path[0].surface, "魔理沙");
        assert_eq!(path[1].surface, "が");
        assert_eq!(path[2].surface, "好き");
    }

    #[test]
    fn solve_path_prefers_higher_band() {
        // input 「上手」: dict に 「上手」 (band 1000) と 「上」 (band 100) + 「手」 (band 100) がある
        // 1000 単一が勝つ
        let dict = DictProvider {
            entries: vec![
                ("上手".into(), "ジョウズ".into(), Score::dict_exact(2)),
                ("上".into(), "ウエ".into(), Score::kanji(1)),
                ("手".into(), "テ".into(), Score::kanji(1)),
            ],
        };
        let path = solve_path(&ctx("上手"), &[&dict]);
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].surface, "上手");
        assert_eq!(path[0].reading, "ジョウズ");
    }

    #[test]
    fn solve_path_prefers_longer_match_within_same_band() {
        // input 「魔理沙」 (UTF-8 9 bytes)
        // dict: 「魔理沙」 全長 + 「魔」 単漢字 + 「理沙」 残り、 全部 band 1000
        // band 同じなら length で勝負 (length 3 > length 1+2)
        let dict = DictProvider {
            entries: vec![
                ("魔理沙".into(), "マリサ".into(), Score::dict_exact(3)),
                ("魔".into(), "マ".into(), Score::dict_exact(1)),
                ("理沙".into(), "リサ".into(), Score::dict_exact(2)),
            ],
        };
        let path = solve_path(&ctx("魔理沙"), &[&dict]);
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].surface, "魔理沙");
    }

    #[test]
    fn solve_path_falls_back_to_char_provider_when_dict_empty() {
        let dict = DictProvider { entries: vec![] };
        let chars = CharProvider {
            score: Score::kanji(1),
        };
        let path = solve_path(&ctx("猫が好"), &[&dict, &chars]);
        // 各 1 文字 fallback で 3 candidate
        assert_eq!(path.len(), 3);
        assert_eq!(path[0].surface, "猫");
        assert_eq!(path[1].surface, "が");
        assert_eq!(path[2].surface, "好");
    }

    #[test]
    fn solve_path_dict_overrides_char_fallback() {
        // dict に 「魔理沙」 entry あり、 char fallback も用意
        // 最良 path は 「魔理沙」 単独 (band 1000) > 「魔」+「理」+「沙」 (band 100 × 3)
        let dict = DictProvider {
            entries: vec![("魔理沙".into(), "マリサ".into(), Score::dict_exact(3))],
        };
        let chars = CharProvider {
            score: Score::kanji(1),
        };
        let path = solve_path(&ctx("魔理沙"), &[&dict, &chars]);
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].surface, "魔理沙");
        assert_eq!(path[0].score.band, BAND_DICT_EXACT);
    }

    #[test]
    fn solve_path_returns_empty_when_uncovered() {
        // dict には 「猫」 だけ、 「が」 を覆える provider なし → unreachable
        let dict = DictProvider {
            entries: vec![("猫".into(), "ネコ".into(), Score::dict_exact(1))],
        };
        let path = solve_path(&ctx("猫が"), &[&dict]);
        assert!(path.is_empty(), "「が」 が unreachable で path 構築不能");
    }

    #[test]
    fn solve_path_partial_dict_with_char_fallback() {
        // dict に 「魔理沙」 のみ、 char fallback で他を埋める
        let dict = DictProvider {
            entries: vec![("魔理沙".into(), "マリサ".into(), Score::dict_exact(3))],
        };
        let chars = CharProvider {
            score: Score::kanji(1),
        };
        let path = solve_path(&ctx("魔理沙が好き"), &[&dict, &chars]);
        // 「魔理沙」 (1) + 「が」 (1) + 「好」 (1) + 「き」 (1) = 4 edges
        assert_eq!(path.len(), 4);
        assert_eq!(path[0].surface, "魔理沙");
        assert_eq!(path[0].score.band, BAND_DICT_EXACT);
        assert_eq!(path[1].surface, "が");
        assert_eq!(path[1].score.band, BAND_KANJI);
    }

    #[test]
    fn solve_path_match_hits_tie_break() {
        // 同 band / 同 length で match_hits 多い方が勝つ
        let dict_with_hits = DictProvider {
            entries: vec![(
                "上手".into(),
                "カミテ".into(),
                Score::new(BAND_DICT_EXACT, 2, 1), // match_hits = 1
            )],
        };
        let dict_default = DictProvider {
            entries: vec![(
                "上手".into(),
                "ジョウズ".into(),
                Score::new(BAND_DICT_EXACT, 2, 0), // match_hits = 0
            )],
        };
        // どちらも band 1000 / length 2、 match_hits で 「カミテ」 が勝つ
        // (provider 順序を with_hits 優先で渡す)
        let path = solve_path(&ctx("上手"), &[&dict_with_hits, &dict_default]);
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].reading, "カミテ");
    }

    #[test]
    fn solve_path_skips_invalid_zero_length_candidate() {
        // 0-length (= range.start == range.end) は skip される
        struct BadProvider;
        impl CandidateProvider for BadProvider {
            fn candidates_at(&self, _ctx: &ScoringContext, pos: usize) -> Vec<Candidate> {
                vec![Candidate::new("", "", pos..pos, Score::dict_exact(0))]
            }
        }
        let chars = CharProvider {
            score: Score::kanji(1),
        };
        let path = solve_path(&ctx("猫"), &[&BadProvider, &chars]);
        // 0-length 候補 skip されて char fallback で 1 文字 candidate
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].surface, "猫");
    }
}

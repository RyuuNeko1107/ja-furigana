//! `analyze()` debug API — caller が candidate / score / path / boundary_regions を inspect 可能。
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §7.2
//!
//! ## 0.1.0 freeze 対象 (★11)
//!
//! `AnalyzeResult` / `Token` の public field は SemVer freeze、 0.1.0 stable 以降は
//! 後で additive 追加のみ可、 既存 field 削除は breaking。
//!
//! ## 用途
//!
//! - debug / 探索: 「なぜこの読みが選ばれたか」 trace
//! - dict 改善判断: caller が path inspect、 PR 起こす材料
//! - **lib は collect しない** (OSS ローカル完結方針)、 caller 任意で persist

use crate::scoring::bracket::{parse_bracket_notation, strip_intonation_markers, AccentPhrase};
use crate::scoring::candidate::{Candidate, CandidateProvider, ScoringContext};
use crate::scoring::engine::solve_path;
use serde::Serialize;
use std::ops::Range;

/// 出力用の代替読み候補 (ADR-0004)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct AlternativeReading {
    pub reading: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sense: Option<String>,
    pub weight: u8,
}

/// 採択 path 上の 1 つの token (= 採用された candidate edge を caller-friendly に)。
///
/// `#[non_exhaustive]`: SemVer 互換維持のため caller の literal struct 構築は禁止。
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct Token {
    /// surface 文字列
    pub surface: String,
    /// reading 文字列 (intonation bracket は strip 済)
    pub reading: String,
    /// input text 上の byte range
    pub range: Range<usize>,
    /// accent phrase 列 (0.2.0)。bracket notation がない reading では空。
    pub accent_phrases: Vec<AccentPhrase>,
    /// 同一位置に代替候補が存在するか (ADR-0004)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ambiguous: bool,
    /// 代替読み候補 (採択されなかった候補、weight 降順)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<AlternativeReading>,
    /// 人名 token flag (crate 内部、 accent 推定用。 ADR-0007)。
    /// 由来: Lindera 固有名詞/人名 candidate、 または NameBoundaryPass の人名補正。
    #[serde(skip)]
    pub(crate) is_name: bool,
}

impl Token {
    /// [`Candidate`] から [`Token`] へ変換 (score を捨てる)。
    #[must_use]
    pub fn from_candidate(c: &Candidate) -> Self {
        let parsed = parse_bracket_notation(&c.reading);
        Self {
            surface: c.surface.clone(),
            reading: parsed.reading,
            range: c.range.clone(),
            accent_phrases: parsed.accent_phrases,
            ambiguous: false,
            alternatives: Vec::new(),
            is_name: c.is_name,
        }
    }
}

/// `analyze()` の戻り値型 (★11、 0.1.0 freeze)。
///
/// ## field 説明
///
/// - `tokens`: 採択 path 上の token 列 (= 順序保証、 input 全体を覆う)
/// - `candidates`: 各 token 位置で 「考慮された候補一覧」 (`tokens[i].range.start` から始まる candidate 全列挙)
/// - `path_indices`: 各 token の start byte 位置 (= `tokens[i].range.start` の copy、 caller の pos lookup 用)
/// - `boundary_regions`: 検出された 漢字連続 region の byte range (= `BoundaryAnalysis::regions[i].range`)
///
/// `#[non_exhaustive]`: 0.2.0+ で field 追加余地 (例: timing metadata、 alternative paths)、
/// SemVer 互換維持のため caller の literal struct 構築は禁止 (= lib が return するのみ)。
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct AnalyzeResult {
    /// 採択 path 上の token 列
    pub tokens: Vec<Token>,
    /// 各 token 位置で考慮された候補一覧 (`candidates[i]` は `tokens[i]` の位置の全候補)
    pub candidates: Vec<Vec<Candidate>>,
    /// 各 token の start byte 位置 (= path lookup 用)
    pub path_indices: Vec<usize>,
    /// 検出された 漢字連続 region (byte range、 `boundary` を渡した場合のみ)
    pub boundary_regions: Vec<Range<usize>>,
}

/// 採択 path の [`Token`] 列だけを返す軽量版 (`candidates` / `alternatives` を計算しない)。
///
/// `to_ruby` / `to_hiragana` / `to_tts` / `to_romaji` / `tokenize` の production 経路は
/// reading 文字列しか使わず、 debug 用の `candidates` 集約 (= 全 provider を path 位置で
/// **再走** ) と `alternatives` 抽出 (sort / dedup / strip) を丸ごと捨てている。 本関数は
/// その死荷重を払わず [`solve_path`] → Token 変換のみ行う。 inspect が要る caller は
/// 引き続き [`analyze`] を使う。
#[must_use]
pub fn analyze_tokens(ctx: &ScoringContext, providers: &[&dyn CandidateProvider]) -> Vec<Token> {
    solve_path(ctx, providers)
        .iter()
        .map(Token::from_candidate)
        .collect()
}

/// `input` を `providers` で analyze、 [`AnalyzeResult`] を返す。
///
/// `boundary` を渡すと `boundary_regions` field に regions を入れる (`None` なら空)。
/// path 選択は [`solve_path`] 経由。
///
/// ## 戻り値の保証
///
/// - 入力空 → 全 field 空
/// - path 構築不能 (= input 覆い切れない) → tokens / path_indices 空、 candidates / boundary_regions は計算結果残る
/// - path 構築成功 → tokens / path_indices / candidates が同 length、 path_indices[i] = tokens[i].range.start
pub fn analyze(ctx: &ScoringContext, providers: &[&dyn CandidateProvider]) -> AnalyzeResult {
    // 1. solve_path で採択 path 取得
    let path = solve_path(ctx, providers);

    // 2. path → Token 列
    let tokens: Vec<Token> = path.iter().map(Token::from_candidate).collect();

    // 3. path_indices: 各 token の byte start 位置
    let path_indices: Vec<usize> = tokens.iter().map(|t| t.range.start).collect();

    // 4. candidates: 各 token 位置で 全 provider が返す候補を集約 (debug 用)
    let candidates: Vec<Vec<Candidate>> = path_indices
        .iter()
        .map(|&pos| {
            let mut all = Vec::new();
            for provider in providers {
                all.extend(provider.candidates_at(ctx, pos));
            }
            all
        })
        .collect();

    // 5. boundary_regions: BoundaryAnalysis から range を抽出
    let boundary_regions: Vec<Range<usize>> = ctx
        .boundary
        .regions
        .iter()
        .map(|r| r.range.clone())
        .collect();

    // 6. alternatives: 同一位置・同一 surface で reading が異なる候補を抽出 (ADR-0004)
    let mut tokens = tokens;
    for (i, token_candidates) in candidates.iter().enumerate() {
        if i >= tokens.len() {
            break;
        }
        let token = &mut tokens[i];
        let winning_reading = &token.reading;

        let mut alts: Vec<AlternativeReading> = token_candidates
            .iter()
            .filter(|c| {
                c.surface == token.surface
                    && c.range == token.range
                    && strip_intonation_markers(&c.reading) != *winning_reading
            })
            .map(|c| AlternativeReading {
                reading: strip_intonation_markers(&c.reading),
                sense: None,
                weight: c.score.weight,
            })
            .collect();

        // dedup by reading (同一 reading の重複を除去、weight 最大を残す)
        alts.sort_by_key(|a| std::cmp::Reverse(a.weight));
        alts.dedup_by(|a, b| a.reading == b.reading);

        if !alts.is_empty() {
            token.ambiguous = true;
            token.alternatives = alts;
        }
    }

    AnalyzeResult {
        tokens,
        candidates,
        path_indices,
        boundary_regions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::boundary::BoundaryAnalysis;
    use crate::scoring::candidate::{Score, ScoringContext};

    fn ctx(input: &str) -> ScoringContext<'_> {
        let boundary = Box::leak(Box::new(BoundaryAnalysis::empty()));
        ScoringContext { input, boundary }
    }

    fn ctx_with_boundary<'a>(input: &'a str, boundary: &'a BoundaryAnalysis) -> ScoringContext<'a> {
        ScoringContext { input, boundary }
    }

    /// dummy provider: 指定 surface→reading mapping を 全位置で試行
    struct DictProvider {
        entries: Vec<(String, String, Score)>,
    }

    impl CandidateProvider for DictProvider {
        fn candidates_at(&self, ctx: &ScoringContext, pos: usize) -> Vec<Candidate> {
            let mut out = Vec::new();
            for (surface, reading, score) in &self.entries {
                if ctx.input[pos..].starts_with(surface.as_str()) {
                    out.push(Candidate::new(
                        surface.clone(),
                        reading.clone(),
                        pos..pos + surface.len(),
                        *score,
                    ));
                }
            }
            out
        }
    }

    #[test]
    fn analyze_empty_input() {
        let dict = DictProvider { entries: vec![] };
        let result = analyze(&ctx(""), &[&dict]);
        assert!(result.tokens.is_empty());
        assert!(result.candidates.is_empty());
        assert!(result.path_indices.is_empty());
        assert!(result.boundary_regions.is_empty());
    }

    #[test]
    fn analyze_basic_path() {
        let dict = DictProvider {
            entries: vec![("猫".into(), "ネコ".into(), Score::dict_exact(1))],
        };
        let result = analyze(&ctx("猫"), &[&dict]);
        assert_eq!(result.tokens.len(), 1);
        assert_eq!(result.tokens[0].surface, "猫");
        assert_eq!(result.tokens[0].reading, "ネコ");
        assert_eq!(result.tokens[0].range, 0..3);
        assert_eq!(result.path_indices, vec![0]);
        // candidates[0] には 「猫」 candidate 1 つ
        assert_eq!(result.candidates[0].len(), 1);
        assert_eq!(result.candidates[0][0].reading, "ネコ");
    }

    #[test]
    fn analyze_multi_token_path() {
        let dict = DictProvider {
            entries: vec![
                ("魔理沙".into(), "マリサ".into(), Score::dict_exact(3)),
                ("が".into(), "ガ".into(), Score::dict_exact(1)),
                ("好き".into(), "スキ".into(), Score::dict_exact(2)),
            ],
        };
        let result = analyze(&ctx("魔理沙が好き"), &[&dict]);
        assert_eq!(result.tokens.len(), 3);
        assert_eq!(result.tokens[0].surface, "魔理沙");
        assert_eq!(result.tokens[1].surface, "が");
        assert_eq!(result.tokens[2].surface, "好き");
        assert_eq!(result.path_indices, vec![0, 9, 12]);
    }

    #[test]
    fn analyze_with_boundary_yields_regions() {
        let dict = DictProvider {
            entries: vec![("魔理沙".into(), "マリサ".into(), Score::dict_exact(3))],
        };
        let boundary = BoundaryAnalysis::analyze("魔理沙");
        let result = analyze(&ctx_with_boundary("魔理沙", &boundary), &[&dict]);
        assert_eq!(result.boundary_regions.len(), 1);
        assert_eq!(result.boundary_regions[0], 0..9);
    }

    #[test]
    fn analyze_without_boundary_yields_empty_regions() {
        let dict = DictProvider {
            entries: vec![("猫".into(), "ネコ".into(), Score::dict_exact(1))],
        };
        let result = analyze(&ctx("猫"), &[&dict]);
        assert!(result.boundary_regions.is_empty());
    }

    #[test]
    fn analyze_unreachable_input_yields_empty_tokens() {
        let dict = DictProvider {
            entries: vec![("猫".into(), "ネコ".into(), Score::dict_exact(1))],
        };
        // 「が」 を覆える provider なし、 path 構築不能
        let result = analyze(&ctx("猫が"), &[&dict]);
        assert!(result.tokens.is_empty());
        assert!(result.path_indices.is_empty());
    }

    #[test]
    fn token_from_candidate_drops_score() {
        let cand = Candidate::new("猫", "ネコ", 0..3, Score::dict_exact(1));
        let token = Token::from_candidate(&cand);
        assert_eq!(token.surface, "猫");
        assert_eq!(token.reading, "ネコ");
        assert_eq!(token.range, 0..3);
        // Token に score field がない (= 0.1.0 freeze の minimal scope)
    }

    #[test]
    fn analyze_candidates_show_all_providers() {
        // 同位置で複数 provider が candidate 出すと、 candidates[i] に全部入る
        let dict_a = DictProvider {
            entries: vec![("猫".into(), "ネコ".into(), Score::dict_exact(1))],
        };
        let dict_b = DictProvider {
            entries: vec![("猫".into(), "ニャア".into(), Score::dict_exact(1))],
        };
        let result = analyze(&ctx("猫"), &[&dict_a, &dict_b]);
        // tokens は 1 つ (path 採択)、 candidates[0] には dict_a + dict_b 両方入る
        assert_eq!(result.tokens.len(), 1);
        assert_eq!(result.candidates[0].len(), 2);
    }
}

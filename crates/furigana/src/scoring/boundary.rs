//! 漢字連続 region の検出。
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §5.2
//!
//! ## 役割
//!
//! 入力中の 漢字連続 region (= contiguous 漢字 char sequence) を検出する。
//! [`crate::scoring::analyze::AnalyzeResult::boundary_regions`] の debug 出力に使う。
//!
//! ## 過分割の抑制について
//!
//! 長文未知語が短い完全一致 entry に切り刻まれる問題は、 旧 (b)(c) boundary penalty
//! ではなく [`crate::scoring::engine::PathScore`] の `edge_count` 軸 ((a) longest match)
//! で抑制する。 penalty 軸は本番で一度も配線されず常に 0 だったため撤去した。
//!
//! ## scope 外
//!
//! 漢字以外の文字種 (カタカナ / ひらがな / 英数 / 記号) 連続は本 module の対象外
//! (= dict 登録すれば band 1000 で動く、 自動 chunk preservation はしない方針)。

use crate::kana;
use std::ops::Range;

/// 入力 1 つの漢字連続 region (= contiguous 漢字 char sequence)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KanjiRegion {
    /// input text 上の byte range
    pub range: Range<usize>,
    /// region 内の文字数 (= 漢字 char 数)
    pub char_count: usize,
}

/// 入力全体の boundary 分析結果。
///
/// [`Self::analyze`] で input を walk して全 漢字連続 region を検出する。
#[derive(Debug, Clone, Default)]
pub struct BoundaryAnalysis {
    /// 検出された 漢字連続 region (順序保証、 byte range 昇順)
    pub regions: Vec<KanjiRegion>,
}

impl BoundaryAnalysis {
    /// 空の分析結果 (= regions なし)。
    #[cfg(test)]
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// `input` を walk して 漢字連続 region を検出する。
    #[must_use]
    pub fn analyze(input: &str) -> Self {
        let regions = find_kanji_regions(input)
            .into_iter()
            .map(|r| {
                let char_count = input[r.clone()].chars().count();
                KanjiRegion {
                    range: r,
                    char_count,
                }
            })
            .collect();
        Self { regions }
    }

    /// `pos` を含む region の reference (なければ None)。 debug / 解析用途。
    #[cfg(test)]
    #[must_use]
    pub fn region_containing(&self, pos: usize) -> Option<&KanjiRegion> {
        self.regions
            .iter()
            .find(|r| pos >= r.range.start && pos < r.range.end)
    }
}

/// 入力中の 「漢字 1 文字以上連続する byte range」 を全列挙して返す。
///
/// 漢字判定は [`crate::kana::is_kanji_char`] (= CJK 統合漢字 + 拡張 A + 互換 + 々/〆/ヶ)。
/// 漢字 1 文字だけの region も含む (proposal §5.2 では漢字連続を抽出、 1 文字も region になる)。
#[must_use]
pub fn find_kanji_regions(input: &str) -> Vec<Range<usize>> {
    let mut regions = Vec::new();
    let mut current_start: Option<usize> = None;
    let mut current_end: usize = 0;

    for (idx, c) in input.char_indices() {
        let char_end = idx + c.len_utf8();
        if kana::is_kanji_char(c) {
            if current_start.is_none() {
                current_start = Some(idx);
            }
            current_end = char_end;
        } else if let Some(start) = current_start.take() {
            regions.push(start..current_end);
        }
    }
    if let Some(start) = current_start {
        regions.push(start..current_end);
    }
    regions
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── find_kanji_regions ──────────────────────────────────────────────────

    #[test]
    fn find_regions_empty_input() {
        assert!(find_kanji_regions("").is_empty());
    }

    #[test]
    fn find_regions_no_kanji() {
        assert!(find_kanji_regions("ひらがな").is_empty());
        assert!(find_kanji_regions("カタカナ").is_empty());
        assert!(find_kanji_regions("ABC123").is_empty());
        assert!(find_kanji_regions("").is_empty());
    }

    #[test]
    fn find_regions_single_kanji() {
        let regions = find_kanji_regions("猫");
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0], 0..3); // "猫" UTF-8 = 3 bytes
    }

    #[test]
    fn find_regions_consecutive_kanji() {
        let regions = find_kanji_regions("魔理沙");
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0], 0..9); // 3 kanji × 3 bytes
    }

    #[test]
    fn find_regions_kanji_then_hiragana() {
        // "魔理沙が好き": 魔理沙 (kanji 3) + が (hira 1) + 好 (kanji 1) + き (hira 1)
        let regions = find_kanji_regions("魔理沙が好き");
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0], 0..9); // 魔理沙
        assert_eq!(regions[1], 12..15); // 好 (= 9 + 3 hira "が" → start 12)
    }

    #[test]
    fn find_regions_includes_odoriji() {
        // 「々」 は kanji_char に含まれる
        let regions = find_kanji_regions("人々");
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0], 0..6);
    }

    // ─── BoundaryAnalysis::analyze ───────────────────────────────────────────

    #[test]
    fn analyze_empty_input_yields_no_regions() {
        let analysis = BoundaryAnalysis::analyze("");
        assert!(analysis.regions.is_empty());
    }

    #[test]
    fn analyze_single_region_records_char_count() {
        let analysis = BoundaryAnalysis::analyze("漢字");
        assert_eq!(analysis.regions.len(), 1);
        assert_eq!(analysis.regions[0].char_count, 2);
        assert_eq!(analysis.regions[0].range, 0..6);
    }

    #[test]
    fn analyze_two_separate_regions() {
        // "魔理沙が好き" → region 0 = 魔理沙 (3 chars)、 region 1 = 好 (1 char)
        let analysis = BoundaryAnalysis::analyze("魔理沙が好き");
        assert_eq!(analysis.regions.len(), 2);
        assert_eq!(analysis.regions[0].char_count, 3);
        assert_eq!(analysis.regions[1].char_count, 1);
    }

    // ─── region_containing ───────────────────────────────────────────────────

    #[test]
    fn region_containing_returns_correct_region() {
        let analysis = BoundaryAnalysis::analyze("紅魔館");
        let r = analysis.region_containing(3).unwrap();
        assert_eq!(r.range, 0..9);
    }

    #[test]
    fn region_containing_returns_none_outside() {
        let analysis = BoundaryAnalysis::analyze("紅魔館");
        assert!(analysis.region_containing(9).is_none());
    }

    // ─── 漢字以外は対象外 ────────────────────────────────────────────────────

    #[test]
    fn analyze_ignores_non_kanji_runs() {
        // カタカナ連続は scope 外、 region に入らない
        let analysis = BoundaryAnalysis::analyze("ボイスボックス");
        assert!(analysis.regions.is_empty());
    }

    #[test]
    fn analyze_ignores_hiragana_runs() {
        let analysis = BoundaryAnalysis::analyze("こんにちは");
        assert!(analysis.regions.is_empty());
    }

    #[test]
    fn analyze_ignores_alphanumeric_runs() {
        let analysis = BoundaryAnalysis::analyze("API123");
        assert!(analysis.regions.is_empty());
    }
}

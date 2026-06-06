//! Candidate / Score 型と band 定数 (Smart engine 共通基盤)。
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §4 score 設計
//!
//! ## 概要
//!
//! - [`Score`]: candidate edge の score tuple、 lexicographic 比較 (band → length → match_hits → weight)
//! - [`Candidate`]: input text 上の 1 つの候補 edge (surface + reading + range + score)
//! - [`CandidateProvider`]: candidate を供給する trait (entry / kanji / Lindera / 特殊処理 各 layer 実装)
//! - band 定数: 1000 = 単語辞書完全一致、 950 = 特殊処理、 100 = 漢字辞書、 50 = Lindera unihan injection

use crate::scoring::boundary::BoundaryAnalysis;
use serde::Serialize;
use std::cmp::Ordering;
use std::ops::Range;

// ─── ScoringContext ─────────────────────────────────────────────────────────

/// provider に渡す解析セッション全体のコンテキスト。
///
/// 各 byte 位置で不変な情報 (input text, boundary analysis) をまとめる。
/// 0.2.0 では bracket 解析結果も追加予定。
pub struct ScoringContext<'a> {
    pub input: &'a str,
    pub boundary: &'a BoundaryAnalysis,
}

// ─── band 値定数 ─────────────────────────────────────────────────────────────

/// 単語辞書 (`[entries]`) 完全一致の band 値。
pub const BAND_DICT_EXACT: u16 = 1000;

/// 特殊処理 (数字+助数詞 / 漢数字 / 数字読み / 踊り字 等の動的合成) の band 値。
///
/// dict 完全一致 (1000) には常に負け、 漢字辞書 (100) / Lindera (50) には常勝。
/// dict に意図的 entry を書けば 1000 で override 可能。
pub const BAND_SPECIAL: u16 = 950;

/// Lindera が **2 字以上 + 全 char 漢字** の surface を 1 token として返したときの band 値
/// (★alpha.20、 形態素信頼版)。
///
/// 単漢字 default (band 100) を上回るが、 dict / 特殊処理 / `[[kanji]]` block (band 1000)
/// には常に負ける。 「dict 未登録の純漢字熟語 (例: 最近 / 風邪 / 宿題 / 海外)」 を
/// 単漢字 default の合成 (= もっとも + ちかい 等) で破壊せず、 形態素エンジン
/// (= IPADIC / UniDic) の reading を信頼するための trick。
///
/// 適用範囲を 「2 字以上 + 全漢字」 に絞る理由:
/// - 単漢字 surface (= 「私」 「彼」 等) は overrides.toml の `[[kanji]]` context match で
///   制御したい (= Lindera が default 訓読みを返すと困る case 多数)
/// - 漢字 + okurigana の混在 (= 「来た」 等) は `[[kanji]]` block の `next_char_type` match
///   で declarative に解決する設計 (= alpha.18 band-up trick を撤回した経緯)
pub const BAND_LINDERA_COMPOUND: u16 = 150;

/// 漢字辞書 (`[[kanji]]`) の band 値。 dict 完全一致 / 特殊処理 がない時の前段 fallback。
pub const BAND_KANJI: u16 = 100;

/// Lindera unihan injection の band 値。 dict / 漢字辞書 にない時の最終 fallback。
pub const BAND_LINDERA_UNIHAN: u16 = 50;

/// 保護トークン (URL / Email / 絵文字 等) の band 値。 全 candidate を上回って必ず勝つ。
///
/// これらは reading 振り対象外、 surface = output で透過する。 path 選択時に必ず採用される
/// よう、 band 1000 (単語辞書完全一致) より高い値を設定。
pub const BAND_PROTECTED: u16 = 2000;

// ─── Score ───────────────────────────────────────────────────────────────────

/// candidate edge の score tuple。
///
/// **連続値ではない、 discrete tuple の lexicographic 比較** で順位決定:
///
/// 1. `band` 大 (band 値で勝負、 1000 vs 950 等)
/// 2. `length` 大 (longest match、 surface 長で勝負)
/// 3. `match_hits` 多 (inline match condition 評価で hit した数)
/// 4. `weight` 大 (代替候補の相対頻度、 ADR-0004)
///
/// 同点 (= 全 4 軸同値) の場合の tie-break は caller 側 (例: TOML 出現順) で。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Score {
    /// band 値 (1000 / 950 / 100 / 50 等)
    pub band: u16,
    /// surface 文字数 (longest match 効果の表現)
    pub length: u8,
    /// inline match condition hit 数 (default ≠ match block)
    pub match_hits: u8,
    /// 候補の相対頻度 (0–100、 default = 100、 alt 候補は dict 指定値)。
    ///
    /// **用途は `AnalyzeResult` の `alternatives` 配列の順位付け** (ADR-0004、
    /// [`crate::scoring::analyze`] が weight 降順で並べる)。 path 選択そのものは
    /// [`crate::scoring::engine::PathScore`] (band → edge_count → match_hits) で行い
    /// weight を集約しない。 default は weight 100 (最大) かつ provider が最初に emit
    /// するので path には常に default が乗る = ADR-0004 の 「default = 最高 weight」 が
    /// 自然に満たされる。 (Score::Ord の weight 軸は test 用、 DP は通らない)
    pub weight: u8,
}

/// default reading の weight (暗黙最大値)。
pub const WEIGHT_DEFAULT: u8 = 100;

impl Score {
    /// 明示値で構築。
    #[must_use]
    pub const fn new(band: u16, length: u8, match_hits: u8) -> Self {
        Self {
            band,
            length,
            match_hits,
            weight: WEIGHT_DEFAULT,
        }
    }

    /// weight 指定付き構築 (alt 候補用)。
    #[must_use]
    pub const fn with_weight(band: u16, length: u8, match_hits: u8, weight: u8) -> Self {
        Self {
            band,
            length,
            match_hits,
            weight,
        }
    }

    /// 単語辞書完全一致の score (band 1000、 length 指定)。
    #[must_use]
    pub const fn dict_exact(length: u8) -> Self {
        Self::new(BAND_DICT_EXACT, length, 0)
    }

    /// 特殊処理 score (band 950、 length 指定)。
    #[must_use]
    pub const fn special(length: u8) -> Self {
        Self::new(BAND_SPECIAL, length, 0)
    }

    /// 漢字辞書 score (band 100、 length 指定)。 length は通常 1。
    #[must_use]
    pub const fn kanji(length: u8) -> Self {
        Self::new(BAND_KANJI, length, 0)
    }

    /// Lindera unihan injection score (band 50、 length 指定)。
    #[must_use]
    pub const fn lindera(length: u8) -> Self {
        Self::new(BAND_LINDERA_UNIHAN, length, 0)
    }

    /// Lindera 2 字以上純漢字 surface 用 score (band 150、 length 指定、 ★alpha.20)。
    #[must_use]
    pub const fn lindera_compound(length: u8) -> Self {
        Self::new(BAND_LINDERA_COMPOUND, length, 0)
    }
}

impl Ord for Score {
    fn cmp(&self, other: &Self) -> Ordering {
        self.band
            .cmp(&other.band)
            .then(self.length.cmp(&other.length))
            .then(self.match_hits.cmp(&other.match_hits))
            .then(self.weight.cmp(&other.weight))
    }
}

impl PartialOrd for Score {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// ─── Candidate ───────────────────────────────────────────────────────────────

/// Viterbi DP 上の 1 つの candidate edge。
///
/// `range` は input text 上の byte range。 `range.end - range.start` が surface byte 長。
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    /// surface 文字列 (= input[range])
    pub surface: String,
    /// reading 文字列 (カタカナ等)
    pub reading: String,
    /// input text 上の byte range
    pub range: Range<usize>,
    /// score tuple
    pub score: Score,
}

impl Candidate {
    /// surface の byte 長 (= `range.end - range.start`)。
    #[must_use]
    pub fn surface_byte_len(&self) -> usize {
        self.range.end.saturating_sub(self.range.start)
    }

    /// 構築 helper (test / 実装中のみ便利)。
    #[must_use]
    pub fn new(
        surface: impl Into<String>,
        reading: impl Into<String>,
        range: Range<usize>,
        score: Score,
    ) -> Self {
        Self {
            surface: surface.into(),
            reading: reading.into(),
            range,
            score,
        }
    }
}

// ─── CandidateProvider trait ─────────────────────────────────────────────────

/// 各 layer (entry / kanji / Lindera / 特殊処理) が実装する candidate 供給 trait。
///
/// `candidates_at` は **byte 位置 `pos` から始まる** 候補を返す。 caller (Smart engine)
/// は input 上の各位置で全 provider を呼び出して候補を集約、 Viterbi-like DP で path を解く。
pub trait CandidateProvider {
    /// `ctx.input` の byte 位置 `pos` から始まる candidate を全列挙して返す。
    ///
    /// 候補が無い (= この位置から始まる surface が dict / kanji / etc に存在しない) 場合は空 Vec。
    /// 候補が複数あっても OK (= 同位置から異なる surface 長の candidate を返してよい)。
    fn candidates_at(&self, ctx: &ScoringContext, pos: usize) -> Vec<Candidate>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Score lexicographic 比較 ────────────────────────────────────────────

    #[test]
    fn score_higher_band_wins() {
        let dict = Score::dict_exact(2);
        let kanji = Score::kanji(5);
        assert!(
            dict > kanji,
            "band 1000 > band 100 even with shorter length"
        );
    }

    #[test]
    fn score_band_special_beats_kanji() {
        let special = Score::special(2);
        let kanji = Score::kanji(5);
        assert!(special > kanji, "band 950 > band 100");
    }

    #[test]
    fn score_band_special_loses_to_dict() {
        let special = Score::special(5);
        let dict = Score::dict_exact(2);
        assert!(
            dict > special,
            "band 1000 > band 950 (dict exact wins over special)"
        );
    }

    #[test]
    fn score_same_band_longer_wins() {
        let long = Score::dict_exact(4);
        let short = Score::dict_exact(2);
        assert!(long > short, "longest match within same band");
    }

    #[test]
    fn score_same_band_length_more_match_hits_wins() {
        let with_hits = Score::new(BAND_DICT_EXACT, 2, 1);
        let without = Score::new(BAND_DICT_EXACT, 2, 0);
        assert!(with_hits > without, "match_hits tie-break");
    }

    #[test]
    fn score_same_band_higher_weight_wins() {
        let high = Score::with_weight(BAND_DICT_EXACT, 2, 0, 100);
        let low = Score::with_weight(BAND_DICT_EXACT, 2, 0, 30);
        assert!(high > low, "weight tie-break (ADR-0004)");
    }

    #[test]
    fn score_equal() {
        let a = Score::dict_exact(3);
        let b = Score::dict_exact(3);
        assert_eq!(a.cmp(&b), Ordering::Equal);
    }

    // ─── band 値 sanity check ────────────────────────────────────────────────

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn band_values_are_correctly_ordered() {
        assert!(BAND_DICT_EXACT > BAND_SPECIAL);
        assert!(BAND_SPECIAL > BAND_LINDERA_COMPOUND);
        assert!(BAND_LINDERA_COMPOUND > BAND_KANJI);
        assert!(BAND_KANJI > BAND_LINDERA_UNIHAN);
        assert_eq!(BAND_DICT_EXACT, 1000);
        assert_eq!(BAND_SPECIAL, 950);
        assert_eq!(BAND_LINDERA_COMPOUND, 150);
        assert_eq!(BAND_KANJI, 100);
        assert_eq!(BAND_LINDERA_UNIHAN, 50);
    }

    // ─── Candidate ───────────────────────────────────────────────────────────

    #[test]
    fn candidate_surface_byte_len() {
        let c = Candidate::new("猫", "ネコ", 0..3, Score::dict_exact(1));
        assert_eq!(c.surface_byte_len(), 3); // "猫" in UTF-8 = 3 bytes
    }
}

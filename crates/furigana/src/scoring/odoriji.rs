//! 踊り字 「々」 の Smart engine 統合 (C4 minimal scope)。
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §5.6
//!
//! ## 役割
//!
//! Smart engine path 上で 「々」 1 文字 surface を表現するための薄い 2 層:
//!
//! - [`OdorijiProvider`]: input 中の 「々」 1 文字位置で band [`BAND_KANJI`] の
//!   placeholder candidate (reading = "々") を出す [`CandidateProvider`]。
//!   path 構築時に他 provider 候補が無い時の fallback edge として乗る。
//! - [`apply_rendaku_inplace`]: path 確定後、 `surface == "々"` の token の reading を
//!   直前 token reading + [`crate::kana::voice_first_kana`] で書き換える post-pass。
//!
//! ## 連濁判定
//!
//! 旧 Strict engine の `expand_odoriji_inplace` (alpha.15 で削除済) から引き継いだ
//! rule。 簡易連濁:
//!
//! - 直前 reading の **第 1 音がカ/サ/タ/ハ 行** → 濁音化して 「々」 reading に採用
//!   (神→ガミ、 人→ビト、 時→ドキ)
//! - **ナ/マ/ヤ/ラ/ワ/ア 行など** → 連濁対象外 → そのまま複製
//!   (我々=ワレワレ、 山々=ヤマヤマ、 年々=ネンネン)
//!
//! 例外語 (個々=ココ など) で誤連濁が出る場合は dict に固有 entry を登録すれば
//! band 1000 で先に勝つ (= override 可能、 同 alpha era policy 継承)。
//!
//! ## 注意
//!
//! - placeholder candidate の reading = "々" のままだと output に 「々」 が残るため、
//!   [`apply_rendaku_inplace`] を必ず post-pass で呼ぶこと。
//! - 直前 token が無い (= 先頭 「々」) / 直前 reading が空 / 連濁対象外 → reading を
//!   そのまま 「々」 / 直前複製 のいずれかで残す (= 入力破壊しない)。

use crate::kana::voice_first_kana;
use crate::scoring::analyze::Token;
use crate::scoring::candidate::{
    CandidateProvider, RawCandidate, Score, ScoringContext, BAND_KANJI,
};

/// 踊り字 (々) char。
const ODORIJI_CHAR: char = '々';

/// input 中の 「々」 1 文字位置に band [`BAND_KANJI`] candidate を出す provider。
///
/// reading 値は placeholder の `"々"`、 caller (= [`crate::api::Furigana::analyze`])
/// は path 解決後に [`apply_rendaku_inplace`] を呼んで連濁適用する想定。
///
/// state を持たない (= input 全体を pre-scan しない)、 各 `candidates_at` で
/// pos の 1 文字を調べるだけの軽量実装。
#[derive(Debug, Default, Clone)]
pub struct OdorijiProvider;

impl OdorijiProvider {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl CandidateProvider for OdorijiProvider {
    fn candidates_at<'b>(
        &'b self,
        ctx: &ScoringContext<'b>,
        pos: usize,
        out: &mut Vec<RawCandidate<'b>>,
    ) {
        let tail = &ctx.input[pos..];
        let Some(c) = tail.chars().next() else {
            return;
        };
        if c != ODORIJI_CHAR {
            return;
        }
        let len = c.len_utf8();
        out.push(RawCandidate::new(
            ODORIJI_CHAR.to_string(), // placeholder、 post-pass で連濁適用
            pos..pos + len,
            Score::new(BAND_KANJI, 1, 0),
        ));
    }
}

/// `tokens` を walk し、 surface = 「々」 の token の reading を直前 token reading +
/// 連濁判定 ([`voice_first_kana`]) で書き換える (in-place)。
///
/// 直前 token が無い / 直前 reading が空 → 「々」 のまま残す (= no-op)、
/// 連濁対象外 (voice_first_kana が None) → 直前 reading をそのまま複製。
///
/// 旧 Strict engine の `expand_odoriji_inplace` (alpha.15 で削除済) と同じ rule。
///
/// ## 例
///
/// - 「神々」 → tokens = [神/カミ, 々/々] → tokens = [神/カミ, 々/ガミ] (連濁あり)
/// - 「我々」 → tokens = [我/ワレ, 々/々] → tokens = [我/ワレ, 々/ワレ] (連濁なし、 複製)
/// - 「々」 単独 → 直前 token なし → no-op
pub fn apply_rendaku_inplace(tokens: &mut [Token]) {
    for i in 1..tokens.len() {
        if tokens[i].surface == ODORIJI_CHAR.to_string().as_str() {
            let prev_reading = tokens[i - 1].reading.clone();
            if prev_reading.is_empty() {
                continue;
            }
            // 音読みの畳語は連濁しない: 早々 = そうそう / 再々 = さいさい / 先々 = せんせん
            // (到着早々 が そうぞう になっていた、 2026-09-27)。 訓読みは従来どおり 人々 = ひとびと / 島々 = しまじま。
            // 音読みと判定するのは 「漢字 1 字の token で、 読みがカタカナかつ ン/ウ/イ/ー で終わる」 時だけ
            // (dict の表記規約では音 = カタカナ・訓 = ひらがな だが、 利用者辞書や形態素解析由来は訓もカタカナなので
            // カミ / ヒト 型は訓として濁らせる。 部屋々々 = へやべや のような 2 字語も従来どおり)。
            // 音読みでも濁る慣用 (散々 / 精々) だけ例外
            let on_yomi = tokens[i - 1].surface.chars().count() == 1
                && prev_reading
                    .chars()
                    .all(|c| ('ァ'..='ヺ').contains(&c) || c == 'ー')
                && prev_reading.ends_with(['ン', 'ウ', 'イ', 'ー']);
            let voiced_on = matches!(tokens[i - 1].surface.as_str(), "散" | "精");
            tokens[i].reading = if on_yomi && !voiced_on {
                prev_reading
            } else {
                voice_first_kana(&prev_reading).unwrap_or(prev_reading)
            };
        }
    }
}

/// 連濁 post-pass の adapter ([`crate::scoring::postpass::ReadingPostPass`])。
///
/// 「々」 placeholder token に直前 token reading の連濁形を入れる。
#[derive(Debug, Clone, Copy)]
pub struct RendakuPass;

impl crate::scoring::postpass::ReadingPostPass for RendakuPass {
    fn apply(&self, tokens: &mut Vec<Token>) {
        apply_rendaku_inplace(tokens);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::boundary::BoundaryAnalysis;
    use std::ops::Range;

    fn ctx(input: &str) -> ScoringContext<'_> {
        let boundary = Box::leak(Box::new(BoundaryAnalysis::empty()));
        ScoringContext::new(input, boundary)
    }

    fn token(surface: &str, reading: &str, range: Range<usize>) -> Token {
        Token {
            surface: surface.to_string(),
            reading: reading.to_string(),
            range,
            accent_phrases: Vec::new(),
            ambiguous: false,
            alternatives: Vec::new(),
            is_name: false,
        }
    }

    // ─── OdorijiProvider ─────────────────────────────────────────────────────

    #[test]
    fn provider_returns_candidate_at_odoriji_position() {
        let p = OdorijiProvider::new();
        // "神々" = 神 (3 bytes) + 々 (3 bytes)
        let input = "神々";
        let cands = p.candidates_vec(&ctx(input), 3);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].surface, "々");
        assert_eq!(cands[0].reading, "々"); // placeholder
        assert_eq!(cands[0].range, 3..6);
        assert_eq!(cands[0].score.band, BAND_KANJI);
    }

    #[test]
    fn provider_returns_empty_at_non_odoriji_position() {
        let p = OdorijiProvider::new();
        let input = "神々";
        // pos 0 は 「神」 (々 ではない)
        assert!(p.candidates_vec(&ctx(input), 0).is_empty());
    }

    #[test]
    fn provider_returns_empty_at_end_of_input() {
        let p = OdorijiProvider::new();
        let input = "神";
        // pos 3 は input.len() = 入力末尾
        assert!(p.candidates_vec(&ctx(input), 3).is_empty());
    }

    #[test]
    fn provider_returns_empty_for_empty_input() {
        let p = OdorijiProvider::new();
        assert!(p.candidates_vec(&ctx(""), 0).is_empty());
    }

    // ─── apply_rendaku_inplace: 連濁あり ─────────────────────────────────────

    #[test]
    fn rendaku_applied_for_voiceable_first_kana() {
        // 神々 → カミ + ガミ
        let mut tokens = vec![token("神", "カミ", 0..3), token("々", "々", 3..6)];
        apply_rendaku_inplace(&mut tokens);
        assert_eq!(tokens[1].reading, "ガミ");
    }

    #[test]
    fn on_yomi_repetition_is_not_voiced() {
        // 早々 = ソウソウ / 再々 = サイサイ / 先々 = センセン (音読みの畳語は連濁しない)
        for (k, r) in [("早", "ソウ"), ("再", "サイ"), ("先", "セン")] {
            let mut tokens = vec![token(k, r, 0..3), token("々", "々", 3..6)];
            apply_rendaku_inplace(&mut tokens);
            assert_eq!(tokens[1].reading, r, "{k}々");
        }
    }

    #[test]
    fn voiced_on_yomi_exceptions() {
        // 散々 = サンザン / 精々 = セイゼイ は音読みでも濁る
        for (k, r, v) in [("散", "サン", "ザン"), ("精", "セイ", "ゼイ")] {
            let mut tokens = vec![token(k, r, 0..3), token("々", "々", 3..6)];
            apply_rendaku_inplace(&mut tokens);
            assert_eq!(tokens[1].reading, v, "{k}々");
        }
    }

    #[test]
    fn two_char_word_repetition_keeps_rendaku() {
        // 部屋々々 = ヘヤベヤ: 形態素解析由来の 2 字語はカタカナでも従来どおり濁らせる
        let mut tokens = vec![token("部屋", "ヘヤ", 0..6), token("々", "々", 6..9)];
        apply_rendaku_inplace(&mut tokens);
        assert_eq!(tokens[1].reading, "ベヤ");
    }

    #[test]
    fn rendaku_applied_for_hito() {
        // 人々 → ヒト + ビト
        let mut tokens = vec![token("人", "ヒト", 0..3), token("々", "々", 3..6)];
        apply_rendaku_inplace(&mut tokens);
        assert_eq!(tokens[1].reading, "ビト");
    }

    #[test]
    fn rendaku_applied_for_hito_hiragana() {
        // ★round 48: 「人/ひと」 (ひらがな default) でも連濁できる。
        // unihan/joyo は default をひらがなで持つ entry が多く、 path は
        // Smart engine 経由で reading がひらがなのまま「々」 token と隣接する。
        let mut tokens = vec![token("人", "ひと", 0..3), token("々", "々", 3..6)];
        apply_rendaku_inplace(&mut tokens);
        assert_eq!(tokens[1].reading, "びと");
    }

    #[test]
    fn rendaku_applied_for_kami_hiragana() {
        // 神々 ひらがな版
        let mut tokens = vec![token("神", "かみ", 0..3), token("々", "々", 3..6)];
        apply_rendaku_inplace(&mut tokens);
        assert_eq!(tokens[1].reading, "がみ");
    }

    // ─── apply_rendaku_inplace: 連濁なし (= 複製) ──────────────────────────

    #[test]
    fn rendaku_falls_back_to_clone_for_non_voiceable() {
        // 我々 → ワレ + ワレ (ワ 行は連濁対象外、 そのまま複製)
        let mut tokens = vec![token("我", "ワレ", 0..3), token("々", "々", 3..6)];
        apply_rendaku_inplace(&mut tokens);
        assert_eq!(tokens[1].reading, "ワレ");
    }

    #[test]
    fn rendaku_falls_back_for_yama() {
        // 山々 → ヤマ + ヤマ
        let mut tokens = vec![token("山", "ヤマ", 0..3), token("々", "々", 3..6)];
        apply_rendaku_inplace(&mut tokens);
        assert_eq!(tokens[1].reading, "ヤマ");
    }

    // ─── apply_rendaku_inplace: edge cases ───────────────────────────────────

    #[test]
    fn rendaku_no_op_when_first_token_is_odoriji() {
        // 「々」 単独 / 先頭 は no-op (直前 token なし)
        let mut tokens = vec![token("々", "々", 0..3)];
        apply_rendaku_inplace(&mut tokens);
        assert_eq!(tokens[0].reading, "々"); // unchanged
    }

    #[test]
    fn rendaku_no_op_when_prev_reading_empty() {
        // 直前 reading が空文字 → 連濁適用しない (々 のまま)
        let mut tokens = vec![token("?", "", 0..1), token("々", "々", 1..4)];
        apply_rendaku_inplace(&mut tokens);
        assert_eq!(tokens[1].reading, "々");
    }

    #[test]
    fn rendaku_no_op_when_token_not_odoriji() {
        // 「々」 でない token は触らない
        let mut tokens = vec![token("神", "カミ", 0..3), token("社", "シャ", 3..6)];
        apply_rendaku_inplace(&mut tokens);
        assert_eq!(tokens[0].reading, "カミ");
        assert_eq!(tokens[1].reading, "シャ");
    }

    #[test]
    fn rendaku_handles_multiple_odoriji_in_sequence() {
        // 仮想例: 神々々 → 1 つ目 々 で カミ → ガミ、 2 つ目 々 は ガミ → 連濁無 (ガ は既に濁音) → ガミ 複製
        // (実用上稀だが logic 的には連鎖して動く)
        let mut tokens = vec![
            token("神", "カミ", 0..3),
            token("々", "々", 3..6),
            token("々", "々", 6..9),
        ];
        apply_rendaku_inplace(&mut tokens);
        assert_eq!(tokens[1].reading, "ガミ");
        // 2 つ目 々 の prev は 「々/ガミ」、 ガ は voice_first_kana 対象外 → 複製
        assert_eq!(tokens[2].reading, "ガミ");
    }
}

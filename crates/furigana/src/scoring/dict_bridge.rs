//! [`DictBridgeProvider`] — [`Dict`] (jukugo + unihan + `[[kanji]]` block) を
//! [`CandidateProvider`] として Smart engine に橋渡しする。

use crate::dict::Dict;
use crate::scoring::candidate::{
    Candidate, CandidateProvider, Score, ScoringContext, BAND_DICT_EXACT, BAND_KANJI,
};
use crate::scoring::matcher::{
    next2_logical_token, next_logical_token, prev_logical_token, resolve_readings, MatchContext,
};

/// 既存 [`Dict`] を [`CandidateProvider`] として scoring engine に流す bridge。
///
/// ## band 割り当て
///
/// - jukugo (≥ 2 文字 surface) → [`Score::dict_exact`] (band 1000)
/// - unihan (= 1 文字 surface) → [`Score::kanji`] (band 100)
///
/// reading は bracket notation を保持したまま Candidate に渡す。
/// Token 変換時に `parse_bracket_notation` で strip + accent 抽出。
///
/// ## 計算量
///
/// `candidates_at(pos)` は先頭 char bucket ([`Dict::rich_starting_with`] /
/// [`Dict::kanji_starting_with`]) だけを引くので O(E_char)
/// (= その char で始まる entry 数)。 0.1.5 で全件 linear scan (O(N×M)) から置換済。
pub struct DictBridgeProvider<'a> {
    dict: &'a Dict,
}

impl<'a> DictBridgeProvider<'a> {
    #[must_use]
    pub fn new(dict: &'a Dict) -> Self {
        Self { dict }
    }

    fn build_match_context(input: &str, pos: usize, end_pos: usize) -> MatchContext<'_> {
        let prev = prev_logical_token(input, pos);
        let next = next_logical_token(input, end_pos);
        let next2 = next2_logical_token(input, end_pos);
        MatchContext::with_all(
            if prev.is_empty() { None } else { Some(prev) },
            if next.is_empty() { None } else { Some(next) },
            if next2.is_empty() { None } else { Some(next2) },
        )
    }

    /// entries (`rich`) を emit。 戻り値 = **1 字 surface (= 先頭 char) を emit したか**
    /// (= 後段 kanji / unihan phase の dedup 判定用)。
    ///
    /// 先頭 char bucket だけを引く ([`Dict::rich_starting_with`])。 旧実装は全 ~44k
    /// entry を毎位置 linear scan していた (O(N×M))。
    fn emit_entries(
        &self,
        input: &str,
        pos: usize,
        tail: &str,
        first_char: char,
        out: &mut Vec<Candidate>,
    ) -> bool {
        let mut char_emitted = false;
        for (surface, entry) in self.dict.rich_starting_with(first_char) {
            if !tail.starts_with(surface) {
                continue;
            }
            let surface_byte_len = surface.len();
            let end_pos = pos + surface_byte_len;
            let char_count = surface.chars().count();
            let length = u8::try_from(char_count).unwrap_or(u8::MAX);

            let mctx = Self::build_match_context(input, pos, end_pos);

            let band = if char_count == 1 {
                BAND_KANJI
            } else {
                BAND_DICT_EXACT
            };

            for (reading, weight, hits) in resolve_readings(
                entry.matches(),
                entry.default_reading(),
                entry.alternatives(),
                &mctx,
            ) {
                out.push(Candidate::new(
                    surface.to_string(),
                    reading.to_string(),
                    pos..end_pos,
                    Score::with_weight(band, length, hits, weight),
                ));
            }

            if char_count == 1 {
                char_emitted = true; // 1 字 surface (= 先頭 char そのもの) を emit
            }
        }
        char_emitted
    }

    /// `[[kanji]]` block を emit (先頭 char の最初の 1 block のみ、 旧実装の dedup 等価)。
    /// 戻り値 = emit したか。 char index 引き ([`Dict::kanji_starting_with`])。
    fn emit_kanji_blocks(
        &self,
        input: &str,
        pos: usize,
        tail: &str,
        first_char: char,
        first_len: usize,
        out: &mut Vec<Candidate>,
    ) -> bool {
        let surface = &tail[..first_len];
        let end_pos = pos + first_len;
        // 旧実装は char 一致 block を全 walk して **最初の 1 つだけ** emit していた
        // (以降は emitted dedup で skip)。 index は char 一致 block のみ返すので first。
        let Some(block) = self.dict.kanji_starting_with(first_char).next() else {
            return false;
        };
        let mctx = Self::build_match_context(input, pos, end_pos);
        for (reading, weight, hits) in
            resolve_readings(&block.matches, &block.default, &block.alt, &mctx)
        {
            out.push(Candidate::new(
                surface.to_string(),
                reading.to_string(),
                pos..end_pos,
                Score::with_weight(BAND_KANJI, 1, hits, weight),
            ));
        }
        true
    }

    fn emit_unihan(&self, pos: usize, tail: &str, first_len: usize, out: &mut Vec<Candidate>) {
        let surface = &tail[..first_len];
        if let Some(reading) = self.dict.lookup_unihan(surface) {
            out.push(Candidate::new(
                surface.to_string(),
                reading.to_string(),
                pos..pos + first_len,
                Score::kanji(1),
            ));
        }
    }
}

impl<'a> CandidateProvider for DictBridgeProvider<'a> {
    fn candidates_at(&self, ctx: &ScoringContext, pos: usize) -> Vec<Candidate> {
        let input = ctx.input;
        let tail = &input[pos..];
        let Some(first_char) = tail.chars().next() else {
            return Vec::new();
        };
        let first_len = first_char.len_utf8();
        let mut out = Vec::new();

        // priority: entries (rich) > kanji block > unihan、 先頭 char surface 1 つ分は
        // 上位 phase が emit したら下位は skip (= 旧 `emitted` HashSet の dedup 等価、
        // ただし query 対象は常に先頭 1 字 surface なので bool で十分)。
        let mut char_emitted = self.emit_entries(input, pos, tail, first_char, &mut out);
        if !char_emitted {
            char_emitted =
                self.emit_kanji_blocks(input, pos, tail, first_char, first_len, &mut out);
        }
        if !char_emitted {
            self.emit_unihan(pos, tail, first_len, &mut out);
        }

        out
    }
}

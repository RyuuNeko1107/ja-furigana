//! [`DictBridgeProvider`] — [`Dict`] (jukugo + unihan + `[[kanji]]` block) を
//! [`CandidateProvider`] として Smart engine に橋渡しする。

use crate::dict::Dict;
use crate::scoring::candidate::{
    CandidateProvider, RawCandidate, Score, ScoringContext, BAND_DICT_EXACT, BAND_KANJI,
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
/// `candidates_at(pos)` は index 引きのみ ([`Dict::rich_matching_prefix`] /
/// [`Dict::kanji_starting_with`])。 0.1.5 で全件 linear scan (O(N×M)) を先頭 char
/// bucket 引き O(E_char) に置換し、 さらに先頭 2 文字での区間絞り込みを入れたので
/// 実走査は 「その 2 文字で始まる entry 数」 まで縮む (= 巨大 bucket の 御 713 件 /
/// 大 358 件 を毎位置舐めない)。
pub struct DictBridgeProvider<'a> {
    dict: &'a Dict,
}

impl<'a> DictBridgeProvider<'a> {
    #[must_use]
    pub fn new(dict: &'a Dict) -> Self {
        Self { dict }
    }

    fn build_match_context(input: &str, pos: usize, end_pos: usize) -> MatchContext<'_> {
        let span = same_char_run(input, pos, end_pos);
        let prev = if span.no_prev {
            ""
        } else {
            prev_logical_token(input, span.start)
        };
        let (next, next2) = if span.no_next {
            ("", "")
        } else {
            (
                next_logical_token(input, span.end),
                next2_logical_token(input, span.end),
            )
        };
        MatchContext::with_all(
            if prev.is_empty() { None } else { Some(prev) },
            if next.is_empty() { None } else { Some(next) },
            if next2.is_empty() { None } else { Some(next2) },
        )
        .with_full_input(input)
    }

    /// entries (`rich`) を emit。 戻り値 = **1 字 surface (= 先頭 char) を emit したか**
    /// (= 後段 kanji / unihan phase の dedup 判定用)。
    ///
    /// `tail` の接頭辞になりうる entry だけを引く ([`Dict::rich_matching_prefix`])。
    /// 旧実装は全 ~44k entry を毎位置 linear scan していた (O(N×M))。
    fn emit_entries<'b>(
        &self,
        input: &str,
        pos: usize,
        tail: &str,
        out: &mut Vec<RawCandidate<'b>>,
    ) -> bool
    where
        'a: 'b,
    {
        let mut char_emitted = false;
        for (surface, entry) in self.dict.rich_matching_prefix(tail) {
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
                out.push(RawCandidate::new(
                    reading,
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
    fn emit_kanji_blocks<'b>(
        &self,
        input: &str,
        pos: usize,
        first_char: char,
        first_len: usize,
        out: &mut Vec<RawCandidate<'b>>,
    ) -> bool
    where
        'a: 'b,
    {
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
            out.push(RawCandidate::new(
                reading,
                pos..end_pos,
                Score::with_weight(BAND_KANJI, 1, hits, weight),
            ));
        }
        true
    }

    /// unihan (1 字) フォールバック。
    ///
    /// ★後方互換のために残す legacy 経路。現行の loading では **到達しない**:
    /// unihan map は必ず `Dict::insert` (= 1 字 simple entry で rich と対) か
    /// `[[kanji]]` block (= kanji index に載る) 経由で埋まるため、この関数に来る前に
    /// `char_emitted` が立つ (entries / kanji block phase で emit 済)。
    /// = mutation は等価変異になるので `.cargo/mutants.toml` で除外している。
    /// 将来 unihan-only の load 経路 (rich にも kanji にも載らない 1 字 reading) を
    /// 追加する場合は、その除外を外して本フォールバックを直接テストすること。
    fn emit_unihan<'b>(
        &self,
        pos: usize,
        tail: &str,
        first_len: usize,
        out: &mut Vec<RawCandidate<'b>>,
    ) where
        'a: 'b,
    {
        let surface = &tail[..first_len];
        if let Some(reading) = self.dict.lookup_unihan(surface) {
            out.push(RawCandidate::new(
                reading,
                pos..pos + first_len,
                Score::kanji(1),
            ));
        }
    }
}

/// 同じ字の連続を何文字先まで見るか (これより長い連続は端が見えないものとして扱う)。
const SAME_CHAR_RUN_SCAN_MAX: usize = 32;

/// 1 字 surface の文脈判定範囲。 [`same_char_run`] の戻り値。
struct ContextSpan {
    /// 前文脈を取る位置 (この位置の手前の token が prev)
    start: usize,
    /// 後文脈を取る位置 (この位置からの token が next / next2)
    end: usize,
    /// 前文脈なしとする
    no_prev: bool,
    /// 後文脈なしとする
    no_next: bool,
}

/// 1 字 surface (`input[pos..end_pos]`) が **同じ字の連続** の一部なら、 文脈判定に使う
/// 範囲を連続全体へ広げる (1 字でなければ・連続でなければ元の範囲のまま)。
///
/// ## なぜ要るか
///
/// `[[kanji]]` block の 「前後が漢字なら音読み」 型 match (例: 海 = default うみ /
/// `prev_char_type = 漢字` で カイ) は、 強調の繰り返し 「海海海海」 で
/// **2 文字目以降だけ** 前が漢字 (= 同じ 海) になり、 「うみかいかいかい」 と
/// 先頭だけ読みが変わっていた。 同じ字の連続は熟語の隣接ではないので、 連続全体を
/// 1 単位として **外側** の文字で文脈を判定する (= 各字が単独の 海 と同じ読みに揃う)。
///
/// 同じ字に面している側を単に 「文脈なし」 にする案は、 連続が単語境界をまたぐ
/// 「三大大手」 「湯婆婆」 「小田田」 で前の漢字文脈を失って退行した (★2026-09-18 A/B)。
/// 一方、 連続の後ろの **ひらがな** (= 送り仮名) は連続の最後の字にしか係らないので、
/// 途中の字には効かせない (「遺言書書いて」 の 1 つ目の 書 が か になるのを防ぐ)。
/// 「堂堂」 のような正規の重ね語は band 1000 の dict entry が勝つので影響しない。
///
/// 連続が [`SAME_CHAR_RUN_SCAN_MAX`] 文字を超えて続く側は、 端を探さず文脈なしとする
/// (巨大な連続入力で位置ごとに全走査すると O(N²) になるため)。
fn same_char_run(input: &str, pos: usize, end_pos: usize) -> ContextSpan {
    let mut span = ContextSpan {
        start: pos,
        end: end_pos,
        no_prev: false,
        no_next: false,
    };
    let mut chars = input[pos..end_pos].chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return span;
    };
    let mut back = input[..pos].chars().rev().take_while(|&p| p == c);
    for n in 0.. {
        if back.next().is_none() {
            break;
        }
        if n >= SAME_CHAR_RUN_SCAN_MAX {
            span.no_prev = true;
            break;
        }
        span.start -= c.len_utf8();
    }
    let mut fwd = input[end_pos..].chars().take_while(|&q| q == c);
    for n in 0.. {
        if fwd.next().is_none() {
            break;
        }
        if n >= SAME_CHAR_RUN_SCAN_MAX {
            span.no_next = true;
            break;
        }
        span.end += c.len_utf8();
    }
    // 連続の途中の字: 後ろに続く送り仮名は最後の字のものなので使わない。
    if span.end != end_pos
        && input[span.end..]
            .chars()
            .next()
            .is_some_and(crate::kana::is_hiragana_char)
    {
        span.no_next = true;
    }
    span
}

impl<'a> CandidateProvider for DictBridgeProvider<'a> {
    fn candidates_at<'b>(
        &'b self,
        ctx: &ScoringContext<'b>,
        pos: usize,
        out: &mut Vec<RawCandidate<'b>>,
    ) {
        let input = ctx.input;
        let tail = &input[pos..];
        let Some(first_char) = tail.chars().next() else {
            return;
        };
        let first_len = first_char.len_utf8();

        // priority: entries (rich) > kanji block > unihan、 先頭 char surface 1 つ分は
        // 上位 phase が emit したら下位は skip (= 旧 `emitted` HashSet の dedup 等価、
        // ただし query 対象は常に先頭 1 字 surface なので bool で十分)。
        let mut char_emitted = self.emit_entries(input, pos, tail, out);
        if !char_emitted {
            char_emitted = self.emit_kanji_blocks(input, pos, first_char, first_len, out);
        }
        if !char_emitted {
            self.emit_unihan(pos, tail, first_len, out);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::Dict;
    use crate::scoring::boundary::BoundaryAnalysis;
    use crate::scoring::candidate::BAND_DICT_EXACT;

    fn ctx(input: &str) -> ScoringContext<'_> {
        let boundary = Box::leak(Box::new(BoundaryAnalysis::empty()));
        ScoringContext { input, boundary }
    }

    /// dedup 契約: 1 字 surface が rich entry にある時、emit_entries が char_emitted を
    /// 立てて下位 phase (kanji block / unihan fallback) を抑止する。これが壊れると
    /// 同じ 1 字に rich と unihan の二重候補が出る。
    /// (故障モデル: `char_count == 1` 判定の反転、または `if !char_emitted` guard の
    ///  ! 欠落で、unihan fallback が同一候補を重複 emit する)
    #[test]
    fn single_char_rich_entry_not_duplicated_by_fallback() {
        // simple entry は 1 字を rich と unihan の両方へ登録する。dedup が無いと
        // unihan fallback が同じ「犬」候補を二重に出してしまう。
        let dict = Dict::from_toml_str("[entries]\n\"犬\" = \"イヌ\"\n", "t.toml").unwrap();
        let provider = DictBridgeProvider::new(&dict);
        let cands = provider.candidates_vec(&ctx("犬"), 0);
        assert_eq!(
            cands.len(),
            1,
            "1字 rich entry が重複してはならない: {cands:?}"
        );
        assert_eq!(cands[0].reading, "イヌ");
        assert_eq!(cands[0].range, 0..3);
    }

    /// jukugo (≥2 字) は BAND_DICT_EXACT で、surface 全体の range を持つ。
    #[test]
    fn jukugo_entry_uses_dict_exact_band_and_full_range() {
        let dict = Dict::from_toml_str("[entries]\n\"猫舌\" = \"ネコジタ\"\n", "t.toml").unwrap();
        let provider = DictBridgeProvider::new(&dict);
        let cands = provider.candidates_vec(&ctx("猫舌だ"), 0);
        let neko = cands
            .iter()
            .find(|c| c.surface == "猫舌")
            .expect("猫舌 候補");
        assert_eq!(neko.reading, "ネコジタ");
        assert_eq!(neko.range, 0..6, "2 字 (各 3 byte) の full range");
        assert_eq!(neko.score.band, BAND_DICT_EXACT);
    }

    /// 先頭 char で始まらない位置では候補を出さない (tail.starts_with guard)。
    #[test]
    fn no_candidate_when_surface_does_not_match_tail() {
        let dict = Dict::from_toml_str("[entries]\n\"犬\" = \"イヌ\"\n", "t.toml").unwrap();
        let provider = DictBridgeProvider::new(&dict);
        // 入力に「犬」が無いので候補ゼロ
        assert!(provider.candidates_vec(&ctx("猫"), 0).is_empty());
    }

    const UMI: &str = "[meta]
schema_version = \"2\"

[[kanji]]
char = \"海\"
default = \"うみ\"

[[kanji.match]]
prev_char_type = \"漢字\"
reading = \"カイ\"
";

    fn readings_at(dict: &Dict, input: &str, pos: usize) -> Vec<String> {
        DictBridgeProvider::new(dict)
            .candidates_vec(&ctx(input), pos)
            .into_iter()
            .map(|c| c.reading)
            .collect()
    }

    /// 同じ字の繰り返しは熟語の隣接ではない: 「海海海」 の 2 字目以降も
    /// 単独の 海 と同じ文脈 (= 前が漢字でない) で読む。
    /// (故障モデル: 連続判定を外すと 2 字目以降だけ prev = 海 (漢字) で カイ になる)
    #[test]
    fn repeated_same_kanji_reads_like_standalone() {
        let dict = Dict::from_toml_str(UMI, "t.toml").unwrap();
        for pos in [0, 3, 6] {
            assert_eq!(readings_at(&dict, "海海海", pos), vec!["うみ"], "pos {pos}");
        }
        // 連続の外側の漢字文脈は効く (日本海 → カイ、 連続全体が 本 の後ろ)
        assert_eq!(readings_at(&dict, "本海海", 3), vec!["カイ"]);
        assert_eq!(readings_at(&dict, "本海海", 6), vec!["カイ"]);
    }

    #[test]
    fn same_char_run_spans_whole_run() {
        // 「山海海海川」 の 2 つ目の 海 (byte 6..9): 連続は 3..12
        let span = same_char_run("山海海海川", 6, 9);
        assert_eq!((span.start, span.end), (3, 12));
        assert!(!span.no_prev && !span.no_next);
        // 連続でない 1 字 / 複数字 surface は範囲そのまま
        let span = same_char_run("山海川", 3, 6);
        assert_eq!((span.start, span.end), (3, 6));
        let span = same_char_run("海海", 0, 6);
        assert_eq!((span.start, span.end), (0, 6));
    }

    /// 連続の後ろの送り仮名は最後の字にだけ係る (「遺言書書いて」 の 1 つ目の 書)。
    #[test]
    fn same_char_run_drops_okurigana_for_inner_char() {
        let inner = same_char_run("書書いて", 0, 3);
        assert!(inner.no_next, "途中の字は送り仮名を見ない");
        let last = same_char_run("書書いて", 3, 6);
        assert!(!last.no_next, "最後の字は送り仮名を見る");
        assert_eq!(last.end, 6);
    }

    /// 巨大な連続は端を探し切らず文脈なし (O(N²) 回避)。
    #[test]
    fn same_char_run_caps_scan_length() {
        let input = "海".repeat(SAME_CHAR_RUN_SCAN_MAX * 3);
        let mid = SAME_CHAR_RUN_SCAN_MAX * 3 / 2 * 3;
        let span = same_char_run(&input, mid, mid + 3);
        assert!(span.no_prev && span.no_next);
        let dict = Dict::from_toml_str(UMI, "t.toml").unwrap();
        assert_eq!(readings_at(&dict, &input, mid), vec!["うみ"]);
    }
}

//! Lindera fallback provider — Smart engine の safety net (★ alpha.13)。
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §5.6 (postprocess 分離) と同 layer 観。
//!
//! ## 役割
//!
//! Smart engine の 5 provider (Protect / Alphabet / DictBridge / Number / Odoriji)
//! が一切覆わない位置 (= 助詞 / okurigana / dict 未登録 単語) を band 50 で
//! 埋めるための **最終 fallback**。
//!
//! - corpus pass で 62% uncovered (= path 構築不能) だった主因が hiragana 助詞 /
//!   okurigana の coverage 不在。 本 provider 追加で path 構築率 ≒ 100% を期待。
//! - band 50 (= `Score::lindera`) で BAND_DICT_EXACT (1000) / BAND_SPECIAL (950) /
//!   BAND_KANJI (100) より低い、 weakest_band 集約で 「他に candidate があるなら
//!   そちら勝ち、 完全に何も無い時のみ Lindera が拾う」 を実現。
//!
//! ## 設計
//!
//! - construction time に input 全体を 1 度だけ tokenize、 edge 配列を保持
//!   (= 各 `candidates_at` で再 tokenize しない、 amortized O(1) lookup)
//! - 各 [`MorphToken`] を `(byte_start, byte_end, reading)` の edge に変換、
//!   reading が無い token (= 記号 / 未知語) は surface をそのまま reading とする
//! - Lindera surface byte 累積が input byte 長と不一致なら、 edge 生成中止
//!   (defensive、 通常は Normal mode で一致するはず)
//!
//! ## band の構造 (★alpha.20 で 2 段化)
//!
//! [`crate::scoring::engine::PathScore`] は `weakest_band` (= path 中の最低 band)
//! で path を比較する。 Lindera fallback は surface 形状で band を 2 段に分ける:
//!
//! - **band 150** (= [`Score::lindera_compound`]): surface 2 字以上 + 全 char 漢字。
//!   dict 未登録の純漢字熟語 (= 最近 / 風邪 / 海外 等) を、 単漢字 default 合成
//!   (band 100 × 2) で破壊せず形態素エンジン 1 token を優先する。
//! - **band 50** (= [`Score::lindera`]): それ以外 (= 単漢字 / 漢字+okurigana 混在 /
//!   助詞 / kana のみ)。 単漢字 default + `[[kanji]]` block match の方が信頼できる
//!   ため、 最終 safety net 位置に留める。
//!
//! いずれも dict 完全一致 (1000) / 特殊処理 (950) / `[[kanji]]` block (1000) には
//! 常に負ける = dict 整備が source of truth、 Lindera は形態素 fallback 専任。
//!
//! ## 注意点
//!
//! - per-input lifetime: input を変えるたびに新しい [`LinderaFallbackProvider`]
//!   を作る必要がある (= state cache が input に依存)。 `Furigana::analyze` 内で
//!   毎回 new する想定。
//! - thread safety: 内部 [`Analyzer`] は [`std::sync::Mutex`] 保護なので、
//!   並列実行は直列化される。 single-thread 用途では問題なし。
//! - reading は Lindera 由来 = カタカナ (IPADIC details[7])、 既存 provider と
//!   出力形式整合。 Lindera reading に bracket は含まれない想定。
//!   bracket strip は Token 変換時に `parse_bracket_notation` が一括処理。

use crate::analyzer::Analyzer;
use crate::scoring::candidate::{Candidate, CandidateProvider, Score, ScoringContext};

/// band-up 対象の判定: 「CJK 統合漢字範囲のみ」 で 々/〆/ヶ は除外する。
///
/// `is_kanji_char` (= kana.rs) は 々/〆/ヶ を含むが、 これらは専用 provider
/// (OdorijiProvider / NumberCandidateProvider) が担当する範囲で、 Lindera が
/// 横取りすると path 構造が壊れる (例: 我々 → 1 token band 150 vs dict 我 +
/// 々 odoriji band 100 で前者が勝ってしまう)。 そのため band-up 判定では
/// real CJK ideograph のみ対象とする。
fn is_real_cjk_ideograph(c: char) -> bool {
    matches!(
        c,
        '\u{3400}'..='\u{4DBF}' |    // CJK 拡張 A
        '\u{4E00}'..='\u{9FFF}' |    // CJK 統合漢字
        '\u{F900}'..='\u{FAFF}' |    // CJK 互換
        '\u{20000}'..='\u{2A6DF}' |  // CJK 拡張 B
        '\u{2A700}'..='\u{2EBEF}' |  // CJK 拡張 C/D/E/F
        '\u{2F800}'..='\u{2FA1F}' |  // CJK 互換補助
        '\u{30000}'..='\u{323AF}'    // CJK 拡張 G/H
    )
}

/// Lindera tokenize 結果を edge 配列で保持する fallback provider。
///
/// construction 時に 1 度だけ tokenize、 以降 `candidates_at` は O(edge_count)
/// で位置 lookup (= 通常 input なら数十 edges 程度、 amortized 軽量)。
#[derive(Debug, Clone, Default)]
pub struct LinderaFallbackProvider {
    /// (byte_start, byte_end, reading) の 3-tuple。
    /// reading は カタカナ (Lindera 由来) または surface fallback。
    edges: Vec<(usize, usize, String)>,
}

impl LinderaFallbackProvider {
    /// `analyzer` で `input` を tokenize、 edge 配列を構築。
    ///
    /// Lindera tokenize 失敗 / surface byte 不一致 時は edges 空で初期化
    /// (= provider 自体が無効、 caller path に影響なし)。
    #[must_use]
    pub fn new(analyzer: &Analyzer, input: &str) -> Self {
        if input.is_empty() {
            return Self::default();
        }
        let tokens = analyzer.tokenize(input);
        let mut edges = Vec::with_capacity(tokens.len());
        let mut byte_pos = 0usize;
        for tok in tokens {
            let surface_len = tok.surface.len();
            // Lindera は空白 / 改行 / 制御文字を token から落とすことがある
            // (例: input に `\n` / `( ・∇・)` の半角 space)。 落ちると byte_pos が
            // surface 開始位置とズレるので、 input 中で surface を前方探索して
            // アラインを回復し、 間に挟まった bytes を passthrough edge で補う。
            // こうしないと改行位置が誰にも覆われず dp[n] 未到達 → path 全空
            // (= 改行入り input で出力が空になる bug)。
            if input.get(byte_pos..byte_pos + surface_len) != Some(tok.surface.as_str()) {
                let Some(rel) = input[byte_pos..].find(tok.surface.as_str()) else {
                    // surface が前方にも無い (= 想定外のズレ) → safety net disable
                    return Self::default();
                };
                let gap_end = byte_pos + rel;
                if !Self::push_skipped_gap(input, byte_pos, gap_end, &mut edges) {
                    return Self::default();
                }
                byte_pos = gap_end;
            }
            let end = byte_pos + surface_len;
            // reading: Lindera details[7] (= カタカナ)、 無ければ surface fallback
            // (= 記号 / 未知語、 reading = surface で 「読まない」 扱い)
            let reading = tok.reading.unwrap_or_else(|| tok.surface.clone());
            edges.push((byte_pos, end, reading));
            byte_pos = end;
        }
        // 末尾に Lindera が落とした空白 / 改行が残る場合も passthrough で補う
        // (= 「猫\n」 のような trailing newline で path 全空になるのを防ぐ)。
        if byte_pos < input.len()
            && !Self::push_skipped_gap(input, byte_pos, input.len(), &mut edges)
        {
            return Self::default();
        }
        Self { edges }
    }

    /// 空 provider (test 用、 input なしで安全に new する)。
    #[allow(dead_code)] // test utility; kept for external callers and future use
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Lindera が落とした `input[start..end]` 区間を passthrough edge で補う。
    ///
    /// 区間が空白 / 制御文字のみ (= Lindera が token から落とす種類) なら、
    /// reading = surface の passthrough edge を 1 本足して `true`。 それ以外
    /// (= 実 surface のズレ、 構造的問題) は edge を足さず `false` を返し、
    /// caller に safety net 全廃 (disable) を促す。 空区間は no-op で `true`。
    fn push_skipped_gap(
        input: &str,
        start: usize,
        end: usize,
        edges: &mut Vec<(usize, usize, String)>,
    ) -> bool {
        if start >= end {
            return true;
        }
        let gap = &input[start..end];
        if gap.chars().all(|c| c.is_whitespace() || c.is_control()) {
            edges.push((start, end, gap.to_string()));
            true
        } else {
            false
        }
    }
}

impl CandidateProvider for LinderaFallbackProvider {
    fn candidates_at(&self, ctx: &ScoringContext, pos: usize) -> Vec<Candidate> {
        let input = ctx.input;
        self.edges
            .iter()
            .filter(|(start, _, _)| *start == pos)
            .map(|(start, end, reading)| {
                let surface = &input[*start..*end];
                let char_count = surface.chars().count();
                let length = u8::try_from(char_count).unwrap_or(u8::MAX);
                // ★alpha.20: 「2 字以上 + 全 char 漢字」 の surface に限り band 150 に
                // 格上げ。 dict 未登録の純漢字熟語 (= 最近 / 風邪 / 海外 等) で 単漢字
                // default 合成 (band 100 × 2) より形態素 1 token を優先する。
                //
                // 単漢字 (例: 私) は 50 のまま (= overrides.toml の `[[kanji]]` context
                // match で制御)、 漢字+okurigana 混在 (例: 来た) も 50 のまま
                // (= `[[kanji]]` block の `next_char_type` match で declarative 解決)。
                let score = if char_count >= 2 && surface.chars().all(is_real_cjk_ideograph) {
                    Score::lindera_compound(length)
                } else {
                    Score::lindera(length)
                };
                Candidate::new(surface.to_string(), reading.clone(), *start..*end, score)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::Analyzer;
    use crate::scoring::boundary::BoundaryAnalysis;

    fn ctx(input: &str) -> ScoringContext<'_> {
        let boundary = Box::leak(Box::new(BoundaryAnalysis::empty()));
        ScoringContext { input, boundary }
    }

    fn analyzer() -> Analyzer {
        Analyzer::new().expect("Analyzer init")
    }

    #[test]
    fn empty_input_yields_no_edges() {
        let a = analyzer();
        let p = LinderaFallbackProvider::new(&a, "");
        assert_eq!(p.candidates_at(&ctx(""), 0).len(), 0);
    }

    #[test]
    fn hiragana_particle_emits_band_50_candidate() {
        let a = analyzer();
        let input = "猫が好き";
        let p = LinderaFallbackProvider::new(&a, input);
        // 「猫」 の byte 範囲は 0..3 (UTF-8 3 byte)、 「が」 は 3..6
        let cands_at_3 = p.candidates_at(&ctx(input), 3);
        assert!(
            !cands_at_3.is_empty(),
            "expected Lindera edge at pos=3 (=が)"
        );
        let ga = cands_at_3.iter().find(|c| c.surface == "が");
        assert!(ga.is_some(), "expected が candidate: {cands_at_3:?}");
        let ga = ga.unwrap();
        assert_eq!(ga.score.band, 50);
        // 助詞 「が」 の Lindera reading は 「ガ」 (カタカナ)
        assert_eq!(ga.reading, "ガ");
    }

    // (band 値の純粋な大小比較は Score の Ord 契約であり candidate.rs で網羅済。
    //  ここでは Provider が実際に出す Candidate.score.band を検証する。)

    #[test]
    fn kanji_compound_surface_gets_band_150() {
        // 「最近」 (= 2 字純漢字 surface) の Lindera token → band 150 になることを確認。
        // dict に 「最近」 が未登録の前提 (= IPADIC 形態素解析だけが頼り)。
        let a = analyzer();
        let input = "最近の話";
        let p = LinderaFallbackProvider::new(&a, input);
        let cands_at_0 = p.candidates_at(&ctx(input), 0);
        // find が None だと旧 test は no-op で緑になっていた。候補存在を強制する。
        let saikin = cands_at_0
            .iter()
            .find(|c| c.surface == "最近")
            .unwrap_or_else(|| panic!("IPADIC は「最近」を 1 token で返すはず: {cands_at_0:?}"));
        assert_eq!(
            saikin.score.band, 150,
            "「最近」 (2 字純漢字) は band 150 (= lindera_compound) を期待"
        );
    }

    #[test]
    fn single_kanji_surface_stays_band_50() {
        // 単漢字 surface (= 1 字) は band 50 のまま (= overrides.toml に譲る)。
        let a = analyzer();
        let input = "私";
        let p = LinderaFallbackProvider::new(&a, input);
        let cands = p.candidates_at(&ctx(input), 0);
        let watashi = cands
            .iter()
            .find(|c| c.surface == "私")
            .unwrap_or_else(|| panic!("「私」 token が出るはず: {cands:?}"));
        assert_eq!(watashi.score.band, 50, "単漢字 surface は band 50 維持");
    }

    #[test]
    fn kanji_okurigana_mixed_stays_band_50() {
        // 漢字 + ひらがな混在 surface (= 「来た」) は band 50 (= [[kanji]] block 経路に譲る)。
        let a = analyzer();
        let input = "来た";
        let p = LinderaFallbackProvider::new(&a, input);
        let cands = p.candidates_at(&ctx(input), 0);
        // 空 Vec だと旧 for は vacuously pass。少なくとも 1 edge は出るはず。
        assert!(!cands.is_empty(), "「来た」 は edge を生むはず");
        // Lindera が 「来」 + 「た」 と 2 token に分ける場合、 各々 1 字 → 50。
        // 仮に 1 token (「来た」) で返した場合も混在で band 50。
        for c in &cands {
            assert_eq!(
                c.score.band, 50,
                "漢字+okurigana 混在 surface は band 50 維持 ({:?})",
                c.surface
            );
        }
    }

    #[test]
    fn unknown_token_falls_back_to_surface_reading() {
        // 完全に未知の記号列 等は Lindera reading None になるので surface を reading 化、
        // 「読まないが path 上の edge にはなる」 動作 (= path 構築の safety net = gap-passthrough)。
        let a = analyzer();
        let input = "★";
        let p = LinderaFallbackProvider::new(&a, input);
        let cands = p.candidates_at(&ctx(input), 0);
        // safety net は「必ず」 edge を出す契約 (空だと後段で文字が消える)。
        let first = cands
            .first()
            .unwrap_or_else(|| panic!("safety net は ★ にも edge を出すはず: {cands:?}"));
        assert!(
            !first.reading.is_empty(),
            "reading should fallback to surface"
        );
    }
}

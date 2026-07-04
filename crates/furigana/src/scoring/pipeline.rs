//! [`Pipeline`] — Smart Engine の解析パイプライン全体を束ねる facade。
//!
//! provider 構成 (6 provider + band 割り当て) ・ [`BoundaryAnalysis`] ・ Viterbi path
//! 選択 ([`crate::scoring::engine::solve_path`]) ・ Reading Post-pass
//! ([`crate::scoring::postpass`]) の適用順を **この 1 module が所有** する。
//! caller ([`crate::Furigana`]) は「input 文字列 → 確定 Token 列 / [`AnalyzeResult`]」
//! という interface だけを見ればよい。
//!
//! ## 構成 provider (band 順)
//!
//! 1. [`ProtectTokenProvider`] (band 2000): URL / Email / 絵文字
//! 2. [`AlphabetPassthroughProvider`] (hit 1000 / miss 100): 英字 passthrough + loanwords lookup
//! 3. [`DictBridgeProvider`] (jukugo 1000 / unihan 100 / `[[kanji]]` block)
//! 4. [`NumberCandidateProvider`] (band 950): 数字 + 助数詞 / スケール / SI 単位 / 日付 / 時刻
//! 5. [`OdorijiProvider`] (band 100): 々 placeholder edge、 post-pass で連濁適用
//! 6. [`LinderaFallbackProvider`] (band 50/150): 他 provider が覆わない位置の safety net
//!
//! provider 追加・順序変更・post-pass 追加はすべて本 module の変更で完結する
//! (= 旧実装は `Furigana::analyze` / `analyze_tokens` に provider 構成をコピペしており、
//! provider 追加時の drift 源だった)。

use crate::analyzer::Analyzer;
use crate::dict::Dict;
use crate::scoring::accent_estimate;
use crate::scoring::analyze::{
    analyze as run_analyze, analyze_tokens as run_analyze_tokens, AnalyzeResult, Token,
};
use crate::scoring::boundary::BoundaryAnalysis;
use crate::scoring::candidate::{CandidateProvider, ScoringContext};
use crate::scoring::dict_bridge::DictBridgeProvider;
use crate::scoring::lindera_fallback::LinderaFallbackProvider;
use crate::scoring::numbers::NumberCandidateProvider;
use crate::scoring::odoriji::OdorijiProvider;
use crate::scoring::postpass;
use crate::scoring::special::{AlphabetPassthroughProvider, ProtectTokenProvider};
use std::collections::HashMap;
use std::sync::Arc;

/// Smart Engine 解析セッションの facade。
///
/// [`crate::Furigana`] が保持する資源 (dict / 数字 provider / loanwords / 形態素解析器)
/// を借用して構築する軽量 view。 1 回の解析ごとに作って捨てる前提 (構築コストは
/// 参照のコピーのみ)。
pub struct Pipeline<'a> {
    dict: &'a Dict,
    number_provider: &'a NumberCandidateProvider,
    loanwords: &'a Arc<HashMap<String, String>>,
    analyzer: &'a Analyzer,
    /// rule-based accent 推定の opt-in flag (ADR-0007)。 post-pass 適用後に
    /// [`crate::scoring::accent_estimate::estimate`] を走らせるかどうか。
    estimate_accent: bool,
}

impl<'a> Pipeline<'a> {
    #[must_use]
    pub fn new(
        dict: &'a Dict,
        number_provider: &'a NumberCandidateProvider,
        loanwords: &'a Arc<HashMap<String, String>>,
        analyzer: &'a Analyzer,
        estimate_accent: bool,
    ) -> Self {
        Self {
            dict,
            number_provider,
            loanwords,
            analyzer,
            estimate_accent,
        }
    }

    /// production 経路: 採択 path の [`Token`] 列を返す (post-pass 適用済)。
    ///
    /// debug 用の `candidates` 集約 / `alternatives` 抽出を行わない軽量版。
    /// `to_ruby` / `to_hiragana` / `to_tts` / `to_romaji` / `tokenize` はこちら。
    #[must_use]
    pub fn tokens(&self, input: &str) -> Vec<Token> {
        let mut tokens = self.with_providers(input, run_analyze_tokens);
        postpass::apply_all(&mut tokens, self.dict, self.analyzer);
        if self.estimate_accent {
            accent_estimate::estimate(&mut tokens, self.analyzer);
        }
        tokens
    }

    /// debug / inspection 経路: [`AnalyzeResult`] (採択 path + 候補 + boundary region) を返す。
    ///
    /// 採択 path は [`Self::tokens`] と完全に同一 (= 同じ provider 構成・同じ post-pass)。
    /// 違いは debug 用集約 (`candidates` / `alternatives`) を含む点だけ。
    #[must_use]
    pub fn analyze(&self, input: &str) -> AnalyzeResult {
        let mut result = self.with_providers(input, run_analyze);
        postpass::apply_all(&mut result.tokens, self.dict, self.analyzer);
        if self.estimate_accent {
            accent_estimate::estimate(&mut result.tokens, self.analyzer);
        }
        result
    }

    /// 6 provider 構成 + [`BoundaryAnalysis`] + [`ScoringContext`] を組み立て、
    /// `run(ctx, providers)` を呼んで結果を返す共通土台。
    ///
    /// provider lifetime は本関数内 local に束縛されるので、 結果を返す形ではなく
    /// closure を渡す形にしている。
    fn with_providers<R>(
        &self,
        input: &str,
        run: impl FnOnce(&ScoringContext, &[&dyn CandidateProvider]) -> R,
    ) -> R {
        let protect = ProtectTokenProvider::new(input);
        let alphabet = AlphabetPassthroughProvider::new(input, Arc::clone(self.loanwords));
        let dict_bridge = DictBridgeProvider::new(self.dict);
        let odoriji = OdorijiProvider::new();
        let lindera = LinderaFallbackProvider::new(self.analyzer, input);
        let providers: [&dyn CandidateProvider; 6] = [
            &protect,
            &alphabet,
            &dict_bridge,
            self.number_provider,
            &odoriji,
            &lindera,
        ];

        let boundary = BoundaryAnalysis::analyze(input);
        let ctx = ScoringContext {
            input,
            boundary: &boundary,
        };
        run(&ctx, &providers)
    }
}

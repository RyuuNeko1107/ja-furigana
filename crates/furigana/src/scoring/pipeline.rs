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
    analyze as run_analyze, analyze_light as run_analyze_light,
    analyze_tokens as run_analyze_tokens, AnalyzeResult, Token,
};
use crate::scoring::boundary::BoundaryAnalysis;
use crate::scoring::candidate::{CandidateProvider, ScoringContext, Solver};
use crate::scoring::dict_bridge::DictBridgeProvider;
use crate::scoring::lattice::IpadicLatticeProvider;
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
    /// アクセント専用の表 (role = "accent")。 post-pass の後、 rule 推定の前に当てる
    accent_lexicon: &'a crate::scoring::accent_lexicon::AccentLexicon,
    analyzer: &'a Analyzer,
    /// コスト lattice engine (ADR-0011) の opt-in flag。
    cost_engine: bool,
    /// rule-based accent 推定の opt-in flag (ADR-0007)。 post-pass 適用後に
    /// [`crate::scoring::accent_estimate::estimate`] を走らせるかどうか。
    estimate_accent: bool,
    /// 人名の専用経路への呼び出し側ヒント (既定 = Auto)
    name_hint: crate::scoring::person_names::NameHint,
}

impl<'a> Pipeline<'a> {
    #[must_use]
    pub fn new(
        dict: &'a Dict,
        number_provider: &'a NumberCandidateProvider,
        loanwords: &'a Arc<HashMap<String, String>>,
        accent_lexicon: &'a crate::scoring::accent_lexicon::AccentLexicon,
        analyzer: &'a Analyzer,
        estimate_accent: bool,
        cost_engine: bool,
    ) -> Self {
        Self {
            dict,
            number_provider,
            loanwords,
            accent_lexicon,
            analyzer,
            estimate_accent,
            cost_engine,
            name_hint: crate::scoring::person_names::NameHint::Auto,
        }
    }

    /// 人名の専用経路へのヒントを指定する。
    #[must_use]
    pub fn with_name_hint(mut self, hint: crate::scoring::person_names::NameHint) -> Self {
        self.name_hint = hint;
        self
    }

    /// post-pass を掛け、 組み直された token (前後で範囲が変わったもの) の `band` を 50 (= 形態素解析だけで決まった扱い)
    /// に戻す。 post-pass の結果は 「どの候補が採用されたか」 の対応を失っているため、 元の token の band を
    /// 引き継がない。 範囲が変わらなかった token (読みだけ直したもの) は採用した候補の band のまま
    fn apply_postpass_with_band(&self, tokens: &mut Vec<Token>) {
        let before: Vec<(usize, usize)> = tokens
            .iter()
            .map(|t| (t.range.start, t.range.end))
            .collect();
        postpass::apply_all(tokens, self.dict, self.analyzer, self.name_hint);
        for t in tokens.iter_mut() {
            if before.binary_search(&(t.range.start, t.range.end)).is_err() {
                t.band = 50;
            }
        }
    }

    /// production 経路: 採択 path の [`Token`] 列を返す (post-pass 適用済)。
    ///
    /// debug 用の `candidates` 集約 / `alternatives` 抽出を行わない軽量版。 accent の付与
    /// (accent 専用の表 / 推定) も行わない: 読みだけ要る呼び出し (`tokenize` と、 そこから作る
    /// `to_hiragana` / `to_ruby` / `to_tts` / `to_romaji`) は accent を使わない。 accent が要るのは
    /// [`Self::analyze`]。
    #[must_use]
    pub fn tokens(&self, input: &str) -> Vec<Token> {
        let mut tokens = self.with_providers(input, run_analyze_tokens);
        postpass::apply_all(&mut tokens, self.dict, self.analyzer, self.name_hint);
        tokens
    }

    /// [`Self::analyze`] から debug 用の集約 (`candidates` / `alternatives` / `why`) を省いた版。 採択 path の token 列
    /// (`band` と accent を含む) は [`Self::analyze`] と同じ。 読みと band だけ要る呼び出し用
    #[must_use]
    pub fn analyze_light(&self, input: &str) -> AnalyzeResult {
        let mut result = self.with_providers(input, run_analyze_light);
        self.apply_postpass_with_band(&mut result.tokens);
        self.accent_lexicon.fill(&mut result.tokens);
        if self.estimate_accent {
            accent_estimate::estimate(&mut result.tokens, self.analyzer);
        }
        result
    }

    /// debug / inspection 経路: [`AnalyzeResult`] (採択 path + 候補 + boundary region) を返す。
    ///
    /// 採択 path は [`Self::tokens`] と完全に同一 (= 同じ provider 構成・同じ post-pass)。
    /// 違いは debug 用集約 (`candidates` / `alternatives`) を含む点だけ。
    #[must_use]
    pub fn analyze(&self, input: &str) -> AnalyzeResult {
        let mut result = self.with_providers(input, run_analyze);
        self.apply_postpass_with_band(&mut result.tokens);
        // 採用した読みの理由と、 採用しなかった読み (debug 経路だけ。 path は変えない)
        crate::scoring::explain::explain(&mut result.tokens, input, self.dict);
        self.accent_lexicon.fill(&mut result.tokens);
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
        run: impl for<'p> FnOnce(&ScoringContext<'p>, &[&'p dyn CandidateProvider]) -> R,
    ) -> R {
        let protect = ProtectTokenProvider::with_emoji(input, self.number_provider.emoji_table());
        let unit_symbols = self.number_provider.unit_symbols();
        let alphabet = AlphabetPassthroughProvider::with_units(
            input,
            Arc::clone(self.loanwords),
            unit_symbols,
        )
        .split_at_model_starts(&self.number_provider.model_starts(input));
        let dict_bridge = DictBridgeProvider::new(self.dict);
        let odoriji = OdorijiProvider::new();
        // 形態素 layer: 既定は Lindera の最良解 1 本、 cost engine では IPADIC lattice 全体
        // (ADR-0011)。 他の provider と post-pass は共通。
        let lindera =
            (!self.cost_engine).then(|| LinderaFallbackProvider::new(self.analyzer, input));
        let lattice = self
            .cost_engine
            .then(|| IpadicLatticeProvider::new(self.analyzer, input));
        let morph: &dyn CandidateProvider = match (&lindera, &lattice) {
            (Some(l), _) => l,
            (_, Some(l)) => l,
            _ => unreachable!("morph provider is always built"),
        };
        let providers: [&dyn CandidateProvider; 6] = [
            &protect,
            &alphabet,
            &dict_bridge,
            self.number_provider,
            &odoriji,
            morph,
        ];

        let boundary = BoundaryAnalysis::analyze(input);
        let mut ctx = ScoringContext::new(input, &boundary);
        if let Some(l) = &lattice {
            ctx.solver = Solver::Cost {
                analyzer: self.analyzer,
                noun_ids: l.noun_ids(),
            };
        }
        run(&ctx, &providers)
    }
}

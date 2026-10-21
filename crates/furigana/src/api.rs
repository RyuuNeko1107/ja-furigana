//! 公開 API: [`Furigana`] + [`FuriganaBuilder`]
//!
//! lib のエントリポイント。形態素解析器・ルール・辞書・チャンカーを
//! 1 つのオブジェクトに束ねて、`to_ruby` / `to_hiragana` / `to_tts` 等の
//! 高レベル変換メソッドを提供する。

use crate::analyzer::Analyzer;
use crate::dict::Dict;
use crate::error::Result;
use crate::reading::{tokens_to_hiragana, tokens_to_ruby, ReadingToken};
use crate::rules::RulesData;
use crate::scoring::analyze::{AlternativeReading, AnalyzeResult, Token as AnalyzeToken};
use crate::scoring::bracket::AccentPhrase;
use crate::scoring::numbers::NumberCandidateProvider;
use crate::scoring::person_names::NameHint;
use crate::scoring::pipeline::Pipeline;
use crate::tts::{self, TtsOptions};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

// ============================================================================
// Furigana 本体
// ============================================================================

/// フリガナ解決器
///
/// 内部で形態素解析器・ルール・辞書を保持する。
/// 通常は [`Furigana::minimal`] か [`Furigana::builder`] で構築する。
///
/// **lazy init**: Lindera 形態素解析器は構築時には初期化されず、最初の
/// `tokenize` / `to_*` 呼び出し時に [`OnceLock`] で 1 度だけ init される。
/// `Furigana::minimal()` の呼び出し自体は ~µs 級で済むため、引数 parse や
/// help 表示など analyze に至らない経路を高速化できる。サーバー起動時に
/// 先に init したい場合は [`Furigana::preload`] を呼ぶ。
pub struct Furigana {
    analyzer: OnceLock<Analyzer>,
    rules: RulesData,
    dict: Dict,
    /// Smart engine 用の数字系 candidate provider (★C3、 band 950)
    ///
    /// `analyze()` で provider として使う。 rules を pre-compile して保持、
    /// 各 `analyze()` 呼び出しごとに regex compile しない。
    number_provider: NumberCandidateProvider,
    /// 外来語 (alphabet surface) lookup map (★alpha.21 再統合)
    ///
    /// `core_dict_dirs` 配下の `role = "loanwords"` file から構築。
    /// key は [`normalize_alphabet`] 後の正規形 (= ASCII lowercase + 全角→半角)、
    /// value はカタカナ reading。
    /// [`AlphabetPassthroughProvider`] に渡して band 1000 で hit させる。
    loanwords: Arc<HashMap<String, String>>,
    /// アクセント専用の表 (dict の `role = "accent"`)。 bracket の無い token に 表記 + 読み一致で accent を付ける
    accent_lexicon: crate::scoring::accent_lexicon::AccentLexicon,
    /// アクセント句の連結規則の表 (dict の `role = "accent_chain"`)。 [`Furigana::to_accent_phrases`] が使う
    chain_table: crate::chain_phrases::ChainTable,
    /// rule-based accent 推定の opt-in flag (ADR-0007、 default false)。
    ///
    /// true のとき dict bracket を持たない token に外来語 -3 rule / 人名 rule で
    /// accent を推定し `estimated: true` で埋める。 読み出力 (`to_hiragana` 等) には
    /// 一切影響しない (accent_phrases のみ)。
    estimate_accent: bool,
    /// コスト lattice engine (ADR-0011、 実験中の opt-in)
    cost_engine: bool,
    /// compat (異体字表) の 1 文字 key の集合。 入力の正規化で 「この文字は何も起きない」 を素早く判定する
    compat_chars: crate::fxmap::FxSet<char>,
}

/// `analyze` 系の詳しさ。
#[derive(Debug, Clone, Copy)]
enum Depth {
    /// 候補・代替・理由まで
    Full,
    /// 採択 path + band + accent
    Light,
    /// 採択 path + band
    Reading,
}

impl Furigana {
    /// 最小構成で初期化 (空 default rules + Lindera + 空辞書)
    ///
    /// rules は `RulesData::default()` (= 全空)、辞書も空のため、
    /// 助数詞・文脈・スケール等の高度な処理は無効化される。
    /// 形態素解析 (Lindera) と直接 [`Self::add_reading`] による補完は動作する。
    /// 本格利用は `furigana dict pull` 後に [`Self::builder`] で
    /// `rules_dir` / `core_dict_dir` を指定してマウントする想定。
    ///
    /// # Errors
    /// 形態素解析器の初期化に失敗した場合。
    pub fn minimal() -> Result<Self> {
        Self::builder().build()
    }

    /// builder を取得
    #[must_use]
    pub fn builder() -> FuriganaBuilder {
        FuriganaBuilder::new()
    }

    /// 内部 [`Analyzer`] を取得 (必要なら初期化する)
    ///
    /// init は最初の呼び出しで 1 度だけ実行 ([`OnceLock`] 経由)。
    /// embed 済みの IPADIC を使うため init はほぼ失敗しないが、リソース不足等で
    /// 失敗した場合は panic する。事前に [`Self::preload`] で eager 初期化して
    /// 失敗を Result で受け取れる。
    fn analyzer(&self) -> &Analyzer {
        self.analyzer
            .get_or_init(|| Analyzer::new().expect("lindera analyzer init failed"))
    }

    /// 形態素解析器を eager に初期化する (server 起動時の preload 用)
    ///
    /// 通常は最初の `tokenize` / `to_*` 呼び出し時に lazy init されるが、
    /// 起動直後の最初のリクエストレイテンシを下げたい場合は build 直後に
    /// 呼んでおく。失敗時は [`crate::FuriganaError::AnalyzerInit`]。
    /// 既に init 済みの場合は no-op。
    ///
    /// # Errors
    /// 形態素解析器の初期化に失敗した場合。
    pub fn preload(&self) -> Result<()> {
        if self.analyzer.get().is_some() {
            return Ok(());
        }
        let analyzer = Analyzer::new()?;
        // set は既に init 済みだと Err を返すが、その場合は他スレッドが先に
        // 入れただけなので無視して良い。
        let _ = self.analyzer.set(analyzer);
        Ok(())
    }

    /// Smart Engine パイプライン facade を組み立てる。
    ///
    /// provider 構成・Viterbi path 選択・Reading Post-pass の適用順は
    /// [`Pipeline`] (`scoring/pipeline.rs`) が所有する。 本 method は Furigana が
    /// 保持する資源 (dict / 数字 provider / loanwords / 形態素解析器) を借用で
    /// 渡すだけの薄い結線。
    fn pipeline(&self) -> Pipeline<'_> {
        Pipeline::new(
            &self.dict,
            &self.number_provider,
            &self.loanwords,
            &self.accent_lexicon,
            self.analyzer(),
            self.estimate_accent,
            self.cost_engine,
        )
    }

    /// テキストをトークン化 (生 [`ReadingToken`] 列)
    ///
    /// 内部で `Pipeline::tokens` を呼び (= Smart engine path + Lindera fallback)、
    /// [`AnalyzeToken`] を [`ReadingToken`] に変換して返す。
    ///
    /// `to_hiragana` / `to_ruby` / `to_tts` / `to_romaji` は内部で本 method を呼ぶので、
    /// production の reading 解決経路はすべて本 method 経由。
    ///
    /// analyze の reading は常に String (空ではあり得るが None ではない)、 一律
    /// `Some(reading)` で包む。 reading が surface と kana 等価 (= 「の」 + 「ノ」) の
    /// ケースは [`tokens_to_hiragana`] / [`tokens_to_ruby`] 側で 「surface そのまま」
    /// と判定される。
    #[must_use]
    pub fn tokenize(&self, text: &str) -> Vec<ReadingToken> {
        self.tokenize_with_hint(text, NameHint::Auto)
    }

    /// [`Self::tokenize`] に、 入力が人名かどうかのヒントを添える版。
    ///
    /// `NameHint::Name` = 入力は人名 (ユーザー名・名簿の欄など。 手がかり無しで姓 + 名の表を引く) /
    /// `NameHint::NotName` = 人名の専用経路を使わない。 `Auto` は [`Self::tokenize`] と同じ。
    #[must_use]
    pub fn tokenize_with_hint(&self, text: &str, hint: NameHint) -> Vec<ReadingToken> {
        // Step 1: lookup 用に入力を正規化 (異体字→標準字 [compat]、 IVS/変異セレクタ除去、
        // NFKC)。 髙→高 / 葛󠄀→葛 / ①→1 等を解決してから形態素解析・辞書 lookup に渡す。
        // **表示 surface は原文を保つ** (lookup のみ正規化): 解析後に各 token の surface を
        // 原文へ戻すので 髙田 は {髙田|たかだ} になる (高田 に化けない)。
        let norm = crate::kana::normalize_text_aligned_with(
            text,
            &self.rules.compat,
            Some(&self.compat_chars),
        );
        let tokens = self.pipeline().with_name_hint(hint).tokens(&norm.text);
        // 正規化で何も変わらない通常の日本語入力は fast path (remap 不要 = 旧挙動と同一)。
        if !norm.changed {
            return tokens
                .into_iter()
                .map(|t| ReadingToken {
                    surface: t.surface,
                    reading: Some(t.reading),
                })
                .collect();
        }
        let remapped = norm.remap(text, tokens.iter().map(|t| t.surface.as_str()));
        let mut out: Vec<ReadingToken> = Vec::with_capacity(tokens.len());
        for (t, (surface, _range)) in tokens.into_iter().zip(remapped) {
            // 原文 1 文字が複数文字に展開され (例: 廿 → 二十)、 その展開が token 分割された
            // 場合、 後続 token は空 surface になる。 その読みを直前 token に結合して落とす
            // (= 原文 1 文字 → 1 出力 token、 `{|じゅう}` のような空 surface ruby を防ぐ)。
            if surface.is_empty() {
                if let Some(last) = out.last_mut() {
                    if let Some(r) = last.reading.as_mut() {
                        r.push_str(&t.reading);
                    }
                    continue;
                }
            }
            out.push(ReadingToken {
                surface,
                reading: Some(t.reading),
            });
        }
        out
    }

    /// 出力文字列に `postprocess.toml` のルール (`mode` に対応するもの) を適用する。
    ///
    /// `to_hiragana` / `to_ruby` / `to_tts` は内部で自動的に適用する。 トークンから出力を
    /// 自前で組み立てる呼び出し側 (HTTP サーバー等) が、 同じ後処理を掛けたい時に使う。
    /// `mode` は `"hiragana"` / `"ruby"` / `"tts"` / `"romaji"`
    #[must_use]
    pub fn postprocess(&self, text: &str, mode: &str) -> String {
        self.rules.postprocess.apply(text, mode)
    }

    /// テキスト → ひらがな文字列
    ///
    /// 漢字部分を読みのひらがなに置き換えた完全展開形を返す。TTS 等向け。
    /// 出力直前に `postprocess.toml` の `mode = "hiragana"` ルールを適用。
    #[must_use]
    pub fn to_hiragana(&self, text: &str) -> String {
        let hira = tokens_to_hiragana(&self.tokenize(text));
        self.rules.postprocess.apply(&hira, "hiragana")
    }

    /// テキスト → `{漢字|ひらがな}` 形式の ruby 文字列
    ///
    /// 例: `"灰桜の道"` → `"{灰桜|はいざくら}の{道|みち}"`
    /// 漢字を含まない部分はそのまま、読みなし部分も surface のまま。
    /// 出力直前に `postprocess.toml` の `mode = "ruby"` ルールを適用。
    ///
    /// **escape 規約**: 入力テキストに ruby 区切り記号 `{` `}` `|` や escape 文字 `\`
    /// が含まれる場合、 markup との衝突を避けるため素テキスト側でこれらを `\` で
    /// escape する (例: 入力 `"猫{犬}"` → `"{猫|ねこ}\{{犬|いぬ}\}"`)。 出力を機械的に
    /// パースする側は `\{` `\}` `\|` `\\` を un-escape すること。 これらの記号を含まない
    /// 通常の日本語入力では escape は発生せず、 出力は従来と完全に同一。
    #[must_use]
    pub fn to_ruby(&self, text: &str) -> String {
        let ruby = tokens_to_ruby(&self.tokenize(text));
        self.rules.postprocess.apply(&ruby, "ruby")
    }

    /// テキスト → TTS 向けに整形されたひらがな (ポーズ込み)
    ///
    /// 内部で [`Self::to_hiragana`] → [`tts::normalize_for_tts`] を走らせる。
    /// VOICEVOX 等の音声合成に流す前段で使う想定。
    /// 出力直前に `postprocess.toml` の `mode = "tts"` ルールを適用。
    #[must_use]
    pub fn to_tts(&self, text: &str, opts: &TtsOptions) -> String {
        // hiragana 自体の postprocess はここでは飛ばす (二重適用回避)。
        // 必要なら hiragana 用 postprocess を tts mode で再度書く想定。
        let mut tokens = self.tokenize(text);
        // 絵文字 / 顔文字パーツを落とす (opts.silence_symbols 次第)
        tts::filter_tokens_for_tts(&mut tokens, opts);
        let hira = tokens_to_hiragana(&tokens);
        let normalized = tts::normalize_for_tts(&hira, opts);
        self.rules.postprocess.apply(&normalized, "tts")
    }

    /// テキスト → ローマ字
    ///
    /// 内部で [`Self::to_hiragana`] → [`crate::romaji::hiragana_to_romaji`] を走らせる。
    /// 例: `"灰桜の散る道"` → `"haizakura no chiru michi"` (ヘボン式)。
    /// `style = RomajiStyle::Hepburn` (default) で b/m/p 前の n→m や ち→chi、
    /// `Kunrei` で規則的な si/ti/tu を出す。
    #[must_use]
    pub fn to_romaji(&self, text: &str, style: crate::romaji::RomajiStyle) -> String {
        // to_hiragana 内で hiragana 用 postprocess は適用済み
        let hira = self.to_hiragana(text);
        let romaji = crate::romaji::hiragana_to_romaji(&hira, style);
        self.rules.postprocess.apply(&romaji, "romaji")
    }

    /// TTS 出力を文末・読点で分割
    ///
    /// `max_segment_len` 以内のチャンクに分割した配列を返す。
    /// VOICEVOX 等の文字数制限対策。
    #[must_use]
    pub fn segment_tts(
        &self,
        text: &str,
        opts: &TtsOptions,
        max_segment_len: usize,
    ) -> Vec<String> {
        let normalized = self.to_tts(text, opts);
        tts::segment_for_tts(&normalized, max_segment_len)
    }

    /// 起動直後・辞書の入れ替え直後の最初のリクエストが遅くならないよう、 遅延初期化を先に済ませる。
    ///
    /// 形態素解析器の用意 (約 20ms)、 数字・URL・メール・絵文字・顔文字・型番の正規表現のコンパイル、
    /// 各経路の初回の確保などは最初に使われた時にまとめて起きるため、 何もしないと最初のリクエストが
    /// 数十 ms かかる (定常は 0.2ms)。 代表的な入力を一通り解析して結果は捨てる。 出力は変わらない
    /// (結果の cache は無い)。 サーバーは build 直後 (入れ替える前) に 1 回呼ぶ。
    pub fn warm_up(&self) {
        const SAMPLES: &[&str] = &[
            "今日は良い天気ですね。ありがとうございます！！ほんとに助かりました…",
            "水田さんと山田選手が来た、 <@123456789012> さん",
            "2026年10月17日 3時45分に10万円と3.5kg、 1/2、 一二三四五六七八九十と百万",
            "電話は090-1234-5678、 〒100-0001、 v2.0.1、 192.168.0.1",
            "詳細は https://example.com/a?b=1 と foo@example.com まで &amp; &#8230;",
            "😀草😭 (´・ω・｀) ^^ w ♪ ①②③ ㈱",
            "Hello World API iPhone15 RTX 4070 Ti GM ABC123 xyz",
        ];
        for s in SAMPLES {
            let _ = self.analyze(s);
            let _ = self.analyze_light(s);
            let _ = self.analyze_reading(s);
            let _ = self.tokenize(s);
        }
        let _ = self.to_mora_phrases("今日は良い天気ですね");
    }

    /// 動的に辞書エントリを追加 (override 用途)
    pub fn add_reading(&mut self, surface: impl Into<String>, reading: impl Into<String>) {
        self.dict.insert(surface, reading);
    }

    /// TOML 文字列を辞書に merge して、追加 (上書き含む) されたエントリ数を返す。
    ///
    /// ファイルシステムベースの `core_dict_dir` が使えない環境 (WASM など) 向け。
    /// ブラウザでは `fetch('./data/unihan.toml').then(r => r.text())` の結果を
    /// そのまま渡せる。形式は通常の `[entries]` セクション付き TOML:
    ///
    /// ```toml
    /// [entries]
    /// "灰桜" = "ハイザクラ"
    /// "黎明" = "レイメイ"
    /// ```
    ///
    /// `[entries]` 以外の TOML (例: `units.toml` の inline table) は内部で
    /// 自動的に skip される (lib 側 `Dict::from_toml_str` の defensive 実装による)。
    ///
    /// # Errors
    /// TOML parse 失敗時 [`crate::FuriganaError::Toml`]。
    pub fn merge_dict_toml(&mut self, content: &str) -> Result<usize> {
        let added = Dict::from_toml_str(content, "<merge_dict_toml>")?;
        let count = added.len();
        self.dict.merge(added);
        Ok(count)
    }

    /// 内部辞書のサイズ (デバッグ用)
    #[must_use]
    pub fn dict_size(&self) -> usize {
        self.dict.len()
    }

    /// Smart engine で input を analyze、 採択 path / 候補 / boundary region を返す (★F1)。
    ///
    /// debug / inspection API。 構成 provider (Protect / Alphabet+loanwords / DictBridge /
    /// Number / Odoriji / Lindera fallback) は `Pipeline` に集約。 本 method はその full 版
    /// (`Pipeline::analyze` = debug 用の `candidates` 集約 + `alternatives` 抽出込み)。
    /// production の `to_*` / `tokenize` は軽量 `Pipeline::tokens` 経由で同一 path を得る。
    ///
    /// ## 戻り値
    ///
    /// [`AnalyzeResult`] (= ★11 freeze、 0.1.0 stable で additive 追加のみ可)。
    ///
    /// 入力空 / 全 field 空、 path 構築不能 (= dict / 特殊処理で覆い切れない) →
    /// `tokens` / `path_indices` 空 で `candidates` / `boundary_regions` のみ返る。
    ///
    /// 「々」 token の reading は path 確定後に直前 token reading + 連濁判定で書き換え
    /// (`crate::scoring::postpass` の post-pass 群)、 placeholder の 「々」 は残らない。
    #[must_use]
    pub fn analyze(&self, input: &str) -> AnalyzeResult {
        self.analyze_with_hint(input, NameHint::Auto)
    }

    /// [`Self::analyze`] に、 入力が人名かどうかのヒントを添える版 ([`Self::tokenize_with_hint`] と同じ)。
    #[must_use]
    pub fn analyze_with_hint(&self, input: &str, hint: NameHint) -> AnalyzeResult {
        self.analyze_impl(input, hint, Depth::Full)
    }

    /// [`Self::analyze`] から debug 用の集約を省いた軽い版。
    ///
    /// 採択 path の token 列 (surface / reading / range / accent_phrases / `band`) は [`Self::analyze`] と同じ。
    /// `candidates` は空で、 token の `alternatives` / `ambiguous` / `why` は埋まらない。 全 provider を path 位置で
    /// 再走させる集約が要らない分、 約 3 割速い。 読みと accent と `band` (辞書が読みを持つ token かの手がかり) だけ
    /// 要る呼び出し (HTTP の通常 mode、 改善 signal の集計) 向け。 迷った読みの候補が要るなら [`Self::analyze`]。
    #[must_use]
    pub fn analyze_light(&self, input: &str) -> AnalyzeResult {
        self.analyze_light_with_hint(input, NameHint::Auto)
    }

    /// [`Self::analyze_light`] に、 入力が人名かどうかのヒントを添える版。
    #[must_use]
    pub fn analyze_light_with_hint(&self, input: &str, hint: NameHint) -> AnalyzeResult {
        self.analyze_impl(input, hint, Depth::Light)
    }

    /// [`Self::analyze_light`] から、 accent 専用の表 / 推定の付与も省いた版 (読みと `band` だけ)。
    ///
    /// `ruby` / `hiragana` / `tts` のように accent を使わない出力を作る呼び出し向け。 辞書の bracket が持つ accent は
    /// 残るが、 表引きの accent (accent 専用の表 / 推定) は付かない。 accent が要るなら [`Self::analyze_light`]
    #[must_use]
    pub fn analyze_reading(&self, input: &str) -> AnalyzeResult {
        self.analyze_impl(input, NameHint::Auto, Depth::Reading)
    }

    fn analyze_impl(&self, input: &str, hint: NameHint, depth: Depth) -> AnalyzeResult {
        // Step 1: lookup 用に入力正規化 (異体字 [compat] / IVS / NFKC)。tokenize と同じ前処理。
        // 表示 surface は原文を保つため、 解析後に token surface / range を原文へ remap する
        // (`candidates` / `boundary_regions` の range は正規化テキスト座標のまま = debug 用途)。
        let norm = crate::kana::normalize_text_aligned_with(
            input,
            &self.rules.compat,
            Some(&self.compat_chars),
        );
        let pipeline = self.pipeline().with_name_hint(hint);
        let mut result = match depth {
            Depth::Full => pipeline.analyze(&norm.text),
            Depth::Light => pipeline.analyze_light(&norm.text),
            Depth::Reading => pipeline.analyze_reading(&norm.text),
        };
        if norm.changed {
            let remapped = norm.remap(input, result.tokens.iter().map(|t| t.surface.as_str()));
            // tokenize と同じ規約: 多文字展開の分割で空 surface になった token は直前 token に
            // reading / accent_phrases / range を結合して落とす (= 原文 1 文字 → 1 token)。
            // candidates / path_indices は正規化座標の debug 集約なので据え置き。
            let mut merged: Vec<AnalyzeToken> = Vec::with_capacity(result.tokens.len());
            for (mut t, (surface, range)) in
                std::mem::take(&mut result.tokens).into_iter().zip(remapped)
            {
                if surface.is_empty() {
                    if let Some(last) = merged.last_mut() {
                        last.reading.push_str(&t.reading);
                        last.range.end = range.end;
                        last.accent_phrases.append(&mut t.accent_phrases);
                        continue;
                    }
                }
                // 異体字などを正規化して解析した token は、 原文に戻すと表記が正規化後と変わる。 改善 signal の集計は
                // 「解析した表記」 と 「原文の表記」 が一致する token だけを辞書の候補に結び付けてきたので、 その
                // 挙動を保つ (一致しない token は band 50 = 形態素解析だけで決まった扱い)
                if t.surface != surface {
                    t.band = 50;
                }
                t.surface = surface;
                t.range = range;
                merged.push(t);
            }
            result.tokens = merged;
        }
        result
    }

    /// accent mode 出力 (intonation.md §7.1)。
    ///
    /// [`Self::analyze`] を呼び、 token の accent_phrases を含む中立 JSON 向け構造体を返す。
    /// bracket notation がない token は `accent_phrases` 空。
    #[must_use]
    pub fn to_accent(&self, input: &str) -> AccentResult {
        AccentResult::from_analysis(self.analyze(input))
    }

    /// 診断用: 語単体の核が表に無く 平板で埋めたノード (表記, 読み, 品詞)。 表を育てる作業の入力で、 公開 API ではない
    #[doc(hidden)]
    #[must_use]
    pub fn chain_missing_accents(&self, input: &str) -> Vec<(String, String, String)> {
        let norm = crate::kana::normalize_text_aligned_with(
            input,
            &self.rules.compat,
            Some(&self.compat_chars),
        );
        let analysis = self.pipeline().analyze(&norm.text);
        let morphs = self.analyzer().tokenize(&norm.text);
        crate::chain_phrases::missing_accents(
            &norm.text,
            &analysis.tokens,
            &morphs,
            &self.accent_lexicon,
            &self.chain_table,
        )
    }

    /// 診断用: 各ノードの (表記, 読み, 品詞, 拍数, 語単体の核, 推定か)。 句の連結の前の値。 公開 API ではない
    #[doc(hidden)]
    #[must_use]
    pub fn chain_debug_nodes_detail(
        &self,
        input: &str,
    ) -> Vec<(String, String, String, usize, usize, bool)> {
        let norm = crate::kana::normalize_text_aligned(input, &self.rules.compat);
        let analysis = self.pipeline().analyze(&norm.text);
        let morphs = self.analyzer().tokenize(&norm.text);
        crate::chain_phrases::debug_nodes_detail(
            &norm.text,
            &analysis.tokens,
            &morphs,
            &self.accent_lexicon,
            &self.chain_table,
        )
    }

    /// 診断用: 各ノードの (表記, 読み, 品詞, 前の語にくっついたか)。 公開 API ではない
    #[doc(hidden)]
    #[must_use]
    pub fn chain_debug_nodes(&self, input: &str) -> Vec<(String, String, String, bool)> {
        let norm = crate::kana::normalize_text_aligned_with(
            input,
            &self.rules.compat,
            Some(&self.compat_chars),
        );
        let analysis = self.pipeline().analyze(&norm.text);
        let morphs = self.analyzer().tokenize(&norm.text);
        crate::chain_phrases::debug_nodes(
            &norm.text,
            &analysis.tokens,
            &morphs,
            &self.accent_lexicon,
            &self.chain_table,
        )
    }

    /// 入力から OpenJTalk (VOICEVOX) 方式のアクセント句列を作る。
    ///
    /// [`Self::to_accent`] の token 単位の核を、 OpenJTalk と同じ **句の連結規則 / アクセント結合** で句にまとめ直す
    /// (助詞は前の語にくっつく、 名詞 + 動詞は別の句、 複合語の核は後部の結合型で決まる、 等)。
    /// 連結規則の表 (dict の `role = "accent_chain"`) が無いと、 句は作れず空の `Vec` を返す。
    ///
    /// 語単体の核が表に無い語は 平板で埋め、 その句に `estimated: true` を付ける。
    #[must_use]
    pub fn to_accent_phrases(&self, input: &str) -> Vec<crate::chain_phrases::ChainedPhrase> {
        if self.chain_table.is_empty() {
            return Vec::new();
        }
        let norm = crate::kana::normalize_text_aligned_with(
            input,
            &self.rules.compat,
            Some(&self.compat_chars),
        );
        let analysis = self.pipeline().analyze(&norm.text);
        let morphs = self.analyzer().tokenize(&norm.text);
        crate::chain_phrases::build_chained_phrases(
            &norm.text,
            &analysis.tokens,
            &morphs,
            &self.accent_lexicon,
            &self.chain_table,
        )
    }

    /// TTS engine 向けの音声記号列 ([`crate::accent_symbols`]) の元になる 「モーラ列 + 核位置 + 区切り」。
    ///
    /// 辞書に連結規則の表 (`role = "accent_chain"`) があれば、 OpenJTalk と同じ句の連結規則で アクセント句を作る
    /// ([`Self::to_accent_phrases`])。 無ければ 従来どおり [`Self::to_accent`] の token 単位の核から作る。
    /// adapter crate (`ja-furigana-aquestalk` / `-voicevox`) の `Converter` と CLI / HTTP はこれを使う。
    #[must_use]
    pub fn to_mora_phrases(&self, input: &str) -> crate::accent_symbols::MoraPhrases {
        self.to_mora_phrases_with(input, crate::accent_symbols::SymbolOptions::default())
            .phrases
    }

    /// [`Self::to_mora_phrases`] の option 付き版。 辞書に無い英字語の扱い ([`crate::accent_symbols::UnknownAlpha`]) を選べ、
    /// 読めずに英字のまま残った語の一覧も返す。
    #[must_use]
    pub fn to_mora_phrases_with(
        &self,
        input: &str,
        opts: crate::accent_symbols::SymbolOptions,
    ) -> crate::accent_symbols::SymbolOutput {
        if self.chain_table.is_empty() {
            return crate::accent_symbols::to_mora_phrases_with(&self.to_accent(input), opts);
        }
        let norm = crate::kana::normalize_text_aligned_with(
            input,
            &self.rules.compat,
            Some(&self.compat_chars),
        );
        let analysis = self.pipeline().analyze(&norm.text);
        let morphs = self.analyzer().tokenize(&norm.text);
        let (phrases, gap) = crate::chain_phrases::build_chained_with_gap(
            &norm.text,
            &analysis.tokens,
            &morphs,
            &self.accent_lexicon,
            &self.chain_table,
        );
        crate::accent_symbols::chained_to_mora_phrases(&phrases, &gap, opts)
    }
}

// ─── AccentResult / AccentToken ─────────────────────────────────────────────

/// `--mode=accent` 中立 JSON 出力 (intonation.md §7.1)。
#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
pub struct AccentResult {
    pub schema_version: String,
    pub tokens: Vec<AccentToken>,
}

impl AccentResult {
    /// 解析済みの結果から accent 出力を作る (再解析しない)。
    ///
    /// [`Furigana::analyze`] の結果を別の用途 (ログの集計など) にも使う呼び出し側が、
    /// [`Furigana::to_accent`] で同じ入力をもう一度解析しなくて済むようにするための入口。
    /// `furigana.to_accent(text)` と `AccentResult::from_analysis(furigana.analyze(text))` は同じ結果になる
    #[must_use]
    pub fn from_analysis(result: AnalyzeResult) -> Self {
        Self {
            schema_version: "1".to_string(),
            tokens: result.tokens.into_iter().map(AccentToken::from).collect(),
        }
    }
}

/// accent mode の 1 token。
#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
pub struct AccentToken {
    pub surface: String,
    pub reading: String,
    pub accent_phrases: Vec<AccentPhrase>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ambiguous: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<AlternativeReading>,
}

impl From<AnalyzeToken> for AccentToken {
    fn from(t: AnalyzeToken) -> Self {
        Self {
            surface: t.surface,
            reading: t.reading,
            accent_phrases: t.accent_phrases,
            ambiguous: t.ambiguous,
            alternatives: t.alternatives,
        }
    }
}

impl std::fmt::Debug for Furigana {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Furigana")
            .field("dict_size", &self.dict.len())
            .field("counters", &self.rules.counters.counter.len())
            .finish_non_exhaustive()
    }
}

mod builder;
pub use builder::FuriganaBuilder;

#[cfg(test)]
mod tests;

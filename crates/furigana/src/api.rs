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
use crate::scoring::pipeline::Pipeline;
use crate::scoring::special::normalize_alphabet;
use crate::tts::{self, TtsOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
            self.analyzer(),
        )
    }

    /// テキストをトークン化 (生 [`ReadingToken`] 列)
    ///
    /// 内部で [`Pipeline::tokens`] を呼び (= Smart engine path + Lindera fallback)、
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
        self.pipeline()
            .tokens(text)
            .into_iter()
            .map(|t| ReadingToken {
                surface: t.surface,
                reading: Some(t.reading),
            })
            .collect()
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
        let hira = tokens_to_hiragana(&self.tokenize(text));
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
    /// Number / Odoriji / Lindera fallback) は [`Pipeline`] に集約。 本 method はその full 版
    /// ([`Pipeline::analyze`] = debug 用の `candidates` 集約 + `alternatives` 抽出込み)。
    /// production の `to_*` / `tokenize` は軽量 [`Pipeline::tokens`] 経由で同一 path を得る。
    ///
    /// ## 戻り値
    ///
    /// [`AnalyzeResult`] (= ★11 freeze、 0.1.0 stable で additive 追加のみ可)。
    ///
    /// 入力空 / 全 field 空、 path 構築不能 (= dict / 特殊処理で覆い切れない) →
    /// `tokens` / `path_indices` 空 で `candidates` / `boundary_regions` のみ返る。
    ///
    /// 「々」 token の reading は path 確定後に直前 token reading + 連濁判定で書き換え
    /// ([`crate::scoring::postpass`] の post-pass 群)、 placeholder の 「々」 は残らない。
    #[must_use]
    pub fn analyze(&self, input: &str) -> AnalyzeResult {
        self.pipeline().analyze(input)
    }

    /// accent mode 出力 (intonation.md §7.1)。
    ///
    /// [`Self::analyze`] を呼び、 token の accent_phrases を含む中立 JSON 向け構造体を返す。
    /// bracket notation がない token は `accent_phrases` 空。
    #[must_use]
    pub fn to_accent(&self, input: &str) -> AccentResult {
        let result = self.analyze(input);
        AccentResult {
            schema_version: "1".to_string(),
            tokens: result.tokens.into_iter().map(AccentToken::from).collect(),
        }
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

// ============================================================================
// FuriganaBuilder
// ============================================================================

/// [`Furigana`] を段階的に構築する builder
///
/// 全フィールド optional。指定しなければデフォルト (空) が使われる。
/// Dict は core → user → overrides → add_entry の順にマージされ、
/// 後のものが優先 (override) される。
#[derive(Debug, Default)]
pub struct FuriganaBuilder {
    rules_dir: Option<PathBuf>,
    core_dict_dirs: Vec<PathBuf>,
    user_dict_dirs: Vec<PathBuf>,
    overrides_files: Vec<PathBuf>,
    extra_entries: Vec<(String, String)>,
}

impl FuriganaBuilder {
    /// 空の builder を作る
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// ルール TOML をディレクトリから読み込む (デフォルト空を上書き)
    #[must_use]
    pub fn rules_dir(mut self, p: impl AsRef<Path>) -> Self {
        self.rules_dir = Some(p.as_ref().to_path_buf());
        self
    }

    /// core 辞書ディレクトリを追加 (複数指定可、優先度: 低)
    #[must_use]
    pub fn core_dict_dir(mut self, p: impl AsRef<Path>) -> Self {
        self.core_dict_dirs.push(p.as_ref().to_path_buf());
        self
    }

    /// user 辞書ディレクトリを追加 (複数指定可、優先度: 中)
    #[must_use]
    pub fn user_dict_dir(mut self, p: impl AsRef<Path>) -> Self {
        self.user_dict_dirs.push(p.as_ref().to_path_buf());
        self
    }

    /// overrides TOML ファイルを追加 (複数指定可、優先度: 高)
    #[must_use]
    pub fn overrides_file(mut self, p: impl AsRef<Path>) -> Self {
        self.overrides_files.push(p.as_ref().to_path_buf());
        self
    }

    /// 個別エントリをコード上で追加 (優先度: 最高)
    #[must_use]
    pub fn add_entry(mut self, surface: impl Into<String>, reading: impl Into<String>) -> Self {
        self.extra_entries.push((surface.into(), reading.into()));
        self
    }

    /// [`Furigana`] を構築
    ///
    /// 形態素解析器 (Lindera + IPADIC) は **lazy init** — 構築時には初期化せず、
    /// 最初の `tokenize` / `to_*` 呼び出し時に 1 度だけ初期化される。サーバー
    /// 起動時に init コストを払いたい場合は [`Furigana::preload`] を呼ぶ。
    ///
    /// # Errors
    /// - ルールファイルパース失敗 ([`crate::FuriganaError::Toml`])
    /// - 辞書ファイル/ディレクトリ I/O 失敗
    pub fn build(self) -> Result<Furigana> {
        let rules = match self.rules_dir.as_ref() {
            Some(dir) => crate::loader::load_rules_dir(dir)?,
            None => crate::embedded::rules()?,
        };

        let mut dict = Dict::new();
        for d in &self.core_dict_dirs {
            dict.merge(Dict::from_toml_dir(d)?);
        }
        for d in &self.user_dict_dirs {
            dict.merge(Dict::from_toml_dir(d)?);
        }
        for f in &self.overrides_files {
            dict.merge(Dict::from_toml_file(f)?);
        }
        for (s, r) in self.extra_entries {
            dict.insert(s, r);
        }

        // Smart engine 用の数字系 provider (★C3): rules を pre-compile して保持。
        // analyze() 呼び出しごとに rebuild すると regex compile cost が乗るので一度作る。
        let number_provider = NumberCandidateProvider::new(&rules);

        // ★alpha.21: loanwords を再統合。 core_dict_dirs / user_dict_dirs 配下から
        // `role = "loanwords"` file を集めて lookup map に。 AlphabetPassthroughProvider
        // に渡して band 1000 dict hit を実現する。
        let mut loanwords_map: HashMap<String, String> = HashMap::new();
        for d in &self.core_dict_dirs {
            load_loanwords_into(&mut loanwords_map, d)?;
        }
        for d in &self.user_dict_dirs {
            load_loanwords_into(&mut loanwords_map, d)?;
        }

        Ok(Furigana {
            analyzer: OnceLock::new(),
            rules,
            dict,
            number_provider,
            loanwords: Arc::new(loanwords_map),
        })
    }
}

/// `dir` 配下を再帰 scan し、 `role = "loanwords"` の TOML file から
/// `[entries]` table の `surface = reading` map を `out` に取り込む。
///
/// surface は [`normalize_alphabet`] で正規化 (= ASCII lowercase + 全角→半角)。
/// 同 surface に複数 reading が現れた場合、 後勝ち (= file 名 sort 順で merge 後勝ち)。
fn load_loanwords_into(out: &mut HashMap<String, String>, dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    // walk + schema 検証 + role 解決は `for_each_toml_in_dir` が共通担当。
    crate::loader::for_each_toml_in_dir(dir, |content, from, role| {
        if role != Some("loanwords") {
            return Ok(());
        }
        // [entries] table を parse (= role 別 toml 構造、 jukugo と同形式)
        #[derive(serde::Deserialize, Default)]
        struct LoanwordsToml {
            #[serde(default)]
            entries: HashMap<String, String>,
        }
        let parsed: LoanwordsToml = crate::loader::parse_toml(content, from)?;
        for (surface, reading) in parsed.entries {
            out.insert(normalize_alphabet(&surface), reading);
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_init_works() {
        let f = Furigana::minimal().expect("minimal init failed");
        // 漢字無しの入力は素通し
        assert_eq!(f.to_ruby("こんにちは"), "こんにちは");
    }

    #[test]
    fn add_reading_then_to_ruby() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("灰桜", "ハイザクラ");
        let ruby = f.to_ruby("灰桜");
        assert!(ruby.contains("はいざくら"), "ruby: {ruby}");
    }

    #[test]
    fn builder_with_extra_entries() {
        let f = Furigana::builder()
            .add_entry("灰桜", "ハイザクラ")
            .add_entry("黎明", "レイメイ")
            .build()
            .unwrap();
        assert_eq!(f.dict_size(), 2);

        let ruby = f.to_ruby("灰桜と黎明");
        assert!(ruby.contains("はいざくら"));
        assert!(ruby.contains("れいめい"));
    }

    #[test]
    fn to_hiragana_basic() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("灰桜", "ハイザクラ");
        let h = f.to_hiragana("灰桜の道");
        assert!(h.starts_with("はいざくら"), "h: {h}");
    }

    #[test]
    fn minimal_has_no_rules_loaded() {
        // 本体には rules を embed しない方針なので、minimal は空 default。
        // 「一人」は context.toml の default が無いため lindera 由来の読みになる。
        let f = Furigana::minimal().unwrap();
        let ruby = f.to_ruby("一人");
        // 何らかのひらがな化はされるはずだが、context default の "ヒトリ" は出ない
        // (lindera が 一+人 で個別に読むため、典型的には「いちにん」)
        assert!(!ruby.is_empty(), "ruby: {ruby}");
    }

    #[test]
    fn empty_input_yields_empty() {
        let f = Furigana::minimal().unwrap();
        assert_eq!(f.to_ruby(""), "");
        assert_eq!(f.to_hiragana(""), "");
        assert!(f.tokenize("").is_empty());
    }

    #[test]
    fn newline_input_does_not_blank_out() {
        // regression: 改行を含む input は Lindera が改行を token から落とし、
        // fallback provider が disable → path 全空 → 出力が空になる bug があった。
        // Lindera fallback の gap-passthrough で改行を覆い、 改行を保持したまま
        // 各行を正しく変換できることを確認する。
        let f = Furigana::minimal().unwrap();
        let hira = f.to_hiragana("猫が\n好き");
        assert_eq!(hira, "ねこが\nすき", "改行を挟んでも path 構築できる");

        // 先頭 / 末尾 / 連続改行も path 全空にならない
        assert_eq!(f.to_hiragana("\n猫"), "\nねこ");
        assert_eq!(f.to_hiragana("猫\n"), "ねこ\n");
        assert_eq!(f.to_hiragana("猫\n\n犬"), "ねこ\n\nいぬ");

        // ruby も同様 (漢字部だけ ruby 化、 改行はそのまま)
        let ruby = f.to_ruby("灰桜\n道");
        assert!(ruby.contains('\n'), "改行が保持される: {ruby}");
        assert!(!ruby.is_empty());
    }

    #[test]
    fn half_width_space_is_preserved_not_widened() {
        // 旧実装は preprocess_input で半角 space → 全角 space (U+3000) に置換していた
        // (= scoring engine が ASCII whitespace を覆えない問題の workaround)。
        // Lindera fallback の gap-passthrough 導入でこの hack は不要になり撤去。
        // 半角 space は出力にそのまま残る (= 全角化けしない) ことを確認する。
        let f = Furigana::minimal().unwrap();

        // 英字混在: space が全角化すると TTS / 表示で不自然
        assert_eq!(f.to_hiragana("hello world"), "hello world");
        assert!(!f.to_ruby("hello world").contains('\u{3000}'));

        // 旧 preprocess_input が導入された元凶ケース。 path 全空にならず、 かつ
        // 半角 space 区切りが維持される (全角に化けない)。
        let hira = f.to_hiragana("変なの 水田");
        assert!(!hira.is_empty(), "path 構築できる");
        assert!(hira.contains(' '), "半角 space が保持される: {hira:?}");
        assert!(!hira.contains('\u{3000}'), "全角化しない: {hira:?}");
    }

    #[test]
    fn debug_format_shows_summary() {
        let f = Furigana::minimal().unwrap();
        let s = format!("{f:?}");
        assert!(s.contains("Furigana"));
        assert!(s.contains("dict_size"));
    }

    // 注: 以下 3 テストは過去 cargo test harness で 51 GB alloc 暴走を起こしていたが、
    // 原因が旧 `NumberChunker` (chunks/ module、 alpha.15 で削除済) の dynamic regex の
    // **never-match pattern** (`r"(?P<n>\A\B)(?P<x>\A\B)"`) であったことを切り分け、
    // `Option<Regex>` 化で完全回避した。 同 pattern は現 scoring/numbers.rs の
    // regex builder 群にも踏襲されている。CHANGELOG 参照。

    #[test]
    fn to_tts_inserts_pauses() {
        let f = Furigana::minimal().unwrap();
        let opts = TtsOptions::default();
        let result = f.to_tts("こんにちは。さようなら。", &opts);
        assert!(result.contains("こんにちは。 "), "result: {result}");
    }

    #[test]
    fn to_tts_with_non_space_marker_preserves_long_pause() {
        let f = Furigana::minimal().unwrap();
        let opts = TtsOptions {
            short_pause: "<s>".to_string(),
            long_pause: "<l>".to_string(),
            keep_period: true,
        };
        let result = f.to_tts("こんにちは。さよなら。", &opts);
        assert!(result.contains("こんにちは。<l>"), "result: {result}");
    }

    #[test]
    fn segment_tts_returns_vec() {
        let f = Furigana::minimal().unwrap();
        let opts = TtsOptions::default();
        let segs = f.segment_tts("ぶん1。ぶん2。ぶん3。", &opts, 60);
        assert_eq!(segs.len(), 3);
    }

    #[test]
    fn rules_dir_overrides_default() {
        // テスト用 fixture (本来は furigana-dict から pull したものを使う)
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("rules");
        let f = Furigana::builder()
            .rules_dir(&dir)
            .build()
            .expect("build with rules_dir failed");
        // 3本 → サンボン (counters.toml 由来、 NumberCandidateProvider が hit)
        let hira = f.to_hiragana("3本");
        assert!(hira.contains("さんぼん"), "hiragana: {hira}");
    }

    // ─── Smart engine wire-up sanity tests ───────────────────────────────────

    #[test]
    fn to_ruby_uses_dict_then_lindera_fallback() {
        // 「灰桜の道」 → 灰桜 (dict band 1000、 ハイザクラ) + の (Lindera band 50、 ノ)
        // + 道 (Lindera band 50、 ミチ) → "{灰桜|はいざくら}の{道|みち}"
        let f = Furigana::builder()
            .add_entry("灰桜", "ハイザクラ")
            .build()
            .unwrap();
        let ruby = f.to_ruby("灰桜の道");
        assert!(ruby.contains("{灰桜|はいざくら}"), "expected ruby: {ruby}");
    }

    // ─── analyze() (F1) tests ────────────────────────────────────────────────

    #[test]
    fn analyze_empty_input_yields_empty_result() {
        let f = Furigana::minimal().unwrap();
        let r = f.analyze("");
        assert!(r.tokens.is_empty());
        assert!(r.candidates.is_empty());
        assert!(r.path_indices.is_empty());
        assert!(r.boundary_regions.is_empty());
    }

    #[test]
    fn analyze_single_jukugo_entry_yields_one_token() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("灰桜", "ハイザクラ");
        let r = f.analyze("灰桜");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].surface, "灰桜");
        assert_eq!(r.tokens[0].reading, "ハイザクラ");
        assert_eq!(r.tokens[0].range, 0..6); // UTF-8 3 bytes × 2
        assert_eq!(r.path_indices, vec![0]);
    }

    #[test]
    fn analyze_jukugo_prefers_longer_match_over_unihan() {
        // 「灰桜」 jukugo (band 1000、 length 2) が
        // 「灰」 unihan + 「桜」 unihan (各 band 100) を path レベルで上回る
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("灰桜", "ハイザクラ");
        f.add_reading("灰", "ハイ");
        f.add_reading("桜", "サクラ");
        let r = f.analyze("灰桜");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].reading, "ハイザクラ");
    }

    #[test]
    fn analyze_unihan_fallback_when_no_jukugo() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("猫", "ネコ");
        let r = f.analyze("猫");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].surface, "猫");
        assert_eq!(r.tokens[0].reading, "ネコ");
    }

    #[test]
    fn analyze_url_protected_token_passthrough() {
        let f = Furigana::minimal().unwrap();
        let input = "https://example.com";
        let r = f.analyze(input);
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].surface, input);
        assert_eq!(r.tokens[0].reading, input); // passthrough
    }

    #[test]
    fn analyze_alphabet_passthrough_returns_surface() {
        let f = Furigana::minimal().unwrap();
        let r = f.analyze("API");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].surface, "API");
        assert_eq!(r.tokens[0].reading, "API"); // passthrough_only (lookup 無し)
    }

    #[test]
    fn analyze_falls_back_to_lindera_when_no_other_provider_covers() {
        // alpha.13 以前: 「猫が好き」 のような ひらがな混在 input は dict / 保護 /
        // 英字 のどれも cover せず path 構築不能だった。
        // alpha.13+ : Lindera fallback (band 50) が input 全体を tokenize、
        // 他 provider が空の位置を埋めるので path が必ず構築される (safety net)。
        let f = Furigana::minimal().unwrap();
        let r = f.analyze("猫が好き");
        assert!(
            !r.tokens.is_empty(),
            "Lindera fallback should cover input: {r:?}"
        );
        // path 全体を Lindera で覆ったので token 列が input を完全に span するはず
        let total_len: usize = r.tokens.iter().map(|t| t.range.end - t.range.start).sum();
        assert_eq!(total_len, "猫が好き".len());
    }

    #[test]
    fn analyze_emits_boundary_region_for_kanji_run() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("灰桜", "ハイザクラ");
        let r = f.analyze("灰桜");
        // 漢字 2 字連続 region として検出される
        assert_eq!(r.boundary_regions.len(), 1);
        assert_eq!(r.boundary_regions[0], 0..6);
    }

    #[test]
    fn analyze_strips_intonation_brackets_from_reading() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("灰桜", "ハ[イザクラ");
        let r = f.analyze("灰桜");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].reading, "ハイザクラ");
        // 0.2.0: accent_phrases にパース結果が入る
        assert_eq!(r.tokens[0].accent_phrases.len(), 1);
        assert_eq!(r.tokens[0].accent_phrases[0].reading, "ハイザクラ");
        assert_eq!(r.tokens[0].accent_phrases[0].mora, 5);
        assert_eq!(r.tokens[0].accent_phrases[0].accent, Some(0)); // flat
    }

    #[test]
    fn to_accent_returns_accent_result_with_bracket_entry() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        let r = f.to_accent("雨");
        assert_eq!(r.schema_version, "1");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].surface, "雨");
        assert_eq!(r.tokens[0].reading, "アメ");
        assert_eq!(r.tokens[0].accent_phrases.len(), 1);
        assert_eq!(r.tokens[0].accent_phrases[0].accent, Some(1)); // 頭高
    }

    #[test]
    fn to_accent_no_brackets_yields_empty_phrases() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("猫", "ネコ");
        let r = f.to_accent("猫");
        assert_eq!(r.tokens[0].reading, "ネコ");
        assert!(r.tokens[0].accent_phrases.is_empty());
    }

    #[test]
    fn analyze_expands_odoriji_with_rendaku() {
        // 神々 → カミ + ガミ (連濁あり)
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("神", "カミ");
        let r = f.analyze("神々");
        assert_eq!(r.tokens.len(), 2);
        assert_eq!(r.tokens[0].surface, "神");
        assert_eq!(r.tokens[0].reading, "カミ");
        assert_eq!(r.tokens[1].surface, "々");
        assert_eq!(r.tokens[1].reading, "ガミ");
    }

    #[test]
    fn analyze_odoriji_falls_back_to_clone_for_non_voiceable() {
        // 我々 → ワレ + ワレ (ワ 行は連濁対象外、 そのまま複製)
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("我", "ワレ");
        let r = f.analyze("我々");
        assert_eq!(r.tokens.len(), 2);
        assert_eq!(r.tokens[1].surface, "々");
        assert_eq!(r.tokens[1].reading, "ワレ");
    }

    #[test]
    fn analyze_odoriji_loses_to_jukugo_when_dict_has_explicit_entry() {
        // dict に 「神々」 = カミガミ を登録すると、 jukugo (band 1000) が
        // 「神」+「々」 (band 100 × 2) を上回り、 単一 token に
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("神々", "カミガミ");
        f.add_reading("神", "カミ");
        let r = f.analyze("神々");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].surface, "神々");
        assert_eq!(r.tokens[0].reading, "カミガミ");
    }

    #[test]
    fn analyze_candidates_include_all_overlapping_entries() {
        // 同位置で jukugo + unihan が両方候補に上がる (path 採択は jukugo 勝ち)
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("灰桜", "ハイザクラ");
        f.add_reading("灰", "ハイ");
        let r = f.analyze("灰桜");
        // 採択 path は 「灰桜」 1 token
        assert_eq!(r.tokens.len(), 1);
        // candidates[0] には dict 由来の 「灰桜」 + 「灰」 の両方が上がる
        let pos0_surfaces: Vec<&str> = r.candidates[0].iter().map(|c| c.surface.as_str()).collect();
        assert!(pos0_surfaces.contains(&"灰桜"));
        assert!(pos0_surfaces.contains(&"灰"));
    }

    // ─── analyze() (C3) tests: NumberCandidateProvider 統合 ────────────────────

    fn fixture_rules_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("rules")
    }

    #[test]
    fn analyze_number_counter_path_uses_band_950() {
        // fixture rules 経由で counter regex が effective、 「3本」 が NumberProvider 経由で path に乗る
        let f = Furigana::builder()
            .rules_dir(fixture_rules_dir())
            .build()
            .expect("build with rules_dir");
        let r = f.analyze("3本");
        assert_eq!(r.tokens.len(), 1, "expected single counter token: {r:?}");
        assert_eq!(r.tokens[0].surface, "3本");
        assert_eq!(r.tokens[0].reading, "サンボン");
    }

    #[test]
    fn analyze_number_si_unit_beats_alphabet_passthrough_for_mixed_surface() {
        // 「100km」: AlphabetPassthrough が pure passthrough だと band 100 (miss)、
        // NumberProvider の SI candidate (band 950) が勝つ
        let f = Furigana::builder()
            .rules_dir(fixture_rules_dir())
            .build()
            .expect("build with rules_dir");
        let r = f.analyze("100km");
        assert_eq!(r.tokens.len(), 1, "expected single SI token: {r:?}");
        assert_eq!(r.tokens[0].surface, "100km");
        assert!(
            r.tokens[0].reading.contains("ヒャク") && r.tokens[0].reading.contains("キロメートル"),
            "reading: {}",
            r.tokens[0].reading,
        );
    }

    #[test]
    fn analyze_pure_digit_uses_number_provider_not_alphabet() {
        // 「100」 のみ: AlphabetPassthrough miss は band 100、 NumberProvider digit は band 950 → 後者が勝つ
        let f = Furigana::builder()
            .rules_dir(fixture_rules_dir())
            .build()
            .expect("build with rules_dir");
        let r = f.analyze("100");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].surface, "100");
        assert_eq!(r.tokens[0].reading, "ヒャク");
    }

    #[test]
    fn analyze_dict_entry_overrides_number_provider_for_counter_surface() {
        // dict に 「3本」 = カスタム読み を入れると band 1000 で NumberProvider 950 を override
        let mut f = Furigana::builder()
            .rules_dir(fixture_rules_dir())
            .build()
            .expect("build with rules_dir");
        f.add_reading("3本", "ミホン");
        let r = f.analyze("3本");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(
            r.tokens[0].reading, "ミホン",
            "dict 1000 が special 950 に勝つ"
        );
    }

    #[test]
    fn analyze_date_full_pattern_emits_single_token() {
        let f = Furigana::builder()
            .rules_dir(fixture_rules_dir())
            .build()
            .expect("build with rules_dir");
        let r = f.analyze("2025年10月30日");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].surface, "2025年10月30日");
        assert!(
            r.tokens[0].reading.contains("ジュウガツ"),
            "reading: {}",
            r.tokens[0].reading,
        );
    }

    #[test]
    fn analyze_numeric_phrase_emits_single_token() {
        // ★0.2.0 残件再統合: numeric_phrases (二十歳=ハタチ) が NumberCandidateProvider
        // (band 950) 経由で 1 token になり、 Lindera 分解 (二十+歳) に勝つ。
        let f = Furigana::builder()
            .rules_dir(fixture_rules_dir())
            .build()
            .expect("build with rules_dir");
        let r = f.analyze("二十歳");
        assert_eq!(r.tokens.len(), 1, "expected single phrase token: {r:?}");
        assert_eq!(r.tokens[0].surface, "二十歳");
        assert_eq!(r.tokens[0].reading, "ハタチ");

        // 非数字先頭の慣用語句 (明後日) も同様に 1 token
        let r2 = f.analyze("明後日");
        assert_eq!(r2.tokens.len(), 1, "expected single phrase token: {r2:?}");
        assert_eq!(r2.tokens[0].reading, "アサッテ");
    }

    #[test]
    fn analyze_dict_entry_overrides_numeric_phrase() {
        // dict 完全一致 (band 1000) は phrase (band 950) を上書きできる
        let mut f = Furigana::builder()
            .rules_dir(fixture_rules_dir())
            .build()
            .expect("build with rules_dir");
        f.add_reading("二十歳", "ニジュッサイ");
        let r = f.analyze("二十歳");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(
            r.tokens[0].reading, "ニジュッサイ",
            "dict 1000 が phrase 950 に勝つ"
        );
    }

    #[test]
    fn analyze_minimal_falls_back_to_lindera_for_counter_when_rules_empty() {
        // alpha.13 以前: minimal() = 空 RulesData → counter regex None で 「3本」 の
        // 「本」 を覆える provider 無し、 path 構築不能だった。
        // alpha.13+ : Lindera fallback が「3」「本」 を band 50 で edge 化、 path 構築成功。
        let f = Furigana::minimal().unwrap();
        let r = f.analyze("3本");
        assert!(
            !r.tokens.is_empty(),
            "Lindera fallback should provide edges: {r:?}"
        );
        let total_len: usize = r.tokens.iter().map(|t| t.range.end - t.range.start).sum();
        assert_eq!(total_len, "3本".len());
    }

    // ─── analyze() (★A2 alpha.12) DictBridge MatchCondition 評価 tests ─────

    /// Detailed entry を含む dict を temp file 経由で build する helper。
    /// Furigana の内部 dict 構築経路は file load なので、 unit test で
    /// rich field を inject するために temp TOML を書き出して dir 経由 load する。
    fn build_with_dict_toml(toml_body: &str) -> Furigana {
        let dir = std::env::temp_dir().join(format!(
            "furigana_dict_bridge_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dict_file = dir.join("test.toml");
        std::fs::write(
            &dict_file,
            format!(
                "[meta]\nschema_version = \"2\"\nrole = \"jukugo\"\n\n{}",
                toml_body
            ),
        )
        .unwrap();
        Furigana::builder()
            .core_dict_dir(&dir)
            .build()
            .expect("build with detailed entry")
    }

    #[test]
    fn dict_bridge_evaluates_inline_match_default_reading() {
        // 「上手」 単独 (= 文脈 「から」 無し) → default 「ジョウズ」
        let f = build_with_dict_toml(
            r#"[entries."上手"]
reading = "ジョウズ"

[[entries."上手".match]]
next_eq = "から"
reading = "カミテ"
"#,
        );
        let r = f.analyze("上手");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].reading, "ジョウズ");
    }

    #[test]
    fn dict_bridge_evaluates_inline_match_with_next_eq() {
        // 「上手から」 → 文脈 「から」 match → reading 「カミテ」
        // path 全 cover のため 「から」 も dict に Simple で追加
        let f = build_with_dict_toml(
            r#"[entries]
"から" = "カラ"

[entries."上手"]
reading = "ジョウズ"

[[entries."上手".match]]
next_eq = "から"
reading = "カミテ"
"#,
        );
        let r = f.analyze("上手から");
        let kamite_token = r.tokens.iter().find(|t| t.surface == "上手");
        assert!(
            kamite_token.is_some(),
            "expected 「上手」 token in path: {r:?}"
        );
        assert_eq!(kamite_token.unwrap().reading, "カミテ");
    }

    #[test]
    fn dict_bridge_evaluates_match_with_prev_eq() {
        // 「下上手」 → 「下」 (= 単漢字 dict entry あり) + 「上手」 (= prev_eq "下" → シタテ)
        let f = build_with_dict_toml(
            r#"[entries]
"下" = "シタ"

[entries."上手"]
reading = "ジョウズ"

[[entries."上手".match]]
prev_eq = "下"
reading = "シタテ"
"#,
        );
        let r = f.analyze("下上手");
        let jouzu_token = r.tokens.iter().find(|t| t.surface == "上手");
        assert!(jouzu_token.is_some(), "expected 「上手」 token: {r:?}");
        assert_eq!(jouzu_token.unwrap().reading, "シタテ");
    }

    // ─── analyze() (★A2 alpha.12) [[kanji]] block 評価 tests ───────────────

    /// [[kanji]] block (role = "kanji") を含む dict を temp file 経由で build する helper。
    fn build_with_kanji_toml(toml_body: &str) -> Furigana {
        let dir = std::env::temp_dir().join(format!(
            "furigana_kanji_block_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dict_file = dir.join("test.toml");
        std::fs::write(
            &dict_file,
            format!(
                "[meta]\nschema_version = \"2\"\nrole = \"kanji\"\n\n{}",
                toml_body
            ),
        )
        .unwrap();
        Furigana::builder()
            .core_dict_dir(&dir)
            .build()
            .expect("build with [[kanji]] block")
    }

    #[test]
    fn dict_bridge_evaluates_kanji_block_default_reading() {
        // 「生」 単独 → default 「セイ」 (= 文脈 「じる」 無し / prev 漢字ではない)
        let f = build_with_kanji_toml(
            r#"[[kanji]]
char = "生"
default = "セイ"

[[kanji.match]]
next_eq = "じる"
reading = "ショウ"
"#,
        );
        let r = f.analyze("生");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].reading, "セイ");
    }

    #[test]
    fn dict_bridge_evaluates_kanji_block_with_next_eq() {
        // 「生じる」 → 「生」 が next_eq "じる" match → 「ショウ」
        // [[kanji]] block の match condition が DictBridge で正しく評価される
        // ことを確認。 ただし Lindera が「生じる」 を 1 token として返す場合
        // (alpha.18 で 活用形 band up)、 1 edge path が勝つ可能性あり。
        // 「生」 単体 reading を確実に評価するため、 後続の context-only
        // 検証で final output 「ショウジル」 が含まれることを check。
        let f = build_with_kanji_toml(
            r#"[entries]
"じる" = "ジル"

[[kanji]]
char = "生"
default = "セイ"

[[kanji.match]]
next_eq = "じる"
reading = "ショウ"
"#,
        );
        let r = f.analyze("生じる");
        // path の reading 全体 (= 各 token の reading 結合) が 「ショウジル」 で始まる
        // ことを check (= 「生」 が 「ショウ」、 「じる」 が 「ジル」 / 「生じる」 1 token も同等)
        let combined: String = r.tokens.iter().map(|t| t.reading.as_str()).collect();
        assert_eq!(combined, "ショウジル", "result: {r:?}");
    }

    // ─── ADR-0004: ambiguous / alternatives ─────────────────────────────────

    #[test]
    fn analyze_entry_with_alt_shows_ambiguous() {
        let mut f = Furigana::minimal().unwrap();
        f.merge_dict_toml(
            r#"
[meta]
schema_version = "2"
role = "jukugo"

[entries."上手"]
reading = "ジョウズ"

[[entries."上手".alt]]
reading = "カミテ"
weight = 30
"#,
        )
        .unwrap();
        let r = f.analyze("上手");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].reading, "ジョウズ");
        assert!(r.tokens[0].ambiguous);
        assert_eq!(r.tokens[0].alternatives.len(), 1);
        assert_eq!(r.tokens[0].alternatives[0].reading, "カミテ");
        assert_eq!(r.tokens[0].alternatives[0].weight, 30);
    }

    #[test]
    fn analyze_entry_without_alt_not_ambiguous() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("猫", "ネコ");
        let r = f.analyze("猫");
        assert!(!r.tokens[0].ambiguous);
        assert!(r.tokens[0].alternatives.is_empty());
    }

    #[test]
    fn to_accent_with_alt_includes_alternatives() {
        let mut f = Furigana::minimal().unwrap();
        f.merge_dict_toml(
            r#"
[meta]
schema_version = "2"
role = "jukugo"

[entries."上手"]
reading = "ジョ]ウズ"

[[entries."上手".alt]]
reading = "カ]ミテ"
weight = 30
"#,
        )
        .unwrap();
        let r = f.to_accent("上手");
        assert!(r.tokens[0].ambiguous);
        assert_eq!(r.tokens[0].alternatives[0].reading, "カミテ");
    }
}

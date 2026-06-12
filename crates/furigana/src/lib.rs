//! # ja-furigana
//!
//! Japanese **furigana / TTS-prep engine** in Rust. 形態素解析 (Lindera + IPADIC) と
//! TOML データ駆動の辞書 / ルールを組み合わせた決定論的エンジン。
//!
//! > **crate 名は `ja-furigana`、import 名は `furigana`** (`[lib] name = "furigana"`)。
//! > `cargo add ja-furigana` した後は `use furigana::Furigana;` で使う。
//!
//! ## 立ち位置
//!
//! - ✅ TTS 前段 (VOICEVOX / OpenAI TTS) で「漢字を含む文 → ひらがな」に一括変換
//! - ✅ Web / ブログ記事の `<ruby>` タグ自動生成
//! - ✅ 配信テロップの難読語チェック
//! - ❌ 機械学習ベースの文脈推論 (BERT 等) は持たない (決定論的)
//! - ❌ 辞書外の人名 / 古文 / 完璧な同形異音語解決
//!
//! 機械学習なしの **保守的な決定論**。コミュニティが [`ja-furigana-dict`](https://github.com/RyuuNeko1107/ja-furigana-dict) に PR で読みを足すほど精度が上がる設計。
//!
//! ## クイックスタート
//!
//! ```no_run
//! use furigana::Furigana;
//!
//! let mut f = Furigana::minimal()?;          // Lindera は lazy init
//! f.add_reading("灰桜", "ハイザクラ");        // 動的に辞書追加
//!
//! assert_eq!(
//!     f.to_ruby("灰桜の散る道"),
//!     "{灰桜|はいざくら}の{散る|ちる}{道|みち}"
//! );
//! assert_eq!(f.to_hiragana("灰桜の散る道"), "はいざくらのちるみち");
//! # Ok::<_, furigana::FuriganaError>(())
//! ```
//!
//! 詳しい使い方:
//! - **builder API で辞書を mount** → [`FuriganaBuilder`]
//! - **TTS 整形** (ポーズ込み) → [`Furigana::to_tts`] + [`TtsOptions`]
//! - **TTS セグメント分割** (VOICEVOX 文字数制限対策) → [`Furigana::segment_tts`]
//! - **ローマ字** (ヘボン式 / 訓令式) → [`Furigana::to_romaji`] + [`RomajiStyle`]
//! - **trace 用 token dump** → [`Furigana::tokenize`] + [`ReadingToken`]
//! - **TOML 文字列を直接 merge** (ファイル不要) → [`Furigana::merge_dict_toml`]
//! - **server で eager init** → [`Furigana::preload`]
//!
//! ## サンプル
//!
//! - [`examples/basic.rs`](https://github.com/RyuuNeko1107/ja-furigana/blob/master/crates/furigana/examples/basic.rs)
//!   — 全モード (ruby / hiragana / tts / romaji) を素朴に
//! - [`examples/builder.rs`](https://github.com/RyuuNeko1107/ja-furigana/blob/master/crates/furigana/examples/builder.rs)
//!   — core / user / overrides 3 階層の優先順位
//!
//! ## アーキテクチャ
//!
//! 公開 API は [`Furigana`] / [`FuriganaBuilder`] で、内部は以下の module に分かれる:
//!
//! - [`analyzer`] : 形態素解析 (Lindera + IPADIC、 Smart engine fallback として使用)
//! - [`kana`]     : ひら⇄カタ + Unicode 正規化 (文字種判定は内部 `char_class` module に集約)
//! - [`numbers`]  : 数値処理 (digit / counter / extras / `kansuji_to_arabic`)
//! - [`reading`]  : 出力 layer (= [`ReadingToken`] + tokens_to_hiragana / tokens_to_ruby)
//! - [`tts`]      : TTS 整形 + segment
//! - [`romaji`]   : ひらがな → ローマ字 (Hepburn / Kunrei)
//! - [`dict`]     : surface → reading 辞書 (内部で **jukugo (≥2 文字) / unihan (1 文字) /
//!   `[[kanji]]` block / Detailed Entry 多重保持**)
//! - [`rules`]    : ルールデータ型 (counters / scales / units / **postprocess** / etc)
//! - [`loader`]   : TOML 汎用パーサ
//! - [`scoring`]  : Smart engine (Viterbi DP + 6 provider band lexicographic)
//!
//! ### 読み解決パイプライン (alpha.15+、 Smart engine 一本化)
//!
//! [`Furigana::analyze`] で input を 6 provider に流し、 [`scoring::engine::solve_path`]
//! で Viterbi-like path を解く。 各 provider が band 付きの candidate edge を emit:
//!
//! 1. **ProtectTokenProvider** (band 2000): URL / Email / 絵文字
//! 2. **AlphabetPassthroughProvider** (band 1000 / 100): 英字 passthrough
//! 3. **DictBridgeProvider** (band 1000 / 100): dict surface (jukugo / unihan / `[[kanji]]`)
//! 4. **NumberCandidateProvider** (band 950): 数字 + 助数詞 / 大数 / SI / 日付 / 時刻
//! 5. **OdorijiProvider** (band 100): 踊り字 「々」 placeholder + post-pass 連濁
//! 6. **LinderaFallbackProvider** (band 50): 上記 5 が一切覆わない位置の safety net
//!
//! [`Furigana::tokenize`] (= `to_hiragana` / `to_ruby` / `to_tts` / `to_romaji` の基盤)
//! は本 analyze 経路を通り、 [`AnalyzeToken`] を [`ReadingToken`] 化して返す。
//! 出力直前に `rules/postprocess.toml` の **mode 別 regex 置換** が適用される。
//!
//! 詳細は [docs/ARCHITECTURE.md](https://github.com/RyuuNeko1107/ja-furigana/blob/master/docs/ARCHITECTURE.md) を参照。
//!
//! ## ステータス
//!
//! v0.1.x (alpha) — 公開 API は変更されうる ([docs/ROADMAP.md](https://github.com/RyuuNeko1107/ja-furigana/blob/master/docs/ROADMAP.md) 参照)。MSRV: Rust 1.88+。
//!
//! 内部例文 75 件回帰で **75/75 (100%)** 達成 (0.1.0-alpha.3、CHANGELOG 参照)。

#![allow(clippy::tabs_in_doc_comments)]

pub mod analyzer;
pub(crate) mod char_class;
pub mod dict;
pub mod error;
pub mod kana;
pub mod loader;
pub mod numbers;
pub mod reading;
pub mod romaji;
pub mod rules;
pub(crate) mod scoring;
pub mod tts;

mod api;
mod embedded;
mod sanitize;

pub use crate::api::{AccentResult, AccentToken, Furigana, FuriganaBuilder};
pub use crate::dict::Dict;
pub use crate::error::{FuriganaError, Result};
pub use crate::reading::{tokens_to_hiragana, tokens_to_ruby, ReadingToken};
pub use crate::romaji::{hiragana_to_romaji, RomajiStyle};
pub use crate::scoring::analyze::{AlternativeReading, AnalyzeResult, Token as AnalyzeToken};
pub use crate::scoring::bracket::AccentPhrase;
pub use crate::scoring::inspect::{
    extract_dict_gap_candidates, surface_with_context, token_band, ContextWindow, DictGapCandidate,
};
pub use crate::tts::TtsOptions;

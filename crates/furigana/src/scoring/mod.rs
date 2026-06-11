//! Scoring engine (alpha.10〜0.1.0 stable で投入する新 architecture)。
//!
//! 「答えを持つ辞書」 から 「候補を出す辞書」 への転換、 Viterbi-like path 選択で
//! 文全体の最良 path を採用する。 詳細仕様は
//! `docs/PROPOSALS/scoring-engine.md` 参照。
//!
//! ## 構成
//!
//! - [`pipeline`]: **解析パイプライン facade** (provider 構成 + Viterbi + post-pass を所有)
//! - [`format`]: 新 dict format の struct + Deserialize (entry inline match /
//!   `[[kanji]]` block / matcher conditions / 文字種列挙)
//! - [`matcher`]: matcher 条件の評価 logic
//! - [`candidate`]: Candidate / Score (band lexicographic 比較)
//! - [`engine`]: Smart engine 本体 (Viterbi-like)
//! - [`boundary`]: (b)(c) 漢字連続 boundary penalty
//! - [`special`] / [`numbers`] / [`odoriji`] / [`dict_bridge`] / [`lindera_fallback`]: 各 provider
//! - [`postpass`]: path 確定後の token 列補正 seam (ADR-0005)
//! - [`analyze`]: `AnalyzeResult` / `analyze()` debug API
//!
//! ## 旧 architecture (Strict engine) との関係
//!
//! 旧 Strict engine (`reading/pipeline.rs` の `resolve_reading` 7-step) は alpha.15 で
//! 削除済。 Smart engine が唯一の読み解決系統 (= [`crate::Furigana::tokenize`] /
//! `to_*` / `analyze` はすべて [`pipeline::Pipeline`] 経由)。
//!
//! ## postprocess との分離 (★C4)
//!
//! [`crate::rules::postprocess`] (mode 別 regex 置換 layer) は **本 module と独立**:
//!
//! - scoring engine は input → candidate path 確定 までを担当 (= 何を読むか の決定)
//! - postprocess は path 確定後の output layer (= 文字列レベルの最終調整)
//! - [`crate::Furigana::analyze`] は postprocess を呼ばず raw reading を返す
//!
//! 詳細: `docs/PROPOSALS/scoring-engine.md` §5.6

pub mod analyze;
pub mod boundary;
pub mod bracket;
pub mod candidate;
pub mod contextual;
pub mod dict_bridge;
pub mod engine;
pub mod format;
pub mod inspect;
pub mod lindera_fallback;
pub mod matcher;
pub mod numbers;
pub mod odoriji;
pub mod pipeline;
pub mod postpass;
pub mod special;

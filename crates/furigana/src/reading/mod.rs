//! 読みトークン型 + 出力 helper (= Smart engine の output layer)
//!
//! alpha.15 で旧 Strict pipeline (`pipeline` / `merge` / `context`) を削除、
//! `output` のみが残った。 [`ReadingToken`] は API 公開型として残し、
//! [`crate::Furigana::tokenize`] が Smart `analyze()` の出力を本型に変換して返す
//! (= 既存 caller の互換性維持)。
//!
//! ## 公開 API
//! - [`ReadingToken`]
//! - [`tokens_to_hiragana`] / [`tokens_to_ruby`]

pub mod output;

pub use output::{tokens_to_hiragana, tokens_to_ruby};

/// 読み付きトークン
///
/// 旧 Strict pipeline では 漢字無し surface の reading が `None` だったが、
/// Smart engine wire-up (alpha.14+) 以降は **常に `Some(reading)`** で返る。
/// 出力 helper ([`tokens_to_hiragana`] / [`tokens_to_ruby`]) は `Some` の場合でも
/// surface == reading (kana 等価) なら surface をそのまま使う設計のため、 caller
/// 側で reading の有無を気にせず使える。
///
/// `None` variant は 0.1.0 以降 emit されなくなるが、 API 互換性のため型としては残す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingToken {
    /// 表層形
    pub surface: String,
    /// カタカナ読み (Smart engine 経路では常に `Some`)
    pub reading: Option<String>,
}

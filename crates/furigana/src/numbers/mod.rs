//! 数値処理 (data-driven)
//!
//! 数値文字列をカタカナ読みに変換するロジック群。
//! 助数詞・スケール・記号・単位のルール表は [`crate::rules`] からロードしたデータを
//! 引数で受け取る (引数は明示的に渡す方針)。 Smart engine 側
//! [`crate::scoring::numbers::NumberCandidateProvider`] から利用される。
//!
//! ## 構成
//! - [`helpers`] (crate-internal): 全角半角・カンマ・末尾数字・促音化等の小関数
//! - [`digit`]   : `number_to_katakana` (純粋アルゴリズム、データ非依存)
//! - [`counter`] : `euphonic_counter_read` (連濁・促音化・kana 末尾置換)
//! - [`extras`]  : スケール / SI 単位 / 記号の単発読み変換
//!
//! ## 公開 API (主なもの)
//! - [`number_to_katakana`]
//! - [`euphonic_counter_read`]
//! - [`scale_reading`] / [`si_unit_reading`] / [`symbol_char_reading`]

mod counter;
mod digit;
mod extras;
pub(crate) mod helpers;

pub use counter::euphonic_counter_read;
pub use digit::number_to_katakana;
pub use extras::{scale_reading, si_unit_reading, symbol_char_reading};

// scoring/numbers.rs から使う internal helper を再 export
pub(crate) use helpers::{kansuji_to_arabic, kansuji_to_arabic_date};

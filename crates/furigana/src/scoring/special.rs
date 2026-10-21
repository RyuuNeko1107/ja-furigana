//! 特殊処理 (cross-cutting) candidate provider 群。
//!
//! - `protect`: 保護トークン (URL / Email / 絵文字 / 顔文字 …)
//! - `alphabet`: 英字の lookup / passthrough と正規化
//!
//! 外部からは従来どおり `scoring::special::*` で使える (下の `pub use`)。
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §5.6
//!
//! ## 含まれる provider
//!
//! - [`ProtectTokenProvider`]: URL / Email / 絵文字 を保護 token として抽出 (band 2000、 必ず勝つ)
//! - 今後追加: アルファベット passthrough / 数字+助数詞 / 漢数字 / 数字読み / 踊り字
//!
//! ## 設計方針
//!
//! 保護トークンは scoring-engine pipeline の **前段** で抽出し、 高 band candidate として
//! `solve_path` に流す。 surface = reading で透過 (= URL や 絵文字をひらがな化しない)、
//! path 選択時に必ず採用される (band 2000 が他の全 band を上回る)。
//!
//! URL_RE / EMAIL_RE は本 module が唯一の実装 (旧 `chunks::regex` の重複実装は
//! alpha.15 の chunks 削除で解消済)。

mod alphabet;
mod protect;

pub use alphabet::{
    normalize_alphabet, AlphabetPassthroughProvider, DIGITS_KEY_PREFIX, DIGITS_SEP,
    SENTENCE_KEY_PREFIX,
};
pub use protect::ProtectTokenProvider;

#[cfg(test)]
mod tests;

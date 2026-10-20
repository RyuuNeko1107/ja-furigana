//! ルールデータ型定義
//!
//! 各 sub module は `data/rules/*.toml` 1 ファイルに対応する Rust 型を提供する。
//! ロード処理は [`crate::loader`] で実装される。
//!
//! 集約型 [`RulesData`] が全ルールをまとめて保持する。

pub mod compat;
pub mod context_cues;
pub mod counters;
pub mod days;
pub mod emoji;
pub mod models;
pub mod numeric_phrases;
pub mod person_name_cues;
pub mod postprocess;
pub mod scales;
pub mod symbols;
pub mod units;

pub use compat::CompatData;
pub use context_cues::{ContextCuesData, PlaceholderCues, SlashPairCues};
pub use counters::{CounterMode, CounterRule, CountersData, EuphonicRule, KanaReplacement};
pub use days::DaysData;
pub use emoji::EmojiData;
pub use models::{ModelSpec, ModelsData};
pub use numeric_phrases::NumericPhrasesData;
pub use person_name_cues::{PersonCueLists, PersonNameCuesData};
pub use postprocess::{PostProcessData, PostProcessRuleSpec, PostProcessSpec};
pub use scales::{ScaleEntry, ScalesData};
pub use symbols::SymbolsData;
pub use units::{UnitEntry, UnitsData};

/// 全ルールを束ねるトップレベル構造体
///
/// 旧 `context` field (= rules/context/*.toml 由来の文脈分岐 reading) は
/// alpha.15 で削除。 context match は dict 側 `[entries."X".match]` /
/// `[[kanji]]` block で表現する設計に移行済 (alpha.11+)。
#[derive(Debug, Default, Clone)]
pub struct RulesData {
    /// 助数詞 (counters.toml)
    pub counters: CountersData,
    /// 1〜31 日の特殊読み (days.toml)
    pub days: DaysData,
    /// 大数スケール: 万/億/兆/京… (scales.toml)
    pub scales: ScalesData,
    /// SI 単位 (units.toml)
    pub units: UnitsData,
    /// 記号読み (symbols.toml)
    pub symbols: SymbolsData,
    /// 型番パターン (models.toml)
    pub models: ModelsData,
    /// 絵文字の読み (emoji.toml)
    pub emoji: EmojiData,
    /// 文脈の手がかり語 (context_cues.toml)
    pub context_cues: ContextCuesData,
    /// 人名の専用経路の手がかり (敬称・肩書き)。 辞書のファイルが正で、 既定値は持たない
    pub person_name_cues: PersonNameCuesData,
    /// 例外語句 (numeric_phrases.toml)
    pub numeric_phrases: NumericPhrasesData,
    /// 異体字マップ (compat_map.toml)
    pub compat: CompatData,
    /// 出力後処理 (postprocess.toml、Step 7 (mode 別後処理 regex))
    pub postprocess: PostProcessData,
}

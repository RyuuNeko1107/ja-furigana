//! 単純な surface → reading 辞書
//!
//! TOML ファイルから読み込む。フォーマット:
//!
//! ```toml
//! [entries]
//! "灰桜" = "ハイザクラ"
//! "黎明" = "レイメイ"
//! ```
//!
//! 起動時に user/core dict ディレクトリ配下の `*.toml` を **全階層再帰** で scan し、
//! `HashMap<String, String>` にマージする (`core/jukugo/general.toml` も
//! `core/works/game/series/touhou.toml` も同じく拾われる)。
//!
//! 優先度の制御は呼び出し側 (Furigana 構造体) で行う想定。
//! Dict 自体は単一階層 — 後に挿入したエントリが先のエントリを上書きする。
//!
//! ## 内部構造 (jukugo / unihan の二段)
//!
//! 内部では surface 文字数で 2 つの HashMap に振り分ける:
//!
//! - **`jukugo`** : surface が 2 文字以上 (= 漢字熟語 / 固有名詞 / 複合語)
//! - **`unihan`** : surface が 1 文字 (= 単漢字フォールバック)
//!
//! [`Self::lookup_jukugo`] と [`Self::lookup_unihan`] で別々に lookup できる。
//! [`Self::lookup`] は両者を試す互換 API (jukugo 優先)。
//!
//! 呼び出し側 ([`crate::reading::pipeline::resolve_reading`]) は
//! `context rule → jukugo lookup → Lindera reading → unihan lookup` の優先順位で評価する。
//! こうすることで、Lindera が動詞活用形 surface に対して持っている自然な reading を
//! 単漢字 unihan の保守的読みが横取りすることがなくなる。

use crate::error::{FuriganaError, Result};
use crate::scoring::format::{Entry, EntryDetail, KanjiBlock};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// TOML ファイルの `[entries]` セクションを受ける defensive な型。
///
/// value を `toml::Value` で受けて、後段で「string 値だけを拾う」フィルタを
/// かける。これにより rules 系ファイル (例: `units.toml` の `[entries]` は
/// `{ kana = "..." }` の inline table) と同じディレクトリに置かれていても
/// silent skip できる。core 辞書と rules を `data/` 1 階層に flat 配置する
/// ユースケース (paths::Paths::dict_core_dir == rules_dir) のための防御。
#[derive(Debug, Default, Deserialize)]
struct DictFile {
    #[serde(default)]
    entries: HashMap<String, toml::Value>,
    /// ★A2 alpha.12: `[[kanji]]` block 配列 (= core/kanji/*.toml の new format)
    /// 単漢字単独 default reading + 文脈分岐を持つ 「first-class candidate generator」
    #[serde(default)]
    kanji: Vec<KanjiBlock>,
}

/// entry value tree (table / array) を再帰走査し、 **非 ASCII の string key**
/// を `(surface, reading)` として収集する (= TOML footgun で detailed entry の
/// match/alt block に吸収された bare simple entry の救済用)。
///
/// matcher 条件 field (`prev_eq` / `next_starts_any` / `reading` 等) は全て ASCII
/// identifier なので `key.is_ascii()` で除外され、 残る非 ASCII key = 吸収された
/// 日本語 surface のみが拾われる。 reading 等の **値** (非 ASCII) は key ではないので
/// 対象外。
fn collect_hoisted_entries(v: &toml::Value, out: &mut Vec<(String, String)>) {
    match v {
        toml::Value::Table(tbl) => {
            for (k, vv) in tbl {
                if !k.is_ascii() {
                    if let Some(s) = vv.as_str() {
                        out.push((k.clone(), s.to_string()));
                    }
                }
                collect_hoisted_entries(vv, out);
            }
        }
        toml::Value::Array(arr) => {
            for vv in arr {
                collect_hoisted_entries(vv, out);
            }
        }
        _ => {}
    }
}

/// 単純 HashMap ベースの surface→reading 辞書
///
/// 内部では surface 文字数で `jukugo` (≥2 文字) と `unihan` (1 文字) を分けて保持。
///
/// alpha.11 の ★A2 wire-up で、 各 entry の **完全 Entry data** (= inline match
/// block 含む) を `rich` field にも保持するようになった。 既存 simple lookup API
/// (`lookup_jukugo` / `lookup_unihan`) は `default_reading` 経由で旧挙動維持、
/// 新 inline match を使う Smart engine 側 logic は別 layer (= scoring engine
/// 内 provider) で `rich` を読む想定。
#[derive(Debug, Default, Clone)]
pub struct Dict {
    /// 熟語・固有名詞・複合語 (surface ≥ 2 文字)、 default reading のみ保持
    jukugo: HashMap<String, String>,
    /// 単漢字フォールバック (surface = 1 文字)、 default reading のみ保持
    unihan: HashMap<String, String>,
    /// 完全 [`Entry`] data (= Simple variant か、 inline / expanded match block 持ち
    /// Detailed variant)。 surface 長で振り分けず全 entry をここに保持、
    /// alpha.11+ で Smart engine が `MatchCondition` 評価に使う想定。
    rich: HashMap<String, Entry>,
    /// `[[kanji]]` block 配列 (★A2 alpha.12、 `core/kanji/*.toml` の新 format)。
    /// 単漢字単独 default + 文脈分岐 reading を持つ first-class candidate generator、
    /// Smart engine から `kanji_iter()` で walk して `MatchCondition` 評価する想定。
    kanji: Vec<KanjiBlock>,
    /// **prefix scan index** (surface 先頭 char → その char で始まる `rich` surface 群)。
    ///
    /// `DictBridgeProvider` が各 byte 位置で 「この位置の文字で始まる entry だけ」 を
    /// 引くための逆引き。 これが無いと全 ~44k entry を毎位置 linear scan して
    /// O(N × M) になる (= 性能 hot path)。 lazy build (= 初回 [`Self::rich_index`]
    /// 呼び出し時に `rich` 全体から構築)、 `insert` / `merge` で invalidate。
    rich_index: OnceLock<HashMap<char, Vec<String>>>,
    /// `[[kanji]]` block の prefix index (char → `kanji` vec 内 index 群)。
    /// rich_index と同趣旨で、 kanji block の毎位置 linear scan を回避する。
    kanji_index: OnceLock<HashMap<char, Vec<usize>>>,
}

impl Dict {
    /// 空辞書を作成
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// TOML 文字列から辞書を構築
    ///
    /// `[entries]` セクション直下の各 entry を取り込む。 後勝ち (同じ surface が
    /// あれば最後のものが採用)、 surface 文字数で内部的に jukugo / unihan に振り分け。
    ///
    /// **新 format 対応** (★A2、 alpha.11): string value (= 旧 Simple 形式) に加え、
    /// inline / expanded sub-table 形式 (= [`EntryDetail`] = `reading` field +
    /// `[[match]]` block 配列) も受け付ける。 Simple は default_reading のみ保持、
    /// Detailed は完全 [`Entry`] を `rich` field に保持して inline match を活用可能に。
    ///
    /// **旧 simple format との互換**: 99% の既存 entry は `"surface" = "reading"`
    /// 1 行 → `Entry::Simple` 経路で従来挙動。 `lookup_jukugo` / `lookup_unihan`
    /// は引き続き default_reading を返す。
    ///
    /// **不一致 value の silent skip**: `[entries]` table に EntryDetail 型でない
    /// inline table (例: `units.toml` の `{ kana = "..." }` 形式) が混在しても
    /// silent skip する (= 同 dir に rules file が混在するケースの defensive 動作)。
    ///
    /// # Errors
    /// TOML 構文エラー / sanitize 失敗時に Err。
    pub fn from_toml_str(content: &str, file: &str) -> Result<Self> {
        // permissive parse: HashMap<String, toml::Value> で受けて、 各 value を
        // Entry に変換可能か個別判定する (= 不一致 value silent skip 維持)。
        let parsed: DictFile = toml::from_str(content).map_err(|e| FuriganaError::Toml {
            file: file.to_string(),
            source: e,
        })?;
        let mut d = Self::default();
        // hoist 候補 (= detailed entry に吸収された bare simple entry) はループ後に
        // **既存 entry を上書きしない形** で適用する。 こうしないと batch に紛れた
        // 重複 (例: detailed `[entries."上手"]` と吸収された `"上手"="ジョウズ"`) で
        // HashMap 反復順依存の非決定的勝者になり、 文脈 match を持つ detailed が
        // 潰れ得る。 explicit entry を常に優先 (= hoist は gap 埋めのみ)。
        let mut pending_hoist: Vec<(String, String)> = Vec::new();
        for (k, v) in parsed.entries {
            // 1) value が string なら Simple Entry
            if let Some(s) = v.as_str() {
                crate::sanitize::sanitize_dict_value("dict surface", &k)
                    .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
                crate::sanitize::sanitize_dict_value("dict reading", s)
                    .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
                d.insert(k.clone(), s.to_string());
                d.rich.insert(k, Entry::Simple(s.to_string()));
                continue;
            }
            // 2) value が table なら EntryDetail として deserialize 試行
            //    rules 系 file の `{ kana = "..." }` のような不一致 inline table は
            //    silent skip (= deserialize 失敗で次の entry に進む)。
            if matches!(v, toml::Value::Table(_)) {
                // ★ TOML footgun 救済: detailed entry sub-table (例 `[entries."流行"]`)
                //   の後ろに書かれた bare simple entry (`"福岡" = "フクオカ"` 等) は
                //   TOML 仕様で直近 table = entry 本体 **または その match/alt block** に
                //   吸収され、 EntryDetail (match block は MatchCondition を flatten) へ
                //   deserialize する際に未知 field として黙って捨てられていた。
                //   entry の value tree を再帰走査し、 **非 ASCII の string key**
                //   (= 条件 field prev_eq/next_starts_any 等は全て ASCII なので surface
                //   のみが残る) を top-level simple entry として hoist 救済する。
                //   これで dict 作者は entry 並び順 (simple/detailed の混在) を
                //   気にせず append できる。 適用はループ後 (= explicit entry 優先)。
                collect_hoisted_entries(&v, &mut pending_hoist);
                let Ok(detail) = v.try_into::<EntryDetail>() else {
                    continue; // 不一致 inline table は skip
                };
                crate::sanitize::sanitize_dict_value("dict surface", &k)
                    .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
                crate::sanitize::sanitize_dict_value("dict default reading", &detail.reading)
                    .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
                for m in &detail.matches {
                    crate::sanitize::sanitize_dict_value("dict match reading", &m.reading)
                        .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
                }
                let default_reading = detail.reading.clone();
                d.insert(k.clone(), default_reading);
                d.rich.insert(k, Entry::Detailed(detail));
                continue;
            }
            // 3) その他 (= bool / array / etc) は silent skip
        }

        // hoist 候補を適用 (= explicit entry を上書きしない gap 埋めのみ)。
        for (surface, reading) in pending_hoist {
            if d.rich.contains_key(&surface) {
                continue; // explicit entry (simple / detailed) が既にある → 優先
            }
            crate::sanitize::sanitize_dict_value("dict surface (hoisted)", &surface)
                .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
            crate::sanitize::sanitize_dict_value("dict reading (hoisted)", &reading)
                .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
            d.insert(surface.clone(), reading.clone());
            d.rich.insert(surface, Entry::Simple(reading));
        }

        // ★A2 alpha.12: `[[kanji]]` block の取り込み (= core/kanji/*.toml 等)
        for block in parsed.kanji {
            // char validate (= 1 字 surface 必須)
            if block.validate().is_err() {
                continue; // invalid block は silent skip (= validate.py で CI 側 reject 想定)
            }
            // sanitize: char (= surface) と default + 各 match reading
            crate::sanitize::sanitize_dict_value("kanji char", &block.char)
                .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
            crate::sanitize::sanitize_dict_value("kanji default reading", &block.default)
                .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
            for m in &block.matches {
                crate::sanitize::sanitize_dict_value("kanji match reading", &m.reading)
                    .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
            }
            // unihan map にも default reading を inject (= Strict engine 後方互換 + 単純 lookup 用)
            d.unihan.insert(block.char.clone(), block.default.clone());
            d.kanji.push(block);
        }
        Ok(d)
    }

    /// 単一 TOML ファイルから辞書を構築
    ///
    /// `[meta] schema_version = "2"` 必須 (alpha.10〜、 ★A1b)。 旧 format (= field 不在
    /// or `"1"`) は [`FuriganaError::Validation`] で reject される。
    ///
    /// # Errors
    /// I/O 失敗 / TOML パース失敗 / schema_version validation 失敗。
    pub fn from_toml_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)?;
        let from = path.display().to_string();
        crate::loader::validate_schema_version(&content, &from)?;
        Self::from_toml_str(&content, &from)
    }

    /// ディレクトリ配下の `*.toml` 全てから辞書をマージ構築
    ///
    /// - **サブディレクトリは無制限に再帰** (例: `core/jukugo/general.toml`、
    ///   `core/works/game/touhou.toml` 等の任意の深さ)
    /// - 全 `*.toml` を集めて絶対パス順でソートし、後に来るファイルが上書きする
    /// - ディレクトリが存在しない場合は空辞書を返す
    /// - 配布 tar.gz から展開した静的データを想定するため、symlink ループ対策は持たない
    ///
    /// **role 駆動 dispatch**: [`crate::loader::resolve_role`] で `[meta] role`
    /// または path-based 推定で role を解決し、 以下の role のみ Dict に load:
    /// - `"jukugo"` (≥2 字 surface)
    /// - `"unihan"` (1 字 surface フォールバック)
    /// - `"works"` (作品造語)
    /// - `"kanji"` (`[[kanji]]` block 形式、 文脈分岐 reading 持ち単漢字)
    /// - role 不明 (= `[meta]` 無し + path 推定不能) → backwards compat で
    ///   Dict に load (古い release との互換性維持)
    ///
    /// 以下の role は **skip** (= 別経路で別データ構造に load、 または alpha.15 で削除済):
    /// - `"loanwords"` → alpha.15 で削除済、 SKIP のまま (0.2.0 で再統合予定)
    /// - `"single_overrides"` → alpha.15 で削除済、 [[kanji]] block で代替
    /// - `"compat"` → rules loader (`load_rules_dir`)
    /// - rules 系 (`"counters"` / `"context"` / 等) も skip
    ///
    /// # Errors
    /// I/O 失敗 / TOML パース失敗。
    pub fn from_toml_dir<P: AsRef<Path>>(dir: P) -> Result<Self> {
        let dir = dir.as_ref();
        if !dir.exists() {
            return Ok(Self::default());
        }
        if !dir.is_dir() {
            return Err(FuriganaError::Validation(format!(
                "dict path is not a directory: {}",
                dir.display()
            )));
        }

        // walk + schema 検証 + role 解決は `for_each_toml_in_dir` が共通担当
        // (★A1b: schema_version = "2" 必須、 alpha.10〜)。
        let mut merged = Self::default();
        crate::loader::for_each_toml_in_dir(dir, |content, from, role| {
            // Dict に load する role 一覧。 role 不明 (None) は backwards compat
            // で Dict として扱う (古い release で role tag が無い file を救う)。
            let load_into_dict = matches!(
                role,
                Some("jukugo") | Some("unihan") | Some("works") | Some("kanji") | None
            );
            if load_into_dict {
                merged.merge(Self::from_toml_str(content, from)?);
            }
            Ok(())
        })?;
        Ok(merged)
    }

    /// surface に対応する読みを返す (jukugo 優先で fallback で unihan を見る、互換 API)
    ///
    /// 新規コードは [`Self::lookup_jukugo`] / [`Self::lookup_unihan`] を分けて
    /// 使い、resolve_reading の優先順位に組み込むのが推奨。
    #[must_use]
    pub fn lookup(&self, surface: &str) -> Option<&str> {
        self.jukugo
            .get(surface)
            .or_else(|| self.unihan.get(surface))
            .map(String::as_str)
    }

    /// 熟語辞書 (surface ≥ 2 文字) のみを lookup
    #[must_use]
    pub fn lookup_jukugo(&self, surface: &str) -> Option<&str> {
        self.jukugo.get(surface).map(String::as_str)
    }

    /// 単漢字辞書 (surface = 1 文字) のみを lookup
    #[must_use]
    pub fn lookup_unihan(&self, surface: &str) -> Option<&str> {
        self.unihan.get(surface).map(String::as_str)
    }

    /// エントリを追加 (既存 surface は上書き)
    ///
    /// surface 文字数で内部的に jukugo / unihan に振り分け、 同時に `rich` にも
    /// `Entry::Simple` で登録する (= ★A2、 lookup_rich でも見える)。
    pub fn insert(&mut self, surface: impl Into<String>, reading: impl Into<String>) {
        let s = surface.into();
        let r = reading.into();
        if s.chars().count() == 1 {
            self.unihan.insert(s.clone(), r.clone());
        } else {
            self.jukugo.insert(s.clone(), r.clone());
        }
        self.rich.insert(s, Entry::Simple(r));
        // rich を変更したので prefix index を invalidate (= 次の lookup で再 build)。
        self.rich_index = OnceLock::new();
    }

    /// 別の Dict を merge (other の方が後勝ち)
    pub fn merge(&mut self, other: Self) {
        self.jukugo.extend(other.jukugo);
        self.unihan.extend(other.unihan);
        self.rich.extend(other.rich);
        // ★A2 alpha.12: [[kanji]] block も merge (= append、 重複 char は両方残るので
        // 「後勝ち」 ではなく order 依存。 同 char の重複は validate.py で reject 想定)
        self.kanji.extend(other.kanji);
        // rich / kanji を変更したので prefix index を invalidate。
        self.rich_index = OnceLock::new();
        self.kanji_index = OnceLock::new();
    }

    /// surface に対応する完全 [`Entry`] を返す (★A2、 alpha.11)。
    ///
    /// Simple variant (= 旧 simple format) なら default reading のみ、 Detailed
    /// variant (= 新 inline match format) なら default + match block 配列を持つ。
    /// Smart engine が `MatchCondition` 評価で文脈分岐 reading を解決する用途。
    #[must_use]
    pub fn lookup_rich(&self, surface: &str) -> Option<&Entry> {
        self.rich.get(surface)
    }

    /// (surface, [`Entry`]) ペアを iter 公開 (★A2、 alpha.11)。
    ///
    /// Smart engine 側 provider が dict 全 entry を walk して inline match を評価
    /// する用途。 `jukugo_iter` と異なり surface 長の制約なし、 Simple / Detailed
    /// 区別なく全 entry を返す。
    pub fn rich_iter(&self) -> impl Iterator<Item = (&str, &Entry)> {
        self.rich.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// `rich` の prefix index を返す (lazy build)。
    ///
    /// surface 先頭 char → その char で始まる surface 群。 [`Self::rich_starting_with`]
    /// が使う内部 helper。
    fn rich_index(&self) -> &HashMap<char, Vec<String>> {
        self.rich_index.get_or_init(|| {
            let mut m: HashMap<char, Vec<String>> = HashMap::new();
            for k in self.rich.keys() {
                if let Some(c) = k.chars().next() {
                    m.entry(c).or_default().push(k.clone());
                }
            }
            m
        })
    }

    /// **先頭 char `c` で始まる `rich` entry のみ** を iterate する (= prefix scan)。
    ///
    /// `DictBridgeProvider` が各 byte 位置で呼ぶ hot path。 全 entry の linear scan
    /// (O(M)) ではなく index 引きした小さな bucket だけを返すので O(k)。
    /// 呼び出し側は更に `surface` 全体の prefix 一致を確認すること (index は先頭
    /// char しか保証しない)。 順序は build 時の `rich` 反復順依存 (= 元の `rich_iter`
    /// と同じく HashMap order、 tie-break は scoring engine が決める)。
    ///
    /// 内部 (DictBridgeProvider) 専用、 公開 API には載せない (= `pub(crate)`)。
    pub(crate) fn rich_starting_with(&self, c: char) -> impl Iterator<Item = (&str, &Entry)> {
        self.rich_index()
            .get(&c)
            .into_iter()
            .flatten()
            .filter_map(move |s| self.rich.get(s).map(|e| (s.as_str(), e)))
    }

    /// `[[kanji]]` block の prefix index を返す (lazy build)。
    fn kanji_index(&self) -> &HashMap<char, Vec<usize>> {
        self.kanji_index.get_or_init(|| {
            let mut m: HashMap<char, Vec<usize>> = HashMap::new();
            for (i, block) in self.kanji.iter().enumerate() {
                if let Some(c) = block.char.chars().next() {
                    m.entry(c).or_default().push(i);
                }
            }
            m
        })
    }

    /// **char `c` の `[[kanji]]` block のみ** を iterate (= prefix scan、 [`Self::kanji_iter`]
    /// の index 引き版)。 block.char は 1 字なので `c` 完全一致と等価。 内部専用 (= `pub(crate)`)。
    pub(crate) fn kanji_starting_with(&self, c: char) -> impl Iterator<Item = &KanjiBlock> {
        self.kanji_index()
            .get(&c)
            .into_iter()
            .flatten()
            .map(move |&i| &self.kanji[i])
    }

    /// [[kanji]] block 配列を iter 公開 (★A2、 alpha.12、 `core/kanji/*.toml` 由来)。
    ///
    /// Smart engine 側 provider が単漢字単位の default + 文脈分岐 reading を
    /// 評価するために walk する。 entries (= rich) と独立して並走、 同 char
    /// surface が両方にある場合は scoring engine path 選択で勝者決定。
    pub fn kanji_iter(&self) -> impl Iterator<Item = &KanjiBlock> {
        self.kanji.iter()
    }

    /// 件数 (jukugo + unihan の合計)
    #[must_use]
    pub fn len(&self) -> usize {
        self.jukugo.len() + self.unihan.len()
    }

    /// 空判定
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.jukugo.is_empty() && self.unihan.is_empty()
    }

    /// 熟語のみの件数 (デバッグ用)
    #[must_use]
    pub fn jukugo_len(&self) -> usize {
        self.jukugo.len()
    }

    /// 単漢字のみの件数 (デバッグ用)
    #[must_use]
    pub fn unihan_len(&self) -> usize {
        self.unihan.len()
    }

    /// 熟語の (surface, reading) ペアを iter 公開
    ///
    /// `chunks::NumberChunker` が起動時に jukugo の Aho-Corasick automaton を
    /// build するために使う (counter chunk が jukugo entry の真部分集合になって
    /// いる場合に jukugo を優先するため)。
    pub fn jukugo_iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.jukugo.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_toml_str_basic() {
        let toml_str = r#"
            [entries]
            "灰桜" = "ハイザクラ"
            "黎明" = "レイメイ"
        "#;
        let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
        assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
        assert_eq!(d.lookup("黎明"), Some("レイメイ"));
        assert_eq!(d.len(), 2);
        // ★A2: rich field にも Simple variant として entry が保持される
        assert!(matches!(d.lookup_rich("灰桜"), Some(Entry::Simple(_))));
        assert_eq!(
            d.lookup_rich("灰桜").unwrap().default_reading(),
            "ハイザクラ"
        );
    }

    // ─── ★A2: 新 format inline match の load test ────────────────────────────

    #[test]
    fn from_toml_str_inline_detailed_form() {
        // inline 形式: "上手" = { reading = "ジョウズ", match = [...] }
        let toml_str = r#"
            [entries]
            "上手" = { reading = "ジョウズ", match = [
              { next_eq = "から", reading = "カミテ" },
            ]}
        "#;
        let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
        // 旧 simple lookup は default_reading が返る
        assert_eq!(d.lookup("上手"), Some("ジョウズ"));
        // rich lookup は Detailed variant
        let entry = d.lookup_rich("上手").expect("rich entry");
        assert!(matches!(entry, Entry::Detailed(_)));
        assert_eq!(entry.default_reading(), "ジョウズ");
        assert_eq!(entry.matches().len(), 1);
        assert_eq!(entry.matches()[0].reading, "カミテ");
        assert_eq!(
            entry.matches()[0].condition.next_eq.as_deref(),
            Some("から")
        );
    }

    #[test]
    fn from_toml_str_expanded_detailed_form() {
        // expanded sub-table 形式 ([entries."x"] と [[entries."x".match]])
        let toml_str = r#"
            [entries."上手"]
            reading = "ジョウズ"

            [[entries."上手".match]]
            next_eq = "から"
            reading = "カミテ"

            [[entries."上手".match]]
            prev_eq = "下"
            reading = "シタテ"
        "#;
        let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
        assert_eq!(d.lookup("上手"), Some("ジョウズ"));
        let entry = d.lookup_rich("上手").expect("rich entry");
        assert_eq!(entry.matches().len(), 2);
        assert_eq!(entry.matches()[0].reading, "カミテ");
        assert_eq!(entry.matches()[1].reading, "シタテ");
        assert_eq!(entry.matches()[1].condition.prev_eq.as_deref(), Some("下"));
    }

    #[test]
    fn from_toml_str_hoists_absorbed_entries() {
        // TOML footgun: detailed entry の match block の後に書かれた bare simple entry は
        // TOML 仕様で match block の table に吸収される。 loader が hoist 救済して
        // top-level simple entry として拾うこと、 かつ detailed entry 本体が壊れないこと。
        let toml_str = r#"
            [entries]
            "霊夢" = "レイム"

            [entries."上手"]
            reading = "ジョウズ"

            [[entries."上手".match]]
            next_eq = "から"
            reading = "カミテ"

            [entries."流行"]
            reading = "リュウコウ"

            [[entries."流行".match]]
            next_starts_any = ["る", "っ"]
            reading = "ハヤ"

            # ↓ これらは TOML 上 entries.流行.match[0] に吸収される
            "完治" = "カンチ"
            "天気" = "テンキ"
            "上手" = "ジョウズ"
        "#;
        let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
        // 吸収されていた simple entry が救済されている
        assert_eq!(d.lookup("完治"), Some("カンチ"));
        assert_eq!(d.lookup("天気"), Some("テンキ"));
        assert!(matches!(d.lookup_rich("完治"), Some(Entry::Simple(_))));
        // detailed entry 本体は壊れていない (match block も保持)
        assert_eq!(d.lookup("流行"), Some("リュウコウ"));
        let ry = d.lookup_rich("流行").expect("流行 rich");
        assert_eq!(ry.matches().len(), 1);
        assert_eq!(ry.matches()[0].reading, "ハヤ");
        // match block の正規 field (reading) は surface として hoist されない
        assert_eq!(d.lookup("ハヤ"), None);
        // 吸収された "上手" は explicit detailed entry を **上書きしない** (= 決定論的、
        // 文脈 match カミテ を保持)
        let kt = d.lookup_rich("上手").expect("上手 rich");
        assert!(
            matches!(kt, Entry::Detailed(_)),
            "detailed 上手 が hoist で潰れた"
        );
        assert_eq!(kt.matches().len(), 1);
        assert_eq!(kt.matches()[0].reading, "カミテ");
    }

    #[test]
    fn from_toml_str_mixed_simple_and_detailed() {
        // 同 file に Simple と Detailed が混在しても OK
        let toml_str = r#"
            [entries]
            "魔理沙" = "マリサ"
            "上手" = { reading = "ジョウズ", match = [
              { next_eq = "から", reading = "カミテ" },
            ]}
            "霊夢" = "レイム"
        "#;
        let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
        assert_eq!(d.len(), 3);
        assert!(matches!(d.lookup_rich("魔理沙"), Some(Entry::Simple(_))));
        assert!(matches!(d.lookup_rich("上手"), Some(Entry::Detailed(_))));
        assert!(matches!(d.lookup_rich("霊夢"), Some(Entry::Simple(_))));
    }

    #[test]
    fn from_toml_str_silently_skips_unrelated_inline_table() {
        // rules 系 file (例: units.toml の `{ kana = "..." }`) と同 dir 混在しても
        // EntryDetail に変換失敗 → silent skip して残り entry は load される
        let toml_str = r#"
            [entries]
            "灰桜" = "ハイザクラ"
            "km" = { kana = "キロメートル" }
            "黎明" = "レイメイ"
        "#;
        let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
        assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
        assert_eq!(d.lookup("黎明"), Some("レイメイ"));
        assert!(
            d.lookup("km").is_none(),
            "rules 系 inline table は silent skip"
        );
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn from_toml_str_empty() {
        let d = Dict::from_toml_str("", "test.toml").unwrap();
        assert!(d.is_empty());
    }

    #[test]
    fn from_toml_str_with_comments() {
        let toml_str = r#"
            # コメント
            [entries]
            "灰桜" = "ハイザクラ"  # inline comment
        "#;
        let d = Dict::from_toml_str(toml_str, "test.toml").unwrap();
        assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
    }

    #[test]
    fn from_toml_str_invalid_errors() {
        let err = Dict::from_toml_str("[invalid", "test.toml").unwrap_err();
        assert!(matches!(err, FuriganaError::Toml { .. }));
    }

    #[test]
    fn merge_overwrites() {
        let mut a =
            Dict::from_toml_str("[entries]\n\"灰桜\" = \"ハイザクラ\"\n", "a.toml").unwrap();
        let b = Dict::from_toml_str(
            "[entries]\n\"灰桜\" = \"カイオウ\"\n\"黎明\" = \"レイメイ\"\n",
            "b.toml",
        )
        .unwrap();
        a.merge(b);
        assert_eq!(a.lookup("灰桜"), Some("カイオウ")); // b が後勝ち
        assert_eq!(a.lookup("黎明"), Some("レイメイ"));
    }

    #[test]
    fn insert_works() {
        let mut d = Dict::new();
        d.insert("灰桜", "ハイザクラ");
        assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
    }

    fn fresh_temp_dir(suffix: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "furigana_dict_test_{}_{}_{}",
            std::process::id(),
            suffix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// `[meta] schema_version = "2"` block を body の先頭に付ける test helper macro。
    /// `concat!` 経由で literal `&'static str` を返す (= `std::fs::write` に直接渡せる)。
    macro_rules! v2 {
        ($body:expr) => {
            concat!("[meta]\nschema_version = \"2\"\n\n", $body)
        };
    }

    #[test]
    fn from_toml_dir_loads_multiple_files() {
        let dir = fresh_temp_dir("dir_load");
        std::fs::write(
            dir.join("01_a.toml"),
            v2!("[entries]\n\"灰桜\" = \"ハイザクラ\"\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("02_b.toml"),
            v2!("[entries]\n\"黎明\" = \"レイメイ\"\n"),
        )
        .unwrap();

        let d = Dict::from_toml_dir(&dir).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
        assert_eq!(d.lookup("黎明"), Some("レイメイ"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn from_toml_dir_filename_order_decides_priority() {
        let dir = fresh_temp_dir("dir_priority");
        std::fs::write(
            dir.join("01_lower.toml"),
            v2!("[entries]\n\"灰桜\" = \"ハイザクラ\"\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("02_higher.toml"),
            v2!("[entries]\n\"灰桜\" = \"カイオウ\"\n"),
        )
        .unwrap();

        let d = Dict::from_toml_dir(&dir).unwrap();
        assert_eq!(d.lookup("灰桜"), Some("カイオウ"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn from_toml_dir_missing_returns_empty() {
        let d = Dict::from_toml_dir("/nonexistent/dir/path/xyz_furigana_test").unwrap();
        assert!(d.is_empty());
    }

    #[test]
    fn from_toml_dir_recurses_one_level_into_subdirs() {
        // jukugo/general.toml + jukugo/places.toml のような構造を扱えること
        let dir = fresh_temp_dir("dir_subdir");
        let sub = dir.join("jukugo");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("general.toml"),
            v2!("[entries]\n\"灰桜\" = \"ハイザクラ\"\n"),
        )
        .unwrap();
        std::fs::write(
            sub.join("places.toml"),
            v2!("[entries]\n\"湯島\" = \"ユシマ\"\n"),
        )
        .unwrap();
        // 直下のファイルもまだ拾えること
        std::fs::write(
            dir.join("top.toml"),
            v2!("[entries]\n\"黎明\" = \"レイメイ\"\n"),
        )
        .unwrap();

        let d = Dict::from_toml_dir(&dir).unwrap();
        assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
        assert_eq!(d.lookup("湯島"), Some("ユシマ"));
        assert_eq!(d.lookup("黎明"), Some("レイメイ"));
        assert_eq!(d.len(), 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn from_toml_dir_recurses_arbitrary_depth() {
        // works/game/touhou.toml のような任意深度の構造を扱えること
        let dir = fresh_temp_dir("dir_deep");
        let deep = dir.join("works").join("game").join("series");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(
            deep.join("touhou.toml"),
            v2!("[entries]\n\"霊夢\" = \"レイム\"\n\"魔理沙\" = \"マリサ\"\n"),
        )
        .unwrap();
        // 別の深い階層
        let deep2 = dir.join("works").join("anime");
        std::fs::create_dir_all(&deep2).unwrap();
        std::fs::write(
            deep2.join("placeholder.toml"),
            v2!("[entries]\n\"宵闇\" = \"ヨイヤミ\"\n"),
        )
        .unwrap();

        let d = Dict::from_toml_dir(&dir).unwrap();
        assert_eq!(d.lookup("霊夢"), Some("レイム"));
        assert_eq!(d.lookup("魔理沙"), Some("マリサ"));
        assert_eq!(d.lookup("宵闇"), Some("ヨイヤミ"));
        assert_eq!(d.len(), 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn from_toml_dir_skips_loanwords_role_via_meta_tag() {
        // role 駆動 dispatch: 同じ dir に jukugo file と loanwords file が混在しても、
        // [meta] role tag で識別して loanwords は Dict に load されないこと
        let dir = fresh_temp_dir("dir_role_loanwords");
        std::fs::write(
            dir.join("jukugo.toml"),
            "[meta]\nschema_version = \"2\"\nrole = \"jukugo\"\n\n[entries]\n\"灰桜\" = \"ハイザクラ\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("loan.toml"),
            "[meta]\nschema_version = \"2\"\nrole = \"loanwords\"\n\n[entries]\n\"Kubernetes\" = \"クバネティス\"\n",
        )
        .unwrap();

        let d = Dict::from_toml_dir(&dir).unwrap();
        assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
        assert_eq!(
            d.lookup("Kubernetes"),
            None,
            "loanwords は role tag で skip"
        );
        assert_eq!(d.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn from_toml_dir_skips_single_overrides_role_via_meta_tag() {
        // single_overrides も role 駆動で識別、 path-based skip に依存しない
        let dir = fresh_temp_dir("dir_role_single");
        std::fs::write(
            dir.join("jukugo.toml"),
            "[meta]\nschema_version = \"2\"\nrole = \"jukugo\"\n\n[entries]\n\"灰桜\" = \"ハイザクラ\"\n",
        )
        .unwrap();
        // 「single_overrides.toml」 という file 名以外で role tag だけで skip されるか
        std::fs::write(
            dir.join("custom_overrides_filename.toml"),
            "[meta]\nschema_version = \"2\"\nrole = \"single_overrides\"\n\n[entries]\n\"土\" = \"ツチ\"\n",
        )
        .unwrap();

        let d = Dict::from_toml_dir(&dir).unwrap();
        assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
        assert_eq!(d.lookup("土"), None, "single_overrides は role tag で skip");
        assert_eq!(d.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn from_toml_dir_path_inference_back_compat() {
        // role tag 無い古い release との互換: path 中の `loanwords/` で skip
        let dir = fresh_temp_dir("dir_path_compat");
        let loan = dir.join("loanwords");
        std::fs::create_dir_all(&loan).unwrap();
        std::fs::write(
            dir.join("jukugo.toml"),
            v2!("[entries]\n\"灰桜\" = \"ハイザクラ\"\n"),
        )
        .unwrap();
        std::fs::write(
            loan.join("it.toml"),
            v2!("[entries]\n\"Kubernetes\" = \"クバネティス\"\n"),
        )
        .unwrap();
        // single_overrides.toml も file 名で skip
        std::fs::write(
            dir.join("single_overrides.toml"),
            v2!("[entries]\n\"土\" = \"ツチ\"\n"),
        )
        .unwrap();

        let d = Dict::from_toml_dir(&dir).unwrap();
        assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
        assert_eq!(
            d.lookup("Kubernetes"),
            None,
            "loanwords/ dir は path 推定で skip"
        );
        assert_eq!(
            d.lookup("土"),
            None,
            "single_overrides.toml は file 名推定で skip"
        );
        assert_eq!(d.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    // ─── A1b: schema_version 強制 tests ──────────────────────────────────────

    #[test]
    fn from_toml_file_rejects_legacy_format_without_meta() {
        // alpha.10〜 dict file も schema_version = "2" を必須化 (★A1b)。
        let dir = fresh_temp_dir("a1b_dict_legacy");
        let path = dir.join("legacy.toml");
        std::fs::write(&path, "[entries]\n\"灰桜\" = \"ハイザクラ\"\n").unwrap();
        let err = Dict::from_toml_file(&path).unwrap_err();
        match err {
            crate::error::FuriganaError::Validation(msg) => {
                assert!(msg.contains("schema_version"), "msg: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn from_toml_dir_rejects_legacy_file() {
        // dir 配下に legacy file が混在する場合、 最初に見つけた legacy で fail
        let dir = fresh_temp_dir("a1b_dict_dir_legacy");
        std::fs::write(
            dir.join("legacy.toml"),
            "[entries]\n\"灰桜\" = \"ハイザクラ\"\n",
        )
        .unwrap();
        let err = Dict::from_toml_dir(&dir).unwrap_err();
        assert!(matches!(err, crate::error::FuriganaError::Validation(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn from_toml_file_accepts_v2_format() {
        let dir = fresh_temp_dir("a1b_dict_v2");
        let path = dir.join("v2.toml");
        std::fs::write(&path, v2!("[entries]\n\"灰桜\" = \"ハイザクラ\"\n")).unwrap();
        let d = Dict::from_toml_file(&path).expect("v2 format should accept");
        assert_eq!(d.lookup("灰桜"), Some("ハイザクラ"));
        std::fs::remove_dir_all(&dir).ok();
    }
}

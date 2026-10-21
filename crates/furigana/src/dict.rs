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
//! `Self::lookup_jukugo` と `Self::lookup_unihan` で別々に lookup できる。
//! `Self::lookup` は両者を試す互換 API (jukugo 優先)。
//!
//! 呼び出し側 (`scoring/dict_bridge.rs` の `DictBridgeProvider`) は
//! `entries (rich) → [[kanji]] block → unihan` の優先順で candidate を emit し、
//! band (jukugo 1000 / unihan 100 / Lindera 50) の lexicographic 比較で
//! Lindera が動詞活用形 surface に持つ自然な reading を単漢字 unihan の
//! 保守的読みが横取りしないようになっている。

mod parse;

use self::parse::collect_hoisted_entries;
use self::parse::parse_simple_entries;
use self::parse::DictFile;
use crate::error::{FuriganaError, Result};
use crate::fxmap::FxMap;
use crate::scoring::format::{Entry, EntryDetail, KanjiBlock};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// 単純 HashMap ベースの surface→reading 辞書
///
/// 内部では surface 文字数で `jukugo` (≥2 文字) と `unihan` (1 文字) を分けて保持。
///
/// alpha.11 の ★A2 wire-up で、 各 entry の **完全 Entry data** (= inline match
/// block 含む) を `rich` field にも保持するようになった。 既存 simple lookup API
/// (`lookup_jukugo` / `lookup_unihan`) は `default_reading` 経由で旧挙動維持、
/// 新 inline match を使う Smart engine 側 logic は別 layer (= scoring engine
/// 内 provider) で `rich` を読む想定。
///
/// **メモリ (2026-09-24)**: 旧実装は同じ entry を `jukugo` (表層 → 読み) / `rich` (表層 → Entry) /
/// `rich_index` (表層のコピー) に 3 重に持ち、 1 件あたり数百 byte 使っていた。 `jukugo` は `rich` と
/// 常に同内容だったので廃止して `rich` から引き、 `rich` のキーは `Arc<str>` にして index と共有する。
/// 先頭 char が同じ rich surface 群の索引 ([`Dict::rich_matching_prefix`] 用)。
///
/// 位置ごとに引くので、 二分探索 (毎回 `Arc<str>` の指す先を辿る) ではなく、
/// 「先頭 char → 2 文字目 → surface 群」 の 2 段のハッシュ引きにしてある。
#[derive(Debug, Default, Clone)]
struct PrefixBucket {
    /// 1 字 surface (= 先頭 char そのもの)
    single: Option<Arc<str>>,
    /// 2 文字目が同じ surface 群 (2 字以上の surface。 surface 昇順)
    by_second: FxMap<char, Vec<Arc<str>>>,
}

#[derive(Debug, Default, Clone)]
pub struct Dict {
    /// 単漢字フォールバック (surface = 1 文字)、 default reading のみ保持
    /// (`[[kanji]]` block の default もここに入るので `rich` とは別に持つ)
    unihan: FxMap<String, String>,
    /// 完全 [`Entry`] data (= Simple variant か、 inline / expanded match block 持ち
    /// Detailed variant)。 surface 長で振り分けず全 entry をここに保持。
    /// 熟語 (surface ≥ 2 文字) の default reading もここから引く (旧 `jukugo` map)。
    rich: FxMap<Arc<str>, Entry>,
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
    rich_index: OnceLock<FxMap<char, PrefixBucket>>,
    /// `[[kanji]]` block の prefix index (char → `kanji` vec 内 index 群)。
    /// rich_index と同趣旨で、 kanji block の毎位置 linear scan を回避する。
    kanji_index: OnceLock<FxMap<char, Vec<usize>>>,
    /// 人名の専用経路の表 (`role = "name_sei"` / `"name_mei"`)。 表記 → 読み (カタカナ)。
    /// 熟語辞書とは別に持ち、 [`crate::scoring::person_names::PersonNamePass`] だけが引く
    /// (一般語と衝突しないよう、 通常の辞書引きには混ぜない)。
    name_sei: HashMap<String, String>,
    name_mei: HashMap<String, String>,
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
    /// inline / expanded sub-table 形式 (= `EntryDetail` = `reading` field +
    /// `[[match]]` block 配列) も受け付ける。 Simple は default_reading のみ保持、
    /// Detailed は完全 `Entry` を `rich` field に保持して inline match を活用可能に。
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
        // 高速経路: `[meta]` + `[entries]` に `"表層" = "読み"` の行しか無い file (大多数) は
        // toml の値の木を作らずに 1 行ずつ読む (21 万件級の蒸留語彙で読み込み中のピークが大きかった)。
        // 少しでも外れる書き方 (detailed entry / [[kanji]] / エスケープ / 裸キー 等) があれば従来の toml 経路
        if let Some(pairs) = parse_simple_entries(content) {
            let mut d = Self::default();
            for (k, s) in pairs {
                crate::sanitize::sanitize_dict_value("dict surface", k)
                    .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
                crate::sanitize::sanitize_dict_value("dict reading", s)
                    .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
                d.insert(k, s);
            }
            return Ok(d);
        }
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
                d.insert(k, s.to_string());
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
                d.rich
                    .insert(Arc::from(k), Entry::Detailed(Box::new(detail)));
                continue;
            }
            // 3) その他 (= bool / array / etc) は silent skip
        }

        // hoist 候補を適用 (= explicit entry を上書きしない gap 埋めのみ)。
        for (surface, reading) in pending_hoist {
            if d.rich.contains_key(surface.as_str()) {
                continue; // explicit entry (simple / detailed) が既にある → 優先
            }
            crate::sanitize::sanitize_dict_value("dict surface (hoisted)", &surface)
                .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
            crate::sanitize::sanitize_dict_value("dict reading (hoisted)", &reading)
                .map_err(|e| FuriganaError::Validation(format!("{file}: {e}")))?;
            d.insert(surface, reading);
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
    /// - `"single_overrides"` → alpha.15 で削除済、 `[[kanji]]` block で代替
    /// - `"compat"` → rules loader (`load_rules_dir`)
    /// - rules 系 (`"counters"` / `"context"` / 等) も skip
    ///
    /// # Errors
    /// I/O 失敗 / TOML パース失敗。
    pub fn from_toml_dir<P: AsRef<Path>>(dir: P) -> Result<Self> {
        Self::from_toml_dir_excluding(dir.as_ref(), &[])
    }

    /// [`Self::from_toml_dir`] の除外付き版 (`excludes` は dir からの相対 path、
    /// [`crate::FuriganaBuilder::exclude_dict_path`] 参照)
    pub(crate) fn from_toml_dir_excluding(dir: &Path, excludes: &[String]) -> Result<Self> {
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
        // Dict が取り込む role (と人名の表) だけ全体を読む。 アクセントの表など他の role は先頭だけ見て飛ばす
        let wants = |role: Option<&str>| {
            matches!(
                role,
                Some("jukugo" | "unihan" | "works" | "kanji" | "name_sei" | "name_mei") | None
            )
        };
        crate::loader::for_each_toml_in_dir_roles(dir, excludes, wants, |content, from, role| {
            // Dict に load する role 一覧。 role 不明 (None) は backwards compat
            // で Dict として扱う (古い release で role tag が無い file を救う)。
            let load_into_dict = matches!(
                role,
                Some("jukugo") | Some("unihan") | Some("works") | Some("kanji") | None
            );
            if load_into_dict {
                merged.merge(Self::from_toml_str(content, from)?);
            }
            match role {
                Some("name_sei") => merged.load_name_table(content, true),
                Some("name_mei") => merged.load_name_table(content, false),
                _ => {}
            }
            Ok(())
        })?;
        Ok(merged)
    }

    /// 人名表 (`[entries]` の `"表記" = "読み"` 行) を行単位で取り込む。 読みはカタカナにそろえる。
    /// 10 万件規模なので serde で全体を展開しない。 同じ表記は先勝ち (表は多数派の読み順に並べる)。
    fn load_name_table(&mut self, content: &str, sei: bool) {
        let map = if sei {
            &mut self.name_sei
        } else {
            &mut self.name_mei
        };
        for (surface, reading) in crate::scoring::person_names::parse_name_table(content) {
            map.entry(surface)
                .or_insert_with(|| crate::kana::hira_to_kata(&reading));
        }
    }

    /// 姓の表で表記の読み (カタカナ) を引く。 人名の専用経路だけが使う
    #[must_use]
    pub(crate) fn lookup_name_sei(&self, surface: &str) -> Option<&str> {
        self.name_sei.get(surface).map(String::as_str)
    }

    /// 名の表で表記の読み (カタカナ) を引く
    #[must_use]
    pub(crate) fn lookup_name_mei(&self, surface: &str) -> Option<&str> {
        self.name_mei.get(surface).map(String::as_str)
    }

    /// 人名表を (表記, 読み) の組で足す (テスト用)
    #[cfg(test)]
    pub(crate) fn insert_name(&mut self, sei: bool, surface: &str, reading: &str) {
        let map = if sei {
            &mut self.name_sei
        } else {
            &mut self.name_mei
        };
        map.insert(surface.to_string(), crate::kana::hira_to_kata(reading));
    }

    /// surface に対応する読みを返す (jukugo 優先で fallback で unihan を見る、互換 API)
    ///
    /// 新規コードは [`Self::lookup_jukugo`] / [`Self::lookup_unihan`] を分けて
    /// 使い、`DictBridgeProvider` の emit 優先順位に組み込むのが推奨。
    #[must_use]
    pub fn lookup(&self, surface: &str) -> Option<&str> {
        self.lookup_jukugo(surface)
            .or_else(|| self.unihan.get(surface).map(String::as_str))
    }

    /// 熟語辞書 (surface ≥ 2 文字) のみを lookup
    #[must_use]
    pub fn lookup_jukugo(&self, surface: &str) -> Option<&str> {
        // 旧 `jukugo` map = rich のうち surface ≥ 2 文字の default reading と同内容
        let mut cs = surface.chars();
        if cs.next().is_none() || cs.next().is_none() {
            return None;
        }
        self.rich.get(surface).map(Entry::default_reading)
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
        // バルク TOML load (from_toml_str) と同じ per-value sanitize を runtime 追加にも
        // 適用する (経路によらず 1 件の追加を bounded に保つ)。上限超過 / 禁止文字は
        // OOM / Trojan-Source 経路なので、 壊れた 1 件は静かに skip する
        // (他経路の不正 entry skip と一貫、 公開 add_reading は infallible 維持)。
        if let Err(e) = crate::sanitize::sanitize_dict_value("dict surface (insert)", &s) {
            tracing::debug!("add_reading skip (surface): {e}");
            return;
        }
        if let Err(e) = crate::sanitize::sanitize_dict_value("dict reading (insert)", &r) {
            tracing::debug!("add_reading skip (reading): {e}");
            return;
        }
        if s.chars().count() == 1 {
            self.unihan.insert(s.clone(), r.clone());
        }
        self.rich
            .insert(Arc::from(s), Entry::Simple(r.into_boxed_str()));
        // rich を変更したので prefix index を invalidate (= 次の lookup で再 build)。
        self.rich_index = OnceLock::new();
    }

    /// 別の Dict を merge (other の方が後勝ち)
    pub fn merge(&mut self, other: Self) {
        self.unihan.extend(other.unihan);
        self.rich.extend(other.rich);
        // ★A2 alpha.12: `[[kanji]]` block も merge (= append、 重複 char は両方残るので
        // 「後勝ち」 ではなく order 依存。 同 char の重複は validate.py で reject 想定)
        self.kanji.extend(other.kanji);
        self.name_sei.extend(other.name_sei);
        self.name_mei.extend(other.name_mei);
        // rich / kanji を変更したので prefix index を invalidate。
        self.rich_index = OnceLock::new();
        self.kanji_index = OnceLock::new();
    }

    /// surface に対応する完全 `Entry` を返す (★A2、 alpha.11)。
    ///
    /// Simple variant (= 旧 simple format) なら default reading のみ、 Detailed
    /// variant (= 新 inline match format) なら default + match block 配列を持つ。
    /// Smart engine が `MatchCondition` 評価で文脈分岐 reading を解決する用途。
    #[must_use]
    pub fn lookup_rich(&self, surface: &str) -> Option<&Entry> {
        self.rich.get(surface)
    }

    /// (surface, `Entry`) ペアを iter 公開 (★A2、 alpha.11)。
    ///
    /// Smart engine 側 provider が dict 全 entry を walk して inline match を評価
    /// する用途。 `jukugo_iter` と異なり surface 長の制約なし、 Simple / Detailed
    /// 区別なく全 entry を返す。
    pub fn rich_iter(&self) -> impl Iterator<Item = (&str, &Entry)> {
        self.rich.iter().map(|(k, v)| (&**k, v))
    }

    /// `rich` の prefix index を返す (lazy build)。
    ///
    /// surface 先頭 char → その char で始まる surface 群。 [`Self::rich_matching_prefix`]
    /// が使う内部 helper。
    fn rich_index(&self) -> &FxMap<char, PrefixBucket> {
        self.rich_index.get_or_init(|| {
            let mut m: FxMap<char, PrefixBucket> = FxMap::default();
            for k in self.rich.keys() {
                let mut cs = k.chars();
                let Some(c1) = cs.next() else { continue };
                let bucket = m.entry(c1).or_default();
                match cs.next() {
                    None => bucket.single = Some(k.clone()),
                    Some(c2) => bucket.by_second.entry(c2).or_default().push(k.clone()),
                }
            }
            // 各群を surface 昇順に sort して列挙順を決定的にする。
            // (HashMap の反復順に依存すると、 同 score 候補の tie-break (= 第一発見勝ち、 engine.rs
            // strict-greater) や analyze() の同 weight alternatives 順序が揺れうる。
            // production の tokens() 採択読みは 「同 range の真の同点 = 同一 surface =
            // 同一 HashMap key = 単一」 ゆえ元から不変だが、 列挙を決定的にして
            // deterministic-runtime 契約 (analyze/accent JSON 含む) を明示的に守る。)
            for bucket in m.values_mut() {
                for group in bucket.by_second.values_mut() {
                    group.sort_unstable();
                }
            }
            m
        })
    }

    /// `tail` の **接頭辞になりうる rich entry だけ** を列挙する。
    ///
    /// bucket は surface 昇順 sort 済みなので、 「先頭 2 文字が一致する surface 群」 は
    /// 連続区間になる。 `partition_point` 2 回でその区間を取り、 1 字 surface
    /// (= 先頭 char そのもの。 昇順では区間より前に来る) を前置して返す。
    ///
    /// これにより 「御」 (bucket 713 件) のような巨大 bucket でも、 実際に 2 文字目まで
    /// 一致する数件しか走査しない。 列挙順は surface 昇順なので
    /// tie-break / alternatives 順序の determinism 契約は変わらない。
    pub(crate) fn rich_matching_prefix(&self, tail: &str) -> impl Iterator<Item = (&str, &Entry)> {
        let mut chars = tail.chars();
        let first = chars.next();
        let bucket = first.and_then(|c| self.rich_index().get(&c));

        // 1 字 surface (= 先頭 char 単体) が先、 続けて 先頭 2 文字が一致する surface 群 (昇順)。
        // tail が 1 字なら 1 字 surface のみ対象。
        let single = bucket.and_then(|b| b.single.as_ref());
        let range: &[Arc<str>] = match (bucket, chars.next()) {
            (Some(b), Some(c2)) => b.by_second.get(&c2).map_or(&[], Vec::as_slice),
            _ => &[],
        };

        single
            .into_iter()
            .chain(range.iter())
            .filter_map(move |s| self.rich.get(&**s).map(|e| (&**s, e)))
    }

    /// `[[kanji]]` block の prefix index を返す (lazy build)。
    fn kanji_index(&self) -> &FxMap<char, Vec<usize>> {
        self.kanji_index.get_or_init(|| {
            let mut m: FxMap<char, Vec<usize>> = FxMap::default();
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

    /// `[[kanji]]` block 配列を iter 公開 (★A2、 alpha.12、 `core/kanji/*.toml` 由来)。
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
        self.jukugo_len() + self.unihan.len()
    }

    /// 空判定
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.jukugo_len() == 0 && self.unihan.is_empty()
    }

    /// 熟語のみの件数 (デバッグ用)
    #[must_use]
    pub fn jukugo_len(&self) -> usize {
        self.rich
            .keys()
            .filter(|k| k.chars().nth(1).is_some())
            .count()
    }

    /// 単漢字のみの件数 (デバッグ用)
    #[must_use]
    pub fn unihan_len(&self) -> usize {
        self.unihan.len()
    }

    /// 熟語の (surface, reading) ペアを iter 公開
    ///
    /// 旧 `chunks::NumberChunker` (alpha.15 で削除済) が使っていた公開 helper。
    /// 現在 lib 内部に caller は無いが、 dict 内容の inspection 用 public API として残置。
    pub fn jukugo_iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.rich
            .iter()
            .filter(|(k, _)| k.chars().nth(1).is_some())
            .map(|(k, v)| (&**k, v.default_reading()))
    }
}

#[cfg(test)]
mod tests;

//! 解析結果から OpenJTalk 方式のアクセント句列を作る (連結規則の結線)。
//!
//! 流れ:
//! 1. 入力全体を Lindera (IPADIC) で形態素解析して品詞を得る (文脈つき。 「で」 のような語の品詞が決まる)
//! 2. こちらの採択 token を形態素に重ねる。 token の読みが形態素の読みの連結と同じなら **形態素ごとのノード** に展開
//!    (OpenJTalk の切り方に近づく)、 違う (辞書で読みを直した語) なら token 全体を 1 ノードにする
//! 3. ノードごとに 語単体の核 / 拍数 (アクセント表) と 結合型 (付属語の表 + 内容語の既定 + 例外の表) を付ける
//! 4. [`crate::accent_chain::build_phrases`] で句を組み立てる
//!
//! 表は dict の `role = "accent_chain"` (`tools/gen_accent_chain.py` が naist-jdic から生成)。

use crate::accent_chain::{build_phrases, ChainRules, MeishiKind, Node, Pos, Sub};
use crate::analyzer::MorphToken;
use crate::scoring::accent_lexicon::AccentLexicon;
use crate::scoring::analyze::Token;
use std::collections::HashMap;
use std::path::Path;

/// 組み立てたアクセント句 1 つ。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChainedPhrase {
    /// 句の表記 (入力の部分文字列)
    pub surface: String,
    /// 句の読み (カタカナ)
    pub reading: String,
    /// 拍数
    pub mora: usize,
    /// 核位置 (0 = 平板、 n = n 拍目で下がる)
    pub accent: usize,
    /// 核に推定が混ざっているか (語単体の核が表に無く 平板で埋めた / rule 推定の核を使った)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub estimated: bool,
    /// 発音表記の読み (助詞の は / へ / を を ワ / エ / オ にしたもの。 TTS の記号列用)
    #[serde(skip)]
    pub sound: String,
    /// この句の前にあった、 読みの無い token (句読点・空白・記号) の表記。 句の切れ目の強さの元
    #[serde(skip)]
    pub gap_before: String,
}

/// 分割表の部品 1 つ (OpenJTalk の形態素)。
#[derive(Debug, Clone)]
pub(crate) struct SplitPart {
    surface: Box<str>,
    reading: Box<str>,
    pos: Pos,
    renyou: bool,
    accent: u8,
    mora: u8,
    rules: ChainRules,
    rule_id: u8,
    /// 品詞の文字列 (診断用。 「品詞1,品詞2」)
    pos_label: Box<str>,
}

/// 付属語・内容語の結合型の表。
#[derive(Debug, Default)]
pub(crate) struct ChainTable {
    /// 内容語の既定 (「品詞1\u{1}品詞2」 → 規則)
    defaults: Vec<(Box<str>, Box<str>, u8)>,
    /// 付属語・接尾・非自立 (key = 「表記\u{1}読み\u{1}品詞1\u{1}品詞2」) → (核, 拍数, 規則)。 key 昇順
    functional: Vec<(u64, u8, u8, u8)>,
    /// 内容語の例外 (key は同じ形) → 規則。 key 昇順
    exceptions: Vec<(u64, u8)>,
    /// 語単体の核 (key = 表記 / 読み / 品詞1 のハッシュ) → (核, 拍数)。 アクセント表に無い語 (活用形・かな語・外来語 等)。 key 昇順
    words: Vec<(u64, u8, u8)>,
    /// 分割表: Lindera が 1 語にして OpenJTalk が複数語にする語 (助詞連語など)。 key = (表記, 品詞1, 品詞2) のハッシュ。 key 昇順
    splits: Vec<(u64, Box<[SplitPart]>)>,
    /// 規則の実体 (重複を畳んだ数十件)
    pool: Vec<ChainRules>,
    pool_index: HashMap<String, u8>,
}

fn quoted(s: &str) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut it = s.char_indices();
    if it.next()?.1 != '"' {
        return None;
    }
    let mut esc = false;
    for (i, c) in it {
        if esc {
            out.push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == '"' {
            return Some((out, i + 1));
        } else {
            out.push(c);
        }
    }
    None
}

impl ChainTable {
    /// バイナリキャッシュへ書く (`tablecache`)
    pub(crate) fn write_to(&self, w: &mut crate::tablecache::Writer) {
        // 規則の実体: id 順の文字列
        let mut by_id = vec![String::new(); self.pool.len()];
        for (rule, &id) in &self.pool_index {
            if let Some(slot) = by_id.get_mut(usize::from(id)) {
                slot.clone_from(rule);
            }
        }
        w.u64(by_id.len() as u64);
        for r in &by_id {
            w.str(r);
        }
        w.u64(self.defaults.len() as u64);
        for d in &self.defaults {
            w.str(&d.0);
            w.str(&d.1);
            w.u8(d.2);
        }
        w.u64(self.functional.len() as u64);
        for e in &self.functional {
            w.u64(e.0);
            w.u8(e.1);
            w.u8(e.2);
            w.u8(e.3);
        }
        w.u64(self.exceptions.len() as u64);
        for e in &self.exceptions {
            w.u64(e.0);
            w.u8(e.1);
        }
        w.u64(self.words.len() as u64);
        for e in &self.words {
            w.u64(e.0);
            w.u8(e.1);
            w.u8(e.2);
        }
        w.u64(self.splits.len() as u64);
        for (key, parts) in &self.splits {
            w.u64(*key);
            w.u64(parts.len() as u64);
            for p in parts {
                w.str(&p.surface);
                w.str(&p.reading);
                w.str(&p.pos_label);
                w.u8(u8::from(p.renyou));
                w.u8(p.accent);
                w.u8(p.mora);
                w.u8(p.rule_id);
            }
        }
    }

    /// バイナリキャッシュから読む。 壊れていれば `None`
    pub(crate) fn read_from(r: &mut crate::tablecache::Reader) -> Option<Self> {
        let mut t = Self::default();
        let n = r.len(4)?;
        for _ in 0..n {
            let rule = r.str()?;
            // 規則の文字列を id 順に積み直す (id は書き出し時と同じ並びになる)
            let id = u8::try_from(t.pool.len()).ok()?;
            t.pool.push(ChainRules::parse(rule));
            t.pool_index.insert(rule.to_string(), id);
        }
        let n = r.len(9)?;
        for _ in 0..n {
            let p1 = r.str()?;
            let p2 = r.str()?;
            t.defaults.push((p1.into(), p2.into(), r.u8()?));
        }
        let n = r.len(11)?;
        t.functional.reserve_exact(n);
        for _ in 0..n {
            t.functional.push((r.u64()?, r.u8()?, r.u8()?, r.u8()?));
        }
        let n = r.len(9)?;
        t.exceptions.reserve_exact(n);
        for _ in 0..n {
            t.exceptions.push((r.u64()?, r.u8()?));
        }
        let n = r.len(10)?;
        t.words.reserve_exact(n);
        for _ in 0..n {
            t.words.push((r.u64()?, r.u8()?, r.u8()?));
        }
        let n = r.len(16)?;
        t.splits.reserve_exact(n);
        for _ in 0..n {
            let key = r.u64()?;
            let m = r.len(12)?;
            let mut parts = Vec::with_capacity(m);
            for _ in 0..m {
                let surface = r.str()?;
                let reading = r.str()?;
                let label = r.str()?;
                let (renyou, accent, mora, rule_id) = (r.u8()? == 1, r.u8()?, r.u8()?, r.u8()?);
                let (p1, p2) = label.split_once(',').unwrap_or((label, "*"));
                parts.push(SplitPart {
                    surface: surface.into(),
                    reading: reading.into(),
                    pos: pos_of(p1, p2),
                    renyou,
                    accent,
                    mora,
                    rules: *t.pool.get(usize::from(rule_id))?,
                    rule_id,
                    pos_label: label.into(),
                });
            }
            t.splits.push((key, parts.into_boxed_slice()));
        }
        Some(t)
    }

    /// 部品 「表記,読み,品詞1,品詞2,連用(0/1),核/拍数,結合型」 を読む
    fn parse_part(&mut self, p: &str) -> Option<SplitPart> {
        let f: Vec<&str> = p.splitn(7, ',').collect();
        if f.len() < 7 {
            return None;
        }
        let (a, m) = f[5].split_once('/')?;
        let id = self.rule_id(f[6]);
        let (p1, p2) = (f[2], f[3]);
        let pos = pos_of(p1, p2);
        Some(SplitPart {
            surface: f[0].into(),
            reading: f[1].into(),
            pos,
            renyou: f[4] == "1",
            accent: a.parse().ok()?,
            mora: m.parse().ok()?,
            rules: self.pool[usize::from(id)],
            rule_id: id,
            pos_label: format!("{p1},{p2}").into(),
        })
    }

    fn rule_id(&mut self, rule: &str) -> u8 {
        if let Some(&i) = self.pool_index.get(rule) {
            return i;
        }
        let i = u8::try_from(self.pool.len()).unwrap_or(u8::MAX);
        if self.pool.len() < usize::from(u8::MAX) {
            self.pool.push(ChainRules::parse(rule));
            self.pool_index.insert(rule.to_string(), i);
        }
        i
    }

    /// 「表記,読み,品詞1,品詞2」 → key のハッシュ
    fn key_of(csv_key: &str) -> Option<u64> {
        let mut it = csv_key.splitn(4, ',');
        let (a, b, c, d) = (it.next()?, it.next()?, it.next()?, it.next()?);
        Some(crate::keyhash::hash_parts(&[a, b, c, d]))
    }

    fn load_str(&mut self, content: &str) {
        let mut section = "";
        for line in content.lines() {
            let t = line.trim();
            if t.starts_with('[')
                && !t.starts_with("[[")
                && t.ends_with(']')
                && !t.starts_with("[\"")
            {
                section = match t {
                    "[defaults]" => "defaults",
                    "[functional]" => "functional",
                    "[exceptions]" => "exceptions",
                    "[words]" => "words",
                    "[splits]" => "splits",
                    _ => "",
                };
                continue;
            }
            let Some((key, end)) = quoted(t) else {
                continue;
            };
            let Some(rest) = t[end..].trim_start().strip_prefix('=') else {
                continue;
            };
            let Some((value, _)) = quoted(rest.trim_start()) else {
                continue;
            };
            match section {
                "defaults" => {
                    if let Some((p1, p2)) = key.split_once(',') {
                        let id = self.rule_id(&value);
                        self.defaults.push((p1.into(), p2.into(), id));
                    }
                }
                "functional" => {
                    let Some(k) = Self::key_of(&key) else {
                        continue;
                    };
                    let Some((am, rule)) = value.split_once('|') else {
                        continue;
                    };
                    let Some((a, m)) = am.split_once('/') else {
                        continue;
                    };
                    let (Ok(a), Ok(m)) = (a.parse::<u8>(), m.parse::<u8>()) else {
                        continue;
                    };
                    let id = self.rule_id(rule);
                    self.functional.push((k, a, m, id));
                }
                "words" => {
                    let mut it = key.splitn(3, ',');
                    let (Some(a), Some(b), Some(c)) = (it.next(), it.next(), it.next()) else {
                        continue;
                    };
                    let Some((n, m)) = value.split_once('/') else {
                        continue;
                    };
                    let (Ok(n), Ok(m)) = (n.parse::<u8>(), m.parse::<u8>()) else {
                        continue;
                    };
                    self.words
                        .push((crate::keyhash::hash_parts(&[a, b, c]), n, m));
                }
                "splits" => {
                    let mut it = key.splitn(3, ',');
                    let (Some(a), Some(b), Some(c)) = (it.next(), it.next(), it.next()) else {
                        continue;
                    };
                    let parts: Vec<SplitPart> = value
                        .split(';')
                        .filter_map(|p| self.parse_part(p))
                        .collect();
                    if parts.len() >= 2 {
                        self.splits.push((
                            crate::keyhash::hash_parts(&[a, b, c]),
                            parts.into_boxed_slice(),
                        ));
                    }
                }
                "exceptions" => {
                    let Some(k) = Self::key_of(&key) else {
                        continue;
                    };
                    let id = self.rule_id(&value);
                    self.exceptions.push((k, id));
                }
                _ => {}
            }
        }
    }

    fn finish(&mut self) {
        self.functional.sort_by_key(|e| e.0);
        self.functional.dedup_by(|b, a| a.0 == b.0);
        self.functional.shrink_to_fit();
        self.splits.sort_by_key(|e| e.0);
        self.splits.dedup_by(|b, a| a.0 == b.0);
        self.splits.shrink_to_fit();
        self.words.sort_by_key(|e| e.0);
        self.words.dedup_by(|b, a| a.0 == b.0);
        self.words.shrink_to_fit();
        self.exceptions.sort_by_key(|e| e.0);
        self.exceptions.dedup_by(|b, a| a.0 == b.0);
        self.exceptions.shrink_to_fit();
    }

    /// `dir` 配下の `role = "accent_chain"` の TOML を取り込む
    pub(crate) fn load_dir(&mut self, dir: &Path, excludes: &[String]) -> crate::error::Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        crate::loader::for_each_toml_in_dir_excluding(dir, excludes, |content, _from, role| {
            if role == Some("accent_chain") && crate::tablecache::is_supported_table(content) {
                self.load_str(content);
            }
            Ok(())
        })?;
        self.finish();
        Ok(())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.functional.is_empty() && self.defaults.is_empty()
    }

    fn functional_get(&self, key: u64) -> Option<(u8, u8, ChainRules)> {
        self.functional
            .binary_search_by_key(&key, |e| e.0)
            .ok()
            .map(|i| {
                (
                    self.functional[i].1,
                    self.functional[i].2,
                    self.pool[usize::from(self.functional[i].3)],
                )
            })
    }

    /// 分割表 (Lindera の 1 語 → OpenJTalk の複数語)。 `key` = (表記, 品詞1, 品詞2) のハッシュ
    fn split_get(&self, key: u64) -> Option<&[SplitPart]> {
        self.splits
            .binary_search_by_key(&key, |e| e.0)
            .ok()
            .map(|i| &*self.splits[i].1)
    }

    /// 語単体の核 (アクセント表に無い語用)。 `key` = (表記, 読み, 品詞1) のハッシュ
    fn word_get(&self, key: u64) -> Option<(u8, u8)> {
        self.words
            .binary_search_by_key(&key, |e| e.0)
            .ok()
            .map(|i| (self.words[i].1, self.words[i].2))
    }

    /// 内容語の結合型。 `key` = (表記, 読み, 品詞1, 品詞2) のハッシュ
    fn rules_for_key(&self, key: u64, p1: &str, p2: &str) -> ChainRules {
        if let Ok(i) = self.exceptions.binary_search_by_key(&key, |e| e.0) {
            return self.pool[usize::from(self.exceptions[i].1)];
        }
        self.defaults
            .iter()
            .find(|d| &*d.0 == p1 && &*d.1 == p2)
            .map_or_else(ChainRules::default, |d| self.pool[usize::from(d.2)])
    }
}

/// 品詞1 / 品詞2 の文字列 → 連結規則の品詞 (分割表の部品用)
fn pos_of(p1: &str, d1: &str) -> Pos {
    let sub = |d: &str| match d {
        "非自立" => Sub::Hijiritsu,
        "接尾" => Sub::Setsubi,
        _ => Sub::Jiritsu,
    };
    match p1 {
        "名詞" => Pos::Meishi(match d1 {
            "接尾" => MeishiKind::Setsubi,
            "形容動詞語幹" => MeishiKind::KeiyoudoushiGokan,
            "副詞可能" => MeishiKind::FukushiKanou,
            "数" => MeishiKind::Kazu,
            _ => MeishiKind::General,
        }),
        "動詞" => Pos::Doushi(sub(d1)),
        "形容詞" => Pos::Keiyoushi(sub(d1)),
        "助詞" => Pos::Joshi(d1 == "接続助詞"),
        "助動詞" => Pos::Jodoushi,
        "副詞" => Pos::Fukushi,
        "接続詞" => Pos::Setsuzokushi,
        "連体詞" => Pos::Rentaishi,
        "接頭詞" => Pos::Settoushi,
        "記号" => Pos::Kigou(d1 == "数"),
        _ => Pos::Other,
    }
}

/// IPADIC の品詞 → 連結規則の品詞。 戻り値の bool = 連用形
fn map_pos(m: &MorphToken) -> (Pos, bool) {
    let p1 = m.pos.as_deref().unwrap_or("");
    let d1 = m.pos_detail.as_deref().unwrap_or("");
    let d2 = m.pos_detail2.as_deref().unwrap_or("");
    let d3 = m.pos_detail3.as_deref().unwrap_or("");
    let renyou = m
        .conjugation_form
        .as_deref()
        .is_some_and(|f| f.starts_with("連用"));
    let pos = match (p1, d1, d2, d3) {
        ("名詞", "固有名詞", "人名", "姓") => Pos::Meishi(MeishiKind::Sei),
        ("名詞", "固有名詞", "人名", "名") => Pos::Meishi(MeishiKind::Mei),
        _ => pos_of(p1, d1),
    };
    (pos, renyou)
}

fn is_kana_char(c: char) -> bool {
    matches!(c, 'ァ'..='ヶ' | 'ー')
}

/// 拍数 (カナだけを数える。 小書きの拗音は前の拍に含める。 句読点などの記号は 0 拍)
fn count_mora(kata: &str) -> usize {
    kata.chars()
        .filter(|&c| {
            is_kana_char(c) && !matches!(c, 'ャ' | 'ュ' | 'ョ' | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ')
        })
        .count()
}

/// 形態素を 入力テキスト上の byte 範囲に並べる (順に surface を探す)。 見つからない形態素は `None`
fn align(text: &str, morphs: &[MorphToken]) -> Vec<Option<(usize, usize)>> {
    let mut cursor = 0usize;
    let mut out = Vec::with_capacity(morphs.len());
    for m in morphs {
        match text[cursor..].find(m.surface.as_str()) {
            Some(off) if off <= 8 => {
                let s = cursor + off;
                let e = s + m.surface.len();
                out.push(Some((s, e)));
                cursor = e;
            }
            _ => out.push(None),
        }
    }
    out
}

/// 採択 token 列 + 形態素列から アクセント句を作る。
///
/// - `text` = 解析に使った (正規化後の) 入力
/// - `tokens` = その上の採択 token (range は `text` 上の byte 範囲)
pub(crate) fn build_chained_phrases(
    text: &str,
    tokens: &[Token],
    morphs: &[MorphToken],
    lexicon: &AccentLexicon,
    table: &ChainTable,
) -> Vec<ChainedPhrase> {
    build_chained_with_gap(text, tokens, morphs, lexicon, table).0
}

/// [`build_chained_phrases`] に加えて、 最後の句の後ろに残った読みの無い token の表記 (入力末の句読点など) を返す。
pub(crate) fn build_chained_with_gap(
    text: &str,
    tokens: &[Token],
    morphs: &[MorphToken],
    lexicon: &AccentLexicon,
    table: &ChainTable,
) -> (Vec<ChainedPhrase>, String) {
    let (mut nodes, meta) = make_nodes(text, tokens, morphs, lexicon, table);
    if nodes.is_empty() {
        return (Vec::new(), String::new());
    }
    let phrases = build_phrases(&mut nodes);
    let mut out = Vec::new();
    let mut gap = String::new();
    for p in phrases {
        let part = &meta[p.start..p.end];
        if p.morae == 0 {
            gap.extend(part.iter().map(|m| m.surface.as_str()));
            continue;
        }
        let sound: String = part
            .iter()
            .map(|m| match (m.surface.as_str(), m.pos.starts_with("助詞")) {
                ("は", true) => "ワ",
                ("へ", true) => "エ",
                ("を", true) => "オ",
                _ => m.reading.as_str(),
            })
            .collect();
        out.push(ChainedPhrase {
            surface: part.iter().map(|m| m.surface.as_str()).collect(),
            reading: part.iter().map(|m| m.reading.as_str()).collect(),
            mora: p.morae,
            accent: p.accent,
            estimated: part.iter().any(|m| m.estimated),
            sound,
            gap_before: std::mem::take(&mut gap),
        });
    }
    (out, gap)
}

/// 診断用: 語単体の核が表に無いノードの (表記, 読み, 品詞)
pub(crate) fn missing_accents(
    text: &str,
    tokens: &[Token],
    morphs: &[MorphToken],
    lexicon: &AccentLexicon,
    table: &ChainTable,
) -> Vec<(String, String, String)> {
    let (_, meta) = make_nodes(text, tokens, morphs, lexicon, table);
    meta.into_iter()
        .filter(|m| m.estimated && !m.reading.is_empty())
        .map(|m| (m.surface, m.reading, m.pos))
        .collect()
}

/// 診断用: 各ノードの (表記, 読み, 品詞, 前の語にくっついたか)
pub(crate) fn debug_nodes(
    text: &str,
    tokens: &[Token],
    morphs: &[MorphToken],
    lexicon: &AccentLexicon,
    table: &ChainTable,
) -> Vec<(String, String, String, bool)> {
    let (mut nodes, meta) = make_nodes(text, tokens, morphs, lexicon, table);
    if nodes.is_empty() {
        return Vec::new();
    }
    build_phrases(&mut nodes);
    nodes
        .iter()
        .zip(meta)
        .map(|(n, m)| (m.surface, m.reading, m.pos, n.chain_flag == Some(true)))
        .collect()
}

/// 診断用: 各ノードの (表記, 読み, 品詞, 拍数, 語単体の核, 推定か)。 句の連結の前の値
pub(crate) fn debug_nodes_detail(
    text: &str,
    tokens: &[Token],
    morphs: &[MorphToken],
    lexicon: &AccentLexicon,
    table: &ChainTable,
) -> Vec<(String, String, String, usize, usize, bool)> {
    let (nodes, meta) = make_nodes(text, tokens, morphs, lexicon, table);
    nodes
        .iter()
        .zip(meta)
        .map(|(n, m)| (m.surface, m.reading, m.pos, n.morae, n.accent, m.estimated))
        .collect()
}

/// ノード 1 つの付随情報 (出力の組み立てと、 欠けている核の診断に使う)
pub(crate) struct NodeMeta {
    pub surface: String,
    pub reading: String,
    pub estimated: bool,
    /// 品詞 (IPADIC の 大分類,細分類1)。 診断用
    pub pos: String,
}

/// 半角の英数字を全角にする (naist-jdic は 英字の語を全角の表記で持つ: `ＰＣ` / `ＳＮＳ`)。 英数字を含まなければ `None`
fn fullwidth_alnum(s: &str) -> Option<String> {
    if !s.chars().any(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    char::from_u32(u32::from(c) - u32::from('!') + 0xFF01).unwrap_or(c)
                } else {
                    c
                }
            })
            .collect(),
    )
}

/// 語単体の核の表を、 半角のまま → 全角に直して (英小文字は大文字にも) の順に引く
fn word_get_alnum(table: &ChainTable, surface: &str, reading: &str, p1: &str) -> Option<(u8, u8)> {
    table
        .word_get(crate::keyhash::hash_parts(&[surface, reading, p1]))
        .or_else(|| {
            let fw = fullwidth_alnum(surface)?;
            table
                .word_get(crate::keyhash::hash_parts(&[&fw, reading, p1]))
                .or_else(|| {
                    let up = fullwidth_alnum(&surface.to_ascii_uppercase())?;
                    table.word_get(crate::keyhash::hash_parts(&[&up, reading, p1]))
                })
        })
}

/// 英字 1 文字の読み (名前)。 長音の綴り違い (エイ / エー) は両方
const LETTER_NAMES: [(char, &[&str]); 26] = [
    ('A', &["エイ", "エー"]),
    ('B', &["ビー"]),
    ('C', &["シー"]),
    ('D', &["ディー"]),
    ('E', &["イー"]),
    ('F', &["エフ"]),
    ('G', &["ジー"]),
    ('H', &["エイチ", "エッチ"]),
    ('I', &["アイ"]),
    ('J', &["ジェイ", "ジェー"]),
    ('K', &["ケイ", "ケー"]),
    ('L', &["エル"]),
    ('M', &["エム"]),
    ('N', &["エヌ"]),
    ('O', &["オー", "オウ"]),
    ('P', &["ピー"]),
    ('Q', &["キュー"]),
    ('R', &["アール"]),
    ('S', &["エス"]),
    ('T', &["ティー"]),
    ('U', &["ユー"]),
    ('V', &["ブイ"]),
    ('W', &["ダブリュー", "ダブルユー"]),
    ('X', &["エックス"]),
    ('Y', &["ワイ"]),
    ('Z', &["ゼット", "ズィー"]),
];

/// 大文字だけの英字略語 (2〜4 字、 読みが文字の名前の連なり) の核: 最後の文字の名前の最初の拍 (OpenJTalk は URL / OP / PTSD …
/// を 辞書語として 後ろから 2 拍目付近に置く。 コメント等 49 語で 25 語が一致、 外れは MTG / NTR / TRPG など辞書に無い略語)。 当てはまらなければ `None`
fn abbreviation_accent(surface: &str, reading: &str) -> Option<usize> {
    let n = surface.chars().count();
    if !(2..=4).contains(&n) || !surface.chars().all(|c| c.is_ascii_uppercase()) {
        return None;
    }
    let last = surface.chars().last()?;
    let names = LETTER_NAMES.iter().find(|(c, _)| *c == last)?.1;
    let name = names.iter().find(|nm| reading.ends_with(**nm))?;
    let total = count_mora(reading);
    let tail = count_mora(name);
    (total > tail).then(|| total - tail + 1)
}

/// 表に無い名詞のカタカナ語 (外来語) の核: 4 拍以上は 後ろから 3 拍目 (OpenJTalk の未知語の傾向。 コメント / 国会 / 法令 / Wikipedia / 青空
/// 41 語で 22 語が一致、 平板は 9 語)。 3 拍以下 / 伸ばし棒だけは平板
fn loanword_accent(surface: &str, morae: usize) -> usize {
    let katakana = !surface.is_empty()
        && surface.chars().all(|c| matches!(c, 'ァ'..='ヶ' | 'ー'))
        && surface.chars().any(|c| c != 'ー');
    if katakana && morae >= 4 {
        morae - 2
    } else {
        0
    }
}

/// 分割表の部品をノードにする
fn parts_to_nodes(parts: &[SplitPart]) -> (Vec<Node>, Vec<NodeMeta>) {
    let mut nodes = Vec::with_capacity(parts.len());
    let mut meta = Vec::with_capacity(parts.len());
    for part in parts {
        let mut node = new_node(
            &part.surface,
            usize::from(part.mora),
            usize::from(part.accent),
            part.pos,
            part.renyou,
            part.rules,
        );
        node.last_pos = part.pos;
        nodes.push(node);
        meta.push(NodeMeta {
            surface: part.surface.to_string(),
            reading: part.reading.to_string(),
            estimated: false,
            pos: part.pos_label.to_string(),
        });
    }
    (nodes, meta)
}

/// 複数部品の語の分割表のキー (表記,品詞1,複合)。 品詞は IPADIC と naist-jdic で食い違う (どう = 副詞 / 感動詞) ので全部試す
const COMPOUND_POS: [(&str, &str); 3] = [("感動詞", "複合"), ("副詞", "複合"), ("接続詞", "複合")];

/// `tokens[start..]` の先頭から数 token の表記をつないだものが分割表 (複数部品の語) にあれば (token 数, 部品)。
/// 部品の読みを連結すると token の読みの連結と一致する時だけ
fn compound_split<'a>(
    tokens: &[Token],
    start: usize,
    table: &'a ChainTable,
) -> Option<(usize, &'a [SplitPart])> {
    let first = &tokens[start];
    let mut surface = String::new();
    let mut reading = String::new();
    let mut end = first.range.start;
    for (k, tok) in tokens[start..].iter().take(5).enumerate() {
        if tok.range.start != end {
            return None;
        }
        end = tok.range.end;
        surface.push_str(&tok.surface);
        reading.push_str(&crate::kana::hira_to_kata(&tok.reading));
        for (p1, p2) in COMPOUND_POS {
            if let Some(parts) = table.split_get(crate::keyhash::hash_parts(&[&surface, p1, p2])) {
                let joined: String = parts.iter().map(|p| &*p.reading).collect();
                if joined == reading {
                    return Some((k + 1, parts));
                }
            }
        }
    }
    None
}

/// 形態素 1 つを ノードにする (分割表にあれば OpenJTalk の複数語に分けて)。
fn expand_morph(
    m: &MorphToken,
    lexicon: &AccentLexicon,
    table: &ChainTable,
) -> (Vec<Node>, Vec<NodeMeta>) {
    let mut out_nodes: Vec<Node> = Vec::new();
    let mut out_meta: Vec<NodeMeta> = Vec::new();
    let reading = crate::kana::hira_to_kata(m.reading.as_deref().unwrap_or(""));
    // Lindera の 1 語を OpenJTalk の複数語に分ける (助詞連語 など)
    let sp1 = m.pos.as_deref().unwrap_or("*");
    let sp2 = m.pos_detail.as_deref().unwrap_or("*");
    if let Some(parts) = table.split_get(crate::keyhash::hash_parts(&[&m.surface, sp1, sp2])) {
        let joined: String = parts.iter().map(|p| &*p.reading).collect();
        if joined == reading {
            return parts_to_nodes(parts);
        }
    }
    let (pos, renyou) = map_pos(m);
    // IPADIC の 「*」 は None で返る。 表のキーは 「*」 なので揃える
    let (p1, p2) = (
        m.pos.as_deref().unwrap_or("*"),
        m.pos_detail.as_deref().unwrap_or("*"),
    );
    let mut estimated = false;
    let key = crate::keyhash::hash_parts(&[&m.surface, &reading, p1, p2]);
    let (accent, mora, rules) = if let Some((a, mo, r)) = table.functional_get(key) {
        (usize::from(a), usize::from(mo), r)
    } else {
        let rules = table.rules_for_key(key, p1, p2);
        // 活用語は 原形つきの表を最初に引く (たべ = 食べる 2 型 / 食ぶ 1 型 のように 綴りだけでは核が割れる語)
        let by_base = m
            .base_form
            .as_deref()
            .filter(|_| matches!(p1, "動詞" | "形容詞"))
            .and_then(|base| {
                let tail = format!("{p1},{base}");
                table.word_get(crate::keyhash::hash_parts(&[&m.surface, &reading, &tail]))
            });
        if let Some((a, mo)) = by_base
            .or_else(|| lexicon.lookup(&m.surface, &reading))
            .or_else(|| word_get_alnum(table, &m.surface, &reading, p1))
        {
            (usize::from(a), usize::from(mo), rules)
        } else {
            estimated = true;
            let morae = count_mora(&reading);
            let accent = if p1 == "名詞" {
                abbreviation_accent(&m.surface, &reading)
                    .unwrap_or_else(|| loanword_accent(&m.surface, morae))
            } else {
                0
            };
            (accent, morae, rules)
        }
    };
    let mora = if mora == 0 {
        count_mora(&reading)
    } else {
        mora
    };
    let mut node = new_node(&m.surface, mora, accent, pos, renyou, rules);
    node.last_pos = pos;
    out_nodes.push(node);
    out_meta.push(NodeMeta {
        surface: m.surface.clone(),
        reading,
        estimated,
        pos: format!("{},{}", p1, p2),
    });
    (out_nodes, out_meta)
}

/// 読み (カタカナ) から 拍数 `n` 分の文字を切り出す (小書きの拗音は前の拍に含める)
fn take_mora(chars: &[char], pos: &mut usize, n: usize) -> String {
    let counts = |c: char| {
        is_kana_char(c) && !matches!(c, 'ャ' | 'ュ' | 'ョ' | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ')
    };
    let mut out = String::new();
    let mut got = 0;
    while got < n && *pos < chars.len() {
        let c = chars[*pos];
        if counts(c) {
            got += 1;
        }
        out.push(c);
        *pos += 1;
    }
    while *pos < chars.len() && is_kana_char(chars[*pos]) && !counts(chars[*pos]) {
        out.push(chars[*pos]);
        *pos += 1;
    }
    out
}

const MONTH_READINGS: [&str; 12] = [
    "イチガツ",
    "ニガツ",
    "サンガツ",
    "シガツ",
    "ゴガツ",
    "ロクガツ",
    "シチガツ",
    "ハチガツ",
    "クガツ",
    "ジュウガツ",
    "ジュウイチガツ",
    "ジュウニガツ",
];

fn is_counter_morph(m: &MorphToken) -> bool {
    // IPADIC は 「月」 を 名詞・一般 にしている (OpenJTalk の辞書では助数詞)
    m.pos.as_deref() == Some("名詞")
        && (matches!(m.pos_detail.as_deref(), Some("接尾") | Some("副詞可能")) || m.surface == "月")
        && m.reading.as_deref().is_some_and(|r| !r.is_empty())
}

fn is_number_morph(m: &MorphToken) -> bool {
    m.pos_detail.as_deref() == Some("数") || matches!(m.surface.as_str(), "," | "，")
}

/// 1〜12 の数字の語から月の漢字表記 (一月 … 十二月) と読みの添字を得る
fn month_of(words: &[crate::accent_digit::Word]) -> Option<(String, usize)> {
    let surfaces: Vec<&str> = words.iter().map(|w| w.surface.as_str()).collect();
    let n = match surfaces.as_slice() {
        ["一"] => 1,
        ["二"] => 2,
        ["三"] => 3,
        ["四"] => 4,
        ["五"] => 5,
        ["六"] => 6,
        ["七"] => 7,
        ["八"] => 8,
        ["九"] => 9,
        ["十"] => 10,
        ["十", "一"] => 11,
        ["十", "二"] => 12,
        _ => return None,
    };
    let kanji = surfaces.concat();
    Some((format!("{kanji}月"), n - 1))
}

/// `tokens[start..]` から、 数字と助数詞の形態素だけでできた token の連なり (2 つ以上) を 1 つの token にまとめる。
/// 先頭の token は数字の形態素だけ。 連なりが 1 つだけ / 数字の形態素で始まらない時は `None`
fn numeral_run(
    tokens: &[Token],
    start: usize,
    morphs: &[MorphToken],
    spans: &[Option<(usize, usize)>],
) -> Option<(usize, Token)> {
    let overlapping = |tok: &Token| -> Vec<usize> {
        spans
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                s.and_then(|(a, b)| (a < tok.range.end && b > tok.range.start).then_some(i))
            })
            .collect()
    };
    let inside = |tok: &Token, idx: &[usize]| {
        idx.iter()
            .all(|&i| spans[i].is_some_and(|(a, b)| a >= tok.range.start && b <= tok.range.end))
    };
    let first = &tokens[start];
    let fi = overlapping(first);
    if fi.is_empty() || !inside(first, &fi) || !fi.iter().all(|&i| is_number_morph(&morphs[i])) {
        return None;
    }
    let mut end = first.range.end;
    let mut count = 1;
    for tok in &tokens[start + 1..] {
        if tok.range.start != end {
            break;
        }
        let idx = overlapping(tok);
        if idx.is_empty()
            || !inside(tok, &idx)
            || !idx
                .iter()
                .all(|&i| is_number_morph(&morphs[i]) || is_counter_morph(&morphs[i]))
        {
            break;
        }
        end = tok.range.end;
        count += 1;
    }
    if count < 2 {
        return None;
    }
    let run = &tokens[start..start + count];
    Some((
        count,
        Token {
            surface: run.iter().map(|t| t.surface.as_str()).collect(),
            reading: run.iter().map(|t| t.reading.as_str()).collect(),
            range: first.range.start..end,
            accent_phrases: Vec::new(),
            ambiguous: false,
            alternatives: Vec::new(),
            is_name: false,
        },
    ))
}

/// 数字 + 助数詞 (年月日のように繰り返してもよい) の token を、 数字段階の移植 ([`crate::accent_digit`]) で
/// ノード列にする。
///
/// 数字側の形態素がすべて 「名詞・数」 (か桁区切り)、 助数詞が 名詞・接尾 / 副詞可能 の時だけ。 数 + 助数詞の並びが終わった後の
/// 形態素は通常どおり。 作ったノードの拍数の合計が token の読みの拍数と食い違う時は `None` (従来どおり)。
#[allow(clippy::too_many_arguments)]
fn numeral_nodes(
    tok: &Token,
    tok_reading: &str,
    idx: &[usize],
    spans: &[Option<(usize, usize)>],
    morphs: &[MorphToken],
    lexicon: &AccentLexicon,
    table: &ChainTable,
    prev_surface: &str,
) -> Option<(Vec<Node>, Vec<NodeMeta>)> {
    // 数字側が token の先頭から始まること (先頭の形態素が 数)
    if idx.is_empty() || !is_number_morph(&morphs[idx[0]]) {
        return None;
    }
    for &i in idx {
        let (a, b) = spans[i]?;
        if a < tok.range.start || b > tok.range.end {
            return None;
        }
    }
    // (数字の形態素, 助数詞の形態素) の並びに分け、 残りは tail
    let mut segs: Vec<(Vec<usize>, Option<usize>)> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut tail_from = idx.len();
    for (k, &i) in idx.iter().enumerate() {
        let m = &morphs[i];
        if is_number_morph(m) {
            cur.push(i);
        } else if !cur.is_empty() && is_counter_morph(m) {
            segs.push((std::mem::take(&mut cur), Some(i)));
        } else {
            tail_from = k;
            break;
        }
    }
    if !cur.is_empty() {
        segs.push((cur, None));
    }
    if segs.is_empty() {
        return None;
    }
    let mut nodes: Vec<Node> = Vec::new();
    let mut meta: Vec<NodeMeta> = Vec::new();
    let mut before = prev_surface.to_string();
    for (num_idx, counter_idx) in &segs {
        let surface: String = num_idx
            .iter()
            .map(|&i| morphs[i].surface.as_str())
            .collect();
        let (words, used) = crate::accent_digit::parse_numeral(&surface)?;
        if used != surface.len() {
            return None;
        }
        let counter = counter_idx.map(|i| {
            let m = &morphs[i];
            (
                m.surface.as_str(),
                crate::kana::hira_to_kata(m.reading.as_deref().unwrap_or("")),
            )
        });
        // 月 (一月 … 十二月) は辞書語: 語単体の核が表にあればそれを使う
        if let (Some(ci), Some((month, n))) = (counter_idx, month_of(&words)) {
            if morphs[*ci].surface == "月" {
                let reading = MONTH_READINGS[n];
                let found = lexicon.lookup(&month, reading).or_else(|| {
                    table.word_get(crate::keyhash::hash_parts(&[&month, reading, "名詞"]))
                });
                if let Some((a, mo)) = found {
                    let key = crate::keyhash::hash_parts(&[&month, reading, "名詞", "副詞可能"]);
                    let rules = table.rules_for_key(key, "名詞", "副詞可能");
                    let mora = if mo == 0 {
                        count_mora(reading)
                    } else {
                        usize::from(mo)
                    };
                    nodes.push(new_node(
                        &month,
                        mora,
                        usize::from(a),
                        Pos::Meishi(MeishiKind::FukushiKanou),
                        false,
                        rules,
                    ));
                    meta.push(NodeMeta {
                        surface: format!("{surface}{}", morphs[*ci].surface),
                        reading: String::new(),
                        estimated: false,
                        pos: "名詞,副詞可能".to_string(),
                    });
                    before = "月".to_string();
                    continue;
                }
            }
        }
        let staged = crate::accent_digit::stage(
            &words,
            counter.as_ref().map(|(s, r)| (*s, r.as_str())),
            before.contains('月'),
        )?;
        let first = nodes.len();
        for d in &staged.nodes {
            let n = d.to_node();
            let pos_label = if n.pos == Pos::Meishi(MeishiKind::FukushiKanou) {
                "名詞,副詞可能"
            } else {
                "名詞,数"
            };
            nodes.push(n);
            meta.push(NodeMeta {
                surface: String::new(),
                reading: String::new(),
                estimated: false,
                pos: pos_label.to_string(),
            });
        }
        meta[first].surface = surface;
        if staged.counter_merged {
            if let (Some(ci), Some(m)) = (counter_idx, meta.last_mut()) {
                m.surface.push_str(&morphs[*ci].surface);
            }
        } else if let Some(ci) = counter_idx {
            let at = nodes.len();
            let (ns, ms) = expand_morph(&morphs[*ci], lexicon, table);
            nodes.extend(ns);
            meta.extend(ms);
            if let (Some(flag), Some(n)) = (staged.counter_flag, nodes.get_mut(at)) {
                n.chain_flag = Some(flag);
            }
        }
        before = counter_idx.map_or_else(String::new, |i| morphs[i].surface.clone());
    }
    for &i in &idx[tail_from..] {
        let (ns, ms) = expand_morph(&morphs[i], lexicon, table);
        nodes.extend(ns);
        meta.extend(ms);
    }
    // 拍数の合計が token の読みと合うことを確かめ、 読みを拍数ごとに配る
    let total: usize = nodes.iter().map(|n| n.morae).sum();
    if total != count_mora(tok_reading) {
        return None;
    }
    let chars: Vec<char> = tok_reading.chars().collect();
    let mut pos = 0;
    let last = nodes.len() - 1;
    for (k, (n, m)) in nodes.iter().zip(meta.iter_mut()).enumerate() {
        m.reading = take_mora(&chars, &mut pos, n.morae);
        if k == last {
            m.reading.extend(&chars[pos..]);
            pos = chars.len();
        }
    }
    debug_assert_eq!(pos, chars.len());
    Some((nodes, meta))
}

/// 採択 token と形態素から ノード列を作る。
fn make_nodes(
    text: &str,
    tokens: &[Token],
    morphs: &[MorphToken],
    lexicon: &AccentLexicon,
    table: &ChainTable,
) -> (Vec<Node>, Vec<NodeMeta>) {
    let spans = align(text, morphs);
    // (Node, 表記, 読み, 推定か)
    let mut nodes: Vec<Node> = Vec::new();
    let mut meta: Vec<NodeMeta> = Vec::new();

    let mut ti = 0;
    while ti < tokens.len() {
        // naist-jdic で 1 語が複数部品になる語 (ありがとうございました = ありがとう + ございました) は、
        // 続く token の表記をつないで分割表を引く
        if let Some((count, parts)) = compound_split(tokens, ti, table) {
            let (mut ns, ms) = parts_to_nodes(parts);
            // 複数部品の語の 2 番目以降は 別のアクセント句 (OpenJTalk は連結フラグを false にする)
            for n in ns.iter_mut().skip(1) {
                n.chain_flag = Some(false);
            }
            nodes.extend(ns);
            meta.extend(ms);
            ti += count;
            continue;
        }
        // 数字 + 助数詞が複数 token に分かれている (漢数字は 1 字ずつ、 2 + 千円 など) 時は、 つないで数字段階を通す
        if let Some((count, merged)) = numeral_run(tokens, ti, morphs, &spans) {
            let (ms_, me_) = (merged.range.start, merged.range.end);
            let midx: Vec<usize> = spans
                .iter()
                .enumerate()
                .filter_map(|(i, s)| s.and_then(|(a, b)| (a < me_ && b > ms_).then_some(i)))
                .collect();
            let reading = crate::kana::hira_to_kata(&merged.reading);
            let prev_surface = meta.last().map_or("", |m| m.surface.as_str());
            if let Some((ns, ms)) = numeral_nodes(
                &merged,
                &reading,
                &midx,
                &spans,
                morphs,
                lexicon,
                table,
                prev_surface,
            ) {
                nodes.extend(ns);
                meta.extend(ms);
                ti += count;
                continue;
            }
        }
        let tok = &tokens[ti];
        ti += 1;
        let (ts, te) = (tok.range.start, tok.range.end);
        // この token に重なる形態素 (完全に含まれるもの + 端でまたぐもの)
        let idx: Vec<usize> = spans
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.and_then(|(a, b)| (a < te && b > ts).then_some(i)))
            .collect();
        let tok_reading = crate::kana::hira_to_kata(&tok.reading);
        let inside = idx
            .iter()
            .all(|&i| spans[i].is_some_and(|(a, b)| a >= ts && b <= te));
        let morph_reading: String = idx
            .iter()
            .map(|&i| morphs[i].reading.as_deref().unwrap_or(""))
            .collect();

        // 伸ばし棒だけの token (たーい / よかったー) は 前の語の一部 (OpenJTalk は ノリター のように 1 語として読む)
        if !tok.surface.is_empty()
            && tok.surface.chars().all(|c| matches!(c, 'ー' | 'ｰ'))
            && tok_reading.chars().all(|c| c == 'ー')
        {
            if let (Some(last_node), Some(last_meta)) = (nodes.last_mut(), meta.last_mut()) {
                if last_node.morae > 0
                    && !last_meta.reading.is_empty()
                    && !matches!(last_node.pos, Pos::Joshi(_))
                {
                    let n = tok_reading.chars().count();
                    last_node.morae += n;
                    last_meta.surface.push_str(&tok.surface);
                    last_meta.reading.push_str(&tok_reading);
                    continue;
                }
            }
        }

        if tok_reading.is_empty() || !tok_reading.chars().any(is_kana_char) {
            // 読めない token (記号 / 絵文字 / 空白) は句を切る記号として扱う
            let mut n = new_node(
                &tok.surface,
                0,
                0,
                Pos::Kigou(false),
                false,
                ChainRules::default(),
            );
            n.chain_flag = None;
            nodes.push(n);
            meta.push(NodeMeta {
                surface: tok.surface.clone(),
                reading: String::new(),
                estimated: false,
                pos: "記号".to_string(),
            });
            continue;
        }

        // 数字 + 助数詞: 数字を OpenJTalk と同じ手順で 1 語ずつのノードにする (六 + 本 = ロッ、 二千円 の核 …)
        if inside {
            let prev_surface = meta.last().map_or("", |m| m.surface.as_str());
            if let Some((ns, ms)) = numeral_nodes(
                tok,
                &tok_reading,
                &idx,
                &spans,
                morphs,
                lexicon,
                table,
                prev_surface,
            ) {
                nodes.extend(ns);
                meta.extend(ms);
                continue;
            }
        }

        if inside
            && !morph_reading.is_empty()
            && crate::kana::hira_to_kata(&morph_reading) == tok_reading
        {
            // 形態素ごとのノードに展開
            for &i in &idx {
                let (ns, ms) = expand_morph(&morphs[i], lexicon, table);
                nodes.extend(ns);
                meta.extend(ms);
            }
            continue;
        }

        // 辞書で読みを直した token など: token 全体を 1 ノード。 品詞は左端 / 右端の形態素から
        let (first, last) = (idx.first().copied(), idx.last().copied());
        let (pos, renyou) = first.map_or((Pos::Meishi(MeishiKind::General), false), |i| {
            map_pos(&morphs[i])
        });
        let last_pos = last.map_or(pos, |i| map_pos(&morphs[i]).0);
        let (p1, p2) = first.map_or(("名詞", "一般"), |i| {
            (
                morphs[i].pos.as_deref().unwrap_or("*"),
                morphs[i].pos_detail.as_deref().unwrap_or("*"),
            )
        });
        let key = crate::keyhash::hash_parts(&[&tok.surface, &tok_reading, p1, p2]);
        let rules = table.rules_for_key(key, p1, p2);
        let mut estimated = false;
        let mora = count_mora(&tok_reading);
        let accent = match tok.accent_phrases.as_slice() {
            [p] if p.accent.is_some() => {
                estimated = p.estimated;
                usize::from(p.accent.unwrap_or(0))
            }
            _ => {
                if let Some((a, _)) = lexicon
                    .lookup(&tok.surface, &tok_reading)
                    .or_else(|| word_get_alnum(table, &tok.surface, &tok_reading, p1))
                {
                    usize::from(a)
                } else {
                    estimated = true;
                    if p1 == "名詞" {
                        abbreviation_accent(&tok.surface, &tok_reading)
                            .unwrap_or_else(|| loanword_accent(&tok.surface, mora))
                    } else {
                        0
                    }
                }
            }
        };
        let mut node = new_node(&tok.surface, mora, accent, pos, renyou, rules);
        node.last_pos = last_pos;
        nodes.push(node);
        meta.push(NodeMeta {
            surface: tok.surface.clone(),
            reading: tok_reading,
            estimated,
            pos: format!("{p1},{p2}"),
        });
    }

    (nodes, meta)
}

fn new_node(
    surface: &str,
    morae: usize,
    accent: usize,
    pos: Pos,
    renyou: bool,
    rules: ChainRules,
) -> Node {
    Node {
        surface: surface.to_string(),
        morae,
        accent,
        pos,
        last_pos: pos,
        renyou,
        rules,
        chain_flag: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accent_chain::AccentType;
    use crate::api::Furigana;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "furigana_chain_phrases_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(d.join("accent")).unwrap();
        d
    }

    /// 最小の辞書 + 表。 初心者(4拍 1型) / の(助詞) / 使い方(5拍 平板) / です(助動詞)
    fn mini(name: &str, with_chain: bool) -> (Furigana, PathBuf) {
        let d = tmp(name);
        std::fs::write(
            d.join("entries.toml"),
            "[meta]
schema_version = \"2\"
role = \"jukugo\"

[entries]
\"初心者\" = \"ショシンシャ\"
\"使い方\" = \"ツカイカタ\"
",
        )
        .unwrap();
        std::fs::write(
            d.join("accent/a.toml"),
            "[meta]
schema_version = \"2\"
role = \"accent\"

[entries]
\"初心者\" = \"[ショ]シンシャ\"
\"使い方\" = \"[ツカイカタ\"
",
        )
        .unwrap();
        if with_chain {
            std::fs::write(
                d.join("accent/chain.toml"),
                "[meta]
schema_version = \"2\"
role = \"accent_chain\"
format = \"accent_chain/1\"

[defaults]
\"名詞,一般\" = \"C1\"

[functional]
\"の,ノ,助詞,連体化\" = \"0/1|名詞%F1\"
\"です,デス,助動詞,*\" = \"1/2|名詞%F2@1/動詞%F1\"

[words]
\"猫,ネコ,名詞\" = \"1/2\"
",
            )
            .unwrap();
        }
        let f = Furigana::builder().core_dict_dir(&d).build().unwrap();
        (f, d)
    }

    #[test]
    fn particles_attach_and_desu_moves_the_nucleus() {
        let (f, d) = mini("attach", true);
        let ph = f.to_accent_phrases("初心者の使い方です");
        // 初心者 + の → 1 句 (核 1 を維持)。 使い方 + です → 1 句 (平板の名詞 + です = F2@1 で 5+1 = 6 型)
        assert_eq!(ph.len(), 2, "{ph:?}");
        assert_eq!(
            (ph[0].reading.as_str(), ph[0].mora, ph[0].accent),
            ("ショシンシャノ", 5, 1)
        );
        assert_eq!(
            (ph[1].reading.as_str(), ph[1].mora, ph[1].accent),
            ("ツカイカタデス", 7, 6)
        );
        assert!(!ph[0].estimated && !ph[1].estimated);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn empty_without_chain_table() {
        let (f, d) = mini("nochain", false);
        assert!(f.to_accent_phrases("初心者の使い方です").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_standalone_accent_is_estimated_and_listed() {
        let (f, d) = mini("missing", true);
        // 犬 は 表に無い (単体の核が不明 → 平板で埋めて estimated)
        let ph = f.to_accent_phrases("犬です");
        assert!(ph.iter().any(|p| p.estimated), "{ph:?}");
        let missing = f.chain_missing_accents("犬です");
        assert!(missing.iter().any(|m| m.0 == "犬"), "{missing:?}");
        let nodes = f.chain_debug_nodes("初心者の使い方です");
        assert_eq!(nodes.len(), 4, "{nodes:?}");
        assert!(!nodes[0].3 && nodes[1].3, "の は前にくっつく");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unreadable_tokens_split_phrases() {
        let (f, d) = mini("sym", true);
        let ph = f.to_accent_phrases("初心者、使い方");
        assert_eq!(ph.len(), 2, "{ph:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn table_roundtrips_through_the_binary_cache() {
        let d = tmp("roundtrip");
        let chain = "[meta]
schema_version = \"2\"
role = \"accent_chain\"
format = \"accent_chain/1\"

[defaults]
\"名詞,一般\" = \"C1\"

[functional]
\"の,ノ,助詞,連体化\" = \"0/1|名詞%F1\"

[exceptions]
\"猫,ネコ,名詞,一般\" = \"C2\"

[words]
\"猫,ネコ,名詞\" = \"1/2\"

[splits]
\"という,助詞,格助詞\" = \"と,ト,助詞,格助詞,0,0/1,動詞%F1;いう,イウ,動詞,自立,0,0/2,*\"
";
        let mut t = ChainTable::default();
        t.load_str(chain);
        t.finish();
        let mut w = crate::tablecache::Writer(Vec::new());
        t.write_to(&mut w);
        let mut r = crate::tablecache::Reader::new(&w.0);
        let u = ChainTable::read_from(&mut r).expect("読める");
        let k = |a: &str, b: &str, c: &str, d_: &str| crate::keyhash::hash_parts(&[a, b, c, d_]);
        assert_eq!(
            t.functional_get(k("の", "ノ", "助詞", "連体化"))
                .map(|x| (x.0, x.1)),
            u.functional_get(k("の", "ノ", "助詞", "連体化"))
                .map(|x| (x.0, x.1))
        );
        assert_eq!(
            u.word_get(crate::keyhash::hash_parts(&["猫", "ネコ", "名詞"])),
            Some((1, 2))
        );
        assert_eq!(
            u.rules_for_key(k("猫", "ネコ", "名詞", "一般"), "名詞", "一般"),
            t.rules_for_key(k("猫", "ネコ", "名詞", "一般"), "名詞", "一般")
        );
        let parts = u
            .split_get(crate::keyhash::hash_parts(&["という", "助詞", "格助詞"]))
            .expect("分割表");
        assert_eq!(parts.len(), 2);
        assert_eq!(&*parts[1].surface, "いう");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn map_pos_covers_the_ipadic_classes() {
        let m = |p: &str, d1: &str, d2: &str, d3: &str, cf: &str| MorphToken {
            surface: String::new(),
            reading: None,
            pos: Some(p.into()),
            pos_detail: Some(d1.into()),
            pos_detail2: Some(d2.into()),
            pos_detail3: Some(d3.into()),
            conjugation_type: None,
            conjugation_form: Some(cf.into()),
            base_form: None,
        };
        assert_eq!(
            map_pos(&m("名詞", "接尾", "*", "*", "*")).0,
            Pos::Meishi(MeishiKind::Setsubi)
        );
        assert_eq!(
            map_pos(&m("名詞", "固有名詞", "人名", "姓", "*")).0,
            Pos::Meishi(MeishiKind::Sei)
        );
        assert_eq!(
            map_pos(&m("名詞", "固有名詞", "人名", "名", "*")).0,
            Pos::Meishi(MeishiKind::Mei)
        );
        assert_eq!(
            map_pos(&m("名詞", "副詞可能", "*", "*", "*")).0,
            Pos::Meishi(MeishiKind::FukushiKanou)
        );
        assert_eq!(
            map_pos(&m("名詞", "数", "*", "*", "*")).0,
            Pos::Meishi(MeishiKind::Kazu)
        );
        assert_eq!(
            map_pos(&m("助詞", "接続助詞", "*", "*", "*")).0,
            Pos::Joshi(true)
        );
        assert_eq!(
            map_pos(&m("助動詞", "*", "*", "*", "基本形")).0,
            Pos::Jodoushi
        );
        let (pos, renyou) = map_pos(&m("動詞", "非自立", "*", "*", "連用形"));
        assert_eq!((pos, renyou), (Pos::Doushi(Sub::Hijiritsu), true));
        assert_eq!(
            map_pos(&m("形容詞", "自立", "*", "*", "基本形")).0,
            Pos::Keiyoushi(Sub::Jiritsu)
        );
        assert_eq!(
            map_pos(&m("接頭詞", "名詞接続", "*", "*", "*")).0,
            Pos::Settoushi
        );
        assert_eq!(map_pos(&m("記号", "数", "*", "*", "*")).0, Pos::Kigou(true));
        assert_eq!(map_pos(&m("副詞", "一般", "*", "*", "*")).0, Pos::Fukushi);
        assert_eq!(map_pos(&m("感動詞", "*", "*", "*", "*")).0, Pos::Other);
        for (p1, d1) in [
            ("名詞", "一般"),
            ("動詞", "自立"),
            ("接続詞", "*"),
            ("連体詞", "*"),
            ("記号", "一般"),
        ] {
            assert_eq!(
                map_pos(&m(p1, d1, "*", "*", "*")).0,
                pos_of(p1, d1),
                "{p1},{d1}"
            );
        }
    }
    #[test]
    fn mora_phrases_use_chain_rules_sound_and_breaks() {
        use crate::accent_symbols::PhraseBreak;
        let (f, d) = mini("mora", true);
        let m = f.to_mora_phrases("初心者、使い方です。");
        assert_eq!(m.phrases.len(), 2, "{m:?}");
        assert_eq!(m.phrases[0].morae.concat(), "ショシンシャ");
        assert_eq!(m.phrases[0].nucleus, Some(1));
        assert_eq!(m.phrases[1].break_before, PhraseBreak::Short);
        assert_eq!(m.phrases[1].morae.concat(), "ツカイカタデス");
        assert_eq!(m.phrases[1].nucleus, Some(6));
        assert_eq!(m.trailing, PhraseBreak::Long);
        // 連結表が無ければ 従来の token 単位の経路 (出力は to_accent 由来)
        let (g, d2) = mini("mora_nochain", false);
        assert!(!g.to_mora_phrases("初心者の使い方です").phrases.is_empty());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    fn tok(p1: &str, d1: &str, d2: &str, d3: &str, form: &str) -> MorphToken {
        MorphToken {
            surface: String::new(),
            reading: None,
            pos: Some(p1.into()),
            pos_detail: Some(d1.into()),
            pos_detail2: Some(d2.into()),
            pos_detail3: Some(d3.into()),
            conjugation_type: None,
            conjugation_form: Some(form.into()),
            base_form: None,
        }
    }

    #[test]
    fn pos_of_covers_every_class() {
        use MeishiKind as M;
        use Sub::*;
        let cases = [
            (("名詞", "一般"), Pos::Meishi(M::General)),
            (("名詞", "接尾"), Pos::Meishi(M::Setsubi)),
            (("名詞", "形容動詞語幹"), Pos::Meishi(M::KeiyoudoushiGokan)),
            (("名詞", "副詞可能"), Pos::Meishi(M::FukushiKanou)),
            (("名詞", "数"), Pos::Meishi(M::Kazu)),
            (("動詞", "自立"), Pos::Doushi(Jiritsu)),
            (("動詞", "非自立"), Pos::Doushi(Hijiritsu)),
            (("動詞", "接尾"), Pos::Doushi(Setsubi)),
            (("形容詞", "自立"), Pos::Keiyoushi(Jiritsu)),
            (("形容詞", "非自立"), Pos::Keiyoushi(Hijiritsu)),
            (("形容詞", "接尾"), Pos::Keiyoushi(Setsubi)),
            (("助詞", "接続助詞"), Pos::Joshi(true)),
            (("助詞", "格助詞"), Pos::Joshi(false)),
            (("助動詞", "*"), Pos::Jodoushi),
            (("副詞", "一般"), Pos::Fukushi),
            (("接続詞", "*"), Pos::Setsuzokushi),
            (("連体詞", "*"), Pos::Rentaishi),
            (("接頭詞", "名詞接続"), Pos::Settoushi),
            (("記号", "数"), Pos::Kigou(true)),
            (("記号", "一般"), Pos::Kigou(false)),
            (("感動詞", "*"), Pos::Other),
        ];
        for ((p1, d1), want) in cases {
            assert_eq!(pos_of(p1, d1), want, "{p1},{d1}");
        }
    }

    #[test]
    fn map_pos_names_and_conjugation() {
        use MeishiKind as M;
        let m = |p1, d1, d2, d3, f| map_pos(&tok(p1, d1, d2, d3, f));
        assert_eq!(
            m("名詞", "固有名詞", "人名", "姓", "*").0,
            Pos::Meishi(M::Sei)
        );
        assert_eq!(
            m("名詞", "固有名詞", "人名", "名", "*").0,
            Pos::Meishi(M::Mei)
        );
        // 人名でない固有名詞 / 姓名どちらでもない人名は 一般
        assert_eq!(
            m("名詞", "固有名詞", "地域", "姓", "*").0,
            Pos::Meishi(M::General)
        );
        assert_eq!(
            m("名詞", "固有名詞", "人名", "一般", "*").0,
            Pos::Meishi(M::General)
        );
        assert_eq!(
            m("名詞", "固有名詞", "一般", "名", "*").0,
            Pos::Meishi(M::General)
        );
        assert_eq!(
            m("名詞", "一般", "人名", "姓", "*").0,
            Pos::Meishi(M::General)
        );
        // 連用形か
        assert!(m("動詞", "自立", "*", "*", "連用形").1);
        assert!(m("動詞", "自立", "*", "*", "連用タ接続").1);
        assert!(!m("動詞", "自立", "*", "*", "基本形").1);
        // 欠けた項目は空文字扱い
        let empty = MorphToken {
            surface: String::new(),
            reading: None,
            pos: None,
            pos_detail: None,
            pos_detail2: None,
            pos_detail3: None,
            conjugation_type: None,
            conjugation_form: None,
            base_form: None,
        };
        assert_eq!(map_pos(&empty), (Pos::Other, false));
    }

    #[test]
    fn count_mora_counts_kana_only() {
        assert_eq!(count_mora("ショーテン"), 4);
        assert_eq!(count_mora("キョウ"), 2);
        assert_eq!(count_mora("ッ"), 1);
        assert_eq!(count_mora("ア、イ!"), 2);
        assert_eq!(count_mora("ABC"), 0);
        assert_eq!(count_mora(""), 0);
        assert_eq!(count_mora("ファイル"), 3);
    }

    #[test]
    fn align_finds_surfaces_in_order_within_a_small_gap() {
        let mk = |s: &str| MorphToken {
            surface: s.to_string(),
            ..tok("名詞", "一般", "*", "*", "*")
        };
        let text = "犬と猫";
        let r = align(text, &[mk("犬"), mk("と"), mk("猫")]);
        assert_eq!(r, vec![Some((0, 3)), Some((3, 6)), Some((6, 9))]);
        // 見つからない形態素は None で、 次の探索位置は動かない
        let r = align(text, &[mk("鳥"), mk("犬")]);
        assert_eq!(r, vec![None, Some((0, 3))]);
        // 8 バイト先までは許す / 9 バイト以上先は None
        let near = format!("{}x", "あい"); // 6 バイト + x
        assert_eq!(align(&near, &[mk("x")]), vec![Some((6, 7))]);
        let eight = format!("{}x", "あい") + "yy"; // x は 6 バイト目
        assert_eq!(align(&eight, &[mk("yy")]), vec![Some((7, 9))]);
        let at8 = format!("{}z", "あいxx"); // z は 8 バイト目
        assert_eq!(align(&at8, &[mk("z")]), vec![Some((8, 9))]);
        let at9 = format!("{}z", "あいう"); // z は 9 バイト目
        assert_eq!(align(&at9, &[mk("z")]), vec![None]);
    }

    #[test]
    fn rule_pool_stops_at_255_rules() {
        let mut t = ChainTable::default();
        let mut last = 0u8;
        for k in 0..300 {
            last = t.rule_id(&format!("F4@{k}"));
        }
        assert_eq!(t.pool.len(), 255);
        assert_eq!(last, u8::MAX);
        // 登録済みの規則は同じ id
        assert_eq!(t.rule_id("F4@0"), 0);
        assert_eq!(t.rule_id("F4@254"), 254);
    }

    const FULL_TABLE: &str = "[meta]
role = \"accent_chain\"
format = \"accent_chain/1\"
ids = [
\"a\", \"b\"
]
[defaults]
\"名詞,一般\" = \"C1\"
[[test]]
[\"x\"]
\"動詞,自立\" = \"F4@1\"
[functional]
\"の,ノ,助詞,連体化\" = \"0/1|名詞%F1\"
\"の,ノ,助詞,連体化\" = \"0/1|名詞%F1\"
\"が,ガ,助詞,格助詞\" = \"0/1|F2@1\"
\"bad,line\" = \"0/1|F1\"
\"で,デ,助詞,格助詞\" = \"x/1|F1\"
# コメント
[exceptions]
\"犬,イヌ,名詞,一般\" = \"C2\"
\"犬,イヌ,名詞,一般\" = \"C3\"
[words]
\"猫,ネコ,名詞\" = \"1/2\"
\"猫,ネコ,名詞\" = \"1/2\"
\"鳥,トリ,名詞\" = \"x/2\"
[splits]
\"という,助詞,格助詞\" = \"と,ト,助詞,格助詞,1,0/1,動詞%F1;いう,イウ,動詞,自立,0,0/2,*\"
\"単独,助詞,格助詞\" = \"と,ト,助詞,格助詞,1,0/1,動詞%F1\"
[unknown]
\"ignored,イグ,名詞,一般\" = \"C5\"
";

    fn full_table() -> ChainTable {
        let mut t = ChainTable::default();
        t.load_str(FULL_TABLE);
        t.finish();
        t
    }

    #[test]
    fn load_str_reads_every_section_and_skips_noise() {
        use crate::keyhash::hash_parts as h;
        let t = full_table();
        assert!(!t.is_empty());
        // defaults: [[test]] や ["x"] の行では section が変わらない
        assert_eq!(t.defaults.len(), 2);
        // functional: 重複は 1 件に / 不正な行は読まない
        assert_eq!(t.functional.len(), 2);
        assert!(t
            .functional_get(h(&["の", "ノ", "助詞", "連体化"]))
            .is_some());
        assert!(t
            .functional_get(h(&["が", "ガ", "助詞", "格助詞"]))
            .is_some());
        assert!(t
            .functional_get(h(&["で", "デ", "助詞", "格助詞"]))
            .is_none());
        // exceptions: 重複は 1 件に
        assert_eq!(t.exceptions.len(), 1);
        // words
        assert_eq!(t.words.len(), 1);
        assert_eq!(t.word_get(h(&["猫", "ネコ", "名詞"])), Some((1, 2)));
        assert_eq!(t.word_get(h(&["鳥", "トリ", "名詞"])), None);
        // splits: 部品が 2 つ以上の語だけ
        assert_eq!(t.splits.len(), 1);
        assert_eq!(
            t.split_get(h(&["という", "助詞", "格助詞"]))
                .map(<[_]>::len),
            Some(2)
        );
        assert!(t.split_get(h(&["単独", "助詞", "格助詞"])).is_none());
        // 知らないセクションは読まない
        assert!(t
            .rules_for_key(h(&["ignored", "イグ", "名詞", "一般"]), "名詞", "一般")
            .get_rule(Pos::Fukushi)
            .is_some_and(|r| r.accent_type == AccentType::C1));
    }

    #[test]
    fn rules_for_key_prefers_exceptions_then_defaults() {
        use crate::keyhash::hash_parts as h;
        let t = full_table();
        let ty = |r: ChainRules| r.get_rule(Pos::Fukushi).map(|x| x.accent_type);
        // 例外
        assert_eq!(
            ty(t.rules_for_key(h(&["犬", "イヌ", "名詞", "一般"]), "名詞", "一般")),
            Some(AccentType::C2)
        );
        // 例外でなければ 品詞の既定
        assert_eq!(
            ty(t.rules_for_key(h(&["猫", "ネコ", "名詞", "一般"]), "名詞", "一般")),
            Some(AccentType::C1)
        );
        assert_eq!(
            ty(t.rules_for_key(h(&["走る", "ハシル", "動詞", "自立"]), "動詞", "自立")),
            Some(AccentType::F4)
        );
        // 既定の品詞1 だけ / 品詞2 だけが合う場合は使わない
        assert_eq!(ty(t.rules_for_key(1, "名詞", "接尾")), None);
        assert_eq!(ty(t.rules_for_key(1, "動詞", "一般")), None);
        assert_eq!(ty(t.rules_for_key(1, "副詞", "一般")), None);
    }

    #[test]
    fn is_empty_needs_both_tables_empty() {
        assert!(ChainTable::default().is_empty());
        let mut t = ChainTable::default();
        t.load_str("[defaults]\n\"名詞,一般\" = \"C1\"\n");
        assert!(!t.is_empty());
        let mut t = ChainTable::default();
        t.load_str("[functional]\n\"の,ノ,助詞,連体化\" = \"0/1|F1\"\n");
        assert!(!t.is_empty());
    }

    #[test]
    fn finish_sorts_and_dedups_each_table() {
        use crate::keyhash::hash_parts as h;
        let mut t = ChainTable::default();
        t.load_str(
            "[words]\n\"b,ビ,名詞\" = \"1/2\"\n\"a,ア,名詞\" = \"2/2\"\n\"b,ビ,名詞\" = \"1/2\"\n\"c,シ,名詞\" = \"0/2\"\n\
             [exceptions]\n\"b,ビ,名詞,一般\" = \"C1\"\n\"a,ア,名詞,一般\" = \"C2\"\n\"b,ビ,名詞,一般\" = \"C1\"\n\
             [functional]\n\"b,ビ,助詞,格助詞\" = \"0/1|F1\"\n\"a,ア,助詞,格助詞\" = \"0/1|F2@1\"\n\"b,ビ,助詞,格助詞\" = \"0/1|F1\"\n\
             [splits]\n\"b,助詞,格助詞\" = \"と,ト,助詞,格助詞,0,0/1,F1;い,イ,動詞,自立,0,0/1,*\"\n\
             \"a,助詞,格助詞\" = \"と,ト,助詞,格助詞,0,0/1,F1;い,イ,動詞,自立,0,0/1,*\"\n\
             \"b,助詞,格助詞\" = \"と,ト,助詞,格助詞,0,0/1,F1;い,イ,動詞,自立,0,0/1,*\"\n",
        );
        t.finish();
        assert_eq!(t.words.len(), 3);
        assert_eq!(t.exceptions.len(), 2);
        assert_eq!(t.functional.len(), 2);
        assert_eq!(t.splits.len(), 2);
        assert!(t.words.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(t.exceptions.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(t.functional.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(t.splits.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(t.word_get(h(&["a", "ア", "名詞"])), Some((2, 2)));
        assert_eq!(t.word_get(h(&["c", "シ", "名詞"])), Some((0, 2)));
    }

    #[test]
    fn parse_part_reads_seven_fields() {
        let mut t = ChainTable::default();
        let p = t
            .parse_part("と,ト,助詞,格助詞,1,2/3,動詞%F1")
            .expect("読める");
        assert_eq!(&*p.surface, "と");
        assert_eq!(&*p.reading, "ト");
        assert_eq!(p.pos, Pos::Joshi(false));
        assert!(p.renyou);
        assert_eq!((p.accent, p.mora), (2, 3));
        assert_eq!(&*p.pos_label, "助詞,格助詞");
        let q = t.parse_part("い,イ,動詞,自立,0,0/1,*").expect("読める");
        assert!(!q.renyou);
        assert_eq!(q.pos, Pos::Doushi(Sub::Jiritsu));
        // 項目が足りない / 核の書式が違う
        assert!(t.parse_part("と,ト,助詞,格助詞,1,2/3").is_none());
        assert!(t.parse_part("と,ト,助詞,格助詞,1,23,F1").is_none());
        assert!(t.parse_part("と,ト,助詞,格助詞,1,x/3,F1").is_none());
        assert!(t.parse_part("と,ト,助詞,格助詞,1,2/y,F1").is_none());
    }

    #[test]
    fn cache_roundtrip_keeps_split_renyou_and_sections() {
        use crate::keyhash::hash_parts as h;
        let t = full_table();
        let mut w = crate::tablecache::Writer(Vec::new());
        t.write_to(&mut w);
        let mut r = crate::tablecache::Reader::new(&w.0);
        let u = ChainTable::read_from(&mut r).expect("読める");
        assert_eq!(u.defaults.len(), t.defaults.len());
        assert_eq!(u.functional.len(), t.functional.len());
        assert_eq!(u.exceptions.len(), t.exceptions.len());
        assert_eq!(u.words.len(), t.words.len());
        let parts = u
            .split_get(h(&["という", "助詞", "格助詞"]))
            .expect("分割表");
        assert!(parts[0].renyou);
        assert!(!parts[1].renyou);
        assert_eq!(parts[0].pos, Pos::Joshi(false));
        assert_eq!(parts[1].pos, Pos::Doushi(Sub::Jiritsu));
    }

    #[test]
    fn load_dir_takes_only_supported_accent_chain_files() {
        let d = tmp("loaddir");
        let w = |name: &str, body: &str| std::fs::write(d.join(name), body).unwrap();
        w(
            "ok.toml",
            "[meta]\nschema_version = \"2\"\nrole = \"accent_chain\"\nformat = \"accent_chain/1\"\n\n[defaults]\n\"名詞,一般\" = \"C1\"\n",
        );
        w(
            "future.toml",
            "[meta]\nschema_version = \"2\"\nrole = \"accent_chain\"\nformat = \"accent_chain/99\"\n\n[defaults]\n\"動詞,自立\" = \"F1\"\n",
        );
        w(
            "other.toml",
            "[meta]\nschema_version = \"2\"\nrole = \"accent\"\n\n[defaults]\n\"形容詞,自立\" = \"F2@1\"\n",
        );
        let mut t = ChainTable::default();
        t.load_dir(&d, &[]).unwrap();
        assert_eq!(t.defaults.len(), 1);
        assert_eq!(&*t.defaults[0].0, "名詞");
        // 無いディレクトリは空のまま
        let mut e = ChainTable::default();
        e.load_dir(&d.join("none"), &[]).unwrap();
        assert!(e.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn particles_are_spoken_as_sounds() {
        let (f, d) = mini("sound", true);
        let s = |t: &str| {
            f.to_mora_phrases(t)
                .phrases
                .iter()
                .map(|p| p.morae.concat())
                .collect::<String>()
        };
        assert_eq!(s("初心者は"), "ショシンシャワ");
        assert_eq!(s("初心者へ"), "ショシンシャエ");
        assert_eq!(s("初心者を"), "ショシンシャオ");
        assert_eq!(s("初心者が"), "ショシンシャガ");
        // 語中の は へ を は 読みのまま (助詞でない)
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_accents_lists_only_unknown_words() {
        let (f, d) = mini("missing2", true);
        assert!(f.chain_missing_accents("初心者の使い方です").is_empty());
        let m = f.chain_missing_accents("犬です");
        assert!(m.iter().any(|x| x.0 == "犬"));
        assert!(!m.iter().any(|x| x.0 == "です"));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn morph(surface: &str, reading: &str, p1: &str, d1: &str) -> MorphToken {
        MorphToken {
            surface: surface.to_string(),
            reading: Some(reading.to_string()),
            ..tok(p1, d1, "*", "*", "*")
        }
    }

    fn token(surface: &str, reading: &str, range: std::ops::Range<usize>) -> Token {
        Token {
            surface: surface.to_string(),
            reading: reading.to_string(),
            range,
            accent_phrases: Vec::new(),
            ambiguous: false,
            alternatives: Vec::new(),
            is_name: false,
        }
    }

    fn table_of(body: &str) -> ChainTable {
        let mut t = ChainTable::default();
        t.load_str(&format!(
            "[meta]\nrole = \"accent_chain\"\nformat = \"accent_chain/1\"\n{body}"
        ));
        t.finish();
        t
    }

    fn numeral_case(
        surface: &str,
        reading: &str,
        morphs: &[MorphToken],
        table: &ChainTable,
    ) -> Option<(Vec<Node>, Vec<NodeMeta>)> {
        let lex = AccentLexicon::default();
        let tok = token(surface, reading, 0..surface.len());
        let spans = align(surface, morphs);
        let idx: Vec<usize> = (0..morphs.len()).collect();
        numeral_nodes(&tok, reading, &idx, &spans, morphs, &lex, table, "")
    }

    #[test]
    fn numeral_nodes_expands_digits_one_word_at_a_time() {
        let t = table_of(
            "[words]
\"円,エン,名詞\" = \"1/2\"
",
        );
        let ms = [
            morph("2000", "", "名詞", "数"),
            morph("円", "エン", "名詞", "接尾"),
        ];
        let (nodes, meta) = numeral_case("2000円", "ニセンエン", &ms, &t).unwrap();
        let shape: Vec<(&str, usize, usize, Option<bool>)> = nodes
            .iter()
            .map(|n| (n.surface.as_str(), n.morae, n.accent, n.chain_flag))
            .collect();
        assert_eq!(
            shape,
            [
                ("二", 1, 1, Some(false)),
                ("千", 2, 1, Some(true)),
                ("円", 2, 1, Some(true))
            ]
        );
        let readings: Vec<&str> = meta.iter().map(|m| m.reading.as_str()).collect();
        assert_eq!(readings, ["ニ", "セン", "エン"]);
        let surfaces: String = meta.iter().map(|m| m.surface.as_str()).collect();
        assert_eq!(surfaces, "2000円");
    }

    #[test]
    fn numeral_nodes_gives_up_when_the_mora_total_disagrees_with_the_reading() {
        let t = table_of(
            "[words]
\"円,エン,名詞\" = \"1/2\"
",
        );
        let ms = [
            morph("2000", "", "名詞", "数"),
            morph("円", "エン", "名詞", "接尾"),
        ];
        // 読みが数字段階の拍数と合わない (二千円 = 5 拍 のはずが 6 拍)
        assert!(numeral_case("2000円", "ニセンエンン", &ms, &t).is_none());
    }

    #[test]
    fn numeral_nodes_keeps_months_as_dictionary_words() {
        let t = table_of(
            "[words]
\"九月,クガツ,名詞\" = \"1/3\"
",
        );
        let ms = [
            morph("9", "", "名詞", "数"),
            morph("月", "ツキ", "名詞", "一般"),
        ];
        let (nodes, meta) = numeral_case("9月", "クガツ", &ms, &t).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!((nodes[0].morae, nodes[0].accent), (3, 1));
        assert_eq!(meta[0].reading, "クガツ");
        assert_eq!(meta[0].surface, "9月");
    }

    #[test]
    fn numeral_nodes_splits_repeated_number_and_counter_pairs() {
        let t = table_of(
            "[words]
\"年,ネン,名詞\" = \"1/2\"
\"日,ニチ,名詞\" = \"1/2\"
",
        );
        let ms = [
            morph("2026", "", "名詞", "数"),
            morph("年", "ネン", "名詞", "接尾"),
            morph("13", "", "名詞", "数"),
            morph("日", "ニチ", "名詞", "接尾"),
        ];
        let (nodes, _) = numeral_case(
            "2026年13日",
            "ニセンニジュウロクネンジュウサンニチ",
            &ms,
            &t,
        )
        .unwrap();
        // 二千二十六年 (二 千 二 十 六 + 年) と、 続く 十三日
        assert_eq!(nodes.first().map(|n| n.surface.as_str()), Some("二"));
        assert_eq!(nodes.iter().filter(|n| n.surface == "年").count(), 1);
    }

    #[test]
    fn numeral_nodes_ignores_tokens_that_do_not_start_with_a_number() {
        let t = table_of("");
        let ms = [morph("円", "エン", "名詞", "接尾")];
        assert!(numeral_case("円", "エン", &ms, &t).is_none());
    }

    #[test]
    fn take_mora_keeps_small_kana_with_the_previous_mora() {
        let chars: Vec<char> = "ヒャクエン".chars().collect();
        let mut pos = 0;
        assert_eq!(take_mora(&chars, &mut pos, 1), "ヒャ");
        assert_eq!(take_mora(&chars, &mut pos, 2), "クエ");
        assert_eq!(take_mora(&chars, &mut pos, 1), "ン");
    }

    #[test]
    fn compound_split_joins_consecutive_tokens_and_splits_into_openjtalk_words() {
        let t = table_of(
            "[splits]
\"ありがとうございました,感動詞,複合\" = \"ありがとう,アリガトウ,感動詞,*,0,2/5,*;ございました,ゴザイマシタ,感動詞,*,0,4/6,*\"
",
        );
        let lex = AccentLexicon::default();
        let toks = [
            token("ありがとう", "ありがとう", 0..15),
            token("ござい", "ござい", 15..24),
            token("まし", "まし", 24..30),
            token("た", "た", 30..33),
        ];
        let text = "ありがとうございました";
        let morphs = [
            morph("ありがとう", "アリガトウ", "感動詞", "*"),
            morph("ござい", "ゴザイ", "助動詞", "*"),
            morph("まし", "マシ", "助動詞", "*"),
            morph("た", "タ", "助動詞", "*"),
        ];
        let (nodes, meta) = make_nodes(text, &toks, &morphs, &lex, &t);
        assert_eq!(nodes.len(), 2);
        assert_eq!((nodes[0].morae, nodes[0].accent), (5, 2));
        assert_eq!((nodes[1].morae, nodes[1].accent), (6, 4));
        // 2 番目の部品は別の句
        assert_eq!(nodes[1].chain_flag, Some(false));
        assert_eq!(meta[1].reading, "ゴザイマシタ");
        // 読みが合わなければ分けない
        let toks2 = [
            token("ありがとう", "ありがとう", 0..15),
            token("ござい", "ござい", 15..24),
            token("まし", "まし", 24..30),
            token("た", "だ", 30..33),
        ];
        let (nodes2, _) = make_nodes(text, &toks2, &morphs, &lex, &t);
        assert_eq!(nodes2.len(), 4);
    }

    #[test]
    fn prolonged_sound_mark_joins_the_previous_word_unless_it_is_a_particle() {
        let t = table_of("");
        let lex = AccentLexicon::default();
        // 「よかった」 + 「ー」 は 1 語 (ヨカッター)
        let toks = [token("た", "た", 0..3), token("ー", "ー", 3..6)];
        let morphs = [
            morph("た", "タ", "助動詞", "*"),
            morph("ー", "ー", "記号", "一般"),
        ];
        let (nodes, meta) = make_nodes("たー", &toks, &morphs, &lex, &t);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].morae, 2);
        assert_eq!(meta[0].reading, "ター");
        // 助詞の後ろは別
        let toks = [token("に", "に", 0..3), token("ー", "ー", 3..6)];
        let morphs = [
            morph("に", "ニ", "助詞", "格助詞"),
            morph("ー", "ー", "記号", "一般"),
        ];
        let (nodes, _) = make_nodes("にー", &toks, &morphs, &lex, &t);
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn numeral_run_joins_kanji_digits_written_one_per_token() {
        let t = table_of(
            "[words]
\"年,ネン,名詞\" = \"1/2\"
",
        );
        let lex = AccentLexicon::default();
        // 三十二年 (漢数字は 1 字ずつ token)
        let toks = [
            token("三", "さん", 0..3),
            token("十", "じゅう", 3..6),
            token("二", "に", 6..9),
            token("年", "ねん", 9..12),
        ];
        let morphs = [
            morph("三", "サン", "名詞", "数"),
            morph("十", "ジュウ", "名詞", "数"),
            morph("二", "ニ", "名詞", "数"),
            morph("年", "ネン", "名詞", "接尾"),
        ];
        let (nodes, meta) = make_nodes("三十二年", &toks, &morphs, &lex, &t);
        // 三 + 十 は 句を分けず、 二 から新しい句 (OpenJTalk: サンジュウ | ニネン)
        let phrases = build_phrases(&mut nodes.clone());
        assert_eq!(phrases.len(), 2);
        assert_eq!(phrases[0].morae, 4);
        assert_eq!(phrases[1].morae, 3);
        let readings: Vec<&str> = meta.iter().map(|m| m.reading.as_str()).collect();
        assert_eq!(readings.concat(), "サンジュウニネン");
        // 数字が 1 つだけの token は まとめない
        let one = [token("三", "さん", 0..3)];
        assert!(numeral_run(&one, 0, &morphs[..1], &align("三", &morphs[..1])).is_none());
    }

    #[test]
    fn abbreviation_accent_puts_the_nucleus_on_the_last_letter() {
        assert_eq!(abbreviation_accent("OP", "オーピー"), Some(3));
        assert_eq!(abbreviation_accent("URL", "ユーアールエル"), Some(6));
        assert_eq!(abbreviation_accent("AI", "エーアイ"), Some(3));
        assert_eq!(abbreviation_accent("PTSD", "ピーティーエスディー"), Some(7));
        // 小文字 / 混在 / 5 字以上 / 読みが文字の名前でない語は対象外
        assert_eq!(abbreviation_accent("op", "オーピー"), None);
        assert_eq!(abbreviation_accent("DbD", "ディービーディー"), None);
        assert_eq!(abbreviation_accent("ABCDE", "エイビーシーディーイー"), None);
        assert_eq!(abbreviation_accent("MAX", "マックス"), None);
        assert_eq!(abbreviation_accent("OP", "オープン"), None);
    }

    #[test]
    fn loanword_accent_is_the_third_mora_from_the_end() {
        assert_eq!(loanword_accent("クロスフォリオ", 6), 4);
        assert_eq!(loanword_accent("ゴブリンブリーチ", 8), 6);
        assert_eq!(loanword_accent("ジオゲッサー", 6), 4);
        // 3 拍以下 / 漢字まじり / 伸ばし棒だけは平板
        assert_eq!(loanword_accent("ヤバイ", 3), 0);
        assert_eq!(loanword_accent("ー", 1), 0);
        assert_eq!(loanword_accent("ーーーー", 4), 0);
        assert_eq!(loanword_accent("ペコ猫ーラ", 5), 0);
    }

    #[test]
    fn conjugated_forms_are_looked_up_by_base_form_first() {
        // たべ は 食べる (2 型) と 食ぶ (1 型) で核が違う。 原形つきの key で引く
        let t = table_of(
            "[words]
\"たべ,タベ,動詞,たべる\" = \"2/2\"
\"たべ,タベ,動詞,たぶ\" = \"1/2\"
",
        );
        let lex = AccentLexicon::default();
        let mut m = morph("たべ", "タベ", "動詞", "自立");
        m.base_form = Some("たべる".to_string());
        let (nodes, meta) = expand_morph(&m, &lex, &t);
        assert_eq!((nodes[0].morae, nodes[0].accent), (2, 2));
        assert!(!meta[0].estimated);
        m.base_form = Some("たぶ".to_string());
        let (nodes, _) = expand_morph(&m, &lex, &t);
        assert_eq!(nodes[0].accent, 1);
        // 原形が無ければ 従来どおり (表に無い → 推定の平板)
        m.base_form = None;
        let (nodes, meta) = expand_morph(&m, &lex, &t);
        assert_eq!(nodes[0].accent, 0);
        assert!(meta[0].estimated);
    }

    #[test]
    fn alphanumeric_words_are_looked_up_in_fullwidth_form() {
        // naist-jdic は 英字の語を全角で持つ (ＰＣ)
        let t = table_of(
            "[words]
\"ＰＣ,ピーシー,名詞\" = \"3/4\"
",
        );
        assert_eq!(word_get_alnum(&t, "PC", "ピーシー", "名詞"), Some((3, 4)));
        assert_eq!(word_get_alnum(&t, "pc", "ピーシー", "名詞"), Some((3, 4)));
        assert_eq!(word_get_alnum(&t, "PC", "ピーシー", "動詞"), None);
        assert_eq!(fullwidth_alnum("あいう"), None);
        assert_eq!(fullwidth_alnum("A1"), Some("Ａ１".to_string()));
    }

    #[test]
    fn make_nodes_splits_a_morph_only_when_the_parts_rejoin() {
        let t = full_table();
        let lex = AccentLexicon::default();
        let m = morph("という", "トイウ", "助詞", "格助詞");
        let (nodes, meta) =
            make_nodes("という", &[token("という", "トイウ", 0..9)], &[m], &lex, &t);
        let surfaces: Vec<&str> = meta.iter().map(|x| x.surface.as_str()).collect();
        assert_eq!(surfaces, ["と", "いう"]);
        assert_eq!(nodes.len(), 2);
        // 読みが分割表と合わなければ 分けない
        let m = morph("という", "トユウ", "助詞", "格助詞");
        let (nodes, _) = make_nodes("という", &[token("という", "トユウ", 0..9)], &[m], &lex, &t);
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn make_nodes_uses_the_table_mora_unless_it_is_zero() {
        let t = table_of("[words]\n\"猫,ネコ,名詞\" = \"1/3\"\n\"鳥,トリ,名詞\" = \"1/0\"\n");
        let lex = AccentLexicon::default();
        let run = |s: &str, r: &str| {
            let m = morph(s, r, "名詞", "一般");
            make_nodes(s, &[token(s, r, 0..s.len())], &[m], &lex, &t).0
        };
        let n = run("猫", "ネコ");
        assert_eq!((n[0].morae, n[0].accent), (3, 1));
        let n = run("鳥", "トリ");
        assert_eq!((n[0].morae, n[0].accent), (2, 1));
    }

    #[test]
    fn make_nodes_treats_readings_without_kana_as_symbols() {
        let t = ChainTable::default();
        let lex = AccentLexicon::default();
        let m = morph("ABC", "ABC", "名詞", "一般");
        let (nodes, meta) = make_nodes("ABC", &[token("ABC", "ABC", 0..3)], &[m], &lex, &t);
        assert_eq!(nodes.len(), 1);
        assert_eq!(meta[0].pos, "記号");
        assert_eq!(meta[0].reading, "");
        // 読みが空の token も 記号
        let (_, meta) = make_nodes("x", &[token("x", "", 0..1)], &[], &lex, &t);
        assert_eq!(meta[0].pos, "記号");
    }

    #[test]
    fn make_nodes_does_not_expand_morphs_that_straddle_the_token() {
        let t = ChainTable::default();
        let lex = AccentLexicon::default();
        // 形態素 「あい」 が token 「いう」 の外へはみ出す。 読みを偽って 連結が token と一致するようにしてある
        let morphs = [
            morph("あい", "イ", "名詞", "一般"),
            morph("う", "ウ", "名詞", "一般"),
        ];
        let (nodes, _) = make_nodes("あいう", &[token("いう", "イウ", 3..9)], &morphs, &lex, &t);
        assert_eq!(nodes.len(), 1, "token 全体を 1 ノードにする");
        assert_eq!(nodes[0].morae, 2);
        // 同じ形でも token に収まっていれば 形態素ごとに展開する
        let (nodes, _) = make_nodes(
            "あいう",
            &[token("あいう", "イウ", 0..9)],
            &morphs,
            &lex,
            &t,
        );
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn make_nodes_prefers_the_token_accent_over_the_lexicon() {
        let d = tmp("nodes_accent");
        std::fs::create_dir_all(d.join("accent")).unwrap();
        std::fs::write(
            d.join("accent/a.toml"),
            "[meta]\nschema_version = \"2\"\nrole = \"accent\"\n\n[entries]\n\"猫\" = \"[ネ]コ\"\n",
        )
        .unwrap();
        let mut lex = AccentLexicon::default();
        lex.load_dir(&d, &[]).unwrap();
        let t = ChainTable::default();
        // 形態素の読みが token と違うので token 全体を 1 ノードにする経路
        let morphs = [morph("猫", "ニャン", "名詞", "一般")];
        let phrase = |accent: Option<u8>, estimated: bool| {
            let mut tk = token("猫", "ネコ", 0..3);
            tk.accent_phrases = vec![crate::AccentPhrase {
                reading: "ネコ".to_string(),
                mora: 2,
                accent,
                estimated,
            }];
            tk
        };
        let (nodes, meta) = make_nodes("猫", &[phrase(Some(2), false)], &morphs, &lex, &t);
        assert_eq!((nodes[0].accent, meta[0].estimated), (2, false));
        let (nodes, meta) = make_nodes("猫", &[phrase(Some(2), true)], &morphs, &lex, &t);
        assert_eq!((nodes[0].accent, meta[0].estimated), (2, true));
        // 核が不明なら 辞書の表を引く
        let (nodes, meta) = make_nodes("猫", &[phrase(None, false)], &morphs, &lex, &t);
        assert_eq!((nodes[0].accent, meta[0].estimated), (1, false));
        // 句が無く 表にも無ければ 平板で埋めて推定扱い
        let (nodes, meta) = make_nodes(
            "犬",
            &[token("犬", "イヌ", 0..3)],
            &[morph("犬", "ニャン", "名詞", "一般")],
            &lex,
            &t,
        );
        assert_eq!((nodes[0].accent, meta[0].estimated), (0, true));
        let _ = std::fs::remove_dir_all(&d);
    }
}

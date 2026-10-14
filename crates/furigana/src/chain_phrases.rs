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
            if section.is_empty() || !t.starts_with('"') {
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
    let sub = |d: &str| match d {
        "非自立" => Sub::Hijiritsu,
        "接尾" => Sub::Setsubi,
        _ => Sub::Jiritsu,
    };
    let pos = match p1 {
        "名詞" => Pos::Meishi(match d1 {
            "接尾" => MeishiKind::Setsubi,
            "形容動詞語幹" => MeishiKind::KeiyoudoushiGokan,
            "副詞可能" => MeishiKind::FukushiKanou,
            "数" => MeishiKind::Kazu,
            "固有名詞" if d2 == "人名" && d3 == "姓" => MeishiKind::Sei,
            "固有名詞" if d2 == "人名" && d3 == "名" => MeishiKind::Mei,
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

/// ノード 1 つの付随情報 (出力の組み立てと、 欠けている核の診断に使う)
pub(crate) struct NodeMeta {
    pub surface: String,
    pub reading: String,
    pub estimated: bool,
    /// 品詞 (IPADIC の 大分類,細分類1)。 診断用
    pub pos: String,
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

    for tok in tokens {
        let (ts, te) = (tok.range.start, tok.range.end);
        // この token に重なる形態素 (完全に含まれるもの + 端でまたぐもの)
        let idx: Vec<usize> = spans
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.and_then(|(a, b)| (a < te && b > ts).then_some(i)))
            .collect();
        let tok_reading = crate::kana::hira_to_kata(&tok.reading);
        let inside = !idx.is_empty()
            && idx
                .iter()
                .all(|&i| spans[i].is_some_and(|(a, b)| a >= ts && b <= te));
        let morph_reading: String = idx
            .iter()
            .map(|&i| morphs[i].reading.as_deref().unwrap_or(""))
            .collect();

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

        if inside
            && !morph_reading.is_empty()
            && crate::kana::hira_to_kata(&morph_reading) == tok_reading
        {
            // 形態素ごとのノードに展開
            for &i in &idx {
                let m = &morphs[i];
                let reading = crate::kana::hira_to_kata(m.reading.as_deref().unwrap_or(""));
                // Lindera の 1 語を OpenJTalk の複数語に分ける (助詞連語 など)
                let sp1 = m.pos.as_deref().unwrap_or("*");
                let sp2 = m.pos_detail.as_deref().unwrap_or("*");
                if let Some(parts) =
                    table.split_get(crate::keyhash::hash_parts(&[&m.surface, sp1, sp2]))
                {
                    let joined: String = parts.iter().map(|p| &*p.reading).collect();
                    if joined == reading {
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
                        continue;
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
                    if let Some((a, mo)) = lexicon.lookup(&m.surface, &reading).or_else(|| {
                        table.word_get(crate::keyhash::hash_parts(&[&m.surface, &reading, p1]))
                    }) {
                        (usize::from(a), usize::from(mo), rules)
                    } else {
                        estimated = true;
                        (0, count_mora(&reading), rules)
                    }
                };
                let mora = if mora == 0 {
                    count_mora(&reading)
                } else {
                    mora
                };
                let mut node = new_node(&m.surface, mora, accent, pos, renyou, rules);
                node.last_pos = pos;
                nodes.push(node);
                meta.push(NodeMeta {
                    surface: m.surface.clone(),
                    reading,
                    estimated,
                    pos: format!("{},{}", p1, p2),
                });
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
                if let Some((a, _)) = lexicon.lookup(&tok.surface, &tok_reading) {
                    usize::from(a)
                } else {
                    estimated = true;
                    0
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
}

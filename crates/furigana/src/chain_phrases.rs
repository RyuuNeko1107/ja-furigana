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
            if t.starts_with('[') && !t.starts_with("[[") && t.ends_with(']') && !t.starts_with("[\"") {
                section = match t {
                    "[defaults]" => "defaults",
                    "[functional]" => "functional",
                    "[exceptions]" => "exceptions",
                    _ => "",
                };
                continue;
            }
            if section.is_empty() || !t.starts_with('"') {
                continue;
            }
            let Some((key, end)) = quoted(t) else { continue };
            let Some(rest) = t[end..].trim_start().strip_prefix('=') else { continue };
            let Some((value, _)) = quoted(rest.trim_start()) else { continue };
            match section {
                "defaults" => {
                    if let Some((p1, p2)) = key.split_once(',') {
                        let id = self.rule_id(&value);
                        self.defaults.push((p1.into(), p2.into(), id));
                    }
                }
                "functional" => {
                    let Some(k) = Self::key_of(&key) else { continue };
                    let Some((am, rule)) = value.split_once('|') else { continue };
                    let Some((a, m)) = am.split_once('/') else { continue };
                    let (Ok(a), Ok(m)) = (a.parse::<u8>(), m.parse::<u8>()) else { continue };
                    let id = self.rule_id(rule);
                    self.functional.push((k, a, m, id));
                }
                "exceptions" => {
                    let Some(k) = Self::key_of(&key) else { continue };
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
            if role == Some("accent_chain") {
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
            .map(|i| (self.functional[i].1, self.functional[i].2, self.pool[usize::from(self.functional[i].3)]))
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

/// IPADIC の品詞 → 連結規則の品詞。 戻り値の bool = 連用形
fn map_pos(m: &MorphToken) -> (Pos, bool) {
    let p1 = m.pos.as_deref().unwrap_or("");
    let d1 = m.pos_detail.as_deref().unwrap_or("");
    let d2 = m.pos_detail2.as_deref().unwrap_or("");
    let d3 = m.pos_detail3.as_deref().unwrap_or("");
    let renyou = m.conjugation_form.as_deref().is_some_and(|f| f.starts_with("連用"));
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

fn count_mora(kata: &str) -> usize {
    kata.chars().filter(|c| !matches!(c, 'ャ' | 'ュ' | 'ョ' | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ')).count()
}

fn is_kata(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| matches!(c, 'ァ'..='ヶ' | 'ー'))
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
    let spans = align(text, morphs);
    // (Node, 表記, 読み, 推定か)
    let mut nodes: Vec<Node> = Vec::new();
    let mut meta: Vec<(String, String, bool)> = Vec::new();

    for tok in tokens {
        let (ts, te) = (tok.range.start, tok.range.end);
        // この token に重なる形態素 (完全に含まれるもの + 端でまたぐもの)
        let idx: Vec<usize> = spans
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.and_then(|(a, b)| (a < te && b > ts).then_some(i)))
            .collect();
        let tok_reading = crate::kana::hira_to_kata(&tok.reading);
        let inside = !idx.is_empty() && idx.iter().all(|&i| spans[i].is_some_and(|(a, b)| a >= ts && b <= te));
        let morph_reading: String = idx.iter().map(|&i| morphs[i].reading.as_deref().unwrap_or("")).collect();

        if tok_reading.is_empty() {
            // 読めない token (記号 / 絵文字 / 空白) は句を切る記号として扱う
            let mut n = new_node(&tok.surface, 0, 0, Pos::Kigou(false), false, ChainRules::default());
            n.chain_flag = None;
            nodes.push(n);
            meta.push((tok.surface.clone(), String::new(), false));
            continue;
        }

        if inside && !morph_reading.is_empty() && crate::kana::hira_to_kata(&morph_reading) == tok_reading {
            // 形態素ごとのノードに展開
            for &i in &idx {
                let m = &morphs[i];
                let reading = crate::kana::hira_to_kata(m.reading.as_deref().unwrap_or(""));
                let (pos, renyou) = map_pos(m);
                let (p1, p2) = (m.pos.as_deref().unwrap_or(""), m.pos_detail.as_deref().unwrap_or(""));
                let mut estimated = false;
                let key = crate::keyhash::hash_parts(&[&m.surface, &reading, p1, p2]);
                let (accent, mora, rules) = if let Some((a, mo, r)) = table.functional_get(key) {
                    (usize::from(a), usize::from(mo), r)
                } else {
                    let rules = table.rules_for_key(key, p1, p2);
                    if let Some((a, mo)) = lexicon.lookup(&m.surface, &reading) {
                        (usize::from(a), usize::from(mo), rules)
                    } else {
                        estimated = true;
                        (0, count_mora(&reading), rules)
                    }
                };
                let mora = if mora == 0 { count_mora(&reading) } else { mora };
                let mut node = new_node(&m.surface, mora, accent, pos, renyou, rules);
                node.last_pos = pos;
                nodes.push(node);
                meta.push((m.surface.clone(), reading, estimated));
            }
            continue;
        }

        // 辞書で読みを直した token など: token 全体を 1 ノード。 品詞は左端 / 右端の形態素から
        let (first, last) = (idx.first().copied(), idx.last().copied());
        let (pos, renyou) = first.map_or((Pos::Meishi(MeishiKind::General), false), |i| map_pos(&morphs[i]));
        let last_pos = last.map_or(pos, |i| map_pos(&morphs[i]).0);
        let (p1, p2) = first.map_or(("名詞", "一般"), |i| (morphs[i].pos.as_deref().unwrap_or(""), morphs[i].pos_detail.as_deref().unwrap_or("")));
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
        meta.push((tok.surface.clone(), tok_reading, estimated));
    }

    if nodes.is_empty() {
        return Vec::new();
    }
    let phrases = build_phrases(&mut nodes);
    phrases
        .into_iter()
        .filter(|p| p.morae > 0)
        .map(|p| {
            let surface: String = meta[p.start..p.end].iter().map(|m| m.0.as_str()).collect();
            let reading: String = meta[p.start..p.end].iter().map(|m| m.1.as_str()).collect();
            let estimated = meta[p.start..p.end].iter().any(|m| m.2);
            ChainedPhrase { surface, reading, mora: p.morae, accent: p.accent, estimated }
        })
        .collect()
}

fn new_node(surface: &str, morae: usize, accent: usize, pos: Pos, renyou: bool, rules: ChainRules) -> Node {
    Node { surface: surface.to_string(), morae, accent, pos, last_pos: pos, renyou, rules, chain_flag: None }
}

#[allow(dead_code)]
fn _unused(_: &str) -> bool {
    is_kata("")
}

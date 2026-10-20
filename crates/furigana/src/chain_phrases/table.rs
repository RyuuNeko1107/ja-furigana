//! 付属語・内容語の結合型の表 ([`ChainTable`]) と、 dict の `role = "accent_chain"` の読み込み

use crate::accent_chain::{ChainRules, MeishiKind, Pos, Sub};
use std::collections::HashMap;
use std::path::Path;

/// 分割表の部品 1 つ (OpenJTalk の形態素)。
#[derive(Debug, Clone)]
pub(crate) struct SplitPart {
    pub(super) surface: Box<str>,
    pub(super) reading: Box<str>,
    pub(super) pos: Pos,
    pub(super) renyou: bool,
    pub(super) accent: u8,
    pub(super) mora: u8,
    pub(super) rules: ChainRules,
    pub(super) rule_id: u8,
    /// 品詞の文字列 (診断用。 「品詞1,品詞2」)
    pub(super) pos_label: Box<str>,
}

/// 付属語・内容語の結合型の表。
#[derive(Debug, Default)]
pub(crate) struct ChainTable {
    /// 内容語の既定 (「品詞1\u{1}品詞2」 → 規則)
    pub(super) defaults: Vec<(Box<str>, Box<str>, u8)>,
    /// 付属語・接尾・非自立 (key = 「表記\u{1}読み\u{1}品詞1\u{1}品詞2」) → (核, 拍数, 規則)。 key 昇順
    pub(super) functional: Vec<(u64, u8, u8, u8)>,
    /// 内容語の例外 (key は同じ形) → 規則。 key 昇順
    pub(super) exceptions: Vec<(u64, u8)>,
    /// 語単体の核 (key = 表記 / 読み / 品詞1 のハッシュ) → (核, 拍数)。 アクセント表に無い語 (活用形・かな語・外来語 等)。 key 昇順
    pub(super) words: Vec<(u64, u8, u8)>,
    /// 分割表: Lindera が 1 語にして OpenJTalk が複数語にする語 (助詞連語など)。 key = (表記, 品詞1, 品詞2) のハッシュ。 key 昇順
    pub(super) splits: Vec<(u64, Box<[SplitPart]>)>,
    /// 規則の実体 (重複を畳んだ数十件)
    pub(super) pool: Vec<ChainRules>,
    pub(super) pool_index: HashMap<String, u8>,
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
    pub(super) fn parse_part(&mut self, p: &str) -> Option<SplitPart> {
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

    pub(super) fn rule_id(&mut self, rule: &str) -> u8 {
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
    pub(super) fn key_of(csv_key: &str) -> Option<u64> {
        let mut it = csv_key.splitn(4, ',');
        let (a, b, c, d) = (it.next()?, it.next()?, it.next()?, it.next()?);
        Some(crate::keyhash::hash_parts(&[a, b, c, d]))
    }

    pub(super) fn load_str(&mut self, content: &str) {
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
                    // 部品が 2 つ以上の語 (助詞連語 / 複数部品の語)、 または 助詞の 「複合」 (IPADIC が 2 語にする 1 語。 てて)
                    if parts.len() >= 2 || (parts.len() == 1 && b == "助詞" && c == "複合") {
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

    pub(super) fn finish(&mut self) {
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
        crate::loader::for_each_toml_in_dir_roles(
            dir,
            excludes,
            |role| role == Some("accent_chain"),
            |content, _from, role| {
                if role == Some("accent_chain") && crate::tablecache::is_supported_table(content) {
                    self.load_str(content);
                }
                Ok(())
            },
        )?;
        self.finish();
        Ok(())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.functional.is_empty() && self.defaults.is_empty()
    }

    pub(super) fn functional_get(&self, key: u64) -> Option<(u8, u8, ChainRules)> {
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
    pub(super) fn split_get(&self, key: u64) -> Option<&[SplitPart]> {
        self.splits
            .binary_search_by_key(&key, |e| e.0)
            .ok()
            .map(|i| &*self.splits[i].1)
    }

    /// 語単体の核 (アクセント表に無い語用)。 `key` = (表記, 読み, 品詞1) のハッシュ
    pub(super) fn word_get(&self, key: u64) -> Option<(u8, u8)> {
        self.words
            .binary_search_by_key(&key, |e| e.0)
            .ok()
            .map(|i| (self.words[i].1, self.words[i].2))
    }

    /// 内容語の結合型。 `key` = (表記, 読み, 品詞1, 品詞2) のハッシュ
    pub(super) fn rules_for_key(&self, key: u64, p1: &str, p2: &str) -> ChainRules {
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
pub(super) fn pos_of(p1: &str, d1: &str) -> Pos {
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

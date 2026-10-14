//! アクセント句の組み立てと、 アクセント結合 (連結規則)。
//!
//! OpenJTalk の `njd_set_accent_phrase` / `njd_set_accent_type` と同じ規則を、 形態素列 ([`Node`]) に適用して
//! アクセント句 ([`ChainPhrase`]) を作る。 VOICEVOX / OpenJTalk の句分割と核の位置に合わせるための層。
//!
//! 出典 (BSD-3-Clause): OpenJTalk (Nagoya Institute of Technology, HTS Working Group) と、
//! その Rust 移植である jpreprocess (`jpreprocess-njd` の `accent_phrase.rs` / `accent_type.rs`)。
//! この file は 上記の規則を 本 crate の型に合わせて書き直したもので、 実行時は表引きと整数計算だけ (決定的)。
//!
//! ## 入力
//!
//! 形態素ごとに [`Node`] を渡す。 品詞 ([`Pos`]) は IPADIC 系の品詞 (OpenJTalk の naist-jdic も同系統)、
//! `accent` / `morae` は語単体の核位置 (0 = 平板) と拍数、 `rules` は語の結合型 (`C1` / `動詞%F4@1/助詞%F2@1` 等)。
//!
//! ## 句の組み立て (accent_phrase)
//!
//! 規則の全文は [`chain_flag`] の doc と本文を参照 (デフォルトはくっつける / 名詞の連続はくっつける /
//! 形容詞の後に名詞が来たら別の句、 など 18 規則)。
//!
//! ## アクセント結合 (accent_type)
//!
//! 句の先頭語を top とし、 後続の語 (連結される語) の結合型 (C1〜C5 = 自立語 / F1〜F5 = 付属語 / P1・P2・P6・P14)
//! で句全体の核位置を決める。 `mora_size` = top から直前までの拍数の合計。

/// 結合型の種類 (OpenJTalk の accent rule)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccentType {
    F1,
    F2,
    F3,
    F4,
    F5,
    C1,
    C2,
    C3,
    C4,
    C5,
    P1,
    P2,
    P6,
    P14,
    None,
}

impl AccentType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "F1" => Self::F1,
            "F2" => Self::F2,
            "F3" => Self::F3,
            "F4" => Self::F4,
            "F5" => Self::F5,
            "C1" => Self::C1,
            "C2" => Self::C2,
            "C3" => Self::C3,
            "C4" => Self::C4,
            "C5" => Self::C5,
            "P1" => Self::P1,
            "P2" => Self::P2,
            "P6" => Self::P6,
            "P14" => Self::P14,
            "" | "*" => Self::None,
            _ => return Option::None,
        })
    }
}

/// 結合型 1 つ (`F4@1` なら `accent_type = F4`, `add_type = 1`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChainRule {
    pub accent_type: AccentType,
    pub add_type: isize,
}

/// 語の結合型。 直前の語の品詞ごとに規則を持つ (`動詞%F4@1/助詞%F2@1` や、 品詞指定なしの `C1`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ChainRules {
    default: Option<ChainRule>,
    doushi: Option<ChainRule>,
    joshi: Option<ChainRule>,
    keiyoushi: Option<ChainRule>,
    meishi: Option<ChainRule>,
}

impl ChainRules {
    /// naist-jdic の結合型の文字列 (`*` / `C1` / `動詞%F4@1/助詞%F2@1` …) を読む。 読めない部分は捨てる。
    pub(crate) fn parse(rules: &str) -> Self {
        let mut r = Self::default();
        if rules.is_empty() || rules == "*" {
            return r;
        }
        for part in rules.split('/') {
            let (pos, rest) = match part.split_once('%') {
                Some((p, rest)) => (Some(p), rest),
                None => (None, part),
            };
            let (kind, add) = match rest.split_once('@') {
                Some((k, a)) => (k, a.parse::<isize>().unwrap_or(0)),
                None => (rest, 0),
            };
            let Some(accent_type) = AccentType::parse(kind) else {
                continue;
            };
            let rule = Some(ChainRule {
                accent_type,
                add_type: add,
            });
            match pos {
                None => r.default = rule,
                Some("動詞") => r.doushi = rule,
                Some("助詞") => r.joshi = rule,
                Some("形容詞") => r.keiyoushi = rule,
                Some("名詞") => r.meishi = rule,
                Some(_) => {}
            }
        }
        r
    }

    /// 直前の語の品詞に対応する規則 (無ければ品詞指定なしの規則)。 助動詞は 動詞扱い。
    fn get_rule(&self, prev: Pos) -> Option<ChainRule> {
        let by_pos = match prev {
            Pos::Doushi(_) | Pos::Jodoushi => self.doushi,
            Pos::Joshi(_) => self.joshi,
            Pos::Keiyoushi(_) => self.keiyoushi,
            Pos::Meishi(_) => self.meishi,
            _ => None,
        };
        by_pos.or(self.default)
    }
}

/// 名詞の細分類 (アクセント句の規則が参照するものだけ)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MeishiKind {
    /// 一般 / 固有名詞(一般・地域・組織) / サ変接続 / 代名詞 ほか、 規則上は区別しない名詞
    General,
    /// 形容動詞語幹
    KeiyoudoushiGokan,
    /// 副詞可能
    FukushiKanou,
    /// 接尾
    Setsubi,
    /// 固有名詞・人名・姓
    Sei,
    /// 固有名詞・人名・名
    Mei,
    /// 数
    Kazu,
}

/// 動詞・形容詞の細分類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sub {
    Jiritsu,
    Hijiritsu,
    Setsubi,
}

/// 品詞 (OpenJTalk の POS を、 アクセント句の規則に必要な粒度に畳んだもの)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pos {
    Meishi(MeishiKind),
    Doushi(Sub),
    Keiyoushi(Sub),
    /// `true` = 接続助詞
    Joshi(bool),
    Jodoushi,
    Fukushi,
    Setsuzokushi,
    Rentaishi,
    Settoushi,
    /// `true` = 記号・数 (数字は前処理で 名詞・数 から 記号・数 に変わる)
    Kigou(bool),
    /// 感動詞 / フィラー / その他
    Other,
}

impl Pos {
    fn is_kazu(self) -> bool {
        matches!(self, Pos::Meishi(MeishiKind::Kazu) | Pos::Kigou(true))
    }
}

/// 1 形態素。
#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub surface: String,
    /// 拍数 (読みの長さ)
    pub morae: usize,
    /// 語単体の核位置 (0 = 平板)。 [`set_accent_type`] が句の先頭語の値を句全体の核に書き換える
    pub accent: usize,
    /// 左端の語の品詞 (この語が 「後ろ」 として前の語に連なるときに見る)
    pub pos: Pos,
    /// 右端の語の品詞 (この語が 「前」 として次の語に連なるときに見る。 1 形態素なら `pos` と同じ)
    pub last_pos: Pos,
    /// 連用形か (動詞・形容詞の「連用*」)
    pub renyou: bool,
    pub rules: ChainRules,
    /// 辞書が明示した句の連結 (`None` = 規則で決める)
    pub chain_flag: Option<bool>,
}

/// 句の連結を決める (`true` = 前の語にくっつける)。
///
/// 規則 (OpenJTalk `njd_set_accent_phrase`):
///
/// 01. デフォルトはくっつける 02. 「名詞」の連続はくっつける 03. 「形容詞」の後に「名詞」がきたら別の句
/// 04. 「名詞,形容動詞語幹」の後に「名詞」がきたら別の句 05. 「動詞」の後に「形容詞」or「名詞」がきたら別の句
/// 06. 「副詞」「接続詞」「連体詞」は単独の句 07. 「名詞,副詞可能」は単独の句
/// 08. 「助詞」「助動詞」(付属語) は前にくっつける 09. 付属語の後の自立語は別の句
/// 10. 「*,接尾」の後の「名詞」は別の句 11. 「形容詞,非自立」は 連用形の動詞・形容詞 / 助詞「て」「で」の後でくっつける
/// 12. 「動詞,非自立」は 連用形の動詞の後でくっつける 13. 「名詞」の後に「動詞」「形容詞」「形容動詞語幹」がきたら別の句
/// 14. 「記号」は単独の句 15. 「接頭詞」は単独の句 16. 姓の後の名詞は別の句 17. 名詞の後の「名」は別の句
/// 18. 「*,接尾」は前にくっつける
fn chain_flag(prev: &Node, node: &Node) -> bool {
    use MeishiKind as M;
    use Pos::{
        Doushi as D, Fukushi as Fu, Jodoushi as Jo, Joshi as Js, Keiyoushi as K, Kigou as Ki, Meishi as Me,
        Rentaishi as Re, Settoushi as Se, Setsuzokushi as Sz,
    };
    use Sub::{Hijiritsu as H, Setsubi as S};
    match (prev.last_pos, node.pos) {
        // 18
        (_, K(S) | D(S) | Me(M::Setsubi)) => true,
        // 17
        (Me(_), Me(M::Mei)) => false,
        // 16
        (Me(M::Sei), Me(_)) => false,
        // 15
        (_, Se) => false,
        // 14
        (Ki(_), _) | (_, Ki(_)) => false,
        // 13
        (Me(_), D(_)) | (Me(_), K(_)) | (Me(_), Me(M::KeiyoudoushiGokan)) => false,
        // 12
        (D(_), D(H)) if prev.renyou => true,
        // 11
        (D(_), K(H)) if prev.renyou => true,
        (K(_), K(H)) if prev.renyou => true,
        (Js(true), K(H)) if matches!(prev.surface.as_str(), "て" | "で") => true,
        // 10
        (K(S) | D(S) | Me(M::Setsubi), Me(_)) => false,
        // 08
        (Jo | Js(_), Jo | Js(_)) => true,
        // 09
        (Jo | Js(_), _) => false,
        // 08
        (_, Jo | Js(_)) => true,
        // 07
        (Me(M::FukushiKanou), _) | (_, Me(M::FukushiKanou)) => false,
        // 06
        (Fu | Sz | Re, _) | (_, Fu | Sz | Re) => false,
        // 05
        (D(_), K(_) | Me(_)) => false,
        // 04
        (Me(M::KeiyoudoushiGokan), Me(_)) => false,
        // 03
        (K(_), Me(_)) => false,
        // 02
        (Me(_), Me(_)) => true,
        // 01
        _ => true,
    }
}

/// アクセント句の連結 ([`Node::chain_flag`] が `None` の語だけ決める)。 先頭語は常に句の頭。
pub(crate) fn set_accent_phrase(nodes: &mut [Node]) {
    for i in 1..nodes.len() {
        if nodes[i].chain_flag.is_none() {
            let c = chain_flag(&nodes[i - 1], &nodes[i]);
            nodes[i].chain_flag = Some(c);
        }
    }
}

/// 句の先頭語 (top) の核を、 連結される語の結合型で決める。 `mora_size` = top から直前の語までの拍数の合計。
fn calc_top_node_acc(node: &Node, prev: &Node, top_acc: usize, mora_size: usize) -> usize {
    let node_acc = node.accent;
    let Some(rule) = node.rules.get_rule(prev.last_pos) else {
        return top_acc;
    };
    let add_rule = || (mora_size as isize + rule.add_type).max(0) as usize;
    match rule.accent_type {
        AccentType::F1 => top_acc,
        AccentType::F2 if top_acc == 0 => add_rule(),
        AccentType::F3 if top_acc != 0 => add_rule(),
        AccentType::F4 => add_rule(),
        AccentType::F5 => 0,
        AccentType::C1 => mora_size + node_acc,
        AccentType::C2 => mora_size + 1,
        AccentType::C3 => mora_size,
        AccentType::C4 => 0,
        AccentType::C5 => top_acc,
        AccentType::P1 | AccentType::P2 if top_acc == 0 => 0,
        AccentType::P1 | AccentType::P2 => mora_size + node_acc,
        AccentType::P6 => 0,
        AccentType::P14 if top_acc != 0 => mora_size + node_acc,
        _ => top_acc,
    }
}

/// 数詞どうしの連結 (十 / 百 / 千 / 万 / 億 / 兆) の核。 `prev` の核を決める。
fn calc_digit_acc(prev: &Node, current: &Node, _next: Option<&Node>) -> Option<usize> {
    let (p, c) = (prev.surface.as_str(), current.surface.as_str());
    let pm = prev.morae;
    let cm = current.morae;
    match (p, c) {
        ("五" | "六" | "八", "十") if _next.is_some_and(|n| matches!(n.surface.as_str(), "一" | "二" | "三" | "四" | "五" | "六" | "七" | "八" | "九")) => Some(0),
        (_, "十") => Some(1),
        ("七", "百") => Some(2),
        ("三" | "四" | "九" | "何", "百") => Some(1),
        (_, "百") => Some(pm + cm),
        (_, "千") => Some(pm + 1),
        (_, "万") => Some(pm + 1),
        ("一" | "六" | "七" | "八" | "幾", "億") => Some(2),
        (_, "億") => Some(1),
        ("六" | "七", "兆") => Some(2),
        (_, "兆") => Some(1),
        _ => None,
    }
}

/// アクセント結合。 [`set_accent_phrase`] の後に呼ぶ。 句の先頭語の `accent` が句全体の核位置になる。
pub(crate) fn set_accent_type(nodes: &mut [Node]) {
    let mut top_i = 0usize;
    let mut mora_size = 0usize;
    for i in 0..nodes.len() {
        let mut top_acc: Option<usize> = None;
        let mut prev_acc: Option<usize> = None;
        let mut current_acc: Option<usize> = None;
        {
            let current = &nodes[i];
            let next = nodes.get(i + 1);
            if i == 0 || current.chain_flag != Some(true) {
                top_i = i;
                mora_size = 0;
                if current.surface == "十" && next.is_some_and(|n| n.pos.is_kazu()) {
                    current_acc = Some(0);
                }
            } else {
                let prev = &nodes[i - 1];
                top_acc = Some(calc_top_node_acc(current, prev, nodes[top_i].accent, mora_size));
                if prev.last_pos.is_kazu() && current.pos.is_kazu() {
                    prev_acc = calc_digit_acc(prev, current, next);
                }
            }
            mora_size += current.morae;
        }
        if let Some(a) = top_acc {
            nodes[top_i].accent = a;
        }
        if let Some(a) = prev_acc {
            nodes[i - 1].accent = a;
        }
        if let Some(a) = current_acc {
            nodes[i].accent = a;
        }
    }
}

/// 組み立て後の 1 アクセント句。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChainPhrase {
    /// `nodes[start..end]` が この句
    pub start: usize,
    pub end: usize,
    /// 句全体の拍数
    pub morae: usize,
    /// 核位置 (0 = 平板、 n = n 拍目で下がる)
    pub accent: usize,
}

/// 形態素列からアクセント句を組み立てる (句の連結 → アクセント結合 → 句ごとにまとめる)。 `nodes` は書き換わる。
pub(crate) fn build_phrases(nodes: &mut [Node]) -> Vec<ChainPhrase> {
    set_accent_phrase(nodes);
    set_accent_type(nodes);
    let mut out: Vec<ChainPhrase> = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        let chained = i > 0 && n.chain_flag == Some(true);
        if chained {
            if let Some(last) = out.last_mut() {
                last.end = i + 1;
                last.morae += n.morae;
                continue;
            }
        }
        out.push(ChainPhrase {
            start: i,
            end: i + 1,
            morae: n.morae,
            accent: n.accent,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(surface: &str, morae: usize, accent: usize, pos: Pos, rules: &str) -> Node {
        Node {
            surface: surface.to_string(),
            morae,
            accent,
            pos,
            last_pos: pos,
            renyou: false,
            rules: ChainRules::parse(rules),
            chain_flag: None,
        }
    }

    #[test]
    fn rules_parse_variants() {
        let r = ChainRules::parse("動詞%F4@1/助詞%F2@1");
        assert_eq!(r.doushi, Some(ChainRule { accent_type: AccentType::F4, add_type: 1 }));
        assert_eq!(r.joshi, Some(ChainRule { accent_type: AccentType::F2, add_type: 1 }));
        assert_eq!(r.default, None);
        let r = ChainRules::parse("C2");
        assert_eq!(r.default, Some(ChainRule { accent_type: AccentType::C2, add_type: 0 }));
        assert_eq!(ChainRules::parse("*"), ChainRules::default());
        assert_eq!(ChainRules::parse(""), ChainRules::default());
        // 助動詞 は 動詞扱い
        let r = ChainRules::parse("動詞%F1");
        assert!(r.get_rule(Pos::Jodoushi).is_some());
        assert!(r.get_rule(Pos::Kigou(false)).is_none());
    }

    #[test]
    fn noun_noun_chain_and_particle_attach() {
        // 初心者(名詞, 4拍, 1型) + の(助詞) → 1 句、 核は 1 のまま (F1 = 先頭語の核を維持)
        let mut v = vec![
            node("初心者", 4, 1, Pos::Meishi(MeishiKind::General), "C1"),
            node("の", 1, 0, Pos::Joshi(false), "名詞%F1"),
        ];
        let p = build_phrases(&mut v);
        assert_eq!(p, vec![ChainPhrase { start: 0, end: 2, morae: 5, accent: 1 }]);
    }

    #[test]
    fn noun_then_verb_is_new_phrase() {
        // 規則 13: 名詞の後の動詞は別の句
        let mut v = vec![
            node("水", 2, 0, Pos::Meishi(MeishiKind::General), "C1"),
            node("飲む", 3, 2, Pos::Doushi(Sub::Jiritsu), "*"),
        ];
        let p = build_phrases(&mut v);
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].morae, p[0].accent), (2, 0));
        assert_eq!((p[1].morae, p[1].accent), (3, 2));
    }

    #[test]
    fn prefix_starts_phrase_and_chains_forward() {
        // 規則 15: 接頭詞は 句の頭になる (前の語にはくっつかない) が、 後ろの語とは 1 句 (お茶)
        let mut v = vec![
            node("お", 1, 0, Pos::Settoushi, "*"),
            node("茶", 1, 1, Pos::Meishi(MeishiKind::General), "C1"),
        ];
        assert_eq!(build_phrases(&mut v).len(), 1);
        let mut w = vec![
            node("水", 2, 0, Pos::Meishi(MeishiKind::General), "C1"),
            node("お", 1, 0, Pos::Settoushi, "*"),
            node("茶", 1, 1, Pos::Meishi(MeishiKind::General), "C1"),
        ];
        assert_eq!(build_phrases(&mut w).len(), 2);
    }

    #[test]
    fn symbol_is_standalone() {
        // 規則 14: 記号は 前後とくっつかない
        let mut v = vec![
            node("犬", 2, 0, Pos::Meishi(MeishiKind::General), "C1"),
            node("、", 0, 0, Pos::Kigou(false), "*"),
            node("猫", 2, 1, Pos::Meishi(MeishiKind::General), "C1"),
        ];
        assert_eq!(build_phrases(&mut v).len(), 3);
    }

    #[test]
    fn cow_example_from_jpreprocess() {
        // 牛飼じゃありませんよ: 付属語が連なって 1 句。 核は ませ(F4@1) で 牛飼 4 + ... = 9
        let mut v = vec![
            node("牛飼", 4, 2, Pos::Meishi(MeishiKind::General), "C2"),
            node("じゃ", 1, 0, Pos::Joshi(false), "名詞%F1"),
            node("あり", 2, 2, Pos::Jodoushi, "動詞%F1"),
            node("ませ", 2, 1, Pos::Jodoushi, "動詞%F4@1/助詞%F2@1"),
            node("ん", 1, 1, Pos::Jodoushi, "動詞%F4"),
            node("よ", 1, 0, Pos::Joshi(false), "動詞%F1/形容詞%F1/名詞%F1"),
        ];
        let p = build_phrases(&mut v);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].morae, 11);
        assert_eq!(p[0].accent, 9);
    }

    #[test]
    fn explicit_chain_flag_is_respected() {
        let mut a = node("犬", 2, 0, Pos::Meishi(MeishiKind::General), "C1");
        let mut b = node("小屋", 2, 0, Pos::Meishi(MeishiKind::General), "C1");
        a.chain_flag = None;
        b.chain_flag = Some(false);
        let mut v = vec![a, b];
        assert_eq!(build_phrases(&mut v).len(), 2);
    }
}

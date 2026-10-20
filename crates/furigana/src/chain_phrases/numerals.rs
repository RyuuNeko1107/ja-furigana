//! 数字 + 助数詞 (年月日など) の token をノード列にする

use super::count_mora;
use super::nodes::expand_morph;
use super::nodes::new_node;
use super::nodes::take_mora;
use super::table::ChainTable;
use super::NodeMeta;
use crate::accent_chain::{MeishiKind, Node, Pos};
use crate::analyzer::MorphToken;
use crate::scoring::accent_lexicon::AccentLexicon;
use crate::scoring::analyze::Token;

pub(super) const MONTH_READINGS: [&str; 12] = [
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
pub(super) fn numeral_run(
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
            band: 50,
            why: None,
        },
    ))
}

/// 数字 + 助数詞 (年月日のように繰り返してもよい) の token を、 数字段階の移植 ([`crate::accent_digit`]) で
/// ノード列にする。
///
/// 数字側の形態素がすべて 「名詞・数」 (か桁区切り)、 助数詞が 名詞・接尾 / 副詞可能 の時だけ。 数 + 助数詞の並びが終わった後の
/// 形態素は通常どおり。 作ったノードの拍数の合計が token の読みの拍数と食い違う時は `None` (従来どおり)。
#[allow(clippy::too_many_arguments)]
pub(super) fn numeral_nodes(
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

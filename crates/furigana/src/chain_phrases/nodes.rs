//! 採択 token と形態素から、 アクセント句の連結用ノード列を作る

use super::align;
use super::count_mora;
use super::is_kana_char;
use super::map_pos;
use super::numerals::numeral_nodes;
use super::numerals::numeral_run;
use super::table::ChainTable;
use super::table::SplitPart;
use super::NodeMeta;
use crate::accent_chain::{ChainRules, MeishiKind, Node, Pos};
use crate::analyzer::MorphToken;
use crate::scoring::accent_lexicon::AccentLexicon;
use crate::scoring::analyze::Token;

/// 半角の英数字を全角にする (naist-jdic は 英字の語を全角の表記で持つ: `ＰＣ` / `ＳＮＳ`)。 英数字を含まなければ `None`
pub(super) fn fullwidth_alnum(s: &str) -> Option<String> {
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
pub(super) fn word_get_alnum(
    table: &ChainTable,
    surface: &str,
    reading: &str,
    p1: &str,
) -> Option<(u8, u8)> {
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
pub(super) fn abbreviation_accent(surface: &str, reading: &str) -> Option<usize> {
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
pub(super) fn loanword_accent(surface: &str, morae: usize) -> usize {
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

/// 複数部品の語の分割表のキー (表記,品詞1,複合)。 品詞は IPADIC と naist-jdic で食い違う (どう = 副詞 / 感動詞) ので全部試す。
/// 助詞は 「てて」 のように IPADIC が 2 語 (て + て) にする naist-jdic の 1 語
const COMPOUND_POS: [(&str, &str); 4] = [
    ("感動詞", "複合"),
    ("副詞", "複合"),
    ("接続詞", "複合"),
    ("助詞", "複合"),
];

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

/// 助詞の形態素 2 つ (て + て) の表記をつないだものが分割表 (キー = 表記,助詞,複合) にあれば その部品。
/// 部品の読みが 2 つの形態素の読みの連結と一致する時だけ
fn particle_pair_split<'a>(
    first: &MorphToken,
    second: Option<&MorphToken>,
    table: &'a ChainTable,
) -> Option<&'a [SplitPart]> {
    let second = second?;
    // 1 つ目は 助詞 か、 「てる」 の 動詞・非自立 (IPADIC は してて を し + て(動詞) + て(助詞) にする)
    if !matches!(first.pos.as_deref(), Some("助詞" | "動詞"))
        || second.pos.as_deref() != Some("助詞")
    {
        return None;
    }
    let surface = format!("{}{}", first.surface, second.surface);
    let reading = crate::kana::hira_to_kata(&format!(
        "{}{}",
        first.reading.as_deref().unwrap_or(""),
        second.reading.as_deref().unwrap_or("")
    ));
    let parts = table.split_get(crate::keyhash::hash_parts(&[&surface, "助詞", "複合"]))?;
    let joined: String = parts.iter().map(|p| &*p.reading).collect();
    (joined == reading).then_some(parts)
}

/// 形態素 1 つを ノードにする (分割表にあれば OpenJTalk の複数語に分けて)。
pub(super) fn expand_morph(
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
pub(super) fn take_mora(chars: &[char], pos: &mut usize, n: usize) -> String {
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

/// 採択 token と形態素から ノード列を作る。
pub(super) fn make_nodes(
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
            let mut k = 0;
            while k < idx.len() {
                // IPADIC が 2 語 (て + て) にする naist-jdic の 1 語の助詞 (してて の てて) は つないで 1 ノード
                if let Some(parts) =
                    particle_pair_split(&morphs[idx[k]], idx.get(k + 1).map(|&j| &morphs[j]), table)
                {
                    let (ns, ms) = parts_to_nodes(parts);
                    nodes.extend(ns);
                    meta.extend(ms);
                    k += 2;
                    continue;
                }
                let (ns, ms) = expand_morph(&morphs[idx[k]], lexicon, table);
                nodes.extend(ns);
                meta.extend(ms);
                k += 1;
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

pub(super) fn new_node(
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

//! 数字 + 助数詞のアクセント (OpenJTalk の `njd_set_digit` / `njd_digit_sequence` の移植)。
//!
//! OpenJTalk は数字を 「二」「千」「円」 のように 1 語ずつのノードにして、 助数詞によって数字の拍数 / 核を差し替え
//! (六 + 本 = ロッ、 九 + 時 = ク、 …)、 句の連結フラグを付けてから、 アクセント結合 ([`crate::accent_chain`]) に渡す。
//! この module はその数字段階を、 本 crate の [`Node`] 列として再現する。 実行時は表引きと整数演算だけ。
//!
//! 出典 (BSD-3-Clause): OpenJTalk (Nagoya Institute of Technology, HTS Working Group) と、
//! その Rust 移植である jpreprocess (`jpreprocess-njd` の `open_jtalk/digit` / `digit_sequence`)。
//! 表は `accent_digit_tables.rs` (jpreprocess 0.15.0 の lut から生成)。
//!
//! 読み (カタカナ) は作らない。 拍数と核だけを決め、 読みは呼び出し側が持つ (こちらの読みを使う)。

use crate::accent_chain::{ChainRules, MeishiKind, Node, Pos};
use crate::accent_digit_tables as t;

/// 数字 1 語 (漢数字 1 字か、 十 / 百 / 千 / 万 / 億 / 兆)。
#[derive(Debug, Clone)]
pub(crate) struct Word {
    pub surface: String,
    /// 算用数字の列から作った 位取りの語か (結合型が `*` になる)
    generated: bool,
}

/// 数字段階を通した結果の 1 ノード。
#[derive(Debug, Clone)]
pub(crate) struct DigitNode {
    pub surface: String,
    pub morae: usize,
    pub accent: usize,
    rules: &'static str,
    chain_flag: Option<bool>,
    /// 助数詞を取り込んだ語 (一人 / 二日 / 十四日 など) は 副詞可能
    merged: bool,
}

impl DigitNode {
    pub(crate) fn to_node(&self) -> Node {
        let pos = if self.merged {
            Pos::Meishi(MeishiKind::FukushiKanou)
        } else {
            Pos::Meishi(MeishiKind::Kazu)
        };
        Node {
            surface: self.surface.clone(),
            morae: self.morae,
            accent: self.accent,
            pos,
            last_pos: pos,
            renyou: false,
            rules: ChainRules::parse(self.rules),
            chain_flag: self.chain_flag,
        }
    }
}

/// 数字段階の結果。
#[derive(Debug)]
pub(crate) struct Staged {
    pub nodes: Vec<DigitNode>,
    /// 助数詞を数字のノードに取り込んだか (その時は助数詞の形態素を別にノードにしない)
    pub counter_merged: bool,
    /// 助数詞のノードに付ける連結フラグ (数字の直後の助数詞は前に連結する)
    pub counter_flag: Option<bool>,
}

fn digit_value(c: char) -> Option<u32> {
    match c {
        '0'..='9' => Some(u32::from(c) - u32::from('0')),
        '\u{FF10}'..='\u{FF19}' => Some(u32::from(c) - u32::from('\u{FF10}')),
        _ => None,
    }
}

fn kanji_digit(c: char) -> Option<&'static str> {
    Some(match c {
        '一' => "一",
        '二' => "二",
        '三' => "三",
        '四' => "四",
        '五' => "五",
        '六' => "六",
        '七' => "七",
        '八' => "八",
        '九' => "九",
        _ => return None,
    })
}

fn kanji_unit(c: char) -> Option<&'static str> {
    Some(match c {
        '十' => "十",
        '百' => "百",
        '千' => "千",
        '万' => "万",
        '億' => "億",
        '兆' => "兆",
        _ => return None,
    })
}

const UNITS_LOW: [&str; 4] = ["", "十", "百", "千"];
const UNITS_HIGH: [&str; 4] = ["", "万", "億", "兆"];

/// 算用数字の列 (2 桁以上) を 位取りの語にする。 桁が多すぎる / 先頭が 0 / 全部 0 は `None`
fn words_of_digits(digits: &[u32]) -> Option<Vec<Word>> {
    if digits.len() > 16 || digits[0] == 0 || digits.iter().all(|&d| d == 0) {
        return None;
    }
    let n = digits.len();
    let digit_word = |d: u32| Word {
        surface: ["", "一", "二", "三", "四", "五", "六", "七", "八", "九"][d as usize].to_string(),
        generated: true,
    };
    let unit_word = |s: &str| Word {
        surface: s.to_string(),
        generated: true,
    };
    let mut out = Vec::new();
    let mut have_digit_in_block = false;
    for (i, &d) in digits.iter().enumerate() {
        let rev = n - i - 1;
        if d != 0 {
            have_digit_in_block = true;
        }
        if rev.is_multiple_of(4) {
            if d != 0 {
                out.push(digit_word(d));
            }
            if have_digit_in_block && rev > 0 {
                out.push(unit_word(UNITS_HIGH[rev / 4]));
            }
            have_digit_in_block = false;
        } else {
            match d {
                0 => {}
                1 => out.push(unit_word(UNITS_LOW[rev % 4])),
                _ => {
                    out.push(digit_word(d));
                    out.push(unit_word(UNITS_LOW[rev % 4]));
                }
            }
        }
    }
    Some(out)
}

/// 先頭の数 (算用数字 / 漢数字 / その混在) を語に分け、 使った byte 長を返す。
/// 数字を 1 つずつ読む形 (一二三) / 〇 / 0 だけ / 先頭 0 の 2 桁以上 / 桁区切りの崩れは `None`。
pub(crate) fn parse_numeral(surface: &str) -> Option<(Vec<Word>, usize)> {
    let chars: Vec<(usize, char)> = surface.char_indices().collect();
    let mut words: Vec<Word> = Vec::new();
    let mut i = 0;
    let mut prev_plain_digit = false;
    while i < chars.len() {
        let c = chars[i].1;
        if digit_value(c).is_some() {
            // 算用数字の列 (3 桁ごとの , を許す)
            let mut j = i;
            let mut digits: Vec<u32> = Vec::new();
            let mut since_comma: Option<usize> = None;
            while j < chars.len() {
                let cj = chars[j].1;
                if let Some(d) = digit_value(cj) {
                    digits.push(d);
                    since_comma = since_comma.map(|n| n + 1);
                    j += 1;
                } else if matches!(cj, ',' | '\u{FF0C}')
                    && !digits.is_empty()
                    && j + 1 < chars.len()
                    && digit_value(chars[j + 1].1).is_some()
                {
                    if since_comma.is_some_and(|n| n != 3)
                        || (since_comma.is_none() && digits.len() > 3)
                    {
                        return None;
                    }
                    since_comma = Some(0);
                    j += 1;
                } else {
                    break;
                }
            }
            if since_comma.is_some_and(|n| n != 3) {
                return None;
            }
            if digits.len() == 1 {
                if digits[0] == 0 {
                    return None;
                }
                words.push(Word {
                    surface: kanji_digit_of(digits[0])?.to_string(),
                    generated: false,
                });
            } else {
                words.extend(words_of_digits(&digits)?);
            }
            prev_plain_digit = false;
            i = j;
            continue;
        }
        if let Some(d) = kanji_digit(c) {
            if prev_plain_digit {
                return None;
            }
            words.push(Word {
                surface: d.to_string(),
                generated: false,
            });
            prev_plain_digit = true;
            i += 1;
            continue;
        }
        if let Some(u) = kanji_unit(c) {
            words.push(Word {
                surface: u.to_string(),
                generated: false,
            });
            prev_plain_digit = false;
            i += 1;
            continue;
        }
        break;
    }
    if words.is_empty() {
        return None;
    }
    let used = chars.get(i).map_or(surface.len(), |&(b, _)| b);
    Some((words, used))
}

fn kanji_digit_of(d: u32) -> Option<&'static str> {
    ["", "一", "二", "三", "四", "五", "六", "七", "八", "九"]
        .get(d as usize)
        .copied()
        .filter(|s| !s.is_empty())
}

/// 語単体の (拍数, 核, 結合型)
fn base(word: &Word) -> Option<(usize, usize, &'static str)> {
    let unit_rule = if word.generated { "*" } else { "C3" };
    Some(match word.surface.as_str() {
        "一" => (2, 2, "C3"),
        "二" => (1, 1, "C3"),
        "三" => (2, 0, "C3"),
        "四" => (2, 1, "C1"),
        "五" => (1, 1, "C3"),
        "六" => (2, 2, "C3"),
        "七" => (2, 1, "C3"),
        "八" => (2, 2, "C3"),
        "九" => (2, 1, "C3"),
        "十" => (2, 1, unit_rule),
        "百" => (2, 2, unit_rule),
        "千" => (2, 1, unit_rule),
        "万" => (2, 1, unit_rule),
        "億" => (2, 1, unit_rule),
        "兆" => (2, 1, "C3"),
        _ => return None,
    })
}

fn lookup_conv(table: t::ConvTable, key: &str, prev: &str) -> Option<(usize, usize)> {
    for (keys, entries) in table {
        if keys.contains(&key) {
            return entries
                .iter()
                .find(|e| e.0 == prev)
                .map(|e| (usize::from(e.1), usize::from(e.2)));
        }
    }
    None
}

/// 数字を OpenJTalk と同じ手順で整える。
///
/// - `counter` = 直後の助数詞の (表記, IPADIC の読み (カタカナ))。 数字の直後が助数詞 (接尾 / 副詞可能) でなければ `None`
/// - `after_gatsu` = 直前の語が 「月」 を含む (一日 を ツイタチ にするため)
///
/// 数字の語が OpenJTalk の表に無い (想定外の) 並びなら `None`。
pub(crate) fn stage(
    words: &[Word],
    counter: Option<(&str, &str)>,
    after_gatsu: bool,
) -> Option<Staged> {
    let mut nodes: Vec<DigitNode> = Vec::with_capacity(words.len());
    for w in words {
        let (morae, accent, rules) = base(w)?;
        nodes.push(DigitNode {
            surface: w.surface.clone(),
            morae,
            accent,
            rules,
            chain_flag: None,
            merged: false,
        });
    }
    let mut counter_flag = None;
    // 1) 助数詞による数字の読みの変化 (六 + 本 = ロッ …) と 連結フラグ
    if let Some((cs, _)) = counter {
        if let Some(last) = nodes.last_mut() {
            if let Some((m, a)) = lookup_conv(t::CLASS1, cs, &last.surface) {
                last.morae = m;
                last.accent = a;
            }
            last.chain_flag = Some(false);
        }
        counter_flag = Some(true);
    }
    // 2) 数字どうしの連結 (一 + 万 は連結 / 百 + 二 は別) と 百 / 千 の前の数字の読み (六百 = ロッピャク)
    for i in 1..nodes.len() {
        let (prev, cur) = (nodes[i - 1].surface.clone(), nodes[i].surface.clone());
        if t::LIST4.contains(&prev.as_str()) && t::LIST5.contains(&cur.as_str()) {
            nodes[i - 1].chain_flag = Some(false);
            nodes[i].chain_flag = Some(true);
        } else if t::LIST5.contains(&prev.as_str()) && t::LIST4.contains(&cur.as_str()) {
            nodes[i].chain_flag = Some(false);
        }
        if let Some((m, a)) = lookup_conv(t::DIGIT_CONV, &cur, &prev) {
            nodes[i - 1].morae = m;
            nodes[i - 1].accent = a;
        }
    }
    let mut counter_merged = false;
    if let Some((cs, cr)) = counter {
        // 3) 数字 1 語 + 助数詞: 一 / 二 + 月 などは ヒト / フタ、 一人 / 二人 / N日 / N日間 は 1 語にする
        if nodes.len() == 1 {
            let only_surface = nodes[0].surface.clone();
            if t::CLASS3.iter().any(|(k, rs)| *k == cs && rs.contains(&cr)) {
                if let Some(&(_, m, a)) = t::CLASS3_CONV.iter().find(|e| e.0 == only_surface) {
                    nodes[0].morae = usize::from(m);
                    nodes[0].accent = usize::from(a);
                }
            }
            let merged_table: Option<&[t::Merged]> = match cs {
                "人" => Some(t::OTHERS_PERSON),
                "日" => Some(t::OTHERS_DAY),
                "日間" => Some(t::OTHERS_DAYS),
                _ => None,
            };
            if let Some(entries) = merged_table {
                if let Some(e) = entries.iter().find(|e| e.0 == only_surface) {
                    let (surface, accent, morae) =
                        if after_gatsu && only_surface == "一" && cs == "日" {
                            ("一日", 4, 4)
                        } else {
                            (e.1, e.3, e.4)
                        };
                    nodes = vec![merged_node(
                        surface,
                        usize::from(accent),
                        usize::from(morae),
                        None,
                    )];
                    counter_merged = true;
                    counter_flag = None;
                }
            }
        }
        // 4) 十四日 / 二十日 / 二十四日 (間つき) は 1 語 (二十四日 は 二十 + 四日)
        if !counter_merged && (cs == "日" || cs == "日間") {
            let kan = cs == "日間";
            let surfaces: Vec<&str> = nodes.iter().map(|n| n.surface.as_str()).collect();
            let special: Option<Vec<DigitNode>> = match surfaces.as_slice() {
                ["十", "四"] => Some(vec![merged_node(
                    if kan { "十四日間" } else { "十四日" },
                    if kan { 5 } else { 1 },
                    if kan { 7 } else { 5 },
                    None,
                )]),
                ["二", "十"] => Some(vec![merged_node(
                    if kan { "二十日間" } else { "二十日" },
                    if kan { 3 } else { 0 },
                    if kan { 5 } else { 3 },
                    None,
                )]),
                ["二", "十", "四"] => Some(vec![
                    DigitNode {
                        surface: "二十".to_string(),
                        morae: 3,
                        accent: 1,
                        rules: "*",
                        chain_flag: None,
                        merged: true,
                    },
                    merged_node(
                        if kan { "四日間" } else { "四日" },
                        if kan { 3 } else { 0 },
                        if kan { 5 } else { 3 },
                        Some(false),
                    ),
                ]),
                _ => None,
            };
            if let Some(n) = special {
                nodes = n;
                counter_merged = true;
                counter_flag = None;
            }
        }
    }
    Some(Staged {
        nodes,
        counter_merged,
        counter_flag,
    })
}

fn merged_node(surface: &str, accent: usize, morae: usize, chain_flag: Option<bool>) -> DigitNode {
    DigitNode {
        surface: surface.to_string(),
        morae,
        accent,
        rules: "*",
        chain_flag,
        merged: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(s: &Staged) -> Vec<(String, usize, usize, Option<bool>)> {
        s.nodes
            .iter()
            .map(|n| (n.surface.clone(), n.morae, n.accent, n.chain_flag))
            .collect()
    }

    fn staged(surface: &str, counter: Option<(&str, &str)>) -> Staged {
        let (words, used) = parse_numeral(surface).unwrap();
        assert!(used > 0);
        stage(&words, counter, false).unwrap()
    }

    #[test]
    fn parse_splits_place_values() {
        let w = |s: &str| {
            parse_numeral(s)
                .unwrap()
                .0
                .into_iter()
                .map(|w| w.surface)
                .collect::<Vec<_>>()
        };
        assert_eq!(w("2000円"), ["二", "千"]);
        assert_eq!(w("123円"), ["百", "二", "十", "三"]);
        assert_eq!(w("10000円"), ["一", "万"]);
        assert_eq!(w("15人"), ["十", "五"]);
        assert_eq!(w("100円"), ["百"]);
        assert_eq!(w("2万3千円"), ["二", "万", "三", "千"]);
        assert_eq!(w("1,234円"), ["千", "二", "百", "三", "十", "四"]);
        assert_eq!(w("３人"), ["三"]);
        assert_eq!(w("二十人"), ["二", "十"]);
        assert_eq!(parse_numeral("2千円").unwrap().1, "2千".len());
    }

    #[test]
    fn parse_rejects_what_openjtalk_reads_digit_by_digit_or_cannot_place() {
        assert!(parse_numeral("一二三").is_none());
        assert!(parse_numeral("0円").is_none());
        assert!(parse_numeral("007").is_none());
        assert!(parse_numeral("12,34円").is_none());
        assert!(parse_numeral("1234567890123456789円").is_none());
        assert!(parse_numeral("円").is_none());
    }

    #[test]
    fn stage_matches_openjtalk_for_money_and_counts() {
        // 2000円: 二(1/1 C3) 千(2/1 *) + 円
        let s = staged("2000円", Some(("円", "エン")));
        assert_eq!(
            shape(&s),
            [
                ("二".into(), 1, 1, Some(false)),
                ("千".into(), 2, 1, Some(true))
            ]
        );
        assert_eq!(s.counter_flag, Some(true));
        // 300円: 三(2/0) 百(2/2)
        let s = staged("300円", Some(("円", "エン")));
        assert_eq!(
            shape(&s),
            [
                ("三".into(), 2, 0, Some(false)),
                ("百".into(), 2, 2, Some(true))
            ]
        );
        // 9時: 九 が ク (1 拍 平板)
        let s = staged("9時", Some(("時", "ジ")));
        assert_eq!(shape(&s), [("九".into(), 1, 0, Some(false))]);
        // 6本 / 8分: ロッ / ハッ (2 拍 1 型)
        assert_eq!(
            shape(&staged("6本", Some(("本", "ホン")))),
            [("六".into(), 2, 1, Some(false))]
        );
        assert_eq!(
            shape(&staged("8分", Some(("分", "フン")))),
            [("八".into(), 2, 1, Some(false))]
        );
        // 四円 は ヨ (1 拍 平板)
        assert_eq!(
            shape(&staged("4円", Some(("円", "エン")))),
            [("四".into(), 1, 0, Some(false))]
        );
        // 助数詞が無い数
        let s = staged("100", None);
        assert_eq!(shape(&s), [("百".into(), 2, 2, None)]);
        assert_eq!(s.counter_flag, None);
    }

    #[test]
    fn stage_merges_persons_and_days() {
        let s = staged("1人", Some(("人", "ニン")));
        assert!(s.counter_merged);
        assert_eq!(shape(&s), [("一人".into(), 3, 2, None)]);
        assert_eq!(
            shape(&staged("2日", Some(("日", "ニチ")))),
            [("二日".into(), 3, 0, None)]
        );
        assert_eq!(
            shape(&staged("9日", Some(("日", "ニチ")))),
            [("九日".into(), 4, 0, None)]
        );
        // 12月1日 の 1日 は ツイタチ
        let (words, _) = parse_numeral("1日").unwrap();
        let s = stage(&words, Some(("日", "ニチ")), true).unwrap();
        assert_eq!(shape(&s), [("一日".into(), 4, 4, None)]);
        // 二十日 / 十四日 / 二十四日 (二十 + 四日)
        assert_eq!(
            shape(&staged("20日", Some(("日", "ニチ")))),
            [("二十日".into(), 3, 0, None)]
        );
        assert_eq!(
            shape(&staged("14日", Some(("日", "ニチ")))),
            [("十四日".into(), 5, 1, None)]
        );
        let s = staged("24日", Some(("日", "ニチ")));
        assert_eq!(
            shape(&s),
            [
                ("二十".into(), 3, 1, None),
                ("四日".into(), 3, 0, Some(false))
            ]
        );
        // 一 / 二 + 月 などの訓の助数詞は ヒト / フタ (2 拍 平板)
        let s = staged("1月", Some(("月", "ツキ")));
        assert_eq!(shape(&s), [("一".into(), 2, 0, Some(false))]);
        // 人 以外の 1 つ目は融合しない (ニ + 回)
        assert!(!staged("2回", Some(("回", "カイ"))).counter_merged);
    }

    #[test]
    fn stage_changes_the_digit_before_hyaku_and_sen() {
        // 六百 = ロッピャク (ロッ 2 拍 平板)、 八千 = ハッセン、 一千 = イッセン、 十千
        let s = staged("600円", Some(("円", "エン")));
        assert_eq!(shape(&s)[0], ("六".into(), 2, 0, Some(false)));
        let s = staged("8000円", Some(("円", "エン")));
        assert_eq!(shape(&s)[0], ("八".into(), 2, 0, Some(false)));
        // 万 の直前の一 は変えない
        let s = staged("10000円", Some(("円", "エン")));
        assert_eq!(shape(&s)[0], ("一".into(), 2, 2, Some(false)));
    }

    #[test]
    fn to_node_marks_merged_words_as_adverbial_nouns() {
        let s = staged("1人", Some(("人", "ニン")));
        assert_eq!(
            s.nodes[0].to_node().pos,
            Pos::Meishi(MeishiKind::FukushiKanou)
        );
        let s = staged("5円", Some(("円", "エン")));
        let n = s.nodes[0].to_node();
        assert_eq!(n.pos, Pos::Meishi(MeishiKind::Kazu));
        assert_eq!(n.chain_flag, Some(false));
    }

    #[test]
    fn every_kanji_digit_and_unit_is_parsed_as_its_own_word() {
        for c in "一二三四五六七八九".chars() {
            let (words, used) = parse_numeral(&format!("{c}円")).unwrap();
            assert_eq!(words.len(), 1, "{c}");
            assert_eq!(words[0].surface, c.to_string());
            assert_eq!(used, c.len_utf8());
        }
        for c in "十百千万億兆".chars() {
            let (words, used) = parse_numeral(&format!("三{c}円")).unwrap();
            let s: Vec<&str> = words.iter().map(|w| w.surface.as_str()).collect();
            assert_eq!(s, ["三", &c.to_string()], "{c}");
            assert_eq!(used, "三".len() + c.len_utf8());
        }
        // 一字目が単位でも読める (十円 / 百円)
        assert_eq!(parse_numeral("百円").unwrap().0[0].surface, "百");
    }

    #[test]
    fn sixteen_digits_are_the_limit() {
        let (words, _) = parse_numeral("1234567890123456円").unwrap();
        assert!(words.iter().any(|w| w.surface == "兆"));
        assert!(parse_numeral("12345678901234567円").is_none());
        // 先頭が 0 / 全部 0 の複数桁
        assert!(parse_numeral("00円").is_none());
        assert!(parse_numeral("0123円").is_none());
    }

    #[test]
    fn comma_groups_must_be_three_digits() {
        assert!(parse_numeral("1,234円").is_some());
        assert!(parse_numeral("12,345,678円").is_some());
        assert!(parse_numeral("1,23円").is_none());
        assert!(parse_numeral("1,2345円").is_none());
        assert!(parse_numeral("1234,567円").is_none());
        // 全角のカンマも同じ
        assert!(parse_numeral("1\u{FF0C}234円").is_some());
        // 末尾のカンマは数に含めない
        assert_eq!(parse_numeral("5,円").unwrap().1, 1);
    }

    #[test]
    fn base_values_of_every_numeral_word_match_openjtalk() {
        // (語, 拍数, 核, 結合型) — jpreprocess の辞書 / 数字列の語
        let table = [
            ("一", 2, 2, "C3"),
            ("二", 1, 1, "C3"),
            ("三", 2, 0, "C3"),
            ("四", 2, 1, "C1"),
            ("五", 1, 1, "C3"),
            ("六", 2, 2, "C3"),
            ("七", 2, 1, "C3"),
            ("八", 2, 2, "C3"),
            ("九", 2, 1, "C3"),
            ("十", 2, 1, "C3"),
            ("百", 2, 2, "C3"),
            ("千", 2, 1, "C3"),
            ("万", 2, 1, "C3"),
            ("億", 2, 1, "C3"),
            ("兆", 2, 1, "C3"),
        ];
        for (s, m, a, r) in table {
            let w = Word {
                surface: s.to_string(),
                generated: false,
            };
            assert_eq!(base(&w), Some((m, a, r)), "{s}");
        }
        // 位取りで作った語 (算用数字の桁) は十・百・千・万・億の結合型が `*`、 兆だけ C3
        for s in ["十", "百", "千", "万", "億"] {
            let w = Word {
                surface: s.to_string(),
                generated: true,
            };
            assert_eq!(base(&w).unwrap().2, "*", "{s}");
        }
        let w = Word {
            surface: "兆".to_string(),
            generated: true,
        };
        assert_eq!(base(&w).unwrap().2, "C3");
        let w = Word {
            surface: "x".to_string(),
            generated: false,
        };
        assert_eq!(base(&w), None);
    }

    #[test]
    fn place_value_words_for_every_position() {
        let w = |s: &str| {
            parse_numeral(s)
                .unwrap()
                .0
                .into_iter()
                .map(|w| w.surface)
                .collect::<Vec<_>>()
        };
        assert_eq!(w("11円"), ["十", "一"]);
        assert_eq!(w("101円"), ["百", "一"]);
        assert_eq!(w("1001円"), ["千", "一"]);
        assert_eq!(w("20000円"), ["二", "万"]);
        assert_eq!(w("100000000円"), ["一", "億"]);
        assert_eq!(w("1000000000000円"), ["一", "兆"]);
        assert_eq!(
            w("12345円"),
            ["一", "万", "二", "千", "三", "百", "四", "十", "五"]
        );
        assert_eq!(w("30005円"), ["三", "万", "五"]);
    }
}

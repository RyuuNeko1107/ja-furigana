//! 人名の専用経路 ([`PersonNamePass`])。
//!
//! 姓の表 (`role = "name_sei"`) と名の表 (`role = "name_mei"`) を、 **人名の手がかりがある所でだけ** 引く
//! post-pass。 通常の辞書引きには混ぜない (水田 = みずた を一般語の辞書に入れると、 田んぼの水田 = すいでん を壊す)。
//!
//! 手がかり = 直後の敬称 (さん / くん / 氏 …) または肩書き (選手 / 監督 / 先生 …)。
//! 手がかりの直前に並ぶ漢字 token 列を結合し、 「姓 + 名」 または 「姓だけ」 として表で引けたら、
//! 表の読みに組み直す (「水田 | 選手」 → みずた、 「水田 | 雄一郎 | さん」 → みずた | ゆういちろう)。
//! 組み直す前の読みは `alternatives` に残す。
//!
//! 判定は [`cue_score`] に分けてある。 いまは規則 (手がかり語の表) だが、 学習済みの重みの表に
//! 差し替えられる (実行時は決定論のまま)。
//!
//! 表は dict repo の `core/names/` (NDL 典拠由来)。 形式は `[entries]` の `"表記" = "読み"`。

use crate::dict::Dict;
use crate::kana::{hira_to_kata, is_kanji_char};
use crate::scoring::analyze::{AlternativeReading, Token};
use crate::scoring::postpass::ReadingPostPass;

/// 敬称 (読み方の揺れがない)。
// 様 / 君 / 殿 は王子様・神様・諸君のような一般語と衝突するので手がかりにしない (実コメント A/B で確認)。
const HONORIFICS: &[&str] = &["さん", "ちゃん", "くん", "氏"];

/// 肩書き・職名。 人名の直後に来る語 (単独の語としても読みが決まっているもの)。
const TITLES: &[&str] = &[
    "選手",
    "監督",
    "先生",
    "社長",
    "議員",
    "首相",
    "大臣",
    "容疑者",
    "教授",
    "部長",
    "課長",
    "会長",
    "代表",
    "知事",
    "市長",
    "町長",
    "村長",
    "総理",
    "隊長",
    "校長",
    "医師",
    "投手",
    "騎手",
    "棋士",
    "名人",
    "記者",
    "弁護士",
    "被告",
    "被害者",
    "師匠",
    "先輩",
    "後輩",
    "コーチ",
    "キャプテン",
    "主将",
    "団長",
    "署長",
    "長官",
    "大統領",
    "元帥",
    "将軍",
    "監察官",
    "社員",
    "店長",
    "支配人",
    "理事長",
    "委員長",
    "幹事長",
    "党首",
    "総裁",
];

/// 結合した名前として許す最大文字数 (姓 1〜3 + 名 1〜4)。 漢字の偶然の連続での誤爆を避ける。
const MAX_RUN_CHARS: usize = 7;
/// 手がかりの前に遡る token 数の上限。
const MAX_RUN_TOKENS: usize = 3;
/// 組み直す前の読みを `alternatives` に残すときの重み。
const ALT_WEIGHT: u8 = 50;

/// 表のファイル (`[entries]` の `"表記" = "読み"` 行) を (表記, 読み) の列にする。 行単位で読む。
pub(crate) fn parse_name_table(content: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut in_entries = false;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') && !t.starts_with("[\"") && t.ends_with(']') {
            in_entries = t == "[entries]";
            continue;
        }
        if !in_entries || !t.starts_with('"') {
            continue;
        }
        let mut parts = t.splitn(2, '=');
        let (Some(k), Some(v)) = (parts.next(), parts.next()) else {
            continue;
        };
        let k = k.trim().trim_matches('"');
        let v = v.trim().trim_matches('"');
        if !k.is_empty() && !v.is_empty() {
            out.push((k.to_string(), v.to_string()));
        }
    }
    out
}

/// 表の読みは NDL の転写で、 づ を ず、 ぢ を じ と書く (望月 = モチズキ)。 現行の読みと比べるときは同一視し、
/// 違いがそれだけなら置き換えない (現行の づ を壊さない)。
fn same_reading(a: &str, b: &str) -> bool {
    let n = |s: &str| -> String {
        hira_to_kata(s)
            .chars()
            .map(|c| match c {
                'ヅ' => 'ズ',
                'ヂ' => 'ジ',
                c => c,
            })
            .collect()
    };
    n(a) == n(b)
}

fn all_kanji(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| is_kanji_char(c) || c == '々')
}

/// token の表記が手がかり語なら、 その点数 (1〜) を返す。 手がかりでなければ 0。
///
/// 手がかり語そのもの、 または手がかり語 + ひらがな (「選手たち」 「さんが」) だけを認める。
/// 「選手権」 のように後ろに漢字が続く語は手がかりにしない。
fn cue_score(surface: &str) -> u8 {
    for (list, score) in [(HONORIFICS, 2u8), (TITLES, 1u8)] {
        for cue in list {
            if let Some(rest) = surface.strip_prefix(cue) {
                if rest.chars().all(crate::kana::is_hiragana_char) {
                    return score;
                }
            }
        }
    }
    0
}

/// 手がかりの前の漢字列を表で引いた結果。
struct Parsed {
    sei: String,
    sei_reading: String,
    mei: Option<(String, String)>,
}

/// 人名の専用経路の post-pass。
pub struct PersonNamePass<'a> {
    dict: &'a Dict,
}

impl<'a> PersonNamePass<'a> {
    #[must_use]
    pub fn new(dict: &'a Dict) -> Self {
        Self { dict }
    }

    /// `run` (漢字列) を 「姓 + 名」 または 「姓だけ」 として表で引く。
    fn parse(&self, run: &str) -> Option<Parsed> {
        // 本屋さん / 八百屋さん のような店は人名にしない (姓の 本屋 = もとや と衝突する)。
        if run.ends_with('屋') {
            return None;
        }
        let chars: Vec<char> = run.chars().collect();
        let n = chars.len();
        // 姓 + 名: 姓は 2 字を優先 (水田 | 雄一郎)、 次に 3 字、 1 字。
        // 辞書にフルネームがあれば辞書が正 (江川卓 = えがわすぐる。 名の表は多数派の たかし)。
        let in_dict = self.dict.lookup_jukugo(run).is_some();
        // 2 字は姓だけとみる (久保 を 久 | 保 に割らない)。 姓 + 名は 3 字以上。
        for k in [2usize, 3, 1] {
            if in_dict || n < 3 || k >= n || n - k > 4 {
                continue;
            }
            let sei: String = chars[..k].iter().collect();
            let mei: String = chars[k..].iter().collect();
            if let (Some(s), Some(m)) = (
                self.dict.lookup_name_sei(&sei),
                self.dict.lookup_name_mei(&mei),
            ) {
                return Some(Parsed {
                    sei,
                    sei_reading: s.to_string(),
                    mei: Some((mei, m.to_string())),
                });
            }
        }
        // 姓だけ (2 字以上。 1 字は 「鳥さん」 のような一般語と区別できないので対象外)
        if n >= 2 {
            if let Some(s) = self.dict.lookup_name_sei(run) {
                return Some(Parsed {
                    sei: run.to_string(),
                    sei_reading: s.to_string(),
                    mei: None,
                });
            }
        }
        None
    }
}

impl ReadingPostPass for PersonNamePass<'_> {
    fn apply(&self, tokens: &mut Vec<Token>) {
        let mut c = 1;
        while c < tokens.len() {
            if cue_score(&tokens[c].surface) == 0 {
                c += 1;
                continue;
            }
            // 手がかりの直前から遡って、 漢字だけの token 列の候補を作り、 表で引けた最良のものを採る。
            let mut best: Option<(usize, Parsed)> = None;
            let mut run = String::new();
            let lo = c.saturating_sub(MAX_RUN_TOKENS);
            for s in (lo..c).rev() {
                if !all_kanji(&tokens[s].surface) {
                    break;
                }
                run.insert_str(0, &tokens[s].surface);
                if run.chars().count() > MAX_RUN_CHARS {
                    break;
                }
                if let Some(p) = self.parse(&run) {
                    let better = match &best {
                        None => true,
                        // 姓 + 名 が引けたものを優先し、 同じなら長いほう
                        Some((_, b)) => {
                            (p.mei.is_some(), run.chars().count())
                                > (
                                    b.mei.is_some(),
                                    b.sei.chars().count()
                                        + b.mei.as_ref().map_or(0, |m| m.0.chars().count()),
                                )
                        }
                    };
                    if better {
                        best = Some((s, p));
                    }
                }
            }
            let Some((s, p)) = best else {
                c += 1;
                continue;
            };
            let old: String = tokens[s..c]
                .iter()
                .map(|t| hira_to_kata(&t.reading))
                .collect();
            let new_reading = format!(
                "{}{}",
                hira_to_kata(&p.sei_reading),
                p.mei.as_ref().map_or(String::new(), |m| hira_to_kata(&m.1))
            );
            let surfaces_now: Vec<&str> = tokens[s..c].iter().map(|t| t.surface.as_str()).collect();
            let boundaries_ok = match &p.mei {
                Some((mei, _)) => surfaces_now == [p.sei.as_str(), mei.as_str()],
                None => surfaces_now == [p.sei.as_str()],
            };
            if same_reading(&old, &new_reading) && boundaries_ok {
                c += 1;
                continue;
            }
            let start = tokens[s].range.start;
            let end = tokens[c - 1].range.end;
            let alt = AlternativeReading {
                reading: old,
                sense: None,
                weight: ALT_WEIGHT,
            };
            let mut replacement = Vec::with_capacity(2);
            let mut head = tokens[s].clone();
            head.surface = p.sei.clone();
            head.reading = hira_to_kata(&p.sei_reading);
            head.range = start..start + p.sei.len();
            head.accent_phrases = Vec::new();
            head.ambiguous = false;
            head.alternatives = vec![alt];
            head.is_name = true;
            replacement.push(head);
            if let Some((mei, mei_reading)) = &p.mei {
                let mut tail = tokens[c - 1].clone();
                tail.surface = mei.clone();
                tail.reading = hira_to_kata(mei_reading);
                tail.range = start + p.sei.len()..end;
                tail.accent_phrases = Vec::new();
                tail.ambiguous = false;
                tail.alternatives = Vec::new();
                tail.is_name = true;
                replacement.push(tail);
            }
            let added = replacement.len();
            tokens.splice(s..c, replacement);
            c = s + added + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    fn token(surface: &str, reading: &str, range: Range<usize>) -> Token {
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

    fn dict() -> Dict {
        let mut d = Dict::new();
        d.insert_name(true, "水田", "みずた");
        d.insert_name(true, "橋本", "はしもと");
        d.insert_name(true, "鳥", "とり");
        d.insert_name(false, "雄一郎", "ゆういちろう");
        d.insert_name(false, "環奈", "かんな");
        d
    }

    #[test]
    fn surname_with_title_is_read_as_name() {
        let d = dict();
        let mut t = vec![
            token("水田", "スイデン", 0..6),
            token("選手", "センシュ", 6..12),
            token("が", "ガ", 12..15),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t[0].reading, "ミズタ");
        assert!(t[0].is_name);
        assert_eq!(t[0].alternatives[0].reading, "スイデン");
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn surname_and_given_with_honorific_is_split() {
        let d = dict();
        // 水田 | 雄一郎 | さん (Lindera が 雄一郎 を 1 字ずつ割った想定も含め、 結合して引く)
        let mut t = vec![
            token("水田", "スイデン", 0..6),
            token("雄", "ユウ", 6..9),
            token("一郎", "イチロウ", 9..15),
            token("さん", "サン", 15..21),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        let surfaces: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
        assert_eq!(surfaces, ["水田", "雄一郎", "さん"]);
        assert_eq!(t[0].reading, "ミズタ");
        assert_eq!(t[1].reading, "ユウイチロウ");
        assert_eq!(t[1].range, 6..15);
    }

    #[test]
    fn no_cue_leaves_common_word_alone() {
        let d = dict();
        let mut t = vec![
            token("水田", "スイデン", 0..6),
            token("に", "ニ", 6..9),
            token("水", "ミズ", 9..12),
        ];
        let before: Vec<String> = t.iter().map(|x| x.reading.clone()).collect();
        PersonNamePass::new(&d).apply(&mut t);
        let after: Vec<String> = t.iter().map(|x| x.reading.clone()).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn compound_title_is_not_a_cue() {
        let d = dict();
        // 選手権 は肩書きではない (水田選手権 は人名にしない)
        let mut t = vec![
            token("水田", "スイデン", 0..6),
            token("選手権", "センシュケン", 6..15),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t[0].reading, "スイデン");
    }

    #[test]
    fn single_char_surname_alone_is_not_converted() {
        let d = dict();
        // 鳥さん は一般語と区別できないので触らない
        let mut t = vec![token("鳥", "チョウ", 0..3), token("さん", "サン", 3..9)];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t[0].reading, "チョウ");
    }

    #[test]
    fn full_name_in_dict_wins_over_tables() {
        let mut d = dict();
        d.insert_name(true, "江川", "えがわ");
        d.insert_name(false, "卓", "たかし");
        d.insert("江川卓", "エガワスグル");
        let mut t = vec![
            token("江川卓", "エガワスグル", 0..9),
            token("投手", "トウシュ", 9..15),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t[0].reading, "エガワスグル");
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn dzu_zu_difference_alone_is_not_replaced() {
        let mut d = dict();
        d.insert_name(true, "望月", "もちずき");
        let mut t = vec![
            token("望月", "モチヅキ", 0..6),
            token("先生", "センセイ", 6..12),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t[0].reading, "モチヅキ", "づ/ず だけの違いは現行を残す");
        assert!(!t[0].is_name);
    }

    #[test]
    fn two_char_run_is_surname_only_not_split() {
        let mut d = dict();
        d.insert_name(true, "久", "ひさ");
        d.insert_name(false, "保", "たもつ");
        d.insert_name(true, "久保", "くぼ");
        let mut t = vec![token("久保", "クボ", 0..6), token("くん", "クン", 6..12)];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t[0].reading, "クボ");
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn shop_ya_is_not_a_name() {
        let mut d = dict();
        d.insert_name(true, "本屋", "もとや");
        let mut t = vec![token("本屋", "ホンヤ", 0..6), token("さん", "サン", 6..12)];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t[0].reading, "ホンヤ");
    }

    #[test]
    fn sama_is_not_a_cue() {
        let mut d = dict();
        d.insert_name(true, "王子", "おうし");
        let mut t = vec![token("王子", "オウジ", 0..6), token("様", "サマ", 6..9)];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t[0].reading, "オウジ");
    }

    #[test]
    fn already_correct_is_untouched() {
        let d = dict();
        let mut t = vec![
            token("橋本", "ハシモト", 0..6),
            token("環奈", "カンナ", 6..12),
            token("さん", "サン", 12..18),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        assert!(!t[0].is_name, "読みも境界も同じなら触らない");
        assert!(t[0].alternatives.is_empty());
    }

    #[test]
    fn parse_table_reads_entries_only() {
        let c = "[meta]\nschema_version = \"2\"\nrole = \"name_sei\"\n\n[entries]\n\"水田\" = \"みずた\"\n\"橋本\" = \"はしもと\"\n";
        let v = parse_name_table(c);
        assert_eq!(
            v,
            vec![
                ("水田".to_string(), "みずた".to_string()),
                ("橋本".to_string(), "はしもと".to_string())
            ]
        );
    }
}

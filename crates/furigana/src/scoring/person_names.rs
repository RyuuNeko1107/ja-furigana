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
const MAX_RUN_TOKENS: usize = 5;
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

fn is_kana(c: char) -> bool {
    matches!(c, 'ぁ'..='ゖ' | 'ァ'..='ヺ')
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

/// 手がかり語 (敬称と肩書き) の先頭の字
static CUE_FIRST_CHARS: once_cell::sync::Lazy<crate::fxmap::FxSet<char>> =
    once_cell::sync::Lazy::new(|| {
        HONORIFICS
            .iter()
            .chain(TITLES.iter())
            .filter_map(|c| c.chars().next())
            .collect()
    });

/// 肩書き語の先頭の字 (選 / 監 / 先 / 社 …)。 肩書きの候補になる token を絞る前判定に使う。
static TITLE_FIRST_CHARS: once_cell::sync::Lazy<crate::fxmap::FxSet<char>> =
    once_cell::sync::Lazy::new(|| TITLES.iter().filter_map(|c| c.chars().next()).collect());

/// token が手がかり語 (敬称 / 肩書き) か、 それを含む形 (原さん / 利さ|ん の ん) でありうるか。
/// 手がかりが 1 つも無い行 (ほとんどの行) を、 本処理に入れずに素通りさせる前判定。
/// 本処理の判定 ([`cue_score`] / 敬称で終わる token / 敬称をまたぐ token) より必ず広い。
/// 大半の token は 先頭の字の表引きと 末尾の字の比較だけで終わるようにしてある
fn token_may_be_cue(surface: &str) -> bool {
    let Some(first) = surface.chars().next() else {
        return false;
    };
    // 敬称の後半だけの token (利さ|ん / ちゃ|ん / ち|ゃん)
    if surface == "ん" || surface == "ゃん" {
        return true;
    }
    // 手がかり語で始まる (さん / ちゃん / くん / 氏 / 肩書き)
    if CUE_FIRST_CHARS.contains(&first)
        && (TITLE_FIRST_CHARS.contains(&first) || HONORIFICS.iter().any(|h| surface.starts_with(h)))
    {
        return true;
    }
    // 敬称で終わる (原さん)
    surface.ends_with(['ん', '氏']) && HONORIFICS.iter().any(|h| surface.ends_with(h))
}

/// 手がかり語になりうる token が 1 つでもあるか。
fn may_have_cue(tokens: &[Token]) -> bool {
    tokens.iter().any(|t| token_may_be_cue(&t.surface))
}

/// 手がかりの前の漢字列を表で引いた結果。
struct Parsed {
    sei: String,
    sei_reading: String,
    mei: Option<(String, String)>,
}

/// 呼び出し側が渡せる 「これは人名か」 のヒント。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NameHint {
    /// 既定。 直後の敬称・肩書きを手がかりに、 人名らしいところだけ人名として読む。
    #[default]
    Auto,
    /// 入力は人名として読む (ユーザー名・選手名簿の欄など)。 手がかりは要らず、 1 字の姓も許す。
    Name,
    /// 人名の専用経路を使わない。
    NotName,
}

/// 人名の専用経路の post-pass。
pub struct PersonNamePass<'a> {
    dict: &'a Dict,
    hint: NameHint,
}

impl<'a> PersonNamePass<'a> {
    #[must_use]
    pub fn new(dict: &'a Dict) -> Self {
        Self {
            dict,
            hint: NameHint::Auto,
        }
    }

    /// ヒントを指定する。
    #[must_use]
    pub fn with_hint(mut self, hint: NameHint) -> Self {
        self.hint = hint;
        self
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
        // 3 字以上で、 そのまま姓の表にあるもの (太田原 = おおたわら) は、 姓 + 名に割る前に採る
        // (割ると 太田 | 原 になり、 姓の表の読みに届かない)。
        if n >= 3 {
            if let Some(s) = self.dict.lookup_name_sei(run) {
                return Some(Parsed {
                    sei: run.to_string(),
                    sei_reading: s.to_string(),
                    mei: None,
                });
            }
        }
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
        // 姓だけ (2 字以上。 1 字は 「鳥さん」 のような一般語と区別できないので対象外。
        // 呼び出し側が人名と言っている時 (NameHint::Name) だけ 1 字も許す)
        if n >= 2 || (n == 1 && self.hint == NameHint::Name) {
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

impl PersonNamePass<'_> {
    /// 「利さ | ん」 「利く | ん」 のように、 名の最後の漢字に敬称の頭のかな (さ / く) が付いて 1 token になり、
    /// 敬称が token をまたいでいる所を 「利 | さん」 に直す。 直前の漢字列が姓 (+ 名) の表に当たる時だけ。
    fn repair_kana_straddle(&self, tokens: &mut Vec<Token>) {
        let mut i = 1;
        while i < tokens.len() {
            // 借用だけで判定し、 該当する時だけ文字列を取る (大半の token は該当しない)
            let found = {
                let surface = tokens[i].surface.as_str();
                let head_len = surface
                    .char_indices()
                    .find(|(_, c)| is_kana(*c))
                    .map_or(surface.len(), |(i, _)| i);
                let (head, tail) = surface.split_at(head_len);
                let straddles = !head.is_empty()
                    && !tail.is_empty()
                    && all_kanji(head)
                    && head.chars().count() <= 3
                    && HONORIFICS.iter().any(|h| {
                        h.len() > tail.len()
                            && h.starts_with(tail)
                            && tokens
                                .get(i + 1)
                                .is_some_and(|n| n.surface == h[tail.len()..])
                    });
                straddles.then(|| (head.to_string(), tail.to_string()))
            };
            let Some((head, tail)) = found else {
                i += 1;
                continue;
            };
            let tail = tail.as_str();
            let honorific = format!("{}{}", tail, tokens[i + 1].surface);
            let tail_kata = hira_to_kata(tail);
            let reading = hira_to_kata(&tokens[i].reading);
            let Some(head_reading) = reading.strip_suffix(tail_kata.as_str()) else {
                i += 1;
                continue;
            };
            let mut run = head.clone();
            let mut hit = false;
            for s in (i.saturating_sub(MAX_RUN_TOKENS)..i).rev() {
                if !all_kanji(&tokens[s].surface) {
                    break;
                }
                run.insert_str(0, &tokens[s].surface);
                if run.chars().count() > MAX_RUN_CHARS {
                    break;
                }
                if self.parse(&run).is_some() {
                    hit = true;
                    break;
                }
            }
            if !hit {
                i += 1;
                continue;
            }
            let start = tokens[i].range.start;
            let end = tokens[i + 1].range.end;
            let mut left = tokens[i].clone();
            left.surface = head.clone();
            left.reading = head_reading.to_string();
            left.range = start..start + head.len();
            left.accent_phrases = Vec::new();
            let mut right = tokens[i + 1].clone();
            right.surface = honorific.clone();
            right.reading = hira_to_kata(&honorific);
            right.range = start + head.len()..end;
            right.accent_phrases = Vec::new();
            right.alternatives = Vec::new();
            right.ambiguous = false;
            right.is_name = false;
            tokens.splice(i..=i + 1, [left, right]);
            i += 2;
        }
    }

    /// 「太田 | 原さん」 のように、 形態素解析が 「名前の末尾の漢字 + 敬称」 を 1 token にした所を、
    /// 直前の漢字 token と合わせて姓 (または 姓 + 名) の表に当たる時だけ 「原 | さん」 に分ける。
    fn split_merged_honorific(&self, tokens: &mut Vec<Token>) {
        self.repair_kana_straddle(tokens);
        let mut i = 1;
        while i < tokens.len() {
            let Some((head, suffix)) = HONORIFICS.iter().find_map(|h| {
                tokens[i]
                    .surface
                    .strip_suffix(h)
                    .filter(|head| all_kanji(head) && head.chars().count() <= 3)
                    .map(|head| (head.to_string(), *h))
            }) else {
                i += 1;
                continue;
            };
            let suffix_kata = hira_to_kata(suffix);
            let reading = hira_to_kata(&tokens[i].reading);
            let Some(head_reading) = reading.strip_suffix(suffix_kata.as_str()) else {
                i += 1;
                continue;
            };
            // 直前の漢字 token 列 + head が表に当たるか
            let mut run = head.clone();
            let mut hit = false;
            for s in (i.saturating_sub(MAX_RUN_TOKENS)..i).rev() {
                if !all_kanji(&tokens[s].surface) {
                    break;
                }
                run.insert_str(0, &tokens[s].surface);
                if run.chars().count() > MAX_RUN_CHARS {
                    break;
                }
                if self.parse(&run).is_some() {
                    hit = true;
                    break;
                }
            }
            if !hit {
                i += 1;
                continue;
            }
            let start = tokens[i].range.start;
            let end = tokens[i].range.end;
            let mut left = tokens[i].clone();
            left.surface = head.clone();
            left.reading = head_reading.to_string();
            left.range = start..start + head.len();
            left.accent_phrases = Vec::new();
            let mut right = tokens[i].clone();
            right.surface = suffix.to_string();
            right.reading = suffix_kata;
            right.range = start + head.len()..end;
            right.accent_phrases = Vec::new();
            right.alternatives = Vec::new();
            right.ambiguous = false;
            right.is_name = false;
            tokens.splice(i..=i, [left, right]);
            i += 2;
        }
    }
}

impl ReadingPostPass for PersonNamePass<'_> {
    fn apply(&self, tokens: &mut Vec<Token>) {
        if self.hint == NameHint::NotName {
            return;
        }
        // 手がかりが無い行は何も起きない (Name ヒントは手がかり無しで引くので除く)
        if self.hint == NameHint::Auto && !may_have_cue(tokens) {
            return;
        }
        self.split_merged_honorific(tokens);
        let mut c = 1;
        while c <= tokens.len() {
            // 手がかりの位置 c (この token の直前までが名前候補)。 Name ヒントの時は、 漢字 token 列の
            // 終わり (次が漢字でない / 文末) を手がかりの代わりにする。
            let is_cue = match self.hint {
                NameHint::Name => {
                    all_kanji(&tokens[c - 1].surface)
                        && (c == tokens.len() || !all_kanji(&tokens[c].surface))
                }
                _ => c < tokens.len() && cue_score(&tokens[c].surface) > 0,
            };
            if !is_cue {
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
            // 形態素解析が 3 字以上の 1 語 (北海道) と読んだものを、 姓 + 名 に割り直さない。
            // (表の姓 + 名 は NDL の分割の癖で、 北海 | 道 のように一般語と重なる。 3 字全体が姓の表にある時は別)
            if p.mei.is_some()
                && c - s == 1
                && tokens[s].surface.chars().count() >= 3
                && self.hint != NameHint::Name
            {
                c += 1;
                continue;
            }
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
            head.why = Some("name: 人名表 (敬称・肩書きの手がかり)".to_string());
            let head_why = head.why.clone();
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
                tail.why = head_why;
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
            band: 0,
            why: None,
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

    /// 位置 (byte) を連続させて token 列を作る
    fn chain(parts: &[(&str, &str)]) -> Vec<Token> {
        let mut pos = 0;
        let mut out = Vec::new();
        for (s, r) in parts {
            out.push(token(s, r, pos..pos + s.len()));
            pos += s.len();
        }
        out
    }

    #[test]
    fn several_names_in_one_sentence_are_all_converted() {
        let d = dict();
        // 前置きの語 2 つ + 姓 名 敬称 (4 token → 3 token に減る) + 助詞 + 姓 敬称 + 姓 敬称
        let mut t = chain(&[
            ("ああ", "アア"),
            ("ええ", "エエ"),
            ("水田", "スイデン"),
            ("雄", "ユウ"),
            ("一郎", "イチロウ"),
            ("さん", "サン"),
            ("と", "ト"),
            ("水田", "スイデン"),
            ("さん", "サン"),
            ("と", "ト"),
            ("橋本", "ハシモト"),
            ("環奈", "タマキナ"),
            ("さん", "サン"),
        ]);
        PersonNamePass::new(&d).apply(&mut t);
        let surfaces: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
        assert_eq!(
            surfaces,
            [
                "ああ",
                "ええ",
                "水田",
                "雄一郎",
                "さん",
                "と",
                "水田",
                "さん",
                "と",
                "橋本",
                "環奈",
                "さん"
            ]
        );
        assert_eq!(t[2].reading, "ミズタ");
        assert_eq!(t[3].reading, "ユウイチロウ");
        assert_eq!(t[6].reading, "ミズタ");
        assert_eq!(t[9].reading, "ハシモト");
        assert_eq!(t[10].reading, "カンナ");
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
    fn name_hint_reads_without_cue_and_allows_single_char_surname() {
        let d = dict();
        let p = PersonNamePass::new(&d).with_hint(NameHint::Name);
        let mut t = vec![
            token("水田", "スイデン", 0..6),
            token("雄一郎", "ユウイチロウ", 6..15),
        ];
        p.apply(&mut t);
        assert_eq!(t[0].reading, "ミズタ");
        let mut one = vec![token("鳥", "チョウ", 0..3)];
        p.apply(&mut one);
        assert_eq!(one[0].reading, "トリ");
        // 文の中の人名でない部分には触れない (漢字列の外)
        let mut s = vec![
            token("広い", "ヒロイ", 0..6),
            token("水田", "スイデン", 6..12),
            token("に", "ニ", 12..15),
        ];
        p.apply(&mut s);
        assert_eq!(
            s[1].reading, "ミズタ",
            "Name ヒントでは文中の漢字列も人名として読む (欄が人名と言っている)"
        );
    }

    #[test]
    fn not_name_hint_disables_the_pass() {
        let d = dict();
        let p = PersonNamePass::new(&d).with_hint(NameHint::NotName);
        let mut t = vec![
            token("水田", "スイデン", 0..6),
            token("選手", "センシュ", 6..12),
        ];
        p.apply(&mut t);
        assert_eq!(t[0].reading, "スイデン");
    }

    #[test]
    fn whole_three_char_surname_beats_surname_plus_given_split() {
        let mut d = dict();
        d.insert_name(true, "太田", "おおた");
        d.insert_name(false, "原", "はら");
        d.insert_name(true, "太田原", "おおたわら");
        let mut t = vec![
            token("太田", "オオタ", 0..6),
            token("原", "ハラ", 6..9),
            token("さん", "サン", 9..15),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t[0].surface, "太田原");
        assert_eq!(t[0].reading, "オオタワラ");
    }

    #[test]
    fn merged_honorific_token_is_split_when_table_hits() {
        let mut d = dict();
        d.insert_name(true, "太田原", "おおたわら");
        // Lindera が 太田 | 原さん と切った所
        let mut t = vec![
            token("太田", "オオタ", 0..6),
            token("原さん", "ハラサン", 6..15),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        let s: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
        assert_eq!(s, ["太田原", "さん"]);
        assert_eq!(t[0].reading, "オオタワラ");
        assert_eq!(t[1].range, 9..15);
    }

    #[test]
    fn honorific_straddling_a_kana_tail_is_repaired() {
        let mut d = dict();
        d.insert_name(true, "田中", "たなか");
        d.insert_name(false, "弘利", "ひろとし");
        // 形態素解析が 田中 | 弘 | 利さ | ん と切った所
        let mut t = vec![
            token("田中", "タナカ", 0..6),
            token("弘", "ヒロ", 6..9),
            token("利さ", "リサ", 9..16),
            token("ん", "ン", 16..19),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        let s: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
        assert_eq!(s, ["田中", "弘利", "さん"]);
        assert_eq!(t[1].reading, "ヒロトシ");
    }

    #[test]
    fn merged_honorific_token_untouched_without_table_hit() {
        let d = dict();
        let mut t = vec![
            token("太田", "オオタ", 0..6),
            token("原さん", "ハラサン", 6..15),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t.len(), 2);
        assert_eq!(t[1].surface, "原さん");
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

    // ---- apply の境界・演算の検査 (mutation testing で見逃された所) ----

    #[test]
    fn name_hint_reads_consecutive_kanji_tokens_as_one_full_name() {
        // Name ヒントの手がかりは 「漢字 token 列の終わり」。 水田 | 雄一郎 の間 (次も漢字) は手がかりではなく、
        // 姓 + 名 を 1 回で組み直す (alternatives に 2 token 分の元の読みが残る)。
        let d = dict();
        let p = PersonNamePass::new(&d).with_hint(NameHint::Name);
        let mut t = vec![
            token("水田", "スイデン", 0..6),
            token("雄一郎", "ユウイチロウ", 6..15),
        ];
        p.apply(&mut t);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].reading, "ミズタ");
        assert_eq!(t[0].alternatives[0].reading, "スイデンユウイチロウ");
        assert_eq!(t[0].range, 0..6);
        assert_eq!(t[1].reading, "ユウイチロウ");
        assert_eq!(t[1].range, 6..15);
    }

    #[test]
    fn replaced_ranges_follow_the_token_start_offset() {
        let d = dict();
        // 前に別の token がある (start != 0) 時も、 姓・名の range は元の位置から続く
        let mut t = vec![
            token("の", "ノ", 0..3),
            token("水田", "スイデン", 3..9),
            token("雄一郎", "ユウイチロウ", 9..18),
            token("さん", "サン", 18..24),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        let s: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
        assert_eq!(s, ["の", "水田", "雄一郎", "さん"]);
        assert_eq!(t[1].range, 3..9);
        assert_eq!(t[2].range, 9..18);
        assert_eq!(t[3].range, 18..24);
        // 姓だけの場合
        let mut u = vec![
            token("の", "ノ", 0..3),
            token("水田", "スイデン", 3..9),
            token("選手", "センシュ", 9..15),
        ];
        PersonNamePass::new(&d).apply(&mut u);
        assert_eq!(u[1].range, 3..9);
        assert_eq!(u[1].reading, "ミズタ");
    }

    #[test]
    fn longest_run_with_given_name_wins_up_to_seven_chars() {
        let mut d = Dict::new();
        d.insert_name(true, "小野", "おの");
        d.insert_name(true, "東小野", "ひがしおの");
        d.insert_name(false, "大二郎太", "だいじろうた");
        // 東 | 小野 | 大二郎太 | さん: 7 字 (姓 3 + 名 4) まで許し、 6 字 (小野 + 大二郎太) より長いほうを採る
        let mut t = vec![
            token("東", "ヒガシ", 0..3),
            token("小野", "オノ", 3..9),
            token("大二郎太", "ダイジロウタ", 9..21),
            token("さん", "サン", 21..27),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        let s: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
        assert_eq!(s, ["東小野", "大二郎太", "さん"]);
        assert_eq!(t[0].reading, "ヒガシオノ");
        assert_eq!(t[0].range, 0..9);
        assert_eq!(t[1].range, 9..21);
        assert!(t[0].is_name && t[1].is_name);
    }

    #[test]
    fn run_longer_than_seven_chars_is_not_looked_up() {
        let mut d = Dict::new();
        d.insert_name(true, "壱弐参肆伍陸漆捌", "ななもじ");
        let mut t = vec![
            token("壱弐参肆", "イチ", 0..12),
            token("伍陸漆捌", "ゴ", 12..24),
            token("さん", "サン", 24..30),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t.len(), 3, "8 字の列は姓の表にあっても引かない");
        assert!(!t[0].is_name);
        assert_eq!(t[0].reading, "イチ");
    }

    #[test]
    fn surname_given_beats_longer_surname_only_run() {
        let mut d = Dict::new();
        d.insert_name(true, "田中", "たなか");
        d.insert_name(false, "太", "ふとし");
        d.insert_name(true, "西田中太", "にしたなかた");
        // 西田中太 全体が姓の表にあっても、 姓 + 名 が引ける 田中 | 太 を優先する (名が引けたほうが先)
        let mut t = vec![
            token("西", "ニシ", 0..3),
            token("田中", "デンチュウ", 3..9),
            token("太", "タ", 9..12),
            token("さん", "サン", 12..18),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        let s: Vec<&str> = t.iter().map(|x| x.surface.as_str()).collect();
        assert_eq!(s, ["西", "田中", "太", "さん"]);
        assert!(!t[0].is_name);
        assert_eq!(t[1].reading, "タナカ");
        assert_eq!(t[2].reading, "フトシ");
    }

    #[test]
    fn single_three_char_token_is_not_resplit_into_surname_and_given() {
        let mut d = Dict::new();
        d.insert_name(true, "北海", "ほっかい");
        d.insert_name(false, "道", "どう");
        // 北海道 を 1 語と読んだ形態素解析の結果は、 北海 | 道 に割り直さない (前に token がある位置でも)
        let mut t = vec![
            token("の", "ノ", 0..3),
            token("北海道", "ホッカイドウ", 3..12),
            token("さん", "サン", 12..18),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t.len(), 3);
        assert_eq!(t[1].surface, "北海道");
        assert_eq!(t[1].reading, "ホッカイドウ");
        assert!(!t[1].is_name);
        // 先頭にある時も同じ
        let mut u = vec![
            token("北海道", "ホッカイドウ", 0..9),
            token("さん", "サン", 9..15),
        ];
        PersonNamePass::new(&d).apply(&mut u);
        assert_eq!(u.len(), 2);
        assert_eq!(u[0].surface, "北海道");
        // 人名と言われている時 (Name) は割る
        let p = PersonNamePass::new(&d).with_hint(NameHint::Name);
        let mut n = vec![token("北海道", "ホッカイドウ", 0..9)];
        p.apply(&mut n);
        let s: Vec<&str> = n.iter().map(|x| x.surface.as_str()).collect();
        assert_eq!(s, ["北海", "道"]);
        assert_eq!(n[0].range, 0..6);
        assert_eq!(n[1].range, 6..9);
    }

    #[test]
    fn three_char_first_token_of_multi_token_name_is_still_corrected() {
        let mut d = Dict::new();
        d.insert_name(true, "左右田", "さうだ");
        d.insert_name(false, "卓", "たく");
        // 左右田 | 卓 のように 3 字の token で始まっても、 複数 token の姓 + 名 は組み直す
        let mut t = vec![
            token("左右田", "サユウデン", 0..9),
            token("卓", "スグル", 9..12),
            token("さん", "サン", 12..18),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t[0].reading, "サウダ");
        assert_eq!(t[1].reading, "タク");
        assert!(t[0].is_name && t[1].is_name);
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

    // ---- 変異テスト (cargo-mutants) の見逃しを殺すための追加 ----

    fn surfaces(t: &[Token]) -> Vec<&str> {
        t.iter().map(|x| x.surface.as_str()).collect()
    }

    fn with_band(mut t: Token, band: u32) -> Token {
        t.band = band as _;
        t
    }

    fn straddle_dict() -> Dict {
        let mut d = Dict::new();
        d.insert_name(true, "田中", "たなか");
        d.insert_name(false, "弘利", "ひろとし");
        d.insert_name(true, "三津谷", "みつや");
        d.insert_name(false, "雄一郎太", "ゆういちろうた");
        d
    }

    #[test]
    fn parse_table_header_detection_needs_all_three_conditions() {
        // 行末が ] でも [ で始まらない行は見出しではない (値に ] を含む entry)
        let c = "[entries]\n\"a\" = \"[b]\"\n\"c\" = \"d\"\n";
        assert_eq!(
            parse_name_table(c),
            vec![
                ("a".to_string(), "[b]".to_string()),
                ("c".to_string(), "d".to_string())
            ]
        );
        // 行末が ] でも ["..."] で始まる行は見出しではない (entries を抜けない)
        let c = "[entries]\n[\"x\"]\n\"c\" = \"d\"\n";
        assert_eq!(
            parse_name_table(c),
            vec![("c".to_string(), "d".to_string())]
        );
    }

    #[test]
    fn parse_table_skips_non_entry_lines_and_empty_parts() {
        // [entries] の中でも " で始まらない行は読まない
        let c = "[entries]\nkey = \"v\"\n\"a\" = \"b\"\n";
        assert_eq!(
            parse_name_table(c),
            vec![("a".to_string(), "b".to_string())]
        );
        // [entries] の外の " で始まる行は読まない
        let c = "[meta]\n\"x\" = \"y\"\n[entries]\n\"a\" = \"b\"\n";
        assert_eq!(
            parse_name_table(c),
            vec![("a".to_string(), "b".to_string())]
        );
        // 表記か読みのどちらかが空の行は捨てる
        let c = "[entries]\n\"\" = \"y\"\n\"a\" = \"\"\n\"c\" = \"d\"\n";
        assert_eq!(
            parse_name_table(c),
            vec![("c".to_string(), "d".to_string())]
        );
    }

    #[test]
    fn token_may_be_cue_is_a_precise_prefilter() {
        // 手がかりでない
        assert!(!token_may_be_cue(""));
        assert!(!token_may_be_cue("に"));
        assert!(!token_may_be_cue("水田"));
        // 敬称の後半だけの token
        assert!(token_may_be_cue("ん"));
        assert!(token_may_be_cue("ゃん"));
        // 手がかり語で始まる
        assert!(token_may_be_cue("さん"));
        assert!(token_may_be_cue("選手"));
        // 手がかり語の先頭の字で始まるだけ (敬称でも肩書きの頭でもない) では通さない
        assert!(!token_may_be_cue("さい"));
        // 敬称で終わる
        assert!(token_may_be_cue("原さん"));
        assert!(token_may_be_cue("原氏"));
        // ん で終わるだけでは通さない
        assert!(!token_may_be_cue("あん"));
    }

    #[test]
    fn may_have_cue_depends_on_tokens() {
        let none = vec![token("水田", "スイデン", 0..6), token("に", "ニ", 6..9)];
        assert!(!may_have_cue(&none));
        let some = vec![
            token("水田", "スイデン", 0..6),
            token("さん", "サン", 6..12),
        ];
        assert!(may_have_cue(&some));
    }

    #[test]
    fn parse_boundaries_of_given_name_length() {
        let mut d = Dict::new();
        d.insert_name(true, "水田", "みずた");
        d.insert_name(false, "雄", "ゆう");
        d.insert_name(false, "雄一郎太", "ゆういちろうた");
        d.insert_name(false, "雄一郎太朗", "ゆういちろうたろう");
        let p = PersonNamePass::new(&d);
        // 3 字 (姓 2 + 名 1) から姓 + 名に割る
        let r = p.parse("水田雄").unwrap();
        assert_eq!(r.sei, "水田");
        assert_eq!(r.mei.unwrap().0, "雄");
        // 名 4 字までは許す
        let r = p.parse("水田雄一郎太").unwrap();
        assert_eq!(r.sei, "水田");
        assert_eq!(r.mei.unwrap().0, "雄一郎太");
        // 名 5 字は許さない (姓だけとしても引けない)
        assert!(p.parse("水田雄一郎太朗").is_none());
    }

    #[test]
    fn dict_full_name_is_not_resplit_across_tokens() {
        let mut d = dict();
        d.insert_name(true, "江川", "えがわ");
        d.insert_name(false, "卓", "たかし");
        d.insert("江川卓", "エガワスグル");
        // 江川 | 卓 に割れていても、 辞書にフルネームがあれば名の表で組み直さない
        let mut t = vec![
            token("江川", "エガワ", 0..6),
            token("卓", "スグル", 6..9),
            token("投手", "トウシュ", 9..15),
        ];
        PersonNamePass::new(&d).apply(&mut t);
        assert_eq!(t[1].reading, "スグル");
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn straddle_repair_fixes_ranges_and_copies_from_honorific_token() {
        let d = straddle_dict();
        let mut t = vec![
            with_band(token("田中", "タナカ", 0..6), 1),
            with_band(token("弘", "ヒロ", 6..9), 2),
            with_band(token("利さ", "リサ", 9..15), 3),
            with_band(token("ん", "ン", 15..18), 7),
        ];
        PersonNamePass::new(&d).repair_kana_straddle(&mut t);
        assert_eq!(surfaces(&t), ["田中", "弘", "利", "さん"]);
        assert_eq!(t[2].reading, "リ");
        assert_eq!(t[2].range, 9..12);
        assert_eq!(t[2].band, 3);
        assert_eq!(t[3].reading, "サン");
        assert_eq!(t[3].range, 12..18);
        assert_eq!(t[3].band, 7, "右側は敬称側の token を元にする");
    }

    #[test]
    fn straddle_repair_at_index_one_and_skip_ahead() {
        let d = straddle_dict();
        // 直前の token が 1 つだけ (index 1 で当たる)
        let mut t = vec![
            token("田中", "タナカ", 0..6),
            token("弘利さ", "ヒロトシサ", 6..15),
            token("ん", "ン", 15..18),
        ];
        PersonNamePass::new(&d).repair_kana_straddle(&mut t);
        assert_eq!(surfaces(&t), ["田中", "弘利", "さん"]);
        assert_eq!(t[1].reading, "ヒロトシ");
        // 1 つ直したあと、 すぐ後ろの (2 token 先以降の) 別の箇所も直す
        let mut t = vec![
            token("を", "ヲ", 0..3),
            token("を", "ヲ", 3..6),
            token("田中", "タナカ", 6..12),
            token("弘", "ヒロ", 12..15),
            token("利さ", "リサ", 15..21),
            token("ん", "ン", 21..24),
            token("田中", "タナカ", 24..30),
            token("弘利さ", "ヒロトシサ", 30..39),
            token("ん", "ン", 39..42),
        ];
        PersonNamePass::new(&d).repair_kana_straddle(&mut t);
        assert_eq!(
            surfaces(&t),
            ["を", "を", "田中", "弘", "利", "さん", "田中", "弘利", "さん"]
        );
    }

    #[test]
    fn straddle_repair_leaves_unmatched_cases_alone() {
        let d = straddle_dict();
        // 読みが かな の部分で終わっていない
        let mut t = vec![
            token("田中", "タナカ", 0..6),
            token("弘", "ヒロ", 6..9),
            token("利さ", "トシ", 9..15),
            token("ん", "ン", 15..18),
        ];
        PersonNamePass::new(&d).repair_kana_straddle(&mut t);
        assert_eq!(surfaces(&t), ["田中", "弘", "利さ", "ん"]);
        // 表に当たらない
        let mut t = vec![
            token("森", "モリ", 0..3),
            token("利さ", "リサ", 3..9),
            token("ん", "ン", 9..12),
        ];
        PersonNamePass::new(&d).repair_kana_straddle(&mut t);
        assert_eq!(surfaces(&t), ["森", "利さ", "ん"]);
        // 次の token が敬称の続きでなければ、 かなの頭が敬称の頭と違っていても直さない
        let mut t = vec![
            token("田中", "タナカ", 0..6),
            token("弘", "ヒロ", 6..9),
            token("利あ", "リア", 9..15),
            token("ん", "ン", 15..18),
        ];
        PersonNamePass::new(&d).repair_kana_straddle(&mut t);
        assert_eq!(surfaces(&t), ["田中", "弘", "利あ", "ん"]);
        // 敬称が丸ごと token に入っている時 (続きの token は空) は 「またぎ」 ではない
        let mut t = vec![
            token("田中", "タナカ", 0..6),
            token("弘", "ヒロ", 6..9),
            token("利さん", "リサン", 9..18),
            token("", "", 18..18),
        ];
        PersonNamePass::new(&d).repair_kana_straddle(&mut t);
        assert_eq!(surfaces(&t), ["田中", "弘", "利さん", ""]);
    }

    #[test]
    fn straddle_repair_allows_exactly_max_run_chars() {
        let d = straddle_dict();
        // 三津谷 (3) + 雄一郎 + 太 = 7 字 (上限ちょうど) は引く
        let mut t = vec![
            token("三津谷", "ミツヤ", 0..9),
            token("雄一郎", "ユウイチロウ", 9..18),
            token("太さ", "タサ", 18..24),
            token("ん", "ン", 24..27),
        ];
        PersonNamePass::new(&d).repair_kana_straddle(&mut t);
        assert_eq!(surfaces(&t), ["三津谷", "雄一郎", "太", "さん"]);
    }

    #[test]
    fn merged_honorific_split_fixes_ranges() {
        let d = straddle_dict();
        let mut t = vec![
            with_band(token("田中", "タナカ", 0..6), 1),
            with_band(token("弘", "ヒロ", 6..9), 2),
            with_band(token("利さん", "リサン", 9..18), 3),
        ];
        PersonNamePass::new(&d).split_merged_honorific(&mut t);
        assert_eq!(surfaces(&t), ["田中", "弘", "利", "さん"]);
        assert_eq!(t[2].reading, "リ");
        assert_eq!(t[2].range, 9..12);
        assert_eq!(t[3].reading, "サン");
        assert_eq!(t[3].range, 12..18);
    }

    #[test]
    fn merged_honorific_split_continues_past_a_split() {
        let d = straddle_dict();
        let mut t = vec![
            token("を", "ヲ", 0..3),
            token("を", "ヲ", 3..6),
            token("田中", "タナカ", 6..12),
            token("弘", "ヒロ", 12..15),
            token("利さん", "リサン", 15..24),
            token("田中", "タナカ", 24..30),
            token("弘利さん", "ヒロトシサン", 30..42),
        ];
        PersonNamePass::new(&d).split_merged_honorific(&mut t);
        assert_eq!(
            surfaces(&t),
            ["を", "を", "田中", "弘", "利", "さん", "田中", "弘利", "さん"]
        );
    }

    #[test]
    fn merged_honorific_split_skips_when_reading_or_table_mismatch() {
        let d = straddle_dict();
        // 読みが 敬称で終わっていない
        let mut t = vec![
            token("田中", "タナカ", 0..6),
            token("弘", "ヒロ", 6..9),
            token("利さん", "トシ", 9..18),
        ];
        PersonNamePass::new(&d).split_merged_honorific(&mut t);
        assert_eq!(surfaces(&t), ["田中", "弘", "利さん"]);
        // 表に当たらない
        let mut t = vec![token("森", "モリ", 0..3), token("利さん", "リサン", 3..12)];
        PersonNamePass::new(&d).split_merged_honorific(&mut t);
        assert_eq!(surfaces(&t), ["森", "利さん"]);
    }

    #[test]
    fn merged_honorific_split_allows_exactly_max_run_chars() {
        let d = straddle_dict();
        // 三津谷 (3) + 雄一郎 + 太 = 7 字 (上限ちょうど) は引く
        let mut t = vec![
            token("三津谷", "ミツヤ", 0..9),
            token("雄一郎", "ユウイチロウ", 9..18),
            token("太さん", "タサン", 18..27),
        ];
        PersonNamePass::new(&d).split_merged_honorific(&mut t);
        assert_eq!(surfaces(&t), ["三津谷", "雄一郎", "太", "さん"]);
    }
}

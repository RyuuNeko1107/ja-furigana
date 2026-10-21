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

mod cues;

use self::cues::cue_score;
use self::cues::may_have_cue;
use self::cues::title_continues_into_compound;
use self::cues::ALT_WEIGHT;
use self::cues::COMMON_WORD_GIVEN;
use self::cues::HONORIFICS;
use self::cues::MAX_RUN_CHARS;
use self::cues::MAX_RUN_TOKENS;
use crate::dict::Dict;
use crate::kana::{hira_to_kata, is_kanji_char};
use crate::scoring::analyze::{AlternativeReading, Token};
use crate::scoring::postpass::ReadingPostPass;

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
                _ => {
                    c < tokens.len()
                        && cue_score(&tokens[c].surface) > 0
                        && !title_continues_into_compound(tokens, c)
                }
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
            // 名 (後ろ) が、 実コメントで姓 + 名 と誤認された一般語 (今日新人さん / ドイツ国初代大統領 / 尚勝利投手) なら、
            // 割り直さない (新人さん = あらと にしない)。 一般語全般を除くと 風花 / 蘭子 のような本当の名前を巻き込むので、 実測した語だけ
            if self.hint != NameHint::Name
                && p.mei
                    .as_ref()
                    .is_some_and(|(m, _)| COMMON_WORD_GIVEN.contains(&m.as_str()))
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
mod tests;
